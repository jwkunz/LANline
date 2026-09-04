//! Mode S / 1090 ES frame parsing: CRC-24, downlink-format dispatch, and the
//! ADS-B (DF17/DF18) message-element decoders we care about — identification,
//! airborne position (raw CPR), and airborne velocity.
//!
//! Bit numbering follows the Mode S convention: bit 1 is the MSB of byte 0.

use std::sync::OnceLock;

/// Mode S CRC-24 generator polynomial (0xFFF409, 24-bit form).
const POLY: u32 = 0x00FF_F409;

/// CRC-24 over `data`. For a valid DF11/DF17/DF18 frame the remainder over the
/// whole frame (including the 24-bit parity field) is 0.
pub fn crc(data: &[u8]) -> u32 {
    let mut rem: u32 = 0;
    for &b in data {
        rem ^= (b as u32) << 16;
        for _ in 0..8 {
            rem = if rem & 0x0080_0000 != 0 { (rem << 1) ^ POLY } else { rem << 1 };
        }
        rem &= 0x00FF_FFFF;
    }
    rem
}

/// syndrome[i] = CRC of a 112-bit frame that is all zero except bit i.
/// Lets us map a non-zero residual back to a single flipped bit in O(1).
fn syndromes_112() -> &'static [u32; 112] {
    static T: OnceLock<[u32; 112]> = OnceLock::new();
    T.get_or_init(|| {
        let mut t = [0u32; 112];
        for (i, slot) in t.iter_mut().enumerate() {
            let mut frame = [0u8; 14];
            frame[i / 8] ^= 0x80 >> (i % 8);
            *slot = crc(&frame);
        }
        t
    })
}

/// If `frame` (14 bytes) has a non-zero CRC that corresponds to exactly one
/// flipped bit, flip it back and return `true`.
pub fn fix_single_bit(frame: &mut [u8; 14]) -> bool {
    let residual = crc(frame);
    if residual == 0 {
        return true;
    }
    if let Some(bit) = syndromes_112().iter().position(|&s| s == residual) {
        frame[bit / 8] ^= 0x80 >> (bit % 8);
        true
    } else {
        false
    }
}

/// Read `len` bits (MSB first) starting at 1-indexed Mode S bit `start`.
fn bits(data: &[u8], start: usize, len: usize) -> u32 {
    let mut v = 0u32;
    for i in 0..len {
        let bit = start - 1 + i;
        let byte = data.get(bit / 8).copied().unwrap_or(0);
        v = (v << 1) | ((byte >> (7 - (bit % 8))) & 1) as u32;
    }
    v
}

#[derive(Debug, Clone, PartialEq)]
pub enum MeBody {
    /// TC 1–4: identification + emitter category (e.g. "A3").
    Identification { callsign: String, category: String },
    /// TC 9–18 (barometric) / 20–22 (GNSS): raw CPR position.
    AirbornePosition {
        altitude_ft: Option<i32>,
        cpr_odd: bool,
        lat_cpr: u32,
        lon_cpr: u32,
    },
    /// TC 19: ground speed / airspeed + vertical rate.
    Velocity {
        ground_speed_kt: Option<f64>,
        track_deg: Option<f64>,
        vertical_rate_fpm: Option<i32>,
    },
    /// A DF17/18 message with a type code we don't decode yet.
    Other { type_code: u8 },
}

#[derive(Debug, Clone, PartialEq)]
pub struct AdsbMessage {
    pub icao: u32,
    pub type_code: u8,
    pub body: MeBody,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ModeS {
    /// DF17 / DF18 extended squitter (CRC verified).
    Adsb(AdsbMessage),
    /// A CRC-valid Mode S frame of another downlink format (kept for the feed).
    Other { df: u8, icao: Option<u32> },
}

const CALLSIGN_CHARS: &[u8; 64] =
    b"#ABCDEFGHIJKLMNOPQRSTUVWXYZ##### ###############0123456789######";

fn decode_callsign(frame: &[u8]) -> String {
    // 8 characters of 6 bits each, ME bits 9..56 -> frame bits 41..88.
    let mut s = String::with_capacity(8);
    for i in 0..8 {
        let c = bits(frame, 41 + i * 6, 6) as usize;
        s.push(CALLSIGN_CHARS[c] as char);
    }
    s.trim_matches(|c| c == '#' || c == ' ').to_string()
}

fn decode_category(tc: u8, ca: u8) -> String {
    // TC 1->D, 2->C, 3->B, 4->A; CA is the numeric class.
    let letter = match tc {
        1 => 'D',
        2 => 'C',
        3 => 'B',
        4 => 'A',
        _ => '?',
    };
    if ca == 0 {
        "none".to_string()
    } else {
        format!("{letter}{ca}")
    }
}

/// Barometric altitude from the 12-bit AC field (Q-bit form only; Gillham
/// Q=0 coding returns `None`).
fn decode_altitude(ac12: u32) -> Option<i32> {
    if ac12 == 0 {
        return None;
    }
    let q = (ac12 >> 4) & 1;
    if q == 1 {
        let upper = (ac12 >> 5) & 0x7F; // bits 1..7 of the field
        let lower = ac12 & 0x0F; // bits 9..12
        let n = ((upper << 4) | lower) as i32;
        Some(n * 25 - 1000)
    } else {
        None
    }
}

fn decode_velocity(frame: &[u8], subtype: u8) -> MeBody {
    // Ground-speed subtypes.
    if subtype == 1 || subtype == 2 {
        let mult = if subtype == 2 { 4.0 } else { 1.0 };
        let dew = bits(frame, 46, 1);
        let vew = bits(frame, 47, 10) as f64;
        let dns = bits(frame, 57, 1);
        let vns = bits(frame, 58, 10) as f64;

        let (gs, track) = if vew == 0.0 || vns == 0.0 {
            (None, None)
        } else {
            let vx = (vew - 1.0) * mult * if dew == 1 { -1.0 } else { 1.0 };
            let vy = (vns - 1.0) * mult * if dns == 1 { -1.0 } else { 1.0 };
            let gs = (vx * vx + vy * vy).sqrt();
            let mut trk = vx.atan2(vy).to_degrees();
            if trk < 0.0 {
                trk += 360.0;
            }
            (Some(gs), Some(trk))
        };

        let vr_sign = bits(frame, 69, 1);
        let vr_raw = bits(frame, 70, 9) as i32;
        let vr = if vr_raw == 0 {
            None
        } else {
            Some((vr_raw - 1) * 64 * if vr_sign == 1 { -1 } else { 1 })
        };

        MeBody::Velocity { ground_speed_kt: gs, track_deg: track, vertical_rate_fpm: vr }
    } else {
        // Airspeed/heading subtypes 3/4 — not decoded yet.
        MeBody::Velocity { ground_speed_kt: None, track_deg: None, vertical_rate_fpm: None }
    }
}

fn decode_me(frame: &[u8], icao: u32) -> AdsbMessage {
    let tc = bits(frame, 33, 5) as u8;
    let body = match tc {
        1..=4 => {
            let ca = bits(frame, 38, 3) as u8;
            MeBody::Identification {
                callsign: decode_callsign(frame),
                category: decode_category(tc, ca),
            }
        }
        9..=18 | 20..=22 => {
            let ac12 = bits(frame, 41, 12);
            MeBody::AirbornePosition {
                altitude_ft: if (9..=18).contains(&tc) { decode_altitude(ac12) } else { None },
                cpr_odd: bits(frame, 54, 1) == 1,
                lat_cpr: bits(frame, 55, 17),
                lon_cpr: bits(frame, 72, 17),
            }
        }
        19 => decode_velocity(frame, bits(frame, 38, 3) as u8),
        _ => MeBody::Other { type_code: tc },
    };
    AdsbMessage { icao, type_code: tc, body }
}

impl ModeS {
    /// Parse a raw frame (7 or 14 bytes). Returns `None` unless the CRC checks
    /// out (after an optional single-bit correction on long frames).
    pub fn parse(frame: &[u8], fix_errors: bool) -> Option<ModeS> {
        let df = frame.first()? >> 3;

        match df {
            17 | 18 if frame.len() == 14 => {
                let mut f = [0u8; 14];
                f.copy_from_slice(&frame[..14]);
                let ok = if fix_errors { fix_single_bit(&mut f) } else { crc(&f) == 0 };
                if !ok {
                    return None;
                }
                let icao = bits(&f, 9, 24);
                Some(ModeS::Adsb(decode_me(&f, icao)))
            }
            11 if frame.len() == 7 => {
                // DF11 all-call reply: CRC parity carries the interrogator id;
                // a squitter has II = 0, so the residual is 0.
                if crc(&frame[..7]) != 0 {
                    return None;
                }
                Some(ModeS::Other { df, icao: Some(bits(frame, 9, 24)) })
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(s: &str) -> Vec<u8> {
        (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
    }

    #[test]
    fn crc_of_valid_df17_is_zero() {
        for m in [
            "8D4840D6202CC371C32CE0576098",
            "8D40621D58C382D690C8AC2863A7",
            "8D40621D58C386435CC412692AD6",
            "8D485020994409940838175B284F",
        ] {
            assert_eq!(crc(&hex(m)), 0, "{m}");
        }
    }

    #[test]
    fn identification() {
        let ModeS::Adsb(m) = ModeS::parse(&hex("8D4840D6202CC371C32CE0576098"), true).unwrap()
        else {
            panic!("not adsb");
        };
        assert_eq!(m.icao, 0x48_40D6);
        assert_eq!(m.type_code, 4);
        let MeBody::Identification { callsign, category } = m.body else { panic!("{:?}", m.body) };
        assert_eq!(callsign, "KLM1023");
        assert_eq!(category, "none"); // CA subfield is 0 for this frame
    }

    #[test]
    fn airborne_position_altitude_and_cpr() {
        let ModeS::Adsb(even) =
            ModeS::parse(&hex("8D40621D58C382D690C8AC2863A7"), true).unwrap()
        else {
            panic!()
        };
        let MeBody::AirbornePosition { altitude_ft, cpr_odd, lat_cpr, lon_cpr } = even.body else {
            panic!()
        };
        assert_eq!(altitude_ft, Some(38000));
        assert!(!cpr_odd);
        assert_eq!(lat_cpr, 93000);
        assert_eq!(lon_cpr, 51372);

        let ModeS::Adsb(odd) = ModeS::parse(&hex("8D40621D58C386435CC412692AD6"), true).unwrap()
        else {
            panic!()
        };
        let MeBody::AirbornePosition { cpr_odd, lat_cpr, lon_cpr, .. } = odd.body else { panic!() };
        assert!(cpr_odd);
        assert_eq!(lat_cpr, 74158);
        assert_eq!(lon_cpr, 50194);
    }

    #[test]
    fn velocity() {
        let ModeS::Adsb(m) = ModeS::parse(&hex("8D485020994409940838175B284F"), true).unwrap()
        else {
            panic!()
        };
        assert_eq!(m.type_code, 19);
        let MeBody::Velocity { ground_speed_kt, track_deg, vertical_rate_fpm } = m.body else {
            panic!()
        };
        assert!((ground_speed_kt.unwrap() - 159.20).abs() < 0.5, "{ground_speed_kt:?}");
        assert!((track_deg.unwrap() - 182.88).abs() < 0.5, "{track_deg:?}");
        assert_eq!(vertical_rate_fpm, Some(-832));
    }

    #[test]
    fn single_bit_correction() {
        let mut f = [0u8; 14];
        f.copy_from_slice(&hex("8D4840D6202CC371C32CE0576098"));
        f[5] ^= 0x08; // flip one bit
        assert!(ModeS::parse(&f, false).is_none());
        let fixed = ModeS::parse(&f, true);
        assert!(matches!(fixed, Some(ModeS::Adsb(_))));
    }

    #[test]
    fn rejects_garbage() {
        // One flipped bit is repairable with fix_errors, not without.
        assert!(ModeS::parse(&hex("8D4840D6202CC371C32CE0576099"), false).is_none());
        // Two flipped bits: unrepairable either way.
        let mut two = hex("8D4840D6202CC371C32CE0576098");
        two[2] ^= 0x01;
        two[9] ^= 0x40;
        assert!(ModeS::parse(&two, true).is_none());
        assert!(ModeS::parse(&[0u8; 14], true).is_none());
        assert!(ModeS::parse(&[0xff; 14], true).is_none());
    }
}
