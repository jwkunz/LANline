//! Speech-to-text: pure-Rust Whisper (candle). One worker thread, two feeds:
//! a `Segment` mpsc for finished PTT overs, and — polled between — a rolling
//! `AudioRing` for the continuous carriers, from which it pulls overlapping
//! ~26 s windows and stitches the text together. Greedy decode with a
//! repetition-loop guard, English `.en` models.

use super::{repetition_cycle_len, stitch, AudioRing, Segment, RING_CAP};
use crate::model::TranscriptEntry;
use anyhow::{bail, Context, Result};
use candle_core::{Device, IndexOp, Tensor};
use candle_nn::VarBuilder;
use candle_transformers::models::whisper::{self as m, audio, Config};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokenizers::Tokenizer;
use tokio::sync::broadcast;

const WHISPER_SR: usize = 16_000;

pub(super) fn spawn_worker(
    cfg: &crate::config::Config,
    tx: broadcast::Sender<TranscriptEntry>,
    ring: Arc<Mutex<VecDeque<TranscriptEntry>>>,
    transcribing: Arc<AtomicBool>,
    audio: Arc<Mutex<AudioRing>>,
) -> Result<mpsc::Sender<Segment>> {
    let dir = cfg
        .stt_model
        .clone()
        .context("--stt needs --stt-model <dir> (config.json / tokenizer.json / model.safetensors)")?;
    let engine = Engine::load(&dir)?;
    tracing::info!("stt: loaded Whisper model from {}", dir.display());

    let (seg_tx, seg_rx) = mpsc::channel::<Segment>();
    std::thread::Builder::new()
        .name("stt-whisper".into())
        .spawn(move || {
            let mut engine = engine;
            loop {
                match seg_rx.recv_timeout(Duration::from_millis(500)) {
                    Ok(seg) => {
                        transcribing.store(true, Ordering::Relaxed);
                        let t0 = Instant::now();
                        match engine.transcribe(&seg.pcm16) {
                            Ok(text) if !text.is_empty() => {
                                tracing::info!(
                                    "stt: [{:.1}s over, {:.0} dBFS] \"{text}\" ({:.1}s decode)",
                                    seg.secs,
                                    seg.rssi_dbfs,
                                    t0.elapsed().as_secs_f32()
                                );
                                push_entry(&ring, &tx, text, seg.rssi_dbfs, seg.secs);
                            }
                            Ok(_) => {
                                tracing::debug!("stt: empty transcript for a {:.1}s over", seg.secs)
                            }
                            Err(e) => tracing::warn!("stt: transcribe failed: {e:#}"),
                        }
                        transcribing.store(false, Ordering::Relaxed);
                    }
                    Err(RecvTimeoutError::Timeout) => {
                        let Some(win) = audio.lock().unwrap().take_window() else {
                            continue;
                        };
                        transcribing.store(true, Ordering::Relaxed);
                        let t0 = Instant::now();
                        match engine.transcribe(&win.pcm16) {
                            Ok(text) if !text.is_empty() => {
                                let (delta, new_tail) =
                                    stitch(&win.tail_words, &text, win.overlap_secs);
                                let delta = delta.trim().to_string();
                                // Discard if a clear / mode switch happened
                                // while this window was decoding.
                                let mut a = audio.lock().unwrap();
                                if a.generation == win.generation {
                                    a.note_commit();
                                    if !delta.is_empty() {
                                        a.tail_words = new_tail;
                                    }
                                    drop(a);
                                    if !delta.is_empty() {
                                        tracing::info!(
                                            "stt: window +{:.0}s (overlap {:.0}s, {:.0} dBFS) \"{delta}\" ({:.1}s decode)",
                                            win.new_secs,
                                            win.overlap_secs,
                                            win.rssi_dbfs,
                                            t0.elapsed().as_secs_f32()
                                        );
                                        push_entry(
                                            &ring,
                                            &tx,
                                            delta,
                                            win.rssi_dbfs,
                                            win.new_secs,
                                        );
                                    }
                                }
                            }
                            Ok(_) => {
                                tracing::debug!("stt: empty window ({:.0}s) — retrying span", win.new_secs);
                                audio.lock().unwrap().retry_empty(win.generation, win.new_secs);
                            }
                            Err(e) => tracing::warn!("stt: window transcribe failed: {e:#}"),
                        }
                        transcribing.store(false, Ordering::Relaxed);
                    }
                    Err(RecvTimeoutError::Disconnected) => break,
                }
            }
            tracing::info!("stt: worker stopped");
        })
        .context("spawning the STT worker thread")?;

    Ok(seg_tx)
}

fn push_entry(
    ring: &Arc<Mutex<VecDeque<TranscriptEntry>>>,
    tx: &broadcast::Sender<TranscriptEntry>,
    text: String,
    rssi_dbfs: f32,
    secs: f32,
) {
    let entry = TranscriptEntry { time: time::OffsetDateTime::now_utc(), text, rssi_dbfs, secs };
    {
        let mut r = ring.lock().unwrap();
        if r.len() >= RING_CAP {
            r.pop_front();
        }
        r.push_back(entry.clone());
    }
    let _ = tx.send(entry);
}

struct Engine {
    device: Device,
    config: Config,
    model: m::model::Whisper,
    tokenizer: Tokenizer,
    mel_filters: Vec<f32>,
    suppress: Tensor,
    sot: u32,
    transcribe: u32,
    eot: u32,
    no_ts: u32,
}

impl Engine {
    fn load(dir: &std::path::Path) -> Result<Self> {
        let device = Device::Cpu;
        let config: Config = serde_json::from_str(
            &std::fs::read_to_string(dir.join("config.json")).context("reading config.json")?,
        )
        .context("parsing config.json")?;

        let tokenizer = Tokenizer::from_file(dir.join("tokenizer.json"))
            .map_err(|e| anyhow::anyhow!("tokenizer.json: {e}"))?;

        let weights = dir.join("model.safetensors");
        if !weights.is_file() {
            bail!("{} not found", weights.display());
        }
        let vb = unsafe { VarBuilder::from_mmaped_safetensors(&[weights], m::DTYPE, &device)? };
        let model = m::model::Whisper::load(&vb, config.clone())?;

        // 80-bin mel filterbank, little-endian f32.
        let raw: &[u8] = include_bytes!("melfilters.bytes");
        let mel_filters: Vec<f32> = raw
            .chunks_exact(4)
            .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
            .collect();

        let tok = |t: &str| -> Result<u32> {
            tokenizer.token_to_id(t).with_context(|| format!("token {t} missing from tokenizer"))
        };
        let sot = tok(m::SOT_TOKEN)?;
        let transcribe = tok(m::TRANSCRIBE_TOKEN)?;
        let eot = tok(m::EOT_TOKEN)?;
        let no_ts = tok(m::NO_TIMESTAMPS_TOKEN)?;

        let suppress: Vec<f32> = (0..config.vocab_size as u32)
            .map(|i| if config.suppress_tokens.contains(&i) { f32::NEG_INFINITY } else { 0.0 })
            .collect();
        let suppress = Tensor::new(suppress.as_slice(), &device)?;

        Ok(Self {
            device,
            config,
            model,
            tokenizer,
            mel_filters,
            suppress,
            sot,
            transcribe,
            eot,
            no_ts,
        })
    }

    /// One 16 kHz mono clip (already LPF'd + resampled by the feed side) →
    /// text. Padded/cut to a single 30 s Whisper window.
    fn transcribe(&mut self, pcm16: &[f32]) -> Result<String> {
        let audio_secs = (pcm16.len() as f32 / WHISPER_SR as f32).max(0.1);
        let mut pcm = pcm16.to_vec();
        pcm.resize(WHISPER_SR * 30, 0.0); // exactly N_FRAMES worth
        pcm.truncate(WHISPER_SR * 30);

        let mel = audio::pcm_to_mel(&self.config, &pcm, &self.mel_filters);
        let n_mels = self.config.num_mel_bins;
        let n_len = mel.len() / n_mels;
        // `pcm_to_mel` always pads by an extra 1500 frames, so a 30 s window
        // comes back as 4500 frames. Whisper's encoder positional table is
        // `2 * max_source_positions` (3000) — feed it exactly that, trimming
        // the pad. (The upstream example instead slides a 3000-frame window;
        // one window is all we need for a single 30 s chunk.)
        let want = (2 * self.config.max_source_positions).min(n_len);
        let mel = if want == n_len {
            mel
        } else {
            let mut trimmed = Vec::with_capacity(n_mels * want);
            for j in 0..n_mels {
                trimmed.extend_from_slice(&mel[j * n_len..j * n_len + want]);
            }
            trimmed
        };
        let mel = Tensor::from_vec(mel, (1, n_mels, want), &self.device)?;

        let audio_features = self.model.encoder.forward(&mel, true)?;

        let mut tokens: Vec<u32> = vec![self.sot, self.transcribe, self.no_ts];
        let sample_len = self.config.max_target_positions / 2;
        for i in 0..sample_len {
            let tokens_t = Tensor::new(tokens.as_slice(), &self.device)?.unsqueeze(0)?;
            let ys = self.model.decoder.forward(&tokens_t, &audio_features, i == 0)?;
            let (_, seq_len, _) = ys.dims3()?;
            let logits = self
                .model
                .decoder
                .final_linear(&ys.i((..1, seq_len - 1..))?)?
                .i(0)?
                .i(0)?
                .broadcast_add(&self.suppress)?;
            let v: Vec<f32> = logits.to_vec1()?;
            let next = v
                .iter()
                .enumerate()
                .max_by(|(_, a), (_, b)| a.total_cmp(b))
                .map(|(i, _)| i as u32)
                .unwrap_or(self.eot);
            tokens.push(next);
            if next == self.eot || tokens.len() > self.config.max_target_positions {
                break;
            }
            // Greedy decode sometimes collapses into a loop ("the south coast
            // of the south coast of…"). Cut it and keep one copy of the cycle.
            if let Some(l) = repetition_cycle_len(&tokens) {
                tokens.truncate(tokens.len() - 2 * l);
                break;
            }
        }

        let text = self
            .tokenizer
            .decode(&tokens, true)
            .map_err(|e| anyhow::anyhow!("detokenize: {e}"))?;
        let text = clean(&text);
        if text.is_empty() {
            return Ok(text);
        }
        // Discard obvious garbage — a low unique-word ratio (residual looping)
        // or an implausible speaking rate. The window's GUARD re-read gives
        // the audio another pass with a different boundary.
        let ws: Vec<String> = text.split_whitespace().map(|w| w.to_lowercase()).collect();
        if ws.len() > 12 {
            let uniq = ws.iter().collect::<std::collections::HashSet<_>>().len();
            if (uniq as f32) / (ws.len() as f32) < 0.35 {
                tracing::debug!("stt: dropped a low-diversity decode: \"{text}\"");
                return Ok(String::new());
            }
        }
        if text.chars().count() as f32 / audio_secs > 25.0 {
            tracing::debug!("stt: dropped an implausibly dense decode ({audio_secs:.0}s)");
            return Ok(String::new());
        }
        Ok(text)
    }
}

/// Whisper emits junk on non-speech audio (silence between products, the SAME
/// data burst / warning-alarm tone): canned hallucinations, a lone stop-word,
/// or a bare number. Drop those so they don't land in the transcript.
fn clean(text: &str) -> String {
    let t = text.trim();
    let low = t.to_lowercase();
    const JUNK: &[&str] = &[
        "",
        "you",
        "thank you.",
        "thanks for watching!",
        "thank you for watching.",
        "thanks for watching.",
        "(buzzing)",
        ".",
        "...",
        "bye.",
        "$1.00",
    ];
    if JUNK.contains(&low.as_str()) {
        return String::new();
    }
    // No letters at all — a bare "$1.00", "72.", "- -".
    if !t.chars().any(|c| c.is_alphabetic()) {
        return String::new();
    }
    // Whisper was trained on subtitles and spits their markup ("{\an2}",
    // "{\fonttbl…}") or a stray price ("$10.00 per hour") on non-speech
    // audio — the SAME data burst, the warning-alarm tone, dead air. None of
    // those characters occur in a spoken weather bulletin.
    if t.contains(['$', '{', '}', '\\', '|']) {
        return String::new();
    }
    // A one- or two-word output made only of stop-words is a decode that
    // gave up on the first token ("The", "The the", "a and").
    let ws: Vec<String> = t
        .split_whitespace()
        .map(|w| w.trim_matches(|c: char| !c.is_alphanumeric()).to_lowercase())
        .collect();
    if ws.len() <= 2
        && ws.iter().all(|w| {
            matches!(
                w.as_str(),
                "" | "the" | "a" | "an" | "and" | "of" | "to" | "in" | "is" | "it" | "for" | "so"
            )
        })
    {
        return String::new();
    }
    t.to_string()
}

#[cfg(test)]
mod tests {
    use super::clean;

    #[test]
    fn clean_drops_non_speech_junk() {
        for j in [
            "The",
            "the the",
            "$1.00",
            "72.",
            "  ...  ",
            "thank you.",
            r"This {\an2{\fonttbl{\f1",
            "$10.00 per hour",
        ] {
            assert_eq!(clean(j), "", "{j:?} should be dropped");
        }
    }

    #[test]
    fn clean_keeps_real_text() {
        let s = "The temperature was 78 degrees.";
        assert_eq!(clean(s), s);
        assert_eq!(clean("  A trace of rain fell yesterday.  "), "A trace of rain fell yesterday.");
    }
}
