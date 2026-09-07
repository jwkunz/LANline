//! Voice ↔ text for the FM/AM modes.
//!
//! * TX — `POST /radio/tx/say` synthesizes text to speech (`tts.rs`, feature
//!   `tts`: shells `espeak-ng` or `piper`) and feeds it into the existing
//!   `TxAudioSource` / `key_tx` path.
//! * RX — each received over (squelch open → close) is buffered and handed to
//!   a pure-Rust Whisper worker (`stt.rs`, feature `stt`); the text lands in a
//!   ring served at `GET /radio/transcript`.
//!
//! `VoiceShared` is always compiled so the pipeline / API wiring is
//! unconditional — it is simply inert when neither feature is built or the
//! matching flag is off.

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
#[cfg(feature = "stt")]
const SR: usize = 48_000;
/// Shortest over worth transcribing.
#[cfg(feature = "stt")]
const MIN_SEG: usize = SR * 2 / 5; // 0.4 s
/// Closed-squelch run that ends a segment.
#[cfg(feature = "stt")]
const HANG: usize = SR * 3 / 10; // 0.3 s
/// Whisper's native window — longer overs are cut here.
#[cfg(feature = "stt")]
const MAX_SEG: usize = SR * 30;
/// A continuous carrier (NOAA weather radio, a long net turn) never closes
/// the squelch — chunk it at this cadence so text streams instead of waiting
/// for `MAX_SEG`.
#[cfg(feature = "stt")]
const CONT_SEG: usize = SR * 10; // 10 s

/// A finished over handed to the STT worker (48 kHz mono).
#[cfg(feature = "stt")]
pub(crate) struct Segment {
    pub pcm: Vec<f32>,
    pub rssi_dbfs: f32,
    pub secs: f32,
}

#[cfg(feature = "stt")]
#[derive(Default)]
struct SegAccum {
    buf: Vec<f32>,
    rssi_sum: f64,
    rssi_n: u64,
    closed_run: usize,
    /// Samples the squelch has been continuously open since the last flush —
    /// drives the `CONT_SEG` chunking of a non-stop transmission.
    open_run: usize,
}

pub struct VoiceShared {
    tts: bool,
    stt: bool,
    transcript: Arc<Mutex<VecDeque<TranscriptEntry>>>,
    tx: broadcast::Sender<TranscriptEntry>,
    transcribing: Arc<AtomicBool>,
    #[cfg(feature = "stt")]
    seg: Mutex<SegAccum>,
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
        let stt_tx = if cfg.stt {
            match stt::spawn_worker(cfg, tx.clone(), transcript.clone(), transcribing.clone()) {
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
            seg: Mutex::new(SegAccum::default()),
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

    /// Empty the transcript ring and drop any half-accumulated chunk. Backs
    /// the web "Clear" button (`POST /radio/transcript/clear`).
    pub fn clear_transcript(&self) {
        self.transcript.lock().unwrap().clear();
        #[cfg(feature = "stt")]
        {
            *self.seg.lock().unwrap() = SegAccum::default();
        }
    }

    /// Called every audio frame from `run_sdr`. Accumulates while the squelch
    /// is open; ships the over to the STT worker when it closes. No-op unless
    /// STT is active.
    pub fn stt_feed(&self, pcm: &[f32], squelch_open: bool, rssi_dbfs: f32) {
        #[cfg(feature = "stt")]
        {
            let Some(tx) = &self.stt_tx else {
                return;
            };
            let mut s = self.seg.lock().unwrap();
            if squelch_open {
                s.closed_run = 0;
                s.buf.extend_from_slice(pcm);
                s.open_run += pcm.len();
                s.rssi_sum += rssi_dbfs as f64;
                s.rssi_n += 1;
                if s.buf.len() >= MAX_SEG || s.open_run >= CONT_SEG {
                    flush(&mut s, tx);
                }
            } else if !s.buf.is_empty() {
                s.closed_run += pcm.len();
                if s.closed_run >= HANG {
                    flush(&mut s, tx);
                }
            }
        }
        #[cfg(not(feature = "stt"))]
        {
            let _ = (pcm, squelch_open, rssi_dbfs);
        }
    }
}

#[cfg(feature = "stt")]
fn flush(s: &mut SegAccum, tx: &std::sync::mpsc::Sender<Segment>) {
    let pcm = std::mem::take(&mut s.buf);
    let rssi = if s.rssi_n > 0 { (s.rssi_sum / s.rssi_n as f64) as f32 } else { -120.0 };
    s.rssi_sum = 0.0;
    s.rssi_n = 0;
    s.closed_run = 0;
    s.open_run = 0;
    if pcm.len() >= MIN_SEG {
        let secs = pcm.len() as f32 / SR as f32;
        let _ = tx.send(Segment { pcm, rssi_dbfs: rssi, secs });
    }
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

    /// A carrier that never drops the squelch is chunked at `CONT_SEG`, not
    /// held back to `MAX_SEG`.
    #[cfg(feature = "stt")]
    #[test]
    fn continuous_carrier_chunks_at_cont_seg() {
        let (tx, rx) = std::sync::mpsc::channel::<Segment>();
        let mut s = SegAccum::default();
        let frame = vec![0.2f32; SR / 10]; // 100 ms
        let mut flushed = 0;
        for _ in 0..250 {
            // 25 s of continuous audio
            s.buf.extend_from_slice(&frame);
            s.open_run += frame.len();
            if s.buf.len() >= MAX_SEG || s.open_run >= CONT_SEG {
                flush(&mut s, &tx);
                flushed += 1;
            }
        }
        assert_eq!(flushed, 2, "25 s / 10 s → two chunks");
        let secs: Vec<f32> = rx.try_iter().map(|seg| seg.secs).collect();
        assert_eq!(secs.len(), 2);
        for x in secs {
            assert!((x - 10.0).abs() < 0.2, "each chunk ≈ 10 s, got {x}");
        }
    }
}
