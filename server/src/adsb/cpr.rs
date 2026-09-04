//! Compact Position Reporting (CPR) decode — the airborne even/odd scheme from
//! DO-260 / ICAO Annex 10. `decode_global` recovers an absolute position from a
//! fresh even+odd pair with no prior knowledge; `decode_local` refines a single
//! frame against a nearby reference (the receiver, or the last known fix).

use std::f64::consts::PI;

const NZ: f64 = 15.0;
const SCALE: f64 = 131_072.0; // 2^17

/// Number of longitude zones at a given latitude.
pub fn nl(lat: f64) -> i32 {
    let lat = lat.abs();
    if lat >= 87.0 {
        return 1;
    }
    if lat < 1e-9 {
        return 59;
    }
    let a = 1.0 - (PI / (2.0 * NZ)).cos();
    let b = (PI / 180.0 * lat).cos().powi(2);
    let nl = 2.0 * PI / (1.0 - a / b).acos();
    (nl.floor() as i32).max(1)
}

fn floor_mod(a: f64, b: f64) -> f64 {
    a - b * (a / b).floor()
}

/// Global decode from an even and an odd frame. `latest_odd` says which of the
/// two arrived more recently (its zone is used for the final fix). Returns
/// `None` if the two frames straddle a latitude zone boundary (caller should
/// wait for a fresher pair).
pub fn decode_global(
    even: (u32, u32),
    odd: (u32, u32),
    latest_odd: bool,
) -> Option<(f64, f64)> {
    let (yz_e, xz_e) = (even.0 as f64 / SCALE, even.1 as f64 / SCALE);
    let (yz_o, xz_o) = (odd.0 as f64 / SCALE, odd.1 as f64 / SCALE);

    let dlat_e = 360.0 / 60.0;
    let dlat_o = 360.0 / 59.0;

    let j = (59.0 * yz_e - 60.0 * yz_o + 0.5).floor();
    let mut rlat_e = dlat_e * (floor_mod(j, 60.0) + yz_e);
    let mut rlat_o = dlat_o * (floor_mod(j, 59.0) + yz_o);
    if rlat_e >= 270.0 {
        rlat_e -= 360.0;
    }
    if rlat_o >= 270.0 {
        rlat_o -= 360.0;
    }
    if !(-90.0..=90.0).contains(&rlat_e) || !(-90.0..=90.0).contains(&rlat_o) {
        return None;
    }
    if nl(rlat_e) != nl(rlat_o) {
        return None;
    }

    let (rlat, nl_lat) = if latest_odd { (rlat_o, nl(rlat_o)) } else { (rlat_e, nl(rlat_e)) };

    let ni = if latest_odd { (nl_lat - 1).max(1) } else { nl_lat.max(1) } as f64;
    let dlon = 360.0 / ni;
    let m = (xz_e * (nl_lat - 1) as f64 - xz_o * nl_lat as f64 + 0.5).floor();
    let xz = if latest_odd { xz_o } else { xz_e };
    let mut rlon = dlon * (floor_mod(m, ni) + xz);
    if rlon >= 180.0 {
        rlon -= 360.0;
    }

    Some((rlat, rlon))
}

/// Local decode of one frame against a reference within ~180 NM.
pub fn decode_local(cpr: (u32, u32), odd: bool, ref_lat: f64, ref_lon: f64) -> (f64, f64) {
    let yz = cpr.0 as f64 / SCALE;
    let xz = cpr.1 as f64 / SCALE;

    let dlat = if odd { 360.0 / 59.0 } else { 360.0 / 60.0 };
    let j = (ref_lat / dlat).floor() + (floor_mod(ref_lat, dlat) / dlat - yz + 0.5).floor();
    let rlat = dlat * (j + yz);

    let nl_lat = nl(rlat);
    let ni = if odd { (nl_lat - 1).max(1) } else { nl_lat.max(1) } as f64;
    let dlon = 360.0 / ni;
    let m = (ref_lon / dlon).floor() + (floor_mod(ref_lon, dlon) / dlon - xz + 0.5).floor();
    let rlon = dlon * (m + xz);

    (rlat, rlon)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nl_table_spot_checks() {
        assert_eq!(nl(0.0), 59);
        assert_eq!(nl(52.0), 36);
        assert_eq!(nl(87.5), 1);
        assert_eq!(nl(-52.0), 36);
    }

    #[test]
    fn global_decode_junzis_example() {
        // The canonical even/odd pair from the-1090mhz-riddle.
        let even = (93000, 51372);
        let odd = (74158, 50194);
        let (lat, lon) = decode_global(even, odd, false).unwrap();
        assert!((lat - 52.2572).abs() < 1e-3, "lat {lat}");
        assert!((lon - 3.91937).abs() < 1e-3, "lon {lon}");
    }

    #[test]
    fn local_decode_matches_global() {
        let even = (93000, 51372);
        let (lat, lon) = decode_local(even, false, 52.258, 3.918);
        assert!((lat - 52.2572).abs() < 1e-3, "lat {lat}");
        assert!((lon - 3.91937).abs() < 1e-3, "lon {lon}");
    }
}
