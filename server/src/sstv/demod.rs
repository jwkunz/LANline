//! SSTV physical layer: RF demod (FM for 2 m / ISS, SSB for HF) recovers the
//! 1100–2300 Hz audio; a subcarrier frequency discriminator turns that into a
//! per-sample instantaneous frequency (1500 Hz = black, 2300 Hz = white,
//! 1200 Hz = sync); a forward-scanning state machine finds the VIS header (or
//! obeys a forced mode), picks a [`crate::sstv::modes::Mode`], and re-samples
//! each scan line into RGB rows with per-line sync tracking (slant
//! correction).
//!
//! Self-contained primitives (own NCO / FIR / one-pole / resampler), mirroring
//! `crate::apt::demod` — the decode logic is unit-testable without a radio.

use super::modes::{by_key, by_vis, Color, Mode};
use num_complex::Complex32;
use std::collections::VecDeque;
use std::f64::consts::PI;

/// Audio working rate for the line decoder.
pub const AUDIO_RATE: f64 = 16_000.0;
/// Subcarrier centre the discriminator references.
const CENTER_HZ: f64 = 1900.0;
const BLACK_HZ: f32 = 1500.0;
const WHITE_HZ: f32 = 2300.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Demod {
    Fm,
    #[default]
    Usb,
    Lsb,
}
impl Demod {
    pub fn parse(s: &str) -> Demod {
        match s.to_ascii_lowercase().as_str() {
            "fm" => Demod::Fm,
            "lsb" => Demod::Lsb,
            _ => Demod::Usb,
        }
    }
}

#[derive(Clone, Debug)]
pub struct SstvParams {
    pub demod: Demod,
    pub deviation_hz: f64,
    pub channel_bw_hz: f64,
    /// SSB suppressed-carrier offset from the tuned dial frequency.
    pub bfo_offset_hz: f64,
    /// The tuner LO offset (`tuner.lo_offset_hz`); the front-end NCO undoes it.
    pub lo_offset_hz: f64,
    /// `None` = auto (decode the VIS header). `Some` = force this mode and
    /// start a frame on the next sync (weak signals with no readable VIS).
    pub manual_mode: Option<String>,
    /// Manual pixel-clock trim, ppm (corrects residual slant).
    pub slant_ppm: f64,
}
impl Default for SstvParams {
    fn default() -> Self {
        Self {
            demod: Demod::Usb,
            deviation_hz: 5_000.0,
            channel_bw_hz: 16_000.0,
            bfo_offset_hz: 0.0,
            lo_offset_hz: 12_000.0,
            manual_mode: None,
            slant_ppm: 0.0,
        }
    }
}

/// One finished image row.
#[derive(Clone, Debug)]
pub struct Row {
    pub y: usize,
    pub rgb: Vec<[u8; 3]>,
    pub mode: &'static str,
    pub width: usize,
    pub height: usize,
    pub frame_done: bool,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Metrics {
    pub locked: bool,
    pub mode: Option<&'static str>,
    pub line: usize,
    pub height: usize,
    pub frames: u64,
    pub snr_db: f32,
}

// --- self-contained primitives ------------------------------------------

struct Nco {
    rot: Complex32,
    step: Complex32,
    n: u32,
}
impl Nco {
    fn new(cycles_per_sample: f64) -> Self {
        let a = 2.0 * PI * cycles_per_sample;
        Self { rot: Complex32::new(1.0, 0.0), step: Complex32::new(a.cos() as f32, a.sin() as f32), n: 0 }
    }
    #[inline]
    fn advance(&mut self, x: Complex32) -> Complex32 {
        let y = x * self.rot;
        self.rot *= self.step;
        self.n += 1;
        if self.n >= 8192 {
            self.n = 0;
            let m = self.rot.norm();
            if m > 1e-6 {
                self.rot /= m;
            }
        }
        y
    }
}

/// Decimating complex FIR low-pass (Blackman-windowed sinc).
struct Fir {
    taps: Vec<f32>,
    hist: Vec<Complex32>,
    widx: usize,
    decim: usize,
    counter: usize,
}
impl Fir {
    fn new(decim: usize, cutoff_hz: f64, fs_in: f64, num_taps: usize) -> Self {
        let n = num_taps | 1;
        let fc = cutoff_hz / fs_in;
        let mid = (n - 1) as f64 / 2.0;
        let mut taps = vec![0.0f32; n];
        let mut sum = 0.0f64;
        for (k, tap) in taps.iter_mut().enumerate() {
            let m = k as f64 - mid;
            let sinc = if m.abs() < 1e-9 { 2.0 * fc } else { (2.0 * PI * fc * m).sin() / (PI * m) };
            let w = 0.42 - 0.5 * (2.0 * PI * k as f64 / (n - 1) as f64).cos()
                + 0.08 * (4.0 * PI * k as f64 / (n - 1) as f64).cos();
            let v = sinc * w;
            *tap = v as f32;
            sum += v;
        }
        for t in &mut taps {
            *t /= sum as f32;
        }
        Self { taps, hist: vec![Complex32::new(0.0, 0.0); n], widx: 0, decim, counter: 0 }
    }
    #[inline]
    fn push(&mut self, x: Complex32) -> Option<Complex32> {
        let n = self.hist.len();
        self.hist[self.widx] = x;
        self.widx = (self.widx + 1) % n;
        self.counter += 1;
        if self.counter < self.decim {
            return None;
        }
        self.counter = 0;
        let mut acc = Complex32::new(0.0, 0.0);
        let mut idx = (self.widx + n - 1) % n;
        for &h in &self.taps {
            let s = self.hist[idx];
            acc.re += h * s.re;
            acc.im += h * s.im;
            idx = if idx == 0 { n - 1 } else { idx - 1 };
        }
        Some(acc)
    }
}

/// One-pole complex low-pass.
struct CLp {
    a: f32,
    y: Complex32,
}
impl CLp {
    fn new(fs: f64, fc: f64) -> Self {
        Self { a: (1.0 - (-2.0 * PI * fc / fs).exp()) as f32, y: Complex32::new(0.0, 0.0) }
    }
    #[inline]
    fn process(&mut self, x: Complex32) -> Complex32 {
        self.y += (x - self.y) * self.a;
        self.y
    }
}

/// Streaming linear-interpolation resampler → pushes into a `Vec<f32>`.
struct Resampler {
    step: f64,
    next_t: f64,
    last: f32,
    primed: bool,
}
impl Resampler {
    fn new(fs_in: f64, fs_out: f64) -> Self {
        Self { step: fs_in / fs_out, next_t: 0.0, last: 0.0, primed: false }
    }
    #[inline]
    fn push(&mut self, x: f32, out: &mut Vec<f32>) {
        if !self.primed {
            self.last = x;
            self.primed = true;
        }
        while self.next_t < 1.0 {
            let f = self.next_t as f32;
            out.push(self.last + (x - self.last) * f);
            self.next_t += self.step;
        }
        self.next_t -= 1.0;
        self.last = x;
    }
}

// --- decoder --------------------------------------------------------------

struct Frame {
    mode: &'static Mode,
    /// Absolute audio index of this line's sync leading edge.
    line_start: f64,
    /// Measured line period (EMA), audio samples.
    line_est: f64,
    rows: usize,
    /// Robot 36 carried chroma.
    ry: Option<Vec<f32>>,
    by: Option<Vec<f32>>,
}

enum State {
    Search,
    Vis { bit0: u64 },
    Frame(Frame),
}

pub struct SstvDemod {
    // front end
    nco: Nco,
    fir: Fir,
    channel_rate: f64,
    prev_iq: Complex32,
    disc_gain: f32,
    demod: Demod,
    ssb_lp: CLp,
    resamp: Resampler,
    scratch: Vec<f32>,
    // audio → instantaneous frequency
    af_nco: Nco,
    af_lp: Fir,
    af_prev: Complex32,
    // per-audio-sample instantaneous frequency ring
    freqs: VecDeque<f32>,
    t0: u64,
    /// Forward-only scan cursor (absolute audio index), used in Search.
    cur: u64,
    // run trackers for the Search state
    leader_run: u32,
    start_run: u32,
    quiet_run: u32,
    /// Latched once a >=100 ms 1900 Hz leader is seen; cleared by a long gap.
    saw_leader: bool,
    start_onset: u64,
    // state
    state: State,
    manual: Option<&'static Mode>,
    slant_ppm: f64,
    frames: u64,
    snr_db: f32,
}

impl SstvDemod {
    pub fn new(device_rate: f64, p: &SstvParams) -> Self {
        let target = (p.channel_bw_hz * 1.4).max(AUDIO_RATE * 2.5);
        let decim = ((device_rate / target).floor() as usize).max(1);
        let channel_rate = device_rate / decim as f64;
        let cutoff = match p.demod {
            Demod::Fm => (p.channel_bw_hz * 0.5).clamp(6_000.0, channel_rate * 0.45),
            _ => 3_400.0_f64.min(channel_rate * 0.45),
        };
        let num_taps = (8 * decim + 1).min(1023);
        let disc_gain = (channel_rate / (2.0 * PI * p.deviation_hz.max(1.0))) as f32;
        let mix = match p.demod {
            Demod::Fm => -p.lo_offset_hz,
            Demod::Usb => -(p.lo_offset_hz + p.bfo_offset_hz),
            Demod::Lsb => -(p.lo_offset_hz - p.bfo_offset_hz),
        };
        SstvDemod {
            nco: Nco::new(mix / device_rate),
            fir: Fir::new(decim, cutoff, device_rate, num_taps),
            channel_rate,
            prev_iq: Complex32::new(1.0, 0.0),
            disc_gain,
            demod: p.demod,
            ssb_lp: CLp::new(channel_rate, 3_400.0),
            resamp: Resampler::new(channel_rate, AUDIO_RATE),
            scratch: Vec::with_capacity(4096),
            af_nco: Nco::new(-CENTER_HZ / AUDIO_RATE),
            af_lp: Fir::new(1, 800.0, AUDIO_RATE, 31),
            af_prev: Complex32::new(1.0, 0.0),
            freqs: VecDeque::with_capacity(AUDIO_RATE as usize * 4),
            t0: 0,
            cur: 0,
            leader_run: 0,
            start_run: 0,
            quiet_run: 0,
            saw_leader: false,
            start_onset: 0,
            state: State::Search,
            manual: p.manual_mode.as_deref().and_then(by_key),
            slant_ppm: p.slant_ppm,
            frames: 0,
            snr_db: -40.0,
        }
    }

    pub fn channel_rate(&self) -> f64 {
        self.channel_rate
    }

    pub fn metrics(&self) -> Metrics {
        match &self.state {
            State::Frame(f) => Metrics {
                locked: true,
                mode: Some(f.mode.name),
                line: f.rows,
                height: f.mode.height,
                frames: self.frames,
                snr_db: self.snr_db,
            },
            _ => Metrics { frames: self.frames, snr_db: self.snr_db, ..Default::default() },
        }
    }

    /// Consume a block of device-rate IQ; return completed image rows.
    pub fn feed(&mut self, iq: &[Complex32]) -> Vec<Row> {
        self.scratch.clear();
        let mut pwr = 0.0f64;
        for &x in iq {
            let mixed = self.nco.advance(x);
            let Some(s) = self.fir.push(mixed) else { continue };
            pwr += s.norm_sqr() as f64;
            let a = match self.demod {
                Demod::Fm => {
                    let prod = s * self.prev_iq.conj();
                    self.prev_iq = s;
                    prod.im.atan2(prod.re) * self.disc_gain
                }
                Demod::Usb | Demod::Lsb => self.ssb_lp.process(s).re * 8.0,
            };
            self.resamp.push(a, &mut self.scratch);
        }
        if !iq.is_empty() {
            let p = (pwr / iq.len().max(1) as f64) as f32;
            self.snr_db = self.snr_db * 0.9 + (10.0 * (p + 1e-9).log10()).clamp(-90.0, 0.0) * 0.1;
        }

        let audio = std::mem::take(&mut self.scratch);
        for &a in &audio {
            let Some(z) = self.af_lp.push(self.af_nco.advance(Complex32::new(a, 0.0))) else { continue };
            let d = z * self.af_prev.conj();
            self.af_prev = z;
            let f = CENTER_HZ as f32
                + d.im.atan2(d.re) * (AUDIO_RATE as f32 / (2.0 * std::f32::consts::PI));
            self.freqs.push_back(f.clamp(500.0, 3_000.0));
        }
        self.scratch = audio;

        let mut rows = Vec::new();
        self.run(&mut rows);
        self.trim();
        rows
    }

    fn end(&self) -> u64 {
        self.t0 + self.freqs.len() as u64
    }
    fn f_at(&self, idx: u64) -> Option<f32> {
        self.freqs.get(idx.checked_sub(self.t0)? as usize).copied()
    }
    fn interp(&self, idx: f64) -> Option<f32> {
        let rel = idx - self.t0 as f64;
        if rel < 0.0 {
            return self.freqs.front().copied();
        }
        let i = rel.floor() as usize;
        let frac = (rel - i as f64) as f32;
        let a = *self.freqs.get(i)?;
        let b = *self.freqs.get(i + 1).unwrap_or(&a);
        Some(a + (b - a) * frac)
    }
    fn window_mean(&self, centre: f64, half: f64) -> Option<f32> {
        let a = ((centre - half).floor() as i64).max(self.t0 as i64) as u64;
        let b = ((centre + half).ceil() as u64).min(self.end());
        if b <= a {
            return None;
        }
        let mut s = 0.0f32;
        for k in a..b {
            s += self.freqs[(k - self.t0) as usize];
        }
        Some(s / (b - a) as f32)
    }

    fn drop_to(&mut self, idx: u64) {
        let idx = idx.min(self.end());
        if idx > self.t0 {
            let d = (idx - self.t0) as usize;
            self.freqs.drain(..d.min(self.freqs.len()));
            self.t0 = idx;
        }
        if self.cur < self.t0 {
            self.cur = self.t0;
        }
    }

    fn trim(&mut self) {
        let keep_from = match &self.state {
            State::Frame(f) => f.line_start.floor() as u64,
            State::Vis { bit0 } => bit0.saturating_sub((0.1 * AUDIO_RATE) as u64),
            State::Search => self.end().saturating_sub((2.0 * AUDIO_RATE) as u64),
        };
        self.drop_to(keep_from);
    }

    fn run(&mut self, out: &mut Vec<Row>) {
        loop {
            let again = match std::mem::replace(&mut self.state, State::Search) {
                State::Search => self.step_search(),
                State::Vis { bit0 } => self.step_vis(bit0),
                State::Frame(f) => self.step_frame(f, out),
            };
            if !again {
                break;
            }
        }
    }

    /// Advance the forward cursor classifying leader (1900) / start (1200) /
    /// other, and hand off to VIS or a forced Frame. Returns true if it
    /// changed state.
    fn step_search(&mut self) -> bool {
        let ms = AUDIO_RATE / 1000.0;
        while self.cur < self.end() {
            let f = self.f_at(self.cur).unwrap();
            let leader = (f - 1900.0).abs() < 70.0;
            let start = (f - 1200.0).abs() < 70.0;

            if start {
                if self.start_run == 0 {
                    self.start_onset = self.cur;
                }
                self.start_run += 1;
            } else {
                // manual: force a frame on any solid sync pulse (the
                // operator has pointed at an ongoing transmission).
                if self.manual.is_some() && self.start_run as f64 >= 4.0 * ms {
                    let mode = self.manual.unwrap();
                    self.state = State::Frame(Frame {
                        mode,
                        line_start: self.start_onset as f64,
                        line_est: mode.line * AUDIO_RATE,
                        rows: 0,
                        ry: None,
                        by: None,
                    });
                    self.reset_runs();
                    return true;
                }
                // auto: leader seen, then a persistent 30 ms start bit → VIS
                if self.manual.is_none()
                    && self.start_run as f64 >= 16.0 * ms
                    && self.saw_leader
                {
                    self.state = State::Vis { bit0: self.start_onset + (30.0 * ms) as u64 };
                    self.reset_runs();
                    return true;
                }
                self.start_run = 0;
            }

            if leader {
                self.leader_run = self.leader_run.saturating_add(1);
                if self.leader_run as f64 >= 100.0 * ms {
                    self.saw_leader = true;
                }
                self.quiet_run = 0;
            } else if !start {
                self.leader_run = 0;
                self.quiet_run = self.quiet_run.saturating_add(1);
                if self.quiet_run as f64 >= 400.0 * ms {
                    self.saw_leader = false;
                }
            }
            self.cur += 1;
        }
        false
    }

    fn step_vis(&mut self, bit0: u64) -> bool {
        let ms = AUDIO_RATE / 1000.0;
        let bit = 30.0 * ms;
        if self.interp(bit0 as f64 + 9.0 * bit).is_none() {
            self.state = State::Vis { bit0 };
            return false;
        }
        let mut code = 0u16;
        let mut ones = 0u32;
        for i in 0..8u32 {
            let c = bit0 as f64 + (i as f64 + 0.5) * bit;
            let m = self.window_mean(c, bit * 0.3).unwrap_or(1200.0);
            if m < 1250.0 {
                code |= 1 << i;
                ones += 1;
            }
        }
        let par = self
            .window_mean(bit0 as f64 + 8.5 * bit, bit * 0.3)
            .map(|m| m < 1250.0)
            .unwrap_or(false);
        let parity_ok = ((ones + par as u32) % 2) == 0;
        let after = bit0 + (9.5 * bit) as u64;
        self.drop_to(after);
        self.reset_runs();
        self.state = State::Search;

        if parity_ok {
            if let Some(mode) = by_vis(code as u8) {
                tracing::info!("sstv: VIS {:#04x} → {}", code, mode.name);
                self.state = State::Frame(Frame {
                    mode,
                    line_start: after as f64,
                    line_est: mode.line * AUDIO_RATE,
                    rows: 0,
                    ry: None,
                    by: None,
                });
                return true;
            }
            tracing::debug!("sstv: VIS {:#04x} unknown", code);
        }
        true
    }

    fn step_frame(&mut self, mut f: Frame, out: &mut Vec<Row>) -> bool {
        let fs = AUDIO_RATE;
        let ms = fs / 1000.0;
        let m = f.mode;
        if self.interp(f.line_start + m.line * fs + 24.0 * ms).is_none() {
            self.state = State::Frame(f);
            return false;
        }

        // refine this line's sync leading edge (slant / drift correction)
        let expect = if f.rows == 0 { f.line_start } else { f.line_start + f.line_est };
        let sync = m.sync * fs;
        let sep = m.sep * fs;
        let mut edge = expect;
        let mut best = f32::MAX;
        let mut k = -12.0 * ms;
        while k <= 12.0 * ms {
            if let Some(mm) = self.window_mean(expect + k + sync * 0.5, sync * 0.35) {
                let c = (mm - 1200.0).abs();
                if c < best {
                    best = c;
                    edge = expect + k;
                }
            }
            k += ms;
        }
        if f.rows > 0 && best < 120.0 {
            let measured = (edge - f.line_start).clamp(m.line * fs * 0.9, m.line * fs * 1.1);
            f.line_est = f.line_est * 0.8 + measured * 0.2;
            f.line_start = edge;
        } else if f.rows > 0 {
            f.line_start = expect;
        }

        let clock = (m.line * fs) / f.line_est * (1.0 + self.slant_ppm * 1e-6);
        let px = m.pixel * fs * clock;
        let base = f.line_start + sync + sep;
        let f2l = |hz: f32| ((hz - BLACK_HZ) / (WHITE_HZ - BLACK_HZ)).clamp(0.0, 1.0);
        let scan = |start: f64, n: usize, step: f64| -> Vec<f32> {
            (0..n).map(|i| self.interp(start + i as f64 * step).map(f2l).unwrap_or(0.0)).collect()
        };

        let new_rows: Vec<Vec<[u8; 3]>> = match m.color {
            Color::Rgb { order } => {
                // Martin: sync,sep,G,sep,B,sep,R.  Scottie: sync,sep,R,sep,G,sep,B
                // (Scottie's sync sits between B and R of the *previous* line;
                //  taken as the line boundary it reads R,G,B here).
                let mut comps: [Vec<f32>; 3] = Default::default();
                let mut t = base;
                for c in comps.iter_mut() {
                    *c = scan(t, m.width, px);
                    t += m.width as f64 * px + sep;
                }
                let (ri, gi, bi) = (order[0] as usize, order[1] as usize, order[2] as usize);
                vec![(0..m.width)
                    .map(|i| {
                        [
                            (comps[ri][i] * 255.0) as u8,
                            (comps[gi][i] * 255.0) as u8,
                            (comps[bi][i] * 255.0) as u8,
                        ]
                    })
                    .collect()]
            }
            Color::Robot36 => {
                let y = scan(base, m.width, px);
                let chroma = scan(base + m.width as f64 * px + sep, m.width, px * 2.0);
                if f.rows % 2 == 0 {
                    f.ry = Some(chroma);
                } else {
                    f.by = Some(chroma);
                }
                let ry = f.ry.clone().unwrap_or_else(|| vec![0.5; m.width]);
                let by = f.by.clone().unwrap_or_else(|| vec![0.5; m.width]);
                vec![ycc_row(&y, &ry, &by, m.width)]
            }
            Color::Pd => {
                let w = m.width;
                let y1 = scan(base, w, px);
                let ry = scan(base + w as f64 * px, w, px);
                let by = scan(base + 2.0 * w as f64 * px, w, px);
                let y2 = scan(base + 3.0 * w as f64 * px, w, px);
                vec![ycc_row(&y1, &ry, &by, w), ycc_row(&y2, &ry, &by, w)]
            }
        };

        let mut done = false;
        for rgb in new_rows {
            if f.rows >= m.height {
                done = true;
                break;
            }
            let y = f.rows;
            f.rows += 1;
            done = f.rows >= m.height;
            out.push(Row { y, rgb, mode: m.name, width: m.width, height: m.height, frame_done: done });
        }

        if done {
            self.frames += 1;
            self.reset_runs();
            // Skip the cursor past the whole finished picture so Search
            // doesn't re-trigger on the frame's own trailing syncs.
            self.cur = (f.line_start + f.line_est).ceil() as u64;
            self.state = State::Search;
        } else {
            self.state = State::Frame(f);
        }
        true
    }

    fn reset_runs(&mut self) {
        self.leader_run = 0;
        self.start_run = 0;
        self.quiet_run = 0;
        self.saw_leader = false;
    }
}

/// YCrCb (0..1 each, chroma centred at 0.5) → RGB row (JPEG conversion).
fn ycc_row(y: &[f32], ry: &[f32], by: &[f32], w: usize) -> Vec<[u8; 3]> {
    (0..w)
        .map(|i| {
            let yy = y.get(i).copied().unwrap_or(0.0) * 255.0;
            let cr = (ry.get(i).copied().unwrap_or(0.5) - 0.5) * 255.0;
            let cb = (by.get(i).copied().unwrap_or(0.5) - 0.5) * 255.0;
            [
                (yy + 1.402 * cr).clamp(0.0, 255.0) as u8,
                (yy - 0.344136 * cb - 0.714136 * cr).clamp(0.0, 255.0) as u8,
                (yy + 1.772 * cb).clamp(0.0, 255.0) as u8,
            ]
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sstv::modes::{Color, Mode};

    const DEV_RATE: f64 = 192_000.0;
    const FM_DEV: f64 = 2_000.0;

    /// Append `secs` of a `freq`-Hz tone to a 16 kHz audio stream.
    fn tone(audio: &mut Vec<f32>, phase: &mut f64, freq: f64, secs: f64) {
        let n = (secs * AUDIO_RATE).round() as usize;
        for _ in 0..n {
            audio.push(phase.sin() as f32);
            *phase += 2.0 * PI * freq / AUDIO_RATE;
        }
    }

    /// FM-modulate a 16 kHz SSTV audio stream onto IQ at `DEV_RATE`.
    fn fm_iq(audio: &[f32]) -> Vec<Complex32> {
        // upsample audio DEV_RATE / AUDIO_RATE, then FM.
        let up = (DEV_RATE / AUDIO_RATE) as usize;
        let mut carrier = 0.0f64;
        let mut out = Vec::with_capacity(audio.len() * up);
        for w in audio.windows(2).chain(std::iter::once([audio[audio.len() - 1]; 2].as_slice())) {
            let (a0, a1) = (w[0], w[1]);
            for k in 0..up {
                let f = k as f32 / up as f32;
                let s = a0 + (a1 - a0) * f;
                carrier += 2.0 * PI * (FM_DEV * s as f64) / DEV_RATE;
                out.push(Complex32::new(carrier.cos() as f32, carrier.sin() as f32));
            }
        }
        out
    }

    fn lvl_to_hz(l: f32) -> f64 {
        (BLACK_HZ + l * (WHITE_HZ - BLACK_HZ)) as f64
    }

    /// A tiny RGB mode for fast round-trip tests.
    const TINY: Mode = Mode {
        name: "TINY",
        vis: 0,
        width: 16,
        height: 4,
        sync: 0.009,
        sep: 0.0015,
        pixel: 0.001,
        line: 0.009 + 3.0 * (0.0015 + 16.0 * 0.001),
        color: Color::Rgb { order: [0, 1, 2] },
        scottie_sync: false,
    };

    #[test]
    fn manual_tiny_frame_round_trips() {
        // A 4-row image: row r has R = 4/16-ish ramp, G = r step, B = const.
        let img: Vec<[f32; 3]> = (0..TINY.height)
            .map(|r| [0.2 + 0.15 * r as f32, r as f32 / 4.0, 0.8])
            .collect();

        let mut audio = Vec::new();
        let mut ph = 0.0;
        tone(&mut audio, &mut ph, 2100.0, 0.30); // quiet lead-in (neither 1900 nor 1200)
        for row in &img {
            tone(&mut audio, &mut ph, 1200.0, TINY.sync);
            tone(&mut audio, &mut ph, 1500.0, TINY.sep);
            for &comp in row {
                for _ in 0..TINY.width {
                    tone(&mut audio, &mut ph, lvl_to_hz(comp), TINY.pixel);
                }
                tone(&mut audio, &mut ph, 1500.0, TINY.sep);
            }
        }
        tone(&mut audio, &mut ph, 2100.0, 0.10);

        let iq = fm_iq(&audio);
        let mut d = SstvDemod::new(
            DEV_RATE,
            &SstvParams { demod: Demod::Fm, deviation_hz: FM_DEV, channel_bw_hz: 16_000.0, lo_offset_hz: 0.0, ..Default::default() },
        );
        d.manual = Some(&TINY);

        let mut rows: Vec<Row> = Vec::new();
        for chunk in iq.chunks(8192) {
            rows.extend(d.feed(chunk));
        }
        assert!(rows.len() >= 3, "decoded {} rows", rows.len());
        // check a mid pixel of row 1 (G should be ~0.25 → ~64)
        let r1 = &rows[1];
        let g = r1.rgb[8][1];
        assert!((g as i32 - 64).abs() < 40, "row1 green {g}, want ~64");
        let b = r1.rgb[8][2];
        assert!((b as i32 - 204).abs() < 45, "row1 blue {b}, want ~204");
    }

    #[test]
    fn freq_recovery() {
        let mut audio = Vec::new();
        let mut ph = 0.0;
        for f in [1200.0, 1900.0, 1500.0, 2300.0] {
            tone(&mut audio, &mut ph, f, 0.2);
        }
        let iq = fm_iq(&audio);
        let mut d = SstvDemod::new(
            DEV_RATE,
            &SstvParams { demod: Demod::Fm, deviation_hz: FM_DEV, channel_bw_hz: 16_000.0, lo_offset_hz: 0.0, ..Default::default() },
        );
        for c in iq.chunks(8192) {
            let _ = d.feed(c);
        }
        let n = d.freqs.len();
        for (seg, want) in [1200.0, 1900.0, 1500.0, 2300.0].iter().enumerate() {
            let got = d.freqs[seg * n / 4 + n / 8];
            assert!((got - want).abs() < 40.0, "seg {seg}: {got:.0} Hz, want {want}");
        }
    }

    #[test]
    fn auto_vis_locks_scottie1() {
        let mut audio = Vec::new();
        let mut ph = 0.0;
        tone(&mut audio, &mut ph, 1900.0, 0.30); // leader
        tone(&mut audio, &mut ph, 1200.0, 0.010); // break
        tone(&mut audio, &mut ph, 1900.0, 0.30); // leader
        tone(&mut audio, &mut ph, 1200.0, 0.030); // start bit
        let vis = 60u8; // Scottie 1
        let mut ones = 0;
        for i in 0..8 {
            let one = (vis >> i) & 1 == 1;
            if one {
                ones += 1;
            }
            tone(&mut audio, &mut ph, if one { 1100.0 } else { 1300.0 }, 0.030);
        }
        // even parity
        tone(&mut audio, &mut ph, if ones % 2 == 1 { 1100.0 } else { 1300.0 }, 0.030);
        tone(&mut audio, &mut ph, 1200.0, 0.030); // stop
        tone(&mut audio, &mut ph, 1500.0, 0.20); // start of the picture

        let iq = fm_iq(&audio);
        let mut d = SstvDemod::new(
            DEV_RATE,
            &SstvParams { demod: Demod::Fm, deviation_hz: FM_DEV, channel_bw_hz: 16_000.0, lo_offset_hz: 0.0, ..Default::default() },
        );
        for chunk in iq.chunks(8192) {
            let _ = d.feed(chunk);
        }
        assert_eq!(d.metrics().mode, Some("Scottie 1"), "VIS {vis} should lock Scottie 1");
    }
}
