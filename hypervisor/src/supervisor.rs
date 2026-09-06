//! Spawn and babysit one `lanline-server` process per radio.

use crate::config::RadioSpec;
use anyhow::{Context, Result};
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::watch;

/// Knobs shared by every child, from the CLI.
#[derive(Clone)]
pub struct Opts {
    pub server_bin: PathBuf,
    pub state_dir: PathBuf,
    pub bind: std::net::IpAddr,
    pub advertise_host: std::net::IpAddr,
    /// `http://<advertise_host>:<fleet_port>` — passed to each child as
    /// `--fleet-url` so its web UI can link back. `None` when the fleet HTTP
    /// endpoint is disabled.
    pub fleet_url: Option<String>,
    pub no_restart: bool,
    pub max_restarts: u32,
}

/// Live state of one supervised child, shared with the discovery + fleet-HTTP
/// tasks.
pub struct Child {
    pub spec: RadioSpec,
    running: std::sync::atomic::AtomicBool,
    restarts: AtomicU32,
    pid: AtomicU32,
    /// Unix seconds of the last successful spawn (0 = never).
    started_at: AtomicU64,
}

impl Child {
    fn new(spec: RadioSpec) -> Self {
        Self {
            spec,
            running: std::sync::atomic::AtomicBool::new(false),
            restarts: AtomicU32::new(0),
            pid: AtomicU32::new(0),
            started_at: AtomicU64::new(0),
        }
    }

    pub fn running(&self) -> bool {
        self.running.load(Ordering::Relaxed)
    }
    pub fn restarts(&self) -> u32 {
        self.restarts.load(Ordering::Relaxed)
    }
    pub fn pid(&self) -> Option<u32> {
        match self.pid.load(Ordering::Relaxed) {
            0 => None,
            p => Some(p),
        }
    }
    pub fn started_at(&self) -> Option<u64> {
        match self.started_at.load(Ordering::Relaxed) {
            0 => None,
            t => Some(t),
        }
    }
}

/// Build the children and their supervision tasks. Returns the shared handles;
/// the `JoinHandle`s run until `shutdown` flips to `true`.
pub fn spawn_fleet(
    specs: Vec<RadioSpec>,
    opts: Opts,
    shutdown: watch::Receiver<bool>,
) -> (Vec<Arc<Child>>, Vec<tokio::task::JoinHandle<()>>) {
    let mut children = Vec::new();
    let mut tasks = Vec::new();
    for spec in specs {
        let child = Arc::new(Child::new(spec));
        children.push(child.clone());
        let task = tokio::spawn(supervise(child, opts.clone(), shutdown.clone()));
        tasks.push(task);
    }
    (children, tasks)
}

async fn supervise(child: Arc<Child>, opts: Opts, mut shutdown: watch::Receiver<bool>) {
    let idx = child.spec.idx;
    let mut attempt: u32 = 0;

    loop {
        if *shutdown.borrow() {
            return;
        }

        let spawn_started = Instant::now();
        match run_once(&child, &opts, &mut shutdown).await {
            RunOutcome::ShutdownRequested => return,
            RunOutcome::Exited(status) => {
                let up = spawn_started.elapsed();
                child.running.store(false, Ordering::Relaxed);
                child.pid.store(0, Ordering::Relaxed);

                if *shutdown.borrow() {
                    return;
                }
                if opts.no_restart {
                    tracing::warn!("[radio {idx}] exited ({status}); --no-restart, leaving it down");
                    return;
                }

                // Reset the backoff if it was up long enough to count as healthy.
                if up > Duration::from_secs(60) {
                    attempt = 0;
                }
                let restarts = child.restarts.fetch_add(1, Ordering::Relaxed) + 1;
                if opts.max_restarts != 0 && restarts > opts.max_restarts {
                    tracing::error!(
                        "[radio {idx}] exited ({status}); hit --max-restarts {}, giving up",
                        opts.max_restarts
                    );
                    return;
                }

                let delay = Duration::from_secs(1u64 << attempt.min(5)); // 1,2,4,8,16,32
                attempt += 1;
                tracing::warn!(
                    "[radio {idx}] exited ({status}) after {up:.1?}; restart #{restarts} in {delay:?}"
                );
                tokio::select! {
                    _ = tokio::time::sleep(delay) => {}
                    _ = shutdown.changed() => return,
                }
            }
            RunOutcome::SpawnFailed(e) => {
                tracing::error!("[radio {idx}] could not spawn lanline-server: {e:#}");
                let delay = Duration::from_secs(5);
                tokio::select! {
                    _ = tokio::time::sleep(delay) => {}
                    _ = shutdown.changed() => return,
                }
            }
        }
    }
}

enum RunOutcome {
    Exited(std::process::ExitStatus),
    SpawnFailed(anyhow::Error),
    ShutdownRequested,
}

async fn run_once(child: &Arc<Child>, opts: &Opts, shutdown: &mut watch::Receiver<bool>) -> RunOutcome {
    let idx = child.spec.idx;
    let mut cmd = match build_command(&child.spec, opts) {
        Ok(c) => c,
        Err(e) => return RunOutcome::SpawnFailed(e),
    };

    let mut proc = match cmd.spawn() {
        Ok(p) => p,
        Err(e) => return RunOutcome::SpawnFailed(anyhow::Error::new(e)),
    };

    let pid = proc.id().unwrap_or(0);
    child.pid.store(pid, Ordering::Relaxed);
    child.running.store(true, Ordering::Relaxed);
    child.started_at.store(now_unix(), Ordering::Relaxed);
    tracing::info!(
        "[radio {idx}] started `{}` pid {pid} on {}://{}:{}",
        child.spec.label,
        if child.spec.tls { "https" } else { "http" },
        opts.advertise_host,
        child.spec.ports.c2,
    );

    if let Some(out) = proc.stdout.take() {
        pipe_lines(idx, out);
    }
    if let Some(err) = proc.stderr.take() {
        pipe_lines(idx, err);
    }

    tokio::select! {
        status = proc.wait() => match status {
            Ok(s) => RunOutcome::Exited(s),
            Err(e) => RunOutcome::SpawnFailed(anyhow::Error::new(e)),
        },
        _ = shutdown.changed() => {
            if pid != 0 {
                tracing::info!("[radio {idx}] stopping (SIGINT) pid {pid}");
                unsafe { libc::kill(pid as libc::pid_t, libc::SIGINT); }
            }
            // Give it up to 5s to shut down gracefully, then force it.
            match tokio::time::timeout(Duration::from_secs(5), proc.wait()).await {
                Ok(_) => {}
                Err(_) => {
                    tracing::warn!("[radio {idx}] did not stop in 5s; killing");
                    let _ = proc.start_kill();
                    let _ = proc.wait().await;
                }
            }
            child.running.store(false, Ordering::Relaxed);
            child.pid.store(0, Ordering::Relaxed);
            RunOutcome::ShutdownRequested
        }
    }
}

fn build_command(spec: &RadioSpec, opts: &Opts) -> Result<Command> {
    let iq_dir = opts.state_dir.join(format!("radio-{}", spec.idx));
    let tls_dir = opts.state_dir.join(format!("tls-radio-{}", spec.idx));
    std::fs::create_dir_all(&iq_dir).with_context(|| format!("creating {}", iq_dir.display()))?;
    std::fs::create_dir_all(&tls_dir).with_context(|| format!("creating {}", tls_dir.display()))?;

    let mut cmd = Command::new(&opts.server_bin);
    cmd.kill_on_drop(true)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        // The child's log goes through a pipe into ours — strip its ANSI so the
        // merged output stays readable.
        .env("NO_COLOR", "1")
        .env("RUST_LOG_STYLE", "never")
        .arg("--bind")
        .arg(opts.bind.to_string())
        .arg("--advertise-host")
        .arg(opts.advertise_host.to_string())
        .arg("--c2-port")
        .arg(spec.ports.c2.to_string())
        .arg("--audio-out-port")
        .arg(spec.ports.audio_out.to_string())
        .arg("--audio-in-port")
        .arg(spec.ports.audio_in.to_string())
        .arg("--beast-port")
        .arg(spec.ports.beast.to_string())
        .arg("--ais-nmea-port")
        .arg(spec.ports.ais_nmea.to_string())
        .arg("--aprs-port")
        .arg(spec.ports.aprs.to_string())
        .arg("--no-beacon")
        .arg("--no-mdns")
        .arg("--mdns-name")
        .arg(format!("lanline-{}", spec.idx))
        .arg("--instance-label")
        .arg(&spec.label)
        .arg("--server-id")
        .arg(spec.server_id.to_string())
        .arg("--device")
        .arg(&spec.device_args)
        .arg("--iq-dir")
        .arg(&iq_dir)
        .arg("--tls-dir")
        .arg(&tls_dir);

    if let Some(url) = &opts.fleet_url {
        cmd.arg("--fleet-url").arg(url);
    }
    if spec.freq_correction_ppm != 0.0 {
        cmd.arg("--freq-correction-ppm").arg(format!("{}", spec.freq_correction_ppm));
    }
    if spec.tls {
        cmd.arg("--tls");
    }
    if spec.enable_tx {
        cmd.arg("--enable-tx");
    }
    Ok(cmd)
}

fn pipe_lines<R>(idx: usize, reader: R)
where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
{
    tokio::spawn(async move {
        let mut lines = BufReader::new(reader).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            tracing::info!("[radio {idx}] {line}");
        }
    });
}

pub fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Locate the `lanline-server` binary: `--server-bin`, else a sibling of this
/// executable, else bare `lanline-server` on `PATH`.
pub fn resolve_server_bin(explicit: Option<PathBuf>) -> PathBuf {
    if let Some(p) = explicit {
        return p;
    }
    if let Ok(me) = std::env::current_exe() {
        if let Some(dir) = me.parent() {
            let sib = dir.join("lanline-server");
            if sib.is_file() {
                return sib;
            }
        }
    }
    PathBuf::from("lanline-server")
}
