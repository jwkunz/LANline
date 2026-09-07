//! SSTV transmit: an image (RGB, at a [`Mode`]'s exact geometry) → the SSTV
//! audio waveform, and that audio → FM IQ for the SDR. The inverse of
//! `crate::sstv::demod`; the two are a matched pair (see the round-trip
//! tests). FM only — 2 m amateur SSTV, keyed like the voice `say` path.

use super::modes::{Color, Mode};
use num_complex::Complex32;
use std::f64::consts::PI;

/// Audio sample rate the encoder emits (the TX chain's mic-audio rate).
pub const TX_AUDIO_RATE: f64 = 48_000.0;
const BLACK_HZ: f64 = 1500.0;
const WHITE_HZ: f64 = 2300.0;

struct Osc {
    phase: f64,
    out: Vec<f32>,
    /// Cumulative *intended* duration (s). Each `tone` extends `out` only to
    /// `round(target * RATE)` samples, so per-segment rounding never
    /// accumulates — the Nth line boundary stays within ½ sample of
    /// `N * mode.line`, which the decoder's sync tracker relies on.
    target: f64,
}
impl Osc {
    fn new(cap: usize) -> Self {
        Self { phase: 0.0, out: Vec::with_capacity(cap), target: 0.0 }
    }
    /// Append a constant `freq` tone spanning the next `secs`, phase-continuous.
    fn tone(&mut self, freq: f64, secs: f64) {
        self.target += secs;
        let want = (self.target * TX_AUDIO_RATE).round() as usize;
        let step = 2.0 * PI * freq / TX_AUDIO_RATE;
        while self.out.len() < want {
            self.out.push(self.phase.sin() as f32);
            self.phase += step;
            if self.phase > 1e6 {
                self.phase %= 2.0 * PI;
            }
        }
    }
    /// A row of pixels: one `mode.pixel`-long tone per level (0..1).
    fn scan(&mut self, levels: &[f64], pixel: f64) {
        for &l in levels {
            self.tone(BLACK_HZ + l.clamp(0.0, 1.0) * (WHITE_HZ - BLACK_HZ), pixel);
        }
    }
    /// The cumulative *intended* time position (drift-free), for line padding.
    fn secs(&self) -> f64 {
        self.target
    }
}

fn rgb_at(rgb: &[u8], i: usize) -> (f64, f64, f64) {
    let o = i * 3;
    (rgb[o] as f64 / 255.0, rgb[o + 1] as f64 / 255.0, rgb[o + 2] as f64 / 255.0)
}

/// JPEG full-range RGB→YCbCr, each returned as a 0..1 level (chroma centred
/// at 0.5) — the inverse of `demod::ycc_row`.
fn ycc_at(rgb: &[u8], i: usize) -> (f64, f64, f64) {
    let (r, g, b) = rgb_at(rgb, i);
    let y = 0.299 * r + 0.587 * g + 0.114 * b;
    let cb = -0.168736 * r - 0.331264 * g + 0.5 * b + 0.5;
    let cr = 0.5 * r - 0.418688 * g - 0.081312 * b + 0.5;
    (y.clamp(0.0, 1.0), cr.clamp(0.0, 1.0), cb.clamp(0.0, 1.0))
}

/// `rgb` must be exactly `mode.width * mode.height * 3` bytes.
pub fn encode(rgb: &[u8], mode: &Mode) -> Vec<f32> {
    let (w, h) = (mode.width, mode.height);
    assert_eq!(rgb.len(), w * h * 3, "encode: wrong pixel count for {}", mode.name);
    let mut o = Osc::new((mode.line * TX_AUDIO_RATE * h as f64) as usize + 48_000);

    // VIS header (matches demod::step_search / step_vis).
    o.tone(1900.0, 0.300);
    o.tone(1200.0, 0.010);
    o.tone(1900.0, 0.300);
    o.tone(1200.0, 0.030); // start bit
    let mut ones = 0u32;
    for i in 0..8 {
        let one = (mode.vis >> i) & 1 == 1;
        if one {
            ones += 1;
        }
        o.tone(if one { 1100.0 } else { 1300.0 }, 0.030);
    }
    o.tone(if ones % 2 == 1 { 1100.0 } else { 1300.0 }, 0.030); // even parity
    o.tone(1200.0, 0.030); // stop bit

    let line_secs = mode.line;
    match mode.color {
        Color::Rgb { order } => {
            // demod reads three components sync-to-sync as comps[0..3] and
            // maps R=comps[order[0]] … — so comps[order[c]] carries channel c.
            for y in 0..h {
                let base = y * w;
                let mut chan = [vec![0.0f64; w], vec![0.0; w], vec![0.0; w]];
                let [c0, c1, c2] = &mut chan;
                for (x, ((cr, cg), cb)) in
                    c0.iter_mut().zip(c1.iter_mut()).zip(c2.iter_mut()).enumerate()
                {
                    let (r, g, b) = rgb_at(rgb, base + x);
                    (*cr, *cg, *cb) = (r, g, b);
                }
                let start = o.secs();
                o.tone(1200.0, mode.sync);
                for c in 0..3 {
                    o.tone(BLACK_HZ, mode.sep);
                    let src = order.iter().position(|&s| s as usize == c).unwrap();
                    o.scan(&chan[src], mode.pixel);
                }
                pad_line(&mut o, start, line_secs);
            }
        }
        Color::Robot36 => {
            for y in 0..h {
                let base = y * w;
                let mut yl = vec![0.0f64; w];
                let mut chroma = vec![0.0f64; w];
                for x in 0..w {
                    let (yy, cr, cb) = ycc_at(rgb, base + x);
                    yl[x] = yy;
                    chroma[x] = if y % 2 == 0 { cr } else { cb };
                }
                let start = o.secs();
                o.tone(1200.0, mode.sync);
                o.tone(BLACK_HZ, mode.sep);
                o.scan(&yl, mode.pixel);
                o.tone(BLACK_HZ, mode.sep);
                o.scan(&chroma, mode.pixel * 2.0);
                pad_line(&mut o, start, line_secs);
            }
        }
        Color::Pd => {
            // Two image rows per transmit line: Y-odd, R-Y, B-Y, Y-even.
            let mut y = 0;
            while y + 1 < h {
                let (b0, b1) = (y * w, (y + 1) * w);
                let mut yo = vec![0.0f64; w];
                let mut ye = vec![0.0f64; w];
                let mut ry = vec![0.0f64; w];
                let mut by = vec![0.0f64; w];
                for x in 0..w {
                    let (yy0, cr0, cb0) = ycc_at(rgb, b0 + x);
                    let (yy1, _, _) = ycc_at(rgb, b1 + x);
                    yo[x] = yy0;
                    ye[x] = yy1;
                    ry[x] = cr0;
                    by[x] = cb0;
                }
                let start = o.secs();
                o.tone(1200.0, mode.sync);
                o.tone(BLACK_HZ, mode.sep);
                o.scan(&yo, mode.pixel);
                o.scan(&ry, mode.pixel);
                o.scan(&by, mode.pixel);
                o.scan(&ye, mode.pixel);
                pad_line(&mut o, start, line_secs);
                y += 2;
            }
        }
    }
    o.out
}

fn pad_line(o: &mut Osc, line_start_secs: f64, line_secs: f64) {
    let remaining = line_secs - (o.secs() - line_start_secs);
    if remaining > 0.0 {
        o.tone(BLACK_HZ, remaining);
    }
}

/// FM-modulate a slab of 48 kHz audio onto IQ at `device_rate`, upsampling
/// linearly. `phase` accumulates across calls so consecutive chunks join
/// without a click.
pub fn fm_iq_chunk(
    audio: &[f32],
    device_rate: f64,
    deviation_hz: f64,
    phase: &mut f64,
) -> Vec<Complex32> {
    if audio.is_empty() {
        return Vec::new();
    }
    let up = device_rate / TX_AUDIO_RATE;
    let mut out = Vec::with_capacity((audio.len() as f64 * up) as usize + 4);
    let k = 2.0 * PI * deviation_hz / device_rate;
    let mut t = 0.0f64; // fractional position within the audio slab
    let last = audio.len() - 1;
    while (t as usize) <= last {
        let i = t as usize;
        let frac = (t - i as f64) as f32;
        let s = audio[i] + (audio[(i + 1).min(last)] - audio[i]) * frac;
        *phase += k * s as f64;
        out.push(Complex32::new(phase.cos() as f32, phase.sin() as f32));
        t += 1.0 / up;
    }
    if *phase > 1e6 {
        *phase %= 2.0 * PI;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sstv::demod::{Demod, Row, SstvDemod, SstvParams};
    use crate::sstv::modes::by_key;

    const DEV_RATE: f64 = 192_000.0;
    const DEV_HZ: f64 = 2_000.0;

    /// Build a `w×h` RGB test image: a horizontal R ramp, vertical G ramp,
    /// constant B.
    fn test_image(w: usize, h: usize) -> Vec<u8> {
        let mut v = vec![0u8; w * h * 3];
        for y in 0..h {
            for x in 0..w {
                let o = (y * w + x) * 3;
                v[o] = ((x * 255) / w.max(1)) as u8;
                v[o + 1] = ((y * 255) / h.max(1)) as u8;
                v[o + 2] = 96;
            }
        }
        v
    }

    fn round_trip(mode_key: &str) -> (Vec<u8>, Vec<Row>, &'static Mode) {
        let mode = by_key(mode_key).unwrap();
        let img = test_image(mode.width, mode.height);
        let audio = encode(&img, mode);
        // audio (48k) → FM IQ (192k)
        let mut phase = 0.0;
        // a little lead-in silence so the decoder's filters settle, and a
        // trailing tail so the final scan line is fully buffered
        let mut wav = vec![0.0f32; 4800];
        wav.extend_from_slice(&audio);
        wav.extend(std::iter::repeat(0.0f32).take(9600));
        let iq = fm_iq_chunk(&wav, DEV_RATE, DEV_HZ, &mut phase);

        let mut d = SstvDemod::new(
            DEV_RATE,
            &SstvParams {
                demod: Demod::Fm,
                deviation_hz: DEV_HZ,
                channel_bw_hz: 16_000.0,
                lo_offset_hz: 0.0,
                ..Default::default()
            },
        );
        let mut rows = Vec::new();
        for c in iq.chunks(16384) {
            rows.extend(d.feed(c));
        }
        (img, rows, mode)
    }

    #[test]
    fn scottie1_round_trips() {
        let (img, rows, mode) = round_trip("scottie1");
        assert!(rows.len() >= mode.height - 2, "decoded {} of {} rows", rows.len(), mode.height);
        // spot-check a mid row against the source image: R ramps along x,
        // G is a constant vertical-ramp value, B is a constant.
        let ry = rows.iter().find(|r| r.y == mode.height / 2).expect("mid row");
        let (w, y) = (mode.width, mode.height / 2);
        let want = |x: usize, ch: usize| img[(y * w + x) * 3 + ch] as i32;
        for x in [w / 4, w / 2, (3 * w) / 4] {
            for (ch, &g) in ry.rgb[x].iter().enumerate() {
                assert!(
                    (g as i32 - want(x, ch)).abs() < 40,
                    "ch{ch}@{x}: {g} vs {}",
                    want(x, ch)
                );
            }
        }
    }

    #[test]
    fn robot36_round_trips() {
        let (img, rows, mode) = round_trip("robot36");
        assert!(rows.len() >= mode.height - 4, "decoded {} of {} rows", rows.len(), mode.height);
        let ry = rows.iter().find(|r| r.y == mode.height / 2).expect("mid row");
        let w = mode.width;
        // luminance ramp should track along x (R climbs 0→255 across the row)
        let l0 = ry.rgb[w / 8].iter().map(|&v| v as i32).sum::<i32>();
        let l1 = ry.rgb[(7 * w) / 8].iter().map(|&v| v as i32).sum::<i32>();
        assert!(l1 > l0 + 60, "luma should brighten to the right: {l0} -> {l1}");
        let _ = img;
    }

    #[test]
    fn vis_header_bits() {
        // Scottie 1 = VIS 60 = 0b0011_1100, even parity (4 ones → parity 0).
        let mode = by_key("scottie1").unwrap();
        let audio = encode(&test_image(mode.width, mode.height), mode);
        let ms = TX_AUDIO_RATE / 1000.0;
        // leader 300 + break 10 + leader 300 + start 30 = 640 ms in; bits are
        // 30 ms each, sampled at their centre.
        let bit0 = 640.0 * ms;
        // frequency of the bit's middle 20 ms via total zero-crossings / 2.
        let freq = |c: f64| {
            let (a, b) = ((c - 10.0 * ms) as usize, (c + 10.0 * ms) as usize);
            let zc = audio[a..b].windows(2).filter(|w| w[0].signum() != w[1].signum()).count();
            zc as f64 / 2.0 * TX_AUDIO_RATE / (b - a) as f64
        };
        for i in 0..8 {
            let f = freq(bit0 + (i as f64 + 0.5) * 30.0 * ms);
            let one = (mode.vis >> i) & 1 == 1;
            let target = if one { 1100.0 } else { 1300.0 };
            assert!((f - target).abs() < 60.0, "vis bit {i}: {f:.0} Hz, want {target}");
        }
    }
}
