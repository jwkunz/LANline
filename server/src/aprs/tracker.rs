//! APRS station table: folds decoded packets into per-callsign tracks, keeps
//! a short position trail, and expires stale entries. Mirrors
//! `crate::ais::tracker` / `crate::adsb::tracker`.

use super::ax25::Ax25Frame;
use super::parse::{AprsData, AprsKind};
use serde::Serialize;
use std::collections::{HashMap, VecDeque};

const EARTH_RADIUS_KM: f64 = 6371.0088;

#[derive(Clone, Default)]
struct Station {
    lat: Option<f64>,
    lon: Option<f64>,
    course_deg: Option<f64>,
    speed_kt: Option<f64>,
    altitude_ft: Option<f64>,
    symbol_table: Option<char>,
    symbol_code: Option<char>,
    comment: String,
    /// Last message text seen addressed *from* this station.
    last_message: Option<String>,
    path: Vec<String>,
    rssi_dbfs: f32,
    packets: u64,
    last_seen: f64,
    last_pos: Option<f64>,
    trail: VecDeque<(f64, f64, f64)>, // (t, lat, lon)
}

pub struct Tracker {
    map: HashMap<String, Station>,
    ref_pos: Option<(f64, f64)>,
    max_range_km: f64,
    trail_secs: f64,
    forget_secs: f64,
    total_packets: u64,
    recent: VecDeque<f64>,
}

impl Default for Tracker {
    fn default() -> Self {
        Self::new(None, 300.0, 1800.0, 3600.0)
    }
}

impl Tracker {
    pub fn new(
        ref_pos: Option<(f64, f64)>,
        max_range_km: f64,
        trail_secs: f64,
        forget_secs: f64,
    ) -> Self {
        Self {
            map: HashMap::new(),
            ref_pos,
            max_range_km,
            trail_secs,
            forget_secs,
            total_packets: 0,
            recent: VecDeque::new(),
        }
    }

    pub fn reset(
        &mut self,
        ref_pos: Option<(f64, f64)>,
        max_range_km: f64,
        trail_secs: f64,
        forget_secs: f64,
    ) {
        *self = Self::new(ref_pos, max_range_km, trail_secs, forget_secs);
    }

    pub fn ingest(&mut self, frame: &Ax25Frame, data: &AprsData, rssi_dbfs: f32, now: f64) {
        self.total_packets += 1;
        self.recent.push_back(now);
        while self.recent.front().is_some_and(|&t| now - t > 30.0) {
            self.recent.pop_front();
        }

        // Range-gate positions against the receiver, if we have both.
        if let (Some((rlat, rlon)), Some(la), Some(lo)) = (self.ref_pos, data.lat, data.lon) {
            if haversine_km(rlat, rlon, la, lo) > self.max_range_km {
                return;
            }
        }

        let key = frame.source.clone();
        let s = self.map.entry(key).or_default();
        s.packets += 1;
        s.last_seen = now;
        s.rssi_dbfs = rssi_dbfs;
        s.path = frame.digipeaters.clone();
        if data.symbol_code.is_some() {
            s.symbol_table = data.symbol_table;
            s.symbol_code = data.symbol_code;
        }
        if !data.comment.is_empty() {
            s.comment = data.comment.clone();
        }
        if data.kind == AprsKind::Message {
            s.last_message = data.message_text.clone();
        }
        if let (Some(la), Some(lo)) = (data.lat, data.lon) {
            s.lat = Some(la);
            s.lon = Some(lo);
            s.course_deg = data.course_deg.or(s.course_deg);
            s.speed_kt = data.speed_kt.or(s.speed_kt);
            s.altitude_ft = data.altitude_ft.or(s.altitude_ft);
            s.last_pos = Some(now);
            s.trail.push_back((now, la, lo));
            let cutoff = now - self.trail_secs;
            while s.trail.front().is_some_and(|&(t, ..)| t < cutoff) {
                s.trail.pop_front();
            }
        }
    }

    pub fn prune(&mut self, now: f64) {
        let forget = self.forget_secs;
        self.map.retain(|_, s| now - s.last_seen <= forget);
    }

    fn packet_rate(&self, now: f64) -> f64 {
        let recent = self.recent.iter().filter(|&&t| now - t <= 30.0).count();
        recent as f64 / 30.0
    }

    pub fn snapshot(&mut self, now: f64) -> Snapshot {
        let rate = self.packet_rate(now);
        let mut stations: Vec<StationView> = self
            .map
            .iter()
            .filter(|(_, s)| now - s.last_seen <= self.forget_secs)
            .map(|(call, s)| {
                let (distance_km, bearing_deg) = match (self.ref_pos, s.lat, s.lon) {
                    (Some((rlat, rlon)), Some(la), Some(lo)) => (
                        Some(round1(haversine_km(rlat, rlon, la, lo))),
                        Some(round1(bearing(rlat, rlon, la, lo))),
                    ),
                    _ => (None, None),
                };
                StationView {
                    call: call.clone(),
                    lat: s.lat,
                    lon: s.lon,
                    course_deg: s.course_deg,
                    speed_kt: s.speed_kt,
                    altitude_ft: s.altitude_ft.map(round1),
                    symbol: match (s.symbol_table, s.symbol_code) {
                        (Some(t), Some(c)) => Some(format!("{t}{c}")),
                        _ => None,
                    },
                    comment: s.comment.clone(),
                    last_message: s.last_message.clone(),
                    path: s.path.clone(),
                    rssi_dbfs: Some(round1(s.rssi_dbfs as f64)),
                    packets: s.packets,
                    age_s: round1(now - s.last_seen),
                    pos_age_s: s.last_pos.map(|t| round1(now - t)),
                    distance_km,
                    bearing_deg,
                    trail: s.trail.iter().map(|&(_, la, lo)| [round5(la), round5(lo)]).collect(),
                }
            })
            .collect();

        stations.sort_by(|a, b| match (a.distance_km, b.distance_km) {
            (Some(x), Some(y)) => x.partial_cmp(&y).unwrap(),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => a.age_s.partial_cmp(&b.age_s).unwrap(),
        });

        Snapshot {
            receiver: self.ref_pos.map(|(la, lo)| [la, lo]),
            packets: self.total_packets,
            packet_rate: round1(rate),
            with_position: stations.iter().filter(|s| s.lat.is_some()).count(),
            station_count: stations.len(),
            stations,
        }
    }
}

#[derive(Serialize)]
pub struct StationView {
    pub call: String,
    pub lat: Option<f64>,
    pub lon: Option<f64>,
    pub course_deg: Option<f64>,
    pub speed_kt: Option<f64>,
    pub altitude_ft: Option<f64>,
    pub symbol: Option<String>,
    pub comment: String,
    pub last_message: Option<String>,
    pub path: Vec<String>,
    pub rssi_dbfs: Option<f64>,
    pub packets: u64,
    pub age_s: f64,
    pub pos_age_s: Option<f64>,
    pub distance_km: Option<f64>,
    pub bearing_deg: Option<f64>,
    pub trail: Vec<[f64; 2]>,
}

#[derive(Serialize)]
pub struct Snapshot {
    pub receiver: Option<[f64; 2]>,
    pub packets: u64,
    pub packet_rate: f64,
    pub station_count: usize,
    pub with_position: usize,
    pub stations: Vec<StationView>,
}

pub fn haversine_km(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> f64 {
    let (p1, p2) = (lat1.to_radians(), lat2.to_radians());
    let dp = (lat2 - lat1).to_radians();
    let dl = (lon2 - lon1).to_radians();
    let a = (dp / 2.0).sin().powi(2) + p1.cos() * p2.cos() * (dl / 2.0).sin().powi(2);
    2.0 * EARTH_RADIUS_KM * a.sqrt().asin()
}

fn bearing(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> f64 {
    let (p1, p2) = (lat1.to_radians(), lat2.to_radians());
    let dl = (lon2 - lon1).to_radians();
    let y = dl.sin() * p2.cos();
    let x = p1.cos() * p2.sin() - p1.sin() * p2.cos() * dl.cos();
    (y.atan2(x).to_degrees() + 360.0) % 360.0
}

fn round1(v: f64) -> f64 {
    (v * 10.0).round() / 10.0
}
fn round5(v: f64) -> f64 {
    (v * 100_000.0).round() / 100_000.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::aprs::ax25::encode_ui;

    #[test]
    fn ingests_position_and_range_gates() {
        let mut t = Tracker::new(Some((29.65, -82.32)), 100.0, 600.0, 3600.0);
        let body = encode_ui("APRS", "KZ4AZ-9", &["WIDE1-1"], b"!2939.00N/08219.00W>near");
        let f = Ax25Frame::parse(&body).unwrap();
        let d = AprsData::parse(&f);
        t.ingest(&f, &d, -40.0, 1.0);

        // Far away -> gated out.
        let body2 = encode_ui("APRS", "W1FAR", &[], b"!4200.00N/07100.00W>far");
        let f2 = Ax25Frame::parse(&body2).unwrap();
        let d2 = AprsData::parse(&f2);
        t.ingest(&f2, &d2, -40.0, 2.0);

        let snap = t.snapshot(3.0);
        assert_eq!(snap.station_count, 1);
        assert_eq!(snap.stations[0].call, "KZ4AZ-9");
        assert!(snap.stations[0].distance_km.unwrap() < 100.0);
        assert_eq!(snap.stations[0].path, vec!["WIDE1-1"]);
        assert_eq!(snap.packets, 2); // both counted, one gated from the map
    }
}
