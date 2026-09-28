//! Platform directories. Every location can be overridden with an environment
//! variable, which the tests use to stay hermetic.

use std::env;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

const APP: &str = "tachobar";

fn from_env(var: &str) -> Option<PathBuf> {
    env::var_os(var)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

/// Directory for downloaded data (pricing, FX rates) and per-session caches.
pub fn cache_dir() -> PathBuf {
    from_env("TACHOBAR_CACHE_DIR")
        .or_else(|| dirs::cache_dir().map(|d| d.join(APP)))
        .unwrap_or_else(|| env::temp_dir().join(APP).join("cache"))
}

/// Directory for runtime state (burn-rate samples). `dirs::state_dir` only
/// exists on Linux, so fall back to the local data dir elsewhere.
pub fn state_dir() -> PathBuf {
    from_env("TACHOBAR_STATE_DIR")
        .or_else(|| dirs::state_dir().map(|d| d.join(APP)))
        .or_else(|| dirs::data_local_dir().map(|d| d.join(APP).join("state")))
        .unwrap_or_else(|| env::temp_dir().join(APP).join("state"))
}

/// Path of the TOML config file.
pub fn config_file() -> Option<PathBuf> {
    from_env("TACHOBAR_CONFIG")
        .or_else(|| dirs::config_dir().map(|d| d.join(APP).join("config.toml")))
}

/// Claude Code's config directory (`CLAUDE_CONFIG_DIR` or `~/.claude`).
pub fn claude_dir() -> Option<PathBuf> {
    from_env("CLAUDE_CONFIG_DIR").or_else(|| dirs::home_dir().map(|h| h.join(".claude")))
}

/// Keep only characters that are safe in a file name on every platform.
pub fn sanitize(name: &str) -> String {
    name.chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .take(128)
        .collect()
}

/// Write via a temp file in the same directory, then rename over the target,
/// so readers never see a half-written file.
pub fn write_atomic(path: &Path, data: &[u8]) -> io::Result<()> {
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(dir)?;
    let file_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("tmp");
    let tmp = dir.join(format!(".{file_name}.{}.tmp", std::process::id()));
    {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(data)?;
        f.sync_all().ok();
    }
    fs::rename(&tmp, path).inspect_err(|_| {
        let _ = fs::remove_file(&tmp);
    })
}

pub fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

pub fn now_secs_f64() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}
