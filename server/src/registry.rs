//! SDR device discovery, capability probing, and selection.
//!
//! With the `soapy` feature (default) this talks to libSoapySDR. Without it,
//! enumeration returns nothing and only `debug_tone` is usable.

use crate::error::ApiError;
use crate::model::*;
use std::sync::Mutex;

#[cfg(feature = "soapy")]
mod soapy;

#[derive(Default)]
pub struct DeviceRegistry {
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    selected: Option<DeviceInfo>,
    last_error: Option<String>,
}

impl DeviceRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Fresh enumeration of every SoapySDR device currently attached.
    pub fn enumerate(&self) -> Vec<DeviceSummary> {
        #[cfg(feature = "soapy")]
        {
            match soapy::enumerate() {
                Ok(list) => list,
                Err(e) => {
                    tracing::warn!("device enumeration failed: {e}");
                    self.inner.lock().unwrap().last_error = Some(e.to_string());
                    Vec::new()
                }
            }
        }
        #[cfg(not(feature = "soapy"))]
        {
            Vec::new()
        }
    }

    /// The currently selected device (with full capabilities), if any.
    pub fn selected(&self) -> Option<DeviceInfo> {
        self.inner.lock().unwrap().selected.clone()
    }

    pub fn selected_summary(&self) -> Option<SelectedDeviceSummary> {
        self.selected().map(|d| SelectedDeviceSummary {
            id: d.id,
            driver: d.driver,
            label: d.label,
            tx_capable: d.tx_capable,
        })
    }

    pub fn health(&self) -> DeviceHealth {
        let inner = self.inner.lock().unwrap();
        match &inner.selected {
            Some(d) => DeviceHealth {
                present: true,
                status: d.status,
                last_error: inner.last_error.clone(),
                samples_read: 0,
                overruns: 0,
                sensors: Default::default(),
            },
            None => {
                let mut h = DeviceHealth::absent();
                h.last_error = inner.last_error.clone();
                h
            }
        }
    }

    /// Select a device explicitly (`PUT /api/v1/device`).
    pub fn select(&self, sel: &DeviceSelector) -> Result<DeviceInfo, ApiError> {
        let args = match (sel.id.as_deref(), sel.soapy_args.as_deref()) {
            (Some(id), None) => self
                .enumerate()
                .into_iter()
                .find(|d| d.id == id)
                .map(|d| d.soapy_args)
                .ok_or_else(|| ApiError::not_found(format!("no device with id {id}")))?,
            (None, Some(args)) => args.to_string(),
            _ => {
                return Err(ApiError::bad_request(
                    "provide exactly one of `id` or `soapy_args`",
                ))
            }
        };
        self.select_by_args(&args)
    }

    /// Startup selection: honour a `driver=...` filter, else pick the most
    /// plausible SDR (SoapySDR also enumerates sound cards via the `audio`
    /// module, which we never auto-select). A failure here is not fatal — the
    /// server still serves `debug_tone`.
    pub fn select_auto(&self, filter: Option<&str>) {
        let devices = self.enumerate();
        for d in &devices {
            tracing::info!(
                "enumerated device: id={} driver={} label={:?} available={}",
                d.id, d.driver, d.label, d.available
            );
        }

        let args = match filter {
            Some(f) => f.to_string(),
            None => match pick_default(&devices) {
                Some(d) => {
                    tracing::info!("auto-selected `{}` (override with --device)", d.id);
                    d.soapy_args.clone()
                }
                None => {
                    tracing::warn!(
                        "no SDR auto-selected ({} device(s) enumerated); pass --device or use debug-tone",
                        devices.len()
                    );
                    return;
                }
            },
        };

        match self.select_by_args(&args) {
            Ok(d) => tracing::info!("selected device: {} ({})", d.label, d.id),
            Err(e) => tracing::warn!("startup device selection failed: {}", e.message),
        }
    }

    fn select_by_args(&self, args: &str) -> Result<DeviceInfo, ApiError> {
        #[cfg(feature = "soapy")]
        {
            let info = soapy::probe(args).map_err(|e| {
                self.inner.lock().unwrap().last_error = Some(e.to_string());
                ApiError::device_unavailable(format!("cannot open device `{args}`: {e}"))
            })?;
            let mut inner = self.inner.lock().unwrap();
            inner.last_error = None;
            inner.selected = Some(info.clone());
            Ok(info)
        }
        #[cfg(not(feature = "soapy"))]
        {
            let _ = args;
            Err(ApiError::device_unavailable(
                "server built without the `soapy` feature",
            ))
        }
    }
}

/// Validate a proposed radio config against a device's reported capabilities.
/// Returns the first violation as a `422` with the offending field in
/// `details`.
pub fn validate_against_device(
    cfg: &crate::model::RadioConfig,
    dev: &DeviceInfo,
) -> Result<(), ApiError> {
    use serde_json::json;
    let rx = &dev.rx;

    let bad = |field: &str, msg: String, allowed: serde_json::Value| {
        ApiError::invalid_parameter(msg).with_details(json!({ "field": field, "allowed": allowed }))
    };

    if cfg.tuner.channel >= rx.channels {
        return Err(bad(
            "tuner.channel",
            format!("channel {} out of range", cfg.tuner.channel),
            json!({ "channels": rx.channels }),
        ));
    }

    if !in_ranges(cfg.frequency_hz as f64, &rx.frequency_ranges_hz) {
        return Err(bad(
            "frequency_hz",
            format!("{} Hz outside the tunable range", cfg.frequency_hz),
            json!(rx.frequency_ranges_hz),
        ));
    }

    if !in_ranges(cfg.tuner.sample_rate_hz, &rx.sample_rate_ranges_hz) {
        return Err(bad(
            "tuner.sample_rate_hz",
            format!("{} Hz is not a supported sample rate", cfg.tuner.sample_rate_hz),
            json!(rx.sample_rate_ranges_hz),
        ));
    }

    if let Some(ant) = &cfg.tuner.antenna {
        if !rx.antennas.iter().any(|a| a == ant) {
            return Err(bad(
                "tuner.antenna",
                format!("unknown antenna `{ant}`"),
                json!(rx.antennas),
            ));
        }
    }

    if matches!(cfg.tuner.gain_mode, crate::model::GainMode::Agc) && !rx.has_agc {
        return Err(bad(
            "tuner.gain_mode",
            "device has no AGC".into(),
            json!(["manual"]),
        ));
    }

    if let Some(g) = cfg.tuner.gain_db {
        let r = rx.overall_gain_range_db;
        if g < r.min - 1e-6 || g > r.max + 1e-6 {
            return Err(bad("tuner.gain_db", format!("{g} dB out of range"), json!(r)));
        }
    }

    for (name, &val) in &cfg.tuner.gain_elements_db {
        match rx.gain_elements.iter().find(|e| &e.name == name) {
            None => {
                return Err(bad(
                    "tuner.gain_elements_db",
                    format!("unknown gain element `{name}`"),
                    json!(rx.gain_elements.iter().map(|e| &e.name).collect::<Vec<_>>()),
                ))
            }
            Some(e) => {
                if val < e.range_db.min - 1e-6 || val > e.range_db.max + 1e-6 {
                    return Err(bad(
                        "tuner.gain_elements_db",
                        format!("{name} = {val} dB out of range"),
                        json!(e.range_db),
                    ));
                }
            }
        }
    }

    Ok(())
}

fn in_ranges(v: f64, ranges: &[crate::model::Range]) -> bool {
    if ranges.is_empty() {
        return true; // device reported nothing; don't second-guess it
    }
    let tol = (v.abs() * 1e-6).max(1.0);
    ranges.iter().any(|r| v >= r.min - tol && v <= r.max + tol)
}

/// Choose a default device for startup: a known SDR driver first (in
/// preference order), then any non-`audio` device. Sound cards (SoapySDR's
/// `audio` module) are never auto-selected.
fn pick_default(devices: &[DeviceSummary]) -> Option<&DeviceSummary> {
    const PREFERRED: &[&str] = &[
        "hackrf", "plutosdr", "soapyplutosdr", "rtlsdr", "airspy", "airspyhf",
        "bladerf", "lime", "sdrplay", "uhd",
    ];
    for driver in PREFERRED {
        if let Some(d) = devices.iter().find(|d| d.driver == *driver) {
            return Some(d);
        }
    }
    devices.iter().find(|d| d.driver != "audio")
}
