//! APRS station-track export (unauthenticated, read-only) + APRS transmit
//! (`POST /aprs/tx`, session-gated, behind `--enable-tx`).

use crate::aprs::Snapshot;
use crate::error::{ApiError, ApiResult};
use crate::model::now_utc;
use crate::sessions::AuthedSession;
use crate::state::AppState;
use axum::extract::State;
use axum::Json;
use serde::Serialize;
use serde_json::Value;
use time::OffsetDateTime;

#[derive(Serialize)]
pub struct StationsResponse {
    #[serde(with = "time::serde::rfc3339")]
    pub time: OffsetDateTime,
    pub mode: String,
    pub running: bool,
    #[serde(flatten)]
    pub snapshot: Snapshot,
}

pub async fn stations(State(st): State<AppState>) -> Json<StationsResponse> {
    let (mode, running) = {
        let r = st.radio.lock().unwrap();
        (r.mode.clone(), st.radio_mgr.is_running())
    };
    let aprs = st.radio_mgr.aprs();
    let snapshot = aprs.tracker.lock().unwrap().snapshot(aprs.now_s());
    Json(StationsResponse { time: now_utc(), mode, running, snapshot })
}

#[derive(Serialize)]
pub struct PacketsResponse {
    #[serde(with = "time::serde::rfc3339")]
    pub time: OffsetDateTime,
    pub count: usize,
    /// Newest last. TNC2 monitor lines (`SRC>DEST,PATH:info`, no CRLF).
    pub packets: Vec<String>,
}

pub async fn packets(State(st): State<AppState>) -> Json<PacketsResponse> {
    let packets: Vec<String> = st.radio_mgr.aprs().packets.lock().unwrap().iter().cloned().collect();
    Json(PacketsResponse { time: now_utc(), count: packets.len(), packets })
}

/// Transmit one APRS packet on the running `aprs` pipeline (a half-duplex
/// burst — RX drops for ~1 s). Requires `--enable-tx`, `aprs` mode, a
/// tx-capable device, and the radio running.
///
/// Body: `source` (required callsign). Optional `dest` (default `APZLNL`),
/// `path` (digipeater aliases, default direct), `gain_db`, `deviation_hz`.
/// Exactly one payload: `info` (raw APRS info text), `message_to` +
/// `message_text` (an APRS text message), or `lat` + `lon` (+ `symbol`,
/// `comment`) for an uncompressed position beacon.
pub async fn tx(
    State(st): State<AppState>,
    auth: AuthedSession,
    body: Option<Json<Value>>,
) -> ApiResult<Json<Value>> {
    if !st.radio_mgr.tx_enabled() {
        return Err(ApiError::forbidden("transmit is disabled on this server — see --enable-tx"));
    }
    let (mode, running, freq_hz, gain_default, dev_default) = {
        let r = st.radio.lock().unwrap();
        let g = r.mode_params.get("tx_gain_db").and_then(Value::as_f64).unwrap_or(30.0);
        let d = r.mode_params.get("tx_deviation_hz").and_then(Value::as_f64).unwrap_or(3_000.0);
        let f = if r.frequency_hz >= 1_000_000 {
            r.frequency_hz
        } else {
            crate::aprs::APRS_HZ as u64
        };
        (r.mode.clone(), r.running, f, g, d)
    };
    if mode != "aprs" {
        return Err(ApiError::bad_request("APRS transmit requires `aprs` mode"));
    }
    if !running {
        return Err(ApiError::conflict("start the radio before transmitting"));
    }
    if !st.registry.selected().map(|d| d.tx_capable).unwrap_or(false) {
        return Err(ApiError::device_unavailable("selected device cannot transmit"));
    }

    let body = body.map(|Json(v)| v).unwrap_or(Value::Null);
    let source = body
        .get("source")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| ApiError::bad_request("`source` callsign is required"))?
        .to_ascii_uppercase();
    let dest = body
        .get("dest")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("APZLNL")
        .to_ascii_uppercase();
    let path: Vec<String> = body
        .get("path")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter().filter_map(Value::as_str).map(|s| s.trim().to_ascii_uppercase()).collect()
        })
        .unwrap_or_default();
    let gain_db = body.get("gain_db").and_then(Value::as_f64).unwrap_or(gain_default).clamp(0.0, 89.0);
    let deviation_hz =
        body.get("deviation_hz").and_then(Value::as_f64).unwrap_or(dev_default).clamp(1_000.0, 5_000.0);

    // Exactly one payload form.
    let info: Vec<u8> = if let Some(raw) = body.get("info").and_then(Value::as_str) {
        raw.as_bytes().to_vec()
    } else if let (Some(to), Some(text)) = (
        body.get("message_to").and_then(Value::as_str),
        body.get("message_text").and_then(Value::as_str),
    ) {
        if text.is_empty() {
            return Err(ApiError::bad_request("`message_text` is empty"));
        }
        crate::aprs::tx::message_info(to.trim(), text)
    } else if let (Some(lat), Some(lon)) =
        (body.get("lat").and_then(Value::as_f64), body.get("lon").and_then(Value::as_f64))
    {
        let symbol = body.get("symbol").and_then(Value::as_str).unwrap_or("/>");
        let comment = body.get("comment").and_then(Value::as_str).unwrap_or("");
        crate::aprs::tx::position_info(lat, lon, symbol, comment).into_bytes()
    } else {
        return Err(ApiError::bad_request(
            "provide one of: `info`, `message_to`+`message_text`, or `lat`+`lon`",
        ));
    };

    let path_refs: Vec<&str> = path.iter().map(String::as_str).collect();
    let frame = crate::aprs::tx::ui_frame(&source, &dest, &path_refs, &info);
    let mut tnc2 = format!("{source}>{dest}");
    for p in &path {
        tnc2.push(',');
        tnc2.push_str(p);
    }
    tnc2.push(':');
    tnc2.push_str(&String::from_utf8_lossy(&info));
    let bytes = frame.len();

    st.radio_mgr
        .aprs_tx(frame, tnc2.clone(), gain_db, deviation_hz)
        .map_err(ApiError::conflict)?;

    let client = st.sessions.get(auth.id).map(|s| s.client.name).unwrap_or_default();
    st.radio_mgr.tx_log_burst(client, "aprs".into(), freq_hz, tnc2.clone());

    Ok(Json(serde_json::json!({ "transmitted": true, "tnc2": tnc2, "bytes": bytes })))
}
