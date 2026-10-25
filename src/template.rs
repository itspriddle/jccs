//! A small markup language for statusline segments.
//!
//! - `{git.branch}` inserts a value; a missing value drops the segment.
//! - `{icon.branch}` inserts an `[icons]` entry.
//! - `[bold red on #222222]` pushes a style, `[/]` pops it.
//! - `[$context.level]` colors with the palette entry named by a value.
//! - `{{`, `}}`, `[[`, `]]` are literal braces and brackets.

use std::{collections::BTreeMap, fmt::Write as _};

use anyhow::{Result, bail};

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
        let mut nodes = Vec::new();
        let mut text = String::new();
        let mut depth = 0usize;
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

                    if !text.is_empty() {
                        nodes.push(Node::Text(std::mem::take(&mut text)));
                    }

                    if c == '{' {
                        nodes.push(parse_value(inner, scope)?);
                    } else if inner == "/" {
                        if depth == 0 {
                            bail!("`[/]` without a matching style");
                        }
                        depth -= 1;
                        nodes.push(Node::Pop);
                    } else {
                        depth += 1;
                        nodes.push(match inner.strip_prefix('$') {
                            Some(name) => Node::PushVar(var_name(name)?),
                            None => Node::Push(Style::parse(inner, scope.palette)?),
                        });
                    }
                }
                _ => text.push(c),
            }
        }

        if !text.is_empty() {
            nodes.push(Node::Text(text));
        }

        Ok(Self { nodes })
    }

    /// True when rendering depends on statusline values.
    pub fn uses_vars(&self) -> bool {
        self.nodes
            .iter()
            .any(|n| matches!(n, Node::Var(_) | Node::PushVar(_)))
    }

    /// Renders the template, or `None` when a referenced value is missing.
    pub fn render(&self, vars: &Vars, palette: &Palette, mode: ColorMode) -> Option<String> {
        let mut out = String::new();
        let mut stack: Vec<Style> = Vec::new();

        for node in &self.nodes {
            match node {
                Node::Text(text) => out.push_str(text),
                Node::Var(name) => {
                    let _ = write!(out, "{}", vars::lookup(vars, name)?);
                }
                Node::Push(style) => {
                    style.write_sgr(&mut out, mode);
                    stack.push(style.clone());
                }
                Node::PushVar(name) => {
                    let value = vars::lookup(vars, name)?.to_string();
                    let style = palette
                        .get(&value)
                        .copied()
                        .map(Style::fg)
                        .unwrap_or_default();
                    style.write_sgr(&mut out, mode);
                    stack.push(style);
                }
                Node::Pop => {
                    stack.pop();
                    if mode != ColorMode::None {
                        out.push_str(RESET);
                        for style in &stack {
                            style.write_sgr(&mut out, mode);
                        }
                    }
                }
            }
        }

        if !stack.is_empty() && mode != ColorMode::None {
            out.push_str(RESET);
        }

        Some(out)
    }
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

/// Whether a `when` condition holds: present and not `false`.
pub fn truthy(vars: &Vars, name: &str) -> bool {
    !matches!(vars::lookup(vars, name), None | Some(Value::Bool(false)))
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
            ("thinking.enabled", Value::Bool(false)),
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
        assert_eq!(render("x {model.name}", &sample(), ColorMode::None)?, None);
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
        ] {
            assert!(parse(source).is_err(), "{source:?} should fail");
        }
    }

    #[test]
    fn detects_var_usage() -> Result<()> {
        assert!(!parse("[dim]|[/] {icon.branch}")?.uses_vars());
        assert!(parse("{git.branch}")?.uses_vars());
        assert!(parse("[$context.level]")?.uses_vars());
        Ok(())
    }

    #[test]
    fn when_is_truthy_unless_missing_or_false() {
        let vars = sample();
        assert!(truthy(&vars, "git.branch"));
        assert!(!truthy(&vars, "thinking.enabled"));
        assert!(!truthy(&vars, "model.name"));
    }
}
