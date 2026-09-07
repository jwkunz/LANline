//! AIS vessel enrichment: given an MMSI, fetch flag / tonnage / year built /
//! photo and (as a fallback) name + type from vesselfinder.com. Together
//! with `crate::flight` this is the only part of the server that reaches the
//! internet — the same opt-out applies (`--flight-lookup false` /
//! `LANLINE_FLIGHT_LOOKUP=0`).
//!
//! The AIS decoder already recovers name / callsign / type / IMO /
//! dimensions / destination from the over-the-air type-5/24 static reports;
//! this adds what is not broadcast (flag state, GT/DWT, year built, a
//! photo). One `VesselLookup` is shared via `AppState`, TTL-cached.
//! `GET /api/v1/ais/vessel/{mmsi}` is the only caller (`api::ais::vessel`).

use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::{Map, Value};
use std::collections::{HashMap, HashSet};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Vessel particulars barely change — cache a good hit for a day.
const HIT_TTL: Duration = Duration::from_secs(24 * 60 * 60);
/// A miss is retried within the hour (the vessel may start being tracked).
const SOFT_TTL: Duration = Duration::from_secs(60 * 60);
/// A network failure is cached briefly so a dead uplink doesn't stall taps.
const FAIL_TTL: Duration = Duration::from_secs(30);
const CACHE_CAP: usize = 4096;

/// Ship-photo host (paired with vesselfinder.com's public embed JSON at
/// `/api/pub/click/{mmsi}`, which is undocumented — best-effort).
const PHOTO_BASE: &str = "https://static.vesselfinder.net/ship-photo";

/// Trimmed, client-facing enrichment for one vessel. Every field is
/// optional; `available` is false when nothing identifying came back and
/// `reason` then says why.
#[derive(Clone, Debug, Default, Serialize)]
pub struct VesselInfo {
    pub available: bool,
    /// `"disabled"` | `"offline"` | `"unknown"` | `"pending"`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ship_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub flag: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub flag_iso: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub imo: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gross_tonnage: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deadweight_t: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub year_built: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub length_m: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub beam_m: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub draught_m: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub destination: Option<String>,
    /// ETA as a Unix timestamp (seconds), when the feed carries one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub eta: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub photo_url: Option<String>,
}

impl VesselInfo {
    fn reason(r: &str) -> Self {
        VesselInfo { available: false, reason: Some(r.to_string()), ..Default::default() }
    }
}

struct CacheEntry {
    fetched: Instant,
    ttl: Duration,
    info: VesselInfo,
}

pub struct VesselLookup {
    enabled: bool,
    base_url: String,
    client: reqwest::Client,
    cache: Mutex<HashMap<String, CacheEntry>>,
    inflight: Mutex<HashSet<String>>,
}

impl VesselLookup {
    pub fn new(cfg: &crate::config::Config) -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(6))
            // vesselfinder 403s the stock reqwest/curl UA; a named one is fine.
            .user_agent(concat!("lanline/", env!("CARGO_PKG_VERSION")))
            .build()
            .unwrap_or_default();
        VesselLookup {
            enabled: cfg.flight_lookup,
            base_url: cfg.vessel_lookup_url.trim_end_matches('/').to_string(),
            client,
            cache: Mutex::new(HashMap::new()),
            inflight: Mutex::new(HashSet::new()),
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// Look up one vessel by MMSI (7–9 digits). Never errors — an
    /// unavailable result carries a `reason`.
    pub async fn lookup(&self, mmsi: &str) -> VesselInfo {
        if !self.enabled {
            return VesselInfo::reason("disabled");
        }
        let mmsi = mmsi.trim().to_string();

        if let Some(hit) = self.cached(&mmsi) {
            return hit;
        }
        if !self.inflight.lock().unwrap().insert(mmsi.clone()) {
            return VesselInfo::reason("pending");
        }

        let info = self.fetch(&mmsi).await;
        self.inflight.lock().unwrap().remove(&mmsi);

        let ttl = match info.reason.as_deref() {
            Some("offline") => FAIL_TTL,
            _ if info.available => HIT_TTL,
            _ => SOFT_TTL,
        };
        let mut cache = self.cache.lock().unwrap();
        if cache.len() >= CACHE_CAP {
            cache.clear();
        }
        cache.insert(mmsi, CacheEntry { fetched: Instant::now(), ttl, info: info.clone() });
        info
    }

    fn cached(&self, mmsi: &str) -> Option<VesselInfo> {
        let cache = self.cache.lock().unwrap();
        let e = cache.get(mmsi)?;
        (e.fetched.elapsed() < e.ttl).then(|| e.info.clone())
    }

    async fn fetch(&self, mmsi: &str) -> VesselInfo {
        let url = format!("{}/api/pub/click/{}", self.base_url, mmsi);
        let map: Map<String, Value> = match self.get_json(&url).await {
            Ok(Some(m)) => m,
            Ok(None) => return VesselInfo::reason("unknown"),
            Err(()) => return VesselInfo::reason("offline"),
        };
        map_vessel(&map, mmsi)
    }

    /// `Ok(Some)` parsed, `Ok(None)` upstream said no (non-2xx),
    /// `Err(())` network or decode failure.
    async fn get_json<T: DeserializeOwned>(&self, url: &str) -> Result<Option<T>, ()> {
        let resp = self.client.get(url).send().await.map_err(|e| {
            tracing::debug!("vessel lookup: {url}: {e}");
        })?;
        if !resp.status().is_success() {
            return Ok(None);
        }
        match resp.json::<T>().await {
            Ok(v) => Ok(Some(v)),
            Err(e) => {
                tracing::debug!("vessel lookup: {url}: bad body: {e}");
                Err(())
            }
        }
    }
}

/// Map one `pub/click` object into a `VesselInfo`. When the feed has no real
/// identity for the MMSI it echoes the MMSI back as `name` with zeroed
/// numerics — that counts as a miss.
fn map_vessel(m: &Map<String, Value>, mmsi: &str) -> VesselInfo {
    let mut info = VesselInfo {
        name: str_field(m, "name").filter(|n| n != mmsi),
        ship_type: str_field(m, "type").filter(|t| !t.eq_ignore_ascii_case("unknown type")),
        flag: str_field(m, "country"),
        flag_iso: str_field(m, "a2").map(|s| s.to_ascii_uppercase()),
        imo: u32_field(m, "imo"),
        gross_tonnage: u32_field(m, "gt"),
        deadweight_t: u32_field(m, "dw"),
        year_built: u32_field(m, "y").and_then(|y| u16::try_from(y).ok()).filter(|&y| y > 1800),
        length_m: u32_field(m, "al"),
        beam_m: u32_field(m, "aw"),
        // `draught` is integer decimetres.
        draught_m: u32_field(m, "draught").map(|d| f64::from(d) / 10.0),
        destination: str_field(m, "dest"),
        eta: num_field(m, "etaTS").map(|v| v as i64).filter(|&t| t > 0),
        photo_url: m
            .get("pic")
            .and_then(Value::as_str)
            .filter(|p| !p.is_empty())
            .map(|p| format!("{PHOTO_BASE}/{p}/1")),
        available: false,
        reason: None,
    };
    info.available =
        info.name.is_some() || info.imo.is_some() || info.photo_url.is_some();
    if !info.available {
        // The feed had no real identity for this MMSI — it echoes the MMSI
        // as the name and any type/flag here is only a guess from the MID.
        // Return a bare miss, like `crate::flight` does.
        return VesselInfo::reason("unknown");
    }
    info
}

fn clean(s: Option<String>) -> Option<String> {
    s.map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty() && !s.eq_ignore_ascii_case("unknown"))
}

fn str_field(m: &Map<String, Value>, k: &str) -> Option<String> {
    clean(m.get(k).and_then(Value::as_str).map(str::to_string))
}

/// Accepts a JSON number or a numeric string; `0` / negatives / unparseable
/// become `None`.
fn num_field(m: &Map<String, Value>, k: &str) -> Option<f64> {
    let v = m.get(k)?;
    let n = v
        .as_f64()
        .or_else(|| v.as_str().and_then(|s| s.trim().parse::<f64>().ok()))?;
    n.is_finite().then_some(n)
}

fn u32_field(m: &Map<String, Value>, k: &str) -> Option<u32> {
    num_field(m, k).filter(|&n| n >= 1.0).map(|n| n as u32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    fn cfg() -> crate::config::Config {
        crate::config::Config::parse_from(["lanline-server"])
    }

    #[tokio::test]
    async fn disabled_short_circuits_without_network() {
        let mut c = cfg();
        c.flight_lookup = false;
        let vl = VesselLookup::new(&c);
        let info = vl.lookup("538005194").await;
        assert!(!info.available);
        assert_eq!(info.reason.as_deref(), Some("disabled"));
    }

    #[test]
    fn maps_a_tracked_vessel() {
        let raw = r#"{"ss":0.0,"country":"Marshall Islands","imo":9304576,"al":225,
            "pic":"9304576-538005194-6dfc","dest":"RUULU","type":"Bulk Carrier","gt":40042,
            "a2":"mh","dw":76801,"etaTS":1802077200,"draught":163,"aw":32,"name":"SFERA",
            "y":2006,"drm":"14.2","ts":1788713524}"#;
        let m: Map<String, Value> = serde_json::from_str(raw).unwrap();
        let v = map_vessel(&m, "538005194");
        assert!(v.available);
        assert_eq!(v.name.as_deref(), Some("SFERA"));
        assert_eq!(v.ship_type.as_deref(), Some("Bulk Carrier"));
        assert_eq!(v.imo, Some(9304576));
        assert_eq!(v.gross_tonnage, Some(40042));
        assert_eq!(v.deadweight_t, Some(76801));
        assert_eq!(v.year_built, Some(2006));
        assert_eq!(v.length_m, Some(225));
        assert_eq!(v.beam_m, Some(32));
        assert_eq!(v.draught_m, Some(16.3));
        assert_eq!(v.flag.as_deref(), Some("Marshall Islands"));
        assert_eq!(v.flag_iso.as_deref(), Some("MH"));
        assert_eq!(v.eta, Some(1802077200));
        assert_eq!(
            v.photo_url.as_deref(),
            Some("https://static.vesselfinder.net/ship-photo/9304576-538005194-6dfc/1")
        );
    }

    #[test]
    fn untracked_mmsi_is_a_miss() {
        // vesselfinder echoes the MMSI as the name with zeroed numerics.
        let raw = r#"{"ss":-1,"country":"United Kingdom (UK)","imo":0,"al":0,"pic":"",
            "dest":"","type":"unknown type","gt":0,"a2":"gb","dw":0,"etaTS":0,"draught":0,
            "aw":0,"name":"235103357","y":0,"ts":-1}"#;
        let m: Map<String, Value> = serde_json::from_str(raw).unwrap();
        let v = map_vessel(&m, "235103357");
        assert!(!v.available);
        assert_eq!(v.reason.as_deref(), Some("unknown"));
        assert_eq!(v.name, None);
        assert_eq!(v.imo, None);
        assert_eq!(v.photo_url, None);
    }

    #[test]
    fn num_field_takes_numbers_and_strings() {
        let m: Map<String, Value> =
            serde_json::from_str(r#"{"a":42,"b":"7.5","c":0,"d":"x","e":-3}"#).unwrap();
        assert_eq!(num_field(&m, "a"), Some(42.0));
        assert_eq!(num_field(&m, "b"), Some(7.5));
        assert_eq!(u32_field(&m, "c"), None);
        assert_eq!(num_field(&m, "d"), None);
        assert_eq!(u32_field(&m, "e"), None);
    }
}
