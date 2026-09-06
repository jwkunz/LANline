//! Resolve each radio's device filter to a distinct physical SDR.

use crate::config::RadioSpec;
use anyhow::{bail, Result};
use std::collections::HashMap;

/// With the `soapy` feature: enumerate SoapySDR, bind each filter to exactly
/// one device, rewrite it to a `serial=`-qualified args string, and refuse two
/// radios that land on the same unit. Without the feature: just require the
/// filter strings to be distinct and pass them through untouched.
pub fn resolve(specs: &mut [RadioSpec]) -> Result<()> {
    #[cfg(feature = "soapy")]
    {
        resolve_soapy(specs)
    }
    #[cfg(not(feature = "soapy"))]
    {
        resolve_by_string(specs)
    }
}

#[cfg(not(feature = "soapy"))]
fn resolve_by_string(specs: &mut [RadioSpec]) -> Result<()> {
    let mut seen: HashMap<&str, usize> = HashMap::new();
    for s in specs.iter() {
        if let Some(other) = seen.insert(s.device_filter.as_str(), s.idx) {
            bail!(
                "radios {other} and {} both request `{}` — with no `soapy` feature the hypervisor \
                 cannot tell them apart; give each a distinct filter",
                s.idx,
                s.device_filter
            );
        }
    }
    tracing::warn!(
        "built without the `soapy` feature — device filters passed through unresolved; \
         make sure each names a different physical SDR"
    );
    Ok(())
}

#[cfg(feature = "soapy")]
fn resolve_soapy(specs: &mut [RadioSpec]) -> Result<()> {
    let all = soapysdr::enumerate("").unwrap_or_default();
    tracing::info!("SoapySDR enumerated {} device(s)", all.len());

    // key (serial, or the canonical args when there is no serial) -> radio idx
    let mut claimed: HashMap<String, usize> = HashMap::new();

    for s in specs.iter_mut() {
        let matches = soapysdr::enumerate(s.device_filter.as_str())
            .map_err(|e| anyhow::anyhow!("enumerating `{}`: {e}", s.device_filter))?;

        let dev = match matches.len() {
            1 => &matches[0],
            0 => bail!(
                "radio {} (`{}`): no SoapySDR device matches{}",
                s.idx,
                s.device_filter,
                device_hint(&all)
            ),
            n => bail!(
                "radio {} (`{}`): {n} devices match — narrow the filter (add serial= or label=)",
                s.idx,
                s.device_filter
            ),
        };

        let driver = dev.get("driver").map(str::to_string);
        let serial = dev.get("serial").filter(|s| !s.is_empty()).map(str::to_string);
        let label = dev.get("label").map(str::to_string);

        // What the child opens. A serial is the least ambiguous; otherwise fall
        // back to SoapySDR's own canonical rendering of the enumeration entry.
        let args = match (&driver, &serial) {
            (Some(d), Some(sn)) => format!("driver={d},serial={sn}"),
            _ => String::from(dev),
        };
        let key = serial.clone().unwrap_or_else(|| args.clone());

        if let Some(other) = claimed.insert(key, s.idx) {
            bail!(
                "radios {other} and {} resolve to the same SDR ({}) — they must be different units",
                s.idx,
                label.as_deref().unwrap_or(&args)
            );
        }

        s.device_label = label.unwrap_or_else(|| args.clone());
        s.device_driver = driver;
        s.device_serial = serial;
        s.device_args = args;
        tracing::info!("radio {} -> {} ({})", s.idx, s.device_label, s.device_args);
    }

    Ok(())
}

/// How many SDRs are attached (for the beacon's `devices_available`). Falls
/// back to the fleet size when built without `soapy`.
pub fn available_count(fleet_size: usize) -> usize {
    #[cfg(feature = "soapy")]
    {
        let _ = fleet_size;
        soapysdr::enumerate("").map(|v| v.len()).unwrap_or(fleet_size)
    }
    #[cfg(not(feature = "soapy"))]
    {
        fleet_size
    }
}

#[cfg(feature = "soapy")]
fn device_hint(all: &[soapysdr::Args]) -> String {
    if all.is_empty() {
        return " (none attached)".into();
    }
    let names: Vec<String> = all
        .iter()
        .map(|a| a.get("label").or_else(|| a.get("driver")).unwrap_or("?").to_string())
        .collect();
    format!(" — attached: {}", names.join(", "))
}

#[cfg(all(test, not(feature = "soapy")))]
mod tests {
    use super::*;
    use crate::config::{ChildPorts, RadioSpec};

    fn spec(idx: usize, filter: &str) -> RadioSpec {
        RadioSpec {
            idx,
            label: format!("radio-{idx}"),
            device_filter: filter.to_string(),
            device_args: filter.to_string(),
            device_label: filter.to_string(),
            device_serial: None,
            device_driver: None,
            server_id: uuid::Uuid::new_v4(),
            ports: ChildPorts {
                c2: 8730 + idx as u16,
                audio_out: 0,
                audio_in: 0,
                beast: 0,
                ais_nmea: 0,
                aprs: 0,
            },
            freq_correction_ppm: 0.0,
            tls: false,
            enable_tx: false,
        }
    }

    #[test]
    fn distinct_filters_pass_identical_ones_fail() {
        let mut ok = [spec(0, "driver=hackrf"), spec(1, "driver=plutosdr")];
        assert!(resolve(&mut ok).is_ok());

        let mut clash = [spec(0, "driver=hackrf"), spec(1, "driver=hackrf")];
        assert!(resolve(&mut clash).is_err());
    }
}
