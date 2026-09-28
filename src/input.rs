//! The JSON Claude Code writes to the status line command's stdin.
//! Every field is optional so older and newer Claude Code versions both work.

use serde::Deserialize;

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct Input {
    pub session_id: Option<String>,
    pub transcript_path: Option<String>,
    pub cwd: Option<String>,
    pub model: Model,
    pub workspace: Workspace,
    pub cost: Cost,
    pub context_window: Option<ContextWindow>,
    pub effort: Option<Effort>,
    pub fast_mode: Option<bool>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct Model {
    pub id: Option<String>,
    pub display_name: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct Workspace {
    pub current_dir: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct Cost {
    pub total_cost_usd: Option<f64>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct ContextWindow {
    pub total_input_tokens: Option<u64>,
    pub context_window_size: Option<u64>,
    pub used_percentage: Option<f64>,
    pub current_usage: Option<CurrentUsage>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct CurrentUsage {
    pub input_tokens: u64,
    pub cache_creation_input_tokens: u64,
    pub cache_read_input_tokens: u64,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct Effort {
    pub level: Option<String>,
}

impl Input {
    pub fn parse(text: &str) -> Result<Input, String> {
        if text.trim().is_empty() {
            return Ok(Input::default());
        }
        serde_json::from_str(text).map_err(|e| format!("invalid status line JSON: {e}"))
    }

    pub fn dir(&self) -> Option<&str> {
        self.workspace
            .current_dir
            .as_deref()
            .or(self.cwd.as_deref())
    }

    /// Tokens currently in context, from the stdin fields if present.
    /// `None` before the first API response or after `/compact`.
    pub fn context_tokens(&self) -> Option<u64> {
        let cw = self.context_window.as_ref()?;
        if let Some(u) = &cw.current_usage {
            return Some(
                u.input_tokens + u.cache_creation_input_tokens + u.cache_read_input_tokens,
            );
        }
        cw.total_input_tokens.filter(|t| *t > 0)
    }

    pub fn context_size(&self) -> Option<u64> {
        self.context_window
            .as_ref()?
            .context_window_size
            .filter(|s| *s > 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_documented_example() {
        let i = Input::parse(include_str!("../tests/fixtures/stdin-full.json")).unwrap();
        assert_eq!(i.model.id.as_deref(), Some("claude-opus-5-5"));
        assert_eq!(i.context_tokens(), Some(15500));
        assert_eq!(i.context_size(), Some(200000));
        assert_eq!(i.effort.unwrap().level.as_deref(), Some("high"));
    }

    #[test]
    fn tolerates_minimal_and_null_fields() {
        let i = Input::parse(r#"{"model":{"id":"x"},"context_window":{"current_usage":null,"used_percentage":null}}"#)
            .unwrap();
        assert_eq!(i.context_tokens(), None);
        assert!(Input::parse("").is_ok());
        assert!(Input::parse("not json").is_err());
    }
}
