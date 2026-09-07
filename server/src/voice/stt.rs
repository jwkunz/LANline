//! Speech-to-text: pure-Rust Whisper (candle). A worker thread receives one
//! finished over at a time, resamples 48 kHz → 16 kHz, runs a single 30 s
//! Whisper window (greedy decode, English `.en` models), and appends the text
//! to the transcript ring.

use super::{Segment, RING_CAP};
use crate::model::TranscriptEntry;
use crate::radio::dsp::LinearResampler;
use anyhow::{bail, Context, Result};
use candle_core::{Device, IndexOp, Tensor};
use candle_nn::VarBuilder;
use candle_transformers::models::whisper::{self as m, audio, Config};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use tokenizers::Tokenizer;
use tokio::sync::broadcast;

const WHISPER_SR: usize = 16_000;

pub(super) fn spawn_worker(
    cfg: &crate::config::Config,
    tx: broadcast::Sender<TranscriptEntry>,
    ring: Arc<Mutex<VecDeque<TranscriptEntry>>>,
    transcribing: Arc<AtomicBool>,
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
            while let Ok(seg) = seg_rx.recv() {
                transcribing.store(true, Ordering::Relaxed);
                let t0 = std::time::Instant::now();
                match engine.transcribe(&seg.pcm) {
                    Ok(text) if !text.is_empty() => {
                        tracing::info!(
                            "stt: [{:.1}s over, {:.0} dBFS] \"{text}\" ({:.1}s decode)",
                            seg.secs,
                            seg.rssi_dbfs,
                            t0.elapsed().as_secs_f32()
                        );
                        let entry = TranscriptEntry {
                            time: time::OffsetDateTime::now_utc(),
                            text,
                            rssi_dbfs: seg.rssi_dbfs,
                            secs: seg.secs,
                        };
                        {
                            let mut r = ring.lock().unwrap();
                            if r.len() >= RING_CAP {
                                r.pop_front();
                            }
                            r.push_back(entry.clone());
                        }
                        let _ = tx.send(entry);
                    }
                    Ok(_) => tracing::debug!("stt: empty transcript for a {:.1}s over", seg.secs),
                    Err(e) => tracing::warn!("stt: transcribe failed: {e:#}"),
                }
                transcribing.store(false, Ordering::Relaxed);
            }
            tracing::info!("stt: worker stopped");
        })
        .context("spawning the STT worker thread")?;

    Ok(seg_tx)
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
    lpf_a: f32,
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

        // Anti-alias pre-filter for the 48 k → 16 k decimation (1-pole, ~7 kHz).
        let lpf_a = 1.0 - (-2.0 * std::f32::consts::PI * 7_000.0 / 48_000.0).exp();

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
            lpf_a,
        })
    }

    /// One over (48 kHz mono) → text. Pads/cuts to a single 30 s Whisper window.
    fn transcribe(&mut self, pcm48: &[f32]) -> Result<String> {
        // Pre-filter, then linear resample to 16 kHz.
        let mut y = 0.0f32;
        let filtered: Vec<f32> = pcm48
            .iter()
            .map(|&x| {
                y += self.lpf_a * (x - y);
                y
            })
            .collect();
        let mut pcm = Vec::with_capacity(WHISPER_SR * 30);
        LinearResampler::new(48_000.0, WHISPER_SR as f64).process(&filtered, &mut pcm);
        pcm.resize(WHISPER_SR * 30, 0.0); // exactly N_FRAMES worth

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
        }

        let text = self
            .tokenizer
            .decode(&tokens, true)
            .map_err(|e| anyhow::anyhow!("detokenize: {e}"))?;
        Ok(clean(&text))
    }
}

/// Whisper hallucinates canned phrases on near-silence; drop the usual ones.
fn clean(text: &str) -> String {
    let t = text.trim();
    let low = t.to_lowercase();
    const JUNK: &[&str] = &[
        "",
        "you",
        "thank you.",
        "thanks for watching!",
        "thank you for watching.",
        "(buzzing)",
        ".",
        "bye.",
    ];
    if JUNK.contains(&low.as_str()) {
        return String::new();
    }
    t.to_string()
}
