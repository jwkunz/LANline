//! 1090 MHz pulse-position demodulation at 2 Msps (2 samples per bit), the
//! classic dump1090 approach: magnitude, preamble correlation, then a
//! first-half/second-half energy slice per bit. Only CRC-valid Mode S frames
//! are emitted.

use super::message::ModeS;
use num_complex::Complex32;

/// Preamble (16) + longest message (112 bits × 2).
const WINDOW: usize = 16 + 112 * 2;
/// One-pole DC-blocker coefficient (~2400-sample time constant at 2 Msps).
const DC_A: f32 = 1.0 / 2048.0;

#[derive(Debug, Clone)]
pub struct RawFrame {
    pub bytes: Vec<u8>,
    pub rssi_dbfs: f32,
    pub decoded: ModeS,
}

pub struct Demod {
    mag: Vec<f32>,
    dc_i: f32,
    dc_q: f32,
    fix_errors: bool,
}

impl Demod {
    pub fn new(fix_errors: bool) -> Self {
        Self { mag: Vec::with_capacity(1 << 18), dc_i: 0.0, dc_q: 0.0, fix_errors }
    }

    /// Feed a block of IQ and get a callback per recovered frame.
    pub fn process<F: FnMut(RawFrame)>(&mut self, iq: &[Complex32], mut on_frame: F) {
        self.mag.reserve(iq.len());
        for &c in iq {
            self.dc_i += (c.re - self.dc_i) * DC_A;
            self.dc_q += (c.im - self.dc_q) * DC_A;
            let i = c.re - self.dc_i;
            let q = c.im - self.dc_q;
            self.mag.push((i * i + q * q).sqrt());
        }

        let m = &self.mag;
        let n = m.len();
        let mut j = 0usize;
        while j + WINDOW <= n {
            let Some(high) = preamble(m, j) else {
                j += 1;
                continue;
            };

            // Slice 112 bits; decide the real length from the downlink format.
            let start = j + 16;
            let mut bytes = [0u8; 14];
            let mut signal = 0.0f32;
            for k in 0..112 {
                let a = m[start + 2 * k];
                let b = m[start + 2 * k + 1];
                if a > b {
                    bytes[k / 8] |= 0x80 >> (k % 8);
                    signal += a * a;
                } else {
                    signal += b * b;
                }
            }

            let df = bytes[0] >> 3;
            let len = match df {
                17 | 18 => 14,
                11 => 7,
                _ => {
                    j += 1;
                    continue;
                }
            };

            match ModeS::parse(&bytes[..len], self.fix_errors) {
                Some(decoded) => {
                    let power = (signal / 112.0).max(1e-12);
                    let rssi_dbfs = (10.0 * power.log10()).clamp(-60.0, 0.0);
                    let _ = high;
                    on_frame(RawFrame { bytes: bytes[..len].to_vec(), rssi_dbfs, decoded });
                    j += 16 + 2 * len * 8; // step past this message
                }
                None => j += 1,
            }
        }

        // Keep a WINDOW-sized tail so a frame spanning two blocks still decodes.
        let keep = WINDOW.min(n);
        self.mag.drain(..n - keep);
    }
}

/// dump1090's preamble gate: pulses at samples 0, 2, 7, 9 and a quiet zone
/// through sample 14. Returns the pulse-height reference on a match.
fn preamble(m: &[f32], j: usize) -> Option<f32> {
    let s = &m[j..j + 16];
    let pulses = s[0] > s[1]
        && s[1] < s[2]
        && s[2] > s[3]
        && s[3] < s[0]
        && s[4] < s[0]
        && s[5] < s[0]
        && s[6] < s[0]
        && s[7] > s[8]
        && s[8] < s[9]
        && s[9] > s[6];
    if !pulses {
        return None;
    }
    let high = (s[0] + s[2] + s[7] + s[9]) / 6.0;
    if s[10] >= high || s[11] >= high || s[12] >= high || s[13] >= high || s[14] >= high {
        return None;
    }
    Some(high)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(s: &str) -> Vec<u8> {
        (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
    }

    /// Build a clean 2 Msps IQ waveform for one frame (energy on I, Q = 0).
    fn synth(frame: &[u8]) -> Vec<Complex32> {
        let nbits = frame.len() * 8;
        let mut mag = vec![0.0f32; 40]; // lead-in silence
        let mut pre = [0.0f32; 16];
        for &p in &[0usize, 2, 7, 9] {
            pre[p] = 1.0;
        }
        mag.extend_from_slice(&pre);
        for k in 0..nbits {
            let bit = (frame[k / 8] >> (7 - (k % 8))) & 1;
            if bit == 1 {
                mag.push(1.0);
                mag.push(0.0);
            } else {
                mag.push(0.0);
                mag.push(1.0);
            }
        }
        mag.extend_from_slice(&[0.0; 40]);
        mag.into_iter().map(|v| Complex32::new(v, 0.0)).collect()
    }

    #[test]
    fn recovers_synthetic_frame() {
        let frame = hex("8D4840D6202CC371C32CE0576098");
        let iq = synth(&frame);
        let mut d = Demod::new(true);
        let mut got = Vec::new();
        d.process(&iq, |f| got.push(f));
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].bytes, frame);
        assert!(matches!(got[0].decoded, ModeS::Adsb(_)));
    }

    #[test]
    fn survives_block_boundary() {
        let frame = hex("8D40621D58C382D690C8AC2863A7");
        let iq = synth(&frame);
        let mut d = Demod::new(true);
        let mut got = Vec::new();
        let split = 50; // mid-preamble
        d.process(&iq[..split], |f| got.push(f));
        d.process(&iq[split..], |f| got.push(f));
        assert_eq!(got.len(), 1, "frame split across two process() calls");
    }

    #[test]
    fn pure_noise_decodes_nothing() {
        let mut seed = 0x1234_5678u32;
        let iq: Vec<_> = (0..20_000)
            .map(|_| {
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                let r = (seed >> 8) as f32 / 16_777_216.0 - 0.5;
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                let i = (seed >> 8) as f32 / 16_777_216.0 - 0.5;
                Complex32::new(r * 0.1, i * 0.1)
            })
            .collect();
        let mut d = Demod::new(true);
        let mut got = 0;
        d.process(&iq, |_| got += 1);
        assert_eq!(got, 0);
    }
}
