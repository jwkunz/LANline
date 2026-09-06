//! CLI + `fleet.toml` parsing, merged into one resolved `RadioSpec` per radio.

use anyhow::{bail, Context, Result};
use clap::Parser;
use serde::Deserialize;
use std::net::{IpAddr, Ipv4Addr};
use std::path::PathBuf;

/// `lanline-hypervisor` — run one `lanline-server` per radio and aggregate
/// their LAN discovery. See docs/hypervisor.md.
#[derive(Debug, Parser)]
#[command(name = "lanline-hypervisor", version, about = "LANline — run a fleet of radios")]
pub struct Cli {
    /// Fleet definition (`[[radio]]` blocks, `[defaults]`, `[ports]`).
    #[arg(long, value_name = "FILE")]
    pub config: Option<PathBuf>,

    /// Shorthand for one radio: the value is its `--device` filter, everything
    /// else defaulted. Repeatable. Appended after any `--config` radios.
    #[arg(long = "radio", value_name = "DEVICE_FILTER")]
    pub radio: Vec<String>,

    /// Path to the `lanline-server` binary. Default: a sibling of this
    /// executable.
    #[arg(long, value_name = "PATH")]
    pub server_bin: Option<PathBuf>,

    /// Parent directory for each child's private state (`radio-<i>/` IQ dir,
    /// `tls-radio-<i>/` cert cache). Default: `<data-dir>/lanline-hypervisor`.
    #[arg(long, value_name = "DIR")]
    pub state_dir: Option<PathBuf>,

    /// Address the children bind and the fleet endpoints listen on.
    #[arg(long, env = "LANLINE_BIND", default_value = "0.0.0.0")]
    pub bind: IpAddr,

    /// Host/IP advertised to clients (child C2 URLs, fleet base URL). Default:
    /// the autodetected primary LAN IPv4.
    #[arg(long, env = "LANLINE_ADVERTISE_HOST")]
    pub advertise_host: Option<IpAddr>,

    /// TCP port for the fleet HTTP endpoint (`GET /api/v1/fleet`, `GET /`).
    #[arg(long, default_value_t = 8720)]
    pub fleet_port: u16,

    /// UDP port for the aggregated discovery beacons.
    #[arg(long, default_value_t = 50055)]
    pub beacon_port: u16,

    /// Serve every child over HTTPS (`--tls`). Per-radio `tls` in the config
    /// overrides this.
    #[arg(long, default_value_t = false)]
    pub tls: bool,

    /// Allow push-to-talk transmit on every child (`--enable-tx`). Per-radio
    /// `enable_tx` in the config overrides this.
    #[arg(long, default_value_t = false)]
    pub enable_tx: bool,

    /// Do not restart a child that exits.
    #[arg(long, default_value_t = false)]
    pub no_restart: bool,

    /// Give up on a child after this many restarts (0 = never give up).
    #[arg(long, default_value_t = 0)]
    pub max_restarts: u32,

    /// Do not emit discovery beacons.
    #[arg(long, default_value_t = false)]
    pub no_beacon: bool,

    /// Do not advertise the children over mDNS.
    #[arg(long, default_value_t = false)]
    pub no_mdns: bool,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FleetFile {
    #[serde(default)]
    pub defaults: Defaults,
    #[serde(default)]
    pub ports: PortBases,
    #[serde(default, rename = "radio")]
    pub radios: Vec<RadioEntry>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Defaults {
    pub tls: Option<bool>,
    pub enable_tx: Option<bool>,
    pub freq_correction_ppm: Option<f64>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PortBases {
    pub c2: u16,
    pub audio_out: u16,
    pub audio_in: u16,
    /// `0` disables the Beast feed on every child.
    pub beast: u16,
    /// `0` disables the AIVDM feed on every child.
    pub ais_nmea: u16,
    /// `0` disables the TNC2 APRS feed on every child.
    pub aprs: u16,
}

impl Default for PortBases {
    fn default() -> Self {
        // Bases at least 10 apart so up to ~8 radios never overlap.
        Self { c2: 8730, audio_out: 8740, audio_in: 8750, beast: 30005, ais_nmea: 10110, aprs: 10152 }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RadioEntry {
    pub label: Option<String>,
    pub device: String,
    pub freq_correction_ppm: Option<f64>,
    pub tls: Option<bool>,
    pub enable_tx: Option<bool>,
}

/// One radio's TCP/UDP ports (`base + index`).
#[derive(Debug, Clone, Copy)]
pub struct ChildPorts {
    pub c2: u16,
    pub audio_out: u16,
    pub audio_in: u16,
    pub beast: u16,
    pub ais_nmea: u16,
    pub aprs: u16,
}

/// A fully-resolved radio: config merged, ports assigned, device resolved,
/// identity pinned.
#[derive(Debug, Clone)]
pub struct RadioSpec {
    pub idx: usize,
    pub label: String,
    /// The filter as written in the config (`driver=hackrf`).
    pub device_filter: String,
    /// What the child is actually told to open — `device_filter` verbatim, or a
    /// `serial=`-qualified string once resolved against the SoapySDR
    /// enumeration.
    pub device_args: String,
    /// Human device name for the fleet view / beacon.
    pub device_label: String,
    pub device_serial: Option<String>,
    pub device_driver: Option<String>,
    pub server_id: uuid::Uuid,
    pub ports: ChildPorts,
    pub freq_correction_ppm: f64,
    pub tls: bool,
    pub enable_tx: bool,
}

/// Parse the CLI, load any `--config`, and produce the merged specs (device
/// resolution happens separately in `devices`).
pub fn load(cli: &Cli) -> Result<Vec<RadioSpec>> {
    let file: FleetFile = match &cli.config {
        Some(path) => {
            let text = std::fs::read_to_string(path)
                .with_context(|| format!("reading {}", path.display()))?;
            toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?
        }
        None => FleetFile::default(),
    };

    let mut entries: Vec<RadioEntry> = file.radios;
    for filter in &cli.radio {
        entries.push(RadioEntry {
            label: None,
            device: filter.clone(),
            freq_correction_ppm: None,
            tls: None,
            enable_tx: None,
        });
    }

    if entries.is_empty() {
        bail!("no radios: pass --config <fleet.toml> with [[radio]] blocks, or one or more --radio <filter>");
    }
    if entries.len() > 8 {
        bail!("{} radios requested; the port layout allows at most 8", entries.len());
    }

    let bases = file.ports;
    let specs = entries
        .into_iter()
        .enumerate()
        .map(|(idx, e)| {
            let i = idx as u16;
            let step = |base: u16| if base == 0 { 0 } else { base + i };
            RadioSpec {
                idx,
                label: e.label.clone().unwrap_or_else(|| format!("radio-{idx}")),
                device_filter: e.device.clone(),
                device_args: e.device.clone(),
                device_label: e.device.clone(),
                device_serial: None,
                device_driver: None,
                server_id: uuid::Uuid::new_v4(),
                ports: ChildPorts {
                    c2: bases.c2 + i,
                    audio_out: bases.audio_out + i,
                    audio_in: bases.audio_in + i,
                    beast: step(bases.beast),
                    ais_nmea: step(bases.ais_nmea),
                    aprs: step(bases.aprs),
                },
                freq_correction_ppm: e
                    .freq_correction_ppm
                    .or(file.defaults.freq_correction_ppm)
                    .unwrap_or(0.0),
                tls: e.tls.or(file.defaults.tls).unwrap_or(cli.tls),
                enable_tx: e.enable_tx.or(file.defaults.enable_tx).unwrap_or(cli.enable_tx),
            }
        })
        .collect();

    Ok(specs)
}

/// Best-effort primary LAN IPv4, mirroring the server's own default.
pub fn advertised_host(cli: &Cli) -> IpAddr {
    cli.advertise_host.unwrap_or_else(|| {
        lanline_server::net::primary_ipv4()
            .map(IpAddr::V4)
            .unwrap_or(IpAddr::V4(Ipv4Addr::LOCALHOST))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cli(args: &[&str]) -> Cli {
        let mut v = vec!["lanline-hypervisor"];
        v.extend_from_slice(args);
        Cli::parse_from(v)
    }

    fn load_toml(text: &str, extra: &[&str]) -> Vec<RadioSpec> {
        let dir = std::env::temp_dir().join(format!("lh-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("fleet.toml");
        std::fs::write(&path, text).unwrap();
        let mut args = vec!["--config".to_string(), path.display().to_string()];
        args.extend(extra.iter().map(|s| s.to_string()));
        let refs: Vec<&str> = args.iter().map(String::as_str).collect();
        let specs = load(&cli(&refs)).unwrap();
        std::fs::remove_dir_all(&dir).ok();
        specs
    }

    #[test]
    fn merges_defaults_and_assigns_port_blocks() {
        let specs = load_toml(
            r#"
            [defaults]
            tls = true
            freq_correction_ppm = -8.9

            [[radio]]
            label = "VHF"
            device = "driver=hackrf"
            enable_tx = true

            [[radio]]
            device = "driver=plutosdr"
            freq_correction_ppm = 0.0
            "#,
            &[],
        );
        assert_eq!(specs.len(), 2);

        assert_eq!(specs[0].label, "VHF");
        assert_eq!(specs[0].ports.c2, 8730);
        assert_eq!(specs[0].ports.beast, 30005);
        assert!(specs[0].tls);
        assert!(specs[0].enable_tx);
        assert_eq!(specs[0].freq_correction_ppm, -8.9);

        assert_eq!(specs[1].label, "radio-1"); // no label given
        assert_eq!(specs[1].ports.c2, 8731);
        assert_eq!(specs[1].ports.aprs, 10153);
        assert!(specs[1].tls); // inherited default
        assert!(!specs[1].enable_tx);
        assert_eq!(specs[1].freq_correction_ppm, 0.0); // per-radio override wins
    }

    #[test]
    fn zero_port_base_disables_that_feed_for_all() {
        let specs = load_toml(
            r#"
            [ports]
            c2 = 9000
            audio_out = 9010
            audio_in = 9020
            beast = 0
            ais_nmea = 10110
            aprs = 0

            [[radio]]
            device = "driver=hackrf"

            [[radio]]
            device = "driver=plutosdr"
            "#,
            &[],
        );
        assert_eq!(specs[0].ports.c2, 9000);
        assert_eq!(specs[1].ports.c2, 9001);
        assert_eq!(specs[0].ports.beast, 0);
        assert_eq!(specs[1].ports.beast, 0);
        assert_eq!(specs[0].ports.aprs, 0);
        assert_eq!(specs[1].ports.ais_nmea, 10111);
    }

    #[test]
    fn radio_shorthand_appends_after_config() {
        let specs = load_toml(
            r#"
            [[radio]]
            label = "A"
            device = "driver=hackrf"
            "#,
            &["--radio", "driver=plutosdr", "--radio", "driver=rtlsdr"],
        );
        assert_eq!(specs.len(), 3);
        assert_eq!(specs[0].label, "A");
        assert_eq!(specs[1].device_filter, "driver=plutosdr");
        assert_eq!(specs[2].device_filter, "driver=rtlsdr");
        assert_eq!(specs[2].ports.c2, 8732);
    }

    #[test]
    fn no_radios_is_an_error() {
        assert!(load(&cli(&[])).is_err());
    }

    #[test]
    fn too_many_radios_is_an_error() {
        let extra: Vec<String> =
            (0..9).flat_map(|i| ["--radio".to_string(), format!("driver=x{i}")]).collect();
        let refs: Vec<&str> = extra.iter().map(String::as_str).collect();
        assert!(load(&cli(&refs)).is_err());
    }
}
