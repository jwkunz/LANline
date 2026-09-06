//! APRS transmit: build an AX.25 UI frame and modulate it to baseband IQ
//! (1200-baud Bell 202 AFSK on NBFM), the inverse of [`crate::aprs::demod`].
//!
//! Used by `radio::run_aprs`'s half-duplex TX burst. Frame construction
//! (FCS, bit-stuffing, NRZI) mirrors the round-trip test the demod is
//! verified against.

use super::ax25::encode_ui;
use super::demod::{BAUD, MARK_HZ, SPACE_HZ};
use num_complex::Complex32;
use std::f64::consts::PI;

/// Build the AX.25 UI-frame octets (no FCS) for an APRS packet.
/// `path` entries are digipeater aliases (`"WIDE1-1"` …); empty = direct.
pub fn ui_frame(source: &str, dest: &str, path: &[&str], info: &[u8]) -> Vec<u8> {
    encode_ui(dest, source, path, info)
}

/// APRS text-message info field: `:` + addressee padded to 9 + `:` + text.
/// Unnumbered (no `{seq`), so no ack is expected.
pub fn message_info(to: &str, text: &str) -> Vec<u8> {
    // Addressee is exactly 9 chars, space-padded, uppercased per spec.
    let to = to.to_ascii_uppercase();
    let mut out = Vec::with_capacity(1 + 9 + 1 + text.len());
    out.push(b':');
    out.extend_from_slice(format!("{to:<9}").as_bytes());
    out.push(b':');
    out.extend_from_slice(text.as_bytes());
    out
}

/// APRS uncompressed position (no timestamp): `!DDMM.hhN{sym0}DDDMM.hhW{sym1}comment`.
/// `symbol` is the 2-char table+code pair (e.g. `"/>"` = car); defaults to
/// `"/-"` (house) if not exactly two chars.
pub fn position_info(lat: f64, lon: f64, symbol: &str, comment: &str) -> String {
    let sym: Vec<char> = symbol.chars().collect();
    let (t, c) = if sym.len() == 2 { (sym[0], sym[1]) } else { ('/', '-') };

    let (lat_h, lat_deg, lat_min) = dms(lat.abs());
    let (lon_h, lon_deg, lon_min) = dms(lon.abs());
    let ns = if lat >= 0.0 { 'N' } else { 'S' };
    let ew = if lon >= 0.0 { 'E' } else { 'W' };
    let _ = (lat_h, lon_h);

    format!(
        "!{:02}{:05.2}{}{}{:03}{:05.2}{}{}{}",
        lat_deg, lat_min, ns, t, lon_deg, lon_min, ew, c, comment
    )
}

fn dms(x: f64) -> (f64, u32, f64) {
    let deg = x.trunc();
    let min = (x - deg) * 60.0;
    (x, deg as u32, min)
}

/// FCS + bit-stuff + NRZI + Bell 202 AFSK → FM, producing baseband IQ at `fs`.
/// `frame_no_fcs` is the AX.25 UI-frame body; `lead_flags` 0x7E flags precede
/// it (TXDelay). Five trailing flags flush the closing flag through a
/// receiver's group delay.
pub fn modulate(fs: f64, deviation_hz: f64, frame_no_fcs: &[u8], lead_flags: usize) -> Vec<Complex32> {
    // 1. FCS (CRC-16/X.25), appended little-endian, complemented.
    let mut crc: u16 = 0xFFFF;
    for &b in frame_no_fcs {
        crc ^= b as u16;
        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0x8408 } else { crc >> 1 };
        }
    }
    let fcs = !crc;
    let mut framed = frame_no_fcs.to_vec();
    framed.push((fcs & 0xFF) as u8);
    framed.push((fcs >> 8) as u8);

    // 2. data bits LSB-first, with bit-stuffing (a 0 after five consecutive 1s).
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

    // 3. flags + data + trailing flags, then NRZI (0 -> transition, 1 -> hold).
    let mut wire: Vec<bool> = Vec::new();
    let push_flag = |w: &mut Vec<bool>| {
        for i in 0..8u8 {
            w.push((0x7Eu8 >> i) & 1 == 1);
        }
    };
    for _ in 0..lead_flags.max(1) {
        push_flag(&mut wire);
    }
    wire.extend_from_slice(&data_bits);
    for _ in 0..5 {
        push_flag(&mut wire);
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
    let mut out = Vec::with_capacity((nrzi.len() as f64 * sps) as usize + 1);
    let mut afsk_phase = 0.0f64;
    let mut fm_phase = 0.0f64;
    let mut t = 0.0f64;
    for &lvl in &nrzi {
        let tone = if lvl { MARK_HZ } else { SPACE_HZ };
        let n = ((t + sps).floor() - t.floor()) as usize;
        for _ in 0..n {
            afsk_phase += 2.0 * PI * tone / fs;
            let audio = afsk_phase.sin();
            fm_phase += 2.0 * PI * deviation_hz * audio / fs;
            out.push(Complex32::new(fm_phase.cos() as f32, fm_phase.sin() as f32));
        }
        t += sps;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::aprs::ax25::Ax25Frame;
    use crate::aprs::demod::AprsDemod;
    use crate::aprs::parse::{AprsData, AprsKind};

    #[test]
    fn message_info_pads_addressee_to_nine() {
        assert_eq!(message_info("KZ4AZ-2", "hi"), b":KZ4AZ-2  :hi");
        assert_eq!(message_info("n0call", "x"), b":N0CALL   :x");
    }

    #[test]
    fn position_info_formats_ddmm() {
        // 29.65, -82.33 -> 29 deg 39.00', 82 deg 19.80' W
        let s = position_info(29.65, -82.33, "/>", "LANline");
        assert_eq!(s, "!2939.00N/08219.80W>LANline");
    }

    #[test]
    fn message_round_trips_through_the_demod_at_1_msps() {
        let info = message_info("KZ4AZ-2", "ping from radio 0");
        let frame = ui_frame("KZ4AZ-1", "APZLNL", &[], &info);
        let fs = 1_000_000.0;
        let iq = modulate(fs, 3_000.0, &frame, 64);

        let mut demod = AprsDemod::new(fs, 0.0);
        let mut got = Vec::new();
        for block in iq.chunks(8192) {
            demod.process(block, |f| got.push(f));
        }
        assert_eq!(got.len(), 1, "exactly one frame decoded");

        let ax = Ax25Frame::parse(&got[0].octets).unwrap();
        assert_eq!(ax.source, "KZ4AZ-1");
        assert_eq!(ax.dest, "APZLNL");
        assert!(ax.digipeaters.is_empty());

        let data = AprsData::parse(&ax);
        assert_eq!(data.kind, AprsKind::Message);
        assert_eq!(data.message_to.as_deref(), Some("KZ4AZ-2"));
        assert_eq!(data.message_text.as_deref(), Some("ping from radio 0"));
    }
}
