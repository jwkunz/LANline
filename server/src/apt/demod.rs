//! NOAA APT (Automatic Picture Transmission) physical layer: FM demod of the
//! 137 MHz downlink recovers a 2400 Hz AM subcarrier; synchronous AM detection
//! of that subcarrier recovers the video signal; resampling to the standard
//! 4160 words/sec word rate and correlating against the Sync-A pattern lines
//! up each 2080-word (0.5 s) scan line into its two 909-pixel image channels.
//!
//! Self-contained (own NCO/FIR/biquad, mirroring `crate::adsb`/`crate::ais`)
//! so the decode logic is unit-testable without a radio.

use num_complex::Complex32;
use std::f64::consts::PI;

/// Standard APT word (pixel sample) rate.
pub const WORD_RATE: f64 = 4160.0;
/// One scan line = 2 channels × (sync + space + image + telemetry).
pub const WORDS_PER_LINE: usize = 2080;
const HALF_LINE: usize = WORDS_PER_LINE / 2; // 1040
const SYNC_LEN: usize = 39;
const SPACE_LEN: usize = 47;
/// Pixels per channel image (the part we actually render).
pub const IMAGE_LEN: usize = 909;
const IMAGE_A_START: usize = SYNC_LEN + SPACE_LEN; // 86
const IMAGE_B_START: usize = HALF_LINE + SYNC_LEN + SPACE_LEN; // 1126

const DECIM_TAPS: usize = 8;
/// How far either side of the expected line start we search to correct drift.
const RESYNC_RADIUS: usize = 24;

/// One decoded scan line: two 909-pixel grayscale channel images.
#[derive(Debug, Clone)]
pub struct Line {
    pub image_a: [u8; IMAGE_LEN],
    pub image_b: [u8; IMAGE_LEN],
    /// Sync-A correlation strength at the chosen line start (diagnostic).
    pub sync_quality: f32,
}

#[derive(Clone, Copy, Debug)]
pub struct AptParams {
    pub deviation_hz: f64,
    pub channel_bw_hz: f64,
    pub subcarrier_hz: f64,
    pub lo_offset_hz: f64,
}

impl Default for AptParams {
    fn default() -> Self {
        Self { deviation_hz: 17_000.0, channel_bw_hz: 40_000.0, subcarrier_hz: 2_400.0, lo_offset_hz: 25_000.0 }
    }
}

// --- primitives (self-contained; see module doc) ---------------------------

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

/// RBJ low-pass biquad.
struct Biquad {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
    x1: f32,
    x2: f32,
    y1: f32,
    y2: f32,
}
impl Biquad {
    fn lowpass(fs: f64, fc: f64, q: f64) -> Self {
        let w0 = 2.0 * PI * (fc / fs);
        let (sn, cs) = w0.sin_cos();
        let alpha = sn / (2.0 * q);
        let b1 = 1.0 - cs;
        let b0 = b1 / 2.0;
        let a0 = 1.0 + alpha;
        Self {
            b0: (b0 / a0) as f32,
            b1: (b1 / a0) as f32,
            b2: (b0 / a0) as f32,
            a1: (-2.0 * cs / a0) as f32,
            a2: ((1.0 - alpha) / a0) as f32,
            x1: 0.0,
            x2: 0.0,
            y1: 0.0,
            y2: 0.0,
        }
    }
    #[inline]
    fn process(&mut self, x: f32) -> f32 {
        let y = self.b0 * x + self.b1 * self.x1 + self.b2 * self.x2 - self.a1 * self.y1 - self.a2 * self.y2;
        self.x2 = self.x1;
        self.x1 = x;
        self.y2 = self.y1;
        self.y1 = y;
        y
    }
}

/// Streaming linear-interpolation resampler.
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
        // One input sample advances local time by 1; emit every output tick
        // that falls within [0, 1) of this step.
        while self.next_t < 1.0 {
            let f = self.next_t as f32;
            out.push(self.last + (x - self.last) * f);
            self.next_t += self.step;
        }
        self.next_t -= 1.0;
        self.last = x;
    }
}

/// A 7-cycle 1040 Hz square wave modeling the Sync-A pattern at 4160 Hz
/// (4 words/cycle: ++--). Same template drives the test-signal encoder and
/// the decoder's correlator.
pub fn sync_a_template() -> [f32; SYNC_LEN] {
    let mut t = [0.0f32; SYNC_LEN];
    for (i, v) in t.iter_mut().enumerate() {
        *v = if (i / 2) % 2 == 0 { 1.0 } else { -1.0 };
    }
    t
}

fn corr_at(words: &[f32], offset: usize, template: &[f32; SYNC_LEN]) -> f32 {
    let seg = &words[offset..offset + SYNC_LEN];
    let mean = seg.iter().sum::<f32>() / SYNC_LEN as f32;
    let mut score = 0.0f32;
    for (i, &w) in seg.iter().enumerate() {
        score += (w - mean) * template[i];
    }
    score.abs()
}

/// Search `[center - radius, center + radius]` (clamped) for the offset with
/// the strongest Sync-A correlation. `words` must be long enough to cover the
/// search window plus one sync length. Returns `(offset, confidence)`.
///
/// Confidence is a matched-filter SNR: the peak correlation normalized by an
/// estimate of the local noise floor, `sqrt(SYNC_LEN) * stddev(words)`. A
/// true Sync-A pulse correlates *coherently* — every one of its `SYNC_LEN`
/// samples contributes with the same sign as the template, so the score
/// grows linearly with `SYNC_LEN`. Uncorrelated content (noise, image data)
/// correlates only by chance, and by the central limit theorem that score
/// grows just with `sqrt(SYNC_LEN)`. Normalizing by the latter turns the
/// `N` vs `sqrt(N)` gap into a large, level-independent ratio for a real
/// lock, while noise stays near the ratio's own scale factor.
///
/// An earlier version normalized by the *mean of the correlation curve*
/// within the search window instead. That failed in practice: neighboring
/// offsets within a few words of a true sync still overlap most of its
/// low-pass-filtered pulse, so they also score moderately high, inflating
/// the mean and collapsing the ratio (measured ~2.3 for both real signal
/// and noise). Computing the noise floor from the raw words instead of
/// from the correlation curve avoids that self-contamination.
fn find_sync(words: &[f32], center: usize, radius: usize, template: &[f32; SYNC_LEN]) -> (usize, f32) {
    let lo = center.saturating_sub(radius);
    let hi = (center + radius).min(words.len().saturating_sub(SYNC_LEN));
    let mut best = (lo, -1.0f32);
    for o in lo..=hi {
        let s = corr_at(words, o, template);
        if s > best.1 {
            best = (o, s);
        }
    }
    let seg_hi = (hi + SYNC_LEN).min(words.len());
    let seg = &words[lo..seg_hi];
    let seg_mean = seg.iter().sum::<f32>() / seg.len() as f32;
    let variance = seg.iter().map(|&w| (w - seg_mean) * (w - seg_mean)).sum::<f32>() / seg.len() as f32;
    let noise_floor = (variance.sqrt() * (SYNC_LEN as f32).sqrt()).max(1e-6);
    (best.0, best.1 / noise_floor)
}

pub struct AptDemod {
    nco: Nco,
    fir: Fir,
    channel_rate: f64,
    prev_iq: Complex32,
    disc_gain: f32,
    sc_phase: f64,
    sc_step: f64,
    lp_i: Biquad,
    lp_q: Biquad,
    resamp: Resampler,
    words: Vec<f32>,
    /// Expected start of the next line (refined by `find_sync` every line);
    /// `None` until the cold-start search locks on.
    next_line_start: Option<usize>,
    lvl_min: f32,
    lvl_max: f32,
    template: [f32; SYNC_LEN],
}

impl AptDemod {
    pub fn new(device_rate: f64, p: AptParams) -> Self {
        let target = (p.channel_bw_hz * 1.6).max(48_000.0);
        let decim = ((device_rate / target).floor() as usize).max(1);
        let channel_rate = device_rate / decim as f64;
        let cutoff = (p.channel_bw_hz * 0.6).clamp(6_000.0, channel_rate / 2.0 * 0.9);
        let num_taps = (DECIM_TAPS * decim + 1).min(1023);
        let disc_gain = (channel_rate / (2.0 * PI * p.deviation_hz.max(1.0))) as f32;

        Self {
            nco: Nco::new(-p.lo_offset_hz / device_rate),
            fir: Fir::new(decim, cutoff, device_rate, num_taps),
            channel_rate,
            prev_iq: Complex32::new(1.0, 0.0),
            disc_gain,
            sc_phase: 0.0,
            sc_step: 2.0 * PI * p.subcarrier_hz / channel_rate,
            lp_i: Biquad::lowpass(channel_rate, 2_200.0, 0.707),
            lp_q: Biquad::lowpass(channel_rate, 2_200.0, 0.707),
            resamp: Resampler::new(channel_rate, WORD_RATE),
            words: Vec::with_capacity(WORDS_PER_LINE * 3),
            next_line_start: None,
            lvl_min: 0.0,
            lvl_max: 0.05,
            template: sync_a_template(),
        }
    }

    pub fn channel_rate(&self) -> f64 {
        self.channel_rate
    }

    /// Consume a block of device-rate IQ, emitting a [`Line`] per completed
    /// scan line via `on_line`.
    pub fn process<F: FnMut(Line)>(&mut self, iq: &[Complex32], mut on_line: F) {
        for &x in iq {
            let mixed = self.nco.advance(x);
            let Some(s) = self.fir.push(mixed) else { continue };

            let prod = s * self.prev_iq.conj();
            self.prev_iq = s;
            let raw = prod.im.atan2(prod.re) * self.disc_gain;

            // Synchronous AM detection of the 2400 Hz subcarrier: mix to
            // baseband with a local cos/sin reference, low-pass each rail,
            // take the magnitude. Phase-independent, unlike a plain envelope
            // detector on the raw (un-downconverted) subcarrier.
            let (s_sin, s_cos) = self.sc_phase.sin_cos();
            let i = self.lp_i.process(raw * s_cos as f32);
            let q = self.lp_q.process(raw * -(s_sin as f32));
            let env = (i * i + q * q).sqrt();
            self.sc_phase += self.sc_step;
            if self.sc_phase > 1e6 {
                self.sc_phase %= 2.0 * PI;
            }

            self.resamp.push(env, &mut self.words);
        }

        self.drain_lines(&mut on_line);
    }

    fn drain_lines<F: FnMut(Line)>(&mut self, on_line: &mut F) {
        loop {
            // Where we expect this line to start, and how far around it we're
            // willing to search: the whole first line on cold start (we have
            // no idea of the true phase yet), a small window otherwise (just
            // correcting drift from the previous line's fix).
            let (center, radius) = match self.next_line_start {
                Some(s) => (s, RESYNC_RADIUS),
                None => (HALF_LINE, HALF_LINE),
            };
            if self.words.len() < center + radius + SYNC_LEN {
                return; // not even enough to run the search yet
            }
            let (start, confidence) = find_sync(&self.words, center, radius, &self.template);

            if self.words.len() < start + WORDS_PER_LINE {
                return; // found a candidate line start, but it isn't fully in yet
            }

            let line_words = &self.words[start..start + WORDS_PER_LINE];
            for &w in line_words {
                if w < self.lvl_min {
                    self.lvl_min = w;
                } else {
                    self.lvl_min += (w - self.lvl_min) * 0.001;
                }
                if w > self.lvl_max {
                    self.lvl_max = w;
                } else {
                    self.lvl_max += (w - self.lvl_max) * 0.001;
                }
            }
            let span = (self.lvl_max - self.lvl_min).max(1e-6);
            let px = |v: f32| (((v - self.lvl_min) / span) * 255.0).clamp(0.0, 255.0) as u8;

            let mut image_a = [0u8; IMAGE_LEN];
            let mut image_b = [0u8; IMAGE_LEN];
            for k in 0..IMAGE_LEN {
                image_a[k] = px(line_words[IMAGE_A_START + k]);
                image_b[k] = px(line_words[IMAGE_B_START + k]);
            }
            let sync_quality = confidence;
            self.next_line_start = Some(start + WORDS_PER_LINE);

            on_line(Line { image_a, image_b, sync_quality });
            self.trim(start);
        }
    }

    /// Drop consumed words from the front, keeping indices in `words` valid.
    fn trim(&mut self, upto: usize) {
        let keep_from = upto.min(self.words.len());
        if keep_from == 0 {
            return;
        }
        self.words.drain(..keep_from);
        if let Some(s) = &mut self.next_line_start {
            *s -= keep_from;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build IQ for one synthetic scan line: known per-word amplitudes
    /// (0..1), AM-modulated onto a 2400 Hz subcarrier, FM-modulated onto the
    /// carrier, at `device_rate` with the given LO offset.
    fn synth_line_iq(words: &[f32], device_rate: f64, p: AptParams) -> Vec<Complex32> {
        assert_eq!(words.len() % WORDS_PER_LINE, 0);
        let sps = device_rate / WORD_RATE; // device samples per word
        let mut phase = 0.0f64;
        let mut sc_phase = 0.0f64;
        let mut out = Vec::with_capacity((words.len() as f64 * sps) as usize + 16);
        let mut t = 0.0f64;
        let mut wi = 0usize;
        let total = (words.len() as f64 * sps) as usize;
        for _ in 0..total {
            let subcarrier = sc_phase.sin() * words[wi.min(words.len() - 1)] as f64;
            sc_phase += 2.0 * PI * p.subcarrier_hz / device_rate;
            let inst_freq = p.lo_offset_hz + p.deviation_hz * subcarrier;
            phase += 2.0 * PI * inst_freq / device_rate;
            out.push(Complex32::new(phase.cos() as f32, phase.sin() as f32));
            t += 1.0;
            if t >= sps {
                t -= sps;
                wi += 1;
            }
        }
        out
    }

    /// A full 2080-word line: correct sync/space patterns, a distinctive
    /// ramp in each channel's image region so we can check pixel recovery.
    fn make_test_line() -> Vec<f32> {
        let mut w = vec![0.3f32; WORDS_PER_LINE]; // "telemetry"/space background
        let tmpl = sync_a_template();
        for i in 0..SYNC_LEN {
            w[i] = 0.5 + 0.5 * tmpl[i];
            w[HALF_LINE + i] = 0.5 + 0.5 * tmpl[i]; // reuse as Sync B stand-in
        }
        for k in 0..IMAGE_LEN {
            w[IMAGE_A_START + k] = k as f32 / IMAGE_LEN as f32; // ramp 0..1
            w[IMAGE_B_START + k] = 1.0 - k as f32 / IMAGE_LEN as f32; // ramp 1..0
        }
        w
    }

    #[test]
    fn recovers_a_synthetic_line() {
        let device_rate = 2_000_000.0;
        let params = AptParams::default();
        let words = make_test_line();
        // Three lines back to back: line 1 completes the cold-start (big
        // window) search; lines 2/3 exercise the steady-state small-window
        // per-line resync, which is what matters for ongoing "am I locked"
        // reporting during a real pass.
        let mut all = words.clone();
        all.extend(words.clone());
        all.extend(words.clone());
        let iq = synth_line_iq(&all, device_rate, params);

        let mut demod = AptDemod::new(device_rate, params);
        let mut lines = Vec::new();
        for block in iq.chunks(4096) {
            demod.process(block, |l| lines.push(l));
        }

        assert!(lines.len() >= 2, "expected at least two decoded lines, got {}", lines.len());
        let l = &lines[0];
        // Ramp should be monotonic-ish and span most of the 0..255 range.
        assert!(l.image_a[0] < 60, "image_a start {}", l.image_a[0]);
        assert!(l.image_a[IMAGE_LEN - 1] > 195, "image_a end {}", l.image_a[IMAGE_LEN - 1]);
        assert!(l.image_b[0] > 195, "image_b start {}", l.image_b[0]);
        assert!(l.image_b[IMAGE_LEN - 1] < 60, "image_b end {}", l.image_b[IMAGE_LEN - 1]);
        eprintln!(
            "sync_quality: line0 (cold start) = {}, line1 (steady-state resync) = {}",
            lines[0].sync_quality, lines[1].sync_quality
        );
        // The steady-state per-line resync (small search window) is the
        // meaningful, comparable-across-runs figure; require a clear lock.
        // (Empirically: real signal ~8, noise's steady-state ceiling ~3.2 —
        // see `noise_produces_no_crash_and_low_sync_quality`. 5.0 sits
        // comfortably between the two with margin on both sides.)
        assert!(lines[1].sync_quality > 5.0, "sync_quality {}", lines[1].sync_quality);
    }

    #[test]
    fn noise_produces_no_crash_and_low_sync_quality() {
        let device_rate = 2_000_000.0;
        let mut seed = 0xC0FFEEu32;
        let iq: Vec<_> = (0..3_000_000)
            .map(|_| {
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                let a = (seed >> 9) as f32 / 8_388_608.0 - 1.0;
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                let b = (seed >> 9) as f32 / 8_388_608.0 - 1.0;
                Complex32::new(a * 0.05, b * 0.05)
            })
            .collect();
        let mut demod = AptDemod::new(device_rate, AptParams::default());
        let mut got: Vec<f32> = Vec::new();
        for block in iq.chunks(4096) {
            demod.process(block, |l| got.push(l.sync_quality));
        }
        eprintln!("sync_quality (noise, {} lines) = {:?}", got.len(), got);
        // Skip line 0: its cold-start search covers a much wider window, so
        // even pure noise's peak-of-many-random-draws can look inflated
        // there (order-statistics, not a real lock) — see module notes.
        // From line 1 on, the steady-state small-window resync must stay
        // well below what a real Sync-A pulse produces (compare
        // recovers_a_synthetic_line's threshold of 5.0; measured noise
        // ceiling here is ~3.2 across many synthetic lines).
        for &q in got.iter().skip(1) {
            assert!(q < 5.0, "noise looked locked in steady state: {q}");
        }
    }
}
