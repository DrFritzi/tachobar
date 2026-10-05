//! End-to-end: run the binary the way Claude Code does.

mod common;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

struct Sandbox {
    dir: common::TempDir,
}

impl Sandbox {
    /// Pinned pricing (as a fresh download) and a fixed EUR rate.
    fn new(config: &str) -> Sandbox {
        let dir = common::tempdir();
        let cache = dir.path().join("cache");
        std::fs::create_dir_all(&cache).unwrap();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let mut pricing: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(fixtures().join("pricing.json")).unwrap(),
        )
        .unwrap();
        pricing["fetched_at"] = now.into();
        std::fs::write(cache.join("pricing.json"), pricing.to_string()).unwrap();
        std::fs::write(
            cache.join("fx.json"),
            format!(
                r#"{{"fetched_at":{now},"date":"2026-01-01","rates":{{"EUR":0.5,"GBP":0.25}}}}"#
            ),
        )
        .unwrap();
        std::fs::write(dir.path().join("config.toml"), config).unwrap();
        Sandbox { dir }
    }

    fn run(&self, stdin: &str, extra_env: &[(&str, &str)]) -> String {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_tachobar"));
        cmd.env("TACHOBAR_CACHE_DIR", self.dir.path().join("cache"))
            .env("TACHOBAR_STATE_DIR", self.dir.path().join("state"))
            .env("TACHOBAR_CONFIG", self.dir.path().join("config.toml"))
            .env("CLAUDE_CONFIG_DIR", self.dir.path().join("claude"))
            .env("TACHOBAR_NO_REFRESH", "1")
            .env("NO_COLOR", "1")
            .env("COLUMNS", "200")
            .env_remove("CLAUDE_EFFORT_LEVEL")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped());
        for (k, v) in extra_env {
            cmd.env(k, v);
        }
        let mut child = cmd.spawn().unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(stdin.as_bytes())
            .unwrap();
        let out = child.wait_with_output().unwrap();
        assert!(out.status.success());
        String::from_utf8(out.stdout).unwrap()
    }
}

fn stdin(model: &str, cost: f64) -> String {
    serde_json::json!({
        "session_id": "e2e-session",
        "transcript_path": fixtures().join("session/sess-1.jsonl"),
        "workspace": { "current_dir": "/work/myproject" },
        "model": { "id": model, "display_name": "Opus 5.5" },
        "cost": { "total_cost_usd": cost },
        "context_window": {
            "context_window_size": 1000000,
            "current_usage": { "input_tokens": 2000, "cache_creation_input_tokens": 10000, "cache_read_input_tokens": 130000 }
        },
        "effort": { "level": "high" }
    })
    .to_string()
}

#[test]
fn renders_full_line_in_eur_with_subagents() {
    let sb = Sandbox::new("currency = \"EUR\"\n");
    let out = sb.run(&stdin("claude-opus-5-5", 1.5), &[]);
    // Main 1.5 USD + subagents 0.512045 USD (incl. fast mode + US-only) = 2.012045 USD -> 1.006 EUR.
    // Prices: 4/20 USD per 1M -> 2/10 EUR.
    assert_eq!(
        out,
        "myproject Opus 5.5 high €2/€10/M 142k/1M ctx (14%) €1.01+ [1 subagent calls unpriced] [1 subagent calls estimated]\n"
    );
    assert!(!out.contains('\r'));
}

#[test]
fn colors_unless_no_color() {
    let sb = Sandbox::new("");
    let mut cmd_out = sb.run(&stdin("claude-opus-5-5", 1.0), &[("NO_COLOR", "")]);
    assert!(cmd_out.contains("\x1b[38;5;"));
    cmd_out = sb.run(&stdin("claude-opus-5-5", 1.0), &[]);
    assert!(!cmd_out.contains('\x1b'));
}

#[test]
fn unknown_model_is_flagged() {
    let sb = Sandbox::new("segments = [\"model\", \"price\"]\n");
    let out = sb.run(&stdin("claude-opus-9-9", 1.0), &[]);
    assert_eq!(out, "Opus 5.5 ?/?/M [unpriced model claude-opus-9-9]\n");
}

#[test]
fn wraps_to_columns() {
    let sb = Sandbox::new("");
    let out = sb.run(&stdin("claude-opus-5-5", 1.0), &[("COLUMNS", "30")]);
    assert!(out.lines().count() > 1);
    assert!(out.lines().all(|l| l.chars().count() <= 30), "{out:?}");
}

#[test]
fn survives_garbage_and_empty_input() {
    let sb = Sandbox::new("");
    assert!(sb
        .run("{not json", &[])
        .starts_with("tachobar: invalid status line JSON"));
    assert_eq!(sb.run("", &[]), "?/?/M\n");
}

#[test]
fn broken_config_still_renders() {
    let sb = Sandbox::new("currency = 12\n");
    let out = sb.run(&stdin("claude-opus-5-5", 1.0), &[]);
    assert!(out.contains("$4/$20/M"), "{out}");
    assert!(out.contains("[config.toml:"), "{out}");
}

#[test]
fn stale_bundled_prices_are_flagged() {
    let sb = Sandbox::new("segments = [\"price\"]\n[thresholds]\nstale_days = 0\n");
    std::fs::remove_file(sb.dir.path().join("cache/pricing.json")).unwrap();
    let out = sb.run(&stdin("claude-opus-5-5", 1.0), &[]);
    assert!(out.contains("[bundled prices"), "{out}");
}

#[test]
fn effort_falls_back_to_env() {
    let sb = Sandbox::new("segments = [\"effort\"]\n");
    let mut v: serde_json::Value = serde_json::from_str(&stdin("claude-opus-5-5", 1.0)).unwrap();
    v.as_object_mut().unwrap().remove("effort");
    assert_eq!(
        sb.run(&v.to_string(), &[("CLAUDE_EFFORT_LEVEL", "low")]),
        "low\n"
    );
}

#[test]
fn init_prints_snippet() {
    let out = Command::new(env!("CARGO_BIN_EXE_tachobar"))
        .arg("--init")
        .output()
        .unwrap();
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(text.contains("\"statusLine\""));
    assert!(text.contains("\"type\": \"command\""));
}

#[test]
fn raw_model_id_shows_family_and_version() {
    let sb = Sandbox::new(
        "[thresholds]
stale_days = 100000
",
    );
    std::fs::remove_file(sb.dir.path().join("cache/pricing.json")).unwrap();
    let run = |id: &str| {
        let input = serde_json::json!({
            "workspace": { "current_dir": "/work/project" },
            "model": { "id": id, "display_name": id },
            "cost": { "total_cost_usd": 1.5 },
            "context_window": { "context_window_size": 1000000, "current_usage": { "input_tokens": 2000 } },
            "effort": { "level": "high" }
        })
        .to_string();
        sb.run(&input, &[])
    };
    let out = run("claude-sonnet-5-5");
    assert_eq!(
        out,
        "project Sonnet 5.5 high $2/$10/M 2k/1M ctx (0%) $1.50\n"
    );
    assert!(!out.contains("...") && !out.contains('\u{2026}'));
    assert!(run("claude-sonnet-5").starts_with("project Sonnet 5 "));
    assert!(run("claude-haiku-4-5-20251001").starts_with("project Haiku 4.5 "));
    assert!(run("claude-opus-5-5-fast").starts_with("project claude-opus-5-5-fast "));
    assert!(run("acme-x").starts_with("project acme-x "));
}

#[test]
fn never_hides_values_behind_ellipsis() {
    let sb = Sandbox::new("");
    let long_dir = format!("/work/{}", "d".repeat(150));
    let long_model = format!("acme-{}", "x".repeat(150));
    let input = serde_json::json!({
        "session_id": "e2e-session",
        "transcript_path": fixtures().join("session/sess-1.jsonl"),
        "workspace": { "current_dir": long_dir },
        "model": { "id": long_model, "display_name": long_model },
        "cost": { "total_cost_usd": 1.0 },
        "context_window": { "context_window_size": 1000000, "current_usage": { "input_tokens": 2000 } },
        "effort": { "level": "high" }
    })
    .to_string();
    for cols in ["10", "30", "80", "300"] {
        let out = sb.run(&input, &[("COLUMNS", cols)]);
        assert!(
            !out.contains("...") && !out.contains('\u{2026}'),
            "{cols}: {out:?}"
        );
        assert!(out.contains(&"d".repeat(150)), "{cols}: {out:?}");
        assert!(out.contains(&"x".repeat(150)), "{cols}: {out:?}");
    }
}
