use crate::model::ModeInfo;
use axum::Json;

pub async fn list() -> Json<Vec<ModeInfo>> {
    Json(crate::catalog::modes())
}
