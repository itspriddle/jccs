//! `jccs install` and `jccs uninstall`: wire jccs into Claude Code's
//! `settings.json` and manage the config file.

use std::{
    env, fmt, fs,
    io::{self, BufRead as _, IsTerminal, Write as _},
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use serde_json::{Map, Value, json};

use crate::{config, ledger};

const STATUS_LINE: &str = "statusLine";
const SUBAGENT_STATUS_LINE: &str = "subagentStatusLine";

/// Every settings key jccs installs, in install order.
const KEYS: [&str; 2] = [STATUS_LINE, SUBAGENT_STATUS_LINE];

/// Everything install and uninstall touch.
struct Paths {
    settings: PathBuf,
    config: PathBuf,
    /// Ledger state; `None` when `HOME` and `XDG_STATE_HOME` are unset.
    state: Option<PathBuf>,
    exe: PathBuf,
}

impl Paths {
    fn discover() -> Result<Self> {
        Ok(Self {
            settings: claude_settings_path()
                .context("cannot find Claude settings; set HOME or CLAUDE_CONFIG_DIR")?,
            config: config::config_path()
                .context("cannot determine config directory; set HOME or XDG_CONFIG_HOME")?,
            state: ledger::state_dir(),
            // Not canonicalized, so a symlink (e.g. from a package manager)
            // keeps working after the binary it points to is upgraded.
            exe: env::current_exe().context("locating the jccs binary")?,
        })
    }

    fn config_dir(&self) -> Option<&Path> {
        self.config.parent()
    }
}

/// Returns `$CLAUDE_CONFIG_DIR/settings.json`, falling back to
/// `$HOME/.claude/settings.json`.
fn claude_settings_path() -> Option<PathBuf> {
    let base = env::var_os("CLAUDE_CONFIG_DIR")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".claude")))?;

    Some(base.join("settings.json"))
}

#[derive(Debug, PartialEq, Eq)]
enum Action {
    SetStatusLine {
        settings: PathBuf,
        key: &'static str,
        command: String,
        replacing: Option<String>,
    },
    RemoveStatusLine {
        settings: PathBuf,
        key: &'static str,
    },
    WriteConfig {
        path: PathBuf,
        overwrite: bool,
    },
    RemoveConfigDir(PathBuf),
    RemoveStateDir(PathBuf),
}

impl fmt::Display for Action {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SetStatusLine {
                settings,
                key,
                command,
                replacing: None,
            } => write!(
                f,
                "add {key} to {} (command: {command})",
                settings.display()
            ),
            Self::SetStatusLine {
                settings,
                key,
                command,
                replacing: Some(old),
            } => write!(
                f,
                "replace {key} in {} (command: {old} -> {command})",
                settings.display()
            ),
            Self::RemoveStatusLine { settings, key } => {
                write!(f, "remove {key} from {}", settings.display())
            }
            Self::WriteConfig {
                path,
                overwrite: false,
            } => write!(f, "create {}", path.display()),
            Self::WriteConfig {
                path,
                overwrite: true,
            } => write!(
                f,
                "overwrite {} (existing file is backed up)",
                path.display()
            ),
            Self::RemoveConfigDir(dir) | Self::RemoveStateDir(dir) => {
                write!(f, "remove {}", dir.display())
            }
        }
    }
}

pub fn install(overwrite: bool, yes: bool) -> Result<()> {
    let paths = Paths::discover()?;
    let settings = read_settings(&paths.settings)?;
    let actions = plan_install(settings.as_ref(), &paths, paths.config.exists(), overwrite)?;

    if actions.is_empty() {
        println!("jccs is already installed");
        return Ok(());
    }

    print_plan("install", &actions);
    if !confirm(yes)? {
        bail!("aborted");
    }
    apply(settings, &actions)?;
    println!("installed");
    Ok(())
}

pub fn uninstall(yes: bool) -> Result<()> {
    let paths = Paths::discover()?;
    let settings = read_settings(&paths.settings)?;
    let existing = Existing {
        config_dir: paths.config_dir().is_some_and(Path::exists),
        state_dir: paths.state.as_deref().is_some_and(Path::exists),
    };
    let (actions, warnings) = plan_uninstall(settings.as_ref(), &paths, existing);

    for warning in warnings {
        println!("warning: {warning}");
    }

    if actions.is_empty() {
        println!("nothing to uninstall");
    } else {
        print_plan("uninstall", &actions);
        if !confirm(yes)? {
            bail!("aborted");
        }
        apply(settings, &actions)?;
    }

    println!(
        "to remove the binary, run:\n\n  rm {}",
        shell_quote(&paths.exe)
    );
    Ok(())
}

fn plan_install(
    settings: Option<&Value>,
    paths: &Paths,
    config_exists: bool,
    overwrite: bool,
) -> Result<Vec<Action>> {
    let command = shell_quote(&paths.exe);
    let mut actions = Vec::new();
    let mut conflicts = Vec::new();

    for key in KEYS {
        let replacing = match settings.and_then(|s| s.get(key)) {
            None => None,
            Some(existing) if status_command(existing) == Some(command.as_str()) => continue,
            Some(existing) => {
                let old = describe(existing);
                if !overwrite {
                    conflicts.push(format!(
                        "{} already has a {key}: {old}",
                        paths.settings.display()
                    ));
                    continue;
                }
                Some(old)
            }
        };
        actions.push(Action::SetStatusLine {
            settings: paths.settings.clone(),
            key,
            command: command.clone(),
            replacing,
        });
    }

    if !conflicts.is_empty() {
        bail!(
            "{}\npass --overwrite to replace {}",
            conflicts.join("\n"),
            if conflicts.len() == 1 { "it" } else { "them" }
        );
    }

    if !config_exists || overwrite {
        actions.push(Action::WriteConfig {
            path: paths.config.clone(),
            overwrite: config_exists,
        });
    }

    Ok(actions)
}

/// Which jccs directories are present on disk.
#[derive(Debug, Default, Clone, Copy)]
struct Existing {
    config_dir: bool,
    state_dir: bool,
}

/// Returns the actions plus a warning for each status line that belongs to
/// something other than jccs and is left alone.
fn plan_uninstall(
    settings: Option<&Value>,
    paths: &Paths,
    existing: Existing,
) -> (Vec<Action>, Vec<String>) {
    let mut actions = Vec::new();
    let mut warnings = Vec::new();

    for key in KEYS {
        let Some(existing) = settings.and_then(|s| s.get(key)) else {
            continue;
        };
        if status_command(existing).is_some_and(|cmd| is_jccs_command(cmd, &paths.exe)) {
            actions.push(Action::RemoveStatusLine {
                settings: paths.settings.clone(),
                key,
            });
        } else {
            warnings.push(format!(
                "leaving {key} in {}, it does not run jccs: {}",
                paths.settings.display(),
                describe(existing)
            ));
        }
    }

    if let Some(dir) = paths.config_dir().filter(|_| existing.config_dir) {
        actions.push(Action::RemoveConfigDir(dir.to_owned()));
    }
    if let Some(dir) = paths.state.as_ref().filter(|_| existing.state_dir) {
        actions.push(Action::RemoveStateDir(dir.clone()));
    }

    (actions, warnings)
}

fn status_command(status_line: &Value) -> Option<&str> {
    status_line.get("command").and_then(Value::as_str)
}

/// The command, or the raw JSON when there isn't one.
fn describe(status_line: &Value) -> String {
    status_command(status_line).map_or_else(|| status_line.to_string(), str::to_owned)
}

/// True when `cmd` runs this binary or any executable named `jccs`.
fn is_jccs_command(cmd: &str, exe: &Path) -> bool {
    if cmd == shell_quote(exe) {
        return true;
    }
    cmd.split_whitespace()
        .next()
        .and_then(|program| Path::new(program).file_name())
        .is_some_and(|name| name == "jccs")
}

/// Quotes `path` for the shell Claude Code runs the command in, leaving
/// ordinary paths untouched.
fn shell_quote(path: &Path) -> String {
    let path = path.to_string_lossy();
    let plain = path
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || "/._-+~@%,:".contains(c));
    if plain && !path.is_empty() {
        path.into_owned()
    } else {
        format!("'{}'", path.replace('\'', r"'\''"))
    }
}

fn print_plan(verb: &str, actions: &[Action]) {
    println!("jccs {verb} will:");
    for action in actions {
        println!("  - {action}");
    }
}

/// Asks `Continue? [Y/n]`; an empty answer means yes.
fn confirm(yes: bool) -> Result<bool> {
    if yes {
        return Ok(true);
    }

    let stdin = io::stdin();
    if !stdin.is_terminal() {
        bail!("stdin is not a terminal; pass --yes to continue without a prompt");
    }

    print!("Continue? [Y/n] ");
    io::stdout().flush()?;

    let mut answer = String::new();
    if stdin.lock().read_line(&mut answer)? == 0 {
        // EOF (Ctrl-D) is not consent.
        println!();
        return Ok(false);
    }
    Ok(matches!(
        answer.trim().to_ascii_lowercase().as_str(),
        "" | "y" | "yes"
    ))
}

fn apply(settings: Option<Value>, actions: &[Action]) -> Result<()> {
    let mut settings = settings.unwrap_or_else(|| Value::Object(Map::new()));

    for action in actions {
        match action {
            Action::SetStatusLine {
                settings: path,
                key,
                command,
                ..
            } => {
                set_status_line(&mut settings, key, command);
                write_settings(path, &settings)?;
            }
            Action::RemoveStatusLine {
                settings: path,
                key,
            } => {
                if let Some(map) = settings.as_object_mut() {
                    map.shift_remove(*key);
                }
                write_settings(path, &settings)?;
            }
            Action::WriteConfig { path, overwrite } => {
                if let Some(backup) = config::write_template(path, *overwrite)? {
                    println!("backed up {} to {}", path.display(), backup.display());
                }
            }
            Action::RemoveConfigDir(dir) | Action::RemoveStateDir(dir) => {
                fs::remove_dir_all(dir).with_context(|| format!("removing {}", dir.display()))?;
            }
        }
    }

    Ok(())
}

fn set_status_line(settings: &mut Value, key: &str, command: &str) {
    let entry = if key == STATUS_LINE {
        json!({ "type": "command", "command": command, "padding": 0, "refreshInterval": 60 })
    } else {
        json!({ "type": "command", "command": command })
    };
    if let Some(map) = settings.as_object_mut() {
        map.insert(key.to_owned(), entry);
    }
}

/// Reads `settings.json`; a missing file is `None`. Anything that isn't a
/// JSON object is an error so we never clobber a file we don't understand.
fn read_settings(path: &Path) -> Result<Option<Value>> {
    let contents = match fs::read_to_string(path) {
        Ok(contents) => contents,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err).with_context(|| format!("reading {}", path.display())),
    };

    let value: Value =
        serde_json::from_str(&contents).with_context(|| format!("parsing {}", path.display()))?;
    if !value.is_object() {
        bail!("{} is not a JSON object", path.display());
    }
    Ok(Some(value))
}

/// Atomically replaces `path`, following a symlink so dotfile setups keep
/// their link and preserving the file's permissions.
fn write_settings(path: &Path, settings: &Value) -> Result<()> {
    let target = match fs::canonicalize(path) {
        Ok(target) => target,
        Err(err) if err.kind() == io::ErrorKind::NotFound => path.to_owned(),
        Err(err) => return Err(err).with_context(|| format!("resolving {}", path.display())),
    };
    let dir = target
        .parent()
        .with_context(|| format!("{} has no parent directory", target.display()))?;
    fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;

    let mut contents = serde_json::to_string_pretty(settings)?;
    contents.push('\n');

    let mut tmp_name = target.file_name().unwrap_or_default().to_owned();
    tmp_name.push(".jccs-tmp");
    let tmp = dir.join(tmp_name);

    fs::write(&tmp, contents).with_context(|| format!("writing {}", tmp.display()))?;
    if let Ok(meta) = fs::metadata(&target) {
        fs::set_permissions(&tmp, meta.permissions())
            .with_context(|| format!("setting permissions on {}", tmp.display()))?;
    }
    fs::rename(&tmp, &target).with_context(|| format!("replacing {}", target.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths() -> Paths {
        Paths {
            settings: PathBuf::from("/home/me/.claude/settings.json"),
            config: PathBuf::from("/home/me/.config/jccs/config.toml"),
            state: Some(PathBuf::from("/home/me/.local/state/jccs")),
            exe: PathBuf::from("/usr/local/bin/jccs"),
        }
    }

    fn with_command(command: &str) -> Value {
        json!({ "model": "opus", STATUS_LINE: { "type": "command", "command": command } })
    }

    fn with_commands(main: &str, subagent: &str) -> Value {
        json!({
            STATUS_LINE: { "type": "command", "command": main },
            SUBAGENT_STATUS_LINE: { "type": "command", "command": subagent },
        })
    }

    fn set(key: &'static str, replacing: Option<&str>) -> Action {
        Action::SetStatusLine {
            settings: paths().settings,
            key,
            command: "/usr/local/bin/jccs".into(),
            replacing: replacing.map(str::to_owned),
        }
    }

    fn remove(key: &'static str) -> Action {
        Action::RemoveStatusLine {
            settings: paths().settings,
            key,
        }
    }

    /// A fresh directory per test; tests run in parallel in one process.
    fn temp_dir(name: &str) -> Result<PathBuf> {
        let dir = env::temp_dir().join(format!("jccs-install-{name}-{}", std::process::id()));
        fs::create_dir_all(&dir)?;
        Ok(dir)
    }

    #[test]
    fn install_into_missing_settings() -> Result<()> {
        let actions = plan_install(None, &paths(), false, false)?;
        assert_eq!(
            actions,
            [
                set(STATUS_LINE, None),
                set(SUBAGENT_STATUS_LINE, None),
                Action::WriteConfig {
                    path: paths().config,
                    overwrite: false,
                },
            ]
        );
        Ok(())
    }

    #[test]
    fn install_preserves_other_settings_in_order() {
        let mut settings = json!({ "zeta": 1, "alpha": 2 });
        set_status_line(&mut settings, STATUS_LINE, "/usr/local/bin/jccs");
        set_status_line(&mut settings, SUBAGENT_STATUS_LINE, "/usr/local/bin/jccs");
        let keys: Vec<&str> = settings
            .as_object()
            .map(|m| m.keys().map(String::as_str).collect())
            .unwrap_or_default();
        assert_eq!(keys, ["zeta", "alpha", STATUS_LINE, SUBAGENT_STATUS_LINE]);
        assert_eq!(
            settings[STATUS_LINE],
            json!({ "type": "command", "command": "/usr/local/bin/jccs", "padding": 0, "refreshInterval": 60 })
        );
        assert_eq!(
            settings[SUBAGENT_STATUS_LINE],
            json!({ "type": "command", "command": "/usr/local/bin/jccs" })
        );
    }

    #[test]
    fn install_is_a_noop_when_already_installed() -> Result<()> {
        let settings = with_commands("/usr/local/bin/jccs", "/usr/local/bin/jccs");
        assert_eq!(plan_install(Some(&settings), &paths(), true, false)?, []);
        Ok(())
    }

    #[test]
    fn install_adds_missing_subagent_statusline() -> Result<()> {
        let settings = with_command("/usr/local/bin/jccs");
        assert_eq!(
            plan_install(Some(&settings), &paths(), true, false)?,
            [set(SUBAGENT_STATUS_LINE, None)]
        );
        Ok(())
    }

    #[test]
    fn install_refuses_to_replace_another_subagent_statusline() {
        let settings = with_commands("/usr/local/bin/jccs", "agents.sh");
        let Err(err) = plan_install(Some(&settings), &paths(), true, false) else {
            panic!("expected an error");
        };
        let message = format!("{err:#}");
        assert!(
            message.contains("subagentStatusLine: agents.sh"),
            "{message}"
        );
        assert!(message.contains("--overwrite"), "{message}");
    }

    #[test]
    fn install_refuses_to_replace_another_statusline() {
        let settings = with_command("ccstatusline");
        let Err(err) = plan_install(Some(&settings), &paths(), true, false) else {
            panic!("expected an error");
        };
        let message = format!("{err:#}");
        assert!(message.contains("ccstatusline"), "{message}");
        assert!(message.contains("--overwrite"), "{message}");
    }

    #[test]
    fn install_overwrite_replaces_statusline_and_config() -> Result<()> {
        let settings = with_commands("ccstatusline", "agents.sh");
        let actions = plan_install(Some(&settings), &paths(), true, true)?;
        assert_eq!(
            actions,
            [
                set(STATUS_LINE, Some("ccstatusline")),
                set(SUBAGENT_STATUS_LINE, Some("agents.sh")),
                Action::WriteConfig {
                    path: paths().config,
                    overwrite: true,
                },
            ]
        );
        Ok(())
    }

    #[test]
    fn uninstall_removes_jccs_statusline_and_dirs() {
        let settings = with_commands("/opt/homebrew/bin/jccs", "/opt/homebrew/bin/jccs");
        let existing = Existing {
            config_dir: true,
            state_dir: true,
        };
        let (actions, warnings) = plan_uninstall(Some(&settings), &paths(), existing);
        assert_eq!(
            actions,
            [
                remove(STATUS_LINE),
                remove(SUBAGENT_STATUS_LINE),
                Action::RemoveConfigDir(PathBuf::from("/home/me/.config/jccs")),
                Action::RemoveStateDir(PathBuf::from("/home/me/.local/state/jccs")),
            ]
        );
        assert_eq!(warnings, Vec::<String>::new());
    }

    #[test]
    fn uninstall_leaves_other_statuslines() {
        let settings = with_command("ccstatusline");
        let (actions, warnings) = plan_uninstall(Some(&settings), &paths(), Existing::default());
        assert_eq!(actions, []);
        assert!(matches!(warnings.as_slice(), [w] if w.contains("ccstatusline")));

        let settings = with_commands("/usr/local/bin/jccs", "agents.sh");
        let (actions, warnings) = plan_uninstall(Some(&settings), &paths(), Existing::default());
        assert_eq!(actions, [remove(STATUS_LINE)]);
        assert!(matches!(warnings.as_slice(), [w] if w.contains("subagentStatusLine")));
    }

    #[test]
    fn uninstall_with_nothing_installed() {
        let (actions, warnings) = plan_uninstall(Some(&json!({})), &paths(), Existing::default());
        assert_eq!(actions, []);
        assert_eq!(warnings, Vec::<String>::new());
    }

    #[test]
    fn recognizes_jccs_commands() {
        let exe = Path::new("/usr/local/bin/jccs");
        assert!(is_jccs_command("/usr/local/bin/jccs", exe));
        assert!(is_jccs_command("/other/path/jccs", exe));
        assert!(is_jccs_command("jccs --color-mode 256", exe));
        assert!(!is_jccs_command("ccstatusline", exe));
        assert!(!is_jccs_command("/usr/local/bin/jccs-old", exe));

        let spaced = Path::new("/Users/me/My Tools/jccs-dev");
        assert!(is_jccs_command("'/Users/me/My Tools/jccs-dev'", spaced));
    }

    #[test]
    fn quotes_paths_for_the_shell() {
        assert_eq!(shell_quote(Path::new("/usr/bin/jccs")), "/usr/bin/jccs");
        assert_eq!(shell_quote(Path::new("/a b/jccs")), "'/a b/jccs'");
        assert_eq!(shell_quote(Path::new("/it's/jccs")), r"'/it'\''s/jccs'");
    }

    #[test]
    fn read_settings_rejects_non_objects() -> Result<()> {
        let dir = temp_dir("read")?;
        let path = dir.join("settings.json");

        let result = (|| -> Result<()> {
            assert!(read_settings(&path)?.is_none());
            for bad in ["{", "[]", "null"] {
                fs::write(&path, bad)?;
                assert!(read_settings(&path).is_err(), "{bad:?} should fail");
            }
            Ok(())
        })();

        fs::remove_dir_all(&dir)?;
        result
    }

    #[test]
    fn write_settings_round_trips_and_follows_symlinks() -> Result<()> {
        let dir = temp_dir("write")?;
        let real = dir.join("real.json");
        let link = dir.join("nested").join("settings.json");

        let result = (|| -> Result<()> {
            let settings = json!({ "b": 1, "a": [true] });
            write_settings(&link, &settings)?;
            assert_eq!(read_settings(&link)?, Some(settings.clone()));
            fs::remove_file(&link)?;

            #[cfg(unix)]
            {
                fs::write(&real, "{}")?;
                std::os::unix::fs::symlink(&real, &link)?;
                write_settings(&link, &settings)?;
                assert!(fs::symlink_metadata(&link)?.file_type().is_symlink());
                assert_eq!(read_settings(&real)?, Some(settings));
            }
            Ok(())
        })();

        fs::remove_dir_all(&dir)?;
        result
    }
}
