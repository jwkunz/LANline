//! FM receive DSP: IQ at the device sample rate → mono audio at 48 kHz.
//! Handles both narrowband (NWR voice, ~16 kHz channel) and wideband
//! (broadcast, ~200 kHz channel) FM — the chain is the same, only the
//! constants scale with `channel_bw_hz`.
//!
//! Chain: digital LO offset (dodge the ZIF DC spike) → windowed-sinc FIR
//! decimation to the channel rate → polar FM discriminator → de-emphasis →
//! audio low-pass → noise squelch → linear resample to 48 kHz.

use num_complex::Complex32;
use std::f64::consts::PI;

const AUDIO_RATE: f64 = 48_000.0;

#[derive(Clone, Copy, Debug)]
pub struct FmParams {
    pub deviation_hz: f64,
    pub channel_bw_hz: f64,
    pub deemphasis_us: f64,
    pub audio_lpf_hz: f64,
    /// RSSI floor (dBFS): gates "no antenna / dead band".
    pub squelch_dbfs: f64,
    /// Noise-squelch threshold: HF-noise energy in the discriminator output
    /// above which the channel is treated as unoccupied. Amplitude-normalized,
    /// so it is independent of RF gain and deviation. ~0.02 (tight) .. ~2.0
    /// (open).
    pub noise_squelch: f64,
    pub lo_offset_hz: f64,
}

impl Default for FmParams {
    fn default() -> Self {
        Self {
            deviation_hz: 5_000.0,
            channel_bw_hz: 16_000.0,
            deemphasis_us: 75.0,
            audio_lpf_hz: 3_400.0,
            squelch_dbfs: -80.0,
            noise_squelch: 0.18,
            lo_offset_hz: 250_000.0,
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct ChainMetrics {
    pub rssi_dbfs: f32,
    pub snr_db: f32,
    pub squelch_open: bool,
    pub audio_dbfs: f32,
}

pub struct FmChain {
    nco: Nco,
    decim: FirDecimator,
    channel_rate: f64,
    prev_iq: Complex32,
    disc_gain: f32,
    deemph: OnePole,
    audio_lp: Biquad,
    pilot_notch: Option<Biquad>,
    noise_hp: Biquad,
    noise_env: f32,
    noise_gate: f32,
    squelch_thresh_dbfs: f32,
    squelch_gain: f32,
    sig_env: f32,
    metrics: ChainMetrics,
    resamp: LinearResampler,
    scratch_iq: Vec<Complex32>,
    scratch_audio: Vec<f32>,
}

impl FmChain {
    pub fn new(device_rate: f64, p: FmParams) -> Self {
        // Decimate to a channel rate comfortably above the occupied bandwidth:
        // ~48 kHz for NWR voice, ~300+ kHz for broadcast FM.
        let target = (p.channel_bw_hz * 1.6).max(48_000.0);
        let decim = ((device_rate / target).floor() as usize).max(1);
        let channel_rate = device_rate / decim as f64;

        // Channel filter: pass the Carson bandwidth with margin, reject the
        // adjacent channel.
        let cutoff = (p.channel_bw_hz * 0.6).clamp(6_000.0, channel_rate / 2.0 * 0.9);
        let num_taps = (16 * decim + 1).min(1023);

        let disc_gain = (channel_rate / (2.0 * PI * p.deviation_hz.max(1.0))) as f32;
        let deemph = if p.deemphasis_us > 0.0 {
            OnePole::lowpass_tau(channel_rate, p.deemphasis_us * 1e-6)
        } else {
            OnePole::passthrough()
        };
        let audio_lp = Biquad::lowpass(channel_rate, p.audio_lpf_hz.min(channel_rate / 2.5), 0.707);
        // Broadcast FM: notch the 19 kHz stereo pilot out of the mono sum so it
        // isn't audible as a whistle (the audio LPF alone doesn't kill it).
        let pilot_notch = (p.channel_bw_hz > 50_000.0 && channel_rate > 45_000.0)
            .then(|| Biquad::notch(channel_rate, 19_000.0, 12.0));
        // Noise squelch: energy just above the audio band in the raw
        // discriminator output. Amplitude-normalized (it works on `arg`), so
        // the threshold is independent of RF gain and deviation. Kept above the
        // audio LPF so program content never trips it.
        let noise_hp = Biquad::highpass(
            channel_rate,
            (p.audio_lpf_hz * 1.3).clamp(4_000.0, channel_rate * 0.45),
            0.707,
        );

        Self {
            nco: Nco::new(-p.lo_offset_hz / device_rate),
            decim: FirDecimator::new(decim, cutoff, device_rate, num_taps),
            channel_rate,
            prev_iq: Complex32::new(1.0, 0.0),
            disc_gain,
            deemph,
            audio_lp,
            pilot_notch,
            noise_hp,
            noise_env: 1.0,
            noise_gate: (p.noise_squelch as f32).clamp(0.01, 2.0),
            squelch_thresh_dbfs: p.squelch_dbfs as f32,
            squelch_gain: 0.0,
            sig_env: 0.0,
            metrics: ChainMetrics::default(),
            resamp: LinearResampler::new(channel_rate, AUDIO_RATE),
            scratch_iq: Vec::new(),
            scratch_audio: Vec::new(),
        }
    }

    pub fn channel_rate(&self) -> f64 {
        self.channel_rate
    }

    pub fn metrics(&self) -> ChainMetrics {
        self.metrics
    }

    /// Retune-time housekeeping: drop the discriminator memory so the
    /// frequency step doesn't produce a click.
    pub fn on_retune(&mut self) {
        self.prev_iq = Complex32::new(1.0, 0.0);
        self.noise_env = 1.0;
    }

    /// Consume a block of device-rate IQ, append 48 kHz mono audio to `out`.
    pub fn process(&mut self, iq: &[Complex32], out: &mut Vec<f32>) {
        // 1. digital down-conversion by the LO offset
        self.scratch_iq.clear();
        self.scratch_iq.reserve(iq.len() / self.decim.decim + 4);
        self.nco.mix_into(iq, &mut self.decim, &mut self.scratch_iq);
        let chan = &self.scratch_iq;
        if chan.is_empty() {
            return;
        }

        // 2. block RSSI + squelch decision. RSSI gates "no antenna / dead
        //    band"; the noise metric (from the previous block) gates hiss.
        let power: f32 =
            chan.iter().map(|c| c.norm_sqr()).sum::<f32>() / chan.len() as f32;
        let rssi_dbfs = 10.0 * (power + 1e-12).log10();
        let open = rssi_dbfs >= self.squelch_thresh_dbfs && self.noise_env < self.noise_gate;
        let target = if open { 1.0 } else { 0.0 };
        // ~5 ms audio ramp, ~10 ms noise envelope
        let ramp = (1.0 / (0.005 * self.channel_rate as f32)).min(1.0);
        let nk = (1.0 / (0.010 * self.channel_rate as f32)).min(1.0);

        // 3. discriminator → de-emphasis → audio LPF → squelch gate
        self.scratch_audio.clear();
        self.scratch_audio.reserve(chan.len());
        let mut peak = 0.0f32;
        for &x in chan {
            let prod = x * self.prev_iq.conj();
            self.prev_iq = x;
            let raw = prod.im.atan2(prod.re);
            let hf = self.noise_hp.process(raw);
            self.noise_env += (hf.abs() - self.noise_env) * nk;

            let mut a = raw * self.disc_gain;
            if let Some(notch) = &mut self.pilot_notch {
                a = notch.process(a);
            }
            a = self.deemph.process(a);
            a = self.audio_lp.process(a);
            // pre-gate voice-band envelope, for the SNR estimate
            self.sig_env += (a.abs() - self.sig_env) * nk;
            self.squelch_gain += (target - self.squelch_gain) * ramp;
            a *= self.squelch_gain;
            peak = peak.max(a.abs());
            self.scratch_audio.push(a.clamp(-1.0, 1.0));
        }

        // 4. resample channel rate → 48 kHz
        self.resamp.process(&self.scratch_audio, out);

        let snr_db = 20.0 * ((self.sig_env + 1e-6) / (self.noise_env + 1e-6)).log10();
        self.metrics = ChainMetrics {
            rssi_dbfs,
            snr_db,
            squelch_open: open,
            audio_dbfs: 20.0 * (peak + 1e-6).log10(),
        };
    }
}

// --- building blocks -----------------------------------------------------

/// Numerically-controlled oscillator that mixes a block and immediately feeds
/// the decimator, so we never materialize the full-rate mixed signal. The
/// phasor advances by one complex multiply per sample (no per-sample trig),
/// renormalized periodically to shed accumulated error.
struct Nco {
    rot: Complex32,
    step: Complex32,
    n: u32,
}

impl Nco {
    fn new(cycles_per_sample: f64) -> Self {
        let a = 2.0 * PI * cycles_per_sample;
        Self {
            rot: Complex32::new(1.0, 0.0),
            step: Complex32::new(a.cos() as f32, a.sin() as f32),
            n: 0,
        }
    }

    fn mix_into(&mut self, input: &[Complex32], decim: &mut FirDecimator, out: &mut Vec<Complex32>) {
        for &x in input {
            decim.push(x * self.rot, out);
            self.rot *= self.step;
            self.n += 1;
            if self.n >= 8192 {
                self.n = 0;
                let m = self.rot.norm();
                if m > 1e-6 {
                    self.rot /= m;
                }
            }
        }
    }
}

/// Windowed-sinc FIR low-pass with integer decimation. Ring-buffered history,
/// output computed only on decimated phases.
pub struct FirDecimator {
    taps: Vec<f32>,
    hist: Vec<Complex32>,
    widx: usize,
    decim: usize,
    counter: usize,
}

impl FirDecimator {
    pub fn new(decim: usize, cutoff_hz: f64, fs_in: f64, num_taps: usize) -> Self {
        let n = num_taps | 1; // force odd for a symmetric linear-phase filter
        let fc = cutoff_hz / fs_in; // normalized (cycles/sample), 0..0.5
        let mid = (n - 1) as f64 / 2.0;
        let mut taps = vec![0.0f32; n];
        let mut sum = 0.0f64;
        for (k, tap) in taps.iter_mut().enumerate() {
            let m = k as f64 - mid;
            let sinc = if m.abs() < 1e-9 {
                2.0 * fc
            } else {
                (2.0 * PI * fc * m).sin() / (PI * m)
            };
            // Blackman window
            let w = 0.42 - 0.5 * (2.0 * PI * k as f64 / (n - 1) as f64).cos()
                + 0.08 * (4.0 * PI * k as f64 / (n - 1) as f64).cos();
            let v = sinc * w;
            *tap = v as f32;
            sum += v;
        }
        let norm = (1.0 / sum) as f32;
        for tap in &mut taps {
            *tap *= norm;
        }
        Self {
            taps,
            hist: vec![Complex32::new(0.0, 0.0); n],
            widx: 0,
            decim,
            counter: 0,
        }
    }

    #[inline]
    pub fn push(&mut self, x: Complex32, out: &mut Vec<Complex32>) {
        let n = self.hist.len();
        self.hist[self.widx] = x;
        self.widx = (self.widx + 1) % n;
        self.counter += 1;
        if self.counter < self.decim {
            return;
        }
        self.counter = 0;
        // newest sample is at widx-1
        let mut acc = Complex32::new(0.0, 0.0);
        let mut idx = (self.widx + n - 1) % n;
        for &h in &self.taps {
            let s = self.hist[idx];
            acc.re += h * s.re;
            acc.im += h * s.im;
            idx = if idx == 0 { n - 1 } else { idx - 1 };
        }
        out.push(acc);
    }
}

/// One-pole low-pass, used for de-emphasis.
pub struct OnePole {
    a: f32,
    y: f32,
}

impl OnePole {
    pub fn lowpass_tau(fs: f64, tau_s: f64) -> Self {
        Self { a: (1.0 / (1.0 + fs * tau_s)) as f32, y: 0.0 }
    }
    pub fn passthrough() -> Self {
        Self { a: 1.0, y: 0.0 }
    }
    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        self.y += self.a * (x - self.y);
        self.y
    }
}

/// RBJ biquad, low-pass configuration.
pub struct Biquad {
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
    pub fn lowpass(fs: f64, fc: f64, q: f64) -> Self {
        let (cs, alpha) = Self::prewarp(fs, fc, q);
        let b1 = 1.0 - cs;
        let b0 = b1 / 2.0;
        Self::normalize(b0, b1, b0, 1.0 + alpha, -2.0 * cs, 1.0 - alpha)
    }

    pub fn highpass(fs: f64, fc: f64, q: f64) -> Self {
        let (cs, alpha) = Self::prewarp(fs, fc, q);
        let b0 = (1.0 + cs) / 2.0;
        Self::normalize(b0, -(1.0 + cs), b0, 1.0 + alpha, -2.0 * cs, 1.0 - alpha)
    }

    pub fn notch(fs: f64, fc: f64, q: f64) -> Self {
        let (cs, alpha) = Self::prewarp(fs, fc, q);
        Self::normalize(1.0, -2.0 * cs, 1.0, 1.0 + alpha, -2.0 * cs, 1.0 - alpha)
    }

    fn prewarp(fs: f64, fc: f64, q: f64) -> (f64, f64) {
        let w0 = 2.0 * PI * (fc / fs);
        let (sn, cs) = w0.sin_cos();
        (cs, sn / (2.0 * q))
    }

    #[allow(clippy::too_many_arguments)]
    fn normalize(b0: f64, b1: f64, b2: f64, a0: f64, a1: f64, a2: f64) -> Self {
        Self {
            b0: (b0 / a0) as f32,
            b1: (b1 / a0) as f32,
            b2: (b2 / a0) as f32,
            a1: (a1 / a0) as f32,
            a2: (a2 / a0) as f32,
            x1: 0.0,
            x2: 0.0,
            y1: 0.0,
            y2: 0.0,
        }
    }
    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        let y = self.b0 * x + self.b1 * self.x1 + self.b2 * self.x2
            - self.a1 * self.y1
            - self.a2 * self.y2;
        self.x2 = self.x1;
        self.x1 = x;
        self.y2 = self.y1;
        self.y1 = y;
        y
    }
}

/// Streaming linear-interpolation resampler (fs_in → fs_out). Adequate for
/// band-limited voice at ratios near 1:1 (e.g. 50 k → 48 k).
pub struct LinearResampler {
    step: f64, // input samples per output sample
    next_t: f64,
    last: f32,
    primed: bool,
}

impl LinearResampler {
    pub fn new(fs_in: f64, fs_out: f64) -> Self {
        Self { step: fs_in / fs_out, next_t: 0.0, last: 0.0, primed: false }
    }

    pub fn process(&mut self, input: &[f32], out: &mut Vec<f32>) {
        if input.is_empty() {
            return;
        }
        if !self.primed {
            self.last = input[0];
            self.primed = true;
        }
        let len = input.len();
        while self.next_t <= (len - 1) as f64 {
            let idx = self.next_t.floor();
            let f = (self.next_t - idx) as f32;
            let i = idx as isize;
            let a = if i < 0 { self.last } else { input[i as usize] };
            let b = input[(i + 1) as usize];
            out.push(a + (b - a) * f);
            self.next_t += self.step;
        }
        self.next_t -= len as f64;
        self.last = input[len - 1];
    }
}

/// Reference NBFM modulator, used only by tests.
#[cfg(test)]
pub fn fm_modulate(
    fs: f64,
    n: usize,
    tone_hz: f64,
    deviation_hz: f64,
    lo_offset_hz: f64,
) -> Vec<Complex32> {
    let mut phase = 0.0f64;
    let mut msg_phase = 0.0f64;
    let dt = 1.0 / fs;
    let mut out = Vec::with_capacity(n);
    for _ in 0..n {
        let msg = (2.0 * PI * msg_phase).sin();
        msg_phase += tone_hz * dt;
        let inst = lo_offset_hz + deviation_hz * msg;
        phase += 2.0 * PI * inst * dt;
        out.push(Complex32::new(phase.cos() as f32, phase.sin() as f32));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn goertzel(x: &[f32], fs: f64, target: f64) -> f32 {
        let k = (0.5 + (x.len() as f64 * target / fs)).floor();
        let w = 2.0 * PI * k / x.len() as f64;
        let cw = 2.0 * w.cos();
        let (mut s1, mut s2) = (0.0f64, 0.0f64);
        for &v in x {
            let s0 = v as f64 + cw * s1 - s2;
            s2 = s1;
            s1 = s0;
        }
        let power = s1 * s1 + s2 * s2 - cw * s1 * s2;
        (power.max(0.0).sqrt() / (x.len() as f64 / 2.0)) as f32
    }

    #[test]
    fn recovers_modulating_tone() {
        let fs = 2_000_000.0;
        let tone = 1_200.0;
        let params = FmParams { deemphasis_us: 0.0, ..FmParams::default() };
        let iq = fm_modulate(fs, 800_000, tone, params.deviation_hz, params.lo_offset_hz);
        let mut chain = FmChain::new(fs, params);
        let mut audio = Vec::new();
        for block in iq.chunks(8192) {
            chain.process(block, &mut audio);
        }

        assert!(audio.len() > 6_000, "got {} samples", audio.len());
        let tail = &audio[audio.len() / 2..];
        let at_tone = goertzel(tail, AUDIO_RATE, tone as f64);
        let off_tone = goertzel(tail, AUDIO_RATE, 400.0);
        assert!(at_tone > 0.15, "tone amplitude {at_tone}");
        assert!(at_tone > off_tone * 5.0, "tone {at_tone} vs off {off_tone}");
        assert!(chain.metrics().squelch_open, "squelch should open on a carrier");
    }

    #[test]
    fn recovers_wideband_tone() {
        // Broadcast-FM-shaped: 4 Msps, ±75 kHz deviation, 6 kHz tone.
        let fs = 4_000_000.0;
        let tone = 6_000.0;
        let params = FmParams {
            deviation_hz: 75_000.0,
            channel_bw_hz: 200_000.0,
            audio_lpf_hz: 15_000.0,
            deemphasis_us: 0.0,
            noise_squelch: 2.0,
            squelch_dbfs: -120.0,
            ..FmParams::default()
        };
        let iq = fm_modulate(fs, 1_600_000, tone, params.deviation_hz, params.lo_offset_hz);
        let mut chain = FmChain::new(fs, params);
        let mut audio = Vec::new();
        for block in iq.chunks(16384) {
            chain.process(block, &mut audio);
        }
        let tail = &audio[audio.len() / 2..];
        let at_tone = goertzel(tail, AUDIO_RATE, tone as f64);
        let off_tone = goertzel(tail, AUDIO_RATE, 1_000.0);
        assert!(at_tone > 0.1, "wideband tone amplitude {at_tone}");
        assert!(at_tone > off_tone * 5.0, "tone {at_tone} vs off {off_tone}");
    }

    #[test]
    fn squelch_stays_closed_on_noise() {
        let fs = 2_000_000.0;
        let params = FmParams { squelch_dbfs: -40.0, ..FmParams::default() };
        let mut state = 12345u64;
        let noise: Vec<Complex32> = (0..400_000)
            .map(|_| {
                state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
                let r = ((state >> 33) as f32 / u32::MAX as f32 - 0.5) * 0.002;
                state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
                let i = ((state >> 33) as f32 / u32::MAX as f32 - 0.5) * 0.002;
                Complex32::new(r, i)
            })
            .collect();
        let mut chain = FmChain::new(fs, params);
        let mut audio = Vec::new();
        for block in noise.chunks(8192) {
            chain.process(block, &mut audio);
        }
        assert!(!chain.metrics().squelch_open, "rssi {}", chain.metrics().rssi_dbfs);
        let peak = audio.iter().fold(0.0f32, |m, &s| m.max(s.abs()));
        assert!(peak < 0.05, "muted audio peak {peak}");
    }
}
