//! Exchange rates (ECB reference rates via frankfurter.app) and money formatting.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use crate::paths;

pub const FRANKFURTER_URL: &str = "https://api.frankfurter.app/latest?from=USD";

const SNAPSHOT: &str = include_str!("../data/fx-snapshot.json");

/// Rates are always stored with USD as the base: 1 USD = `rates[code]`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FxFile {
    pub fetched_at: u64,
    /// ECB publication date of the rates (YYYY-MM-DD).
    pub date: String,
    pub rates: BTreeMap<String, f64>,
}

#[derive(Debug, Clone)]
pub struct FxTable {
    pub fetched_at: u64,
    pub date: String,
    pub bundled: bool,
    pub rates: BTreeMap<String, f64>,
}

pub fn cache_path() -> PathBuf {
    paths::cache_dir().join("fx.json")
}

impl FxTable {
    pub fn bundled() -> FxTable {
        let f: FxFile = serde_json::from_str(SNAPSHOT).expect("bundled FX snapshot is valid JSON");
        FxTable {
            fetched_at: f.fetched_at,
            date: f.date,
            bundled: true,
            rates: f.rates,
        }
    }

    pub fn load() -> FxTable {
        let bundled = Self::bundled();
        match fs::read(cache_path())
            .ok()
            .and_then(|b| serde_json::from_slice::<FxFile>(&b).ok())
        {
            Some(f) if !f.rates.is_empty() && f.fetched_at >= bundled.fetched_at => FxTable {
                fetched_at: f.fetched_at,
                date: f.date,
                bundled: false,
                rates: f.rates,
            },
            _ => bundled,
        }
    }

    /// Multiplier from USD to `code`. USD itself needs no data.
    pub fn rate(&self, code: &str) -> Option<f64> {
        if code.eq_ignore_ascii_case("USD") {
            return Some(1.0);
        }
        self.rates
            .get(&code.to_ascii_uppercase())
            .copied()
            .filter(|r| *r > 0.0)
    }

    pub fn age_secs(&self, now: u64) -> u64 {
        now.saturating_sub(self.fetched_at)
    }
}

/// Parse frankfurter's `{"base":"USD","date":..,"rates":{..}}` response.
pub fn parse_frankfurter(body: &str, fetched_at: u64) -> Result<FxFile, String> {
    #[derive(Deserialize)]
    struct Resp {
        base: String,
        date: String,
        rates: BTreeMap<String, f64>,
    }
    let r: Resp =
        serde_json::from_str(body).map_err(|e| format!("invalid frankfurter JSON: {e}"))?;
    if r.base != "USD" {
        return Err(format!("expected USD base, got {}", r.base));
    }
    if r.rates.is_empty() {
        return Err("frankfurter returned no rates".into());
    }
    Ok(FxFile {
        fetched_at,
        date: r.date,
        rates: r.rates,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Position {
    Before,
    After,
}

/// How to print amounts in one currency.
#[derive(Debug, Clone, PartialEq)]
pub struct Style {
    pub symbol: String,
    pub position: Position,
    /// Space between symbol and number.
    pub spaced: bool,
    pub decimals: usize,
    pub decimal_separator: char,
}

impl Style {
    /// Conventional symbol and placement for an ISO 4217 code.
    pub fn for_code(code: &str) -> Style {
        use Position::*;
        let code = code.to_ascii_uppercase();
        let (symbol, position, spaced, decimals): (&str, Position, bool, usize) =
            match code.as_str() {
                "USD" => ("$", Before, false, 2),
                "EUR" => ("€", Before, false, 2),
                "GBP" => ("£", Before, false, 2),
                "JPY" => ("¥", Before, false, 0),
                "CNY" => ("¥", Before, false, 2),
                "KRW" => ("₩", Before, false, 0),
                "INR" => ("₹", Before, false, 2),
                "CHF" => ("CHF", Before, true, 2),
                "CAD" => ("CA$", Before, false, 2),
                "AUD" => ("A$", Before, false, 2),
                "NZD" => ("NZ$", Before, false, 2),
                "HKD" => ("HK$", Before, false, 2),
                "SGD" => ("S$", Before, false, 2),
                "MXN" => ("MX$", Before, false, 2),
                "BRL" => ("R$", Before, false, 2),
                "ZAR" => ("R", Before, false, 2),
                "TRY" => ("₺", Before, false, 2),
                "ILS" => ("₪", Before, false, 2),
                "THB" => ("฿", Before, false, 2),
                "PHP" => ("₱", Before, false, 2),
                "MYR" => ("RM", Before, false, 2),
                "IDR" => ("Rp", Before, false, 0),
                "SEK" | "NOK" | "DKK" => ("kr", After, true, 2),
                "ISK" => ("kr", After, true, 0),
                "PLN" => ("zł", After, true, 2),
                "CZK" => ("Kč", After, true, 2),
                "HUF" => ("Ft", After, true, 0),
                "RON" => ("lei", After, true, 2),
                _ => ("", Before, true, 2),
            };
        Style {
            symbol: if symbol.is_empty() {
                code
            } else {
                symbol.to_string()
            },
            position,
            spaced,
            decimals,
            decimal_separator: '.',
        }
    }

    /// Format with a fixed number of decimals.
    pub fn format(&self, amount: f64) -> String {
        self.wrap(&self.number(amount, self.decimals, false))
    }

    /// Format a unit price: up to `decimals` places, trailing zeros trimmed
    /// (`4.6`, `23`), which keeps the price segment short.
    pub fn format_compact(&self, amount: f64) -> String {
        self.wrap(&self.number(amount, self.decimals, true))
    }

    fn number(&self, amount: f64, decimals: usize, trim: bool) -> String {
        let mut s = format!("{:.*}", decimals, amount);
        if s.starts_with('-') && s[1..].chars().all(|c| c == '0' || c == '.') {
            s.remove(0);
        }
        if trim && s.contains('.') {
            s = s.trim_end_matches('0').trim_end_matches('.').to_string();
        }
        if self.decimal_separator != '.' {
            s = s.replace('.', &self.decimal_separator.to_string());
        }
        s
    }

    fn wrap(&self, n: &str) -> String {
        let sp = if self.spaced { " " } else { "" };
        match self.position {
            Position::Before => format!("{}{sp}{n}", self.symbol),
            Position::After => format!("{n}{sp}{}", self.symbol),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_common_currencies() {
        assert_eq!(Style::for_code("EUR").format(12.345), "€12.35");
        assert_eq!(Style::for_code("usd").format(0.5), "$0.50");
        assert_eq!(Style::for_code("JPY").format(1234.6), "¥1235");
        assert_eq!(Style::for_code("CHF").format(3.0), "CHF 3.00");
        assert_eq!(Style::for_code("SEK").format(3.0), "3.00 kr");
        assert_eq!(Style::for_code("XYZ").format(1.0), "XYZ 1.00");
    }

    #[test]
    fn compact_trims_zeros() {
        let eur = Style::for_code("EUR");
        assert_eq!(eur.format_compact(4.6), "€4.6");
        assert_eq!(eur.format_compact(23.0), "€23");
        assert_eq!(eur.format_compact(0.126), "€0.13");
    }

    #[test]
    fn custom_separator() {
        let mut s = Style::for_code("EUR");
        s.decimal_separator = ',';
        s.position = Position::After;
        s.spaced = true;
        assert_eq!(s.format(12.3), "12,30 €");
    }

    #[test]
    fn no_negative_zero() {
        assert_eq!(Style::for_code("USD").format(-0.001), "$0.00");
    }

    #[test]
    fn parses_frankfurter() {
        let f = parse_frankfurter(
            r#"{"amount":1.0,"base":"USD","date":"2026-09-25","rates":{"EUR":0.877}}"#,
            7,
        )
        .unwrap();
        assert_eq!(f.rates["EUR"], 0.877);
        assert_eq!(f.date, "2026-09-25");
        assert!(parse_frankfurter(r#"{"base":"EUR","date":"x","rates":{"USD":1.1}}"#, 0).is_err());
    }

    #[test]
    fn rate_lookup() {
        let t = FxTable::bundled();
        assert_eq!(t.rate("USD"), Some(1.0));
        assert!(t.rate("eur").is_some());
        assert_eq!(t.rate("NOPE"), None);
    }
}
