//! tachobar: a Claude Code status line with accurate costs.

pub mod burn;
pub mod config;
pub mod currency;
pub mod input;
pub mod paths;
pub mod pricing;
pub mod refresh;
pub mod render;
pub mod setup;
pub mod transcript;

#[cfg(test)]
#[path = "../tests/common/mod.rs"]
mod testutil;

use std::path::Path;

use config::{Config, Segment};
use currency::FxTable;
use input::Input;
use pricing::{PriceTable, Source};
use render::{Painter, Report};

/// Everything besides stdin that affects a render, so tests can pin it down.
pub struct Env {
    pub config: Config,
    pub config_error: Option<String>,
    pub prices: PriceTable,
    pub fx: FxTable,
    pub now: f64,
    pub no_color: bool,
    pub width: usize,
    /// Effort from `CLAUDE_EFFORT_LEVEL` or `~/.claude/settings.json`.
    pub effort_fallback: Option<String>,
}

impl Env {
    pub fn from_system() -> Env {
        let (config, config_error) = Config::load();
        let no_color =
            config.no_color || std::env::var_os("NO_COLOR").is_some_and(|v| !v.is_empty());
        let width = std::env::var("COLUMNS")
            .ok()
            .and_then(|c| c.trim().parse().ok())
            .filter(|w| *w > 0)
            .unwrap_or(120);
        Env {
            config,
            config_error,
            prices: PriceTable::load(),
            fx: FxTable::load(),
            now: paths::now_secs_f64(),
            no_color,
            width,
            effort_fallback: effort_fallback(),
        }
    }
}

fn effort_fallback() -> Option<String> {
    if let Some(e) = std::env::var("CLAUDE_EFFORT_LEVEL")
        .ok()
        .filter(|e| !e.trim().is_empty())
    {
        return Some(e.trim().to_string());
    }
    let path = paths::claude_dir()?.join("settings.json");
    let v: serde_json::Value = serde_json::from_slice(&std::fs::read(path).ok()?).ok()?;
    v.get("effortLevel")?.as_str().map(str::to_string)
}

fn days(secs: u64) -> u64 {
    secs / 86_400
}

/// Compute the report for one stdin payload.
pub fn build_report(input: &Input, env: &Env) -> Report {
    let cfg = &env.config;
    let now = env.now as u64;
    let mut notes = Vec::new();
    if let Some(e) = &env.config_error {
        notes.push(e.clone());
    }

    let model_id = input.model.id.clone().unwrap_or_default();
    let model_name = input
        .model
        .display_name
        .clone()
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| model_id.clone());
    let price = env.prices.lookup(&model_id).map(|(_, p)| p);
    if price.is_none() && !model_id.is_empty() {
        notes.push(format!("unpriced model {model_id}"));
    }
    let price_per_mtok = price
        .and_then(|p| p.rates(0))
        .map(|r| (r.input * 1e6, r.output * 1e6));

    let stale_secs = cfg.thresholds.stale_days * 86_400;
    let price_age = env.prices.age_secs(now);
    if price_age > stale_secs {
        let what = if env.prices.source == Source::Bundled {
            "bundled prices"
        } else {
            "prices"
        };
        notes.push(format!("{what} {}d old", days(price_age)));
    }

    let wants_fx = !cfg.currency.eq_ignore_ascii_case("USD");
    let fx_rate = env.fx.rate(&cfg.currency);
    if wants_fx {
        match fx_rate {
            None => notes.push(format!("no {} rate, showing USD", cfg.currency)),
            Some(_) if env.fx.age_secs(now) > stale_secs => {
                notes.push(format!("fx {}d old", days(env.fx.age_secs(now))))
            }
            Some(_) => {}
        }
    }

    let context_size = input
        .context_size()
        .or_else(|| price.and_then(|p| p.max_input_tokens))
        .unwrap_or(200_000);
    let transcript = input
        .transcript_path
        .as_deref()
        .map(Path::new)
        .filter(|p| p.is_file());
    let context_tokens = input
        .context_tokens()
        .or_else(|| transcript.and_then(transcript::context_tokens_from_tail));

    let needs_transcripts = cfg
        .segments
        .iter()
        .any(|s| matches!(s, Segment::Cost | Segment::Burn));
    let usage = match (transcript, &input.session_id) {
        (Some(t), Some(id)) if needs_transcripts => {
            Some(transcript::session_usage(t, id, &env.prices))
        }
        _ => None,
    };

    let main_usd = input.cost.total_cost_usd;
    let sub = usage.map(|u| u.subagents).unwrap_or_default();
    let cost_usd = match (main_usd, sub.usd > 0.0 || sub.unpriced > 0) {
        (None, false) => None,
        (m, _) => Some(m.unwrap_or(0.0) + sub.usd),
    };
    if sub.unpriced > 0 {
        notes.push(format!("{} subagent calls unpriced", sub.unpriced));
    }
    if sub.estimated > 0 {
        notes.push(format!("{} subagent calls estimated", sub.estimated));
    }

    let burn = match (
        &input.session_id,
        cost_usd,
        cfg.segments.contains(&Segment::Burn),
    ) {
        (Some(id), Some(usd), true) => {
            let tokens = usage.map_or(0, |u| u.main_tokens + u.subagents.tokens);
            burn::record(id, env.now, usd, tokens)
        }
        _ => None,
    };

    let dir = input.dir().map(|d| {
        Path::new(d)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| d.to_string())
    });

    Report {
        dir,
        model_name,
        price_per_mtok,
        effort: input
            .effort
            .as_ref()
            .and_then(|e| e.level.clone())
            .or_else(|| env.effort_fallback.clone()),
        context: context_tokens.map(|t| (t, context_size)),
        cost_usd,
        cost_partial: sub.unpriced > 0 || sub.estimated > 0,
        burn,
        fx_rate: if wants_fx { fx_rate } else { Some(1.0) },
        notes,
    }
}

/// Render one status line from raw stdin text.
pub fn render_statusline(stdin: &str, env: &Env) -> String {
    let painter = Painter {
        color: !env.no_color,
    };
    let input = match Input::parse(stdin) {
        Ok(i) => i,
        Err(e) => return render::wrap(&[format!("tachobar: {e}")], env.width),
    };
    let report = build_report(&input, env);
    render::wrap(&render::segments(&report, &env.config, &painter), env.width)
}

/// Start a background refresh if data is due and auto-refresh is on.
pub fn maybe_refresh(env: &Env) {
    if !env.config.auto_refresh || std::env::var_os("TACHOBAR_NO_REFRESH").is_some() {
        return;
    }
    let now = env.now as u64;
    let need_fx = !env.config.currency.eq_ignore_ascii_case("USD");
    let fx_age = need_fx.then(|| {
        if env.fx.bundled {
            u64::MAX
        } else {
            env.fx.age_secs(now)
        }
    });
    if refresh::due(
        env.prices.age_secs(now),
        env.prices.source == Source::Cache,
        fx_age,
        now,
    ) {
        refresh::spawn_background(need_fx);
    }
}
