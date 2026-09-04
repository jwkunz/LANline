//! AIS vessel-track export. Unauthenticated, read-only (same posture as
//! `GET /api/v1/radio/status`). Live only while the `ais` mode pipeline runs.

use crate::ais::Snapshot;
use crate::model::now_utc;
use crate::state::AppState;
use axum::extract::State;
use axum::Json;
use serde::Serialize;
use time::OffsetDateTime;

#[derive(Serialize)]
pub struct VesselsResponse {
    #[serde(with = "time::serde::rfc3339")]
    pub time: OffsetDateTime,
    pub mode: String,
    pub running: bool,
    #[serde(flatten)]
    pub snapshot: Snapshot,
}

pub async fn vessels(State(st): State<AppState>) -> Json<VesselsResponse> {
    let (mode, running) = {
        let r = st.radio.lock().unwrap();
        (r.mode.clone(), st.radio_mgr.is_running())
    };
    let ais = st.radio_mgr.ais();
    let snapshot = ais.tracker.lock().unwrap().snapshot(ais.now_s());
    Json(VesselsResponse { time: now_utc(), mode, running, snapshot })
}

#[derive(Serialize)]
pub struct SentencesResponse {
    #[serde(with = "time::serde::rfc3339")]
    pub time: OffsetDateTime,
    pub count: usize,
    /// Newest last. `!AIVDM` sentences (checksummed, no trailing CRLF).
    pub sentences: Vec<String>,
}

pub async fn sentences(State(st): State<AppState>) -> Json<SentencesResponse> {
    let sentences = st.radio_mgr.ais().recent_sentences();
    Json(SentencesResponse { time: now_utc(), count: sentences.len(), sentences })
}
