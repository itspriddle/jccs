use std::{
    collections::BTreeMap,
    env, fs, io,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use serde::Deserialize;

use crate::{
    style::{self, Color, ColorMode, Palette},
    template::{self, Scope, Template},
    vars::Vars,
};

/// User configuration loaded from `~/.config/jccs/config.toml`.
#[derive(Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub color_mode: ColorMode,
    /// Template placed between rendered segments; may not use values.
    pub separator: String,
    pub segments: Vec<SegmentSpec>,
    /// Merged over [`DEFAULT_ICONS`]; set an icon to `""` to remove it.
    pub icons: BTreeMap<String, String>,
    /// Merged over the built-in color names and [`DEFAULT_COLORS`].
    pub colors: BTreeMap<String, ColorSpec>,
    pub context: ContextConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            color_mode: ColorMode::default(),
            separator: DEFAULT_SEPARATOR.to_owned(),
            segments: DEFAULT_SEGMENTS
                .iter()
                .map(|&s| SegmentSpec::Format(s.to_owned()))
                .chain([SegmentSpec::When {
                    format: "[yellow]{icon.thinking}[/]".to_owned(),
                    when: "thinking.enabled".to_owned(),
                }])
                .collect(),
            icons: BTreeMap::new(),
            colors: BTreeMap::new(),
            context: ContextConfig::default(),
        }
    }
}

const DEFAULT_SEPARATOR: &str = " [dim]│[/] ";

const DEFAULT_SEGMENTS: [&str; 5] = [
    "[bold]{model.name}[/]",
    "[cyan]{icon.branch} {git.branch}[/]",
    "[$context.level]{icon.context} {context.used_pct}[/]",
    "{icon.clock} {rate.five_hour.used_pct}",
    "{cost.total_usd}",
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
    segments: Vec<Segment>,
}

impl Layout {
    /// Renders every segment whose values are present, joined by the separator.
    pub fn render(&self, vars: &Vars) -> String {
        self.segments
            .iter()
            .filter(|s| s.when.is_none_or(|name| template::truthy(vars, name)))
            .filter_map(|s| s.template.render(vars, &self.palette, self.mode))
            .collect::<Vec<_>>()
            .join(&self.separator)
    }
}

impl Config {
    /// Validates templates, icons, and colors into a [`Layout`].
    pub fn compile(&self) -> Result<Layout> {
        let builtins = style::builtin_palette();
        let mut palette = builtins.clone();
        palette.extend(DEFAULT_COLORS.map(|(name, color)| (name.to_owned(), color)));
        for (name, spec) in &self.colors {
            let color = spec
                .resolve(&builtins)
                .with_context(|| format!("invalid color for `colors.{name}`"))?;
            palette.insert(name.clone(), color);
        }

        let mut icons: BTreeMap<String, String> = DEFAULT_ICONS
            .iter()
            .map(|&(name, icon)| (name.to_owned(), icon.to_owned()))
            .collect();
        icons.extend(self.icons.clone());

        let scope = Scope {
            icons: &icons,
            palette: &palette,
        };

        let separator = Template::parse(&self.separator, &scope).context("in `separator`")?;
        if separator.uses_vars() {
            bail!("`separator` cannot reference values");
        }
        let separator = separator
            .render(&Vars::new(), &palette, self.color_mode)
            .unwrap_or_default();

        let segments = self
            .segments
            .iter()
            .enumerate()
            .map(|(i, spec)| {
                let (format, when) = match spec {
                    SegmentSpec::Format(format) => (format, None),
                    SegmentSpec::When { format, when } => (format, Some(when)),
                };
                let segment = || -> Result<Segment> {
                    Ok(Segment {
                        template: Template::parse(format, &scope)?,
                        when: when.map(|w| template::var_name(w)).transpose()?,
                    })
                };
                segment().with_context(|| format!("in `segments[{i}]`"))
            })
            .collect::<Result<_>>()?;

        Ok(Layout {
            mode: self.color_mode,
            palette,
            separator,
            segments,
        })
    }
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
            ("model.name", Value::Text("Opus".into())),
            ("thinking.enabled", Value::Bool(true)),
        ];
        let line = parse("color_mode = \"none\"")?.compile()?.render(&vars);
        assert_eq!(line, "Opus │ ✻");
        Ok(())
    }

    #[test]
    fn parses_segment_forms() -> Result<()> {
        let config = parse(r#"segments = ["a", { format = "b", when = "thinking.enabled" }]"#)?;
        assert_eq!(
            config.segments,
            [
                SegmentSpec::Format("a".into()),
                SegmentSpec::When {
                    format: "b".into(),
                    when: "thinking.enabled".into()
                },
            ]
        );
        Ok(())
    }

    #[test]
    fn drops_missing_and_false_segments() -> Result<()> {
        let toml = r#"
            color_mode = "none"
            separator = " | "
            segments = [
                "{model.name}",
                "{git.branch}",
                { format = "T", when = "thinking.enabled" },
                "end",
            ]
        "#;
        let vars = vec![
            ("model.name", Value::Text("Opus".into())),
            ("thinking.enabled", Value::Bool(false)),
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
            r#"separator = "{model.name}""#,
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
        Ok(())
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
