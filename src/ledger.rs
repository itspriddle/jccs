//! Today's spend across every session, recorded by each statusline render.
//!
//! Claude Code only reports the current session's cumulative cost, so each
//! render writes it to a small per-session file and sums today's files:
//!
//! - `sessions/<id>` holds the last cost seen for the session, on any day.
//! - `days/<YYYY-MM-DD>/<id>` holds `<baseline> <last>`; the session spent
//!   `last - baseline` today.
//!
//! Files hold absolute values, never running sums, so overlapping renders
//! can't double count, and each session only writes its own files.

use std::{
    env, fs, io,
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};

const KEEP_DAYS: Duration = Duration::from_hours(7 * 24);
const KEEP_SESSIONS: Duration = Duration::from_hours(30 * 24);

/// Returns `$XDG_STATE_HOME/jccs`, falling back to `$HOME/.local/state/jccs`.
pub fn state_dir() -> Option<PathBuf> {
    let base = env::var_os("XDG_STATE_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            env::var_os("HOME").map(|home| PathBuf::from(home).join(".local").join("state"))
        })?;

    Some(base.join("jccs"))
}

/// Today's local date as `YYYY-MM-DD`.
pub fn today() -> String {
    jiff::Zoned::now().date().to_string()
}

/// Records `session` (id, cumulative cost) under `date` and returns the total
/// spent on `date`, or `None` when nothing has been recorded. Errors are
/// swallowed: a broken ledger must never break the statusline.
pub fn today_usd(state_dir: &Path, date: &str, session: Option<(&str, f64)>) -> Option<f64> {
    let day_dir = state_dir.join("days").join(date);

    if let Some((id, cost)) = session.filter(|(id, cost)| is_safe_id(id) && cost.is_finite()) {
        let _ = record(state_dir, &day_dir, id, cost);
    }

    let mut total = None;
    for entry in fs::read_dir(&day_dir).ok()?.flatten() {
        if let Some((baseline, last)) = read_pair(&entry.path()) {
            *total.get_or_insert(0.0) += (last - baseline).max(0.0);
        }
    }
    total
}

#[expect(
    clippy::float_cmp,
    reason = "costs round-trip exactly through Display and parse"
)]
fn record(state_dir: &Path, day_dir: &Path, id: &str, cost: f64) -> io::Result<()> {
    let day_file = day_dir.join(id);
    let session_file = state_dir.join("sessions").join(id);

    match read_pair(&day_file) {
        Some((_, last)) if last == cost => return Ok(()),
        // A lower cost means the session's total was reset; count from zero.
        Some((_, last)) if cost < last => write_atomic(&day_file, &pair(0.0, cost))?,
        Some((baseline, _)) => write_atomic(&day_file, &pair(baseline, cost))?,
        None => {
            let new_day = !day_dir.exists();
            fs::create_dir_all(day_dir)?;
            let baseline = read_floats(&session_file)
                .and_then(|v| v.first().copied())
                .filter(|&b| b <= cost)
                .unwrap_or(0.0);
            write_atomic(&day_file, &pair(baseline, cost))?;
            if new_day {
                prune(state_dir);
            }
        }
    }

    fs::create_dir_all(state_dir.join("sessions"))?;
    write_atomic(&session_file, &cost.to_string())
}

/// Removes day directories and session files that are no longer needed.
fn prune(state_dir: &Path) {
    let now = SystemTime::now();
    let stale = |path: &Path, keep: Duration| {
        fs::metadata(path)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|modified| now.duration_since(modified).ok())
            .is_some_and(|age| age > keep)
    };

    for (dir, keep) in [("days", KEEP_DAYS), ("sessions", KEEP_SESSIONS)] {
        let Ok(entries) = fs::read_dir(state_dir.join(dir)) else {
            continue;
        };
        for path in entries.flatten().map(|e| e.path()) {
            if stale(&path, keep) {
                let _ = if path.is_dir() {
                    fs::remove_dir_all(&path)
                } else {
                    fs::remove_file(&path)
                };
            }
        }
    }
}

/// Session ids become file names, so allow nothing that could escape the
/// directory.
fn is_safe_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 128 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

fn pair(baseline: f64, last: f64) -> String {
    format!("{baseline} {last}")
}

fn read_pair(path: &Path) -> Option<(f64, f64)> {
    match read_floats(path)?.as_slice() {
        &[baseline, last] => Some((baseline, last)),
        _ => None,
    }
}

fn read_floats(path: &Path) -> Option<Vec<f64>> {
    fs::read_to_string(path)
        .ok()?
        .split_whitespace()
        .map(|s| s.parse().ok().filter(|v: &f64| v.is_finite()))
        .collect()
}

/// Writes via a temp file and rename so readers never see a partial file.
fn write_atomic(path: &Path, contents: &str) -> io::Result<()> {
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(format!(".{}.tmp", std::process::id()));
    let tmp = PathBuf::from(tmp);
    fs::write(&tmp, contents)?;
    fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh directory per test; tests run in parallel in one process.
    fn temp_dir(name: &str) -> PathBuf {
        let dir = env::temp_dir().join(format!("jccs-ledger-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    fn render(dir: &Path, date: &str, id: &str, cost: f64) -> Option<f64> {
        today_usd(dir, date, Some((id, cost)))
    }

    #[test]
    fn first_render_counts_full_cost() {
        let dir = temp_dir("first");
        assert_eq!(render(&dir, "2026-01-01", "a", 1.5), Some(1.5));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn repeated_renders_are_idempotent() {
        let dir = temp_dir("repeat");
        render(&dir, "2026-01-01", "a", 1.5);
        assert_eq!(render(&dir, "2026-01-01", "a", 1.5), Some(1.5));
        assert_eq!(render(&dir, "2026-01-01", "a", 2.0), Some(2.0));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn splits_sessions_across_midnight() {
        let dir = temp_dir("midnight");
        render(&dir, "2026-01-01", "a", 3.0);
        assert_eq!(render(&dir, "2026-01-02", "a", 3.0), Some(0.0));
        assert_eq!(render(&dir, "2026-01-02", "a", 4.25), Some(1.25));
        assert_eq!(today_usd(&dir, "2026-01-01", None), Some(3.0));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn sums_sessions() {
        let dir = temp_dir("sum");
        render(&dir, "2026-01-01", "a", 1.0);
        assert_eq!(render(&dir, "2026-01-01", "b", 2.5), Some(3.5));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn ignores_unsafe_ids() {
        let dir = temp_dir("unsafe");
        for id in ["", "../x", "a/b", "."] {
            assert_eq!(render(&dir, "2026-01-01", id, 1.0), None);
        }
        assert!(!dir.exists());
    }

    #[test]
    fn lower_cost_resets_baseline() {
        let dir = temp_dir("reset");
        render(&dir, "2026-01-01", "a", 3.0);
        render(&dir, "2026-01-02", "a", 4.0);
        assert_eq!(render(&dir, "2026-01-02", "a", 0.5), Some(0.5));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_state_dir_is_none() {
        let dir = temp_dir("missing");
        assert_eq!(today_usd(&dir, "2026-01-01", None), None);
    }
}
