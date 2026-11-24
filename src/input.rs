//! Statusline JSON piped to stdin by Claude Code. See `docs/schema.json`.
//!
//! Every field is optional and unknown fields are ignored so changes to the
//! Claude Code payload never break parsing.

// Field names mirror the JSON payload, and not every field is rendered yet.
#![allow(
    dead_code,
    clippy::struct_field_names,
    reason = "fields mirror the Claude Code statusline JSON"
)]

use std::io::Read;

use anyhow::{Context, Result};
use serde::Deserialize;

#[derive(Debug, Default, Deserialize)]
pub struct StatusInput {
    pub cwd: Option<String>,
    pub session_id: Option<String>,
    pub version: Option<String>,
    pub model: Option<Model>,
    pub workspace: Option<Workspace>,
    pub cost: Option<Cost>,
    pub context_window: Option<ContextWindow>,
    pub effort: Option<Effort>,
    pub thinking: Option<Thinking>,
    pub rate_limits: Option<RateLimits>,
    pub pr: Option<PullRequest>,
    pub worktree: Option<Worktree>,
}

#[derive(Debug, Default, Deserialize)]
pub struct Model {
    pub id: Option<String>,
    pub display_name: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
pub struct Workspace {
    pub current_dir: Option<String>,
    pub project_dir: Option<String>,
    pub repo: Option<Repo>,
}

#[derive(Debug, Default, Deserialize)]
pub struct Repo {
    pub host: Option<String>,
    pub owner: Option<String>,
    pub name: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
pub struct Cost {
    pub total_cost_usd: Option<f64>,
    pub total_duration_ms: Option<u64>,
    pub total_lines_added: Option<u64>,
    pub total_lines_removed: Option<u64>,
}

#[derive(Debug, Default, Deserialize)]
pub struct ContextWindow {
    pub total_input_tokens: Option<u64>,
    pub total_output_tokens: Option<u64>,
    pub context_window_size: Option<u64>,
    pub used_percentage: Option<f64>,
    pub remaining_percentage: Option<f64>,
    pub current_usage: Option<CurrentUsage>,
}

#[derive(Debug, Default, Deserialize)]
pub struct CurrentUsage {
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub cache_creation_input_tokens: Option<u64>,
    pub cache_read_input_tokens: Option<u64>,
}

#[derive(Debug, Default, Deserialize)]
pub struct Effort {
    pub level: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
pub struct Thinking {
    pub enabled: Option<bool>,
}

#[derive(Debug, Default, Deserialize)]
pub struct RateLimits {
    pub five_hour: Option<RateLimit>,
    pub seven_day: Option<RateLimit>,
}

#[derive(Debug, Default, Deserialize)]
pub struct RateLimit {
    pub used_percentage: Option<f64>,
    /// Unix timestamp (seconds).
    pub resets_at: Option<i64>,
}

#[derive(Debug, Default, Deserialize)]
pub struct PullRequest {
    pub number: Option<u64>,
    pub url: Option<String>,
    pub review_state: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
pub struct Worktree {
    pub name: Option<String>,
    pub path: Option<String>,
    pub branch: Option<String>,
}

/// Parses statusline JSON from `reader`.
pub fn read(reader: impl Read) -> Result<StatusInput> {
    serde_json::from_reader(reader).context("parsing statusline JSON from stdin")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_sample_schema() -> Result<()> {
        let input = read(include_str!("../docs/schema.json").as_bytes())?;
        assert_eq!(
            input.model.and_then(|m| m.display_name).as_deref(),
            Some("Opus")
        );
        assert_eq!(input.effort.and_then(|e| e.level).as_deref(), Some("high"));
        assert_eq!(input.thinking.and_then(|t| t.enabled), Some(true));

        let usage = input
            .context_window
            .and_then(|c| c.current_usage)
            .unwrap_or_default();
        assert_eq!(usage.cache_creation_input_tokens, Some(5000));

        let limits = input.rate_limits.unwrap_or_default();
        assert_eq!(
            limits.five_hour.and_then(|r| r.resets_at),
            Some(1_738_425_600)
        );
        assert_eq!(limits.seven_day.and_then(|r| r.used_percentage), Some(41.2));

        let repo = input.workspace.and_then(|w| w.repo).unwrap_or_default();
        assert_eq!(repo.owner.as_deref(), Some("anthropics"));
        assert_eq!(repo.name.as_deref(), Some("claude-code"));

        assert_eq!(input.pr.and_then(|p| p.number), Some(1234));
        Ok(())
    }

    #[test]
    fn missing_seven_day_is_none() -> Result<()> {
        let json = r#"{"rate_limits":{"five_hour":{"used_percentage":1,"resets_at":0}}}"#;
        let limits = read(json.as_bytes())?.rate_limits.unwrap_or_default();
        assert!(limits.five_hour.is_some());
        assert!(limits.seven_day.is_none());
        Ok(())
    }

    #[test]
    fn parses_empty_object() {
        assert!(read("{}".as_bytes()).is_ok());
    }

    #[test]
    fn rejects_invalid_json() {
        assert!(read("nope".as_bytes()).is_err());
    }
}
