//! AX.25 UI-frame parsing — just enough for APRS: the address fields
//! (source, destination, digipeater path) and the raw information bytes.

/// A decoded AX.25 UI frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ax25Frame {
    pub dest: String,
    pub source: String,
    /// Digipeater path, each already suffixed with `*` if it has the
    /// "has-been-repeated" bit set.
    pub digipeaters: Vec<String>,
    pub info: Vec<u8>,
}

/// Decode one address field (7 octets). Returns `(callsign[-ssid], last, repeated)`.
fn decode_addr(a: &[u8]) -> Option<(String, bool, bool)> {
    if a.len() < 7 {
        return None;
    }
    let mut call = String::new();
    for &b in &a[..6] {
        let c = (b >> 1) as char;
        if c != ' ' {
            if !c.is_ascii_alphanumeric() {
                return None;
            }
            call.push(c);
        }
    }
    if call.is_empty() {
        return None;
    }
    let ssid = (a[6] >> 1) & 0x0F;
    if ssid != 0 {
        call.push('-');
        call.push_str(&ssid.to_string());
    }
    let last = a[6] & 0x01 != 0; // address-extension bit
    let repeated = a[6] & 0x80 != 0; // "has been repeated" (H) bit
    Some((call, last, repeated))
}

impl Ax25Frame {
    /// Parse AX.25 octets (FCS already stripped). `None` if it isn't a UI
    /// frame or the addresses don't decode.
    pub fn parse(octets: &[u8]) -> Option<Self> {
        if octets.len() < 15 {
            return None;
        }
        let (dest, dlast, _) = decode_addr(&octets[0..7])?;
        if dlast {
            return None; // source must follow
        }
        let (source, mut slast, _) = decode_addr(&octets[7..14])?;

        let mut digipeaters = Vec::new();
        let mut pos = 14;
        while !slast {
            if pos + 7 > octets.len() {
                return None;
            }
            let (mut call, last, repeated) = decode_addr(&octets[pos..pos + 7])?;
            if repeated {
                call.push('*');
            }
            digipeaters.push(call);
            slast = last;
            pos += 7;
            if digipeaters.len() > 8 {
                return None;
            }
        }

        // control (UI = 0x03), PID (0xF0 = no layer 3)
        if pos + 2 > octets.len() || octets[pos] != 0x03 {
            return None;
        }
        pos += 2; // skip control + PID
        Some(Self { dest, source, digipeaters, info: octets[pos..].to_vec() })
    }

    /// The TNC2 monitor line: `SRC>DEST[,PATH…]:info`.
    pub fn to_tnc2(&self) -> String {
        let mut s = format!("{}>{}", self.source, self.dest);
        for d in &self.digipeaters {
            s.push(',');
            s.push_str(d);
        }
        s.push(':');
        s.push_str(&String::from_utf8_lossy(&self.info));
        s
    }
}

/// Encode one address field into 7 octets. (Test-only — the receiver never
/// builds AX.25.)
#[cfg(test)]
fn encode_addr(call: &str, last: bool) -> [u8; 7] {
    let (name, ssid) = match call.split_once('-') {
        Some((n, s)) => (n, s.parse::<u8>().unwrap_or(0) & 0x0F),
        None => (call, 0),
    };
    let mut out = [b' ' << 1; 7];
    for (i, c) in name.bytes().take(6).enumerate() {
        out[i] = c << 1;
    }
    out[6] = (ssid << 1) | 0x60 | (last as u8); // 0x60 = reserved bits set
    out
}

/// Build an AX.25 UI frame body (no FCS) for `dest`/`source`/`path`/`info` —
/// used by the demod round-trip test.
#[cfg(test)]
pub fn encode_ui(dest: &str, source: &str, path: &[&str], info: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&encode_addr(dest, false));
    out.extend_from_slice(&encode_addr(source, path.is_empty()));
    for (i, p) in path.iter().enumerate() {
        out.extend_from_slice(&encode_addr(p, i + 1 == path.len()));
    }
    out.push(0x03); // UI
    out.push(0xF0); // no layer 3
    out.extend_from_slice(info);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_addresses_and_path() {
        let body = encode_ui("APRS", "KZ4AZ-7", &["WIDE1-1", "WIDE2-1"], b"!hello");
        let f = Ax25Frame::parse(&body).unwrap();
        assert_eq!(f.dest, "APRS");
        assert_eq!(f.source, "KZ4AZ-7");
        assert_eq!(f.digipeaters, vec!["WIDE1-1", "WIDE2-1"]);
        assert_eq!(f.info, b"!hello");
        assert_eq!(f.to_tnc2(), "KZ4AZ-7>APRS,WIDE1-1,WIDE2-1:!hello");
    }

    #[test]
    fn rejects_non_ui_and_short_frames() {
        assert!(Ax25Frame::parse(b"too short").is_none());
        let mut body = encode_ui("APRS", "N0CALL", &[], b"x");
        body[14] = 0x00; // not UI
        assert!(Ax25Frame::parse(&body).is_none());
    }
}
