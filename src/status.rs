//! Values derived from the raw statusline input, ready for display.

use std::path::Path;

use jiff::{Timestamp, civil::Date, tz::TimeZone};

use crate::{
    config::{Config, ContextConfig},
    input::{self, StatusInput, Task},
};

const FIVE_HOURS_SECS: i64 = 5 * 60 * 60;
const SEVEN_DAYS_SECS: i64 = 7 * 24 * 60 * 60;
const ONE_MILLION_TOKENS: u64 = 1_000_000;
const MODEL_FAMILIES: [&str; 4] = ["Opus", "Sonnet", "Haiku", "Fable"];

/// Facts from outside the JSON payload, injected so derivation stays pure.
#[derive(Debug)]
pub struct Env {
    /// Current Unix timestamp (seconds).
    pub now: i64,
    /// Zone used to show clock times.
    pub tz: TimeZone,
    pub git_branch: Option<String>,
    /// Spend across every session today, from the ledger.
    pub cost_today_usd: Option<f64>,
}

impl Default for Env {
    fn default() -> Self {
        Self {
            now: 0,
            tz: TimeZone::UTC,
            git_branch: None,
            cost_today_usd: None,
        }
    }
}

#[derive(Debug, Default)]
pub struct Status {
    pub model: ModelStatus,
    pub cost_usd: Option<f64>,
    pub cost_today_usd: Option<f64>,
    pub lines_added: Option<u64>,
    pub lines_removed: Option<u64>,
    pub five_hour: Option<RateWindow>,
    pub seven_day: Option<RateWindow>,
    pub context: Option<ContextStatus>,
    pub git: GitStatus,
    pub workspace: WorkspaceStatus,
}

/// One subagent row, derived from a task in the subagent payload.
#[derive(Debug, Default)]
pub struct SubagentStatus {
    /// The task's name, falling back to its agent type.
    pub name: Option<String>,
    pub agent_type: Option<String>,
    pub description: Option<String>,
    pub status: Option<String>,
    /// Seconds since the task started, never negative.
    pub elapsed: Option<i64>,
    pub model: ModelStatus,
    pub context: Option<ContextStatus>,
}

#[derive(Debug, Default)]
pub struct ModelStatus {
    /// `"{family} {version}"`, with `" (1M)"` appended for a 1M context.
    pub display_name: Option<String>,
    pub family: Option<String>,
    pub version: Option<String>,
    pub effort: Option<String>,
    pub thinking: Option<bool>,
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
    /// Clock times in [`Env::tz`], e.g. `5:00pm`.
    pub started_at: Option<String>,
    pub resets_at: Option<String>,
    /// Unix timestamps (seconds).
    pub started_at_unix: Option<i64>,
    pub resets_at_unix: Option<i64>,
    /// Seconds until reset, never negative.
    pub resets_in: Option<i64>,
}

#[derive(Debug, PartialEq)]
pub struct ContextStatus {
    pub size: u64,
    pub million_tokens: bool,
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

#[derive(Debug, Default)]
pub struct WorkspaceStatus {
    pub project_dir: Option<String>,
    /// Basename of `project_dir`.
    pub project_name: Option<String>,
}

impl Status {
    pub fn from_input(input: &StatusInput, env: &Env, config: &Config) -> Self {
        let cost = input.cost.as_ref();
        let limits = input.rate_limits.as_ref();
        let workspace = input.workspace.as_ref();
        let repo = workspace.and_then(|w| w.repo.as_ref());
        let pr = input.pr.as_ref();
        let project_dir = workspace.and_then(|w| w.project_dir.clone());
        let context = input
            .context_window
            .as_ref()
            .and_then(|c| ContextStatus::new(c, &config.context));
        let million_tokens = context.as_ref().is_some_and(|c| c.million_tokens);

        Self {
            model: ModelStatus::new(input, million_tokens),
            cost_usd: cost.and_then(|c| c.total_cost_usd),
            cost_today_usd: env.cost_today_usd,
            lines_added: cost.and_then(|c| c.total_lines_added),
            lines_removed: cost.and_then(|c| c.total_lines_removed),
            five_hour: limits
                .and_then(|l| l.five_hour.as_ref())
                .and_then(|r| RateWindow::new(r, FIVE_HOURS_SECS, env.now, &env.tz)),
            seven_day: limits
                .and_then(|l| l.seven_day.as_ref())
                .and_then(|r| RateWindow::new(r, SEVEN_DAYS_SECS, env.now, &env.tz)),
            context,
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
            workspace: WorkspaceStatus {
                project_name: project_dir.as_deref().and_then(basename),
                project_dir,
            },
        }
    }
}

impl SubagentStatus {
    /// `now` is the current Unix timestamp in seconds.
    pub fn from_task(task: &Task, now: i64, config: &ContextConfig) -> Self {
        let context = task
            .context_window_size
            .filter(|&s| s > 0)
            .map(|size| ContextStatus::from_tokens(size, task.token_count.unwrap_or(0), config));
        let million_tokens = context.as_ref().is_some_and(|c| c.million_tokens);

        Self {
            name: task.name.clone().or_else(|| task.agent_type.clone()),
            agent_type: task.agent_type.clone(),
            description: task.description.clone(),
            status: task.status.clone(),
            elapsed: task.start_time.map(|ms| (now - ms / 1_000).max(0)),
            model: ModelStatus::from_id(task.model.as_deref(), None, million_tokens),
            context,
        }
    }
}

impl ModelStatus {
    fn new(input: &StatusInput, million_tokens: bool) -> Self {
        let model = input.model.as_ref();
        Self {
            effort: input.effort.as_ref().and_then(|e| e.level.clone()),
            thinking: input.thinking.as_ref().and_then(|t| t.enabled),
            ..Self::from_id(
                model.and_then(|m| m.id.as_deref()),
                model.and_then(|m| m.display_name.clone()),
                million_tokens,
            )
        }
    }

    /// Derives the family, version, and display name from a model ID,
    /// falling back to `display_name` when the ID isn't recognized.
    fn from_id(id: Option<&str>, display_name: Option<String>, million_tokens: bool) -> Self {
        let (family, version) = id.map(parse_model_id).unwrap_or_default();

        let display_name = match (&family, &version) {
            (Some(f), Some(v)) => Some(format!("{f} {v}")),
            _ => display_name,
        }
        .map(|name| {
            if million_tokens && !name.contains("1M") {
                format!("{name} (1M)")
            } else {
                name
            }
        });

        Self {
            display_name,
            family,
            version,
            effort: None,
            thinking: None,
        }
    }
}

/// Extracts the family and version from a model ID such as
/// `claude-opus-5-5`, `claude-3-5-sonnet-20241022`, or
/// `us.anthropic.claude-sonnet-4-5-20250929-v1:0`.
fn parse_model_id(id: &str) -> (Option<String>, Option<String>) {
    let Some((_, rest)) = id.split_once("claude-") else {
        return (None, None);
    };
    // Drop variant suffixes like `[1m]`.
    let rest = rest.split('[').next().unwrap_or_default();
    let parts: Vec<&str> = rest.split('-').collect();

    let family = parts.iter().find_map(|part| {
        MODEL_FAMILIES
            .iter()
            .find(|f| f.eq_ignore_ascii_case(part))
            .map(|&f| f.to_owned())
    });

    // Version numbers are short; longer digit runs are release dates.
    let version: Vec<&str> = parts
        .iter()
        .copied()
        .filter(|p| p.len() <= 2 && p.bytes().all(|b| b.is_ascii_digit()))
        .take(2)
        .collect();
    let version = (!version.is_empty()).then(|| version.join("."));

    (family, version)
}

fn basename(path: &str) -> Option<String> {
    Path::new(path)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
}

impl RateWindow {
    fn new(limit: &input::RateLimit, window_secs: i64, now: i64, tz: &TimeZone) -> Option<Self> {
        let used_pct = limit.used_percentage?;
        let resets_at = limit.resets_at;
        let started_at = resets_at.map(|r| r - window_secs);
        let today = local_date(now, tz);

        Some(Self {
            used_pct,
            remaining_pct: (100.0 - used_pct).max(0.0),
            started_at: started_at.and_then(|t| clock_time(t, tz, today)),
            resets_at: resets_at.and_then(|t| clock_time(t, tz, today)),
            started_at_unix: started_at,
            resets_at_unix: resets_at,
            resets_in: resets_at.map(|r| (r - now).max(0)),
        })
    }
}

/// Formats a Unix timestamp as a 12-hour clock time, e.g. `5:00pm`, with the
/// weekday prepended (`Tue 5:00pm`) when it falls on a date other than `today`.
fn clock_time(secs: i64, tz: &TimeZone, today: Option<Date>) -> Option<String> {
    let zoned = Timestamp::from_second(secs).ok()?.to_zoned(tz.clone());
    let format = if today.is_some_and(|d| d != zoned.date()) {
        "%a %-I:%M%P"
    } else {
        "%-I:%M%P"
    };
    Some(zoned.strftime(format).to_string())
}

fn local_date(secs: i64, tz: &TimeZone) -> Option<Date> {
    Some(
        Timestamp::from_second(secs)
            .ok()?
            .to_zoned(tz.clone())
            .date(),
    )
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

        Some(Self {
            reported_used_pct: window.used_percentage,
            reported_remaining_pct: window.remaining_percentage,
            ..Self::from_tokens(size, used_tokens, config)
        })
    }

    /// Context usage measured against `size - reserved_tokens`, with no
    /// reported percentages.
    #[expect(
        clippy::cast_precision_loss,
        reason = "token counts are far below f64's exact integer range"
    )]
    fn from_tokens(size: u64, used_tokens: u64, config: &ContextConfig) -> Self {
        let effective = size.saturating_sub(config.reserved_tokens).max(1);
        let used_pct = (used_tokens as f64 / effective as f64 * 100.0).clamp(0.0, 100.0);

        Self {
            size,
            million_tokens: size >= ONE_MILLION_TOKENS,
            reserved_tokens: config.reserved_tokens,
            used_tokens,
            used_pct,
            remaining_pct: 100.0 - used_pct,
            level: level_for(used_pct, config),
            reported_used_pct: None,
            reported_remaining_pct: None,
        }
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
        let w = RateWindow::new(&rate(23.5, 20_000), FIVE_HOURS_SECS, 15_000, &TimeZone::UTC);
        assert_eq!(
            w,
            Some(RateWindow {
                used_pct: 23.5,
                remaining_pct: 76.5,
                started_at: Some("12:33am".into()),
                resets_at: Some("5:33am".into()),
                started_at_unix: Some(2_000),
                resets_at_unix: Some(20_000),
                resets_in: Some(5_000),
            })
        );
    }

    #[test]
    fn clock_time_is_12_hour_in_zone() -> Result<(), jiff::Error> {
        let cases = [
            (0, "12:00am"),
            (9 * 3_600 + 5 * 60, "9:05am"),
            (12 * 3_600, "12:00pm"),
            (17 * 3_600, "5:00pm"),
        ];
        for (secs, expected) in cases {
            assert_eq!(
                clock_time(secs, &TimeZone::UTC, None).as_deref(),
                Some(expected)
            );
        }

        let tz = TimeZone::get("America/New_York")?;
        assert_eq!(clock_time(0, &tz, None).as_deref(), Some("7:00pm"));
        assert_eq!(clock_time(i64::MAX, &TimeZone::UTC, None), None);
        Ok(())
    }

    #[test]
    fn clock_time_shows_weekday_off_today() {
        // 1970-01-01 was a Thursday.
        let today = local_date(0, &TimeZone::UTC);
        let at = |secs| clock_time(secs, &TimeZone::UTC, today);
        assert_eq!(at(17 * 3_600).as_deref(), Some("5:00pm"));
        assert_eq!(at(86_400 + 17 * 3_600).as_deref(), Some("Fri 5:00pm"));
        assert_eq!(at(-3_600).as_deref(), Some("Wed 11:00pm"));
    }

    #[test]
    fn rate_window_shows_weekdays_off_today() {
        let now = 17 * 3_600;
        let w = RateWindow::new(&rate(1.0, now), SEVEN_DAYS_SECS, now, &TimeZone::UTC);
        let w = w.as_ref();
        assert_eq!(w.and_then(|w| w.resets_at.as_deref()), Some("5:00pm"));
        assert_eq!(w.and_then(|w| w.started_at.as_deref()), Some("Thu 5:00pm"));
    }

    #[test]
    fn rate_window_countdown_saturates_at_zero() {
        let w = RateWindow::new(&rate(10.0, 100), SEVEN_DAYS_SECS, 500, &TimeZone::UTC);
        assert_eq!(w.and_then(|w| w.resets_in), Some(0));
    }

    #[test]
    fn rate_window_requires_used_percentage() {
        let limit = RateLimit {
            used_percentage: None,
            resets_at: Some(1),
        };
        assert!(RateWindow::new(&limit, FIVE_HOURS_SECS, 0, &TimeZone::UTC).is_none());
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
    fn parses_model_ids() {
        let cases = [
            ("claude-opus-5-5", Some("Opus"), Some("5.5")),
            ("claude-fable-5-1", Some("Fable"), Some("5.1")),
            ("claude-haiku-5-5[1m]", Some("Haiku"), Some("5.5")),
            ("claude-opus-4-20250514", Some("Opus"), Some("4")),
            ("claude-3-5-sonnet-20241022", Some("Sonnet"), Some("3.5")),
            (
                "us.anthropic.claude-sonnet-4-5-20250929-v1:0",
                Some("Sonnet"),
                Some("4.5"),
            ),
            ("claude-mystery", None, None),
            ("gpt-4", None, None),
        ];
        for (id, family, version) in cases {
            let (f, v) = parse_model_id(id);
            assert_eq!((f.as_deref(), v.as_deref()), (family, version), "{id}");
        }
    }

    fn model_input(id: &str, display_name: &str, size: u64) -> StatusInput {
        StatusInput {
            model: Some(input::Model {
                id: Some(id.into()),
                display_name: Some(display_name.into()),
            }),
            context_window: Some(window(size, 0)),
            ..StatusInput::default()
        }
    }

    #[test]
    fn model_display_name() {
        let cases = [
            ("claude-opus-5-5", "Opus", 200_000, "Opus 5.5"),
            ("claude-opus-5-5[1m]", "Opus", 1_000_000, "Opus 5.5 (1M)"),
            ("custom", "Custom", 200_000, "Custom"),
            ("custom", "Custom", 1_000_000, "Custom (1M)"),
            (
                "custom",
                "Custom (1M context)",
                1_000_000,
                "Custom (1M context)",
            ),
        ];
        for (id, display_name, size, expected) in cases {
            let input = model_input(id, display_name, size);
            let status = Status::from_input(&input, &Env::default(), &Config::default());
            assert_eq!(status.model.display_name.as_deref(), Some(expected), "{id}");
        }
    }

    #[test]
    fn context_flags_million_tokens() {
        let ctx = |size| ContextStatus::new(&window(size, 0), &no_reserve());
        assert_eq!(ctx(200_000).map(|c| c.million_tokens), Some(false));
        assert_eq!(ctx(1_000_000).map(|c| c.million_tokens), Some(true));
    }

    #[test]
    fn workspace_project_name_is_basename() {
        let input = StatusInput {
            workspace: Some(input::Workspace {
                project_dir: Some("/src/jccs/".into()),
                ..input::Workspace::default()
            }),
            ..StatusInput::default()
        };
        let status = Status::from_input(&input, &Env::default(), &Config::default());
        assert_eq!(status.workspace.project_dir.as_deref(), Some("/src/jccs/"));
        assert_eq!(status.workspace.project_name.as_deref(), Some("jccs"));
    }

    fn task() -> Task {
        Task {
            id: Some("t1".into()),
            agent_type: Some("Explore".into()),
            model: Some("claude-opus-5".into()),
            token_count: Some(90_000),
            context_window_size: Some(200_000),
            start_time: Some(1_000_500),
            ..Task::default()
        }
    }

    #[test]
    fn subagent_derives_row_values() {
        let status = SubagentStatus::from_task(&task(), 1_090, &ContextConfig::default());
        assert_eq!(status.name.as_deref(), Some("Explore"));
        assert_eq!(status.elapsed, Some(90));
        assert_eq!(status.model.display_name.as_deref(), Some("Opus 5"));
        let ctx = status.context;
        assert_eq!(ctx.as_ref().map(|c| c.used_tokens), Some(90_000));
        assert!(ctx.is_some_and(|c| (c.used_pct - 50.0).abs() < f64::EPSILON));
    }

    #[test]
    fn subagent_prefers_name_over_agent_type() {
        let task = Task {
            name: Some("auth-mapper".into()),
            ..task()
        };
        let status = SubagentStatus::from_task(&task, 0, &ContextConfig::default());
        assert_eq!(status.name.as_deref(), Some("auth-mapper"));
        assert_eq!(status.agent_type.as_deref(), Some("Explore"));
    }

    #[test]
    fn subagent_missing_values() {
        let status = SubagentStatus::from_task(&Task::default(), 0, &ContextConfig::default());
        assert_eq!(status.name, None);
        assert_eq!(status.elapsed, None);
        assert!(status.context.is_none());

        let task = Task {
            context_window_size: Some(0),
            start_time: Some(5_000),
            ..Task::default()
        };
        let status = SubagentStatus::from_task(&task, 1, &ContextConfig::default());
        assert!(status.context.is_none());
        assert_eq!(status.elapsed, Some(0));
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
