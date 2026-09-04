//! AIS (ITU-R M.1371) message-layer decode: the de-framed payload bit string is
//! parsed into the message types the tracker cares about. Also holds the 6-bit
//! ASCII table, the AIVDM armor codec, the HDLC FCS, and ship-type labels.

/// A bit view over the reconstructed big-endian AIS payload.
pub struct Bits<'a> {
    data: &'a [u8],
    len: usize,
}

impl<'a> Bits<'a> {
    pub fn new(data: &'a [u8], len: usize) -> Self {
        Self { data, len }
    }
    pub fn len(&self) -> usize {
        self.len
    }
    /// Unsigned big-endian field.
    pub fn u(&self, start: usize, n: usize) -> u64 {
        let mut v = 0u64;
        for i in 0..n {
            let bit = start + i;
            let b = if bit < self.len {
                (self.data[bit / 8] >> (7 - (bit % 8))) & 1
            } else {
                0
            };
            v = (v << 1) | b as u64;
        }
        v
    }
    /// Two's-complement signed field.
    pub fn i(&self, start: usize, n: usize) -> i64 {
        let raw = self.u(start, n);
        let sign = 1u64 << (n - 1);
        if raw & sign != 0 {
            (raw as i64) - (1i64 << n)
        } else {
            raw as i64
        }
    }
    /// 6-bit-ASCII text field, trailing '@' and spaces trimmed.
    pub fn text(&self, start: usize, n_chars: usize) -> String {
        let mut s = String::with_capacity(n_chars);
        for k in 0..n_chars {
            let v = self.u(start + k * 6, 6) as u8;
            s.push(sixbit_to_ascii(v));
        }
        s.trim_end_matches(['@', ' ']).trim().to_string()
    }
}

fn sixbit_to_ascii(v: u8) -> char {
    // 0->'@', 1..=31 -> 'A'..; 32 -> ' '; 33.. -> '!'..'?'
    let c = if v < 32 { v + 64 } else { v };
    c as char
}

/// AIVDM payload armor: char -> 6-bit value.
#[allow(dead_code)] // used by tests + a future AIVDM ingest path
pub fn unarmor_char(c: u8) -> Option<u8> {
    let mut v = c.wrapping_sub(48);
    if v > 40 {
        v = v.wrapping_sub(8);
    }
    if v < 64 {
        Some(v)
    } else {
        None
    }
}

/// 6-bit value -> AIVDM armor char.
pub fn armor_value(v: u8) -> u8 {
    if v < 40 {
        v + 48
    } else {
        v + 56
    }
}

/// Decode an AIVDM armored payload (+ fill bits) into a packed big-endian bit
/// string. Returns `(bytes, bit_len)`.
#[allow(dead_code)] // used by tests + a future AIVDM ingest path
pub fn unarmor(payload: &str, fill_bits: u8) -> Option<(Vec<u8>, usize)> {
    let mut bits: Vec<bool> = Vec::with_capacity(payload.len() * 6);
    for &c in payload.as_bytes() {
        let v = unarmor_char(c)?;
        for k in (0..6).rev() {
            bits.push((v >> k) & 1 == 1);
        }
    }
    let keep = bits.len().saturating_sub(fill_bits as usize);
    bits.truncate(keep);
    Some((pack_msb(&bits), keep))
}

/// Re-armor a big-endian bit string into an AIVDM payload string + fill bits.
pub fn armor(bits: &[bool]) -> (String, u8) {
    let fill = (6 - bits.len() % 6) % 6;
    let mut s = String::with_capacity((bits.len() + fill) / 6);
    let mut i = 0;
    while i < bits.len() {
        let mut v = 0u8;
        for k in 0..6 {
            v = (v << 1) | (*bits.get(i + k).unwrap_or(&false) as u8);
        }
        s.push(armor_value(v) as char);
        i += 6;
    }
    (s, fill as u8)
}

#[allow(dead_code)]
fn pack_msb(bits: &[bool]) -> Vec<u8> {
    let mut out = vec![0u8; bits.len().div_ceil(8)];
    for (i, &b) in bits.iter().enumerate() {
        if b {
            out[i / 8] |= 0x80 >> (i % 8);
        }
    }
    out
}

/// HDLC / X.25 frame-check sequence check. `octets` includes the 2 FCS octets;
/// a good frame leaves the residual 0xF0B8.
pub fn fcs_ok(octets: &[u8]) -> bool {
    if octets.len() < 3 {
        return false;
    }
    let mut crc: u16 = 0xFFFF;
    for &b in octets {
        crc ^= b as u16;
        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0x8408 } else { crc >> 1 };
        }
    }
    crc == 0xF0B8
}

/// NMEA 0183 checksum (XOR of the bytes between `!`/`$` and `*`).
pub fn nmea_checksum(body: &str) -> u8 {
    body.bytes().fold(0u8, |a, b| a ^ b)
}

// --- parsed messages ---------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub struct Position {
    pub lat: f64,
    pub lon: f64,
    pub sog_kt: Option<f64>,
    pub cog_deg: Option<f64>,
    pub heading_deg: Option<f64>,
    pub nav_status: Option<u8>,
    pub turn_rate: Option<f64>,
    pub class_b: bool,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Static {
    pub name: Option<String>,
    pub callsign: Option<String>,
    pub ship_type: Option<u8>,
    pub imo: Option<u32>,
    pub length_m: Option<u32>,
    pub beam_m: Option<u32>,
    pub draught_m: Option<f64>,
    pub destination: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum AisMessage {
    Position { mmsi: u32, pos: Position },
    Static { mmsi: u32, data: Static },
    /// AtoN (type 21): a fixed/floating navigation aid with a position + name.
    AidToNavigation { mmsi: u32, name: Option<String>, lat: f64, lon: f64, aid_type: u8 },
    /// Base station (type 4): position only.
    BaseStation { mmsi: u32, lat: f64, lon: f64 },
    /// A CRC-valid message of a type we don't model.
    Other { mmsi: u32, msg_type: u8 },
}

impl AisMessage {
    pub fn mmsi(&self) -> u32 {
        match self {
            AisMessage::Position { mmsi, .. }
            | AisMessage::Static { mmsi, .. }
            | AisMessage::AidToNavigation { mmsi, .. }
            | AisMessage::BaseStation { mmsi, .. }
            | AisMessage::Other { mmsi, .. } => *mmsi,
        }
    }

    pub fn parse(b: &Bits) -> Option<AisMessage> {
        if b.len() < 38 {
            return None;
        }
        let msg_type = b.u(0, 6) as u8;
        let mmsi = b.u(8, 30) as u32;

        match msg_type {
            1 | 2 | 3 => {
                if b.len() < 168 {
                    return None;
                }
                let (lat, lon) = latlon(b.i(89, 27), b.i(61, 28))?;
                Some(AisMessage::Position {
                    mmsi,
                    pos: Position {
                        lat,
                        lon,
                        sog_kt: sog(b.u(50, 10)),
                        cog_deg: cog(b.u(116, 12)),
                        heading_deg: heading(b.u(128, 9)),
                        nav_status: Some(b.u(38, 4) as u8),
                        turn_rate: turn(b.i(42, 8)),
                        class_b: false,
                    },
                })
            }
            18 | 19 => {
                if b.len() < 168 {
                    return None;
                }
                let (lat, lon) = latlon(b.i(85, 27), b.i(57, 28))?;
                let mut pos = Position {
                    lat,
                    lon,
                    sog_kt: sog(b.u(46, 10)),
                    cog_deg: cog(b.u(112, 12)),
                    heading_deg: heading(b.u(124, 9)),
                    nav_status: None,
                    turn_rate: None,
                    class_b: true,
                };
                // Type 19 carries the name too.
                if msg_type == 19 && b.len() >= 312 {
                    let _ = &mut pos;
                }
                Some(AisMessage::Position { mmsi, pos })
            }
            5 => {
                if b.len() < 240 {
                    return None;
                }
                Some(AisMessage::Static {
                    mmsi,
                    data: Static {
                        imo: Some(b.u(40, 30) as u32).filter(|v| *v != 0),
                        callsign: nonempty(b.text(70, 7)),
                        name: nonempty(b.text(112, 20)),
                        ship_type: Some(b.u(232, 8) as u8).filter(|v| *v != 0),
                        length_m: Some((b.u(240, 9) + b.u(249, 9)) as u32).filter(|v| *v != 0),
                        beam_m: Some((b.u(258, 6) + b.u(264, 6)) as u32).filter(|v| *v != 0),
                        draught_m: Some(b.u(294, 8) as f64 / 10.0).filter(|v| *v > 0.0),
                        destination: nonempty(b.text(302, 20)),
                    },
                })
            }
            24 => {
                let part = b.u(38, 2);
                let mut data = Static::default();
                if part == 0 && b.len() >= 160 {
                    data.name = nonempty(b.text(40, 20));
                } else if part == 1 && b.len() >= 168 {
                    data.ship_type = Some(b.u(40, 8) as u8).filter(|v| *v != 0);
                    data.callsign = nonempty(b.text(90, 7));
                    data.length_m = Some((b.u(132, 9) + b.u(141, 9)) as u32).filter(|v| *v != 0);
                    data.beam_m = Some((b.u(150, 6) + b.u(156, 6)) as u32).filter(|v| *v != 0);
                } else {
                    return Some(AisMessage::Other { mmsi, msg_type });
                }
                Some(AisMessage::Static { mmsi, data })
            }
            4 | 11 => {
                if b.len() < 168 {
                    return None;
                }
                let (lat, lon) = latlon(b.i(107, 27), b.i(79, 28))?;
                Some(AisMessage::BaseStation { mmsi, lat, lon })
            }
            21 => {
                if b.len() < 272 {
                    return None;
                }
                let (lat, lon) = latlon(b.i(192, 27), b.i(164, 28))?;
                Some(AisMessage::AidToNavigation {
                    mmsi,
                    name: nonempty(b.text(43, 20)),
                    lat,
                    lon,
                    aid_type: b.u(38, 5) as u8,
                })
            }
            _ => Some(AisMessage::Other { mmsi, msg_type }),
        }
    }
}

fn latlon(lat_raw: i64, lon_raw: i64) -> Option<(f64, f64)> {
    // 1/600000 minute units; 91°/181° are "not available".
    let lat = lat_raw as f64 / 600_000.0;
    let lon = lon_raw as f64 / 600_000.0;
    if lat.abs() > 90.0 || lon.abs() > 180.0 {
        return None;
    }
    Some((lat, lon))
}
fn sog(raw: u64) -> Option<f64> {
    (raw != 1023).then_some(raw as f64 / 10.0)
}
fn cog(raw: u64) -> Option<f64> {
    (raw < 3600).then_some(raw as f64 / 10.0)
}
fn heading(raw: u64) -> Option<f64> {
    (raw != 511 && raw < 360).then_some(raw as f64)
}
fn turn(raw: i64) -> Option<f64> {
    if raw == -128 {
        None
    } else {
        let s = (raw as f64 / 4.733).powi(2);
        Some(if raw < 0 { -s } else { s })
    }
}
fn nonempty(s: String) -> Option<String> {
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

/// Coarse ship-type label from the ITU type code.
pub fn ship_type_label(code: u8) -> &'static str {
    match code {
        20..=29 => "WIG",
        30 => "Fishing",
        31 | 32 => "Towing",
        33 => "Dredging",
        34 => "Diving",
        35 => "Military",
        36 => "Sailing",
        37 => "Pleasure craft",
        40..=49 => "High-speed craft",
        50 => "Pilot",
        51 => "Search & rescue",
        52 => "Tug",
        53 => "Port tender",
        54 => "Anti-pollution",
        55 => "Law enforcement",
        58 => "Medical transport",
        60..=69 => "Passenger",
        70..=79 => "Cargo",
        80..=89 => "Tanker",
        90..=99 => "Other",
        _ => "Unknown",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nmea_checksum_matches() {
        assert_eq!(nmea_checksum("AIVDM,1,1,,B,177KQJ5000G?tO`K>RA1wUbN0TKH,0"), 0x5C);
    }

    #[test]
    fn armor_round_trip() {
        let payload = "177KQJ5000G?tO`K>RA1wUbN0TKH";
        let (bytes, len) = unarmor(payload, 0).unwrap();
        let bits: Vec<bool> =
            (0..len).map(|i| (bytes[i / 8] >> (7 - i % 8)) & 1 == 1).collect();
        let (again, fill) = armor(&bits);
        assert_eq!(again, payload);
        assert_eq!(fill, 0);
    }

    #[test]
    fn parses_type1_position() {
        // gpsd AIVDM spec worked example (MMSI 477553000, moored, Elliott Bay).
        let (bytes, len) = unarmor("177KQJ5000G?tO`K>RA1wUbN0TKH", 0).unwrap();
        let b = Bits::new(&bytes, len);
        let m = AisMessage::parse(&b).unwrap();
        let AisMessage::Position { mmsi, pos } = m else { panic!("{m:?}") };
        assert_eq!(mmsi, 477553000);
        assert_eq!(pos.nav_status, Some(5));
        assert!(!pos.class_b);
        assert!((pos.lat - 47.5828).abs() < 1e-3, "lat {}", pos.lat);
        assert!((pos.lon + 122.3458).abs() < 1e-3, "lon {}", pos.lon);
        assert_eq!(pos.sog_kt, Some(0.0));
        assert!((pos.cog_deg.unwrap() - 51.0).abs() < 0.1, "cog {:?}", pos.cog_deg);
        assert_eq!(pos.heading_deg, Some(181.0));
    }

    #[test]
    fn parses_type5_static() {
        // gpsd AIVDM spec type-5 example: the two AIVDM fragments' payloads
        // concatenated, fill bits from the last fragment.
        let payload = "55?MbV02;H;s<HtKR20EHE:0@T4@Dn2222222216L961O5Gf0NSQEp6ClRp88888888880";
        let (bytes, len) = unarmor(payload, 2).unwrap();
        let b = Bits::new(&bytes, len);
        let m = AisMessage::parse(&b).unwrap();
        let AisMessage::Static { mmsi, data } = m else { panic!("{m:?}") };
        assert_eq!(mmsi, 351759000);
        assert_eq!(data.imo, Some(9134270));
        assert_eq!(data.callsign.as_deref(), Some("3FOF8"));
        assert_eq!(data.name.as_deref(), Some("EVER DIADEM"));
        assert_eq!(data.ship_type, Some(70));
    }

    #[test]
    fn fcs_residual() {
        // A frame whose last two octets are a correct FCS leaves 0xF0B8.
        let mut octets = vec![0x01, 0x02, 0x03, 0x04];
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
        assert!(fcs_ok(&octets));
        octets[0] ^= 0x01;
        assert!(!fcs_ok(&octets));
    }
}
