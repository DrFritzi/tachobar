//! Turns computed numbers into the colored status line.

use crate::burn::Rate;
use crate::config::{Config, Segment};
use crate::currency::Style;

const GRAY: u8 = 244;
const PURPLE: u8 = 141;
const GREEN: u8 = 40;
const YELLOW: u8 = 220;
const ORANGE: u8 = 208;
const RED: u8 = 196;

/// Everything the status line shows, already computed. Money is in USD.
#[derive(Debug, Clone, Default)]
pub struct Report {
    pub dir: Option<String>,
    pub model_name: String,
    /// USD per 1M tokens (input, output); `None` for an unpriced model.
    pub price_per_mtok: Option<(f64, f64)>,
    pub effort: Option<String>,
    pub context: Option<(u64, u64)>,
    pub cost_usd: Option<f64>,
    /// Some responses in the total could not be priced exactly.
    pub cost_partial: bool,
    pub burn: Option<Rate>,
    /// USD -> display currency. `None` shows USD.
    pub fx_rate: Option<f64>,
    /// Short warnings shown dimmed at the end (stale data, unknown model...).
    pub notes: Vec<String>,
}

pub struct Painter {
    pub color: bool,
}

impl Painter {
    fn paint(&self, color: u8, text: &str) -> String {
        if self.color {
            format!("\x1b[38;5;{color}m{text}\x1b[0m")
        } else {
            text.to_string()
        }
    }
}

fn tier(value: f64, thresholds: &[f64; 3]) -> u8 {
    if value < thresholds[0] {
        GREEN
    } else if value < thresholds[1] {
        YELLOW
    } else if value < thresholds[2] {
        ORANGE
    } else {
        RED
    }
}

fn effort_color(level: &str) -> u8 {
    match level {
        "low" => GREEN,
        "medium" => YELLOW,
        "high" => ORANGE,
        "xhigh" | "max" => RED,
        _ => GRAY,
    }
}

/// `950`, `1.2k`, `142k`, `1M`, `1.5M`.
pub fn format_count(n: f64) -> String {
    fn one_decimal(x: f64, unit: &str) -> String {
        let s = if x >= 100.0 {
            format!("{x:.0}")
        } else {
            format!("{x:.1}")
        };
        format!("{}{unit}", s.trim_end_matches(".0"))
    }
    if n >= 999_950.0 {
        one_decimal(n / 1_000_000.0, "M")
    } else if n >= 1000.0 {
        one_decimal(n / 1000.0, "k")
    } else {
        format!("{n:.0}")
    }
}

pub fn segments(r: &Report, cfg: &Config, painter: &Painter) -> Vec<String> {
    let (style, rate) = match r.fx_rate {
        Some(rate) => (cfg.money_style(), rate),
        None => (Style::for_code("USD"), 1.0),
    };
    let t = &cfg.thresholds;
    let model_color = r
        .price_per_mtok
        .map_or(GRAY, |(_, out)| tier(out, &t.price_per_mtok));
    let mut out = Vec::new();
    for seg in &cfg.segments {
        match seg {
            Segment::Dir => {
                if let Some(d) = &r.dir {
                    out.push(painter.paint(PURPLE, d));
                }
            }
            Segment::Model => {
                if !r.model_name.is_empty() {
                    out.push(painter.paint(model_color, &r.model_name));
                }
            }
            Segment::Effort => {
                if let Some(e) = &r.effort {
                    out.push(painter.paint(effort_color(e), e));
                }
            }
            Segment::Price => {
                let text = match r.price_per_mtok {
                    Some((i, o)) => {
                        format!(
                            "{}/{}/M",
                            style.format_compact(i * rate),
                            style.format_compact(o * rate)
                        )
                    }
                    None => "?/?/M".to_string(),
                };
                out.push(painter.paint(GRAY, &text));
            }
            Segment::Context => {
                if let Some((used, size)) = r.context.filter(|(_, s)| *s > 0) {
                    let pct = used as f64 * 100.0 / size as f64;
                    let text = format!(
                        "{}/{} ctx ({:.0}%)",
                        format_count(used as f64),
                        format_count(size as f64),
                        pct
                    );
                    out.push(painter.paint(tier(pct, &t.context_pct), &text));
                }
            }
            Segment::Cost => {
                if let Some(usd) = r.cost_usd {
                    let mut text = style.format(usd * rate);
                    if r.cost_partial {
                        text.push('+');
                    }
                    out.push(painter.paint(model_color, &text));
                }
            }
            Segment::Burn => {
                if let Some(b) = r.burn {
                    let text = format!(
                        "{} tok/s {}/h",
                        format_count(b.tokens_per_sec.round()),
                        style.format(b.usd_per_hour * rate)
                    );
                    out.push(painter.paint(GRAY, &text));
                }
            }
        }
    }
    for n in &r.notes {
        out.push(painter.paint(ORANGE, &format!("[{n}]")));
    }
    out
}

/// Visible width of a string with ANSI color codes.
pub fn visible_width(s: &str) -> usize {
    let mut n = 0;
    let mut in_esc = false;
    for c in s.chars() {
        if in_esc {
            if c.is_ascii_alphabetic() {
                in_esc = false;
            }
        } else if c == '\x1b' {
            in_esc = true;
        } else {
            n += 1;
        }
    }
    n
}

/// Join segments with spaces, breaking onto a new line (LF) before a segment
/// that would overflow `width`.
pub fn wrap(parts: &[String], width: usize) -> String {
    let mut lines: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut cur_w = 0;
    for p in parts {
        let w = visible_width(p);
        if cur.is_empty() {
            cur.push_str(p);
            cur_w = w;
        } else if cur_w + 1 + w > width {
            lines.push(std::mem::take(&mut cur));
            cur.push_str(p);
            cur_w = w;
        } else {
            cur.push(' ');
            cur.push_str(p);
            cur_w += 1 + w;
        }
    }
    if !cur.is_empty() {
        lines.push(cur);
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts() {
        assert_eq!(format_count(950.0), "950");
        assert_eq!(format_count(1234.0), "1.2k");
        assert_eq!(format_count(142_000.0), "142k");
        assert_eq!(format_count(1_000_000.0), "1M");
        assert_eq!(format_count(1_500_000.0), "1.5M");
        assert_eq!(format_count(999_999.0), "1M");
    }

    #[test]
    fn wraps_on_visible_width() {
        let p = Painter { color: true };
        let parts = vec![p.paint(1, "aaaa"), p.paint(2, "bbbb"), p.paint(3, "cc")];
        assert_eq!(visible_width(&parts[0]), 4);
        let out = wrap(&parts, 9);
        assert_eq!(out.lines().count(), 2);
        assert!(!out.contains('\r'));
        assert_eq!(wrap(&parts, 12).lines().count(), 1);
    }

    #[test]
    fn full_line_matches_reference_layout() {
        let r = Report {
            dir: Some("myproject".into()),
            model_name: "Opus 5.5".into(),
            price_per_mtok: Some((4.0, 20.0)),
            effort: Some("high".into()),
            context: Some((142_000, 1_000_000)),
            cost_usd: Some(10.0),
            cost_partial: false,
            burn: Some(Rate {
                tokens_per_sec: 1234.0,
                usd_per_hour: 8.0,
            }),
            fx_rate: Some(0.9),
            notes: vec![],
        };
        let cfg = Config::parse("currency = \"EUR\"").unwrap();
        let line = wrap(&segments(&r, &cfg, &Painter { color: false }), 200);
        assert_eq!(
            line,
            "myproject Opus 5.5 high €3.6/€18/M 142k/1M ctx (14%) €9.00 1.2k tok/s €7.20/h"
        );
    }

    #[test]
    fn unknown_model_shows_question_marks() {
        let r = Report {
            model_name: "Mystery".into(),
            notes: vec!["unpriced model: x".into()],
            ..Default::default()
        };
        let line = wrap(
            &segments(&r, &Config::default(), &Painter { color: false }),
            200,
        );
        assert_eq!(line, "Mystery ?/?/M [unpriced model: x]");
    }
}
