//! Cost calculations against fixture transcripts with hand-computed results.
//!
//! Prices come from `tests/fixtures/pricing.json`, a pinned copy of litellm's
//! entries, so these numbers do not move when the bundled snapshot is updated.
//!
//! Rates used below (USD per token):
//!   Haiku 4.5   in 1e-6, out 5e-6, 5m write 1.25e-6, 1h write 2e-6, read 1e-7
//!   Sonnet 4.5  in 3e-6, out 15e-6, 5m write 3.75e-6, 1h write 6e-6, read 3e-7,
//!               web search 0.01/request;
//!               >200k prompt: in 6e-6, out 22.5e-6, 5m write 7.5e-6, read 6e-7
//!   Opus 5.5    in 4e-6, out 20e-6, 1h write 8e-6, read 2e-7;
//!               fast mode in 8e-6, out 40e-6 (cache prices scale with input);
//!               US-only inference (inference_geo "us") 1.1x on all tokens

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use tachobar::pricing::{PriceFile, PriceTable, Priced, Source, Usage};
use tachobar::transcript::{self, file_cost, session_usage};

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn prices() -> PriceTable {
    let text = std::fs::read_to_string(fixtures().join("pricing.json")).unwrap();
    let file: PriceFile = serde_json::from_str(&text).unwrap();
    PriceTable::from_file(file, Source::Cache)
}

/// All tests in this binary share one scratch cache dir.
fn isolate() {
    static DIR: OnceLock<tempfile::TempDir> = OnceLock::new();
    let d = DIR.get_or_init(|| tempfile::tempdir().unwrap());
    std::env::set_var("TACHOBAR_CACHE_DIR", d.path().join("cache"));
    std::env::set_var("TACHOBAR_STATE_DIR", d.path().join("state"));
}

fn close(a: f64, b: f64) {
    assert!((a - b).abs() < 1e-9, "{a} != {b}");
}

fn subagent(name: &str) -> PathBuf {
    fixtures().join("session/sess-1/subagents").join(name)
}

#[test]
fn haiku_subagent_prices_each_cache_bucket_and_dedupes() {
    // Response 1 appears on two lines (two content blocks) and counts once:
    //   10*1e-6 + 200*5e-6 + 5000*1e-7 + 1000*1.25e-6 + 2000*2e-6 = 0.00676
    // Response 2: 5*1e-6 + 100*5e-6 + 8000*1e-7 + 500*2e-6 = 0.002305
    let c = file_cost(&subagent("agent-a.jsonl"), &prices());
    close(c.usd, 0.00676 + 0.002305);
    assert_eq!(c.tokens, (10 + 200 + 5000 + 3000) + (5 + 100 + 8000 + 500));
    assert_eq!(c.unpriced, 0);
}

#[test]
fn sonnet_subagent_uses_long_context_tier_and_web_search() {
    // Response 1 (160,100 prompt tokens, below 200k), 3 content-block lines:
    //   100*3e-6 + 1000*15e-6 + 150000*3e-7 + 10000*6e-6 + 1*0.01 = 0.1303
    // Response 2 (255,000 prompt tokens, long-context rates, legacy log
    // without TTL split so the 4000 writes are 5m writes):
    //   1000*6e-6 + 2000*22.5e-6 + 250000*6e-7 + 4000*7.5e-6 = 0.231
    let c = file_cost(&subagent("agent-b.jsonl"), &prices());
    close(c.usd, 0.1303 + 0.231);
    assert_eq!(c.unpriced, 0);
    // The legacy line's TTL is unknown, so the 5m price is only a lower bound.
    assert_eq!(c.estimated, 1);
}

#[test]
fn fast_mode_with_us_inference() {
    // One response on 2 lines, speed "fast", inference_geo "us":
    //   fast rates: in 8e-6, out 40e-6, 1h write 2x8e-6 = 16e-6, read 0.05x8e-6 = 0.4e-6
    //   (100*8e-6 + 1000*40e-6 + 20000*0.4e-6 + 5000*16e-6) * 1.1
    //   = (0.0008 + 0.04 + 0.008 + 0.08) * 1.1 = 0.14168
    let c = file_cost(&subagent("agent-d.jsonl"), &prices());
    close(c.usd, 0.14168);
    assert_eq!(c.unpriced, 0);
    assert_eq!(c.estimated, 0);
}

#[test]
fn unknown_model_is_reported_not_guessed() {
    let c = file_cost(&subagent("agent-c.jsonl"), &prices());
    close(c.usd, 0.0);
    assert_eq!(
        c.unpriced, 1,
        "claude-opus-9-9 must be unpriced, <synthetic> ignored"
    );
    assert_eq!(c.tokens, 2);
}

#[test]
fn main_transcript_matches_claude_codes_own_cost() {
    // Same token counts as a real session where Claude Code reported
    // costUSD = 0.2378552: 8*4e-6 + 682*20e-6 + 231276*2e-7 + 22241*8e-6.
    // Pricing the 1h writes at the 5m rate (1.25x input) would give 0.1711.
    let p = prices();
    let u = Usage {
        input: 8,
        output: 682,
        cache_read: 231_276,
        cache_write_1h: 22_241,
        ..Default::default()
    };
    let priced = p.price("claude-opus-5-5", &u).unwrap();
    assert!(matches!(priced, Priced::Exact(_)));
    close(priced.usd(), 0.2378552);
    let c = file_cost(&fixtures().join("session/sess-1.jsonl"), &p);
    close(c.usd, 0.2378552);
}

#[test]
fn session_totals_and_cache_reuse() {
    isolate();
    let p = prices();
    let main = fixtures().join("session/sess-1.jsonl");
    let u = session_usage(&main, "sess-1-test", &p);
    assert_eq!(u.subagent_files, 4);
    close(u.subagents.usd, 0.009065 + 0.3613 + 0.14168);
    assert_eq!(u.subagents.unpriced, 1);
    assert_eq!(u.subagents.estimated, 1);
    assert_eq!(u.main_tokens, 8 + 682 + 231_276 + 22_241);
    // Second call is served from the cache and must agree.
    let again = session_usage(&main, "sess-1-test", &p);
    assert_eq!(again, u);
}

#[test]
fn context_fallback_from_transcript_tail() {
    let main = fixtures().join("session/sess-1.jsonl");
    assert_eq!(
        transcript::context_tokens_from_tail(&main),
        Some(8 + 231_276 + 22_241)
    );
}
