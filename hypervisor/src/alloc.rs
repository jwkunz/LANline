//! Pre-flight the assigned ports so a collision fails before anything spawns.

use crate::config::RadioSpec;
use anyhow::{bail, Result};
use std::collections::HashMap;
use std::net::IpAddr;

/// Check every TCP port each child will bind (`c2`, `beast`, `ais_nmea`,
/// `aprs`) is free and unique across the fleet. `audio_out` / `audio_in` are
/// UDP and left to the child (they still get a unique number, just not a
/// bind check here).
pub fn preflight(bind: IpAddr, specs: &[RadioSpec]) -> Result<()> {
    let mut seen: HashMap<u16, String> = HashMap::new();

    for s in specs {
        for (label, port) in [
            ("c2", s.ports.c2),
            ("beast", s.ports.beast),
            ("ais_nmea", s.ports.ais_nmea),
            ("aprs", s.ports.aprs),
        ] {
            if port == 0 {
                continue; // disabled feed
            }
            if let Some(other) = seen.insert(port, format!("radio {} {label}", s.idx)) {
                bail!(
                    "port {port} assigned twice ({other} and radio {} {label}) — widen the [ports] bases",
                    s.idx
                );
            }
            match lanline_server::net::bind_tcp(bind, port) {
                Ok((listener, _)) => drop(listener),
                Err(e) => bail!("radio {} {label} port {port} is not available: {e}", s.idx),
            }
        }
    }
    Ok(())
}
