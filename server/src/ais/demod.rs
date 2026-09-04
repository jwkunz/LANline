//! AIS physical layer: per-channel GMSK receive at 9600 baud.
//!
//! chain: digital down-convert (NCO) → decimating FIR low-pass → polar FM
//! discriminator → slow DC block → boxcar matched filter → zero-crossing
//! symbol-timing sync → hard slice → NRZI decode → HDLC deframe / de-stuff /
//! FCS. GMSK with h≈0.5 demodulates cleanly through an FM discriminator, the
//! same idea as the FM audio chain.
//!
//! Note: the symbol sync is a lightweight hard-lock-then-track design, verified
//! against synthesised GMSK bursts (incl. carrier offset). On-air performance
//! against weak / faded signals is not yet field-validated.

use super::message::fcs_ok;
use num_complex::Complex32;
use std::f64::consts::PI;

pub const BAUD: f64 = 9600.0;
const DECIM: usize = 10;
const FIR_TAPS: usize = 121;
const FIR_CUTOFF_HZ: f64 = 7_000.0;

/// A validated AIS frame recovered from one channel.
#[derive(Debug, Clone)]
pub struct RawSentence {
    /// 'A' (161.975) or 'B' (162.025).
    pub channel: char,
    /// Big-endian AIS payload octets (FCS already stripped and verified).
    pub payload: Vec<u8>,
    pub bit_len: usize,
    pub rssi_dbfs: f32,
}

// --- primitives -----------------------------------------------------------

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

/// Windowed-sinc decimating FIR low-pass over complex input.
struct Fir {
    taps: Vec<f32>,
    hist: Vec<Complex32>,
    widx: usize,
    counter: usize,
}
impl Fir {
    fn new(cutoff_hz: f64, fs_in: f64, num_taps: usize) -> Self {
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
        Self { taps, hist: vec![Complex32::new(0.0, 0.0); n], widx: 0, counter: 0 }
    }
    #[inline]
    fn push(&mut self, x: Complex32) -> Option<Complex32> {
        let n = self.hist.len();
        self.hist[self.widx] = x;
        self.widx = (self.widx + 1) % n;
        self.counter += 1;
        if self.counter < DECIM {
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

/// Symbol-timing recovery on the FM-discriminated real signal. Data-transition
/// zero-crossings mark symbol boundaries: hard-align to the first strong
/// crossing, then track subsequent ones gently. Emits one interpolated sample
/// per symbol at the symbol centre.
struct SymSync {
    sps: f64,
    phase: f64, // 0..sps, 0 == symbol boundary
    prev: f32,
    gain: f64,
    scale: f32,
    synced: bool,
}
impl SymSync {
    fn new(fs: f64) -> Self {
        Self { sps: fs / BAUD, phase: 0.0, prev: 0.0, gain: 0.02, scale: 0.1, synced: false }
    }
    #[inline]
    fn push(&mut self, x: f32) -> Option<f32> {
        self.scale += 0.001 * (x.abs() - self.scale);
        let inv = 1.0 / self.scale.max(1e-4);
        let half = self.sps / 2.0;
        let p0 = self.phase; // phase of `self.prev`; `x` sits at `self.phase + 1`.

        // Data-transition zero-crossings mark symbol boundaries (phase 0 ≡ sps).
        let crossing = (self.prev >= 0.0) != (x >= 0.0)
            && (self.prev.abs() > 0.02 || x.abs() > 0.02);
        if crossing {
            let frac = (self.prev.abs() / (self.prev.abs() + x.abs()).max(1e-9)) as f64;
            let cross = p0 + frac;
            let err = if cross <= half { cross } else { cross - self.sps };
            if !self.synced {
                // hard-align to the first real crossing, then track gently
                self.phase -= err;
                self.synced = true;
            } else {
                self.phase -= self.gain * err.clamp(-2.0, 2.0);
            }
            while self.phase < 0.0 {
                self.phase += self.sps;
            }
        }

        let sym = if p0 <= half && half < p0 + 1.0 {
            let t = (half - p0) as f32;
            Some((self.prev + (x - self.prev) * t) * inv)
        } else {
            None
        };

        self.prev = x;
        self.phase += 1.0;
        while self.phase >= self.sps {
            self.phase -= self.sps;
        }
        sym
    }
}

/// Boxcar matched filter — a moving average roughly one symbol long cleans the
/// GMSK inter-symbol ringing the FM discriminator leaves behind.
struct Boxcar {
    buf: Vec<f32>,
    idx: usize,
    sum: f32,
}
impl Boxcar {
    fn new(len: usize) -> Self {
        Self { buf: vec![0.0; len.max(1)], idx: 0, sum: 0.0 }
    }
    #[inline]
    fn push(&mut self, x: f32) -> f32 {
        self.sum += x - self.buf[self.idx];
        self.buf[self.idx] = x;
        self.idx = (self.idx + 1) % self.buf.len();
        self.sum / self.buf.len() as f32
    }
}

/// NRZI decode + HDLC deframe / de-stuff / FCS.
struct Hdlc {
    prev_level: bool,
    // deframer
    in_frame: bool,
    hunt: u8,      // last 8 raw (post-NRZI-decode? no: pre) bits for flag detection
    ones: u8,      // consecutive ones since frame start (for de-stuff / end flag)
    bits: Vec<bool>,
}
impl Hdlc {
    fn new() -> Self {
        Self { prev_level: false, in_frame: false, hunt: 0, ones: 0, bits: Vec::with_capacity(1200) }
    }

    /// Feed one (zero-mean) symbol sample; returns the frame's data bits
    /// (payload+FCS, wire order) when a good end flag is seen.
    fn push(&mut self, sample: f32) -> Option<Vec<bool>> {
        let level = sample >= 0.0;
        // NRZI: no transition -> 1, transition -> 0
        let bit = level == self.prev_level;
        self.prev_level = level;

        // Rolling window for flag detection while hunting.
        self.hunt = (self.hunt << 1) | bit as u8;

        if !self.in_frame {
            if self.hunt == 0x7E {
                self.in_frame = true;
                self.ones = 0;
                self.bits.clear();
            }
            return None;
        }

        // In-frame: watch for stuffing bit / end flag / abort.
        if bit {
            self.ones += 1;
            self.bits.push(true);
            if self.ones >= 7 {
                // aborted frame
                self.in_frame = false;
                return None;
            }
            return None;
        }

        // bit == 0
        if self.ones == 5 {
            // stuffed zero -> drop it
            self.ones = 0;
            return None;
        }
        if self.ones == 6 {
            // We pushed the closing flag's leading 0 + its six 1s as data; the
            // whole 0x7E flag (this trailing 0 included) is not frame content.
            let n = self.bits.len().saturating_sub(7);
            self.bits.truncate(n);
            self.in_frame = false;
            let frame = std::mem::take(&mut self.bits);
            return Some(frame);
        }
        self.ones = 0;
        self.bits.push(false);
        None
    }
}

/// One AIS channel: everything from IQ to validated frames.
pub struct ChannelDemod {
    label: char,
    nco: Nco,
    fir: Fir,
    prev_iq: Complex32,
    dc: f32,
    mf: Boxcar,
    sync: SymSync,
    hdlc: Hdlc,
    mag_ema: f32,
}

impl ChannelDemod {
    pub fn new(device_rate: f64, freq_offset_hz: f64, label: char) -> Self {
        let ch_rate = device_rate / DECIM as f64;
        let mf_len = ((ch_rate / BAUD) * 0.75).round() as usize;
        Self {
            label,
            nco: Nco::new(-freq_offset_hz / device_rate),
            fir: Fir::new(FIR_CUTOFF_HZ, device_rate, FIR_TAPS),
            prev_iq: Complex32::new(0.0, 0.0),
            dc: 0.0,
            mf: Boxcar::new(mf_len),
            sync: SymSync::new(ch_rate),
            hdlc: Hdlc::new(),
            mag_ema: 1e-6,
        }
    }

    pub fn channel_rate(&self) -> f64 {
        self.sync.sps * BAUD
    }

    pub fn process<F: FnMut(RawSentence)>(&mut self, iq: &[Complex32], mut on_frame: F) {
        for &x in iq {
            let mixed = self.nco.advance(x);
            let Some(s) = self.fir.push(mixed) else { continue };

            self.mag_ema += 0.001 * (s.norm() - self.mag_ema);

            // polar FM discriminator
            let d = s * self.prev_iq.conj();
            self.prev_iq = s;
            let disc = d.im.atan2(d.re);

            // remove the static carrier-frequency offset only (slow, so it does
            // not eat multi-symbol NRZI runs), then matched-filter.
            self.dc += 0.0008 * (disc - self.dc);
            let mf = self.mf.push(disc - self.dc);

            let Some(sym) = self.sync.push(mf) else { continue };
            if let Some(frame) = self.hdlc.push(sym) {
                if let Some(raw) = validate(frame, self.label, self.mag_ema) {
                    on_frame(raw);
                }
            }
        }
    }
}

/// wire-order data bits -> octets (each byte reconstructed LSB-first) -> FCS
/// check -> payload octets without the trailing FCS.
fn validate(frame: Vec<bool>, channel: char, mag: f32) -> Option<RawSentence> {
    if frame.len() < 40 || frame.len() % 8 != 0 || frame.len() > 1200 {
        return None;
    }
    let mut octets = vec![0u8; frame.len() / 8];
    for (i, &b) in frame.iter().enumerate() {
        if b {
            octets[i / 8] |= 1 << (i % 8);
        }
    }
    if !fcs_ok(&octets) {
        return None;
    }
    let bit_len = frame.len() - 16;
    octets.truncate(octets.len() - 2);
    let rssi_dbfs = (20.0 * mag.max(1e-6).log10()).clamp(-90.0, 0.0);
    Some(RawSentence { channel, payload: octets, bit_len, rssi_dbfs })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ais::message::{unarmor, AisMessage, Bits};

    /// Build the wire bit stream (training + flag + NRZI/stuffed data + FCS +
    /// flag) for a big-endian AIS payload, then modulate it as MSK IQ at
    /// `device_rate`.
    fn synth_iq(payload_be: &[u8], nbits: usize, device_rate: f64, freq_off: f64) -> Vec<Complex32> {
        // 1. octets (LSB-first on the wire) + X.25 FCS
        let mut wire: Vec<bool> = Vec::new();
        let octet_count = nbits.div_ceil(8);
        let mut octets: Vec<u8> = (0..octet_count).map(|i| payload_be[i]).collect();
        let mut crc: u16 = 0xFFFF;
        for &b in &octets {
            crc ^= b as u16;
            for _ in 0..8 {
                crc = if crc & 1 != 0 { (crc >> 1) ^ 0x8408 } else { crc >> 1 };
            }
        }
        let fcs = !crc;
        octets.push((fcs & 0xff) as u8);
        octets.push((fcs >> 8) as u8);
        for &o in &octets {
            for k in 0..8 {
                wire.push((o >> k) & 1 == 1); // LSB first
            }
        }

        // 2. NRZI encode + bit stuffing + HDLC flags + training
        let mut level = false;
        let mut chan: Vec<bool> = Vec::new();
        let emit = |b: bool, chan: &mut Vec<bool>, level: &mut bool| {
            if !b {
                *level = !*level; // 0 -> transition
            }
            chan.push(*level);
        };
        // 24-bit training 0101...
        for i in 0..24 {
            emit(i % 2 == 1, &mut chan, &mut level);
        }
        // start flag 0x7E, sent 0 1 1 1 1 1 1 0
        for &b in &[false, true, true, true, true, true, true, false] {
            emit(b, &mut chan, &mut level);
        }
        // data with stuffing
        let mut ones = 0;
        for &b in &wire {
            emit(b, &mut chan, &mut level);
            if b {
                ones += 1;
                if ones == 5 {
                    emit(false, &mut chan, &mut level);
                    ones = 0;
                }
            } else {
                ones = 0;
            }
        }
        // end flag
        for &b in &[false, true, true, true, true, true, true, false] {
            emit(b, &mut chan, &mut level);
        }
        for _ in 0..24 {
            emit(true, &mut chan, &mut level);
        }

        // 3. GMSK modulate: per-sample frequency ±BAUD/4, Gaussian-smoothed to
        //    add realistic inter-symbol interference, phase integrated.
        let sps = device_rate / BAUD;
        let dev = BAUD / 4.0;
        let nsamp = (chan.len() as f64 * sps) as usize + 8;
        let mut freq = vec![0.0f64; nsamp];
        for (i, f) in freq.iter_mut().enumerate() {
            let sidx = ((i as f64) / sps) as usize;
            *f = if *chan.get(sidx).unwrap_or(&true) { dev } else { -dev };
        }
        // Gaussian pulse-shaping (~0.35 symbol sigma ≈ BT 0.4).
        let sigma = sps * 0.35;
        let half = (sigma * 2.5) as isize;
        let kernel: Vec<f64> =
            (-half..=half).map(|k| (-(k as f64).powi(2) / (2.0 * sigma * sigma)).exp()).collect();
        let ksum: f64 = kernel.iter().sum();
        let mut phase = 0.0f64;
        let mut out = Vec::with_capacity(nsamp);
        for i in 0..nsamp {
            let mut f = 0.0;
            for (j, &w) in kernel.iter().enumerate() {
                let idx = i as isize + j as isize - half;
                if idx >= 0 && (idx as usize) < nsamp {
                    f += w * freq[idx as usize];
                }
            }
            f = f / ksum + freq_off;
            phase += 2.0 * PI * f / device_rate;
            out.push(Complex32::new(phase.cos() as f32, phase.sin() as f32));
        }
        // lead-in / trail silence
        let mut framed = vec![Complex32::new(0.0, 0.0); 400];
        framed.extend(out);
        framed.extend(std::iter::repeat_n(Complex32::new(0.0, 0.0), 400));
        framed
    }

    #[test]
    fn recovers_synthetic_ais_frame() {
        let (payload, nbits) = unarmor("177KQJ5000G?tO`K>RA1wUbN0TKH", 0).unwrap();
        let iq = synth_iq(&payload, nbits, 2_000_000.0, 0.0);

        let mut ch = ChannelDemod::new(2_000_000.0, 0.0, 'A');
        let mut got: Vec<RawSentence> = Vec::new();
        ch.process(&iq, |r| got.push(r));

        assert_eq!(got.len(), 1, "exactly one frame recovered");
        let r = &got[0];
        assert_eq!(r.bit_len, nbits);
        let b = Bits::new(&r.payload, r.bit_len);
        let m = AisMessage::parse(&b).unwrap();
        assert_eq!(m.mmsi(), 477553000);
    }

    #[test]
    fn recovers_with_offset_and_split_blocks() {
        let (payload, nbits) = unarmor("177KQJ5000G?tO`K>RA1wUbN0TKH", 0).unwrap();
        let iq = synth_iq(&payload, nbits, 2_000_000.0, 350.0); // 350 Hz carrier error

        let mut ch = ChannelDemod::new(2_000_000.0, 0.0, 'B');
        let mut got = 0;
        let cut = iq.len() / 3;
        ch.process(&iq[..cut], |_| got += 1);
        ch.process(&iq[cut..], |_| got += 1);
        assert_eq!(got, 1);
    }

    #[test]
    fn noise_yields_nothing() {
        let mut seed = 0xC0FFEEu32;
        let iq: Vec<_> = (0..200_000)
            .map(|_| {
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                let a = (seed >> 9) as f32 / 8_388_608.0 - 1.0;
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                let b = (seed >> 9) as f32 / 8_388_608.0 - 1.0;
                Complex32::new(a * 0.3, b * 0.3)
            })
            .collect();
        let mut ch = ChannelDemod::new(2_000_000.0, 0.0, 'A');
        let mut got = 0;
        ch.process(&iq, |_| got += 1);
        assert_eq!(got, 0);
    }
}
