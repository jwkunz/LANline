//! ADS-B track export. Unauthenticated, read-only — same posture as
//! `GET /api/v1/radio/status`. The live tracks come from the `adsb` mode
//! pipeline; when another mode is running these simply report an empty table.

use crate::adsb::Snapshot;
use crate::model::now_utc;
use crate::state::AppState;
use axum::extract::State;
use axum::Json;
use serde::Serialize;
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
