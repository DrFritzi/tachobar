//! Daily download of pricing and FX data.
//!
//! Rendering never waits on the network: when data is older than a day, the
//! renderer starts `tachobar refresh --quiet` as a detached background process
//! and renders with what it has. At most one attempt per hour is made, so an
//! offline machine does not spawn a process on every render.

use std::fs;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Duration;

use crate::currency::{self, FxFile};
use crate::paths;
use crate::pricing::{self, PriceFile};

const MAX_AGE_SECS: u64 = 24 * 3600;
const RETRY_SECS: u64 = 3600;
const TIMEOUT: Duration = Duration::from_secs(5);
const MAX_BODY: u64 = 64 * 1024 * 1024;

fn attempt_marker() -> PathBuf {
    paths::cache_dir().join("refresh-attempt")
}

fn agent() -> ureq::Agent {
    let tls = ureq::tls::TlsConfig::builder()
        .root_certs(ureq::tls::RootCerts::PlatformVerifier)
        .build();
    ureq::Agent::config_builder()
        .timeout_global(Some(TIMEOUT))
        .user_agent(concat!("tachobar/", env!("CARGO_PKG_VERSION")))
        .tls_config(tls)
        .build()
        .into()
}

fn get(agent: &ureq::Agent, url: &str) -> Result<String, String> {
    agent
        .get(url)
        .call()
        .map_err(|e| format!("{url}: {e}"))?
        .body_mut()
        .with_config()
        .limit(MAX_BODY)
        .read_to_string()
        .map_err(|e| format!("{url}: {e}"))
}

pub fn fetch_pricing(agent: &ureq::Agent) -> Result<PriceFile, String> {
    let body = get(agent, pricing::LITELLM_URL)?;
    let models = pricing::extract_from_litellm(&body)?;
    Ok(PriceFile {
        fetched_at: paths::now_secs(),
        models,
    })
}

pub fn fetch_fx(agent: &ureq::Agent) -> Result<FxFile, String> {
    let body = get(agent, currency::FRANKFURTER_URL)?;
    currency::parse_frankfurter(&body, paths::now_secs())
}

/// Download both data sets into the cache. Returns one line per source.
pub fn run(need_fx: bool) -> Vec<Result<String, String>> {
    let agent = agent();
    let mut out = Vec::new();
    out.push(fetch_pricing(&agent).and_then(|f| {
        let n = f.models.len();
        write_json(&pricing::cache_path(), &f)?;
        Ok(format!("pricing: {n} Claude models from litellm"))
    }));
    if need_fx {
        out.push(fetch_fx(&agent).and_then(|f| {
            let msg = format!("fx: {} currencies, ECB rates of {}", f.rates.len(), f.date);
            write_json(&currency::cache_path(), &f)?;
            Ok(msg)
        }));
    }
    out
}

/// Write fresh snapshots for the bundled data (maintainers, before a release).
pub fn write_snapshots(dir: &std::path::Path) -> Result<(), String> {
    let agent = agent();
    let p = fetch_pricing(&agent)?;
    let fx = fetch_fx(&agent)?;
    write_json_pretty(&dir.join("pricing-snapshot.json"), &p)?;
    write_json_pretty(&dir.join("fx-snapshot.json"), &fx)?;
    Ok(())
}

fn write_json<T: serde::Serialize>(path: &std::path::Path, v: &T) -> Result<(), String> {
    let bytes = serde_json::to_vec(v).map_err(|e| e.to_string())?;
    paths::write_atomic(path, &bytes).map_err(|e| format!("{}: {e}", path.display()))
}

fn write_json_pretty<T: serde::Serialize>(path: &std::path::Path, v: &T) -> Result<(), String> {
    let mut bytes = serde_json::to_vec_pretty(v).map_err(|e| e.to_string())?;
    bytes.push(b'\n');
    paths::write_atomic(path, &bytes).map_err(|e| format!("{}: {e}", path.display()))
}

/// Should the renderer kick off a background refresh?
pub fn due(pricing_age: u64, pricing_from_cache: bool, fx_age: Option<u64>, now: u64) -> bool {
    let stale = !pricing_from_cache
        || pricing_age > MAX_AGE_SECS
        || fx_age.is_some_and(|a| a > MAX_AGE_SECS);
    if !stale {
        return false;
    }
    let last_attempt = fs::metadata(attempt_marker())
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_secs());
    now.saturating_sub(last_attempt) > RETRY_SECS
}

/// Start `tachobar refresh --quiet` detached from this process.
pub fn spawn_background(need_fx: bool) {
    if paths::write_atomic(&attempt_marker(), b"").is_err() {
        return; // can't rate-limit attempts, so don't try at all
    }
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let mut cmd = Command::new(exe);
    cmd.arg("refresh").arg("--quiet");
    if !need_fx {
        cmd.arg("--no-fx");
    }
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(DETACHED_PROCESS | CREATE_NO_WINDOW);
    }
    let _ = cmd.spawn();
}
