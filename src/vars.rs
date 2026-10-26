//! Flattens [`Status`] into named, typed variables. These names are what
//! format strings will reference, e.g. `{rate.five_hour.resets_in}`.

use std::fmt;

use crate::status::{Level, RateWindow, Status};

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Text(String),
    Int(u64),
    Usd(f64),
    Percent(f64),
    Bool(bool),
    /// Unix timestamp (seconds).
    Timestamp(i64),
    /// Seconds.
    Duration(i64),
    Level(Level),
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Text(s) => f.write_str(s),
            Self::Int(n) => write!(f, "{n}"),
            Self::Usd(n) => write!(f, "${n:.2}"),
            Self::Percent(n) => write!(f, "{n:.0}%"),
            Self::Bool(b) => write!(f, "{b}"),
            Self::Timestamp(t) => write!(f, "{t}"),
            Self::Duration(secs) => write_duration(f, *secs),
            Self::Level(level) => f.write_str(level.as_str()),
        }
    }
}

fn write_duration(f: &mut fmt::Formatter<'_>, secs: i64) -> fmt::Result {
    let (days, rem) = (secs / 86_400, secs % 86_400);
    let (hours, rem) = (rem / 3_600, rem % 3_600);
    let (mins, secs) = (rem / 60, rem % 60);

    match (days, hours, mins) {
        (0, 0, 0) => write!(f, "{secs}s"),
        (0, 0, _) => write!(f, "{mins}m"),
        (0, _, _) => write!(f, "{hours}h {mins}m"),
        _ => write!(f, "{days}d {hours}h"),
    }
}

/// Ordered `(name, value)` pairs; absent values are omitted.
pub type Vars = Vec<(&'static str, Value)>;

/// Every name [`Status::vars`] can produce, in output order.
pub const NAMES: &[&str] = &[
    "model.display_name",
    "model.family",
    "model.version",
    "model.effort.level",
    "model.thinking.enabled",
    "cost.total_usd",
    "cost.today_usd",
    "lines.added",
    "lines.removed",
    "lines.changed",
    RATE_FIVE_HOUR[0],
    RATE_FIVE_HOUR[1],
    RATE_FIVE_HOUR[2],
    RATE_FIVE_HOUR[3],
    RATE_FIVE_HOUR[4],
    RATE_FIVE_HOUR[5],
    RATE_FIVE_HOUR[6],
    RATE_SEVEN_DAY[0],
    RATE_SEVEN_DAY[1],
    RATE_SEVEN_DAY[2],
    RATE_SEVEN_DAY[3],
    RATE_SEVEN_DAY[4],
    RATE_SEVEN_DAY[5],
    RATE_SEVEN_DAY[6],
    "context.size",
    "context.1m_tokens",
    "context.reserved_tokens",
    "context.used_tokens",
    "context.used_pct",
    "context.remaining_pct",
    "context.level",
    "context.reported_used_pct",
    "context.reported_remaining_pct",
    "git.branch",
    "git.owner",
    "git.repo",
    "git.pr.number",
    "git.pr.url",
    "workspace.project_dir",
    "workspace.project_name",
];

pub fn lookup<'a>(vars: &'a Vars, name: &str) -> Option<&'a Value> {
    vars.iter().find(|(n, _)| *n == name).map(|(_, v)| v)
}

impl Status {
    pub fn vars(&self) -> Vars {
        let mut vars = Vars::new();
        let mut push = |name, value: Option<Value>| {
            if let Some(value) = value {
                vars.push((name, value));
            }
        };

        let model = &self.model;
        push(
            "model.display_name",
            model.display_name.clone().map(Value::Text),
        );
        push("model.family", model.family.clone().map(Value::Text));
        push("model.version", model.version.clone().map(Value::Text));
        push("model.effort.level", model.effort.clone().map(Value::Text));
        push("model.thinking.enabled", model.thinking.map(Value::Bool));

        push("cost.total_usd", self.cost_usd.map(Value::Usd));
        push("cost.today_usd", self.cost_today_usd.map(Value::Usd));
        push("lines.added", self.lines_added.map(Value::Int));
        push("lines.removed", self.lines_removed.map(Value::Int));
        push(
            "lines.changed",
            match (self.lines_added, self.lines_removed) {
                (None, None) => None,
                (a, r) => Some(Value::Int(a.unwrap_or(0) + r.unwrap_or(0))),
            },
        );

        if let Some(w) = &self.five_hour {
            push_rate(&mut push, RATE_FIVE_HOUR, w);
        }
        if let Some(w) = &self.seven_day {
            push_rate(&mut push, RATE_SEVEN_DAY, w);
        }

        if let Some(c) = &self.context {
            push("context.size", Some(Value::Int(c.size)));
            push("context.1m_tokens", Some(Value::Bool(c.million_tokens)));
            push(
                "context.reserved_tokens",
                Some(Value::Int(c.reserved_tokens)),
            );
            push("context.used_tokens", Some(Value::Int(c.used_tokens)));
            push("context.used_pct", Some(Value::Percent(c.used_pct)));
            push(
                "context.remaining_pct",
                Some(Value::Percent(c.remaining_pct)),
            );
            push("context.level", Some(Value::Level(c.level)));
            push(
                "context.reported_used_pct",
                c.reported_used_pct.map(Value::Percent),
            );
            push(
                "context.reported_remaining_pct",
                c.reported_remaining_pct.map(Value::Percent),
            );
        }

        let git = &self.git;
        push("git.branch", git.branch.clone().map(Value::Text));
        push("git.owner", git.owner.clone().map(Value::Text));
        push("git.repo", git.repo.clone().map(Value::Text));
        push("git.pr.number", git.pr_number.map(Value::Int));
        push("git.pr.url", git.pr_url.clone().map(Value::Text));

        let workspace = &self.workspace;
        push(
            "workspace.project_dir",
            workspace.project_dir.clone().map(Value::Text),
        );
        push(
            "workspace.project_name",
            workspace.project_name.clone().map(Value::Text),
        );

        vars
    }
}

/// Variable names for one rate-limit window, in output order.
type RateNames = [&'static str; 7];

const RATE_FIVE_HOUR: RateNames = [
    "rate.five_hour.used_pct",
    "rate.five_hour.remaining_pct",
    "rate.five_hour.started_at",
    "rate.five_hour.started_at_unix",
    "rate.five_hour.resets_at",
    "rate.five_hour.resets_at_unix",
    "rate.five_hour.resets_in",
];

const RATE_SEVEN_DAY: RateNames = [
    "rate.seven_day.used_pct",
    "rate.seven_day.remaining_pct",
    "rate.seven_day.started_at",
    "rate.seven_day.started_at_unix",
    "rate.seven_day.resets_at",
    "rate.seven_day.resets_at_unix",
    "rate.seven_day.resets_in",
];

fn push_rate(push: &mut impl FnMut(&'static str, Option<Value>), names: RateNames, w: &RateWindow) {
    let [
        used,
        remaining,
        started_at,
        started_at_unix,
        resets_at,
        resets_at_unix,
        resets_in,
    ] = names;
    push(used, Some(Value::Percent(w.used_pct)));
    push(remaining, Some(Value::Percent(w.remaining_pct)));
    push(started_at, w.started_at.clone().map(Value::Text));
    push(started_at_unix, w.started_at_unix.map(Value::Timestamp));
    push(resets_at, w.resets_at.clone().map(Value::Text));
    push(resets_at_unix, w.resets_at_unix.map(Value::Timestamp));
    push(resets_in, w.resets_in.map(Value::Duration));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::status::{ContextStatus, GitStatus, ModelStatus, WorkspaceStatus};

    #[test]
    fn formats_durations() {
        let cases = [
            (0, "0s"),
            (59, "59s"),
            (60, "1m"),
            (3_660, "1h 1m"),
            (90_000, "1d 1h"),
        ];
        for (secs, expected) in cases {
            assert_eq!(Value::Duration(secs).to_string(), expected);
        }
    }

    #[test]
    fn formats_percents_as_whole_numbers() {
        let cases = [(42.0, "42%"), (12.5, "12%"), (99.6, "100%"), (0.4, "0%")];
        for (n, expected) in cases {
            assert_eq!(Value::Percent(n).to_string(), expected);
        }
    }

    #[test]
    fn lines_changed_sums_added_and_removed() {
        let changed = |added, removed| {
            let status = Status {
                lines_added: added,
                lines_removed: removed,
                ..Status::default()
            };
            lookup(&status.vars(), "lines.changed").cloned()
        };
        assert_eq!(changed(Some(3), Some(2)), Some(Value::Int(5)));
        assert_eq!(changed(Some(3), None), Some(Value::Int(3)));
        assert_eq!(changed(Some(0), Some(0)), Some(Value::Int(0)));
        assert_eq!(changed(None, None), None);
    }

    #[test]
    fn names_match_fully_populated_status() {
        let window = || {
            Some(RateWindow {
                used_pct: 1.0,
                remaining_pct: 99.0,
                started_at: Some("12:00am".into()),
                resets_at: Some("12:00am".into()),
                started_at_unix: Some(1),
                resets_at_unix: Some(2),
                resets_in: Some(1),
            })
        };
        let status = Status {
            model: ModelStatus {
                display_name: Some("Opus 5.5".into()),
                family: Some("Opus".into()),
                version: Some("5.5".into()),
                effort: Some("high".into()),
                thinking: Some(true),
            },
            cost_usd: Some(1.0),
            cost_today_usd: Some(2.0),
            lines_added: Some(1),
            lines_removed: Some(1),
            five_hour: window(),
            seven_day: window(),
            context: Some(ContextStatus {
                size: 1,
                million_tokens: false,
                reserved_tokens: 0,
                used_tokens: 1,
                used_pct: 1.0,
                remaining_pct: 99.0,
                level: Level::Good,
                reported_used_pct: Some(1.0),
                reported_remaining_pct: Some(99.0),
            }),
            git: GitStatus {
                branch: Some("main".into()),
                owner: Some("o".into()),
                repo: Some("r".into()),
                pr_number: Some(1),
                pr_url: Some("u".into()),
            },
            workspace: WorkspaceStatus {
                project_dir: Some("/p".into()),
                project_name: Some("p".into()),
            },
        };
        let names: Vec<_> = status.vars().into_iter().map(|(n, _)| n).collect();
        assert_eq!(names, NAMES);
    }

    #[test]
    fn omits_missing_values() {
        let vars = Status::default().vars();
        assert_eq!(vars, Vars::new());
    }
}
