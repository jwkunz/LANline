//! HTTPS for the C2 port (`--tls`).
//!
//! Browsers only expose `getUserMedia` (the push-to-talk mic) and the
//! geolocation API on a **secure context**: `https://`, or `http://localhost`.
//! Reached from another device over a plain-`http://` LAN address they don't,
//! so those features silently do nothing. Serving TLS fixes that.
//!
//! With `--tls-cert`/`--tls-key` the server just loads those PEM files (use
//! `mkcert` for a cert your devices trust with no warning). Otherwise it
//! generates a self-signed cert covering `localhost`, `127.0.0.1`, `::1`, the
//! mDNS name, and every non-loopback IP it can see; caches it under
//! `--tls-dir` so the cert — and thus the browser's "proceed anyway"
//! exception — is stable across restarts; and logs its SHA-256 fingerprint so
//! you can check what the browser shows against what the server made.

use anyhow::{anyhow, Context, Result};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// Cert + key PEM plus a line describing where they came from.
pub struct Materials {
    pub cert_pem: Vec<u8>,
    pub key_pem: Vec<u8>,
    pub source: String,
    pub fingerprint_sha256: String,
}

/// Regenerate a cached self-signed cert once it's within this of expiry, or
/// this far past creation — keeps a long-lived install from ever serving an
/// expired cert without making the fingerprint churn.
const MAX_AGE: Duration = Duration::from_secs(300 * 24 * 3600);
/// Self-signed validity window.
const VALIDITY_DAYS: i64 = 397; // CA/B forum max for leaf certs; browsers accept it

pub fn prepare(cfg: &crate::config::Config, mdns_name: &str) -> Result<Materials> {
    match (&cfg.tls_cert, &cfg.tls_key) {
        (Some(c), Some(k)) => load_pem_files(c, k),
        (None, None) => auto_self_signed(cfg, mdns_name),
        _ => Err(anyhow!("--tls-cert and --tls-key must be given together")),
    }
}

fn load_pem_files(cert: &Path, key: &Path) -> Result<Materials> {
    let cert_pem = std::fs::read(cert).with_context(|| format!("reading {}", cert.display()))?;
    let key_pem = std::fs::read(key).with_context(|| format!("reading {}", key.display()))?;
    let fingerprint_sha256 = fingerprint(&cert_pem)?;
    Ok(Materials {
        cert_pem,
        key_pem,
        source: format!("{} + {}", cert.display(), key.display()),
        fingerprint_sha256,
    })
}

fn auto_self_signed(cfg: &crate::config::Config, mdns_name: &str) -> Result<Materials> {
    let dir = cfg.tls_dir.clone().unwrap_or_else(default_tls_dir);
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    let cert_path = dir.join("cert.pem");
    let key_path = dir.join("key.pem");

    let sans = collect_sans(mdns_name);

    if let Some(m) = try_reuse(&cert_path, &key_path)? {
        return Ok(m);
    }

    let (cert_pem, key_pem) = generate(&sans)?;
    write_secret(&key_path, key_pem.as_bytes())?;
    std::fs::write(&cert_path, cert_pem.as_bytes())
        .with_context(|| format!("writing {}", cert_path.display()))?;

    let fingerprint_sha256 = fingerprint(cert_pem.as_bytes())?;
    Ok(Materials {
        cert_pem: cert_pem.into_bytes(),
        key_pem: key_pem.into_bytes(),
        source: format!(
            "self-signed, generated for [{}], cached at {}",
            sans.join(", "),
            dir.display()
        ),
        fingerprint_sha256,
    })
}

/// Reuse the cached pair if both files exist and the cert is recent enough.
/// (No X.509 parse to check SAN coverage — if your address changed, reach the
/// server by its mDNS name, which is always covered, or delete the cache.)
fn try_reuse(cert_path: &Path, key_path: &Path) -> Result<Option<Materials>> {
    if !cert_path.is_file() || !key_path.is_file() {
        return Ok(None);
    }
    let meta = std::fs::metadata(cert_path)?;
    let age = meta
        .modified()
        .ok()
        .and_then(|m| SystemTime::now().duration_since(m).ok())
        .unwrap_or(Duration::ZERO);
    if age > MAX_AGE {
        tracing::info!("tls: cached cert is stale, regenerating");
        return Ok(None);
    }
    let cert_pem = std::fs::read(cert_path)?;
    let key_pem = std::fs::read(key_path)?;
    let fingerprint_sha256 = fingerprint(&cert_pem)?;
    Ok(Some(Materials {
        cert_pem,
        key_pem,
        source: format!("self-signed, cached ({})", cert_path.display()),
        fingerprint_sha256,
    }))
}

fn collect_sans(mdns_name: &str) -> Vec<String> {
    let mut sans = vec![
        "localhost".to_string(),
        format!("{mdns_name}.local"),
        "127.0.0.1".to_string(),
        "::1".to_string(),
    ];
    if let Ok(ifaces) = if_addrs::get_if_addrs() {
        for iface in ifaces {
            let ip = iface.ip();
            // Non-loopback IPv4 only. Global IPv6 addresses are usually
            // rotating privacy addresses that would just bloat the cert and
            // go stale; nobody browses to `https://[2600:…]:8730/`.
            if ip.is_loopback() || ip.is_ipv6() {
                continue;
            }
            let s = ip.to_string();
            if !sans.contains(&s) {
                sans.push(s);
            }
        }
    }
    sans
}

fn generate(sans: &[String]) -> Result<(String, String)> {
    let mut params =
        rcgen::CertificateParams::new(sans.to_vec()).context("building cert params")?;
    let mut dn = rcgen::DistinguishedName::new();
    dn.push(rcgen::DnType::CommonName, "LANline self-signed");
    dn.push(rcgen::DnType::OrganizationName, "LANline");
    params.distinguished_name = dn;

    let now = time::OffsetDateTime::now_utc();
    params.not_before = now - time::Duration::days(1);
    params.not_after = now + time::Duration::days(VALIDITY_DAYS);

    let key_pair = rcgen::KeyPair::generate().context("generating key pair")?;
    let cert = params.self_signed(&key_pair).context("self-signing cert")?;
    Ok((cert.pem(), key_pair.serialize_pem()))
}

fn fingerprint(cert_pem: &[u8]) -> Result<String> {
    use sha2::{Digest, Sha256};
    let der = rustls_pemfile::certs(&mut &cert_pem[..])
        .next()
        .context("no certificate in PEM")?
        .context("malformed certificate PEM")?;
    let digest = Sha256::digest(der.as_ref());
    Ok(digest
        .iter()
        .map(|b| format!("{b:02X}"))
        .collect::<Vec<_>>()
        .join(":"))
}

/// Write a private key with owner-only permissions where the platform supports
/// it.
fn write_secret(path: &Path, bytes: &[u8]) -> Result<()> {
    std::fs::write(path, bytes).with_context(|| format!("writing {}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

fn default_tls_dir() -> PathBuf {
    let sub = |base: PathBuf| base.join("lanline").join("tls");
    if let Ok(d) = std::env::var("LANLINE_STATE_DIR") {
        return PathBuf::from(d).join("tls");
    }
    #[cfg(windows)]
    if let Ok(d) = std::env::var("LOCALAPPDATA") {
        return sub(PathBuf::from(d));
    }
    #[cfg(not(windows))]
    if let Ok(d) = std::env::var("XDG_STATE_HOME") {
        return sub(PathBuf::from(d));
    }
    if let Ok(home) = std::env::var("HOME") {
        #[cfg(target_os = "macos")]
        return sub(PathBuf::from(home).join("Library").join("Application Support"));
        #[cfg(not(target_os = "macos"))]
        return sub(PathBuf::from(home).join(".local").join("state"));
    }
    #[cfg(windows)]
    if let Ok(up) = std::env::var("USERPROFILE") {
        return sub(PathBuf::from(up));
    }
    PathBuf::from("lanline-tls")
}

/// Install the process-wide rustls crypto provider (ring). Idempotent; a
/// second call is a no-op error we ignore.
pub fn install_crypto_provider() {
    let _ = rustls::crypto::ring::default_provider().install_default();
}

/// `true` when TLS should be served.
pub fn enabled(cfg: &crate::config::Config) -> bool {
    cfg.tls || cfg.tls_cert.is_some()
}

/// Convenience for logging: the scheme clients should use.
pub fn scheme(cfg: &crate::config::Config) -> &'static str {
    if enabled(cfg) {
        "https"
    } else {
        "http"
    }
}
