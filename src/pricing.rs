//! Model prices from litellm's `model_prices_and_context_window.json`.
//!
//! Only first-party Anthropic `claude-*` entries are kept. Field names are
//! litellm's own, so the cache file and the bundled snapshot are plain subsets
//! of the upstream JSON.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use crate::paths;

pub const LITELLM_URL: &str =
    "https://raw.githubusercontent.com/BerriAI/litellm/main/model_prices_and_context_window.json";

/// Requests whose total input (fresh + cache reads + cache writes) exceeds this
/// are billed at the long-context rates, when a model has them.
pub const LONG_CONTEXT_THRESHOLD: u64 = 200_000;

/// Anthropic's published cache multipliers, used only when litellm lacks the
/// explicit field for a model.
const CACHE_WRITE_5M_RATIO: f64 = 1.25;
const CACHE_WRITE_1H_RATIO: f64 = 2.0;
const CACHE_READ_RATIO: f64 = 0.1;

const SNAPSHOT: &str = include_str!("../data/pricing-snapshot.json");

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct SearchCost {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub search_context_size_medium: Option<f64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct ModelPrice {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_cost_per_token: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_cost_per_token: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_creation_input_token_cost: Option<f64>,
    /// litellm's name for the 1-hour cache write price.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_creation_input_token_cost_above_1hr: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_read_input_token_cost: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_cost_per_token_above_200k_tokens: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_cost_per_token_above_200k_tokens: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_creation_input_token_cost_above_200k_tokens: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_creation_input_token_cost_above_1hr_above_200k_tokens: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_read_input_token_cost_above_200k_tokens: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_input_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub search_context_cost_per_query: Option<SearchCost>,
}

/// `usage.speed` of a response.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Speed {
    #[default]
    Standard,
    /// Fast mode: premium per-model input/output prices.
    Fast,
    /// A speed this version does not know how to price.
    Unknown,
}

/// `usage.inference_geo` of a response.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Geo {
    /// Global routing (or not reported): standard prices.
    #[default]
    Global,
    /// US-only inference: a multiplier on every token category.
    Us,
    Unknown,
}

/// `usage.service_tier` of a response.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Tier {
    #[default]
    Standard,
    /// Batch API: 50% off every token category.
    Batch,
    /// Priority Tier is billed through a capacity commitment, not list
    /// prices, so a list-price figure is only an estimate.
    Priority,
    Unknown,
}

/// Token counts of one API response, split by billing bucket, plus the
/// request options that change its price.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Usage {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write_5m: u64,
    pub cache_write_1h: u64,
    pub web_search_requests: u64,
    /// Cache writes were logged without the 5m/1h split and are counted as
    /// 5-minute writes, which is a lower bound.
    pub cache_ttl_unknown: bool,
    pub speed: Speed,
    pub geo: Geo,
    pub tier: Tier,
}

/// USD price of one response.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Priced {
    Exact(f64),
    /// Best available figure, but a lower bound or list-price estimate.
    Estimate(f64),
}

impl Priced {
    pub fn usd(&self) -> f64 {
        match self {
            Priced::Exact(v) | Priced::Estimate(v) => *v,
        }
    }
}

/// Fast mode prices (USD per token) for one model, from Anthropic's pricing page.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq)]
pub struct FastPrice {
    pub input_cost_per_token: f64,
    pub output_cost_per_token: f64,
}

/// Batch API discount on every token category (Anthropic pricing page).
pub const BATCH_MULTIPLIER: f64 = 0.5;

/// Anthropic's multiplier for `inference_geo: "us"`, used until a refresh
/// reads the current value from the pricing page.
pub const DEFAULT_GEO_US_MULTIPLIER: f64 = 1.1;

fn default_geo_us() -> f64 {
    DEFAULT_GEO_US_MULTIPLIER
}

impl Usage {
    /// Tokens sent to the model: fresh input plus cache reads and writes.
    pub fn prompt_tokens(&self) -> u64 {
        self.input + self.cache_read + self.cache_write_5m + self.cache_write_1h
    }

    pub fn total_tokens(&self) -> u64 {
        self.prompt_tokens() + self.output
    }

    pub fn is_empty(&self) -> bool {
        self.total_tokens() == 0 && self.web_search_requests == 0
    }
}

/// Per-token USD rates that apply to one request.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rates {
    pub input: f64,
    pub output: f64,
    pub cache_write_5m: f64,
    pub cache_write_1h: f64,
    pub cache_read: f64,
}

impl ModelPrice {
    /// Rates for a request, picking the >200k tier when the prompt is that
    /// long and the model has a long-context price. `None` if the base
    /// input/output prices are missing.
    pub fn rates(&self, prompt_tokens: u64) -> Option<Rates> {
        let base_in = self.input_cost_per_token?;
        let base_out = self.output_cost_per_token?;
        let long = prompt_tokens > LONG_CONTEXT_THRESHOLD
            && self.input_cost_per_token_above_200k_tokens.is_some();
        let (input, output, cw5, cw1h, cr) = if long {
            (
                self.input_cost_per_token_above_200k_tokens
                    .unwrap_or(base_in),
                self.output_cost_per_token_above_200k_tokens
                    .unwrap_or(base_out),
                self.cache_creation_input_token_cost_above_200k_tokens,
                self.cache_creation_input_token_cost_above_1hr_above_200k_tokens,
                self.cache_read_input_token_cost_above_200k_tokens,
            )
        } else {
            (
                base_in,
                base_out,
                self.cache_creation_input_token_cost,
                self.cache_creation_input_token_cost_above_1hr,
                self.cache_read_input_token_cost,
            )
        };
        Some(Rates {
            input,
            output,
            cache_write_5m: cw5.unwrap_or(input * CACHE_WRITE_5M_RATIO),
            cache_write_1h: cw1h.unwrap_or(input * CACHE_WRITE_1H_RATIO),
            cache_read: cr.unwrap_or(input * CACHE_READ_RATIO),
        })
    }

    /// USD cost of one API response at standard speed, global routing and
    /// standard tier. `None` when the model has no usable price, or when web
    /// searches were made and litellm has no search price.
    pub fn cost(&self, u: &Usage) -> Option<f64> {
        let r = self.rates(u.prompt_tokens())?;
        Some(token_cost(&r, u) + self.search_cost(u)?)
    }

    fn search_cost(&self, u: &Usage) -> Option<f64> {
        if u.web_search_requests == 0 {
            return Some(0.0);
        }
        let per_query = self
            .search_context_cost_per_query
            .as_ref()?
            .search_context_size_medium?;
        Some(u.web_search_requests as f64 * per_query)
    }
}

fn token_cost(r: &Rates, u: &Usage) -> f64 {
    u.input as f64 * r.input
        + u.output as f64 * r.output
        + u.cache_write_5m as f64 * r.cache_write_5m
        + u.cache_write_1h as f64 * r.cache_write_1h
        + u.cache_read as f64 * r.cache_read
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// Downloaded copy in the cache dir.
    Cache,
    /// Snapshot compiled into the binary.
    Bundled,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PriceFile {
    /// Unix seconds when the data was fetched from litellm.
    pub fetched_at: u64,
    pub models: BTreeMap<String, ModelPrice>,
    /// Fast mode prices by model id (not in litellm; from Anthropic's pricing page).
    #[serde(default)]
    pub fast_mode: BTreeMap<String, FastPrice>,
    /// Multiplier for US-only inference (`inference_geo: "us"`).
    #[serde(default = "default_geo_us")]
    pub inference_geo_us_multiplier: f64,
}

impl Default for PriceFile {
    fn default() -> Self {
        PriceFile {
            fetched_at: 0,
            models: BTreeMap::new(),
            fast_mode: BTreeMap::new(),
            inference_geo_us_multiplier: DEFAULT_GEO_US_MULTIPLIER,
        }
    }
}

#[derive(Debug, Clone)]
pub struct PriceTable {
    pub fetched_at: u64,
    pub source: Source,
    pub models: BTreeMap<String, ModelPrice>,
    pub fast_mode: BTreeMap<String, FastPrice>,
    pub inference_geo_us_multiplier: f64,
}

/// How a model id was resolved against the table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Match {
    Exact(String),
    /// Matched after normalising (provider prefix, date suffix, `[1m]` tag...).
    Normalized(String),
    /// Matched by family prefix, e.g. `claude-opus-5-5-thinking` -> `claude-opus-5-5`.
    Family(String),
}

impl Match {
    pub fn key(&self) -> &str {
        match self {
            Match::Exact(k) | Match::Normalized(k) | Match::Family(k) => k,
        }
    }
}

pub fn cache_path() -> PathBuf {
    paths::cache_dir().join("pricing.json")
}

impl PriceTable {
    pub fn bundled() -> PriceTable {
        let file: PriceFile =
            serde_json::from_str(SNAPSHOT).expect("bundled pricing snapshot is valid JSON");
        Self::from_file(file, Source::Bundled)
    }

    pub fn from_file(file: PriceFile, source: Source) -> PriceTable {
        PriceTable {
            fetched_at: file.fetched_at,
            source,
            models: file.models,
            fast_mode: file.fast_mode,
            inference_geo_us_multiplier: file.inference_geo_us_multiplier,
        }
    }

    /// Load the downloaded cache, falling back to the bundled snapshot when the
    /// cache is missing, corrupt, or older than the snapshot.
    pub fn load() -> PriceTable {
        let bundled = Self::bundled();
        match fs::read(cache_path())
            .ok()
            .and_then(|b| serde_json::from_slice::<PriceFile>(&b).ok())
        {
            Some(mut f) if !f.models.is_empty() && f.fetched_at >= bundled.fetched_at => {
                if f.fast_mode.is_empty() {
                    f.fast_mode = bundled.fast_mode;
                }
                Self::from_file(f, Source::Cache)
            }
            _ => bundled,
        }
    }

    /// Fast mode prices for a resolved table key (dated keys share the
    /// undated model's entry).
    pub fn fast_price(&self, key: &str) -> Option<FastPrice> {
        self.fast_mode
            .get(key)
            .or_else(|| self.fast_mode.get(&normalize_model_id(key)))
            .copied()
    }

    /// Price one response with every modifier it reports: fast mode, US-only
    /// inference, batch/priority tier. `None` if it cannot be priced at all
    /// (unknown model, unknown speed, missing fast-mode or search price).
    pub fn price(&self, model_id: &str, u: &Usage) -> Option<Priced> {
        let (key, p) = self.lookup(model_id)?;
        let mut estimate = u.cache_ttl_unknown;
        let r = match u.speed {
            Speed::Standard => p.rates(u.prompt_tokens())?,
            Speed::Fast => {
                // Fast mode has one price across the whole context window;
                // cache multipliers apply on top of the fast input price.
                let f = self.fast_price(key)?;
                let base = p.rates(0)?;
                let k = f.input_cost_per_token / base.input;
                Rates {
                    input: f.input_cost_per_token,
                    output: f.output_cost_per_token,
                    cache_write_5m: base.cache_write_5m * k,
                    cache_write_1h: base.cache_write_1h * k,
                    cache_read: base.cache_read * k,
                }
            }
            Speed::Unknown => return None,
        };
        let mut multiplier = 1.0;
        match u.geo {
            Geo::Global => {}
            Geo::Us => multiplier *= self.inference_geo_us_multiplier,
            Geo::Unknown => estimate = true,
        }
        match u.tier {
            Tier::Standard => {}
            Tier::Batch => multiplier *= BATCH_MULTIPLIER,
            Tier::Priority | Tier::Unknown => estimate = true,
        }
        let usd = token_cost(&r, u) * multiplier + p.search_cost(u)?;
        Some(if estimate {
            Priced::Estimate(usd)
        } else {
            Priced::Exact(usd)
        })
    }

    pub fn age_secs(&self, now: u64) -> u64 {
        now.saturating_sub(self.fetched_at)
    }

    pub fn get(&self, key: &str) -> Option<&ModelPrice> {
        self.models.get(key)
    }

    /// Resolve a model id. Exact id first, then the normalised id, then the
    /// longest table key that is a `-`-segment prefix of the normalised id, as
    /// long as the leftover does not start with a version number. So
    /// `claude-opus-5-5-thinking` resolves to `claude-opus-5-5`, but an unknown
    /// `claude-opus-5-6` does not silently become `claude-opus-5`.
    pub fn resolve(&self, model_id: &str) -> Option<Match> {
        if self.models.contains_key(model_id) {
            return Some(Match::Exact(model_id.to_string()));
        }
        let norm = normalize_model_id(model_id);
        if norm.is_empty() {
            return None;
        }
        if self.models.contains_key(&norm) {
            return Some(Match::Normalized(norm));
        }
        let segments: Vec<&str> = norm.split('-').collect();
        // Require at least `claude-<family>-<major>` so a bare family never matches.
        for n in (3..segments.len()).rev() {
            let rest = segments[n];
            if rest.chars().next().is_some_and(|c| c.is_ascii_digit()) {
                continue;
            }
            let prefix = segments[..n].join("-");
            if self.models.contains_key(&prefix) {
                return Some(Match::Family(prefix));
            }
        }
        None
    }

    pub fn lookup(&self, model_id: &str) -> Option<(&str, &ModelPrice)> {
        let m = self.resolve(model_id)?;
        self.models
            .get_key_value(m.key())
            .map(|(k, v)| (k.as_str(), v))
    }
}

/// Lower-case, strip provider prefixes (`anthropic/`, `us.anthropic.`,
/// `vertex_ai/`...), a trailing `[1m]`-style tag, Bedrock `-v1:0` and Vertex
/// `@20250101` suffixes, and a trailing `-YYYYMMDD` date.
pub fn normalize_model_id(id: &str) -> String {
    let mut s = id.trim().to_ascii_lowercase();
    if let Some(i) = s.find('[') {
        s.truncate(i);
    }
    if let Some(i) = s.find("claude-") {
        s.drain(..i);
    }
    if let Some(i) = s.find('@') {
        s.truncate(i);
    }
    if let Some(i) = s.find(':') {
        s.truncate(i);
    }
    if let Some(stripped) = strip_suffix_version(&s) {
        s = stripped;
    }
    if let Some(i) = s.rfind('-') {
        let tail = &s[i + 1..];
        if tail.len() == 8 && tail.chars().all(|c| c.is_ascii_digit()) {
            s.truncate(i);
        }
    }
    s.trim_end_matches('-').to_string()
}

/// Strip a Bedrock-style `-v1` / `-v2` suffix.
fn strip_suffix_version(s: &str) -> Option<String> {
    let i = s.rfind("-v")?;
    let tail = &s[i + 2..];
    (!tail.is_empty() && tail.chars().all(|c| c.is_ascii_digit())).then(|| s[..i].to_string())
}

/// Reduce litellm's full JSON to first-party Anthropic Claude entries.
pub fn extract_from_litellm(body: &str) -> Result<BTreeMap<String, ModelPrice>, String> {
    let value: serde_json::Value =
        serde_json::from_str(body).map_err(|e| format!("invalid litellm JSON: {e}"))?;
    let obj = value.as_object().ok_or("litellm JSON is not an object")?;
    let mut out = BTreeMap::new();
    for (key, entry) in obj {
        if !key.starts_with("claude-") {
            continue;
        }
        if entry.get("litellm_provider").and_then(|p| p.as_str()) != Some("anthropic") {
            continue;
        }
        if let Ok(price) = serde_json::from_value::<ModelPrice>(entry.clone()) {
            if price.input_cost_per_token.is_some() && price.output_cost_per_token.is_some() {
                out.insert(key.clone(), price);
            }
        }
    }
    if out.is_empty() {
        return Err("no Anthropic Claude models found in litellm JSON".into());
    }
    Ok(out)
}

/// Anthropic's pricing page in Markdown. Source of the fast mode prices and
/// the data residency multiplier, which litellm does not carry.
pub const ANTHROPIC_PRICING_URL: &str =
    "https://platform.claude.com/docs/en/about-claude/pricing.md";

/// Price modifiers read from Anthropic's pricing page.
#[derive(Debug, Clone, PartialEq)]
pub struct PageModifiers {
    pub fast_mode: BTreeMap<String, FastPrice>,
    pub inference_geo_us_multiplier: f64,
}

/// `$8 / MTok` -> USD per token.
fn parse_mtok(cell: &str) -> Option<f64> {
    let n: f64 = cell
        .trim()
        .strip_prefix('$')?
        .split('/')
        .next()?
        .trim()
        .replace(',', "")
        .parse()
        .ok()?;
    (n > 0.0 && n < 10_000.0).then_some(n / 1e6)
}

/// `Claude Opus 5.5` -> `claude-opus-5-5`.
fn model_id_from_name(name: &str) -> Option<String> {
    let id = name.trim().to_ascii_lowercase().replace(['.', ' '], "-");
    let valid = id.starts_with("claude-")
        && id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    valid.then_some(id)
}

/// Parse the "Fast mode pricing" table and the "Data residency pricing"
/// multiplier. Strict: any row that does not parse fails the whole page, so
/// a layout change leaves the last good data in place instead of half-reading it.
pub fn parse_anthropic_pricing(md: &str) -> Result<PageModifiers, String> {
    let section = md
        .split("### Fast mode pricing")
        .nth(1)
        .ok_or("no \"Fast mode pricing\" section")?;
    let section = section.split("\n#").next().unwrap_or(section);
    let mut fast_mode = BTreeMap::new();
    let rows = section
        .lines()
        .map(str::trim)
        .skip_while(|l| !l.starts_with('|'))
        .take_while(|l| l.starts_with('|'))
        .skip(2); // header + separator
    for row in rows {
        let cells: Vec<&str> = row.trim_matches('|').split('|').collect();
        let [names, input, output] = cells[..] else {
            return Err(format!("unexpected fast mode row: {row}"));
        };
        let price = FastPrice {
            input_cost_per_token: parse_mtok(input).ok_or_else(|| format!("bad price: {row}"))?,
            output_cost_per_token: parse_mtok(output).ok_or_else(|| format!("bad price: {row}"))?,
        };
        for name in names.split('/') {
            let id = model_id_from_name(name).ok_or_else(|| format!("bad model: {row}"))?;
            fast_mode.insert(id, price);
        }
    }
    if fast_mode.is_empty() {
        return Err("fast mode table is empty".into());
    }
    let residency = md
        .split("### Data residency pricing")
        .nth(1)
        .ok_or("no \"Data residency pricing\" section")?;
    let before = residency
        .split("x multiplier")
        .next()
        .filter(|s| s.len() < residency.len())
        .ok_or("no data residency multiplier")?;
    let geo: f64 = before
        .rsplit(|c: char| !(c.is_ascii_digit() || c == '.'))
        .next()
        .and_then(|n| n.parse().ok())
        .ok_or("unreadable data residency multiplier")?;
    if !(1.0..=2.0).contains(&geo) {
        return Err(format!("implausible data residency multiplier {geo}"));
    }
    Ok(PageModifiers {
        fast_mode,
        inference_geo_us_multiplier: geo,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table() -> PriceTable {
        let mut models = BTreeMap::new();
        for k in [
            "claude-opus-5",
            "claude-opus-5-5",
            "claude-sonnet-5",
            "claude-haiku-4-5",
        ] {
            models.insert(
                k.to_string(),
                ModelPrice {
                    input_cost_per_token: Some(1e-6),
                    output_cost_per_token: Some(5e-6),
                    ..Default::default()
                },
            );
        }
        PriceTable::from_file(
            PriceFile {
                models,
                ..Default::default()
            },
            Source::Bundled,
        )
    }

    const PAGE: &str = "## Model pricing\n| x | y |\n\n### Data residency pricing\n\n\
        For Claude 4.6 and later models, specifying US-only inference through the `inference_geo` \
        parameter incurs a 1.1x multiplier on all token pricing categories.\n\n\
        ### Fast mode pricing\n\nIntro text.\n\n\
        | Model                           | Input      | Output     |\n\
        | ------------------------------- | ---------- | ---------- |\n\
        | Claude Opus 5.5                 | $8 / MTok  | $40 / MTok |\n\
        | Claude Opus 5 / Claude Opus 4.8 | $10 / MTok | $50 / MTok |\n\n\
        Fast mode is not available on Claude Opus 4.7.\n\n### Batch processing\n\n\
        | Model | Batch input | Batch output |\n|--|--|--|\n| Claude Opus 5.5 | $2 / MTok | $10 / MTok |\n";

    #[test]
    fn parses_anthropic_pricing_page() {
        let m = parse_anthropic_pricing(PAGE).unwrap();
        assert_eq!(m.inference_geo_us_multiplier, 1.1);
        assert_eq!(m.fast_mode.len(), 3);
        assert_eq!(m.fast_mode["claude-opus-5-5"].input_cost_per_token, 8e-6);
        assert_eq!(m.fast_mode["claude-opus-4-8"].output_cost_per_token, 50e-6);
        assert!(!m.fast_mode.contains_key("claude-opus-5-5-batch"));
    }

    #[test]
    fn rejects_changed_page_layout() {
        assert!(parse_anthropic_pricing("nothing here").is_err());
        let broken = PAGE.replace("$8 / MTok", "eight dollars");
        assert!(parse_anthropic_pricing(&broken).is_err());
        let no_geo = PAGE.replace("1.1x multiplier", "a multiplier");
        assert!(parse_anthropic_pricing(&no_geo).is_err());
    }

    #[test]
    fn modifiers_change_the_price() {
        let mut t = table();
        t.models.insert(
            "claude-opus-5-5".into(),
            ModelPrice {
                input_cost_per_token: Some(4e-6),
                output_cost_per_token: Some(20e-6),
                cache_creation_input_token_cost: Some(5e-6),
                cache_creation_input_token_cost_above_1hr: Some(8e-6),
                cache_read_input_token_cost: Some(0.2e-6),
                ..Default::default()
            },
        );
        t.fast_mode.insert(
            "claude-opus-5-5".into(),
            FastPrice {
                input_cost_per_token: 8e-6,
                output_cost_per_token: 40e-6,
            },
        );
        let u = Usage {
            input: 1000,
            output: 1000,
            cache_read: 1000,
            cache_write_5m: 1000,
            cache_write_1h: 1000,
            ..Default::default()
        };
        let close = |p: Option<Priced>, want: Priced| {
            let p = p.unwrap();
            assert!((p.usd() - want.usd()).abs() < 1e-12, "{p:?} != {want:?}");
            assert_eq!(std::mem::discriminant(&p), std::mem::discriminant(&want));
        };
        // Standard: 1000 * (4 + 20 + 0.2 + 5 + 8) / 1e6
        close(t.price("claude-opus-5-5", &u), Priced::Exact(0.0372));
        // Fast: every input-side rate doubles with the input price, output 40.
        let fast = Usage {
            speed: Speed::Fast,
            ..u
        };
        close(t.price("claude-opus-5-5", &fast), Priced::Exact(0.0744));
        // US-only inference: 1.1x on everything.
        let us = Usage { geo: Geo::Us, ..u };
        close(t.price("claude-opus-5-5", &us), Priced::Exact(0.0372 * 1.1));
        // Fast + US stack.
        let both = Usage {
            geo: Geo::Us,
            ..fast
        };
        close(
            t.price("claude-opus-5-5", &both),
            Priced::Exact(0.0744 * 1.1),
        );
        // Batch: half price.
        let batch = Usage {
            tier: Tier::Batch,
            ..u
        };
        close(t.price("claude-opus-5-5", &batch), Priced::Exact(0.0186));
        // Priority tier / unknown TTL / unknown geo: list price, flagged.
        let prio = Usage {
            tier: Tier::Priority,
            ..u
        };
        close(t.price("claude-opus-5-5", &prio), Priced::Estimate(0.0372));
        let ttl = Usage {
            cache_ttl_unknown: true,
            ..u
        };
        close(t.price("claude-opus-5-5", &ttl), Priced::Estimate(0.0372));
        // Fast mode on a model without a fast price, or an unknown speed: unpriced.
        assert_eq!(
            t.price(
                "claude-sonnet-5",
                &Usage {
                    speed: Speed::Fast,
                    ..u
                }
            ),
            None
        );
        assert_eq!(
            t.price(
                "claude-opus-5-5",
                &Usage {
                    speed: Speed::Unknown,
                    ..u
                }
            ),
            None
        );
        // Dated ids share the undated fast price.
        assert!(t.fast_price("claude-opus-5-5-20270101").is_some());
    }

    #[test]
    fn normalizes_ids() {
        assert_eq!(
            normalize_model_id("claude-haiku-4-5-20251001"),
            "claude-haiku-4-5"
        );
        assert_eq!(normalize_model_id("claude-opus-5-5[1m]"), "claude-opus-5-5");
        assert_eq!(
            normalize_model_id("us.anthropic.claude-sonnet-5-20260101-v1:0"),
            "claude-sonnet-5"
        );
        assert_eq!(
            normalize_model_id("anthropic/claude-opus-5"),
            "claude-opus-5"
        );
        assert_eq!(
            normalize_model_id("vertex_ai/claude-opus-5@20260101"),
            "claude-opus-5"
        );
        assert_eq!(normalize_model_id("Claude-Opus-5-5"), "claude-opus-5-5");
        assert_eq!(normalize_model_id("<synthetic>"), "<synthetic>");
    }

    #[test]
    fn resolves_models() {
        let t = table();
        assert_eq!(
            t.resolve("claude-opus-5-5"),
            Some(Match::Exact("claude-opus-5-5".into()))
        );
        assert_eq!(
            t.resolve("claude-haiku-4-5-20251001"),
            Some(Match::Normalized("claude-haiku-4-5".into()))
        );
        assert_eq!(
            t.resolve("claude-opus-5-5-thinking"),
            Some(Match::Family("claude-opus-5-5".into()))
        );
        // Unknown point release must not silently borrow another version's price.
        assert_eq!(t.resolve("claude-opus-5-6"), None);
        assert_eq!(t.resolve("claude-sonnet-6"), None);
        assert_eq!(t.resolve("gpt-5"), None);
        assert_eq!(t.resolve("<synthetic>"), None);
        assert_eq!(t.resolve(""), None);
    }

    #[test]
    fn long_context_tier_applies_above_200k() {
        let p = ModelPrice {
            input_cost_per_token: Some(3e-6),
            output_cost_per_token: Some(15e-6),
            cache_creation_input_token_cost: Some(3.75e-6),
            cache_creation_input_token_cost_above_1hr: Some(6e-6),
            cache_read_input_token_cost: Some(0.3e-6),
            input_cost_per_token_above_200k_tokens: Some(6e-6),
            output_cost_per_token_above_200k_tokens: Some(22.5e-6),
            cache_creation_input_token_cost_above_200k_tokens: Some(7.5e-6),
            cache_creation_input_token_cost_above_1hr_above_200k_tokens: Some(12e-6),
            cache_read_input_token_cost_above_200k_tokens: Some(0.6e-6),
            ..Default::default()
        };
        let short = p.rates(200_000).unwrap();
        assert_eq!(short.input, 3e-6);
        assert_eq!(short.cache_write_1h, 6e-6);
        let long = p.rates(200_001).unwrap();
        assert_eq!(long.input, 6e-6);
        assert_eq!(long.output, 22.5e-6);
        assert_eq!(long.cache_write_5m, 7.5e-6);
        assert_eq!(long.cache_write_1h, 12e-6);
        assert_eq!(long.cache_read, 0.6e-6);
    }

    #[test]
    fn derives_missing_cache_prices_from_input() {
        let p = ModelPrice {
            input_cost_per_token: Some(4e-6),
            output_cost_per_token: Some(20e-6),
            ..Default::default()
        };
        let r = p.rates(10).unwrap();
        assert!((r.cache_write_5m - 5e-6).abs() < 1e-15);
        assert!((r.cache_write_1h - 8e-6).abs() < 1e-15);
        assert!((r.cache_read - 0.4e-6).abs() < 1e-15);
    }

    #[test]
    fn web_search_without_price_is_unpriced() {
        let p = ModelPrice {
            input_cost_per_token: Some(1e-6),
            output_cost_per_token: Some(1e-6),
            ..Default::default()
        };
        let u = Usage {
            web_search_requests: 1,
            ..Default::default()
        };
        assert_eq!(p.cost(&u), None);
    }

    #[test]
    fn bundled_snapshot_parses() {
        let t = PriceTable::bundled();
        assert!(t.fetched_at > 0);
        assert!(!t.models.is_empty());
        assert!(t.models.keys().all(|k| k.starts_with("claude-")));
    }
}
