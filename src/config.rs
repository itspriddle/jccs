use std::{
    env, fs, io,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use serde::Deserialize;

/// User configuration loaded from `~/.config/jccs/config.toml`.
#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub context: ContextConfig,
}

/// Context window accounting and status thresholds.
#[derive(Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ContextConfig {
    /// Tokens Claude Code reserves that are not reported in the JSON payload.
    pub reserved_tokens: u64,
    /// Used percentage at which the context status becomes "caution".
    pub caution_at: f64,
    /// Used percentage at which the context status becomes "danger".
    pub danger_at: f64,
}

impl Default for ContextConfig {
    fn default() -> Self {
        Self {
            reserved_tokens: 20_000,
            caution_at: 76.0,
            danger_at: 86.0,
        }
    }
}

/// Returns `$XDG_CONFIG_HOME/jccs/config.toml`, falling back to
/// `$HOME/.config/jccs/config.toml`.
pub fn config_path() -> Option<PathBuf> {
    let base = env::var_os("XDG_CONFIG_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))?;

    Some(base.join("jccs").join("config.toml"))
}

/// Loads the config file, returning defaults when it does not exist.
pub fn load() -> Result<Config> {
    match config_path() {
        Some(path) => load_from(&path),
        None => Ok(Config::default()),
    }
}

fn load_from(path: &Path) -> Result<Config> {
    let contents = match fs::read_to_string(path) {
        Ok(contents) => contents,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Config::default()),
        Err(err) => return Err(err).with_context(|| format!("reading {}", path.display())),
    };

    parse(&contents).with_context(|| format!("parsing {}", path.display()))
}

fn parse(contents: &str) -> Result<Config> {
    Ok(toml::from_str(contents)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_empty_config() {
        assert!(parse("").is_ok());
    }

    #[test]
    fn rejects_unknown_keys() {
        assert!(parse("bogus = true").is_err());
        assert!(parse("[context]\nbogus = true").is_err());
    }

    #[test]
    fn parses_context_table() -> Result<()> {
        let config = parse("[context]\nreserved_tokens = 10000\ndanger_at = 90")?;
        assert_eq!(config.context.reserved_tokens, 10_000);
        assert!((config.context.caution_at - 76.0).abs() < f64::EPSILON);
        assert!((config.context.danger_at - 90.0).abs() < f64::EPSILON);
        Ok(())
    }

    #[test]
    fn missing_file_uses_defaults() {
        assert!(load_from(Path::new("/nonexistent/jccs/config.toml")).is_ok());
    }
}
