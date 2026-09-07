//! SSTV image export (unauthenticated, read-only — same posture as
//! `GET /api/v1/apt/image`) + SSTV transmit (`POST /sstv/tx`, session-gated,
//! behind `--enable-tx`).

use crate::error::ApiError;
use crate::sessions::AuthedSession;
use crate::sstv::Status;
use crate::state::AppState;
use axum::body::Bytes;
use axum::extract::{Query, State};
use axum::http::header;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::Value;

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

#[derive(Deserialize)]
pub struct TxQuery {
    /// SSTV mode key: `scottie1`, `scottie2`, `scottiedx`, `martin1`,
    /// `martin2`, `robot36`, `pd120`, `pd180`.
    pub mode: String,
}

/// Transmit an image as FM SSTV on the running `sstv` pipeline — a streamed
/// half-duplex burst (RX drops for the picture's length, 30 s–4 min).
/// Requires `--enable-tx`, `sstv` mode, a tx-capable device, and the radio
/// running.
///
/// Body: `application/octet-stream`, raw row-major RGB (`width*height*3` bytes
/// for the chosen mode's geometry — the caller resizes to fit). TX gain and
/// FM deviation come from the running `mode_params` (`tx_gain_db`,
/// `tx_deviation_hz`).
pub async fn tx(
    State(st): State<AppState>,
    auth: AuthedSession,
    Query(q): Query<TxQuery>,
    body: Bytes,
) -> Response {
    if !st.radio_mgr.tx_enabled() {
        return ApiError::forbidden("transmit is disabled on this server — see --enable-tx")
            .into_response();
    }

    let Some(mode) = crate::sstv::modes::by_key(&q.mode) else {
        return ApiError::bad_request(format!("unknown SSTV mode `{}`", q.mode)).into_response();
    };

    let (radio_mode, running, freq_hz, gain_db, deviation_hz) = {
        let r = st.radio.lock().unwrap();
        let g = r.mode_params.get("tx_gain_db").and_then(Value::as_f64).unwrap_or(30.0);
        let d =
            r.mode_params.get("tx_deviation_hz").and_then(Value::as_f64).unwrap_or(5_000.0);
        let f = if r.frequency_hz >= 1_000_000 { r.frequency_hz } else { 144_500_000 };
        (r.mode.clone(), r.running, f, g.clamp(0.0, 89.0), d.clamp(1_000.0, 8_000.0))
    };
    if radio_mode != "sstv" {
        return ApiError::bad_request("SSTV transmit requires `sstv` mode").into_response();
    }
    if !running {
        return ApiError::conflict("start the radio before transmitting").into_response();
    }
    if !st.registry.selected().map(|d| d.tx_capable).unwrap_or(false) {
        return ApiError::device_unavailable("selected device cannot transmit").into_response();
    }

    let want = mode.width * mode.height * 3;
    if body.len() != want {
        return ApiError::bad_request(format!(
            "{} needs {want} RGB bytes ({}×{}×3), got {}",
            mode.name,
            mode.width,
            mode.height,
            body.len()
        ))
        .into_response();
    }

    // Rough on-air time: VIS header (~0.9 s) + one line per output row.
    let secs = 0.9 + mode.line * mode.height as f64;

    if let Err(e) =
        st.radio_mgr.sstv_tx(body.to_vec(), q.mode.clone(), gain_db, deviation_hz)
    {
        return ApiError::conflict(e).into_response();
    }

    let client = st.sessions.get(auth.id).map(|s| s.client.name).unwrap_or_default();
    let detail = format!("{} ({}×{}, ~{:.0} s)", mode.name, mode.width, mode.height, secs);
    st.radio_mgr.tx_log_burst(client, "sstv".into(), freq_hz, detail);

    Json(serde_json::json!({
        "transmitted": true,
        "mode": mode.name,
        "secs": (secs * 10.0).round() / 10.0,
        "bytes": want,
    }))
    .into_response()
}
