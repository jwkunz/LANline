//! Vessel state table: folds AIS position + static messages into per-MMSI
//! tracks, keeps a short position trail, and expires stale entries. Mirrors
//! `crate::adsb::tracker`.

use super::message::{ship_type_label, AisMessage};
use serde::Serialize;
use std::collections::{HashMap, VecDeque};

const MAX_SPEED_KT: f64 = 80.0;
const EARTH_RADIUS_NM: f64 = 3440.065;

#[derive(Clone, Default)]
struct Vessel {
    name: Option<String>,
    callsign: Option<String>,
    ship_type: Option<u8>,
    imo: Option<u32>,
    nav_status: Option<u8>,
    lat: Option<f64>,
    lon: Option<f64>,
    sog_kt: Option<f64>,
    cog_deg: Option<f64>,
    heading_deg: Option<f64>,
    length_m: Option<u32>,
    beam_m: Option<u32>,
    draught_m: Option<f64>,
    destination: Option<String>,
    class_b: bool,
    is_aid: bool,
    rssi_dbfs: f32,
    messages: u64,
    last_seen: f64,
    last_pos: Option<f64>,
    trail: VecDeque<(f64, f64, f64)>,
}

pub struct Tracker {
    map: HashMap<u32, Vessel>,
    ref_pos: Option<(f64, f64)>,
    max_range_nm: f64,
    trail_secs: f64,
    forget_secs: f64,
    total_messages: u64,
    recent: VecDeque<f64>,
}

impl Default for Tracker {
    fn default() -> Self {
        Self::new(None, 60.0, 600.0, 900.0)
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
        while self.recent.front().is_some_and(|&t| t < now - 10.0) {
            self.recent.pop_front();
        }
        self.recent.len() as f64 / 10.0
    }

    pub fn ingest(&mut self, msg: &AisMessage, rssi_dbfs: f32, now: f64) {
        self.total_messages += 1;
        self.recent.push_back(now);
        if self.recent.len() > 200_000 {
            self.recent.pop_front();
        }

        let mmsi = msg.mmsi();
        if mmsi == 0 {
            return;
        }
        let v = self.map.entry(mmsi).or_default();
        v.messages += 1;
        v.last_seen = now;
        v.rssi_dbfs = if v.messages == 1 { rssi_dbfs } else { v.rssi_dbfs * 0.9 + rssi_dbfs * 0.1 };

        match msg {
            AisMessage::Position { pos, .. } => {
                v.class_b = pos.class_b;
                if pos.sog_kt.is_some() {
                    v.sog_kt = pos.sog_kt;
                }
                if pos.cog_deg.is_some() {
                    v.cog_deg = pos.cog_deg;
                }
                if pos.heading_deg.is_some() {
                    v.heading_deg = pos.heading_deg;
                }
                if pos.nav_status.is_some() {
                    v.nav_status = pos.nav_status;
                }
                Self::set_position(
                    v,
                    self.ref_pos,
                    self.max_range_nm,
                    self.trail_secs,
                    pos.lat,
                    pos.lon,
                    now,
                );
            }
            AisMessage::Static { data, .. } => {
                if data.name.is_some() {
                    v.name = data.name.clone();
                }
                if data.callsign.is_some() {
                    v.callsign = data.callsign.clone();
                }
                if data.ship_type.is_some() {
                    v.ship_type = data.ship_type;
                }
                if data.imo.is_some() {
                    v.imo = data.imo;
                }
                if data.length_m.is_some() {
                    v.length_m = data.length_m;
                }
                if data.beam_m.is_some() {
                    v.beam_m = data.beam_m;
                }
                if data.draught_m.is_some() {
                    v.draught_m = data.draught_m;
                }
                if data.destination.is_some() {
                    v.destination = data.destination.clone();
                }
            }
            AisMessage::AidToNavigation { name, lat, lon, .. } => {
                v.is_aid = true;
                if name.is_some() {
                    v.name = name.clone();
                }
                Self::set_position(
                    v,
                    self.ref_pos,
                    self.max_range_nm,
                    self.trail_secs,
                    *lat,
                    *lon,
                    now,
                );
            }
            AisMessage::BaseStation { lat, lon, .. } => {
                Self::set_position(
                    v,
                    self.ref_pos,
                    self.max_range_nm,
                    self.trail_secs,
                    *lat,
                    *lon,
                    now,
                );
            }
            AisMessage::Other { .. } => {}
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn set_position(
        v: &mut Vessel,
        ref_pos: Option<(f64, f64)>,
        max_range_nm: f64,
        trail_secs: f64,
        lat: f64,
        lon: f64,
        now: f64,
    ) {
        if let Some((rlat, rlon)) = ref_pos {
            if haversine_nm(rlat, rlon, lat, lon) > max_range_nm {
                return;
            }
        }
        if let (Some(pla), Some(plo), Some(tp)) = (v.lat, v.lon, v.last_pos) {
            let dt = (now - tp).max(1e-3);
            if haversine_nm(pla, plo, lat, lon) / dt * 3600.0 > MAX_SPEED_KT * 3.0 {
                return;
            }
        }
        v.lat = Some(lat);
        v.lon = Some(lon);
        v.last_pos = Some(now);
        v.trail.push_back((now, lat, lon));
        while v.trail.front().is_some_and(|&(t, _, _)| t < now - trail_secs) {
            v.trail.pop_front();
        }
    }

    pub fn prune(&mut self, now: f64) {
        let forget = self.forget_secs;
        self.map.retain(|_, v| now - v.last_seen <= forget);
    }

    pub fn snapshot(&mut self, now: f64) -> Snapshot {
        let rate = self.message_rate(now);
        let mut vessels: Vec<VesselView> = self
            .map
            .iter()
            .filter(|(_, v)| now - v.last_seen <= self.forget_secs)
            .map(|(mmsi, v)| {
                let (distance_nm, bearing_deg) = match (self.ref_pos, v.lat, v.lon) {
                    (Some((rlat, rlon)), Some(la), Some(lo)) => (
                        Some(round1(haversine_nm(rlat, rlon, la, lo))),
                        Some(round1(bearing_deg(rlat, rlon, la, lo))),
                    ),
                    _ => (None, None),
                };
                VesselView {
                    mmsi: *mmsi,
                    name: v.name.clone(),
                    callsign: v.callsign.clone(),
                    ship_type: v.ship_type,
                    ship_type_label: v.ship_type.map(|c| ship_type_label(c).to_string()),
                    imo: v.imo,
                    nav_status: v.nav_status,
                    nav_status_label: v.nav_status.map(|s| nav_status_label(s).to_string()),
                    class_b: v.class_b,
                    aid: v.is_aid,
                    lat: v.lat,
                    lon: v.lon,
                    sog_kt: v.sog_kt,
                    cog_deg: v.cog_deg,
                    heading_deg: v.heading_deg,
                    length_m: v.length_m,
                    beam_m: v.beam_m,
                    draught_m: v.draught_m,
                    destination: v.destination.clone(),
                    rssi_dbfs: Some(round1(v.rssi_dbfs as f64)),
                    messages: v.messages,
                    age_s: round1(now - v.last_seen),
                    pos_age_s: v.last_pos.map(|t| round1(now - t)),
                    distance_nm,
                    bearing_deg,
                    trail: v.trail.iter().map(|&(_, la, lo)| [round5(la), round5(lo)]).collect(),
                }
            })
            .collect();

        vessels.sort_by(|a, b| match (a.distance_nm, b.distance_nm) {
            (Some(x), Some(y)) => x.partial_cmp(&y).unwrap(),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => a.age_s.partial_cmp(&b.age_s).unwrap(),
        });

        Snapshot {
            receiver: self.ref_pos.map(|(la, lo)| [la, lo]),
            messages: self.total_messages,
            message_rate: round1(rate),
            with_position: vessels.iter().filter(|v| v.lat.is_some()).count(),
            vessel_count: vessels.len(),
            vessels,
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

fn nav_status_label(s: u8) -> &'static str {
    match s {
        0 => "under way (engine)",
        1 => "at anchor",
        2 => "not under command",
        3 => "restricted manoeuvrability",
        4 => "constrained by draught",
        5 => "moored",
        6 => "aground",
        7 => "fishing",
        8 => "under way (sailing)",
        11 => "towing astern",
        12 => "pushing ahead",
        14 => "AIS-SART",
        _ => "unknown",
    }
}

// --- serialized views ------------------------------------------------------

#[derive(Serialize, Clone, Debug)]
pub struct VesselView {
    pub mmsi: u32,
    pub name: Option<String>,
    pub callsign: Option<String>,
    pub ship_type: Option<u8>,
    pub ship_type_label: Option<String>,
    pub imo: Option<u32>,
    pub nav_status: Option<u8>,
    pub nav_status_label: Option<String>,
    pub class_b: bool,
    pub aid: bool,
    pub lat: Option<f64>,
    pub lon: Option<f64>,
    pub sog_kt: Option<f64>,
    pub cog_deg: Option<f64>,
    pub heading_deg: Option<f64>,
    pub length_m: Option<u32>,
    pub beam_m: Option<u32>,
    pub draught_m: Option<f64>,
    pub destination: Option<String>,
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
    pub vessel_count: usize,
    pub with_position: usize,
    pub vessels: Vec<VesselView>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ais::message::{unarmor, AisMessage, Bits};

    fn parse(payload: &str, fill: u8) -> AisMessage {
        let (bytes, len) = unarmor(payload, fill).unwrap();
        AisMessage::parse(&Bits::new(&bytes, len)).unwrap()
    }

    #[test]
    fn merges_position_and_static() {
        let mut t = Tracker::new(Some((47.6, -122.35)), 200.0, 600.0, 900.0);
        t.ingest(&parse("177KQJ5000G?tO`K>RA1wUbN0TKH", 0), -20.0, 1.0);
        t.ingest(
            &parse("55?MbV02;H;s<HtKR20EHE:0@T4@Dn2222222216L961O5Gf0NSQEp6ClRp88888888880", 2),
            -20.0,
            2.0,
        );
        let snap = t.snapshot(3.0);
        // two MMSIs (position vs static example differ)
        let moored = snap.vessels.iter().find(|v| v.mmsi == 477553000).unwrap();
        assert!(moored.lat.is_some());
        assert_eq!(moored.nav_status_label.as_deref(), Some("moored"));
        assert_eq!(snap.with_position, 1);
    }

    #[test]
    fn expires_and_range_gates() {
        let mut t = Tracker::new(Some((0.0, 0.0)), 5.0, 600.0, 30.0);
        t.ingest(&parse("177KQJ5000G?tO`K>RA1wUbN0TKH", 0), -20.0, 1.0);
        // far from (0,0) -> position rejected, but the vessel is still tracked
        let snap = t.snapshot(2.0);
        assert_eq!(snap.vessel_count, 1);
        assert_eq!(snap.with_position, 0);
        t.prune(40.0);
        assert_eq!(t.snapshot(40.0).vessel_count, 0);
    }
}
