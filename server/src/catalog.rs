//! Static catalogs: demodulation modes and built-in presets.

use crate::model::{ModeInfo, ModeParamSpec, Preset};
use serde_json::{json, Value};
use std::collections::BTreeMap;

fn num(default: f64, min: f64, max: f64, unit: &'static str) -> ModeParamSpec {
    ModeParamSpec { type_: "number", default, min: Some(min), max: Some(max), enum_: None, unit }
}

fn num_enum(default: f64, choices: &[f64], unit: &'static str) -> ModeParamSpec {
    ModeParamSpec {
        type_: "number",
        default,
        min: None,
        max: None,
        enum_: Some(choices.to_vec()),
        unit,
    }
}

/// The full mode catalog served by `GET /api/v1/modes`.
pub fn modes() -> Vec<ModeInfo> {
    vec![
        ModeInfo {
            id: "nbfm",
            name: "Narrowband FM",
            tx_capable: false,
            params: BTreeMap::from([
                ("deviation_hz", num(5000.0, 1000.0, 15000.0, "Hz")),
                ("channel_bw_hz", num(16000.0, 8000.0, 25000.0, "Hz")),
                ("deemphasis_us", num_enum(75.0, &[0.0, 50.0, 75.0], "us")),
                ("audio_lpf_hz", num(3400.0, 1000.0, 8000.0, "Hz")),
                ("squelch_db", num(-80.0, -120.0, 0.0, "dBFS")),
                ("noise_squelch", num(0.18, 0.02, 2.0, "ratio")),
            ]),
        },
        ModeInfo {
            id: "wbfm",
            name: "Wideband FM (broadcast)",
            tx_capable: false,
            params: BTreeMap::from([
                ("deviation_hz", num(75000.0, 50000.0, 100000.0, "Hz")),
                ("channel_bw_hz", num(200000.0, 120000.0, 256000.0, "Hz")),
                ("deemphasis_us", num_enum(75.0, &[0.0, 50.0, 75.0], "us")),
                ("audio_lpf_hz", num(15000.0, 5000.0, 17000.0, "Hz")),
                ("squelch_db", num(-120.0, -120.0, 0.0, "dBFS")),
                ("noise_squelch", num(2.0, 0.02, 2.0, "ratio")),
            ]),
        },
        ModeInfo {
            id: "adsb",
            name: "ADS-B (1090 MHz aircraft)",
            tx_capable: false,
            params: BTreeMap::from([
                // 0,0 means "no reference" -> global CPR only, no range gate.
                ("reference_lat", num(0.0, -90.0, 90.0, "deg")),
                ("reference_lon", num(0.0, -180.0, 180.0, "deg")),
                ("max_range_nm", num(250.0, 10.0, 500.0, "NM")),
                ("trail_seconds", num(120.0, 10.0, 600.0, "s")),
                ("forget_seconds", num(60.0, 10.0, 600.0, "s")),
                ("fix_errors", num_enum(1.0, &[0.0, 1.0], "bool")),
            ]),
        },
        ModeInfo {
            id: "debug_tone",
            name: "Debug Tone (A4 440 Hz)",
            tx_capable: false,
            params: BTreeMap::from([
                ("tone_hz", num(440.0, 20.0, 20000.0, "Hz")),
                ("level_dbfs", num(-12.0, -60.0, -1.0, "dBFS")),
            ]),
        },
    ]
}

/// Default `mode_params` object for a mode id (empty object if unknown).
pub fn default_mode_params(mode: &str) -> Value {
    match modes().into_iter().find(|m| m.id == mode) {
        Some(m) => {
            let mut obj = serde_json::Map::new();
            for (k, spec) in m.params {
                obj.insert(k.to_string(), json!(spec.default));
            }
            Value::Object(obj)
        }
        None => json!({}),
    }
}

/// Built-in, read-only presets served by `GET /api/v1/presets`.
pub fn presets() -> Vec<Preset> {
    vec![Preset {
        id: "noaa-khb29",
        name: "NOAA Weather Radio \u{2014} KHB29 (162.550 MHz)",
        builtin: true,
        config: json!({
            "mode": "nbfm",
            "frequency_hz": 162_550_000,
            "tuner": {
                "sample_rate_hz": 2_000_000,
                "lo_offset_hz": 250_000,
                "gain_elements_db": { "AMP": 0, "LNA": 32, "VGA": 30 }
            },
            "mode_params": {
                "deviation_hz": 5000,
                "channel_bw_hz": 16000,
                "deemphasis_us": 75,
                "audio_lpf_hz": 3400,
                "squelch_db": -80
            }
        }),
    }]
}

/// Capability strings for `GET /api/v1/server` and the beacon. `tx` is added
/// by the caller when the selected device supports it.
pub fn base_capabilities() -> Vec<String> {
    ["rx", "webrtc", "nbfm", "wbfm", "adsb", "debug_tone"].iter().map(|s| s.to_string()).collect()
}
