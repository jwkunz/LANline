//! WebRTC signaling endpoints.
//!
//! Phase 1a: the routes exist and enforce auth/ownership so the client code
//! path is real, but `offer` returns 501 until the WebRTC peer + Opus track
//! land in phase 1b.

use crate::api::sessions::require_own;
use crate::error::{ApiError, ApiResult};
use crate::model::*;
use crate::sessions::AuthedSession;
use crate::state::AppState;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use uuid::Uuid;

pub async fn offer(
    State(_st): State<AppState>,
    auth: AuthedSession,
    Path(id): Path<Uuid>,
    Json(_offer): Json<SdpMessage>,
) -> ApiResult<Json<SdpMessage>> {
    require_own(&auth, id)?;
    Err(ApiError::not_implemented(
        "WebRTC audio negotiation arrives in phase 1b",
    ))
}

pub async fn ice(
    State(_st): State<AppState>,
    auth: AuthedSession,
    Path(id): Path<Uuid>,
    Json(_ice): Json<IceMessage>,
) -> ApiResult<StatusCode> {
    require_own(&auth, id)?;
    // Non-trickle in phase 1: accept and ignore.
    Ok(StatusCode::ACCEPTED)
}

pub async fn state(
    State(st): State<AppState>,
    auth: AuthedSession,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<AudioStateResponse>> {
    require_own(&auth, id)?;
    let conn = st.sessions.audio_state(id).unwrap_or(AudioConnState::Idle);
    Ok(Json(AudioStateResponse {
        state: conn,
        ice_state: "new",
        dtls_state: "new",
        packets_sent: 0,
        bytes_sent: 0,
    }))
}

pub async fn close(
    State(st): State<AppState>,
    auth: AuthedSession,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    require_own(&auth, id)?;
    st.sessions.set_audio_state(id, AudioConnState::Closed);
    Ok(StatusCode::NO_CONTENT)
}
