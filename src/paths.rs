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

#[derive(Clone, Copy, PartialEq, Debug)]
enum Os {
    Windows,
    Mac,
    Unix,
}

#[derive(Clone, Copy)]
enum Kind {
    Cache,
    Config,
    State,
}

const OS: Os = if cfg!(windows) {
    Os::Windows
} else if cfg!(target_os = "macos") {
    Os::Mac
} else {
    Os::Unix
};

/// Each OS's standard per-user directory, following the same conventions as
/// the `dirs` crate: XDG on Linux and BSD, `~/Library` on macOS, the
/// `%APPDATA%` / `%LOCALAPPDATA%` variables on Windows. `get` reads env vars.
fn base_dir(os: Os, kind: Kind, get: &dyn Fn(&str) -> Option<PathBuf>) -> Option<PathBuf> {
    match os {
        Os::Windows => get(match kind {
            Kind::Config => "APPDATA",
            _ => "LOCALAPPDATA",
        }),
        Os::Mac => {
            let lib = get("HOME")?.join("Library");
            Some(lib.join(match kind {
                Kind::Cache => "Caches",
                _ => "Application Support",
            }))
        }
        Os::Unix => {
            let (var, rel) = match kind {
                Kind::Cache => ("XDG_CACHE_HOME", ".cache"),
                Kind::Config => ("XDG_CONFIG_HOME", ".config"),
                Kind::State => ("XDG_STATE_HOME", ".local/state"),
            };
            // XDG says relative values must be ignored.
            get(var)
                .filter(|p| p.is_absolute())
                .or_else(|| get("HOME").map(|h| h.join(rel)))
        }
    }
}

/// State goes in the XDG state dir on Linux; elsewhere in a `state` folder
/// inside the local data dir.
fn state_path(os: Os, get: &dyn Fn(&str) -> Option<PathBuf>) -> Option<PathBuf> {
    let base = base_dir(os, Kind::State, get)?.join(APP);
    Some(if os == Os::Unix {
        base
    } else {
        base.join("state")
    })
}

fn home() -> Option<PathBuf> {
    from_env(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
}

/// Directory for downloaded data (pricing, FX rates) and per-session caches.
pub fn cache_dir() -> PathBuf {
    from_env("TACHOBAR_CACHE_DIR")
        .or_else(|| base_dir(OS, Kind::Cache, &from_env).map(|d| d.join(APP)))
        .unwrap_or_else(|| env::temp_dir().join(APP).join("cache"))
}

/// Directory for runtime state (burn-rate samples).
pub fn state_dir() -> PathBuf {
    from_env("TACHOBAR_STATE_DIR")
        .or_else(|| state_path(OS, &from_env))
        .unwrap_or_else(|| env::temp_dir().join(APP).join("state"))
}

/// Path of the TOML config file.
pub fn config_file() -> Option<PathBuf> {
    from_env("TACHOBAR_CONFIG")
        .or_else(|| base_dir(OS, Kind::Config, &from_env).map(|d| d.join(APP).join("config.toml")))
}

/// Claude Code's config directory (`CLAUDE_CONFIG_DIR` or `~/.claude`).
pub fn claude_dir() -> Option<PathBuf> {
    from_env("CLAUDE_CONFIG_DIR").or_else(|| home().map(|h| h.join(".claude")))
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

#[cfg(test)]
mod tests {
    use super::*;

    fn env_of<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<PathBuf> + 'a {
        move |k| {
            pairs
                .iter()
                .find(|(n, _)| *n == k)
                .map(|(_, v)| PathBuf::from(v))
        }
    }

    #[test]
    fn linux_follows_xdg() {
        let home = env_of(&[("HOME", "/h")]);
        assert_eq!(
            base_dir(Os::Unix, Kind::Cache, &home),
            Some("/h/.cache".into())
        );
        assert_eq!(
            base_dir(Os::Unix, Kind::Config, &home),
            Some("/h/.config".into())
        );
        assert_eq!(
            state_path(Os::Unix, &home),
            Some("/h/.local/state/tachobar".into())
        );
        let xdg = env_of(&[
            ("HOME", "/h"),
            ("XDG_CACHE_HOME", "/x"),
            ("XDG_STATE_HOME", "rel"),
        ]);
        assert_eq!(base_dir(Os::Unix, Kind::Cache, &xdg), Some("/x".into()));
        // A relative XDG value is ignored.
        assert_eq!(
            state_path(Os::Unix, &xdg),
            Some("/h/.local/state/tachobar".into())
        );
        assert_eq!(base_dir(Os::Unix, Kind::Cache, &env_of(&[])), None);
    }

    #[test]
    fn macos_uses_library() {
        let home = env_of(&[("HOME", "/Users/u")]);
        assert_eq!(
            base_dir(Os::Mac, Kind::Cache, &home),
            Some("/Users/u/Library/Caches".into())
        );
        assert_eq!(
            base_dir(Os::Mac, Kind::Config, &home),
            Some("/Users/u/Library/Application Support".into())
        );
        assert_eq!(
            state_path(Os::Mac, &home),
            Some("/Users/u/Library/Application Support/tachobar/state".into())
        );
    }

    #[test]
    fn windows_uses_appdata() {
        let e = env_of(&[("APPDATA", r"C:\R"), ("LOCALAPPDATA", r"C:\L")]);
        assert_eq!(
            base_dir(Os::Windows, Kind::Config, &e),
            Some(r"C:\R".into())
        );
        assert_eq!(base_dir(Os::Windows, Kind::Cache, &e), Some(r"C:\L".into()));
        assert_eq!(
            state_path(Os::Windows, &e),
            Some(PathBuf::from(r"C:\L").join("tachobar").join("state"))
        );
    }
}
