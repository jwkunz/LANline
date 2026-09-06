//! APRS station-track export. Unauthenticated, read-only (same posture as
//! `GET /api/v1/radio/status`). Live only while the `aprs` mode pipeline runs.

use crate::aprs::Snapshot;
use crate::model::now_utc;
use crate::state::AppState;
use axum::extract::State;
use axum::Json;
use serde::Serialize;
use time::OffsetDateTime;

#[derive(Serialize)]
pub struct StationsResponse {
    #[serde(with = "time::serde::rfc3339")]
    pub time: OffsetDateTime,
    pub mode: String,
    pub running: bool,
    #[serde(flatten)]
    pub snapshot: Snapshot,
}

pub async fn stations(State(st): State<AppState>) -> Json<StationsResponse> {
    let (mode, running) = {
        let r = st.radio.lock().unwrap();
        (r.mode.clone(), st.radio_mgr.is_running())
    };
    let aprs = st.radio_mgr.aprs();
    let snapshot = aprs.tracker.lock().unwrap().snapshot(aprs.now_s());
    Json(StationsResponse { time: now_utc(), mode, running, snapshot })
}

#[derive(Serialize)]
pub struct PacketsResponse {
    #[serde(with = "time::serde::rfc3339")]
    pub time: OffsetDateTime,
    pub count: usize,
    /// Newest last. TNC2 monitor lines (`SRC>DEST,PATH:info`, no CRLF).
    pub packets: Vec<String>,
}

pub async fn packets(State(st): State<AppState>) -> Json<PacketsResponse> {
    let packets: Vec<String> = st.radio_mgr.aprs().packets.lock().unwrap().iter().cloned().collect();
    Json(PacketsResponse { time: now_utc(), count: packets.len(), packets })
}
