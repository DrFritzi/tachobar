//! User configuration (`config.toml` in the platform config dir).

use serde::{Deserialize, Serialize};
use std::fs;

use crate::currency::{Position, Style};
use crate::paths;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Segment {
    Dir,
    Model,
    Effort,
    Price,
    Context,
    Cost,
    Burn,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
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

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
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
        let mut c: Config =
            toml::from_str(text).map_err(|e| format!("config.toml: {}", e.message()))?;
        c.currency = c.currency.trim().to_ascii_uppercase();
        if c.currency.len() != 3 || !c.currency.chars().all(|ch| ch.is_ascii_alphabetic()) {
            return Err(format!(
                "config.toml: currency must be a 3-letter ISO code, got {:?}",
                c.currency
            ));
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
