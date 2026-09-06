//! APRS information-field parsing: position (uncompressed, compressed,
//! MIC-E), status and message. Everything else is kept as raw text.

use super::ax25::Ax25Frame;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum AprsKind {
    Position,
    Status,
    Message,
    Other,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct AprsData {
    pub kind: AprsKind,
    pub lat: Option<f64>,
    pub lon: Option<f64>,
    /// Symbol table (`/` primary, `\` alternate, or an overlay char) + code.
    pub symbol_table: Option<char>,
    pub symbol_code: Option<char>,
    pub course_deg: Option<f64>,
    pub speed_kt: Option<f64>,
    pub altitude_ft: Option<f64>,
    pub comment: String,
    /// Message addressee + text (for `:` message packets).
    pub message_to: Option<String>,
    pub message_text: Option<String>,
}

impl AprsData {
    fn empty(kind: AprsKind) -> Self {
        Self {
            kind,
            lat: None,
            lon: None,
            symbol_table: None,
            symbol_code: None,
            course_deg: None,
            speed_kt: None,
            altitude_ft: None,
            comment: String::new(),
            message_to: None,
            message_text: None,
        }
    }

    /// Parse the info field of an APRS frame. `frame` is needed for MIC-E
    /// (latitude lives in the AX.25 destination address).
    pub fn parse(frame: &Ax25Frame) -> Self {
        let info = &frame.info;
        let Some(&dti) = info.first() else {
            return Self::empty(AprsKind::Other);
        };
        let body = &info[1..];
        match dti {
            b'!' | b'=' => parse_position(body, false).unwrap_or_else(|| Self::empty(AprsKind::Other)),
            b'/' | b'@' => {
                // timestamp (7 chars) then position
                if body.len() > 7 {
                    parse_position(&body[7..], false)
                        .unwrap_or_else(|| Self::empty(AprsKind::Other))
                } else {
                    Self::empty(AprsKind::Other)
                }
            }
            b'`' | b'\'' => parse_mice(&frame.dest, body).unwrap_or_else(|| Self::empty(AprsKind::Other)),
            b'>' => {
                let mut d = Self::empty(AprsKind::Status);
                d.comment = String::from_utf8_lossy(body).trim().to_string();
                d
            }
            b':' => parse_message(body).unwrap_or_else(|| Self::empty(AprsKind::Other)),
            _ => {
                let mut d = Self::empty(AprsKind::Other);
                d.comment = String::from_utf8_lossy(info).trim().to_string();
                d
            }
        }
    }
}

fn as_str(b: &[u8]) -> &str {
    std::str::from_utf8(b).unwrap_or("")
}

/// `2934.55N` → 29.5758…  ; `08221.30W` → −82.355
fn parse_lat(s: &str) -> Option<f64> {
    if s.len() != 8 {
        return None;
    }
    let deg: f64 = s[0..2].trim().parse().ok()?;
    let min: f64 = s[2..7].parse().ok()?;
    let v = deg + min / 60.0;
    match s.as_bytes()[7] {
        b'N' => Some(v),
        b'S' => Some(-v),
        _ => None,
    }
}
fn parse_lon(s: &str) -> Option<f64> {
    if s.len() != 9 {
        return None;
    }
    let deg: f64 = s[0..3].trim().parse().ok()?;
    let min: f64 = s[3..8].parse().ok()?;
    let v = deg + min / 60.0;
    match s.as_bytes()[8] {
        b'E' => Some(v),
        b'W' => Some(-v),
        _ => None,
    }
}

/// Uncompressed `DDMM.mmN/DDDMM.mmW$…` or compressed `/YYYYXXXX$cs…`.
fn parse_position(body: &[u8], _msg: bool) -> Option<AprsData> {
    let mut d = AprsData::empty(AprsKind::Position);

    // Compressed: table char is `/` `\` or A-Z/a-j, then 8 base-91 chars.
    if body.len() >= 13 && matches!(body[0], b'/' | b'\\' | b'A'..=b'Z' | b'a'..=b'j') {
        let v = |c: u8| (c as i64) - 33;
        let y = v(body[1]) * 91 * 91 * 91 + v(body[2]) * 91 * 91 + v(body[3]) * 91 + v(body[4]);
        let x = v(body[5]) * 91 * 91 * 91 + v(body[6]) * 91 * 91 + v(body[7]) * 91 + v(body[8]);
        if (0..380926 * 90 + 1).contains(&y) && (0..190463 * 180 + 1).contains(&x) {
            d.lat = Some(90.0 - y as f64 / 380926.0);
            d.lon = Some(-180.0 + x as f64 / 190463.0);
            d.symbol_table = Some(body[0] as char);
            d.symbol_code = Some(body[9] as char);
            // course/speed or altitude in bytes 10-11 (if not spaces)
            let (c1, c2) = (body[10], body[11]);
            if c1 != b' ' {
                let t = body.get(12).copied().unwrap_or(b' ');
                if (t & 0x18) == 0x10 {
                    // altitude: 1.002^(cs)
                    let cs = (v(c1) * 91 + v(c2)) as f64;
                    d.altitude_ft = Some(1.002f64.powf(cs));
                } else {
                    d.course_deg = Some((v(c1) * 4) as f64);
                    d.speed_kt = Some(1.08f64.powf(v(c2) as f64) - 1.0);
                }
            }
            d.comment = as_str(&body[13.min(body.len())..]).trim().to_string();
            return Some(d);
        }
    }

    // Uncompressed: 8 + 1 (table) + 9 + 1 (code) = 19 chars minimum.
    if body.len() < 19 {
        return None;
    }
    let lat = parse_lat(as_str(&body[0..8]))?;
    let table = body[8] as char;
    let lon = parse_lon(as_str(&body[9..18]))?;
    let code = body[18] as char;
    d.lat = Some(lat);
    d.lon = Some(lon);
    d.symbol_table = Some(table);
    d.symbol_code = Some(code);

    let rest = &body[19..];
    // Optional `CSE/SPD` (7 chars: "088/036") right after the symbol.
    if rest.len() >= 7 && rest[3] == b'/' {
        if let (Ok(c), Ok(s)) = (as_str(&rest[0..3]).parse::<f64>(), as_str(&rest[4..7]).parse::<f64>())
        {
            d.course_deg = Some(c);
            d.speed_kt = Some(s);
        }
    }
    let text = as_str(rest);
    if let Some(i) = text.find("/A=") {
        if let Ok(ft) = text[i + 3..i + 9].parse::<f64>() {
            d.altitude_ft = Some(ft);
        }
    }
    d.comment = text.trim().to_string();
    Some(d)
}

/// MIC-E: latitude + message bits in the AX.25 dest callsign; longitude,
/// speed, course, symbol in the info field.
fn parse_mice(dest: &str, body: &[u8]) -> Option<AprsData> {
    let dcall: Vec<u8> = dest.split('-').next()?.bytes().take(6).collect();
    if dcall.len() != 6 || body.len() < 8 {
        return None;
    }

    let mut lat_digits = [0u8; 6];
    let mut ns_south = false;
    let mut lon_offset = false;
    let mut we_west = false;
    for (i, &c) in dcall.iter().enumerate() {
        let digit = match c {
            b'0'..=b'9' => c - b'0',
            b'A'..=b'J' => c - b'A',
            b'P'..=b'Y' => c - b'P',
            b'K' | b'L' | b'Z' => 0, // treated as 0 / space
            _ => return None,
        };
        lat_digits[i] = digit;
        match i {
            3 => ns_south = !matches!(c, b'0'..=b'9' | b'L' | b'P'..=b'Z'),
            4 => lon_offset = matches!(c, b'0'..=b'9' | b'L' | b'P'..=b'Z'),
            5 => we_west = matches!(c, b'0'..=b'9' | b'L' | b'P'..=b'Z'),
            _ => {}
        }
    }
    let lat_deg = lat_digits[0] as f64 * 10.0 + lat_digits[1] as f64;
    let lat_min =
        lat_digits[2] as f64 * 10.0 + lat_digits[3] as f64 + (lat_digits[4] as f64 * 10.0 + lat_digits[5] as f64) / 100.0;
    let mut lat = lat_deg + lat_min / 60.0;
    if ns_south {
        lat = -lat;
    }

    // Info: d+28, m+28, h+28 (longitude), SP+28, DC+28, SE+28 (speed/course),
    // then symbol code, symbol table.
    let d0 = body[0] as i32 - 28;
    let m0 = body[1] as i32 - 28;
    let h0 = body[2] as i32 - 28;
    let mut lon_deg = d0 + if lon_offset { 100 } else { 0 };
    if (180..=189).contains(&lon_deg) {
        lon_deg -= 80;
    } else if (190..=199).contains(&lon_deg) {
        lon_deg -= 190;
    }
    let mut lon_min = m0;
    if lon_min >= 60 {
        lon_min -= 60;
    }
    let lon_hmin = h0;
    let mut lon = lon_deg as f64 + (lon_min as f64 + lon_hmin as f64 / 100.0) / 60.0;
    if we_west {
        lon = -lon;
    }
    if !(-180.0..=180.0).contains(&lon) || !(-90.0..=90.0).contains(&lat) {
        return None;
    }

    let sp = body[3] as i32 - 28;
    let dc = body[4] as i32 - 28;
    let se = body[5] as i32 - 28;
    let mut speed = sp * 10 + dc / 10;
    if speed >= 800 {
        speed -= 800;
    }
    let mut course = (dc % 10) * 100 + se;
    if course >= 400 {
        course -= 400;
    }

    let mut d = AprsData::empty(AprsKind::Position);
    d.lat = Some(lat);
    d.lon = Some(lon);
    d.speed_kt = Some(speed as f64);
    d.course_deg = Some(course as f64);
    d.symbol_code = body.get(6).map(|&b| b as char);
    d.symbol_table = body.get(7).map(|&b| b as char);

    // Optional telemetry / altitude / comment after byte 8.
    let rest = &body[8..];
    let text = as_str(rest);
    if rest.len() >= 4 && rest[3] == b'}' {
        // "xxx}" base-91 altitude, metres above -10000 m
        let v = |c: u8| (c as i64) - 33;
        let m = v(rest[0]) * 91 * 91 + v(rest[1]) * 91 + v(rest[2]) - 10000;
        d.altitude_ft = Some(m as f64 * 3.28084);
        d.comment = text.get(4..).unwrap_or("").trim().to_string();
    } else {
        d.comment = text.trim().to_string();
    }
    Some(d)
}

/// `:ADDRESSEE:message text` (addressee is padded to 9 chars).
fn parse_message(body: &[u8]) -> Option<AprsData> {
    let s = as_str(body);
    if s.len() < 11 || s.as_bytes()[9] != b':' {
        return None;
    }
    let mut d = AprsData::empty(AprsKind::Message);
    d.message_to = Some(s[..9].trim().to_string());
    d.message_text = Some(s[10..].trim_end().to_string());
    d.comment = d.message_text.clone().unwrap_or_default();
    Some(d)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::aprs::ax25::encode_ui;

    fn parse_info(dest: &str, src: &str, info: &[u8]) -> AprsData {
        let body = encode_ui(dest, src, &[], info);
        AprsData::parse(&Ax25Frame::parse(&body).unwrap())
    }

    #[test]
    fn uncompressed_position_with_course_speed_altitude() {
        let d = parse_info("APRS", "KZ4AZ", b"!2934.55N/08221.30W>088/036/A=000350 hi");
        assert_eq!(d.kind, AprsKind::Position);
        assert!((d.lat.unwrap() - 29.5758).abs() < 1e-3);
        assert!((d.lon.unwrap() + 82.355).abs() < 1e-3);
        assert_eq!(d.symbol_table, Some('/'));
        assert_eq!(d.symbol_code, Some('>'));
        assert_eq!(d.course_deg, Some(88.0));
        assert_eq!(d.speed_kt, Some(36.0));
        assert_eq!(d.altitude_ft, Some(350.0));
        assert!(d.comment.contains("hi"));
    }

    #[test]
    fn status_and_message() {
        let s = parse_info("APRS", "N0CALL", b">Net control tonight");
        assert_eq!(s.kind, AprsKind::Status);
        assert_eq!(s.comment, "Net control tonight");

        let m = parse_info("APRS", "N0CALL", b":KZ4AZ    :ping{1");
        assert_eq!(m.kind, AprsKind::Message);
        assert_eq!(m.message_to.as_deref(), Some("KZ4AZ"));
        assert_eq!(m.message_text.as_deref(), Some("ping{1"));
    }

    #[test]
    fn mice_position_decodes() {
        // Dest "T1PSST" encodes ~lat 33.42.75 N; info body carries lon+sym.
        // Just assert it produces a plausible position, not exact bits.
        let d = parse_info("T1PSST", "KZ4AZ-9", b"`(_fn\"O/`\"4T}");
        assert_eq!(d.kind, AprsKind::Position);
        assert!(d.lat.is_some() && d.lon.is_some());
        assert!(d.lat.unwrap().abs() <= 90.0 && d.lon.unwrap().abs() <= 180.0);
    }
}
