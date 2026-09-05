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

    // Reset the device-specific tuner fields so the config can't carry the
    // old radio's antenna name / gain elements into the new one, and clamp
    // the sample rate into what the new device supports.
    {
        let mut r = st.radio.lock().unwrap();
        r.tuner.device_id = Some(info.id.clone());
        r.tuner.antenna = None;
        r.tuner.gain_elements_db.clear();
        r.tuner.gain_db = None;
        r.tuner.gain_mode = crate::model::GainMode::Manual;
        r.tuner.bandwidth_hz = None;
        let rates = &info.rx.sample_rate_ranges_hz;
        let in_range = |v: f64| {
            rates.iter().any(|rg| v >= rg.min - 1.0 && v <= rg.max + 1.0)
        };
        if !rates.is_empty() && !in_range(r.tuner.sample_rate_hz) {
            r.tuner.sample_rate_hz = pick_rate(rates, 2_500_000.0);
        }
    }
    Ok(Json(info))
}

/// A rate this device can do, at or below `want`: `want` itself if a
/// *continuous* range spans it, else the highest discrete point ≤ `want`,
/// else the lowest available.
fn pick_rate(rates: &[Range], want: f64) -> f64 {
    if rates.iter().any(|rg| rg.max > rg.min && want >= rg.min && want <= rg.max) {
        return want;
    }
    let pts: Vec<f64> = rates
        .iter()
        .flat_map(|rg| if rg.max > rg.min { vec![rg.min, rg.max] } else { vec![rg.min] })
        .filter(|&v| v > 0.0)
        .collect();
    let under: Vec<f64> = pts.iter().copied().filter(|&v| v <= want).collect();
    if !under.is_empty() {
        under.into_iter().fold(f64::MIN, f64::max)
    } else {
        pts.into_iter().fold(f64::MAX, f64::min)
    }
}

pub async fn health(State(st): State<AppState>) -> Json<DeviceHealth> {
    Json(st.registry.health())
}
