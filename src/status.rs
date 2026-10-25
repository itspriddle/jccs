//! Values derived from the raw statusline input, ready for display.

use crate::{
    config::{Config, ContextConfig},
    input::{self, StatusInput},
};

const FIVE_HOURS_SECS: i64 = 5 * 60 * 60;
const SEVEN_DAYS_SECS: i64 = 7 * 24 * 60 * 60;

/// Facts from outside the JSON payload, injected so derivation stays pure.
#[derive(Debug, Default)]
pub struct Env {
    /// Current Unix timestamp (seconds).
    pub now: i64,
    pub git_branch: Option<String>,
}

#[derive(Debug, Default)]
pub struct Status {
    pub model: Option<String>,
    pub effort: Option<String>,
    pub thinking: Option<bool>,
    pub cost_usd: Option<f64>,
    pub lines_added: Option<u64>,
    pub lines_removed: Option<u64>,
    pub five_hour: Option<RateWindow>,
    pub seven_day: Option<RateWindow>,
    pub context: Option<ContextStatus>,
    pub git: GitStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Good,
    Caution,
    Danger,
}

impl Level {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Good => "good",
            Self::Caution => "caution",
            Self::Danger => "danger",
        }
    }
}

#[derive(Debug, PartialEq)]
pub struct RateWindow {
    pub used_pct: f64,
    pub remaining_pct: f64,
    /// Unix timestamps (seconds).
    pub starts_at: Option<i64>,
    pub resets_at: Option<i64>,
    /// Seconds until reset, never negative.
    pub resets_in: Option<i64>,
}

#[derive(Debug, PartialEq)]
pub struct ContextStatus {
    pub size: u64,
    pub reserved_tokens: u64,
    pub used_tokens: u64,
    /// Percentages relative to `size - reserved_tokens`.
    pub used_pct: f64,
    pub remaining_pct: f64,
    pub level: Level,
    /// Percentages as reported by Claude Code, without the reserve.
    pub reported_used_pct: Option<f64>,
    pub reported_remaining_pct: Option<f64>,
}

#[derive(Debug, Default)]
pub struct GitStatus {
    pub branch: Option<String>,
    pub owner: Option<String>,
    pub repo: Option<String>,
    pub pr_number: Option<u64>,
    pub pr_url: Option<String>,
}

impl Status {
    pub fn from_input(input: &StatusInput, env: &Env, config: &Config) -> Self {
        let cost = input.cost.as_ref();
        let limits = input.rate_limits.as_ref();
        let repo = input.workspace.as_ref().and_then(|w| w.repo.as_ref());
        let pr = input.pr.as_ref();

        Self {
            model: input.model.as_ref().and_then(|m| m.display_name.clone()),
            effort: input.effort.as_ref().and_then(|e| e.level.clone()),
            thinking: input.thinking.as_ref().and_then(|t| t.enabled),
            cost_usd: cost.and_then(|c| c.total_cost_usd),
            lines_added: cost.and_then(|c| c.total_lines_added),
            lines_removed: cost.and_then(|c| c.total_lines_removed),
            five_hour: limits
                .and_then(|l| l.five_hour.as_ref())
                .and_then(|r| RateWindow::new(r, FIVE_HOURS_SECS, env.now)),
            seven_day: limits
                .and_then(|l| l.seven_day.as_ref())
                .and_then(|r| RateWindow::new(r, SEVEN_DAYS_SECS, env.now)),
            context: input
                .context_window
                .as_ref()
                .and_then(|c| ContextStatus::new(c, &config.context)),
            git: GitStatus {
                branch: env
                    .git_branch
                    .clone()
                    .or_else(|| input.worktree.as_ref().and_then(|w| w.branch.clone())),
                owner: repo.and_then(|r| r.owner.clone()),
                repo: repo.and_then(|r| r.name.clone()),
                pr_number: pr.and_then(|p| p.number),
                pr_url: pr.and_then(|p| p.url.clone()),
            },
        }
    }
}

impl RateWindow {
    fn new(limit: &input::RateLimit, window_secs: i64, now: i64) -> Option<Self> {
        let used_pct = limit.used_percentage?;
        let resets_at = limit.resets_at;

        Some(Self {
            used_pct,
            remaining_pct: (100.0 - used_pct).max(0.0),
            starts_at: resets_at.map(|r| r - window_secs),
            resets_at,
            resets_in: resets_at.map(|r| (r - now).max(0)),
        })
    }
}

impl ContextStatus {
    #[expect(
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "token counts are far below f64's exact integer range"
    )]
    fn new(window: &input::ContextWindow, config: &ContextConfig) -> Option<Self> {
        let size = window.context_window_size.filter(|&s| s > 0)?;

        let used_tokens = match &window.current_usage {
            Some(usage) => {
                usage.input_tokens.unwrap_or(0)
                    + usage.cache_creation_input_tokens.unwrap_or(0)
                    + usage.cache_read_input_tokens.unwrap_or(0)
            }
            // `current_usage` is null before the first API call of a session;
            // fall back to the reported percentage, then to nothing used.
            None => window
                .used_percentage
                .map_or(0, |pct| (pct / 100.0 * size as f64).round() as u64),
        };

        let effective = size.saturating_sub(config.reserved_tokens).max(1);
        let used_pct = (used_tokens as f64 / effective as f64 * 100.0).clamp(0.0, 100.0);

        Some(Self {
            size,
            reserved_tokens: config.reserved_tokens,
            used_tokens,
            used_pct,
            remaining_pct: 100.0 - used_pct,
            level: level_for(used_pct, config),
            reported_used_pct: window.used_percentage,
            reported_remaining_pct: window.remaining_percentage,
        })
    }
}

fn level_for(used_pct: f64, config: &ContextConfig) -> Level {
    if used_pct >= config.danger_at {
        Level::Danger
    } else if used_pct >= config.caution_at {
        Level::Caution
    } else {
        Level::Good
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::{ContextWindow, CurrentUsage, RateLimit};

    fn rate(used: f64, resets_at: i64) -> RateLimit {
        RateLimit {
            used_percentage: Some(used),
            resets_at: Some(resets_at),
        }
    }

    fn window(size: u64, input_tokens: u64) -> ContextWindow {
        ContextWindow {
            context_window_size: Some(size),
            current_usage: Some(CurrentUsage {
                input_tokens: Some(input_tokens),
                ..CurrentUsage::default()
            }),
            ..ContextWindow::default()
        }
    }

    fn no_reserve() -> ContextConfig {
        ContextConfig {
            reserved_tokens: 0,
            ..ContextConfig::default()
        }
    }

    #[test]
    fn rate_window_derives_start_and_countdown() {
        let w = RateWindow::new(&rate(23.5, 20_000), FIVE_HOURS_SECS, 15_000);
        assert_eq!(
            w,
            Some(RateWindow {
                used_pct: 23.5,
                remaining_pct: 76.5,
                starts_at: Some(2_000),
                resets_at: Some(20_000),
                resets_in: Some(5_000),
            })
        );
    }

    #[test]
    fn rate_window_countdown_saturates_at_zero() {
        let w = RateWindow::new(&rate(10.0, 100), SEVEN_DAYS_SECS, 500);
        assert_eq!(w.and_then(|w| w.resets_in), Some(0));
    }

    #[test]
    fn rate_window_requires_used_percentage() {
        let limit = RateLimit {
            used_percentage: None,
            resets_at: Some(1),
        };
        assert!(RateWindow::new(&limit, FIVE_HOURS_SECS, 0).is_none());
    }

    #[test]
    fn context_level_boundaries() {
        let cases = [
            (75, Level::Good),
            (76, Level::Caution),
            (85, Level::Caution),
            (86, Level::Danger),
        ];
        for (used, expected) in cases {
            let ctx = ContextStatus::new(&window(100, used), &no_reserve());
            assert_eq!(ctx.map(|c| c.level), Some(expected), "used = {used}");
        }
    }

    #[test]
    fn context_reserve_shrinks_window() {
        let ctx = ContextStatus::new(&window(200_000, 90_000), &ContextConfig::default());
        assert_eq!(ctx.as_ref().map(|c| c.used_tokens), Some(90_000));
        assert!(ctx.is_some_and(|c| (c.used_pct - 50.0).abs() < f64::EPSILON));
    }

    #[test]
    fn context_sums_cache_tokens() {
        let mut w = window(100_000, 1_000);
        w.current_usage = Some(CurrentUsage {
            input_tokens: Some(1_000),
            output_tokens: Some(9_999),
            cache_creation_input_tokens: Some(2_000),
            cache_read_input_tokens: Some(3_000),
        });
        let ctx = ContextStatus::new(&w, &no_reserve());
        assert_eq!(ctx.map(|c| c.used_tokens), Some(6_000));
    }

    #[test]
    fn context_falls_back_to_reported_percentage() {
        let w = ContextWindow {
            context_window_size: Some(200_000),
            used_percentage: Some(10.0),
            ..ContextWindow::default()
        };
        let ctx = ContextStatus::new(&w, &ContextConfig::default());
        assert_eq!(ctx.map(|c| c.used_tokens), Some(20_000));
    }

    #[test]
    fn context_without_usage_counts_as_empty() {
        let w = ContextWindow {
            context_window_size: Some(200_000),
            ..ContextWindow::default()
        };
        let ctx = ContextStatus::new(&w, &ContextConfig::default());
        assert_eq!(ctx.as_ref().map(|c| c.size), Some(200_000));
        assert_eq!(ctx.as_ref().map(|c| c.used_tokens), Some(0));
        assert_eq!(ctx.map(|c| c.level), Some(Level::Good));
    }

    #[test]
    fn context_requires_size() {
        assert!(ContextStatus::new(&ContextWindow::default(), &no_reserve()).is_none());
    }

    #[test]
    fn git_branch_falls_back_to_worktree() {
        let input = StatusInput {
            worktree: Some(input::Worktree {
                branch: Some("wt".into()),
                ..input::Worktree::default()
            }),
            ..StatusInput::default()
        };
        let status = Status::from_input(&input, &Env::default(), &Config::default());
        assert_eq!(status.git.branch.as_deref(), Some("wt"));

        let env = Env {
            git_branch: Some("main".into()),
            ..Env::default()
        };
        let status = Status::from_input(&input, &env, &Config::default());
        assert_eq!(status.git.branch.as_deref(), Some("main"));
    }
}
