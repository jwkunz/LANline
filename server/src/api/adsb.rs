//! ADS-B track export. Unauthenticated, read-only — same posture as
//! `GET /api/v1/radio/status`. The live tracks come from the `adsb` mode
//! pipeline; when another mode is running these simply report an empty table.

use crate::adsb::Snapshot;
use crate::flight::FlightInfo;
use crate::model::now_utc;
use crate::state::AppState;
use axum::extract::{Path, Query, State};
use axum::Json;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

#[derive(Serialize)]
pub struct AircraftResponse {
    #[serde(with = "time::serde::rfc3339")]
    pub time: OffsetDateTime,
    pub mode: String,
    pub running: bool,
    #[serde(flatten)]
    pub snapshot: Snapshot,
}

pub async fn aircraft(State(st): State<AppState>) -> Json<AircraftResponse> {
    let (mode, running) = {
        let r = st.radio.lock().unwrap();
        (r.mode.clone(), st.radio_mgr.is_running())
    };
    let adsb = st.radio_mgr.adsb();
    let snapshot = adsb.tracker.lock().unwrap().snapshot(adsb.now_s());
    Json(AircraftResponse { time: now_utc(), mode, running, snapshot })
}

#[derive(Serialize)]
pub struct MessagesResponse {
    #[serde(with = "time::serde::rfc3339")]
    pub time: OffsetDateTime,
    pub count: usize,
    /// Newest last. Hex of the raw Mode S frame (7 or 14 bytes).
    pub messages: Vec<String>,
}

pub async fn messages(State(st): State<AppState>) -> Json<MessagesResponse> {
    let messages = st.radio_mgr.adsb().recent_hex();
    Json(MessagesResponse { time: now_utc(), count: messages.len(), messages })
}

#[derive(Deserialize)]
pub struct FlightQuery {
    /// Decoded flight id, if the client has one — enables the route lookup.
    pub callsign: Option<String>,
}

/// `GET /api/v1/adsb/flight/{icao}?callsign=SWA123` — tail number, type,
/// owner and route for one contact, via adsbdb.com (see `crate::flight`).
/// Always 200: an unavailable result carries a `reason`
/// (`disabled` / `offline` / `unknown` / `pending`). Unauthenticated,
/// read-only, same posture as the other `/adsb/*` reads.
pub async fn flight(
    State(st): State<AppState>,
    Path(icao): Path<String>,
    Query(q): Query<FlightQuery>,
) -> Json<FlightInfo> {
    // 6 hex chars, nothing else — keep junk out of the upstream URL.
    let icao = icao.trim().to_ascii_lowercase();
    if icao.len() != 6 || !icao.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Json(FlightInfo { available: false, reason: Some("unknown".into()), ..Default::default() });
    }
    let callsign: Option<String> = q.callsign.map(|c| {
        c.trim()
            .to_ascii_uppercase()
            .chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .take(8)
            .collect()
    });
    Json(st.flight.lookup(&icao, callsign.as_deref()).await)
}
