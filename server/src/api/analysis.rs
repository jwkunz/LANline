//! Receiver Analysis: the FFT panadapter + waterfall frame, plus IQ `.wav`
//! recording control and download. Read endpoints are unauthenticated (same
//! posture as `/radio/status`); starting/stopping a recording needs a
//! session token.

use crate::error::{ApiError, ApiResult};
use crate::state::AppState;
use axum::body::Body;
use axum::extract::{Query, State};
use axum::http::header;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

#[derive(Deserialize)]
pub struct SpectrumQuery {
    /// Client's row cursor — send back `seq` from the previous frame.
    #[serde(default)]
    since: u64,
    /// Cap on waterfall rows returned in one frame (catch-up bound).
    max_rows: Option<usize>,
}

/// Binary spectrum/waterfall frame — see `analysis::Spectrum::encode`.
pub async fn spectrum(State(st): State<AppState>, Query(q): Query<SpectrumQuery>) -> Response {
    let max_rows = q.max_rows.unwrap_or(256).clamp(1, 4000);
    let bytes = st.radio_mgr.analysis().spectrum.lock().unwrap().encode(q.since, max_rows);
    (
        [
            (header::CONTENT_TYPE, "application/octet-stream"),
            (header::CACHE_CONTROL, "no-store"),
        ],
        bytes,
    )
        .into_response()
}

pub async fn status(State(st): State<AppState>) -> Json<Value> {
    let a = st.radio_mgr.analysis();
    let (center_hz, span_hz, n_bins, seq, rows) = {
        let s = a.spectrum.lock().unwrap();
        (s.center_hz, s.span_hz, s.n_bins, s.seq, s.rows_held())
    };
    let running = st.radio_mgr.is_running() && st.radio.lock().unwrap().mode == "analysis";
    let recording = a.recorder.lock().unwrap().is_some();
    let last = a.last_recording.lock().unwrap().clone();
    Json(json!({
        "running": running,
        "center_hz": center_hz,
        "span_hz": span_hz,
        "n_bins": n_bins,
        "seq": seq,
        "rows_held": rows,
        "recording": recording,
        "last_recording": last,
    }))
}

#[derive(Deserialize)]
pub struct RecordBody {
    /// `"start"` or `"stop"`.
    action: String,
    /// Recording length cap in seconds (default 60, max 600).
    max_secs: Option<f64>,
}

/// Start or stop an IQ recording. int16 stereo (I,Q) WAV at the device rate.
pub async fn record(
    State(st): State<AppState>,
    _auth: crate::sessions::AuthedSession,
    body: Option<Json<RecordBody>>,
) -> ApiResult<Json<Value>> {
    let body = body.map(|Json(b)| b).ok_or_else(|| ApiError::bad_request("body required"))?;
    let a = st.radio_mgr.analysis();

    match body.action.as_str() {
        "stop" => {
            if let Some(rec) = a.recorder.lock().unwrap().take() {
                let info = rec.finish().map_err(|e| ApiError::internal(e.to_string()))?;
                *a.last_recording.lock().unwrap() = Some(info.clone());
                return Ok(Json(json!({ "recording": false, "last_recording": info })));
            }
            Ok(Json(json!({ "recording": false })))
        }
        "start" => {
            {
                let r = st.radio.lock().unwrap();
                if !r.running || r.mode != "analysis" {
                    return Err(ApiError::conflict(
                        "start Receiver Analysis before recording IQ",
                    ));
                }
            }
            if a.recorder.lock().unwrap().is_some() {
                return Err(ApiError::conflict("already recording"));
            }
            let (rate, center) = {
                let r = st.radio.lock().unwrap();
                (r.tuner.sample_rate_hz.round() as u32, r.frequency_hz as f64)
            };
            let max_secs = body.max_secs.unwrap_or(60.0).clamp(1.0, 600.0);
            let stamp: String = OffsetDateTime::now_utc()
                .format(&Rfc3339)
                .unwrap_or_default()
                .chars()
                .filter(|c| !matches!(c, ':' | '-'))
                .collect();
            let name = format!(
                "lanline-iq-{:.3}MHz-{:.2}Msps-{}.wav",
                center / 1e6,
                rate as f64 / 1e6,
                stamp
            );
            let path = a.iq_dir.join(&name);
            let rec = crate::analysis::iq_wav::IqRecorder::start(&path, rate, center, max_secs)
                .map_err(|e| ApiError::internal(format!("open {}: {e}", path.display())))?;
            *a.recorder.lock().unwrap() = Some(rec);
            tracing::info!("analysis: recording IQ to {} (cap {max_secs:.0}s)", path.display());
            Ok(Json(json!({
                "recording": true,
                "filename": name,
                "path": path.display().to_string(),
                "sample_rate_hz": rate,
                "max_secs": max_secs,
            })))
        }
        other => Err(ApiError::bad_request(format!("unknown action `{other}`"))),
    }
}

/// Download the most recent completed IQ recording as a file attachment.
pub async fn download(State(st): State<AppState>) -> Response {
    let info = st.radio_mgr.analysis().last_recording.lock().unwrap().clone();
    let Some(info) = info else {
        return ApiError::not_found("no completed IQ recording").into_response();
    };
    if st.radio_mgr.analysis().recorder.lock().unwrap().is_some() {
        return ApiError::conflict("stop the current recording first").into_response();
    }
    let file = match tokio::fs::File::open(&info.path).await {
        Ok(f) => f,
        Err(e) => return ApiError::internal(format!("open recording: {e}")).into_response(),
    };
    let stream = tokio_util::io::ReaderStream::new(file);
    (
        [
            (header::CONTENT_TYPE, "audio/wav".to_string()),
            (header::CONTENT_LENGTH, info.bytes.to_string()),
            (
                header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"{}\"", info.filename),
            ),
            (header::CACHE_CONTROL, "no-store".to_string()),
        ],
        Body::from_stream(stream),
    )
        .into_response()
}
