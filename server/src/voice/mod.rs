//! Voice ↔ text for the FM/AM modes.
//!
//! * TX — `POST /radio/tx/say` synthesizes text to speech (`tts.rs`, feature
//!   `tts`: shells `espeak-ng` or `piper`) and feeds it into the existing
//!   `TxAudioSource` / `key_tx` path.
//! * RX — feature `stt`, pure-Rust Whisper worker (`stt.rs`). Two segmentation
//!   strategies:
//!   - **PTT modes** (`frs` / `ham`): squelch-gated — each over (open → close)
//!     is one segment, transcribed on close, one transcript entry per over.
//!   - **Continuous modes** (`nbfm` / `wbfm` / `am`): the NOAA-weather-radio
//!     carrier never drops the squelch, so per-over segmentation is
//!     meaningless. Audio streams into a rolling `AudioRing`; the worker pulls
//!     overlapping ~26 s windows, transcribes each, and stitches the text onto
//!     the running transcript (dropping the re-read overlap). `RadioManager`
//!     flips the mode with `set_continuous`.
//!
//! Text lands in a ring served at `GET /radio/transcript`. `VoiceShared` is
//! always compiled so the pipeline / API wiring is unconditional — it is
//! simply inert when neither feature is built or the matching flag is off.

#[cfg(feature = "stt")]
pub mod stt;
#[cfg(feature = "tts")]
pub mod tts;

use crate::model::TranscriptEntry;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tokio::sync::broadcast;

#[cfg(feature = "stt")]
const RING_CAP: usize = 200;
/// Demod audio rate (input to `stt_feed`).
#[cfg(feature = "stt")]
const SR: usize = 48_000;
/// Whisper's rate — the ring stores audio already resampled to this.
#[cfg(feature = "stt")]
const SR16: usize = 16_000;

// --- PTT (squelch-gated) segmentation ---------------------------------
/// Shortest over worth transcribing.
#[cfg(feature = "stt")]
const MIN_SEG: usize = SR * 2 / 5; // 0.4 s
/// Closed-squelch run that ends a segment.
#[cfg(feature = "stt")]
const HANG: usize = SR * 3 / 10; // 0.3 s
/// Whisper's native window — a single over longer than this is cut here.
#[cfg(feature = "stt")]
const MAX_SEG: usize = SR * 30;

// --- continuous (rolling-window) transcription ------------------------
/// Rolling PCM buffer depth (16 kHz). Deep enough that a slow decode can
/// fall a few windows behind without losing audio.
#[cfg(feature = "stt")]
const RING_CAP16: usize = 45 * SR16;
/// Don't open a new window until at least this much fresh audio has arrived.
#[cfg(feature = "stt")]
const MIN_NEW16: usize = 8 * SR16;
/// Re-read this much already-committed audio at the head of every window so
/// Whisper has lead-in context (trimmed back out by `stitch`).
#[cfg(feature = "stt")]
const LEAD16: usize = 5 * SR16;
/// Longest window handed to Whisper (must stay ≤ its 30 s frame).
#[cfg(feature = "stt")]
const MAX_WIN16: usize = 26 * SR16;
/// Leave this much of the tail un-committed each window: the next window
/// re-reads it so a word mid-utterance at the cut gets one clean pass.
#[cfg(feature = "stt")]
const GUARD16: usize = 2 * SR16;

/// A finished PTT over handed to the STT worker (16 kHz mono, pre-filtered).
#[cfg(feature = "stt")]
pub(crate) struct Segment {
    pub pcm16: Vec<f32>,
    pub rssi_dbfs: f32,
    pub secs: f32,
}

/// One overlapping window pulled from the continuous ring.
#[cfg(feature = "stt")]
pub(crate) struct Window {
    pub pcm16: Vec<f32>,
    /// Seconds of already-committed audio re-read at the front (for `stitch`).
    pub overlap_secs: f32,
    /// Seconds of genuinely new audio this window commits.
    pub new_secs: f32,
    pub rssi_dbfs: f32,
    /// Bumped by `reset` — a window whose generation no longer matches when
    /// the worker goes to commit it is discarded (a clear / mode switch
    /// happened mid-decode).
    pub generation: u64,
    /// Snapshot of the running tail, for `stitch`.
    pub tail_words: Vec<String>,
}

#[cfg(feature = "stt")]
#[derive(Default)]
struct SegAccum {
    buf: Vec<f32>, // 48 kHz
    rssi_sum: f64,
    rssi_n: u64,
    closed_run: usize,
}

#[cfg(feature = "stt")]
struct AudioRing {
    buf: VecDeque<f32>, // 16 kHz mono, LPF'd
    /// Total samples ever pushed (window positions live in this space).
    head: u64,
    /// Everything up to here has been turned into committed transcript text.
    committed: u64,
    active: bool,
    generation: u64,
    rssi_ema: f32,
    lpf_y: f32,
    resampler: crate::radio::dsp::LinearResampler,
    tail_words: Vec<String>,
    /// Samples skipped (never transcribed) because a decode fell too far
    /// behind — surfaced in a log warning.
    behind: u64,
    /// How many times the current span has been re-tried after an empty
    /// decode (bounded, so a genuinely dead stretch doesn't loop).
    retries: u8,
}

#[cfg(feature = "stt")]
impl AudioRing {
    fn new() -> Self {
        AudioRing {
            buf: VecDeque::with_capacity(RING_CAP16 + SR16),
            head: 0,
            committed: 0,
            active: false,
            generation: 0,
            rssi_ema: -120.0,
            lpf_y: 0.0,
            resampler: crate::radio::dsp::LinearResampler::new(SR as f64, SR16 as f64),
            tail_words: Vec::new(),
            behind: 0,
            retries: 0,
        }
    }

    fn reset(&mut self) {
        self.buf.clear();
        self.head = 0;
        self.committed = 0;
        self.generation = self.generation.wrapping_add(1);
        self.rssi_ema = -120.0;
        self.lpf_y = 0.0;
        self.resampler = crate::radio::dsp::LinearResampler::new(SR as f64, SR16 as f64);
        self.tail_words.clear();
        self.behind = 0;
        self.retries = 0;
    }

    /// The last window came back empty (Whisper fumbled a section transition
    /// or a pause). Roll `committed` back so the next, larger window re-reads
    /// that span with a different boundary — at most once, then accept the
    /// loss so a truly non-speech stretch can't stall forever.
    fn retry_empty(&mut self, generation: u64, new_secs: f32) {
        if self.generation != generation || self.retries >= 1 {
            self.retries = 0;
            return;
        }
        self.retries += 1;
        let keep = MIN_NEW16 as f32 / SR16 as f32;
        let back = ((new_secs - keep).max(0.0) * SR16 as f32) as u64;
        self.committed = self.committed.saturating_sub(back).max(self.oldest());
    }

    fn note_commit(&mut self) {
        self.retries = 0;
    }

    fn oldest(&self) -> u64 {
        self.head - self.buf.len() as u64
    }

    /// Feed one demod frame (48 kHz). LPF + resample to 16 kHz, append, evict.
    fn push(&mut self, pcm48: &[f32], rssi_dbfs: f32) {
        // 1-pole ~7 kHz anti-alias for the 3:1 decimation.
        let a = 1.0 - (-2.0 * std::f32::consts::PI * 7_000.0 / SR as f32).exp();
        let mut lp = Vec::with_capacity(pcm48.len());
        for &x in pcm48 {
            self.lpf_y += a * (x - self.lpf_y);
            lp.push(self.lpf_y);
        }
        let mut out = Vec::with_capacity(pcm48.len() / 3 + 2);
        self.resampler.process(&lp, &mut out);
        self.head += out.len() as u64;
        self.buf.extend(out);
        while self.buf.len() > RING_CAP16 {
            self.buf.pop_front();
        }
        self.rssi_ema += 0.05 * (rssi_dbfs - self.rssi_ema);
    }

    /// Next window to transcribe, or `None` if not enough new audio yet.
    fn take_window(&mut self) -> Option<Window> {
        if !self.active {
            return None;
        }
        if self.head - self.committed < MIN_NEW16 as u64 {
            return None;
        }
        let win_end = self.head;
        let mut win_start = self.committed.saturating_sub(LEAD16 as u64);
        if win_end - win_start > MAX_WIN16 as u64 {
            let skip = (win_end - win_start) - MAX_WIN16 as u64;
            win_start = win_end - MAX_WIN16 as u64;
            self.behind += skip;
        }
        let oldest = self.oldest();
        if win_start < oldest {
            self.behind += oldest - win_start;
            win_start = oldest;
        }
        if self.behind > 0 {
            tracing::warn!(
                "stt: decode is behind — {:.1} s of audio skipped from the transcript",
                self.behind as f32 / SR16 as f32
            );
            self.behind = 0;
        }
        let s0 = (win_start - oldest) as usize;
        let s1 = (win_end - oldest) as usize;
        let pcm16: Vec<f32> = self.buf.iter().copied().skip(s0).take(s1 - s0).collect();
        let overlap_secs = self.committed.saturating_sub(win_start) as f32 / SR16 as f32;
        let new_secs = (win_end - self.committed) as f32 / SR16 as f32;
        self.committed = win_end.saturating_sub(GUARD16 as u64);
        Some(Window {
            pcm16,
            overlap_secs,
            new_secs,
            rssi_dbfs: self.rssi_ema,
            generation: self.generation,
            tail_words: self.tail_words.clone(),
        })
    }
}

pub struct VoiceShared {
    tts: bool,
    stt: bool,
    transcript: Arc<Mutex<VecDeque<TranscriptEntry>>>,
    tx: broadcast::Sender<TranscriptEntry>,
    transcribing: Arc<AtomicBool>,
    /// Dispatch flag for `stt_feed` (no lock on the hot path).
    #[cfg(feature = "stt")]
    continuous: AtomicBool,
    #[cfg(feature = "stt")]
    seg: Mutex<SegAccum>,
    #[cfg(feature = "stt")]
    audio: Arc<Mutex<AudioRing>>,
    #[cfg(feature = "stt")]
    stt_tx: Option<std::sync::mpsc::Sender<Segment>>,
}

impl VoiceShared {
    pub fn new(cfg: &crate::config::Config) -> Arc<Self> {
        let _ = cfg;
        let transcript = Arc::new(Mutex::new(VecDeque::new()));
        let transcribing = Arc::new(AtomicBool::new(false));
        let (tx, _) = broadcast::channel(64);
        #[cfg(feature = "stt")]
        let audio = Arc::new(Mutex::new(AudioRing::new()));

        #[cfg(feature = "stt")]
        let stt_tx = if cfg.stt {
            match stt::spawn_worker(
                cfg,
                tx.clone(),
                transcript.clone(),
                transcribing.clone(),
                audio.clone(),
            ) {
                Ok(s) => {
                    tracing::info!("stt: Whisper worker ready");
                    Some(s)
                }
                Err(e) => {
                    tracing::error!("stt: disabled — {e:#}");
                    None
                }
            }
        } else {
            None
        };

        #[cfg(feature = "stt")]
        let stt_on = cfg.stt && stt_tx.is_some();
        #[cfg(not(feature = "stt"))]
        let stt_on = false;

        #[cfg(feature = "tts")]
        let tts_on = cfg.tts;
        #[cfg(not(feature = "tts"))]
        let tts_on = false;

        Arc::new(Self {
            tts: tts_on,
            stt: stt_on,
            transcript,
            tx,
            transcribing,
            #[cfg(feature = "stt")]
            continuous: AtomicBool::new(false),
            #[cfg(feature = "stt")]
            seg: Mutex::new(SegAccum::default()),
            #[cfg(feature = "stt")]
            audio,
            #[cfg(feature = "stt")]
            stt_tx,
        })
    }

    pub fn tts_enabled(&self) -> bool {
        self.tts
    }
    pub fn stt_enabled(&self) -> bool {
        self.stt
    }
    pub fn transcribing(&self) -> bool {
        self.transcribing.load(Ordering::Relaxed)
    }
    pub fn subscribe(&self) -> broadcast::Receiver<TranscriptEntry> {
        self.tx.subscribe()
    }
    pub fn transcript(&self) -> Vec<TranscriptEntry> {
        self.transcript.lock().unwrap().iter().cloned().collect()
    }

    /// The whole ring as one flowing block of text (for the `.txt` download).
    pub fn transcript_text(&self) -> String {
        let ring = self.transcript.lock().unwrap();
        let mut out = ring
            .iter()
            .map(|e| e.text.trim())
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join(" ");
        out.push('\n');
        out
    }

    /// Switch RX segmentation strategy for the running mode. `true` for the
    /// continuous carriers (`nbfm` / `wbfm` / `am`); `false` (default) for the
    /// squelch-gated PTT modes. Resets the rolling buffer either way.
    pub fn set_continuous(&self, on: bool) {
        #[cfg(feature = "stt")]
        {
            self.continuous.store(on, Ordering::Relaxed);
            let mut a = self.audio.lock().unwrap();
            a.reset();
            a.active = on;
        }
        #[cfg(not(feature = "stt"))]
        {
            let _ = on;
        }
    }

    /// Empty the transcript ring and drop any buffered / half-decoded audio.
    /// Backs the web "Clear" button (`POST /radio/transcript/clear`).
    pub fn clear_transcript(&self) {
        self.transcript.lock().unwrap().clear();
        #[cfg(feature = "stt")]
        {
            *self.seg.lock().unwrap() = SegAccum::default();
            let mut a = self.audio.lock().unwrap();
            let was = a.active;
            a.reset();
            a.active = was;
        }
    }

    /// Called every audio frame from `run_sdr`. In a continuous mode the frame
    /// goes into the rolling ring; otherwise it feeds the squelch-gated
    /// accumulator. No-op unless STT is active.
    pub fn stt_feed(&self, pcm: &[f32], squelch_open: bool, rssi_dbfs: f32) {
        #[cfg(feature = "stt")]
        {
            let Some(tx) = &self.stt_tx else {
                return;
            };
            if self.continuous.load(Ordering::Relaxed) {
                self.audio.lock().unwrap().push(pcm, rssi_dbfs);
                return;
            }
            let mut s = self.seg.lock().unwrap();
            if squelch_open {
                s.closed_run = 0;
                s.buf.extend_from_slice(pcm);
                s.rssi_sum += rssi_dbfs as f64;
                s.rssi_n += 1;
                if s.buf.len() >= MAX_SEG {
                    flush_seg(&mut s, tx);
                }
            } else if !s.buf.is_empty() {
                s.closed_run += pcm.len();
                if s.closed_run >= HANG {
                    flush_seg(&mut s, tx);
                }
            }
        }
        #[cfg(not(feature = "stt"))]
        {
            let _ = (pcm, squelch_open, rssi_dbfs);
        }
    }
}

/// Ship a finished PTT over: resample 48 k → 16 k and hand it to the worker.
#[cfg(feature = "stt")]
fn flush_seg(s: &mut SegAccum, tx: &std::sync::mpsc::Sender<Segment>) {
    let pcm48 = std::mem::take(&mut s.buf);
    let rssi = if s.rssi_n > 0 { (s.rssi_sum / s.rssi_n as f64) as f32 } else { -120.0 };
    s.rssi_sum = 0.0;
    s.rssi_n = 0;
    s.closed_run = 0;
    if pcm48.len() < MIN_SEG {
        return;
    }
    let a = 1.0 - (-2.0 * std::f32::consts::PI * 7_000.0 / SR as f32).exp();
    let mut y = 0.0f32;
    let lp: Vec<f32> = pcm48
        .iter()
        .map(|&x| {
            y += a * (x - y);
            y
        })
        .collect();
    let mut pcm16 = Vec::with_capacity(pcm48.len() / 3 + 2);
    crate::radio::dsp::LinearResampler::new(SR as f64, SR16 as f64).process(&lp, &mut pcm16);
    let secs = pcm48.len() as f32 / SR as f32;
    let _ = tx.send(Segment { pcm16, rssi_dbfs: rssi, secs });
}

/// Merge a freshly transcribed continuous window onto the running transcript.
/// `tail_words` is the last few dozen words already committed; `candidate` is
/// the new window's text, whose first `overlap_secs` re-reads committed
/// audio. Returns `(delta, new_tail)` — `delta` is the genuinely new text
/// (empty if the window added nothing).
///
/// Strategy: find a run of ≥ 2 identical words shared by the end of the
/// committed tail and the head of the candidate, and drop everything in the
/// candidate up to the end of the latest such run. The search into the
/// candidate is bounded to roughly the overlap duration so a chance match
/// deep in the new text can't swallow it. When Whisper re-renders the
/// overlap too differently to anchor (numbers spelled out vs. digits, say),
/// nothing is dropped — a little duplication beats losing text.
#[cfg(feature = "stt")]
pub(crate) fn stitch(
    tail_words: &[String],
    candidate: &str,
    overlap_secs: f32,
) -> (String, Vec<String>) {
    let cand: Vec<&str> = candidate.split_whitespace().collect();
    let new_tail: Vec<String> =
        cand.iter().rev().take(30).rev().map(|s| s.to_string()).collect();
    if cand.is_empty() {
        return (String::new(), tail_words.to_vec());
    }
    let norm = |w: &str| {
        w.to_lowercase()
            .trim_matches(|c: char| !c.is_alphanumeric())
            .to_string()
    };
    let cand_lc: Vec<String> = cand.iter().map(|w| norm(w)).collect();
    let tail_lc: Vec<String> = tail_words.iter().map(|w| norm(w)).collect();

    // ~3 words/sec of speech, plus a few words of slack.
    let search = (((overlap_secs * 3.0).ceil() as usize) + 5).min(cand_lc.len());
    if tail_lc.is_empty() || search < 2 {
        return (candidate.to_string(), new_tail);
    }
    let tw = &tail_lc[tail_lc.len().saturating_sub(search + 8)..];
    let cw = &cand_lc[..search];

    let mut consumed = 0usize;
    for ci in 0..cw.len() {
        for ti in 0..tw.len() {
            let mut l = 0;
            while ci + l < cw.len()
                && ti + l < tw.len()
                && !cw[ci + l].is_empty()
                && cw[ci + l] == tw[ti + l]
            {
                l += 1;
            }
            if l >= 2 && ci + l > consumed {
                consumed = ci + l;
            }
        }
    }
    (cand[consumed..].join(" "), new_tail)
}

/// A greedy decode has collapsed into a loop when the last three `L`-token
/// blocks (for some small `L`) are identical.
#[cfg(feature = "stt")]
pub(crate) fn repetition_cycle_len(tokens: &[u32]) -> Option<usize> {
    let n = tokens.len();
    (3..=8usize).find(|&l| {
        n >= 3 * l
            && tokens[n - l..] == tokens[n - 2 * l..n - l]
            && tokens[n - 2 * l..n - l] == tokens[n - 3 * l..n - 2 * l]
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn clear_transcript_empties_ring_and_text() {
        let cfg = crate::config::Config::parse_from(["lanline-server"]);
        let v = VoiceShared::new(&cfg);
        v.transcript.lock().unwrap().push_back(TranscriptEntry {
            time: time::OffsetDateTime::now_utc(),
            text: "  hello world  ".into(),
            rssi_dbfs: -40.0,
            secs: 3.0,
        });
        assert_eq!(v.transcript().len(), 1);
        assert_eq!(v.transcript_text(), "hello world\n");
        v.clear_transcript();
        assert!(v.transcript().is_empty());
        assert_eq!(v.transcript_text(), "\n");
    }

    #[cfg(feature = "stt")]
    mod stt {
        use super::*;

        fn words(s: &str) -> Vec<String> {
            s.split_whitespace().map(|w| w.to_string()).collect()
        }

        #[test]
        fn stitch_trims_an_exact_overlap() {
            let tail = words("the wind was north at ten knots");
            let (delta, _) = stitch(&tail, "at ten knots. the seas were two feet", 3.0);
            assert_eq!(delta, "the seas were two feet");
        }

        #[test]
        fn stitch_anchors_past_a_reworded_number() {
            let tail = words("the dew point was seventy two degrees");
            // Whisper re-rendered the trailing "seventy two" as "72" but the
            // "the dew point was" run still anchors the cut.
            let (delta, _) = stitch(&tail, "the dew point was 72 relative humidity ninety percent", 4.0);
            assert_eq!(delta, "72 relative humidity ninety percent");
        }

        #[test]
        fn stitch_keeps_everything_when_nothing_overlaps() {
            let tail = words("first sentence here");
            let (delta, tnew) = stitch(&tail, "a completely different second sentence", 4.0);
            assert_eq!(delta, "a completely different second sentence");
            assert_eq!(tnew.last().unwrap(), "sentence");
        }

        #[test]
        fn stitch_full_duplicate_yields_empty_delta() {
            let tail = words("alpha bravo charlie delta echo");
            let (delta, _) = stitch(&tail, "alpha bravo charlie delta echo", 5.0);
            assert_eq!(delta, "");
        }

        #[test]
        fn stitch_first_window_keeps_all_when_overlap_is_zero() {
            let (delta, _) = stitch(&[], "the coastal waters forecast for", 0.0);
            assert_eq!(delta, "the coastal waters forecast for");
        }

        #[test]
        fn repetition_cycle_spots_a_loop_and_ignores_prose() {
            let loopy: Vec<u32> = [10u32, 20, 30].iter().cycle().take(15).copied().collect();
            assert!(repetition_cycle_len(&loopy).is_some());
            let prose: Vec<u32> = (0..40).collect();
            assert!(repetition_cycle_len(&prose).is_none());
        }

        #[test]
        fn ring_windows_overlap_and_advance() {
            let mut r = AudioRing::new();
            r.active = true;
            let frame = vec![0.1f32; SR / 50]; // 20 ms @ 48 kHz
            let push_secs = |r: &mut AudioRing, secs: usize| {
                for _ in 0..(secs * 50) {
                    r.push(&frame, -20.0);
                }
            };

            push_secs(&mut r, 10);
            let w1 = r.take_window().expect("first window");
            assert!(w1.overlap_secs < 0.2, "nothing committed yet: {}", w1.overlap_secs);
            assert!((w1.new_secs - 10.0).abs() < 0.3, "new≈10s: {}", w1.new_secs);
            let committed1 = r.committed;
            assert!(
                (committed1 as f32 / SR16 as f32 - 8.0).abs() < 0.3,
                "committed to win_end − GUARD"
            );

            push_secs(&mut r, 4);
            assert!(r.take_window().is_none(), "not MIN_NEW yet");

            push_secs(&mut r, 8);
            let w2 = r.take_window().expect("second window");
            assert!((w2.overlap_secs - 5.0).abs() < 0.3, "re-reads LEAD: {}", w2.overlap_secs);
            assert!((w2.new_secs - 14.0).abs() < 0.4, "new≈14s: {}", w2.new_secs);
            assert!(r.committed > committed1);
        }

        #[test]
        fn ring_retries_an_empty_window_once() {
            let mut r = AudioRing::new();
            r.active = true;
            let frame = vec![0.1f32; SR / 50];
            for _ in 0..(20 * 50) {
                r.push(&frame, -20.0);
            }
            let w = r.take_window().expect("window");
            let committed_after_take = r.committed;
            let gen = w.generation;

            // First empty decode → committed rolls back for another pass.
            r.retry_empty(gen, w.new_secs);
            assert!(r.committed < committed_after_take, "rewound for a retry");
            let after_retry = r.committed;

            // Second empty decode on the same span → no further rewind.
            r.retry_empty(gen, w.new_secs);
            assert_eq!(r.committed, after_retry, "at most one retry");

            // A good window later clears the retry budget.
            r.note_commit();
            r.retry_empty(gen, 12.0);
            assert!(r.committed < after_retry, "budget refreshed after a commit");
        }

        #[test]
        fn ring_clamps_when_far_behind() {
            let mut r = AudioRing::new();
            r.active = true;
            let frame = vec![0.1f32; SR / 50];
            for _ in 0..(70 * 50) {
                r.push(&frame, -20.0);
            }
            let w = r.take_window().expect("window");
            assert!(w.pcm16.len() <= MAX_WIN16 + SR16, "capped near MAX_WIN");
            assert!(r.behind == 0, "warned + reset");
        }
    }
}
