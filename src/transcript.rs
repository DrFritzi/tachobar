//! Claude Code transcript (`*.jsonl`) parsing.
//!
//! Each assistant API response is written as one line per content block
//! (thinking, text, tool_use...), and every one of those lines repeats the
//! same `message.id`, `requestId` and `usage`. Summing lines naively counts a
//! response two or three times, so usage is deduplicated by
//! `(message.id, requestId)` and the last copy wins.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use crate::paths;
use crate::pricing::{Geo, PriceTable, Priced, Speed, Tier, Usage};

#[derive(Deserialize)]
struct Line {
    #[serde(rename = "requestId")]
    request_id: Option<String>,
    uuid: Option<String>,
    #[serde(rename = "isSidechain", default)]
    is_sidechain: bool,
    message: Option<Message>,
}

#[derive(Deserialize)]
struct Message {
    id: Option<String>,
    model: Option<String>,
    usage: Option<RawUsage>,
}

#[derive(Deserialize, Default)]
struct RawUsage {
    #[serde(default)]
    input_tokens: u64,
    #[serde(default)]
    output_tokens: u64,
    #[serde(default)]
    cache_creation_input_tokens: u64,
    #[serde(default)]
    cache_read_input_tokens: u64,
    cache_creation: Option<CacheCreation>,
    server_tool_use: Option<ServerToolUse>,
    speed: Option<String>,
    inference_geo: Option<String>,
    service_tier: Option<String>,
}

#[derive(Deserialize, Default)]
struct CacheCreation {
    #[serde(default)]
    ephemeral_5m_input_tokens: u64,
    #[serde(default)]
    ephemeral_1h_input_tokens: u64,
}

#[derive(Deserialize, Default)]
struct ServerToolUse {
    #[serde(default)]
    web_search_requests: u64,
}

impl RawUsage {
    fn to_usage(&self) -> Usage {
        // The API splits cache writes by TTL in `cache_creation`. Without that
        // split the TTL is unknown: count them as 5-minute writes (the cheaper
        // rate, so a lower bound) and flag the result as an estimate.
        let split = self
            .cache_creation
            .as_ref()
            .filter(|cc| cc.ephemeral_5m_input_tokens + cc.ephemeral_1h_input_tokens > 0);
        let (w5, w1h, ttl_unknown) = match split {
            Some(cc) => (
                cc.ephemeral_5m_input_tokens,
                cc.ephemeral_1h_input_tokens,
                false,
            ),
            None => (
                self.cache_creation_input_tokens,
                0,
                self.cache_creation_input_tokens > 0,
            ),
        };
        let speed = match self.speed.as_deref() {
            None | Some("standard") => Speed::Standard,
            Some("fast") => Speed::Fast,
            Some(_) => Speed::Unknown,
        };
        let geo = match self.inference_geo.as_deref() {
            None | Some("global" | "not_available" | "") => Geo::Global,
            Some("us") => Geo::Us,
            Some(_) => Geo::Unknown,
        };
        let tier = match self.service_tier.as_deref() {
            None | Some("standard") => Tier::Standard,
            Some("batch") => Tier::Batch,
            Some("priority") => Tier::Priority,
            Some(_) => Tier::Unknown,
        };
        Usage {
            input: self.input_tokens,
            output: self.output_tokens,
            cache_read: self.cache_read_input_tokens,
            cache_write_5m: w5,
            cache_write_1h: w1h,
            web_search_requests: self
                .server_tool_use
                .as_ref()
                .map_or(0, |s| s.web_search_requests),
            cache_ttl_unknown: ttl_unknown,
            speed,
            geo,
            tier,
        }
    }
}

/// One deduplicated API response.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub model: String,
    pub usage: Usage,
    pub sidechain: bool,
}

fn parse_line(line: &str) -> Option<(Option<String>, Entry)> {
    // Cheap pre-filter: most lines (tool results, attachments) have no usage.
    if !line.contains("\"usage\"") {
        return None;
    }
    let l: Line = serde_json::from_str(line).ok()?;
    let msg = l.message?;
    let raw = msg.usage?;
    let key = match (&msg.id, &l.request_id) {
        (Some(m), Some(r)) => Some(format!("{m}:{r}")),
        (Some(m), None) => Some(m.clone()),
        (None, Some(r)) => Some(r.clone()),
        (None, None) => l.uuid.clone(),
    };
    let entry = Entry {
        model: msg.model.unwrap_or_default(),
        usage: raw.to_usage(),
        sidechain: l.is_sidechain,
    };
    Some((key, entry))
}

/// Parse and deduplicate all usage entries from a reader, in first-seen order.
pub fn read_entries<R: BufRead>(reader: R) -> Vec<Entry> {
    let mut order: Vec<Entry> = Vec::new();
    let mut index: HashMap<String, usize> = HashMap::new();
    for line in reader.lines().map_while(Result::ok) {
        let Some((key, entry)) = parse_line(&line) else {
            continue;
        };
        match key {
            Some(k) => match index.get(&k) {
                Some(&i) => order[i] = entry,
                None => {
                    index.insert(k, order.len());
                    order.push(entry);
                }
            },
            None => order.push(entry),
        }
    }
    order
}

/// Aggregated, priced usage.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct CostSum {
    pub usd: f64,
    pub tokens: u64,
    /// Responses that could not be priced (unknown model, missing price).
    pub unpriced: u32,
    /// Responses priced as a lower bound or list-price estimate (unknown
    /// cache TTL, Priority Tier...).
    #[serde(default)]
    pub estimated: u32,
}

impl CostSum {
    pub fn add(&mut self, other: &CostSum) {
        self.usd += other.usd;
        self.tokens += other.tokens;
        self.unpriced += other.unpriced;
        self.estimated += other.estimated;
    }
}

/// Price every entry with its own model's rates and modifiers.
pub fn price_entries(entries: &[Entry], prices: &PriceTable) -> CostSum {
    let mut sum = CostSum::default();
    for e in entries {
        if e.usage.is_empty() {
            continue; // e.g. `<synthetic>` error messages
        }
        sum.tokens += e.usage.total_tokens();
        match prices.price(&e.model, &e.usage) {
            Some(Priced::Exact(usd)) => sum.usd += usd,
            Some(Priced::Estimate(usd)) => {
                sum.usd += usd;
                sum.estimated += 1;
            }
            None => sum.unpriced += 1,
        }
    }
    sum
}

pub fn file_cost(path: &Path, prices: &PriceTable) -> CostSum {
    match File::open(path) {
        Ok(f) => price_entries(&read_entries(BufReader::new(f)), prices),
        Err(_) => CostSum::default(),
    }
}

/// `<dir>/<session-id>.jsonl` -> `<dir>/<session-id>/subagents`.
pub fn subagents_dir(transcript: &Path) -> Option<PathBuf> {
    let stem = transcript.file_stem()?;
    Some(transcript.parent()?.join(stem).join("subagents"))
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
struct FileStamp {
    size: u64,
    mtime: u64,
    cost: CostSum,
}

/// Per-session cache so finished subagent transcripts are parsed once.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct SessionCache {
    /// `fetched_at` of the price table the costs were computed with.
    #[serde(default)]
    prices_at: u64,
    #[serde(default)]
    subagents: BTreeMap<String, FileStamp>,
    #[serde(default)]
    main: MainScan,
}

/// Incremental scan state of the main transcript (for burn-rate tokens).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
struct MainScan {
    offset: u64,
    tokens: u64,
    last_key: Option<String>,
}

fn session_cache_path(session_id: &str) -> PathBuf {
    paths::cache_dir()
        .join("sessions")
        .join(format!("{}.json", paths::sanitize(session_id)))
}

fn stamp(meta: &fs::Metadata) -> (u64, u64) {
    let mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_secs());
    (meta.len(), mtime)
}

/// What the transcripts add on top of Claude Code's own session cost.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct SessionUsage {
    pub subagents: CostSum,
    pub subagent_files: usize,
    /// Tokens of the main conversation so far (deduplicated).
    pub main_tokens: u64,
}

/// Cost of all subagent transcripts plus a running token count of the main
/// transcript. Files whose size and mtime are unchanged are served from the
/// per-session cache; only growing files are re-read.
pub fn session_usage(transcript: &Path, session_id: &str, prices: &PriceTable) -> SessionUsage {
    let cache_path = session_cache_path(session_id);
    let mut cache: SessionCache = fs::read(&cache_path)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default();
    if cache.prices_at != prices.fetched_at {
        cache.subagents.clear();
        cache.prices_at = prices.fetched_at;
    }
    let before_subs = cache.subagents.clone();
    let before_main = cache.main.clone();

    let mut out = SessionUsage::default();
    let mut seen = Vec::new();
    if let Some(dir) = subagents_dir(transcript) {
        let mut files: Vec<_> = fs::read_dir(&dir)
            .map(|rd| rd.flatten().collect())
            .unwrap_or_default();
        // Sorted so the floating-point sum is the same on every render.
        files.sort_by_key(|e| e.file_name());
        for ent in files {
            let name = ent.file_name().to_string_lossy().into_owned();
            if !(name.starts_with("agent-") && name.ends_with(".jsonl")) {
                continue;
            }
            let Ok(meta) = ent.metadata() else { continue };
            let (size, mtime) = stamp(&meta);
            let cost = match cache.subagents.get(&name) {
                Some(s) if s.size == size && s.mtime == mtime => s.cost,
                _ => {
                    let cost = file_cost(&ent.path(), prices);
                    cache
                        .subagents
                        .insert(name.clone(), FileStamp { size, mtime, cost });
                    cost
                }
            };
            out.subagents.add(&cost);
            out.subagent_files += 1;
            seen.push(name);
        }
    }
    cache.subagents.retain(|k, _| seen.contains(k));

    scan_main(transcript, &mut cache.main);
    out.main_tokens = cache.main.tokens;

    if cache.subagents != before_subs || cache.main != before_main {
        if let Ok(bytes) = serde_json::to_vec(&cache) {
            let _ = paths::write_atomic(&cache_path, &bytes);
        }
    }
    out
}

/// Read only the bytes appended since the last scan.
fn scan_main(path: &Path, state: &mut MainScan) {
    let Ok(mut f) = File::open(path) else { return };
    let Ok(len) = f.metadata().map(|m| m.len()) else {
        return;
    };
    if len < state.offset {
        *state = MainScan::default(); // truncated or replaced
    }
    if len == state.offset || f.seek(SeekFrom::Start(state.offset)).is_err() {
        return;
    }
    let mut buf = Vec::with_capacity((len - state.offset) as usize);
    if f.take(len - state.offset).read_to_end(&mut buf).is_err() {
        return;
    }
    // Only consume complete lines; a partially written last line is re-read next time.
    let Some(end) = buf.iter().rposition(|b| *b == b'\n') else {
        return;
    };
    let text = String::from_utf8_lossy(&buf[..=end]);
    for line in text.lines() {
        let Some((key, entry)) = parse_line(line) else {
            continue;
        };
        if key.is_some() && key == state.last_key {
            continue;
        }
        state.tokens += entry.usage.total_tokens();
        state.last_key = key;
    }
    state.offset += end as u64 + 1;
}

/// Context tokens from the newest main-chain usage line, reading only the
/// tail of the transcript. Fallback for Claude Code versions whose stdin has
/// no `context_window` block.
pub fn context_tokens_from_tail(path: &Path) -> Option<u64> {
    const TAIL: u64 = 256 * 1024;
    let mut f = File::open(path).ok()?;
    let len = f.metadata().ok()?.len();
    let start = len.saturating_sub(TAIL);
    f.seek(SeekFrom::Start(start)).ok()?;
    let mut buf = Vec::new();
    f.read_to_end(&mut buf).ok()?;
    let text = String::from_utf8_lossy(&buf);
    text.lines()
        .rev()
        .filter_map(parse_line)
        .find_map(|(_, e)| {
            (!e.sidechain && !e.usage.is_empty() && e.model != "<synthetic>")
                .then(|| e.usage.prompt_tokens())
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    const DUP: &str = r#"{"type":"assistant","requestId":"r1","message":{"id":"m1","model":"claude-opus-5-5","usage":{"input_tokens":2,"output_tokens":10,"cache_read_input_tokens":100,"cache_creation_input_tokens":50,"cache_creation":{"ephemeral_5m_input_tokens":0,"ephemeral_1h_input_tokens":50}}}}
{"type":"assistant","requestId":"r1","message":{"id":"m1","model":"claude-opus-5-5","usage":{"input_tokens":2,"output_tokens":10,"cache_read_input_tokens":100,"cache_creation_input_tokens":50,"cache_creation":{"ephemeral_5m_input_tokens":0,"ephemeral_1h_input_tokens":50}}}}
{"type":"user","message":{"role":"user","content":"hi"}}
{"type":"assistant","requestId":"r2","message":{"id":"m2","model":"claude-opus-5-5","usage":{"input_tokens":1,"output_tokens":5,"cache_creation_input_tokens":7}}}
"#;

    #[test]
    fn dedupes_split_content_blocks() {
        let e = read_entries(DUP.as_bytes());
        assert_eq!(e.len(), 2);
        assert_eq!(e[0].usage.cache_write_1h, 50);
        assert_eq!(e[0].usage.cache_write_5m, 0);
        // No TTL breakdown: counted as 5m writes and flagged.
        assert_eq!(e[1].usage.cache_write_5m, 7);
        assert!(e[1].usage.cache_ttl_unknown);
        assert!(!e[0].usage.cache_ttl_unknown);
    }

    #[test]
    fn scan_main_is_incremental_and_deduped() {
        let dir = crate::testutil::tempdir();
        let p = dir.path().join("s.jsonl");
        let (first, rest) = DUP.split_at(DUP.find('\n').unwrap() + 1);
        fs::write(&p, first).unwrap();
        let mut st = MainScan::default();
        scan_main(&p, &mut st);
        assert_eq!(st.tokens, 162);
        fs::write(&p, DUP).unwrap();
        let _ = rest;
        scan_main(&p, &mut st);
        assert_eq!(st.tokens, 162 + 13);
        assert_eq!(st.offset, DUP.len() as u64);
    }

    #[test]
    fn partial_last_line_is_not_consumed() {
        let dir = crate::testutil::tempdir();
        let p = dir.path().join("s.jsonl");
        let first_len = DUP.find('\n').unwrap() + 1;
        fs::write(&p, &DUP[..first_len + 20]).unwrap();
        let mut st = MainScan::default();
        scan_main(&p, &mut st);
        assert_eq!(st.offset, first_len as u64);
    }

    #[test]
    fn context_from_tail_skips_sidechain() {
        let dir = crate::testutil::tempdir();
        let p = dir.path().join("s.jsonl");
        let side = r#"{"isSidechain":true,"requestId":"r9","message":{"id":"m9","model":"claude-haiku-4-5","usage":{"input_tokens":1,"output_tokens":1}}}"#;
        fs::write(&p, format!("{DUP}{side}\n")).unwrap();
        assert_eq!(context_tokens_from_tail(&p), Some(8));
    }
}
