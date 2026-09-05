//! Receiver Analysis mode: a live FFT panadapter + waterfall computed on the
//! server, plus an IQ-to-WAV recorder.
//!
//! Same shape as the other non-audio modes (`adsb`, `ais`, `apt`): the SDR
//! pipeline thread (`radio::run_analysis`) writes into a lock-guarded
//! [`Spectrum`] here, and `GET /api/v1/analysis/spectrum` serves it as a
//! compact binary frame the web client renders to canvas.
//!
//! The waterfall rows are quantised to `u8` over a fixed, wide dB window
//! ([`DB_LO`]..[`DB_HI`]); the client picks its own visible floor/ceiling
//! within that for the colour map, so dragging the dynamic range is instant
//! and needs no server round-trip (as in GQRX / SDR#).

use num_complex::Complex32;
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Instant;

pub mod iq_wav;

/// `u8` 0 maps here.
pub const DB_LO: f32 = -150.0;
/// `u8` 255 maps here.
pub const DB_HI: f32 = 10.0;

fn quantise(db: f32) -> u8 {
    let t = (db - DB_LO) / (DB_HI - DB_LO);
    (t.clamp(0.0, 1.0) * 255.0 + 0.5) as u8
}

/// The rolling spectrum + waterfall, written by the pipeline thread.
pub struct Spectrum {
    pub center_hz: f64,
    pub span_hz: f64,
    pub n_bins: usize,
    /// Total rows produced since the pipeline (re)started — the client's
    /// `since` cursor.
    pub seq: u64,
    /// EMA-averaged power spectrum in dBFS (the panadapter trace), bin 0 =
    /// lowest frequency (DC-centred, `fftshift`ed).
    pub avg_db: Vec<f32>,
    /// Instantaneous rows, quantised, newest last.
    rows: VecDeque<Vec<u8>>,
    max_rows: usize,
}

impl Spectrum {
    fn new(n_bins: usize, max_rows: usize) -> Self {
        Self {
            center_hz: 0.0,
            span_hz: 0.0,
            n_bins,
            seq: 0,
            avg_db: vec![DB_LO; n_bins],
            rows: VecDeque::new(),
            max_rows: max_rows.max(1),
        }
    }

    /// Replace geometry + clear history (pipeline (re)start).
    pub fn reset(&mut self, center_hz: f64, span_hz: f64, n_bins: usize, max_rows: usize) {
        self.center_hz = center_hz;
        self.span_hz = span_hz;
        self.n_bins = n_bins;
        self.seq = 0;
        self.avg_db = vec![DB_LO; n_bins];
        self.rows.clear();
        self.max_rows = max_rows.max(1);
    }

    pub fn set_center(&mut self, center_hz: f64) {
        self.center_hz = center_hz;
    }

    /// Push one instantaneous dB spectrum and fold it into the average.
    pub fn push(&mut self, db: &[f32], avg_alpha: f32) {
        if db.len() != self.n_bins {
            return;
        }
        for (a, &x) in self.avg_db.iter_mut().zip(db) {
            *a = *a * (1.0 - avg_alpha) + x * avg_alpha;
        }
        let row: Vec<u8> = db.iter().map(|&x| quantise(x)).collect();
        if self.rows.len() == self.max_rows {
            self.rows.pop_front();
        }
        self.rows.push_back(row);
        self.seq += 1;
    }

    /// Binary frame for `GET /api/v1/analysis/spectrum?since=<seq>`:
    /// ```text
    /// "LWF1"         magic (4)
    /// seq            u64 LE   — current total row count
    /// center_hz      f64 LE
    /// span_hz        f64 LE
    /// n_bins         u32 LE
    /// db_lo, db_hi   f32 LE   — the u8 row quantisation window
    /// n_new_rows     u32 LE
    /// avg_db         [f32 LE; n_bins]           — panadapter, always sent
    /// rows           [u8; n_bins] × n_new_rows  — oldest→newest since `since`
    /// ```
    pub fn encode(&self, since: u64, max_rows: usize) -> Vec<u8> {
        let n = self.n_bins;
        // How many of the tail rows are "new" relative to `since`.
        let produced_visible = self.rows.len() as u64;
        let first_visible_seq = self.seq.saturating_sub(produced_visible);
        let start = since.max(first_visible_seq);
        let mut new_count = self.seq.saturating_sub(start) as usize;
        if new_count > max_rows {
            new_count = max_rows;
        }
        let skip = self.rows.len().saturating_sub(new_count);

        let mut buf = Vec::with_capacity(38 + n * 4 + new_count * n);
        buf.extend_from_slice(b"LWF1");
        buf.extend_from_slice(&self.seq.to_le_bytes());
        buf.extend_from_slice(&self.center_hz.to_le_bytes());
        buf.extend_from_slice(&self.span_hz.to_le_bytes());
        buf.extend_from_slice(&(n as u32).to_le_bytes());
        buf.extend_from_slice(&DB_LO.to_le_bytes());
        buf.extend_from_slice(&DB_HI.to_le_bytes());
        buf.extend_from_slice(&(new_count as u32).to_le_bytes());
        for &v in &self.avg_db {
            buf.extend_from_slice(&v.to_le_bytes());
        }
        for row in self.rows.iter().skip(skip) {
            buf.extend_from_slice(row);
        }
        buf
    }

    pub fn rows_held(&self) -> usize {
        self.rows.len()
    }
}

/// Metadata for the last completed IQ recording (served by the download
/// endpoint and reported in the status).
#[derive(Clone, serde::Serialize)]
pub struct RecordingInfo {
    pub path: String,
    pub filename: String,
    pub bytes: u64,
    pub secs: f64,
    pub sample_rate_hz: u32,
    pub center_hz: f64,
}

/// `RadioManager`-owned analysis state.
pub struct AnalysisShared {
    pub spectrum: Mutex<Spectrum>,
    pub recorder: Mutex<Option<iq_wav::IqRecorder>>,
    pub last_recording: Mutex<Option<RecordingInfo>>,
    /// Directory new recordings are written to (`--iq-dir`, default cwd).
    pub iq_dir: PathBuf,
}

impl AnalysisShared {
    pub fn new(iq_dir: PathBuf) -> Self {
        Self {
            spectrum: Mutex::new(Spectrum::new(4096, 2000)),
            recorder: Mutex::new(None),
            last_recording: Mutex::new(None),
            iq_dir,
        }
    }

    /// Feed the current IQ block to an active recording. Auto-stops (and
    /// records `last_recording`) when the byte/second cap is hit.
    pub fn record_iq(&self, iq: &[Complex32]) {
        let mut guard = self.recorder.lock().unwrap();
        let done = match guard.as_mut() {
            Some(rec) => rec.write(iq).unwrap_or(true),
            None => return,
        };
        if done {
            if let Some(rec) = guard.take() {
                if let Ok(info) = rec.finish() {
                    *self.last_recording.lock().unwrap() = Some(info);
                }
            }
        }
    }
}

/// Streaming FFT engine: overlapped windowed FFTs → dBFS power, `fftshift`ed.
pub struct Analyzer {
    fft: std::sync::Arc<dyn rustfft::Fft<f32>>,
    size: usize,
    window: Vec<f32>,
    win_gain: f32,
    /// Ring of the most-recent `size` input samples.
    hist: VecDeque<Complex32>,
    /// Samples until the next FFT (hop = size / 2, 50% overlap).
    to_next: usize,
    hop: usize,
    scratch_in: Vec<Complex32>,
    scratch_db: Vec<f32>,
    last_row: Instant,
    min_row_gap: std::time::Duration,
}

impl Analyzer {
    pub fn new(size: usize, window_code: u32, frame_rate_hz: f64) -> Self {
        let size = size.clamp(256, 1 << 16).next_power_of_two();
        let window = make_window(size, window_code);
        let win_gain: f32 = window.iter().sum::<f32>().max(1.0);
        let fft = rustfft::FftPlanner::new().plan_fft_forward(size);
        Self {
            fft,
            size,
            window,
            win_gain,
            hist: VecDeque::with_capacity(size),
            to_next: size,
            hop: (size / 2).max(1),
            scratch_in: vec![Complex32::default(); size],
            scratch_db: vec![0.0; size],
            last_row: Instant::now() - std::time::Duration::from_secs(1),
            min_row_gap: std::time::Duration::from_secs_f64(1.0 / frame_rate_hz.clamp(1.0, 120.0)),
        }
    }

    pub fn size(&self) -> usize {
        self.size
    }

    /// Feed samples; `emit(&[f32])` is called with a fresh dBFS spectrum
    /// (length `size`, DC-centred) no faster than the configured frame rate.
    pub fn process(&mut self, iq: &[Complex32], mut emit: impl FnMut(&[f32])) {
        for &s in iq {
            if self.hist.len() == self.size {
                self.hist.pop_front();
            }
            self.hist.push_back(s);
            if self.to_next > 0 {
                self.to_next -= 1;
            }
            if self.to_next == 0 && self.hist.len() == self.size {
                self.to_next = self.hop;
                if self.last_row.elapsed() < self.min_row_gap {
                    continue;
                }
                self.last_row = Instant::now();
                for (i, s) in self.hist.iter().enumerate() {
                    self.scratch_in[i] = s * self.window[i];
                }
                self.fft.process(&mut self.scratch_in);
                let norm = 1.0 / self.win_gain;
                let half = self.size / 2;
                for k in 0..self.size {
                    // fftshift: bin k of the shifted view = raw bin (k+half)%size
                    let raw = (k + half) % self.size;
                    let p = self.scratch_in[raw].norm_sqr() * norm * norm;
                    self.scratch_db[k] = 10.0 * (p + 1e-20).log10();
                }
                emit(&self.scratch_db);
            }
        }
    }
}

fn make_window(n: usize, code: u32) -> Vec<f32> {
    let m = (n - 1) as f32;
    (0..n)
        .map(|i| {
            let x = i as f32 / m;
            let two_pi_x = std::f32::consts::TAU * x;
            match code {
                // rectangular
                3 => 1.0,
                // blackman
                1 => 0.42 - 0.5 * two_pi_x.cos() + 0.08 * (2.0 * two_pi_x).cos(),
                // blackman-harris (7-term-ish 4-term)
                2 => {
                    0.35875 - 0.48829 * two_pi_x.cos() + 0.14128 * (2.0 * two_pi_x).cos()
                        - 0.01168 * (3.0 * two_pi_x).cos()
                }
                // hann (default, code 0)
                _ => 0.5 - 0.5 * two_pi_x.cos(),
            }
        })
        .collect()
}
