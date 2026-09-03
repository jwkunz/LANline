use crate::error::{ApiError, ApiResult};
use crate::model::*;
use crate::sessions::AuthedSession;
use crate::state::AppState;
use crate::util::merge_json;
use axum::extract::State;
use axum::Json;
use serde_json::Value;

pub async fn get_radio(State(st): State<AppState>) -> Json<RadioConfig> {
    Json(st.radio.lock().unwrap().clone())
}

pub async fn patch_radio(
    State(st): State<AppState>,
    _auth: AuthedSession,
    Json(mut patch): Json<Value>,
) -> ApiResult<Json<RadioConfig>> {
    let obj = patch
        .as_object_mut()
        .ok_or_else(|| ApiError::bad_request("body must be a JSON object"))?;

    // Read-only fields cannot be patched.
    obj.remove("running");
    if let Some(tuner) = obj.get_mut("tuner").and_then(Value::as_object_mut) {
        tuner.remove("device_id");
    }

    let new_mode = obj.get("mode").and_then(Value::as_str).map(str::to_string);
    let has_mode_params = obj.contains_key("mode_params");

    let mut radio = st.radio.lock().unwrap();
    let mut merged = serde_json::to_value(&*radio).map_err(|e| ApiError::internal(e.to_string()))?;
    merge_json(&mut merged, &patch);

    // Switching mode without supplying params resets them to that mode's
    // defaults.
    if let Some(mode) = &new_mode {
        if !has_mode_params {
            merged["mode_params"] = crate::catalog::default_mode_params(mode);
        }
    }

    let new_cfg: RadioConfig = serde_json::from_value(merged)
        .map_err(|e| ApiError::invalid_parameter(e.to_string()))?;

    // Phase 1a validates structure + the mode id. Device-range and
    // mode-parameter validation lands with the real pipeline in phase 1c.
    if !crate::catalog::modes().iter().any(|m| m.id == new_cfg.mode) {
        return Err(ApiError::invalid_parameter(format!("unknown mode `{}`", new_cfg.mode))
            .with_details(serde_json::json!({
                "field": "mode",
                "allowed": crate::catalog::modes().iter().map(|m| m.id).collect::<Vec<_>>()
            })));
    }

    *radio = new_cfg;
    Ok(Json(radio.clone()))
}

pub async fn start(State(st): State<AppState>, _auth: AuthedSession) -> ApiResult<Json<RadioConfig>> {
    let mut radio = st.radio.lock().unwrap();
    if radio.mode != "debug_tone" {
        let ready = st
            .registry
            .selected()
            .map(|d| d.status == DeviceStatus::Ready)
            .unwrap_or(false);
        if !ready {
            return Err(ApiError::device_unavailable(
                "no ready SDR; select a device or use mode=debug_tone",
            ));
        }
    }
    // Phase 1a: flag only. The DSP + Opus pipeline starts here from phase 1b.
    radio.running = true;
    Ok(Json(radio.clone()))
}

pub async fn stop(State(st): State<AppState>, _auth: AuthedSession) -> ApiResult<Json<RadioConfig>> {
    let mut radio = st.radio.lock().unwrap();
    radio.running = false;
    Ok(Json(radio.clone()))
}

pub async fn status(State(st): State<AppState>) -> Json<RadioStatus> {
    let radio = st.radio.lock().unwrap();
    let device_status = st
        .registry
        .selected()
        .map(|d| d.status)
        .unwrap_or(DeviceStatus::Absent);
    let is_tone = radio.mode == "debug_tone";

    Json(RadioStatus {
        running: radio.running,
        mode: radio.mode.clone(),
        frequency_hz: radio.frequency_hz,
        device_status,
        dsp: DspStatus { squelch_open: is_tone, ..Default::default() },
        audio: AudioStatus {
            encoder: "opus",
            bitrate_bps: radio.audio.opus_bitrate_bps,
            frames_sent: 0,
            sample_rate_hz: radio.audio.sample_rate_hz,
            channels: radio.audio.channels,
        },
        clients: st.sessions.active_count(),
        time: now_utc(),
    })
}
