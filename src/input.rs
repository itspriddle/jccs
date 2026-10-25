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
    pub context_window_size: Option<u64>,
    pub used_percentage: Option<f64>,
    pub remaining_percentage: Option<f64>,
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
