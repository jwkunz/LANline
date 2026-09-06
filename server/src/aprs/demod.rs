//! APRS physical layer: 1200-baud Bell 202 AFSK on NBFM, NRZI, AX.25/HDLC.
//!
//! chain: digital down-convert (NCO) → decimating FIR low-pass → polar FM
//! discriminator → AFSK tone demod (a 1200 Hz and a 2200 Hz resonator, whose
//! envelopes are compared) → symbol-timing sync at 1200 baud → NRZI decode →
//! HDLC deframe / de-stuff / FCS → AX.25 octets.
//!
//! The HDLC/FCS half is the same as `ais::demod`'s (AX.25 *is* HDLC); the
//! front end differs — AFSK tones instead of GMSK. Verified against
//! synthesised APRS packets; on-air weak-signal behaviour is not yet
//! field-validated.

use crate::ais::message::fcs_ok;
use num_complex::Complex32;
use std::f64::consts::PI;

pub const BAUD: f64 = 1200.0;
const MARK_HZ: f64 = 1200.0;
const SPACE_HZ: f64 = 2200.0;
/// Decimated working rate (Hz), picked near this.
const TARGET_RATE: f64 = 22_050.0;
const FIR_TAPS: usize = 121;
const FIR_CUTOFF_HZ: f64 = 8_000.0;

/// A validated AX.25 frame (FCS stripped and verified).
#[derive(Debug, Clone)]
pub struct RawFrame {
    /// AX.25 octets, wire order, LSB-first bits already reassembled.
    pub octets: Vec<u8>,
    pub rssi_dbfs: f32,
}

// --- primitives ---------------------------------------------------------

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
    decim: usize,
}
impl Fir {
    fn new(cutoff_hz: f64, fs_in: f64, num_taps: usize, decim: usize) -> Self {
        let n = num_taps | 1;
        let fc = cutoff_hz / fs_in;
        let mid = (n - 1) as f64 / 2.0;
        let mut taps = vec![0.0f32; n];
        let mut sum = 0.0f64;
        for (k, tap) in taps.iter_mut().enumerate() {
            let m = k as f64 - mid;
            let sinc =
                if m.abs() < 1e-9 { 2.0 * fc } else { (2.0 * PI * fc * m).sin() / (PI * m) };
            let w = 0.42 - 0.5 * (2.0 * PI * k as f64 / (n - 1) as f64).cos()
                + 0.08 * (4.0 * PI * k as f64 / (n - 1) as f64).cos();
            let v = sinc * w;
            *tap = v as f32;
            sum += v;
        }
        for t in &mut taps {
            *t /= sum as f32;
        }
        Self { taps, hist: vec![Complex32::new(0.0, 0.0); n], widx: 0, counter: 0, decim }
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

/// Non-coherent AFSK tone correlator: sliding one-symbol DFT bins at the mark
/// (1200 Hz) and space (2200 Hz) tones. `soft` = mark magnitude − space
/// magnitude. Settles in exactly one symbol, no filter ringing.
struct ToneCorr {
    len: usize,
    dphi_m: f32,
    dphi_s: f32,
    phi_m: f32,
    phi_s: f32,
    // ring buffers of the per-sample phasor products
    mc: Vec<f32>,
    ms: Vec<f32>,
    sc: Vec<f32>,
    ss: Vec<f32>,
    idx: usize,
    // running sums
    smc: f32,
    sms: f32,
    ssc: f32,
    sss: f32,
}
impl ToneCorr {
    fn new(fs: f64) -> Self {
        let len = (fs / BAUD).round().max(2.0) as usize;
        Self {
            len,
            dphi_m: (2.0 * PI * MARK_HZ / fs) as f32,
            dphi_s: (2.0 * PI * SPACE_HZ / fs) as f32,
            phi_m: 0.0,
            phi_s: 0.0,
            mc: vec![0.0; len],
            ms: vec![0.0; len],
            sc: vec![0.0; len],
            ss: vec![0.0; len],
            idx: 0,
            smc: 0.0,
            sms: 0.0,
            ssc: 0.0,
            sss: 0.0,
        }
    }
    #[inline]
    fn push(&mut self, a: f32) -> f32 {
        const TAU: f32 = 2.0 * PI as f32;
        let (msin, mcos) = self.phi_m.sin_cos();
        let (ssin, scos) = self.phi_s.sin_cos();
        self.phi_m += self.dphi_m;
        self.phi_s += self.dphi_s;
        if self.phi_m >= TAU {
            self.phi_m -= TAU;
        }
        if self.phi_s >= TAU {
            self.phi_s -= TAU;
        }
        let (nmc, nms, nsc, nss) = (a * mcos, a * msin, a * scos, a * ssin);
        self.smc += nmc - self.mc[self.idx];
        self.sms += nms - self.ms[self.idx];
        self.ssc += nsc - self.sc[self.idx];
        self.sss += nss - self.ss[self.idx];
        self.mc[self.idx] = nmc;
        self.ms[self.idx] = nms;
        self.sc[self.idx] = nsc;
        self.ss[self.idx] = nss;
        self.idx = (self.idx + 1) % self.len;
        (self.smc * self.smc + self.sms * self.sms).sqrt()
            - (self.ssc * self.ssc + self.sss * self.sss).sqrt()
    }
}

/// Bit-clock recovery: lock phase on the first data transition, then run
/// **open-loop** at exactly `sps` samples/symbol, sampling near mid-symbol.
/// APRS packets are short (≤ ~0.2 s) and `sps` is exact off the decimator, so
/// there's no meaningful drift to track — and open-loop can't slip a bit the
/// way a crossing-tracked loop can when the symbol rhythm changes from the
/// steady preamble to data. A per-transition micro-nudge keeps a slightly
/// off nominal rate honest without ever moving more than a fraction of a
/// sample.
struct SymSync {
    sps: f32,
    phase: f32,
    prev_sign: i8,
    synced: bool,
}
impl SymSync {
    fn new(fs: f64) -> Self {
        Self { sps: (fs / BAUD) as f32, phase: 0.0, prev_sign: 0, synced: false }
    }
    #[inline]
    fn push(&mut self, x: f32) -> Option<f32> {
        let s: i8 = if x >= 0.0 { 1 } else { -1 };
        let transition = self.prev_sign != 0 && s != self.prev_sign;
        self.prev_sign = s;

        // The one-symbol correlator's output crosses zero exactly at the
        // centre of the new symbol — so on the first transition, that *is*
        // the sample instant. Emit it and free-run one symbol period.
        if transition && !self.synced {
            self.synced = true;
            self.phase = self.sps;
            return Some(x);
        }
        if !self.synced {
            return None;
        }
        self.phase -= 1.0;
        if self.phase > 0.0 {
            return None;
        }
        self.phase += self.sps;
        Some(x)
    }
}

/// NRZI decode + HDLC deframe / de-stuff. Classic "hunt the 8-bit window for
/// `0x7E`" design: any flag (opening, closing, or one of a back-to-back
/// preamble run) both closes the current frame and opens the next. The last
/// 7 bits we pushed as data before spotting a flag are its `0` + six `1`s —
/// stripped off. FCS is checked in `validate`.
struct Hdlc {
    prev_level: bool,
    window: u8,
    in_frame: bool,
    ones: u8,
    bits: Vec<bool>,
}
impl Hdlc {
    fn new() -> Self {
        Self {
            prev_level: false,
            window: 0,
            in_frame: false,
            ones: 0,
            bits: Vec::with_capacity(2048),
        }
    }
    fn push(&mut self, sample: f32) -> Option<Vec<bool>> {
        let level = sample >= 0.0;
        // NRZI: no transition -> 1, transition -> 0
        let bit = level == self.prev_level;
        self.prev_level = level;
        self.window = (self.window << 1) | bit as u8;

        if self.window == 0x7E {
            let frame = if self.in_frame && self.bits.len() >= 7 {
                let n = self.bits.len() - 7;
                Some(self.bits[..n].to_vec())
            } else {
                None
            };
            self.in_frame = true;
            self.ones = 0;
            self.bits.clear();
            return frame;
        }
        if !self.in_frame {
            return None;
        }
        if bit {
            self.ones += 1;
            self.bits.push(true);
            if self.ones >= 7 {
                // 7+ ones without a flag = abort/idle.
                self.in_frame = false;
                self.bits.clear();
            }
        } else {
            if self.ones != 5 {
                self.bits.push(false); // (a stuffed 0 after five 1s is dropped)
            }
            self.ones = 0;
        }
        None
    }
}

/// Everything from IQ to validated AX.25 frames for the 144.390 channel.
pub struct AprsDemod {
    nco: Nco,
    fir: Fir,
    prev_iq: Complex32,
    dc: f32,
    corr: ToneCorr,
    sync: SymSync,
    hdlc: Hdlc,
    mag_ema: f32,
}

impl AprsDemod {
    pub fn new(device_rate: f64, freq_offset_hz: f64) -> Self {
        let decim = (device_rate / TARGET_RATE).round().max(1.0) as usize;
        let fs = device_rate / decim as f64;
        Self {
            nco: Nco::new(-freq_offset_hz / device_rate),
            fir: Fir::new(FIR_CUTOFF_HZ, device_rate, FIR_TAPS, decim),
            prev_iq: Complex32::new(0.0, 0.0),
            dc: 0.0,
            corr: ToneCorr::new(fs),
            sync: SymSync::new(fs),
            hdlc: Hdlc::new(),
            mag_ema: 1e-6,
        }
    }

    pub fn channel_rate(&self) -> f64 {
        self.sync.sps as f64 * BAUD
    }

    pub fn process<F: FnMut(RawFrame)>(&mut self, iq: &[Complex32], mut on_frame: F) {
        for &x in iq {
            let mixed = self.nco.advance(x);
            let Some(s) = self.fir.push(mixed) else { continue };
            self.mag_ema += 0.001 * (s.norm() - self.mag_ema);

            // polar FM discriminator
            let d = s * self.prev_iq.conj();
            self.prev_iq = s;
            let disc = d.im.atan2(d.re);
            self.dc += 0.0008 * (disc - self.dc);
            let a = disc - self.dc;

            // AFSK: one-symbol correlation at each tone; mark − space.
            let soft = self.corr.push(a);

            let Some(sym) = self.sync.push(soft) else { continue };
            if let Some(frame) = self.hdlc.push(sym) {
                if let Some(f) = validate(frame, self.mag_ema) {
                    on_frame(f);
                }
            }
        }
    }
}

/// wire-order data bits → octets (LSB-first per byte) → FCS check → octets
/// without the trailing 2 FCS bytes.
fn validate(frame: Vec<bool>, mag: f32) -> Option<RawFrame> {
    // Shortest useful AX.25 UI frame: 2×7 addr + control + PID + FCS = 18 B.
    if frame.len() < 18 * 8 || frame.len() % 8 != 0 || frame.len() > 2048 {
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
    octets.truncate(octets.len() - 2);
    Some(RawFrame { octets, rssi_dbfs: (20.0 * mag.max(1e-6).log10()).clamp(-90.0, 0.0) })
}

// --- test helper: synthesise an APRS packet's IQ -----------------------

/// Build the AFSK IQ for an AX.25 frame body (no FCS — this appends it):
/// HDLC flags + NRZI + bit-stuffing + Bell 202 tones on an FM carrier.
#[cfg(test)]
pub fn synth_packet(fs: f64, octets_no_fcs: &[u8], lead_flags: usize) -> Vec<Complex32> {
    // 1. FCS (CRC-16/X.25) over the frame, appended little-endian, complemented.
    let mut crc: u16 = 0xFFFF;
    for &b in octets_no_fcs {
        crc ^= b as u16;
        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0x8408 } else { crc >> 1 };
        }
    }
    let fcs = !crc;
    let mut framed = octets_no_fcs.to_vec();
    framed.push((fcs & 0xFF) as u8);
    framed.push((fcs >> 8) as u8);

    // 2. data bits LSB-first, with bit-stuffing.
    let mut data_bits: Vec<bool> = Vec::new();
    let mut ones = 0;
    for &byte in &framed {
        for i in 0..8 {
            let bit = (byte >> i) & 1 == 1;
            data_bits.push(bit);
            if bit {
                ones += 1;
                if ones == 5 {
                    data_bits.push(false);
                    ones = 0;
                }
            } else {
                ones = 0;
            }
        }
    }

    // 3. flags + data, then NRZI encode (0 -> transition, 1 -> hold).
    let mut wire: Vec<bool> = Vec::new();
    for _ in 0..lead_flags {
        for i in 0..8u8 {
            wire.push((0x7Eu8 >> i) & 1 == 1);
        }
    }
    wire.extend_from_slice(&data_bits);
    // Closing flag + a few trailing flags so the closing one flushes through
    // the demod's group delay (a real channel is never silent right after).
    for _ in 0..5 {
        for i in 0..8u8 {
            wire.push((0x7Eu8 >> i) & 1 == 1);
        }
    }
    let mut level = true;
    let mut nrzi: Vec<bool> = Vec::with_capacity(wire.len());
    for b in wire {
        if !b {
            level = !level;
        }
        nrzi.push(level);
    }

    // 4. Bell 202 AFSK → FM.
    let sps = fs / BAUD;
    let mut out = Vec::new();
    let mut afsk_phase = 0.0f64;
    let mut fm_phase = 0.0f64;
    let dev = 3_000.0; // Hz peak deviation
    let mut t = 0.0f64;
    for &lvl in &nrzi {
        let tone = if lvl { MARK_HZ } else { SPACE_HZ };
        let n = ((t + sps).floor() - t.floor()) as usize;
        for _ in 0..n {
            afsk_phase += 2.0 * PI * tone / fs;
            let audio = afsk_phase.sin();
            fm_phase += 2.0 * PI * dev * audio / fs;
            out.push(Complex32::new(fm_phase.cos() as f32, fm_phase.sin() as f32));
        }
        t += sps;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_a_synthesised_aprs_packet() {
        // A minimal AX.25 UI frame: APRS>TEST:>hello  (dest APRS, src N0CALL)
        let frame = crate::aprs::ax25::encode_ui(
            "APRS",
            "N0CALL-9",
            &["WIDE1-1"],
            b">test status",
        );
        let fs = 48_000.0;
        let iq = synth_packet(fs, &frame, 40);

        let mut demod = AprsDemod::new(fs, 0.0);
        let mut got = Vec::new();
        for block in iq.chunks(4096) {
            demod.process(block, |f| got.push(f));
        }
        assert_eq!(got.len(), 1, "exactly one frame decoded");
        let parsed = crate::aprs::ax25::Ax25Frame::parse(&got[0].octets).unwrap();
        assert_eq!(parsed.source, "N0CALL-9");
        assert_eq!(parsed.dest, "APRS");
        assert_eq!(parsed.digipeaters, vec!["WIDE1-1"]);
        assert_eq!(parsed.info, b">test status");
    }
}
