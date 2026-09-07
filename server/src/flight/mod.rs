//! ADS-B flight enrichment: given a Mode S ICAO hex (and, when known, the
//! decoded callsign) fetch tail number / type / owner / route from
//! adsbdb.com. This is the only part of the server that reaches the
//! internet — it is opt-out (`--flight-lookup false` /
//! `LANLINE_FLIGHT_LOOKUP=0`) and it discloses to adsbdb which aircraft this
//! receiver is watching.
//!
//! One `FlightLookup` is shared by every client via `AppState`; results are
//! cached with a TTL so a screen full of aircraft and several browser tabs
//! do not each hammer the upstream. `GET /api/v1/adsb/flight/{icao}` is the
//! only caller (see `api::adsb::flight`).

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// How long a good hit stays fresh (registrations/owners barely change).
const HIT_TTL: Duration = Duration::from_secs(24 * 60 * 60);
/// A miss, or a hit missing the route, is retried sooner — a flight gains a
/// filed route mid-air.
const SOFT_TTL: Duration = Duration::from_secs(60 * 60);
/// A network failure is cached briefly so a dead uplink does not stall every
/// tap for six seconds.
const FAIL_TTL: Duration = Duration::from_secs(30);
/// Flush the cache wholesale past this many entries (a long session watching
/// a busy sky). Simpler than per-entry LRU and plenty for this.
const CACHE_CAP: usize = 4096;

/// Trimmed, client-facing enrichment for one aircraft. Every field is
/// optional; `available` is false when nothing useful came back and `reason`
/// then says why.
#[derive(Clone, Debug, Default, Serialize)]
pub struct FlightInfo {
    pub available: bool,
    /// `"disabled"` | `"offline"` | `"unknown"` | `"pending"`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub registration: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub aircraft_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icao_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub manufacturer: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub owner_country: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub photo_thumb_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub airline: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub route: Option<Route>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Route {
    pub origin: Airport,
    pub destination: Airport,
}

#[derive(Clone, Debug, Serialize)]
pub struct Airport {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub city: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub iata: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icao: Option<String>,
}

impl FlightInfo {
    fn reason(r: &str) -> Self {
        FlightInfo { available: false, reason: Some(r.to_string()), ..Default::default() }
    }
}

struct CacheEntry {
    fetched: Instant,
    ttl: Duration,
    /// The callsign this entry was fetched with — a different one forces a
    /// refresh so the route can update.
    callsign: Option<String>,
    info: FlightInfo,
}

pub struct FlightLookup {
    enabled: bool,
    base_url: String,
    client: reqwest::Client,
    cache: Mutex<HashMap<String, CacheEntry>>,
    inflight: Mutex<HashSet<String>>,
}

impl FlightLookup {
    pub fn new(cfg: &crate::config::Config) -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(6))
            .user_agent(concat!("lanline/", env!("CARGO_PKG_VERSION")))
            .build()
            .unwrap_or_default();
        FlightLookup {
            enabled: cfg.flight_lookup,
            base_url: cfg.flight_lookup_url.trim_end_matches('/').to_string(),
            client,
            cache: Mutex::new(HashMap::new()),
            inflight: Mutex::new(HashSet::new()),
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// Look up one aircraft. `icao` is a 6-hex Mode S address; `callsign` is
    /// the decoded flight id when the tracker has one. Never errors — an
    /// unavailable result carries a `reason`.
    pub async fn lookup(&self, icao: &str, callsign: Option<&str>) -> FlightInfo {
        if !self.enabled {
            return FlightInfo::reason("disabled");
        }
        let icao = icao.to_ascii_lowercase();
        let cs = callsign
            .map(|c| c.trim().to_ascii_uppercase())
            .filter(|c| !c.is_empty());

        if let Some(hit) = self.cached(&icao, &cs) {
            return hit;
        }

        // De-dupe concurrent taps on the same aircraft: the first caller
        // fetches, the rest get "pending" and re-poll.
        if !self.inflight.lock().unwrap().insert(icao.clone()) {
            return FlightInfo::reason("pending");
        }

        let info = self.fetch(&icao, cs.as_deref()).await;
        self.inflight.lock().unwrap().remove(&icao);

        let ttl = if info.reason.as_deref() == Some("offline") {
            FAIL_TTL
        } else if info.available && info.route.is_some() {
            HIT_TTL
        } else {
            // A hit still missing its route, or a clean miss — retry soon.
            SOFT_TTL
        };
        let mut cache = self.cache.lock().unwrap();
        if cache.len() >= CACHE_CAP {
            cache.clear();
        }
        cache.insert(
            icao,
            CacheEntry { fetched: Instant::now(), ttl, callsign: cs, info: info.clone() },
        );
        info
    }

    fn cached(&self, icao: &str, cs: &Option<String>) -> Option<FlightInfo> {
        let cache = self.cache.lock().unwrap();
        let e = cache.get(icao)?;
        (e.fetched.elapsed() < e.ttl && &e.callsign == cs).then(|| e.info.clone())
    }

    async fn fetch(&self, icao: &str, callsign: Option<&str>) -> FlightInfo {
        let ac_url = format!("{}/aircraft/{}", self.base_url, icao);
        let ac_fut = self.get_json::<AircraftResp>(&ac_url);
        let rt_fut = async {
            match callsign {
                Some(cs) => {
                    self.get_json::<RouteResp>(&format!("{}/callsign/{}", self.base_url, cs)).await
                }
                None => Ok(None),
            }
        };
        let (ac_res, rt_res) = tokio::join!(ac_fut, rt_fut);

        let mut info = FlightInfo::default();
        let mut net_err = false;
        match ac_res {
            Ok(Some(r)) => {
                let a = r.response.aircraft;
                info.registration = clean(a.registration);
                info.aircraft_type = clean(a.aircraft_type);
                info.icao_type = clean(a.icao_type);
                info.manufacturer = clean(a.manufacturer);
                info.owner = clean(a.registered_owner);
                info.owner_country = clean(a.registered_owner_country_name);
                info.photo_thumb_url =
                    clean(a.url_photo_thumbnail).filter(|u| u.starts_with("https://"));
            }
            Ok(None) => {}
            Err(()) => net_err = true,
        }
        if let Ok(Some(r)) = rt_res {
            let fr = r.response.flightroute;
            info.airline = fr.airline.and_then(|x| clean(x.name));
            if let (Some(o), Some(d)) = (fr.origin, fr.destination) {
                info.route = Some(Route { origin: map_airport(o), destination: map_airport(d) });
            }
        }

        info.available =
            info.registration.is_some() || info.aircraft_type.is_some() || info.route.is_some();
        if !info.available {
            info.reason = Some(if net_err { "offline" } else { "unknown" }.to_string());
        }
        info
    }

    /// `Ok(Some)` parsed, `Ok(None)` upstream said no (404 / other non-2xx),
    /// `Err(())` network or decode failure.
    async fn get_json<T: DeserializeOwned>(&self, url: &str) -> Result<Option<T>, ()> {
        let resp = self.client.get(url).send().await.map_err(|e| {
            tracing::debug!("flight lookup: {url}: {e}");
        })?;
        if !resp.status().is_success() {
            if resp.status() != reqwest::StatusCode::NOT_FOUND {
                tracing::debug!("flight lookup: {url}: HTTP {}", resp.status());
            }
            return Ok(None);
        }
        match resp.json::<T>().await {
            Ok(v) => Ok(Some(v)),
            Err(e) => {
                tracing::debug!("flight lookup: {url}: bad body: {e}");
                Err(())
            }
        }
    }
}

fn clean(s: Option<String>) -> Option<String> {
    s.map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty() && !s.eq_ignore_ascii_case("unknown"))
}

fn map_airport(a: AdsbdbAirport) -> Airport {
    Airport {
        name: clean(a.name),
        city: clean(a.municipality),
        iata: clean(a.iata_code),
        icao: clean(a.icao_code),
    }
}

// --- adsbdb.com wire shapes (only the fields we surface) ------------------

#[derive(Deserialize)]
struct AircraftResp {
    response: AircraftWrap,
}
#[derive(Deserialize)]
struct AircraftWrap {
    aircraft: AdsbdbAircraft,
}
#[derive(Deserialize)]
struct AdsbdbAircraft {
    #[serde(rename = "type")]
    aircraft_type: Option<String>,
    icao_type: Option<String>,
    manufacturer: Option<String>,
    registration: Option<String>,
    registered_owner: Option<String>,
    registered_owner_country_name: Option<String>,
    url_photo_thumbnail: Option<String>,
}

#[derive(Deserialize)]
struct RouteResp {
    response: RouteWrap,
}
#[derive(Deserialize)]
struct RouteWrap {
    flightroute: FlightRoute,
}
#[derive(Deserialize)]
struct FlightRoute {
    airline: Option<Airline>,
    origin: Option<AdsbdbAirport>,
    destination: Option<AdsbdbAirport>,
}
#[derive(Deserialize)]
struct Airline {
    name: Option<String>,
}
#[derive(Deserialize)]
struct AdsbdbAirport {
    name: Option<String>,
    municipality: Option<String>,
    iata_code: Option<String>,
    icao_code: Option<String>,
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
        let fl = FlightLookup::new(&c);
        let info = fl.lookup("A835AF", Some("SWA1")).await;
        assert!(!info.available);
        assert_eq!(info.reason.as_deref(), Some("disabled"));
    }

    #[test]
    fn parses_adsbdb_aircraft_hit() {
        let raw = r#"{"response":{"aircraft":{"type":"G650 ER","icao_type":"G650",
            "manufacturer":"Gulfstream Aerospace","registration":"N628TS",
            "registered_owner":"Falcon Landing LLC",
            "registered_owner_country_name":"United States",
            "url_photo_thumbnail":"https://airport-data.com/x.jpg"}}}"#;
        let r: AircraftResp = serde_json::from_str(raw).unwrap();
        let a = r.response.aircraft;
        assert_eq!(a.registration.as_deref(), Some("N628TS"));
        assert_eq!(a.aircraft_type.as_deref(), Some("G650 ER"));
        assert_eq!(clean(a.icao_type).as_deref(), Some("G650"));
    }

    #[test]
    fn parses_adsbdb_route_hit() {
        let raw = r#"{"response":{"flightroute":{"callsign":"UAL1",
            "airline":{"name":"United Airlines"},
            "origin":{"iata_code":"SFO","icao_code":"KSFO",
                "name":"San Francisco International Airport","municipality":"San Francisco"},
            "destination":{"iata_code":"SIN","icao_code":"WSSS",
                "name":"Singapore Changi Airport","municipality":"Singapore"}}}}"#;
        let r: RouteResp = serde_json::from_str(raw).unwrap();
        let fr = r.response.flightroute;
        assert_eq!(fr.airline.unwrap().name.as_deref(), Some("United Airlines"));
        let o = map_airport(fr.origin.unwrap());
        assert_eq!(o.iata.as_deref(), Some("SFO"));
        assert_eq!(o.city.as_deref(), Some("San Francisco"));
    }

    #[test]
    fn clean_drops_blank_and_unknown() {
        assert_eq!(clean(Some("   ".into())), None);
        assert_eq!(clean(Some("Unknown".into())), None);
        assert_eq!(clean(Some(" N1 ".into())).as_deref(), Some("N1"));
    }
}
