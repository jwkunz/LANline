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

    let new_cfg = {
        let radio = st.radio.lock().unwrap();
        let mut merged =
            serde_json::to_value(&*radio).map_err(|e| ApiError::internal(e.to_string()))?;
        merge_json(&mut merged, &patch);

        // Switching mode without supplying params resets them to that mode's
        // defaults.
        if let Some(mode) = &new_mode {
            if !has_mode_params {
                merged["mode_params"] = crate::catalog::default_mode_params(mode);
            }
        }

        let cfg: RadioConfig = serde_json::from_value(merged)
            .map_err(|e| ApiError::invalid_parameter(e.to_string()))?;

        if !crate::catalog::modes().iter().any(|m| m.id == cfg.mode) {
            return Err(ApiError::invalid_parameter(format!("unknown mode `{}`", cfg.mode))
                .with_details(serde_json::json!({
                    "field": "mode",
                    "allowed": crate::catalog::modes().iter().map(|m| m.id).collect::<Vec<_>>()
                })));
        }

        // Validate the tuner against the selected device's real capabilities.
        if let Some(dev) = st.registry.selected() {
            crate::registry::validate_against_device(&cfg, &dev)?;
        }
        cfg
    };

    let (old, response) = {
        let mut radio = st.radio.lock().unwrap();
        let old = radio.clone();
        *radio = new_cfg;
        (old, radio.clone())
    };

    // Hot-apply to a running pipeline: live retune/gain/filter where possible,
    // a pipeline bounce for rate/mode/antenna/LO changes.
    st.radio_mgr.apply_patch(&old, &response);

    Ok(Json(response))
}

pub async fn start(State(st): State<AppState>, _auth: AuthedSession) -> ApiResult<Json<RadioConfig>> {
    {
        let radio = st.radio.lock().unwrap();
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
    }

    st.radio_mgr.start();
    let response = {
        let mut radio = st.radio.lock().unwrap();
        radio.running = true;
        radio.clone()
    };
    Ok(Json(response))
}

pub async fn stop(State(st): State<AppState>, _auth: AuthedSession) -> ApiResult<Json<RadioConfig>> {
    st.radio_mgr.stop();
    let response = {
        let mut radio = st.radio.lock().unwrap();
        radio.running = false;
        radio.clone()
    };
    Ok(Json(response))
}

pub async fn status(State(st): State<AppState>) -> Json<RadioStatus> {
    let (mode, frequency_hz, bitrate_bps, sample_rate_hz, channels) = {
        let r = st.radio.lock().unwrap();
        (
            r.mode.clone(),
            r.frequency_hz,
            r.audio.opus_bitrate_bps,
            r.audio.sample_rate_hz,
            r.audio.channels,
        )
    };
    let tele = st.radio_mgr.telemetry();
    let running = st.radio_mgr.is_running();
    let device_status = st
        .registry
        .selected()
        .map(|d| d.status)
        .unwrap_or(DeviceStatus::Absent);

    Json(RadioStatus {
        running,
        mode,
        frequency_hz,
        device_status,
        dsp: DspStatus {
            rssi_dbfs: tele.rssi_dbfs.map(f64::from),
            snr_db: tele.snr_db.map(f64::from),
            squelch_open: tele.squelch_open,
            audio_level_dbfs: tele.audio_level_dbfs.map(f64::from),
            sample_overruns: tele.overruns,
            pipeline_latency_ms: running.then_some(20.0),
        },
        audio: AudioStatus {
            encoder: "opus",
            bitrate_bps,
            frames_sent: tele.frames_sent,
            sample_rate_hz,
            channels,
        },
        clients: st.webrtc.peer_count(),
        time: now_utc(),
    })
}
