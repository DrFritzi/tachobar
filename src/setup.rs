//! `tachobar --init`: the `statusLine` entry for Claude Code's settings.json.

use serde_json::{json, Value};
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

/// Byte spans of one top-level member: its key's opening quote and its value.
struct Member {
    key: String,
    key_start: usize,
    val: std::ops::Range<usize>,
}

/// Top-level members of a JSON object and the position of its closing brace.
/// The text must already be valid JSON.
fn scan_object(text: &str) -> Option<(Vec<Member>, usize)> {
    let b = text.as_bytes();
    let ws = |mut i: usize| {
        while i < b.len() && b[i].is_ascii_whitespace() {
            i += 1;
        }
        i
    };
    // End of the string that starts at `i` (index after the closing quote).
    let string_end = |mut i: usize| {
        i += 1;
        while i < b.len() && b[i] != b'"' {
            i += if b[i] == b'\\' { 2 } else { 1 };
        }
        i + 1
    };
    let value_end = |mut i: usize| {
        if b[i] == b'"' {
            return string_end(i);
        }
        let mut depth = 0usize;
        while i < b.len() {
            match b[i] {
                b'"' => i = string_end(i) - 1,
                b'{' | b'[' => depth += 1,
                b'}' | b']' if depth > 0 => depth -= 1,
                b',' | b'}' | b']' | b' ' | b'\t' | b'\r' | b'\n' if depth == 0 => return i,
                _ => {}
            }
            i += 1;
            if depth == 0 && i < b.len() && matches!(b[i - 1], b'}' | b']') {
                return i;
            }
        }
        i
    };
    let mut i = ws(0);
    if b.get(i) != Some(&b'{') {
        return None;
    }
    i = ws(i + 1);
    let mut members = Vec::new();
    while b.get(i) == Some(&b'"') {
        let key_start = i;
        let key_end = string_end(i);
        let key = serde_json::from_str::<String>(&text[key_start..key_end]).ok()?;
        i = ws(key_end);
        i = ws(i + 1); // the colon
        let val_end = value_end(i);
        members.push(Member {
            key,
            key_start,
            val: i..val_end,
        });
        i = ws(val_end);
        if b.get(i) == Some(&b',') {
            i = ws(i + 1);
        }
    }
    (b.get(i) == Some(&b'}')).then_some((members, i))
}

/// Set `statusLine` in a settings.json by editing the text in place, so every
/// other byte (key order, indentation, line endings)
/// stays as the user wrote it. Backs the old file up to `settings.json.bak`.
pub fn write_settings(path: &Path, command: &str) -> Result<Option<Value>, String> {
    let text = match fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(format!("{}: {e}", path.display())),
    };
    let new_value = status_line_value(command);
    let (updated, previous) = if text.trim().is_empty() {
        let mut doc = serde_json::to_string_pretty(&json!({ "statusLine": new_value }))
            .map_err(|e| e.to_string())?;
        doc.push('\n');
        (doc, None)
    } else {
        let old: Value = serde_json::from_str(&text)
            .map_err(|e| format!("{}: not valid JSON ({e}), not touching it", path.display()))?;
        if !old.is_object() {
            return Err(format!("{}: top level is not an object", path.display()));
        }
        let updated = splice(&text, &new_value)
            .ok_or_else(|| format!("{}: unexpected layout, not touching it", path.display()))?;
        // Whatever the edit did, the result must be the old settings plus our key.
        let mut expected = old.clone();
        expected["statusLine"] = new_value;
        if serde_json::from_str::<Value>(&updated).ok().as_ref() != Some(&expected) {
            return Err(format!(
                "{}: could not edit safely, not touching it",
                path.display()
            ));
        }
        (updated, old.get("statusLine").cloned())
    };
    if path.exists() {
        let bak = path.with_extension("json.bak");
        fs::copy(path, &bak).map_err(|e| format!("backup to {}: {e}", bak.display()))?;
    }
    paths::write_atomic(path, updated.as_bytes())
        .map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(previous)
}

/// `text` with the top-level `statusLine` set to `value`.
fn splice(text: &str, value: &Value) -> Option<String> {
    let (members, close) = scan_object(text)?;
    let multiline = text.contains('\n');
    let eol = if text.contains("\r\n") { "\r\n" } else { "\n" };
    // Indentation of the members (empty for a compact file).
    let indent = members
        .last()
        .and_then(|m| {
            let line = text[..m.key_start].rsplit('\n').next()?;
            line.trim().is_empty().then(|| line.to_string())
        })
        .unwrap_or_else(|| "  ".to_string());
    let render = |base: &str| {
        if multiline {
            let pretty = serde_json::to_string_pretty(value).unwrap_or_default();
            pretty.replace('\n', &format!("{eol}{base}"))
        } else {
            value.to_string()
        }
    };
    let mut out = String::with_capacity(text.len() + 128);
    if let Some(m) = members.iter().rev().find(|m| m.key == "statusLine") {
        out.push_str(&text[..m.val.start]);
        out.push_str(&render(&indent));
        out.push_str(&text[m.val.end..]);
        return Some(out);
    }
    match members.last() {
        Some(last) => {
            out.push_str(&text[..last.val.end]);
            if multiline {
                out.push_str(&format!(
                    ",{eol}{indent}\"statusLine\": {}",
                    render(&indent)
                ));
            } else {
                out.push_str(&format!(",\"statusLine\":{}", render("")));
            }
            out.push_str(&text[last.val.end..]);
        }
        None => {
            // `{}` with whatever whitespace is inside.
            let open = text[..close].find('{')?;
            out.push_str(&text[..=open]);
            out.push_str(&format!("{eol}  \"statusLine\": {}{eol}", render("  ")));
            out.push_str(&text[close..]);
        }
    }
    Some(out)
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

    fn edit(before: &str) -> String {
        let dir = crate::testutil::tempdir();
        let p = dir.path().join("settings.json");
        fs::write(&p, before).unwrap();
        write_settings(&p, "tb").unwrap();
        fs::read_to_string(&p).unwrap()
    }

    #[test]
    fn keeps_formatting_when_adding() {
        let out = edit("{\n\t\"a\": [1, 2],\n\t\"b\": {\"x\": \"}\"}\n}\n");
        assert!(
            out.starts_with("{\n\t\"a\": [1, 2],\n\t\"b\": {\"x\": \"}\"},\n\t\"statusLine\": {")
        );
        assert!(out.ends_with("}\n}\n"));
        serde_json::from_str::<Value>(&out).unwrap();
    }

    #[test]
    fn keeps_crlf_and_replaces_in_place() {
        let out = edit("{\r\n  \"statusLine\": \"old\",\r\n  \"z\": 1\r\n}\r\n");
        assert!(out.contains("\"z\": 1\r\n}\r\n"));
        assert!(!out.contains("old"));
        assert!(!out.replace("\r\n", "").contains('\n'));
    }

    #[test]
    fn handles_compact_empty_and_nested_lookalikes() {
        let out = edit(r#"{"a":1}"#);
        assert!(out.starts_with(r#"{"a":1,"statusLine":{"#) && !out.contains('\n'));
        let v: Value = serde_json::from_str(&edit("{}")).unwrap();
        assert_eq!(v["statusLine"]["command"], "tb");
        // A nested statusLine is not the top-level one.
        let v: Value = serde_json::from_str(&edit(r#"{"n":{"statusLine":1}}"#)).unwrap();
        assert_eq!(v["n"]["statusLine"], 1);
        assert_eq!(v["statusLine"]["command"], "tb");
    }
}
