use crate::error::{ApiError, ApiResult};
use crate::model::{Preset, RadioConfig};
use crate::sessions::AuthedSession;
use crate::state::AppState;
use crate::util::merge_json;
use axum::extract::{Path, State};
use axum::Json;

pub async fn list() -> Json<Vec<Preset>> {
    Json(crate::catalog::presets())
}

pub async fn apply(
    State(st): State<AppState>,
    _auth: AuthedSession,
    Path(id): Path<String>,
) -> ApiResult<Json<RadioConfig>> {
    let preset = crate::catalog::presets()
        .into_iter()
        .find(|p| p.id == id)
        .ok_or_else(|| ApiError::not_found(format!("no preset `{id}`")))?;

    let mut radio = st.radio.lock().unwrap();
    let mut merged = serde_json::to_value(&*radio).map_err(|e| ApiError::internal(e.to_string()))?;
    merge_json(&mut merged, &preset.config);

    let new_cfg: RadioConfig = serde_json::from_value(merged)
        .map_err(|e| ApiError::invalid_parameter(format!("preset produced an invalid config: {e}")))?;
    *radio = new_cfg;
    Ok(Json(radio.clone()))
}
