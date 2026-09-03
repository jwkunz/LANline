//! SDR device discovery, capability probing, and selection.
//!
//! With the `soapy` feature (default) this talks to libSoapySDR. Without it,
//! enumeration returns nothing and only `debug_tone` is usable.

use crate::error::ApiError;
use crate::model::*;
use std::sync::Mutex;

#[cfg(feature = "soapy")]
mod soapy;

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
        Self { inner: Mutex::new(Inner::default()) }
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
