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
            Self::Percent(n) => write!(f, "{n:.1}%"),
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

impl Status {
    pub fn vars(&self) -> Vars {
        let mut vars = Vars::new();
        let mut push = |name, value: Option<Value>| {
            if let Some(value) = value {
                vars.push((name, value));
            }
        };

        push("model.name", self.model.clone().map(Value::Text));
        push("effort.level", self.effort.clone().map(Value::Text));
        push("thinking.enabled", self.thinking.map(Value::Bool));

        push("cost.total_usd", self.cost_usd.map(Value::Usd));
        push("lines.added", self.lines_added.map(Value::Int));
        push("lines.removed", self.lines_removed.map(Value::Int));

        if let Some(w) = &self.five_hour {
            push_rate(&mut push, RATE_FIVE_HOUR, w);
        }
        if let Some(w) = &self.seven_day {
            push_rate(&mut push, RATE_SEVEN_DAY, w);
        }

        if let Some(c) = &self.context {
            push("context.size", Some(Value::Int(c.size)));
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

        vars
    }
}

/// Variable names for one rate-limit window, in output order.
type RateNames = [&'static str; 5];

const RATE_FIVE_HOUR: RateNames = [
    "rate.five_hour.used_pct",
    "rate.five_hour.remaining_pct",
    "rate.five_hour.starts_at",
    "rate.five_hour.resets_at",
    "rate.five_hour.resets_in",
];

const RATE_SEVEN_DAY: RateNames = [
    "rate.seven_day.used_pct",
    "rate.seven_day.remaining_pct",
    "rate.seven_day.starts_at",
    "rate.seven_day.resets_at",
    "rate.seven_day.resets_in",
];

fn push_rate(push: &mut impl FnMut(&'static str, Option<Value>), names: RateNames, w: &RateWindow) {
    let [used, remaining, starts_at, resets_at, resets_in] = names;
    push(used, Some(Value::Percent(w.used_pct)));
    push(remaining, Some(Value::Percent(w.remaining_pct)));
    push(starts_at, w.starts_at.map(Value::Timestamp));
    push(resets_at, w.resets_at.map(Value::Timestamp));
    push(resets_in, w.resets_in.map(Value::Duration));
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn omits_missing_values() {
        let vars = Status::default().vars();
        assert!(vars.is_empty());
    }
}
