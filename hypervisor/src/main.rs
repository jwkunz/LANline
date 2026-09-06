//! `lanline-hypervisor` — run one `lanline-server` per radio and aggregate
//! their LAN discovery. See docs/hypervisor.md.

mod alloc;
mod config;
mod devices;
mod discovery;
mod supervisor;

use anyhow::{Context, Result};
use clap::Parser;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::watch;
use uuid::Uuid;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,lanline_hypervisor=debug".into()),
        )
        .init();

    let cli = config::Cli::parse();

    let mut specs = config::load(&cli)?;
    devices::resolve(&mut specs)?;

    let advertise_host = config::advertised_host(&cli);
    alloc::preflight(cli.bind, &specs)?;

    let server_bin = supervisor::resolve_server_bin(cli.server_bin.clone());
    if !server_bin.is_file() && server_bin.components().count() > 1 {
        anyhow::bail!("lanline-server not found at {}", server_bin.display());
    }

    let state_dir = cli.state_dir.clone().unwrap_or_else(default_state_dir);
    std::fs::create_dir_all(&state_dir)
        .with_context(|| format!("creating state dir {}", state_dir.display()))?;

    tracing::info!(
        "fleet: {} radio(s), server binary {}, state {}",
        specs.len(),
        server_bin.display(),
        state_dir.display()
    );
    for s in &specs {
        tracing::info!(
            "  radio {} \"{}\": {} -> c2 {}{}{}",
            s.idx,
            s.label,
            s.device_args,
            s.ports.c2,
            if s.tls { " tls" } else { "" },
            if s.enable_tx { " tx" } else { "" },
        );
    }

    let fleet_id = Uuid::new_v4();
    let devices_available = devices::available_count(specs.len());

    let (shutdown_tx, shutdown_rx) = watch::channel(false);

    let sup_opts = supervisor::Opts {
        server_bin,
        state_dir,
        bind: cli.bind,
        advertise_host,
        no_restart: cli.no_restart,
        max_restarts: cli.max_restarts,
    };
    let (children, sup_tasks) = supervisor::spawn_fleet(specs, sup_opts, shutdown_rx.clone());
    let children = Arc::new(children);

    let disc_opts = discovery::Opts {
        bind: cli.bind,
        advertise_host,
        beacon_port: cli.beacon_port,
        fleet_port: cli.fleet_port,
        no_beacon: cli.no_beacon,
        no_mdns: cli.no_mdns,
        devices_available,
    };
    let disc_task = tokio::spawn(discovery::run(
        fleet_id,
        children.clone(),
        disc_opts,
        shutdown_rx.clone(),
    ));

    wait_for_signal().await;
    tracing::info!("shutdown: signalling {} child server(s)", children.len());
    let _ = shutdown_tx.send(true);

    // Give the supervisors time to stop their children, then move on.
    let _ = tokio::time::timeout(Duration::from_secs(10), async {
        for t in sup_tasks {
            let _ = t.await;
        }
    })
    .await;
    disc_task.abort();
    tracing::info!("shutdown: done");
    Ok(())
}

async fn wait_for_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        let mut term = signal(SignalKind::terminate()).expect("SIGTERM handler");
        let mut int = signal(SignalKind::interrupt()).expect("SIGINT handler");
        tokio::select! {
            _ = term.recv() => tracing::info!("SIGTERM"),
            _ = int.recv() => tracing::info!("SIGINT"),
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

fn default_state_dir() -> PathBuf {
    if let Ok(x) = std::env::var("XDG_STATE_HOME") {
        if !x.is_empty() {
            return PathBuf::from(x).join("lanline-hypervisor");
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        if !home.is_empty() {
            return PathBuf::from(home).join(".local/state/lanline-hypervisor");
        }
    }
    std::env::temp_dir().join("lanline-hypervisor")
}
