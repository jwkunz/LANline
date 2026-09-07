//! SSTV image export. Unauthenticated, read-only (same posture as
//! `GET /api/v1/apt/image`). Populated only while the `sstv` mode pipeline
//! runs; a `Stop radio` + `Start radio` cycle starts a fresh capture.

use crate::sstv::Status;
use crate::state::AppState;
use axum::extract::State;
use axum::http::header;
use axum::response::{IntoResponse, Response};
use axum::Json;

/// `[u32 width LE][u32 height LE][row-major RGBA bytes]`.
pub async fn image(State(st): State<AppState>) -> Response {
    let bytes = st.radio_mgr.sstv().image.lock().unwrap().encode();
    (
        [
            (header::CONTENT_TYPE, "application/octet-stream"),
            (header::CACHE_CONTROL, "no-store"),
        ],
        bytes,
    )
        .into_response()
}

pub async fn status(State(st): State<AppState>) -> Json<Status> {
    Json(st.radio_mgr.sstv().image.lock().unwrap().status())
}
