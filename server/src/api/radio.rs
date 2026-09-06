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

/// Begin a push-to-talk transmission, keying live mic audio streamed up
/// over the session's WebRTC connection (see docs/architecture.md's PTT
/// section) — silence if none has arrived yet by the time this returns.
/// Body is optional: `{"gain_db": 10}` overrides the (deliberately
/// conservative) default of 0 dB; `{"offset_hz": -600000, "tone_hz": 100.0}`
/// key a repeater through its input with an encoded CTCSS uplink tone (`ham`
/// mode). Requires `--enable-tx` on the server, `frs` or `ham` mode, a
/// tx-capable device, and the radio already running.
pub async fn key_tx(
    State(st): State<AppState>,
    auth: AuthedSession,
    body: Option<Json<Value>>,
) -> ApiResult<Json<Value>> {
    // Server policy first, ahead of any other check — a consistent 403
    // regardless of mode/running/device state when TX is simply off.
    if !st.radio_mgr.tx_enabled() {
        return Err(ApiError::forbidden("transmit is disabled on this server — see --enable-tx"));
    }
    let (mode, running, freq_hz, deviation_hz, mic_gain) = {
        let r = st.radio.lock().unwrap();
        let deviation_hz =
            r.mode_params.get("deviation_hz").and_then(Value::as_f64).unwrap_or(2_500.0);
        let mic_gain =
            r.mode_params.get("tx_mic_gain").and_then(Value::as_f64).unwrap_or(1.0).clamp(1.0, 32.0);
        (r.mode.clone(), r.running, r.frequency_hz, deviation_hz, mic_gain)
    };
    if mode != "frs" && mode != "ham" {
        return Err(ApiError::bad_request(
            "push-to-talk transmit is only supported in `frs` and `ham` modes",
        ));
    }
    if !running {
        return Err(ApiError::conflict("start the radio before keying"));
    }
    let device_tx_capable = st.registry.selected().map(|d| d.tx_capable).unwrap_or(false);
    if !device_tx_capable {
        return Err(ApiError::device_unavailable("selected device cannot transmit"));
    }

    let body = body.map(|Json(v)| v).unwrap_or(Value::Null);
    let gain_db = body.get("gain_db").and_then(Value::as_f64).unwrap_or(0.0).clamp(0.0, 61.0);
    // Repeater split + uplink tone (ham only; ignored/zeroed for frs, a
    // simplex Part 95 service).
    let (offset_hz, tone_hz) = if mode == "ham" {
        let off = body.get("offset_hz").and_then(Value::as_f64).unwrap_or(0.0).clamp(-30e6, 30e6);
        let tone = match body.get("tone_hz").and_then(Value::as_f64).unwrap_or(0.0) {
            t if (60.0..=260.0).contains(&t) => t,
            _ => 0.0,
        };
        (off, tone)
    } else {
        (0.0, 0.0)
    };

    st.radio_mgr
        .key(gain_db, deviation_hz, mic_gain, offset_hz, tone_hz)
        .map_err(ApiError::forbidden)?;

    let client = st.sessions.get(auth.id).map(|s| s.client.name).unwrap_or_default();
    let tx_frequency_hz = (freq_hz as f64 + offset_hz).round().max(0.0) as u64;
    st.radio_mgr.tx_log_key(
        client,
        mode,
        tx_frequency_hz,
        offset_hz as i64,
        tone_hz,
        gain_db,
    );

    Ok(Json(serde_json::json!({
        "keyed": true,
        "gain_db": gain_db,
        "mic_gain": mic_gain,
        "offset_hz": offset_hz,
        "tone_hz": tone_hz,
    })))
}

/// End the current transmission early (a no-op if nothing is keyed —
/// including after it's already auto-unkeyed at the server's safety
/// timeout, so a client is always safe to call this on release).
pub async fn unkey_tx(State(st): State<AppState>, auth: AuthedSession) -> ApiResult<Json<Value>> {
    st.radio_mgr.unkey();
    if let Some(client) = st.sessions.get(auth.id).map(|s| s.client.name) {
        st.radio_mgr.tx_log_release(&client);
    }
    Ok(Json(serde_json::json!({ "keyed": false })))
}

/// The transmit audit log — every push-to-talk key this server has served
/// (bounded, in-memory), newest first.
pub async fn tx_log(
    State(st): State<AppState>,
    _auth: AuthedSession,
) -> Json<Vec<crate::model::TxLogEntry>> {
    Json(st.radio_mgr.tx_log())
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
            ctcss_tone_hz: tele.ctcss_tone_hz.map(f64::from),
            ctcss_scan_hz: tele.ctcss_scan_hz.map(f64::from),
            sample_overruns: tele.overruns,
            pipeline_latency_ms: running.then_some(20.0),
            tx_keyed: tele.tx_keyed,
        },
        audio: AudioStatus {
            encoder: "opus",
            bitrate_bps,
            frames_sent: tele.frames_sent,
            sample_rate_hz,
            channels,
        },
        recording: RecordingStatus {
            active: st.radio_mgr.audio_rec().is_active(),
            last: st.radio_mgr.audio_rec().last(),
        },
        scan: ScanStatus {
            active: tele.scanning,
            parked: tele.scan_parked,
            frequency_hz: tele.scan_freq_hz.map(|f| f.round().max(0.0) as u64),
            label: tele.scan_label.clone(),
            index: tele.scan_index,
            total: tele.scan_total,
        },
        clients: st.webrtc.peer_count(),
        time: now_utc(),
    })
}

#[derive(serde::Deserialize)]
pub struct ScanChannelIn {
    frequency_hz: f64,
    label: Option<String>,
}

#[derive(serde::Deserialize)]
pub struct ScanRangeIn {
    lo_hz: f64,
    hi_hz: f64,
    step_hz: f64,
}

#[derive(serde::Deserialize)]
pub struct ScanBody {
    /// `"start"` or `"stop"`.
    action: String,
    /// Explicit channel list. Combined with `range` if both are given.
    #[serde(default)]
    channels: Vec<ScanChannelIn>,
    /// Shorthand: expand `[lo_hz, hi_hz]` on `step_hz` into channels.
    range: Option<ScanRangeIn>,
    /// Milliseconds to dwell on each channel before deciding (default 150).
    dwell_ms: Option<u64>,
    /// Milliseconds to stay parked after the signal drops (default 2500).
    hang_ms: Option<u64>,
    /// RSSI floor (dBFS) a channel must clear to park on it (default -75).
    rssi_gate_dbfs: Option<f64>,
}

const SCAN_MAX_CHANNELS: usize = 4000;

/// Start or stop a server-side channel scan on the running SDR pipeline.
pub async fn scan(
    State(st): State<AppState>,
    _auth: AuthedSession,
    body: Option<Json<ScanBody>>,
) -> ApiResult<Json<Value>> {
    let body = body.map(|Json(b)| b).ok_or_else(|| ApiError::bad_request("body required"))?;
    match body.action.as_str() {
        "stop" => {
            st.radio_mgr.scan_stop();
            Ok(Json(serde_json::json!({ "scanning": false })))
        }
        "start" => {
            let (mode, running) = {
                let r = st.radio.lock().unwrap();
                (r.mode.clone(), r.running)
            };
            if !running || !matches!(mode.as_str(), "nbfm" | "wbfm" | "am" | "frs" | "ham") {
                return Err(ApiError::conflict(
                    "start an audio mode (nbfm/wbfm/am/frs/ham) before scanning",
                ));
            }
            let mut chans: Vec<(f64, String)> = body
                .channels
                .into_iter()
                .filter(|c| c.frequency_hz.is_finite() && c.frequency_hz > 0.0)
                .map(|c| {
                    let label = c
                        .label
                        .unwrap_or_else(|| format!("{:.4} MHz", c.frequency_hz / 1e6));
                    (c.frequency_hz, label)
                })
                .collect();
            if let Some(r) = body.range {
                let (lo, hi, step) = (r.lo_hz, r.hi_hz, r.step_hz);
                if step <= 0.0 || !(lo.is_finite() && hi.is_finite()) || hi <= lo {
                    return Err(ApiError::bad_request("invalid scan range"));
                }
                if ((hi - lo) / step) as usize > SCAN_MAX_CHANNELS {
                    return Err(ApiError::bad_request("scan range too wide for the step"));
                }
                let mut f = lo;
                while f <= hi + 1.0 {
                    chans.push((f, format!("{:.4} MHz", f / 1e6)));
                    f += step;
                }
            }
            if chans.is_empty() {
                return Err(ApiError::bad_request("no scan channels"));
            }
            chans.truncate(SCAN_MAX_CHANNELS);
            let n = chans.len();
            let dwell_ms = body.dwell_ms.unwrap_or(150);
            let hang_ms = body.hang_ms.unwrap_or(2500);
            let gate = body.rssi_gate_dbfs.unwrap_or(-75.0) as f32;
            st.radio_mgr
                .scan_start(chans, dwell_ms, hang_ms, gate)
                .map_err(ApiError::conflict)?;
            Ok(Json(serde_json::json!({ "scanning": true, "channels": n })))
        }
        other => Err(ApiError::bad_request(format!("unknown action `{other}`"))),
    }
}

/// Modes whose pipeline produces the 48 kHz demod audio a recording captures.
fn is_audio_mode(mode: &str) -> bool {
    matches!(mode, "nbfm" | "wbfm" | "am" | "frs" | "ham" | "debug_tone")
}

#[derive(serde::Deserialize)]
pub struct RecordBody {
    /// `"start"` or `"stop"`.
    action: String,
    /// Length cap in seconds (default 300, max 3600).
    max_secs: Option<f64>,
}

/// Start or stop a recording of the demodulated audio (48 kHz mono int16
/// WAV) on the server. Download the finished file from `GET /radio/recording`.
pub async fn record(
    State(st): State<AppState>,
    _auth: AuthedSession,
    body: Option<Json<RecordBody>>,
) -> ApiResult<Json<Value>> {
    let body = body.map(|Json(b)| b).ok_or_else(|| ApiError::bad_request("body required"))?;
    let rec = st.radio_mgr.audio_rec();

    match body.action.as_str() {
        "stop" => {
            let last = rec.stop();
            Ok(Json(serde_json::json!({ "active": false, "last": last })))
        }
        "start" => {
            let (mode, freq, running) = {
                let r = st.radio.lock().unwrap();
                (r.mode.clone(), r.frequency_hz, r.running)
            };
            if !running || !is_audio_mode(&mode) {
                return Err(ApiError::conflict(
                    "start an audio mode (nbfm/wbfm/am/frs/ham) before recording — \
                     Receiver Analysis has its own IQ recorder",
                ));
            }
            if rec.is_active() {
                return Err(ApiError::conflict("already recording"));
            }
            let max_secs = body.max_secs.unwrap_or(300.0).clamp(1.0, 3600.0);
            let info = rec
                .start(&mode, freq, max_secs)
                .map_err(|e| ApiError::internal(format!("open recording: {e}")))?;
            tracing::info!(
                "audio recording started: {} (cap {max_secs:.0}s)",
                info.filename
            );
            Ok(Json(serde_json::json!({
                "active": true,
                "filename": info.filename,
                "path": info.path,
                "sample_rate_hz": info.sample_rate_hz,
                "max_secs": max_secs,
            })))
        }
        other => Err(ApiError::bad_request(format!("unknown action `{other}`"))),
    }
}

/// Download the most recent completed audio recording as a file attachment.
pub async fn download(State(st): State<AppState>) -> axum::response::Response {
    use axum::body::Body;
    use axum::http::header;
    use axum::response::IntoResponse;

    let rec = st.radio_mgr.audio_rec();
    if rec.is_active() {
        return ApiError::conflict("stop the current recording first").into_response();
    }
    let Some(info) = rec.last() else {
        return ApiError::not_found("no completed audio recording").into_response();
    };
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
