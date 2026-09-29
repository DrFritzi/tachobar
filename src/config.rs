//! User configuration (`config.toml` in the platform config dir).

use std::fs;

use crate::currency::{Position, Style};
use crate::minitoml::{self, Value};
use crate::paths;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Segment {
    Dir,
    Model,
    Effort,
    Price,
    Context,
    Cost,
    Burn,
}

impl Segment {
    fn parse(name: &str) -> Option<Segment> {
        DEFAULT_SEGMENTS.into_iter().find(|s| s.name() == name)
    }

    pub fn name(self) -> &'static str {
        match self {
            Segment::Dir => "dir",
            Segment::Model => "model",
            Segment::Effort => "effort",
            Segment::Price => "price",
            Segment::Context => "context",
            Segment::Cost => "cost",
            Segment::Burn => "burn",
        }
    }
}

pub const DEFAULT_SEGMENTS: [Segment; 7] = [
    Segment::Dir,
    Segment::Model,
    Segment::Effort,
    Segment::Price,
    Segment::Context,
    Segment::Cost,
    Segment::Burn,
];

#[derive(Debug, Clone)]
pub struct Config {
    /// ISO 4217 code, e.g. "USD", "EUR", "GBP", "CHF", "JPY".
    pub currency: String,
    /// Override the currency's usual number of decimals.
    pub decimals: Option<usize>,
    /// Override the currency symbol, e.g. "EUR" instead of "€".
    pub symbol: Option<String>,
    /// "before" or "after" the number.
    pub symbol_position: Option<String>,
    /// Decimal separator, "." or ",".
    pub decimal_separator: Option<char>,
    /// Segments to show, in order.
    pub segments: Vec<Segment>,
    /// Disable ANSI colors (NO_COLOR in the environment also does).
    pub no_color: bool,
    /// Download pricing and FX data once a day in the background.
    pub auto_refresh: bool,
    pub thresholds: Thresholds,
}

#[derive(Debug, Clone)]
pub struct Thresholds {
    /// Output price in USD per 1M tokens at which the model (and cost) color
    /// turns yellow, orange, red.
    pub price_per_mtok: [f64; 3],
    /// Context usage in percent at which the context color turns yellow, orange, red.
    pub context_pct: [f64; 3],
    /// Days after which pricing/FX data is flagged as stale.
    pub stale_days: u64,
}

impl Default for Thresholds {
    fn default() -> Self {
        Thresholds {
            price_per_mtok: [10.0, 25.0, 50.0],
            context_pct: [50.0, 80.0, 95.0],
            stale_days: 7,
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Config {
            currency: "USD".into(),
            decimals: None,
            symbol: None,
            symbol_position: None,
            decimal_separator: None,
            segments: DEFAULT_SEGMENTS.to_vec(),
            no_color: false,
            auto_refresh: true,
            thresholds: Thresholds::default(),
        }
    }
}

impl Config {
    /// Load the config file. A missing file gives defaults; a broken one gives
    /// defaults plus the error, so the status line can still render and say so.
    pub fn load() -> (Config, Option<String>) {
        let Some(path) = paths::config_file() else {
            return (Config::default(), None);
        };
        match fs::read_to_string(&path) {
            Ok(text) => match Self::parse(&text) {
                Ok(c) => (c, None),
                Err(e) => (Config::default(), Some(e)),
            },
            Err(_) => (Config::default(), None),
        }
    }

    pub fn parse(text: &str) -> Result<Config, String> {
        let bad = |e: String| format!("config.toml: {e}");
        let mut tables = minitoml::parse(text).map_err(bad)?;
        let mut root = tables.remove("").unwrap_or_default();
        let mut th = tables.remove("thresholds").unwrap_or_default();
        if let Some(name) = tables.keys().next() {
            return Err(bad(format!("unknown table [{name}]")));
        }
        let mut c = Config::default();

        if let Some(v) = root.remove("currency") {
            let code = string(&v, "currency").map_err(bad)?;
            c.currency = code.trim().to_ascii_uppercase();
        }
        if c.currency.len() != 3 || !c.currency.chars().all(|ch| ch.is_ascii_alphabetic()) {
            return Err(bad(format!(
                "currency must be a 3-letter ISO code, got {:?}",
                c.currency
            )));
        }
        if let Some(v) = root.remove("decimals") {
            c.decimals = Some(
                usize::try_from(int(&v, "decimals").map_err(bad)?)
                    .map_err(|_| bad("decimals must not be negative".into()))?,
            );
        }
        if let Some(v) = root.remove("symbol") {
            c.symbol = Some(string(&v, "symbol").map_err(bad)?);
        }
        if let Some(v) = root.remove("symbol_position") {
            let p = string(&v, "symbol_position").map_err(bad)?;
            if p != "before" && p != "after" {
                return Err(bad(format!(
                    "symbol_position must be \"before\" or \"after\", got {p:?}"
                )));
            }
            c.symbol_position = Some(p);
        }
        if let Some(v) = root.remove("decimal_separator") {
            let s = string(&v, "decimal_separator").map_err(bad)?;
            let mut chars = s.chars();
            c.decimal_separator = match (chars.next(), chars.next()) {
                (Some(ch), None) => Some(ch),
                _ => return Err(bad("decimal_separator must be a single character".into())),
            };
        }
        if let Some(v) = root.remove("segments") {
            let Value::Array(items) = &v else {
                return Err(bad("segments must be an array of names".into()));
            };
            c.segments = items
                .iter()
                .map(|i| {
                    let name = string(i, "segments")?;
                    Segment::parse(&name).ok_or_else(|| format!("unknown segment {name:?}"))
                })
                .collect::<Result<_, _>>()
                .map_err(bad)?;
        }
        if let Some(v) = root.remove("no_color") {
            c.no_color = boolean(&v, "no_color").map_err(bad)?;
        }
        if let Some(v) = root.remove("auto_refresh") {
            c.auto_refresh = boolean(&v, "auto_refresh").map_err(bad)?;
        }
        if let Some(key) = root.keys().next() {
            return Err(bad(format!("unknown key {key}")));
        }

        if let Some(v) = th.remove("price_per_mtok") {
            c.thresholds.price_per_mtok = triple(&v, "price_per_mtok").map_err(bad)?;
        }
        if let Some(v) = th.remove("context_pct") {
            c.thresholds.context_pct = triple(&v, "context_pct").map_err(bad)?;
        }
        if let Some(v) = th.remove("stale_days") {
            c.thresholds.stale_days = u64::try_from(int(&v, "stale_days").map_err(bad)?)
                .map_err(|_| bad("stale_days must not be negative".into()))?;
        }
        if let Some(key) = th.keys().next() {
            return Err(bad(format!("unknown key thresholds.{key}")));
        }
        Ok(c)
    }

    pub fn money_style(&self) -> Style {
        let mut s = Style::for_code(&self.currency);
        if let Some(d) = self.decimals {
            s.decimals = d.min(6);
        }
        if let Some(sym) = &self.symbol {
            s.symbol = sym.clone();
        }
        match self.symbol_position.as_deref() {
            Some("after") => s.position = Position::After,
            Some("before") => s.position = Position::Before,
            _ => {}
        }
        if let Some(sep) = self.decimal_separator {
            s.decimal_separator = sep;
        }
        s
    }
}

fn string(v: &Value, key: &str) -> Result<String, String> {
    match v {
        Value::Str(s) => Ok(s.clone()),
        _ => Err(format!("{key} must be a string")),
    }
}

fn int(v: &Value, key: &str) -> Result<i64, String> {
    match v {
        Value::Int(n) => Ok(*n),
        _ => Err(format!("{key} must be a whole number")),
    }
}

fn boolean(v: &Value, key: &str) -> Result<bool, String> {
    match v {
        Value::Bool(b) => Ok(*b),
        _ => Err(format!("{key} must be true or false")),
    }
}

fn triple(v: &Value, key: &str) -> Result<[f64; 3], String> {
    let err = || format!("{key} must be three numbers, e.g. [10.0, 25.0, 50.0]");
    let Value::Array(items) = v else {
        return Err(err());
    };
    let nums: Vec<f64> = items
        .iter()
        .map(|i| match i {
            Value::Float(f) => Some(*f),
            Value::Int(n) => Some(*n as f64),
            _ => None,
        })
        .collect::<Option<_>>()
        .ok_or_else(err)?;
    nums.try_into().map_err(|_| err())
}

pub const DEFAULT_TOML: &str = r#"# tachobar configuration
# Location: see `tachobar doctor`. All keys are optional.

# ISO 4217 currency code. Rates: ECB reference rates via frankfurter.app,
# refreshed daily. Examples: "USD", "EUR", "GBP", "CHF", "JPY", "SEK".
currency = "USD"

# Formatting overrides (defaults depend on the currency).
# decimals = 2
# symbol = "€"
# symbol_position = "before"   # or "after"
# decimal_separator = "."      # or ","

# Segments to show, in order.
# Available: dir, model, effort, price, context, cost, burn
segments = ["dir", "model", "effort", "price", "context", "cost", "burn"]

# Disable colors (the NO_COLOR environment variable also works).
no_color = false

# Refresh pricing and exchange rates once a day in the background.
auto_refresh = true

[thresholds]
# Output price (USD per 1M tokens) where the model color turns yellow/orange/red.
price_per_mtok = [10.0, 25.0, 50.0]
# Context usage (%) where the context color turns yellow/orange/red.
context_pct = [50.0, 80.0, 95.0]
# Flag pricing / FX data older than this many days as stale.
stale_days = 7
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_toml_matches_defaults() {
        let c = Config::parse(DEFAULT_TOML).unwrap();
        let d = Config::default();
        assert_eq!(c.currency, d.currency);
        assert_eq!(c.segments, d.segments);
        assert_eq!(c.thresholds.price_per_mtok, d.thresholds.price_per_mtok);
        assert_eq!(c.auto_refresh, d.auto_refresh);
    }

    #[test]
    fn partial_config() {
        let c = Config::parse("currency = \"eur\"\nsegments = [\"cost\", \"model\"]\n").unwrap();
        assert_eq!(c.currency, "EUR");
        assert_eq!(c.segments, vec![Segment::Cost, Segment::Model]);
        assert_eq!(c.thresholds.stale_days, 7);
    }

    #[test]
    fn rejects_bad_values() {
        assert!(Config::parse("currency = \"euro\"").is_err());
        assert!(Config::parse("segments = [\"nope\"]").is_err());
        assert!(Config::parse("colour = true").is_err());
        assert!(Config::parse("[nope]").is_err());
        assert!(Config::parse("[thresholds]\nstale_days = -1").is_err());
        assert!(Config::parse("[thresholds]\nprice_per_mtok = [1, 2]").is_err());
    }

    #[test]
    fn style_overrides() {
        let c = Config::parse(
            "currency = \"EUR\"\nsymbol_position = \"after\"\ndecimal_separator = \",\"\ndecimals = 3\n",
        )
        .unwrap();
        let s = c.money_style();
        assert_eq!(s.format(1.5), "1,500€");
    }
}
