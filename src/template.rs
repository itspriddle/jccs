//! A small markup language for statusline segments.
//!
//! - `{git.branch}` inserts a value; a missing value drops the segment.
//! - `{icon.branch}` inserts an `[icons]` entry.
//! - `[bold red on #222222]` pushes a style, `[/]` pops it.
//! - `[$context.level]` colors with the palette entry named by a value.
//! - `{?lines.added}…{/}` renders its contents only when the value is
//!   truthy; a missing value inside drops just the group.
//! - `{{`, `}}`, `[[`, `]]` are literal braces and brackets.

use std::{collections::BTreeMap, fmt::Write as _};

use anyhow::{Result, anyhow, bail};

use crate::{
    style::{ColorMode, Palette, RESET, Style},
    vars::{self, Value, Vars},
};

#[derive(Debug, Clone, PartialEq, Eq)]
enum Node {
    Text(String),
    Var(&'static str),
    Push(Style),
    /// Foreground color from the palette entry named by a value.
    PushVar(&'static str),
    Pop,
    /// Contents shown only when the named value is truthy.
    Group {
        when: &'static str,
        nodes: Vec<Node>,
    },
}

/// A construct opened while parsing and awaiting its closing tag.
enum Open {
    Style,
    Group(&'static str),
}

/// Everything a template may reference, fixed at config load.
pub struct Scope<'a> {
    pub icons: &'a BTreeMap<String, String>,
    pub palette: &'a Palette,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Template {
    nodes: Vec<Node>,
}

impl Template {
    pub fn parse(source: &str, scope: &Scope<'_>) -> Result<Self> {
        // `levels[0]` is the top level; each open group adds one.
        let mut levels: Vec<Vec<Node>> = vec![Vec::new()];
        let mut open: Vec<Open> = Vec::new();
        let mut text = String::new();
        let mut chars = source.chars().peekable();

        while let Some(c) = chars.next() {
            match c {
                '{' | '[' | '}' | ']' if chars.peek() == Some(&c) => {
                    chars.next();
                    text.push(c);
                }
                '}' | ']' => bail!("unmatched `{c}`; write `{c}{c}` for a literal"),
                '{' | '[' => {
                    let close = if c == '{' { '}' } else { ']' };
                    let mut inner = String::new();
                    loop {
                        match chars.next() {
                            Some(ch) if ch == close => break,
                            Some(ch) => inner.push(ch),
                            None => bail!("unterminated `{c}`"),
                        }
                    }
                    let inner = inner.trim();

                    let nodes = levels
                        .last_mut()
                        .ok_or_else(|| anyhow!("unbalanced group"))?;
                    if !text.is_empty() {
                        nodes.push(Node::Text(std::mem::take(&mut text)));
                    }

                    if c == '{' {
                        if inner == "/" {
                            let Some(Open::Group(when)) = open.pop() else {
                                bail!(
                                    "`{{/}}` without a matching `{{?…}}`, or a style left open inside it"
                                );
                            };
                            let nodes = levels.pop().ok_or_else(|| anyhow!("unbalanced group"))?;
                            levels
                                .last_mut()
                                .ok_or_else(|| anyhow!("unbalanced group"))?
                                .push(Node::Group { when, nodes });
                        } else if let Some(name) = inner.strip_prefix('?') {
                            open.push(Open::Group(var_name(name.trim())?));
                            levels.push(Vec::new());
                        } else {
                            nodes.push(parse_value(inner, scope)?);
                        }
                    } else if inner == "/" {
                        if !matches!(open.pop(), Some(Open::Style)) {
                            bail!("`[/]` without a matching style");
                        }
                        nodes.push(Node::Pop);
                    } else {
                        open.push(Open::Style);
                        nodes.push(match inner.strip_prefix('$') {
                            Some(name) => Node::PushVar(var_name(name)?),
                            None => Node::Push(Style::parse(inner, scope.palette)?),
                        });
                    }
                }
                _ => text.push(c),
            }
        }

        if open.iter().any(|o| matches!(o, Open::Group(_))) {
            bail!("unterminated `{{?…}}`; close it with `{{/}}`");
        }

        let mut nodes = levels.pop().ok_or_else(|| anyhow!("unbalanced group"))?;
        if !text.is_empty() {
            nodes.push(Node::Text(text));
        }

        Ok(Self { nodes })
    }

    /// True when rendering depends on statusline values.
    pub fn uses_vars(&self) -> bool {
        uses_vars(&self.nodes)
    }

    /// Renders the template, or `None` when a referenced value is missing.
    pub fn render(&self, vars: &Vars, palette: &Palette, mode: ColorMode) -> Option<String> {
        let mut out = String::new();
        let mut stack: Vec<Style> = Vec::new();

        render_nodes(&self.nodes, vars, palette, mode, &mut out, &mut stack)?;

        if !stack.is_empty() && mode != ColorMode::None {
            out.push_str(RESET);
        }

        Some(out)
    }
}

fn uses_vars(nodes: &[Node]) -> bool {
    nodes
        .iter()
        .any(|n| matches!(n, Node::Var(_) | Node::PushVar(_) | Node::Group { .. }))
}

/// Appends `nodes` to `out`, or returns `None` when a value is missing.
/// On `None`, `out` and `stack` may hold partial output.
fn render_nodes(
    nodes: &[Node],
    vars: &Vars,
    palette: &Palette,
    mode: ColorMode,
    out: &mut String,
    stack: &mut Vec<Style>,
) -> Option<()> {
    for node in nodes {
        match node {
            Node::Text(text) => out.push_str(text),
            Node::Var(name) => {
                let _ = write!(out, "{}", vars::lookup(vars, name)?);
            }
            Node::Push(style) => {
                style.write_sgr(out, mode);
                stack.push(style.clone());
            }
            Node::PushVar(name) => {
                let value = vars::lookup(vars, name)?.to_string();
                let style = palette
                    .get(&value)
                    .copied()
                    .map(Style::fg)
                    .unwrap_or_default();
                style.write_sgr(out, mode);
                stack.push(style);
            }
            Node::Pop => {
                stack.pop();
                if mode != ColorMode::None {
                    out.push_str(RESET);
                    for style in stack.iter() {
                        style.write_sgr(out, mode);
                    }
                }
            }
            Node::Group { when, nodes } => {
                if !truthy(vars, when) {
                    continue;
                }
                // Styles are balanced inside a group, so on success `stack`
                // is unchanged; on failure, roll back to before the group.
                let (len, depth) = (out.len(), stack.len());
                if render_nodes(nodes, vars, palette, mode, out, stack).is_none() {
                    out.truncate(len);
                    stack.truncate(depth);
                }
            }
        }
    }
    Some(())
}

fn parse_value(inner: &str, scope: &Scope<'_>) -> Result<Node> {
    match inner.strip_prefix("icon.") {
        Some(name) => match scope.icons.get(name) {
            Some(icon) => Ok(Node::Text(icon.clone())),
            None => bail!("unknown icon `{name}`; define it under [icons]"),
        },
        None => Ok(Node::Var(var_name(inner)?)),
    }
}

/// Resolves a value name to its `'static` form, rejecting unknown names.
pub fn var_name(name: &str) -> Result<&'static str> {
    match vars::NAMES.iter().find(|&&n| n == name) {
        Some(&n) => Ok(n),
        None => bail!("unknown value `{name}`; run `jccs vars` to list values"),
    }
}

/// Whether a `when` or `{?…}` condition holds: present, not `false`, not
/// zero, and not empty.
pub fn truthy(vars: &Vars, name: &str) -> bool {
    match vars::lookup(vars, name) {
        None | Some(Value::Bool(false) | Value::Int(0) | Value::Duration(0)) => false,
        Some(Value::Usd(n) | Value::Percent(n)) => *n != 0.0,
        Some(Value::Text(s)) => !s.is_empty(),
        Some(_) => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{status::Level, style::builtin_palette};

    fn parse(source: &str) -> Result<Template> {
        let icons = BTreeMap::from([("branch".to_owned(), "*".to_owned())]);
        let palette = builtin_palette();
        Template::parse(
            source,
            &Scope {
                icons: &icons,
                palette: &palette,
            },
        )
    }

    fn render(source: &str, vars: &Vars, mode: ColorMode) -> Result<Option<String>> {
        let mut palette = builtin_palette();
        palette.insert("caution".into(), crate::style::Color::ansi(3));
        Ok(parse(source)?.render(vars, &palette, mode))
    }

    fn sample() -> Vars {
        vec![
            ("git.branch", Value::Text("main".into())),
            ("context.level", Value::Level(Level::Caution)),
            ("model.thinking.enabled", Value::Bool(false)),
        ]
    }

    #[test]
    fn renders_text_values_and_icons() -> Result<()> {
        assert_eq!(
            render("{icon.branch} {git.branch}", &sample(), ColorMode::None)?,
            Some("* main".into())
        );
        Ok(())
    }

    #[test]
    fn missing_value_drops_template() -> Result<()> {
        assert_eq!(
            render("x {model.display_name}", &sample(), ColorMode::None)?,
            None
        );
        assert_eq!(
            render("[$cost.total_usd]x", &sample(), ColorMode::None)?,
            None
        );
        Ok(())
    }

    #[test]
    fn handles_escapes() -> Result<()> {
        assert_eq!(
            render("{{a}} [[b]]", &sample(), ColorMode::TrueColor)?,
            Some("{a} [b]".into())
        );
        Ok(())
    }

    #[test]
    fn nested_styles_reapply_on_pop() -> Result<()> {
        assert_eq!(
            render("[bold]a[red]b[/]c[/]d", &sample(), ColorMode::TrueColor)?,
            Some("\x1b[1ma\x1b[31mb\x1b[0m\x1b[1mc\x1b[0md".into())
        );
        Ok(())
    }

    #[test]
    fn unclosed_style_is_reset() -> Result<()> {
        assert_eq!(
            render("[red]a", &sample(), ColorMode::TrueColor)?,
            Some("\x1b[31ma\x1b[0m".into())
        );
        Ok(())
    }

    #[test]
    fn dynamic_style_uses_value_as_palette_name() -> Result<()> {
        assert_eq!(
            render("[$context.level]x[/]", &sample(), ColorMode::TrueColor)?,
            Some("\x1b[33mx\x1b[0m".into())
        );
        let vars = vec![("context.level", Value::Text("nope".into()))];
        assert_eq!(
            render("[$context.level]x[/]", &vars, ColorMode::TrueColor)?,
            Some("x\x1b[0m".into())
        );
        Ok(())
    }

    #[test]
    fn none_mode_writes_no_escapes() -> Result<()> {
        assert_eq!(
            render(
                "[bold red]a[/][$context.level]b",
                &sample(),
                ColorMode::None
            )?,
            Some("ab".into())
        );
        Ok(())
    }

    #[test]
    fn rejects_bad_templates() {
        for source in [
            "{nope}",
            "{icon.nope}",
            "[blurple]",
            "[/]",
            "[red][/][/]",
            "{git.branch",
            "[red",
            "{git.branch}{git.branch",
            "a}",
            "a]",
            "[$nope]",
            "{?nope}x{/}",
            "{?git.branch}x",
            "{/}",
            "{?git.branch}[red]x{/}",
            "[red]{?git.branch}x[/]{/}",
        ] {
            assert!(parse(source).is_err(), "{source:?} should fail");
        }
    }

    #[test]
    fn detects_var_usage() -> Result<()> {
        assert!(!parse("[dim]|[/] {icon.branch}")?.uses_vars());
        assert!(parse("{git.branch}")?.uses_vars());
        assert!(parse("[$context.level]")?.uses_vars());
        assert!(parse("{?git.branch}x{/}")?.uses_vars());
        Ok(())
    }

    #[test]
    fn when_is_truthy_unless_missing_false_zero_or_empty() {
        let vars = vec![
            ("git.branch", Value::Text("main".into())),
            ("git.repo", Value::Text(String::new())),
            ("model.thinking.enabled", Value::Bool(false)),
            ("lines.added", Value::Int(3)),
            ("lines.removed", Value::Int(0)),
            ("cost.total_usd", Value::Usd(0.0)),
            ("context.used_pct", Value::Percent(0.5)),
        ];
        assert!(truthy(&vars, "git.branch"));
        assert!(truthy(&vars, "lines.added"));
        assert!(truthy(&vars, "context.used_pct"));
        assert!(!truthy(&vars, "git.repo"));
        assert!(!truthy(&vars, "model.thinking.enabled"));
        assert!(!truthy(&vars, "lines.removed"));
        assert!(!truthy(&vars, "cost.total_usd"));
        assert!(!truthy(&vars, "model.display_name"));
    }

    #[test]
    fn group_renders_only_when_truthy() -> Result<()> {
        let source = "d{?lines.added} +{lines.added}{/}{?lines.removed} -{lines.removed}{/}";
        let lines = |added, removed| {
            vec![
                ("lines.added", Value::Int(added)),
                ("lines.removed", Value::Int(removed)),
            ]
        };
        assert_eq!(
            render(source, &lines(3, 1), ColorMode::None)?,
            Some("d +3 -1".into())
        );
        assert_eq!(
            render(source, &lines(3, 0), ColorMode::None)?,
            Some("d +3".into())
        );
        assert_eq!(
            render(source, &lines(0, 0), ColorMode::None)?,
            Some("d".into())
        );
        Ok(())
    }

    #[test]
    fn missing_value_in_group_drops_only_the_group() -> Result<()> {
        assert_eq!(
            render(
                "{git.branch}{?git.branch}[red] {model.display_name}[/]{/}!",
                &sample(),
                ColorMode::TrueColor
            )?,
            Some("main!".into())
        );
        Ok(())
    }

    #[test]
    fn groups_nest_with_styles() -> Result<()> {
        assert_eq!(
            render(
                "[bold]{?git.branch}[red]{git.branch}[/]{/}x[/]",
                &sample(),
                ColorMode::TrueColor
            )?,
            Some("\x1b[1m\x1b[31mmain\x1b[0m\x1b[1mx\x1b[0m".into())
        );
        Ok(())
    }
}
