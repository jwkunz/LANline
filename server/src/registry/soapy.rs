//! SoapySDR-backed device enumeration and capability probing.
//!
//! Phase 1a keeps this synchronous and stateless: `probe` opens the device,
//! reads its capabilities, and drops it. Holding an open `Device` for the RX
//! stream (on a dedicated thread) comes with the pipeline in phase 1c.

use crate::model::*;
use soapysdr::{Args, Device, Direction};

type SoapyResult<T> = std::result::Result<T, soapysdr::Error>;

fn conv_range(r: &soapysdr::Range) -> Range {
    Range { min: r.minimum, max: r.maximum, step: r.step }
}

fn args_to_string(a: &Args) -> String {
    a.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join(",")
}

fn stable_id(driver: &str, serial: &str, idx: usize) -> String {
    if serial.is_empty() {
        format!("{driver}/{idx}")
    } else {
        format!("{driver}/{serial}")
    }
}

/// Coarse guess used only for the enumeration summary; `probe` reports the
/// authoritative value from `num_channels(Tx)`.
fn driver_is_tx_capable(driver: &str) -> bool {
    matches!(
        driver,
        "hackrf" | "lime" | "bladerf" | "plutosdr" | "soapyplutosdr" | "uhd" | "sidekiq"
    )
}

pub fn enumerate() -> SoapyResult<Vec<DeviceSummary>> {
    let found = soapysdr::enumerate("")?;
    Ok(found
        .iter()
        .enumerate()
        .map(|(idx, args)| {
            let driver = args.get("driver").unwrap_or("unknown").to_string();
            let serial = args.get("serial").unwrap_or("").to_string();
            let label = args
                .get("label")
                .map(str::to_string)
                .unwrap_or_else(|| driver.clone());
            DeviceSummary {
                id: stable_id(&driver, &serial, idx),
                tx_capable: driver_is_tx_capable(&driver),
                driver,
                label,
                serial,
                soapy_args: args_to_string(args),
                available: true,
            }
        })
        .collect())
}

/// Open the device matching `args`, read its RX capabilities, and drop it.
pub fn probe(args: &str) -> SoapyResult<DeviceInfo> {
    let dev = Device::new(args)?;
    let dir = Direction::Rx;
    let ch = 0usize;

    // Lower-case so the id matches the `driver=<key>` factory name used in
    // `enumerate()` (SoapyHackRF's driver_key is "HackRF", its factory key is
    // "hackrf").
    let driver = dev.driver_key().unwrap_or_else(|_| "unknown".to_string()).to_lowercase();
    let parsed = Args::from(args);
    let serial = dev
        .hardware_info()
        .ok()
        .and_then(|hw| hw.get("serial").map(str::to_string))
        .or_else(|| parsed.get("serial").map(str::to_string))
        .unwrap_or_default();
    let label = parsed
        .get("label")
        .map(str::to_string)
        .unwrap_or_else(|| dev.hardware_key().unwrap_or_else(|_| driver.clone()));

    let channels = dev.num_channels(dir).unwrap_or(1).max(1);
    let tx_capable = dev.num_channels(Direction::Tx).unwrap_or(0) > 0;

    let vec_ranges = |r: SoapyResult<Vec<soapysdr::Range>>| -> Vec<Range> {
        r.map(|v| v.iter().map(conv_range).collect()).unwrap_or_default()
    };

    let gain_names = dev.list_gains(dir, ch).unwrap_or_default();
    let gain_elements = gain_names
        .iter()
        .filter_map(|name| {
            dev.gain_element_range(dir, ch, name.as_str())
                .ok()
                .map(|r| GainElement { name: name.clone(), range_db: conv_range(&r) })
        })
        .collect();

    let has_frequency_correction = dev
        .list_frequencies(dir, ch)
        .map(|names| names.iter().any(|n| n.eq_ignore_ascii_case("CORR")))
        .unwrap_or(false);

    Ok(DeviceInfo {
        id: stable_id(&driver, &serial, 0),
        driver,
        label,
        serial,
        soapy_args: args.to_string(),
        status: DeviceStatus::Ready,
        tx_capable,
        rx: RxCapabilities {
            channels,
            antennas: dev.antennas(dir, ch).unwrap_or_default(),
            frequency_ranges_hz: vec_ranges(dev.frequency_range(dir, ch)),
            sample_rate_ranges_hz: vec_ranges(dev.get_sample_rate_range(dir, ch)),
            bandwidth_ranges_hz: vec_ranges(dev.bandwidth_range(dir, ch)),
            gain_elements,
            overall_gain_range_db: dev
                .gain_range(dir, ch)
                .map(|r| conv_range(&r))
                .unwrap_or(Range { min: 0.0, max: 0.0, step: 0.0 }),
            has_agc: dev.has_gain_mode(dir, ch).unwrap_or(false),
            has_dc_offset_mode: dev.has_dc_offset_mode(dir, ch).unwrap_or(false),
            has_iq_balance_mode: dev.has_iq_balance(dir, ch).unwrap_or(false),
            has_frequency_correction,
            sensors: dev.list_sensors().unwrap_or_default(),
            // TODO(phase 1c): populate from SoapySDRDevice_getSettingInfo (not
            // exposed by the soapysdr crate; needs a raw soapysdr-sys call).
            setting_info: Vec::new(),
        },
    })
}
