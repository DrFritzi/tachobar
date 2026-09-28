//! `tachobar --init`: the `statusLine` entry for Claude Code's settings.json.

use serde_json::{json, Map, Value};
use std::fs;
use std::path::{Path, PathBuf};

use crate::paths;

/// Command string for settings.json. Uses the absolute path of this binary so
/// it works even when the install dir is not on Claude Code's PATH.
pub fn command() -> String {
    match std::env::current_exe()
        .ok()
        .and_then(|p| p.canonicalize().ok().or(Some(p)))
    {
        Some(p) => {
            let s = p.to_string_lossy().into_owned();
            // Strip Windows' verbatim prefix; settings.json wants a plain path.
            let s = s.strip_prefix(r"\\?\").map(str::to_string).unwrap_or(s);
            // Claude Code may run the command through bash on Windows, where
            // backslashes are escapes; forward slashes work in every shell.
            let s = if cfg!(windows) {
                s.replace('\\', "/")
            } else {
                s
            };
            if s.contains(' ') || cfg!(windows) {
                format!("\"{s}\"")
            } else {
                s
            }
        }
        None => "tachobar".to_string(),
    }
}

pub fn status_line_value(command: &str) -> Value {
    json!({ "type": "command", "command": command, "padding": 0 })
}

pub fn snippet(command: &str) -> String {
    let v = json!({ "statusLine": status_line_value(command) });
    serde_json::to_string_pretty(&v).unwrap_or_default()
}

pub fn settings_path() -> Option<PathBuf> {
    paths::claude_dir().map(|d| d.join("settings.json"))
}

/// Set `statusLine` in a settings.json, keeping every other key and their
/// order. Backs the old file up to `settings.json.bak` first.
pub fn write_settings(path: &Path, command: &str) -> Result<Option<Value>, String> {
    let mut root = match fs::read_to_string(path) {
        Ok(text) if !text.trim().is_empty() => serde_json::from_str::<Value>(&text)
            .map_err(|e| format!("{}: not valid JSON ({e}), not touching it", path.display()))?,
        Ok(_) => Value::Object(Map::new()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Value::Object(Map::new()),
        Err(e) => return Err(format!("{}: {e}", path.display())),
    };
    let obj = root
        .as_object_mut()
        .ok_or_else(|| format!("{}: top level is not an object", path.display()))?;
    if path.exists() {
        let bak = path.with_extension("json.bak");
        fs::copy(path, &bak).map_err(|e| format!("backup to {}: {e}", bak.display()))?;
    }
    let previous = obj.insert("statusLine".into(), status_line_value(command));
    let mut bytes = serde_json::to_vec_pretty(&root).map_err(|e| e.to_string())?;
    bytes.push(b'\n');
    paths::write_atomic(path, &bytes).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(previous)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merges_into_existing_settings() {
        let dir = crate::testutil::tempdir();
        let p = dir.path().join("settings.json");
        fs::write(
            &p,
            r#"{"effortLevel":"high","statusLine":{"type":"command","command":"old"},"zzz":1}"#,
        )
        .unwrap();
        let prev = write_settings(&p, "/bin/tachobar").unwrap();
        assert_eq!(prev.unwrap()["command"], "old");
        let v: Value = serde_json::from_str(&fs::read_to_string(&p).unwrap()).unwrap();
        assert_eq!(v["statusLine"]["command"], "/bin/tachobar");
        assert_eq!(v["effortLevel"], "high");
        let keys: Vec<_> = v.as_object().unwrap().keys().cloned().collect();
        assert_eq!(keys, ["effortLevel", "statusLine", "zzz"]);
        assert!(dir.path().join("settings.json.bak").exists());
    }

    #[test]
    fn creates_missing_settings() {
        let dir = crate::testutil::tempdir();
        let p = dir.path().join("sub").join("settings.json");
        write_settings(&p, "tachobar").unwrap();
        assert!(fs::read_to_string(&p).unwrap().contains("\"statusLine\""));
    }

    #[test]
    fn refuses_invalid_json() {
        let dir = crate::testutil::tempdir();
        let p = dir.path().join("settings.json");
        fs::write(&p, "{ nope").unwrap();
        assert!(write_settings(&p, "x").is_err());
        assert_eq!(fs::read_to_string(&p).unwrap(), "{ nope");
    }
}
