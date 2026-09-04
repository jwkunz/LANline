//! WebRTC signaling endpoints.
//!
//! The browser is the offerer (recvonly audio); the server answers with a
//! send-only Opus track fed from the radio pipeline's broadcast fan-out.
//! Non-trickle ICE: the answer is returned only after gathering completes.

use super::sessions::require_own;
use crate::error::{ApiError, ApiResult};
use crate::model::*;
use crate::sessions::AuthedSession;
use crate::state::AppState;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use uuid::Uuid;

pub async fn offer(
    State(st): State<AppState>,
    auth: AuthedSession,
    Path(id): Path<Uuid>,
    Json(offer): Json<SdpMessage>,
) -> ApiResult<Json<SdpMessage>> {
    require_own(&auth, id)?;
    if offer.type_ != "offer" {
        return Err(ApiError::bad_request("expected an SDP offer"));
    }
    let answer_sdp = st
        .webrtc
        .negotiate(id, offer.sdp)
        .await
        .map_err(|e| ApiError::internal(format!("webrtc negotiation failed: {e}")))?;
    Ok(Json(SdpMessage { sdp: answer_sdp, type_: "answer".to_string() }))
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
    let snap = st.webrtc.snapshot(id).unwrap_or(AudioStateResponse {
        state: AudioConnState::Idle,
        ice_state: "new",
        dtls_state: "new",
        packets_sent: 0,
        bytes_sent: 0,
    });
    Ok(Json(snap))
}

pub async fn close(
    State(st): State<AppState>,
    auth: AuthedSession,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    require_own(&auth, id)?;
    st.webrtc.close(id).await;
    Ok(StatusCode::NO_CONTENT)
}
