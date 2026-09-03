use crate::error::{ApiError, ApiResult};
use crate::model::*;
use crate::sessions::AuthedSession;
use crate::state::AppState;
use axum::extract::State;
use axum::Json;

pub async fn list(State(st): State<AppState>) -> Json<Vec<DeviceSummary>> {
    Json(st.registry.enumerate())
}

pub async fn selected(State(st): State<AppState>) -> ApiResult<Json<DeviceInfo>> {
    st.registry
        .selected()
        .map(Json)
        .ok_or_else(|| ApiError::not_found("no device selected"))
}

pub async fn select(
    State(st): State<AppState>,
    _auth: AuthedSession,
    Json(sel): Json<DeviceSelector>,
) -> ApiResult<Json<DeviceInfo>> {
    if st.radio.lock().unwrap().running {
        return Err(ApiError::conflict("stop the radio before selecting a device"));
    }
    let info = st.registry.select(&sel)?;
    st.radio.lock().unwrap().tuner.device_id = Some(info.id.clone());
    Ok(Json(info))
}

pub async fn health(State(st): State<AppState>) -> Json<DeviceHealth> {
    Json(st.registry.health())
}
