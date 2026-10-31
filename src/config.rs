use std::{
    collections::BTreeMap,
    env,
    fs::{self, OpenOptions},
    io::{self, Write as _},
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use serde::Deserialize;

use crate::{
    style::{self, Color, ColorMode, Palette},
    template::{self, Scope, Template},
    vars::{self, Vars},
};

/// A fully documented config that renders the same line as the defaults.
pub const TEMPLATE: &str = include_str!("../config.example.toml");

/// User configuration loaded from `~/.config/jccs/config.toml`.
#[derive(Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub color_mode: ColorMode,
    /// Template placed between rendered segments; may not use values.
    pub separator: String,
    pub segments: Segments,
    /// Merged over [`DEFAULT_ICONS`]; set an icon to `""` to remove it.
    pub icons: BTreeMap<String, String>,
    /// Merged over the built-in color names and [`DEFAULT_COLORS`].
    pub colors: BTreeMap<String, ColorSpec>,
    pub context: ContextConfig,
    pub subagent: SubagentConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            color_mode: ColorMode::default(),
            separator: DEFAULT_SEPARATOR.to_owned(),
            segments: Segments::Line(
                DEFAULT_SEGMENTS
                    .iter()
                    .map(|&s| SegmentSpec::Format(s.to_owned()))
                    .chain([SegmentSpec::When {
                        format: "[yellow]{icon.thinking}[/]".to_owned(),
                        when: "model.thinking.enabled".to_owned(),
                    }])
                    .collect(),
            ),
            icons: BTreeMap::new(),
            colors: BTreeMap::new(),
            context: ContextConfig::default(),
            subagent: SubagentConfig::default(),
        }
    }
}

/// The subagent statusline: one row per running subagent. Icons, colors, and
/// `color_mode` are shared with the main statusline.
#[derive(Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SubagentConfig {
    /// Template placed between rendered segments; may not use values.
    pub separator: String,
    /// A single row; each subagent gets one.
    pub segments: Vec<SegmentSpec>,
    pub context: ContextConfig,
}

impl Default for SubagentConfig {
    fn default() -> Self {
        Self {
            separator: DEFAULT_SEPARATOR.to_owned(),
            segments: DEFAULT_SUBAGENT_SEGMENTS
                .iter()
                .map(|&s| SegmentSpec::Format(s.to_owned()))
                .collect(),
            context: ContextConfig::default(),
        }
    }
}

const DEFAULT_SEPARATOR: &str = " [dim]│[/] ";

const DEFAULT_SEGMENTS: [&str; 5] = [
    "[bold]{model.display_name}[/]",
    "[cyan]{icon.branch} {git.branch}[/]",
    "[$context.level]{icon.context} {context.used_pct}[/]",
    "{icon.clock} {rate.five_hour.used_pct}",
    "{cost.total_usd}",
];

const DEFAULT_SUBAGENT_SEGMENTS: [&str; 4] = [
    "[bold]{agent.name}[/]",
    "{model.display_name}",
    "[$context.level]{icon.context} {context.used_pct}[/]",
    "{agent.elapsed}",
];

/// Plain Unicode so the default line works without a Nerd Font.
const DEFAULT_ICONS: [(&str, &str); 4] = [
    ("branch", "⎇"),
    ("clock", "↻"),
    ("context", "◔"),
    ("thinking", "✻"),
];

/// Context levels use ANSI indexes so they follow the terminal theme.
const DEFAULT_COLORS: [(&str, Color); 3] = [
    ("good", Color::ansi(2)),
    ("caution", Color::ansi(3)),
    ("danger", Color::ansi(1)),
];

/// Segments for a single line, or one array of segments per line.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(
    untagged,
    expecting = "an array of segments, or an array of arrays of segments (one per line)"
)]
pub enum Segments {
    Lines(Vec<Vec<SegmentSpec>>),
    Line(Vec<SegmentSpec>),
}

/// A segment: a bare template, or one shown only when `when` is truthy.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(untagged)]
pub enum SegmentSpec {
    Format(String),
    When { format: String, when: String },
}

/// A palette entry: `"#rrggbb"`, an ANSI index like `"3"`, a built-in name
/// like `"yellow"`, or `{ rgb = "#rrggbb", ansi = 3 }`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(untagged)]
pub enum ColorSpec {
    Name(String),
    Both {
        rgb: Option<String>,
        ansi: Option<u8>,
    },
}

impl ColorSpec {
    fn resolve(&self, builtins: &Palette) -> Option<Color> {
        match self {
            Self::Name(spec) => style::resolve_color(spec, builtins),
            Self::Both { rgb, ansi } => {
                let rgb = match rgb {
                    Some(hex) => Color::parse(hex)?.rgb,
                    None => None,
                };
                (rgb.is_some() || ansi.is_some()).then_some(Color { rgb, ansi: *ansi })
            }
        }
    }
}

struct Segment {
    template: Template,
    when: Option<&'static str>,
}

/// A validated, ready-to-render statusline layout.
pub struct Layout {
    mode: ColorMode,
    palette: Palette,
    separator: String,
    lines: Vec<Vec<Segment>>,
}

impl Layout {
    /// Renders every segment whose values are present, joined by the
    /// separator. Lines are joined by newlines; empty lines are dropped.
    pub fn render(&self, vars: &Vars) -> String {
        self.lines
            .iter()
            .map(|segments| {
                segments
                    .iter()
                    .filter(|s| s.when.is_none_or(|name| template::truthy(vars, name)))
                    .filter_map(|s| s.template.render(vars, &self.palette, self.mode))
                    .collect::<Vec<_>>()
                    .join(&self.separator)
            })
            .filter(|line| !line.is_empty())
            .collect::<Vec<_>>()
            .join("\n")
    }
}

impl Config {
    /// Validates the main statusline's templates, icons, and colors into a
    /// [`Layout`].
    pub fn compile(&self) -> Result<Layout> {
        let palette = self.palette()?;
        let icons = self.icons();
        let scope = Scope {
            icons: &icons,
            palette: &palette,
            names: vars::NAMES,
        };

        let separator = self.compile_separator(&self.separator, &scope, "separator")?;
        let lines = match &self.segments {
            Segments::Line(specs) => {
                vec![compile_line(specs, &scope, &|i| format!("segments[{i}]"))?]
            }
            Segments::Lines(lines) => lines
                .iter()
                .enumerate()
                .map(|(l, specs)| compile_line(specs, &scope, &|i| format!("segments[{l}][{i}]")))
                .collect::<Result<_>>()?,
        };

        Ok(Layout {
            mode: self.color_mode,
            palette,
            separator,
            lines,
        })
    }

    /// Validates the `[subagent]` templates into a single-line [`Layout`],
    /// rendered once per subagent.
    pub fn compile_subagent(&self) -> Result<Layout> {
        let palette = self.palette()?;
        let icons = self.icons();
        let scope = Scope {
            icons: &icons,
            palette: &palette,
            names: vars::SUBAGENT_NAMES,
        };

        let sub = &self.subagent;
        let separator = self.compile_separator(&sub.separator, &scope, "subagent.separator")?;
        let line = compile_line(&sub.segments, &scope, &|i| {
            format!("subagent.segments[{i}]")
        })?;

        Ok(Layout {
            mode: self.color_mode,
            palette,
            separator,
            lines: vec![line],
        })
    }

    /// Built-in colors and [`DEFAULT_COLORS`], with `[colors]` merged over.
    fn palette(&self) -> Result<Palette> {
        let builtins = style::builtin_palette();
        let mut palette = builtins.clone();
        palette.extend(DEFAULT_COLORS.map(|(name, color)| (name.to_owned(), color)));
        for (name, spec) in &self.colors {
            let color = spec
                .resolve(&builtins)
                .with_context(|| format!("invalid color for `colors.{name}`"))?;
            palette.insert(name.clone(), color);
        }
        Ok(palette)
    }

    /// [`DEFAULT_ICONS`] with `[icons]` merged over.
    fn icons(&self) -> BTreeMap<String, String> {
        let mut icons: BTreeMap<String, String> = DEFAULT_ICONS
            .iter()
            .map(|&(name, icon)| (name.to_owned(), icon.to_owned()))
            .collect();
        icons.extend(self.icons.clone());
        icons
    }

    /// Parses and pre-renders a separator, which may not use values.
    fn compile_separator(&self, source: &str, scope: &Scope<'_>, path: &str) -> Result<String> {
        let separator = Template::parse(source, scope).with_context(|| format!("in `{path}`"))?;
        if separator.uses_vars() {
            bail!("`{path}` cannot reference values");
        }
        Ok(separator
            .render(&Vars::new(), scope.palette, self.color_mode)
            .unwrap_or_default())
    }
}

fn compile_line(
    specs: &[SegmentSpec],
    scope: &Scope<'_>,
    path: &dyn Fn(usize) -> String,
) -> Result<Vec<Segment>> {
    specs
        .iter()
        .enumerate()
        .map(|(i, spec)| {
            let (format, when) = match spec {
                SegmentSpec::Format(format) => (format, None),
                SegmentSpec::When { format, when } => (format, Some(when)),
            };
            let segment = || -> Result<Segment> {
                Ok(Segment {
                    template: Template::parse(format, scope)?,
                    when: when
                        .map(|w| template::var_name(w, scope.names))
                        .transpose()?,
                })
            };
            segment().with_context(|| format!("in `{}`", path(i)))
        })
        .collect()
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
    /// Cells in the `context.bar.*` progress bar.
    pub bar_width: u8,
    /// Repeated once per used cell of the bar.
    pub bar_used: String,
    /// Repeated once per remaining cell of the bar.
    pub bar_remaining: String,
}

impl Default for ContextConfig {
    fn default() -> Self {
        Self {
            reserved_tokens: 20_000,
            caution_at: 76.0,
            danger_at: 86.0,
            bar_width: 10,
            bar_used: "━".to_owned(),
            bar_remaining: "─".to_owned(),
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

/// Loads `path`, or the default config file. Only the default file may be
/// missing, in which case built-in defaults are used.
pub fn load(path: Option<&Path>) -> Result<Config> {
    if let Some(path) = path {
        let contents =
            fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        return parse(&contents).with_context(|| format!("parsing {}", path.display()));
    }

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

/// Writes [`TEMPLATE`] to `path`, creating parent directories. Refuses to
/// replace an existing file unless `force` is set, in which case the existing
/// file is first copied to a timestamped backup. Returns the backup's path if
/// one was made.
pub fn write_template(path: &Path, force: bool) -> Result<Option<PathBuf>> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    }

    if force {
        let backup = backup_existing(path, &jiff::Zoned::now())?;
        fs::write(path, TEMPLATE).with_context(|| format!("writing {}", path.display()))?;
        return Ok(backup);
    }

    // `create_new` fails atomically if the file exists, so there's no window
    // between checking and writing.
    let mut file = match OpenOptions::new().write(true).create_new(true).open(path) {
        Ok(file) => file,
        Err(err) if err.kind() == io::ErrorKind::AlreadyExists => {
            bail!(
                "{} already exists; pass --force to overwrite",
                path.display()
            )
        }
        Err(err) => return Err(err).with_context(|| format!("creating {}", path.display())),
    };
    file.write_all(TEMPLATE.as_bytes())
        .with_context(|| format!("writing {}", path.display()))?;
    Ok(None)
}

/// Copies `path` to [`backup_path`], keeping its permissions. Skips the
/// backup if `path` is missing or already matches [`TEMPLATE`].
fn backup_existing(path: &Path, now: &jiff::Zoned) -> Result<Option<PathBuf>> {
    let contents = match fs::read(path) {
        Ok(contents) => contents,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err).with_context(|| format!("reading {}", path.display())),
    };
    if contents == TEMPLATE.as_bytes() {
        return Ok(None);
    }

    let backup = backup_path(path, now);
    // `create_new` so a backup made in the same second is never replaced.
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&backup)
        .with_context(|| format!("creating {}", backup.display()))?;
    file.write_all(&contents)
        .with_context(|| format!("writing {}", backup.display()))?;
    let permissions = fs::metadata(path)
        .with_context(|| format!("reading {}", path.display()))?
        .permissions();
    fs::set_permissions(&backup, permissions)
        .with_context(|| format!("setting permissions on {}", backup.display()))?;
    Ok(Some(backup))
}

/// Returns `path` with `.bak.<timestamp>` appended to its file name.
fn backup_path(path: &Path, now: &jiff::Zoned) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_owned();
    name.push(format!(".bak.{}", now.strftime("%Y%m%dT%H%M%S")));
    path.with_file_name(name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vars::Value;

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

    fn render(toml: &str, vars: &Vars) -> Result<String> {
        Ok(parse(toml)?.compile()?.render(vars))
    }

    #[test]
    fn default_layout_compiles() -> Result<()> {
        let vars = vec![
            ("model.display_name", Value::Text("Opus".into())),
            ("model.thinking.enabled", Value::Bool(true)),
        ];
        let line = parse("color_mode = \"none\"")?.compile()?.render(&vars);
        assert_eq!(line, "Opus │ ✻");
        Ok(())
    }

    #[test]
    fn default_subagent_layout_compiles() -> Result<()> {
        let vars = vec![
            ("agent.name", Value::Text("Explore".into())),
            ("agent.elapsed", Value::Duration(90)),
        ];
        let line = parse("color_mode = \"none\"")?
            .compile_subagent()?
            .render(&vars);
        assert_eq!(line, "Explore │ 1m");
        Ok(())
    }

    #[test]
    fn parses_subagent_table() -> Result<()> {
        let config = parse(
            r#"
            [subagent]
            separator = " · "
            segments = ["{agent.name}", { format = "{agent.status}", when = "agent.status" }]
            [subagent.context]
            reserved_tokens = 0
            "#,
        )?;
        assert_eq!(config.subagent.separator, " · ");
        assert_eq!(config.subagent.segments.len(), 2);
        assert_eq!(config.subagent.context.reserved_tokens, 0);
        assert!((config.subagent.context.danger_at - 86.0).abs() < f64::EPSILON);
        assert_eq!(config.context.reserved_tokens, 20_000);
        Ok(())
    }

    #[test]
    fn rejects_invalid_subagent_layouts() {
        for toml in [
            "[subagent]\nbogus = true",
            "[subagent.context]\nbogus = true",
            r#"subagent.segments = [["{agent.name}"]]"#,
            r#"subagent.segments = ["{git.branch}"]"#,
            r#"subagent.segments = [{ format = "x", when = "cost.total_usd" }]"#,
            r#"subagent.separator = "{agent.name}""#,
        ] {
            let result = parse(toml).and_then(|c| c.compile_subagent().map(|_| ()));
            assert!(result.is_err(), "{toml:?} should fail");
        }
    }

    #[test]
    fn subagent_values_are_rejected_in_main_segments() {
        assert!(
            parse(r#"segments = ["{agent.name}"]"#)
                .and_then(|c| c.compile().map(|_| ()))
                .is_err()
        );
    }

    #[test]
    fn subagent_segment_errors_name_the_index() -> Result<()> {
        let Err(err) = parse(r#"subagent.segments = ["ok", "{nope}"]"#)?.compile_subagent() else {
            anyhow::bail!("expected an error");
        };
        assert!(format!("{err:#}").contains("subagent.segments[1]"));
        Ok(())
    }

    #[test]
    fn parses_segment_forms() -> Result<()> {
        let config =
            parse(r#"segments = ["a", { format = "b", when = "model.thinking.enabled" }]"#)?;
        assert_eq!(
            config.segments,
            Segments::Line(vec![
                SegmentSpec::Format("a".into()),
                SegmentSpec::When {
                    format: "b".into(),
                    when: "model.thinking.enabled".into()
                },
            ])
        );

        let config = parse(r#"segments = [["a"], ["b", "c"]]"#)?;
        assert_eq!(
            config.segments,
            Segments::Lines(vec![
                vec![SegmentSpec::Format("a".into())],
                vec![
                    SegmentSpec::Format("b".into()),
                    SegmentSpec::Format("c".into())
                ],
            ])
        );
        Ok(())
    }

    #[test]
    fn renders_multiple_lines() -> Result<()> {
        let toml = r#"
            color_mode = "none"
            separator = " | "
            segments = [
                ["{model.display_name}", "a"],
                ["{git.branch}", { format = "T", when = "model.thinking.enabled" }],
                ["b"],
            ]
        "#;
        let vars = vec![("model.display_name", Value::Text("Opus".into()))];
        assert_eq!(render(toml, &vars)?, "Opus | a\nb");

        let vars = vec![("model.thinking.enabled", Value::Bool(true))];
        assert_eq!(render(toml, &vars)?, "a\nT\nb");
        Ok(())
    }

    #[test]
    fn drops_missing_and_false_segments() -> Result<()> {
        let toml = r#"
            color_mode = "none"
            separator = " | "
            segments = [
                "{model.display_name}",
                "{git.branch}",
                { format = "T", when = "model.thinking.enabled" },
                "end",
            ]
        "#;
        let vars = vec![
            ("model.display_name", Value::Text("Opus".into())),
            ("model.thinking.enabled", Value::Bool(false)),
        ];
        assert_eq!(render(toml, &vars)?, "Opus | end");
        Ok(())
    }

    #[test]
    fn user_icons_and_colors_merge_over_defaults() -> Result<()> {
        let toml = r##"
            segments = ["[accent]{icon.model}{icon.branch}[/]", "[good]x[/]"]
            separator = ""
            [icons]
            model = "M"
            [colors]
            accent = { rgb = "#010203", ansi = 4 }
            good = "bright_green"
        "##;
        assert_eq!(
            render(toml, &Vars::new())?,
            "\x1b[38;2;1;2;3mM⎇\x1b[0m\x1b[92mx\x1b[0m"
        );
        assert_eq!(
            render(&format!("color_mode = \"256\"\n{toml}"), &Vars::new())?,
            "\x1b[34mM⎇\x1b[0m\x1b[92mx\x1b[0m"
        );
        Ok(())
    }

    #[test]
    fn rejects_invalid_layouts() {
        for toml in [
            r#"segments = ["{nope}"]"#,
            r#"segments = ["{icon.nope}"]"#,
            r#"segments = [{ format = "x", when = "nope" }]"#,
            r#"segments = ["a", ["b"]]"#,
            r#"segments = [["{nope}"]]"#,
            r#"separator = "{model.display_name}""#,
            r#"colors.x = "blurple""#,
            r"colors.x = {}",
            r##"colors.x = { rgb = "#zzzzzz" }"##,
            r#"color_mode = "16""#,
        ] {
            let result = parse(toml).and_then(|c| c.compile().map(|_| ()));
            assert!(result.is_err(), "{toml:?} should fail");
        }
    }

    #[test]
    fn segment_errors_name_the_index() -> Result<()> {
        let Err(err) = parse(r#"segments = ["ok", "{nope}"]"#)?.compile() else {
            anyhow::bail!("expected an error");
        };
        assert!(format!("{err:#}").contains("segments[1]"));

        let Err(err) = parse(r#"segments = [["ok"], ["{nope}"]]"#)?.compile() else {
            anyhow::bail!("expected an error");
        };
        assert!(format!("{err:#}").contains("segments[1][0]"));
        Ok(())
    }

    /// A value for every name, so every default segment renders.
    fn all_vars(names: &'static [&'static str]) -> Vars {
        use crate::status::Level;

        names
            .iter()
            .map(|&name| {
                let value = match name {
                    "model.thinking.enabled" => Value::Bool(true),
                    "context.level" => Value::Level(Level::Caution),
                    "cost.total_usd" => Value::Usd(1.23),
                    "cost.today_usd" => Value::Usd(12.34),
                    n if n.ends_with("_pct") => Value::Percent(42.0),
                    n if n.ends_with("_in") => Value::Duration(3_900),
                    _ => Value::Text(name.to_owned()),
                };
                (name, value)
            })
            .collect()
    }

    #[test]
    fn template_matches_defaults() -> Result<()> {
        let full = all_vars(vars::NAMES);
        let sparse: Vars = full
            .iter()
            .filter(|(name, _)| !name.starts_with("git.") && !name.starts_with("cost."))
            .map(|(name, value)| match *name {
                "model.thinking.enabled" => (*name, Value::Bool(false)),
                _ => (*name, value.clone()),
            })
            .collect();

        for mode in ["256", "none"] {
            let mode_line = format!("color_mode = \"{mode}\"\n");
            let template = TEMPLATE.replace("color_mode = \"truecolor\"\n", &mode_line);
            let template = parse(&template)?.compile()?;
            let defaults = parse(&mode_line)?.compile()?;

            for vars in [&full, &sparse, &Vars::new()] {
                assert_eq!(template.render(vars), defaults.render(vars), "mode {mode}");
            }
        }
        Ok(())
    }

    #[test]
    fn template_matches_subagent_defaults() -> Result<()> {
        let full = all_vars(vars::SUBAGENT_NAMES);
        let sparse: Vars = full
            .iter()
            .filter(|(name, _)| !name.starts_with("context.") && !name.starts_with("model."))
            .cloned()
            .collect();

        for mode in ["256", "none"] {
            let mode_line = format!("color_mode = \"{mode}\"\n");
            let template = TEMPLATE.replace("color_mode = \"truecolor\"\n", &mode_line);
            let template = parse(&template)?.compile_subagent()?;
            let defaults = parse(&mode_line)?.compile_subagent()?;

            for vars in [&full, &sparse, &Vars::new()] {
                assert_eq!(template.render(vars), defaults.render(vars), "mode {mode}");
            }
        }
        Ok(())
    }

    #[test]
    fn template_documents_every_value() {
        for name in vars::NAMES.iter().chain(vars::SUBAGENT_NAMES) {
            assert!(TEMPLATE.contains(name), "{name} is not documented");
        }
    }

    #[test]
    fn write_template_refuses_overwrite() -> Result<()> {
        let dir = env::temp_dir().join(format!("jccs-test-{}", std::process::id()));
        let path = dir.join("nested").join("config.toml");

        let result = (|| -> Result<()> {
            write_template(&path, false)?;
            assert_eq!(fs::read_to_string(&path)?, TEMPLATE);

            fs::write(&path, "edited")?;
            let Err(err) = write_template(&path, false) else {
                bail!("expected an error");
            };
            assert!(format!("{err:#}").contains("--force"));
            assert_eq!(fs::read_to_string(&path)?, "edited");

            let Some(backup) = write_template(&path, true)? else {
                bail!("expected a backup");
            };
            assert_eq!(fs::read_to_string(&backup)?, "edited");
            assert_eq!(fs::read_to_string(&path)?, TEMPLATE);

            // Nothing to lose, so no second backup.
            assert_eq!(write_template(&path, true)?, None);
            Ok(())
        })();

        fs::remove_dir_all(&dir)?;
        result
    }

    #[test]
    fn backup_path_appends_timestamp() -> Result<()> {
        let now: jiff::Zoned = "2026-10-25T14:30:12[UTC]".parse()?;
        assert_eq!(
            backup_path(Path::new("/home/me/.config/jccs/config.toml"), &now),
            PathBuf::from("/home/me/.config/jccs/config.toml.bak.20261025T143012")
        );
        Ok(())
    }

    #[test]
    fn backup_refuses_to_replace_existing_backup() -> Result<()> {
        let dir = env::temp_dir().join(format!("jccs-backup-test-{}", std::process::id()));
        let path = dir.join("config.toml");
        let now: jiff::Zoned = "2026-10-25T14:30:12[UTC]".parse()?;

        let result = (|| -> Result<()> {
            fs::create_dir_all(&dir)?;
            fs::write(&path, "edited")?;
            fs::write(backup_path(&path, &now), "earlier")?;
            assert!(backup_existing(&path, &now).is_err());
            assert_eq!(fs::read_to_string(backup_path(&path, &now))?, "earlier");
            Ok(())
        })();

        fs::remove_dir_all(&dir)?;
        result
    }

    #[test]
    fn explicit_missing_path_is_an_error() {
        assert!(load(Some(Path::new("/nonexistent/jccs.toml"))).is_err());
    }

    #[test]
    fn missing_file_uses_defaults() {
        assert!(load_from(Path::new("/nonexistent/jccs/config.toml")).is_ok());
    }
}
