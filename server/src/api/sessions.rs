use crate::error::{ApiError, ApiResult};
use crate::model::*;
use crate::sessions::AuthedSession;
use crate::state::AppState;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use uuid::Uuid;

/// A caller may only act on its own session.
pub(crate) fn require_own(auth: &AuthedSession, id: Uuid) -> ApiResult<()> {
    if auth.id == id {
        Ok(())
    } else {
        Err(ApiError::not_found("unknown or expired session"))
    }
}

pub async fn create(
    State(st): State<AppState>,
    Json(req): Json<CreateSessionRequest>,
) -> ApiResult<(StatusCode, Json<CreateSessionResponse>)> {
    let resp = st.sessions.create(req.client)?;
    Ok((StatusCode::CREATED, Json(resp)))
}

pub async fn list(
    State(st): State<AppState>,
    _auth: AuthedSession,
) -> Json<Vec<SessionSummary>> {
    Json(st.sessions.list())
}

pub async fn get_one(
    State(st): State<AppState>,
    auth: AuthedSession,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<SessionSummary>> {
    require_own(&auth, id)?;
    st.sessions
        .get(id)
        .map(Json)
        .ok_or_else(|| ApiError::not_found("unknown or expired session"))
}

pub async fn heartbeat(
    State(st): State<AppState>,
    auth: AuthedSession,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<HeartbeatResponse>> {
    require_own(&auth, id)?;
    st.sessions.heartbeat(id).map(Json)
}

pub async fn delete(
    State(st): State<AppState>,
    auth: AuthedSession,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    require_own(&auth, id)?;
    st.sessions.delete(id);
    Ok(StatusCode::NO_CONTENT)
}
