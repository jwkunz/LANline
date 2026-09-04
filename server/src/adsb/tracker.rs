//! Aircraft state table: folds decoded Mode S messages into per-ICAO tracks
//! (position via CPR, velocity, identification), keeps a short position trail,
//! and expires stale entries.

use super::cpr;
use super::message::{AdsbMessage, MeBody, ModeS};
use std::collections::HashMap;
use std::collections::VecDeque;

const CPR_PAIR_MAX_AGE_S: f64 = 10.0;
const LOCAL_REF_MAX_AGE_S: f64 = 30.0;
const MAX_GROUND_SPEED_KT: f64 = 1400.0;
const EARTH_RADIUS_NM: f64 = 3440.065;

#[derive(Clone)]
struct CprFrame {
    t: f64,
    lat: u32,
    lon: u32,
}

#[derive(Clone)]
struct Aircraft {
    callsign: Option<String>,
    category: Option<String>,
    altitude_ft: Option<i32>,
    lat: Option<f64>,
    lon: Option<f64>,
    ground_speed_kt: Option<f64>,
    track_deg: Option<f64>,
    vertical_rate_fpm: Option<i32>,
    rssi_dbfs: f32,
    messages: u64,
    last_seen: f64,
    last_pos: Option<f64>,
    even: Option<CprFrame>,
    odd: Option<CprFrame>,
    trail: VecDeque<(f64, f64, f64)>, // (t, lat, lon)
}

impl Aircraft {
    fn new(t: f64, rssi: f32) -> Self {
        Self {
            callsign: None,
            category: None,
            altitude_ft: None,
            lat: None,
            lon: None,
            ground_speed_kt: None,
            track_deg: None,
            vertical_rate_fpm: None,
            rssi_dbfs: rssi,
            messages: 0,
            last_seen: t,
            last_pos: None,
            even: None,
            odd: None,
            trail: VecDeque::new(),
        }
    }
}

pub struct Tracker {
    map: HashMap<u32, Aircraft>,
    ref_pos: Option<(f64, f64)>,
    max_range_nm: f64,
    trail_secs: f64,
    forget_secs: f64,
    total_messages: u64,
    recent: VecDeque<f64>,
}

impl Default for Tracker {
    fn default() -> Self {
        Self::new(None, 250.0, 120.0, 60.0)
    }
}

impl Tracker {
    pub fn new(
        ref_pos: Option<(f64, f64)>,
        max_range_nm: f64,
        trail_secs: f64,
        forget_secs: f64,
    ) -> Self {
        Self {
            map: HashMap::new(),
            ref_pos,
            max_range_nm,
            trail_secs,
            forget_secs,
            total_messages: 0,
            recent: VecDeque::new(),
        }
    }

    /// Reconfigure and drop all tracks (called when the pipeline (re)starts).
    pub fn reset(
        &mut self,
        ref_pos: Option<(f64, f64)>,
        max_range_nm: f64,
        trail_secs: f64,
        forget_secs: f64,
    ) {
        *self = Self::new(ref_pos, max_range_nm, trail_secs, forget_secs);
    }

    pub fn message_rate(&mut self, now: f64) -> f64 {
        while self.recent.front().is_some_and(|&t| t < now - 5.0) {
            self.recent.pop_front();
        }
        self.recent.len() as f64 / 5.0
    }

    pub fn ingest(&mut self, msg: &ModeS, rssi_dbfs: f32, now: f64) {
        self.total_messages += 1;
        self.recent.push_back(now);
        if self.recent.len() > 100_000 {
            self.recent.pop_front();
        }

        let (icao, adsb) = match msg {
            ModeS::Adsb(m) => (m.icao, Some(m)),
            ModeS::Other { icao: Some(i), .. } => (*i, None),
            ModeS::Other { icao: None, .. } => return,
        };

        let ac = self.map.entry(icao).or_insert_with(|| Aircraft::new(now, rssi_dbfs));
        ac.messages += 1;
        ac.last_seen = now;
        ac.rssi_dbfs = ac.rssi_dbfs * 0.9 + rssi_dbfs * 0.1;

        let Some(m) = adsb else { return };
        match &m.body {
            MeBody::Identification { callsign, category } => {
                if !callsign.is_empty() {
                    ac.callsign = Some(callsign.clone());
                }
                ac.category = Some(category.clone());
            }
            MeBody::Velocity { ground_speed_kt, track_deg, vertical_rate_fpm } => {
                if ground_speed_kt.is_some() {
                    ac.ground_speed_kt = *ground_speed_kt;
                }
                if track_deg.is_some() {
                    ac.track_deg = *track_deg;
                }
                if vertical_rate_fpm.is_some() {
                    ac.vertical_rate_fpm = *vertical_rate_fpm;
                }
            }
            MeBody::AirbornePosition { altitude_ft, cpr_odd, lat_cpr, lon_cpr } => {
                if altitude_ft.is_some() {
                    ac.altitude_ft = *altitude_ft;
                }
                Self::update_position(
                    ac,
                    self.ref_pos,
                    self.max_range_nm,
                    self.trail_secs,
                    *cpr_odd,
                    *lat_cpr,
                    *lon_cpr,
                    now,
                );
            }
            MeBody::Other { .. } => {}
        }
        let _: &AdsbMessage = m;
    }

    #[allow(clippy::too_many_arguments)]
    fn update_position(
        ac: &mut Aircraft,
        ref_pos: Option<(f64, f64)>,
        max_range_nm: f64,
        trail_secs: f64,
        odd: bool,
        lat_cpr: u32,
        lon_cpr: u32,
        now: f64,
    ) {
        let slot = CprFrame { t: now, lat: lat_cpr, lon: lon_cpr };
        if odd {
            ac.odd = Some(slot);
        } else {
            ac.even = Some(slot);
        }

        let mut fix = None;
        if let (Some(e), Some(o)) = (&ac.even, &ac.odd) {
            if (e.t - o.t).abs() <= CPR_PAIR_MAX_AGE_S {
                let latest_odd = o.t >= e.t;
                fix = cpr::decode_global((e.lat, e.lon), (o.lat, o.lon), latest_odd);
            }
        }
        if fix.is_none() {
            let reference = match (ac.lat, ac.lon, ac.last_pos) {
                (Some(la), Some(lo), Some(tp)) if now - tp <= LOCAL_REF_MAX_AGE_S => Some((la, lo)),
                _ => ref_pos,
            };
            if let Some((rlat, rlon)) = reference {
                fix = Some(cpr::decode_local((lat_cpr, lon_cpr), odd, rlat, rlon));
            }
        }

        let Some((lat, lon)) = fix else { return };
        if !(-90.0..=90.0).contains(&lat) || !(-180.0..=180.0).contains(&lon) {
            return;
        }
        if let Some((rlat, rlon)) = ref_pos {
            if haversine_nm(rlat, rlon, lat, lon) > max_range_nm {
                return;
            }
        }
        if let (Some(pla), Some(plo), Some(tp)) = (ac.lat, ac.lon, ac.last_pos) {
            let dt = (now - tp).max(1e-3);
            if haversine_nm(pla, plo, lat, lon) / dt * 3600.0 > MAX_GROUND_SPEED_KT {
                return; // implausible jump
            }
        }

        ac.lat = Some(lat);
        ac.lon = Some(lon);
        ac.last_pos = Some(now);
        ac.trail.push_back((now, lat, lon));
        while ac.trail.front().is_some_and(|&(t, _, _)| t < now - trail_secs) {
            ac.trail.pop_front();
        }
    }

    pub fn prune(&mut self, now: f64) {
        let forget = self.forget_secs;
        self.map.retain(|_, ac| now - ac.last_seen <= forget);
    }

    pub fn snapshot(&mut self, now: f64) -> Snapshot {
        let rate = self.message_rate(now);
        let mut aircraft: Vec<AircraftView> = self
            .map
            .iter()
            .filter(|(_, ac)| now - ac.last_seen <= self.forget_secs)
            .map(|(icao, ac)| {
                let (distance_nm, bearing_deg) = match (self.ref_pos, ac.lat, ac.lon) {
                    (Some((rlat, rlon)), Some(la), Some(lo)) => (
                        Some(haversine_nm(rlat, rlon, la, lo)),
                        Some(bearing_deg(rlat, rlon, la, lo)),
                    ),
                    _ => (None, None),
                };
                AircraftView {
                    icao: format!("{icao:06x}"),
                    callsign: ac.callsign.clone(),
                    category: ac.category.clone(),
                    altitude_ft: ac.altitude_ft,
                    lat: ac.lat,
                    lon: ac.lon,
                    ground_speed_kt: ac.ground_speed_kt.map(round1),
                    track_deg: ac.track_deg.map(round1),
                    vertical_rate_fpm: ac.vertical_rate_fpm,
                    rssi_dbfs: Some(round1(ac.rssi_dbfs as f64)),
                    messages: ac.messages,
                    age_s: round1(now - ac.last_seen),
                    pos_age_s: ac.last_pos.map(|t| round1(now - t)),
                    distance_nm: distance_nm.map(round1),
                    bearing_deg: bearing_deg.map(round1),
                    trail: ac.trail.iter().map(|&(_, la, lo)| [round5(la), round5(lo)]).collect(),
                }
            })
            .collect();

        aircraft.sort_by(|a, b| match (a.distance_nm, b.distance_nm) {
            (Some(x), Some(y)) => x.partial_cmp(&y).unwrap(),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => a.age_s.partial_cmp(&b.age_s).unwrap(),
        });

        Snapshot {
            receiver: self.ref_pos.map(|(la, lo)| [la, lo]),
            messages: self.total_messages,
            message_rate: round1(rate),
            with_position: aircraft.iter().filter(|a| a.lat.is_some()).count(),
            aircraft_count: aircraft.len(),
            aircraft,
        }
    }
}

fn round1(v: f64) -> f64 {
    (v * 10.0).round() / 10.0
}
fn round5(v: f64) -> f64 {
    (v * 100_000.0).round() / 100_000.0
}

pub fn haversine_nm(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> f64 {
    let (p1, p2) = (lat1.to_radians(), lat2.to_radians());
    let dp = (lat2 - lat1).to_radians();
    let dl = (lon2 - lon1).to_radians();
    let a = (dp / 2.0).sin().powi(2) + p1.cos() * p2.cos() * (dl / 2.0).sin().powi(2);
    2.0 * EARTH_RADIUS_NM * a.sqrt().asin()
}

fn bearing_deg(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> f64 {
    let (p1, p2) = (lat1.to_radians(), lat2.to_radians());
    let dl = (lon2 - lon1).to_radians();
    let y = dl.sin() * p2.cos();
    let x = p1.cos() * p2.sin() - p1.sin() * p2.cos() * dl.cos();
    (y.atan2(x).to_degrees() + 360.0) % 360.0
}

// --- serialized views -------------------------------------------------------

use serde::Serialize;

#[derive(Serialize, Clone, Debug)]
pub struct AircraftView {
    pub icao: String,
    pub callsign: Option<String>,
    pub category: Option<String>,
    pub altitude_ft: Option<i32>,
    pub lat: Option<f64>,
    pub lon: Option<f64>,
    pub ground_speed_kt: Option<f64>,
    pub track_deg: Option<f64>,
    pub vertical_rate_fpm: Option<i32>,
    pub rssi_dbfs: Option<f64>,
    pub messages: u64,
    pub age_s: f64,
    pub pos_age_s: Option<f64>,
    pub distance_nm: Option<f64>,
    pub bearing_deg: Option<f64>,
    pub trail: Vec<[f64; 2]>,
}

#[derive(Serialize, Clone, Debug)]
pub struct Snapshot {
    pub receiver: Option<[f64; 2]>,
    pub messages: u64,
    pub message_rate: f64,
    pub aircraft_count: usize,
    pub with_position: usize,
    pub aircraft: Vec<AircraftView>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adsb::message::ModeS;

    fn hex(s: &str) -> Vec<u8> {
        (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
    }
    fn parse(s: &str) -> ModeS {
        ModeS::parse(&hex(s), true).unwrap()
    }

    #[test]
    fn builds_track_from_cpr_pair() {
        let mut t = Tracker::new(Some((52.0, 4.0)), 300.0, 120.0, 60.0);
        t.ingest(&parse("8D40621D58C382D690C8AC2863A7"), -20.0, 1.0); // even
        t.ingest(&parse("8D40621D58C386435CC412692AD6"), -20.0, 2.0); // odd
        let snap = t.snapshot(2.5);
        assert_eq!(snap.aircraft_count, 1);
        let a = &snap.aircraft[0];
        assert_eq!(a.icao, "40621d");
        assert_eq!(a.altitude_ft, Some(38000));
        assert!((a.lat.unwrap() - 52.2572).abs() < 1e-3);
        assert!((a.lon.unwrap() - 3.91937).abs() < 1e-3);
        assert!(a.distance_nm.unwrap() > 0.0 && a.distance_nm.unwrap() < 30.0);
        assert_eq!(a.trail.len(), 1);
    }

    #[test]
    fn identification_and_velocity_merge() {
        let mut t = Tracker::default();
        t.ingest(&parse("8D4840D6202CC371C32CE0576098"), -25.0, 1.0);
        t.ingest(&parse("8D485020994409940838175B284F"), -25.0, 1.1);
        // different ICAOs -> two aircraft, each with its own field populated
        let snap = t.snapshot(2.0);
        assert_eq!(snap.aircraft_count, 2);
        let klm = snap.aircraft.iter().find(|a| a.icao == "4840d6").unwrap();
        assert_eq!(klm.callsign.as_deref(), Some("KLM1023"));
        let vel = snap.aircraft.iter().find(|a| a.icao == "485020").unwrap();
        assert!(vel.ground_speed_kt.unwrap() > 150.0);
    }

    #[test]
    fn expires_stale_aircraft() {
        let mut t = Tracker::new(None, 250.0, 120.0, 30.0);
        t.ingest(&parse("8D4840D6202CC371C32CE0576098"), -25.0, 1.0);
        assert_eq!(t.snapshot(5.0).aircraft_count, 1);
        t.prune(40.0);
        assert_eq!(t.snapshot(40.0).aircraft_count, 0);
    }
}
