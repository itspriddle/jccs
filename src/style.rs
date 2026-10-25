//! Colors, text styles, and the SGR escapes that render them.

use std::collections::BTreeMap;

use anyhow::{Result, bail};
use serde::Deserialize;

pub const RESET: &str = "\x1b[0m";

/// How colors are written to the terminal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, clap::ValueEnum)]
pub enum ColorMode {
    /// 24-bit `38;2;r;g;b` escapes, falling back to the ANSI index.
    #[default]
    #[serde(rename = "truecolor")]
    #[value(name = "truecolor")]
    TrueColor,
    /// ANSI 256-color palette; RGB colors are approximated.
    #[serde(rename = "256")]
    #[value(name = "256")]
    Ansi256,
    /// No escapes at all.
    #[serde(rename = "none")]
    #[value(name = "none")]
    None,
}

/// A color with an RGB value, an ANSI palette index, or both. The color mode
/// decides which is used.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Color {
    pub rgb: Option<[u8; 3]>,
    pub ansi: Option<u8>,
}

impl Color {
    pub const fn ansi(index: u8) -> Self {
        Self {
            rgb: None,
            ansi: Some(index),
        }
    }

    /// Parses `#rrggbb` or an ANSI index `0`–`255`.
    pub fn parse(spec: &str) -> Option<Self> {
        if let Some(hex) = spec.strip_prefix('#') {
            return parse_hex(hex).map(|rgb| Self {
                rgb: Some(rgb),
                ansi: None,
            });
        }
        spec.parse().ok().map(Self::ansi)
    }

    fn sgr(self, mode: ColorMode, bg: bool) -> Option<String> {
        let index = match (mode, self.rgb, self.ansi) {
            (ColorMode::None, ..) | (_, None, None) => return None,
            (ColorMode::TrueColor, Some([r, g, b]), _) => {
                let base = if bg { 48 } else { 38 };
                return Some(format!("{base};2;{r};{g};{b}"));
            }
            (_, _, Some(index)) => index,
            (ColorMode::Ansi256, Some(rgb), None) => rgb_to_256(rgb),
        };

        let (normal, bright, extended) = if bg { (40, 100, 48) } else { (30, 90, 38) };
        Some(match index {
            0..=7 => format!("{}", normal + index),
            8..=15 => format!("{}", bright + index - 8),
            _ => format!("{extended};5;{index}"),
        })
    }
}

fn parse_hex(hex: &str) -> Option<[u8; 3]> {
    if hex.len() != 6 || !hex.is_ascii() {
        return None;
    }
    let channel = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).ok();
    Some([channel(0)?, channel(2)?, channel(4)?])
}

/// Nearest entry in the 6×6×6 color cube or the grayscale ramp.
fn rgb_to_256([r, g, b]: [u8; 3]) -> u8 {
    const LEVELS: [u8; 6] = [0, 95, 135, 175, 215, 255];

    let nearest_level = |c: u8| {
        LEVELS
            .iter()
            .enumerate()
            .min_by_key(|&(_, &l)| c.abs_diff(l))
            .map_or(0, |(i, _)| i)
    };
    let dist = |[r2, g2, b2]: [u8; 3]| {
        [r.abs_diff(r2), g.abs_diff(g2), b.abs_diff(b2)]
            .iter()
            .map(|&d| u32::from(d).pow(2))
            .sum::<u32>()
    };

    let (ri, gi, bi) = (nearest_level(r), nearest_level(g), nearest_level(b));
    let cube = [LEVELS[ri], LEVELS[gi], LEVELS[bi]];
    let cube_index = 16 + 36 * ri + 6 * gi + bi;

    // Grayscale ramp 232–255 covers 8, 18, …, 238.
    let avg = (u32::from(r) + u32::from(g) + u32::from(b)) / 3;
    let gray_step = (avg.saturating_sub(3) / 10).min(23);
    let gray = u8::try_from(8 + 10 * gray_step).unwrap_or(238);

    if dist([gray; 3]) < dist(cube) {
        u8::try_from(232 + gray_step).unwrap_or(255)
    } else {
        u8::try_from(cube_index).unwrap_or(231)
    }
}

/// Named colors available to templates and the dynamic `[$var]` style.
pub type Palette = BTreeMap<String, Color>;

const BUILTIN_COLORS: [&str; 16] = [
    "black",
    "red",
    "green",
    "yellow",
    "blue",
    "magenta",
    "cyan",
    "white",
    "bright_black",
    "bright_red",
    "bright_green",
    "bright_yellow",
    "bright_blue",
    "bright_magenta",
    "bright_cyan",
    "bright_white",
];

/// The 16 standard ANSI color names mapped to their palette indexes.
pub fn builtin_palette() -> Palette {
    (0u8..)
        .zip(BUILTIN_COLORS)
        .map(|(index, name)| (name.to_owned(), Color::ansi(index)))
        .collect()
}

/// Resolves a palette name, `#rrggbb`, or ANSI index.
pub fn resolve_color(spec: &str, palette: &Palette) -> Option<Color> {
    palette.get(spec).copied().or_else(|| Color::parse(spec))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Modifier {
    Bold,
    Dim,
    Italic,
    Underline,
    Reverse,
    Strike,
}

impl Modifier {
    fn parse(name: &str) -> Option<Self> {
        Some(match name {
            "bold" => Self::Bold,
            "dim" => Self::Dim,
            "italic" => Self::Italic,
            "underline" => Self::Underline,
            "reverse" => Self::Reverse,
            "strike" => Self::Strike,
            _ => return None,
        })
    }

    fn sgr(self) -> &'static str {
        match self {
            Self::Bold => "1",
            Self::Dim => "2",
            Self::Italic => "3",
            Self::Underline => "4",
            Self::Reverse => "7",
            Self::Strike => "9",
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Style {
    fg: Option<Color>,
    bg: Option<Color>,
    modifiers: Vec<Modifier>,
}

impl Style {
    pub fn fg(color: Color) -> Self {
        Self {
            fg: Some(color),
            ..Self::default()
        }
    }

    /// Parses a space-separated style such as `bold red on #222222`.
    pub fn parse(spec: &str, palette: &Palette) -> Result<Self> {
        let mut style = Self::default();
        let mut words = spec.split_whitespace();

        while let Some(word) = words.next() {
            if word == "on" {
                let Some(name) = words.next() else {
                    bail!("missing color after `on` in `[{spec}]`");
                };
                let Some(color) = resolve_color(name, palette) else {
                    bail!("unknown color `{name}` in `[{spec}]`");
                };
                style.bg = Some(color);
            } else if let Some(modifier) = Modifier::parse(word) {
                style.modifiers.push(modifier);
            } else if let Some(color) = resolve_color(word, palette) {
                if style.fg.is_some() {
                    bail!("more than one color in `[{spec}]`");
                }
                style.fg = Some(color);
            } else {
                bail!("unknown color or style `{word}` in `[{spec}]`");
            }
        }

        Ok(style)
    }

    /// Appends the escape that turns this style on; nothing in `None` mode.
    pub fn write_sgr(&self, out: &mut String, mode: ColorMode) {
        if mode == ColorMode::None {
            return;
        }

        let codes: Vec<String> = self
            .modifiers
            .iter()
            .map(|m| m.sgr().to_owned())
            .chain(self.fg.and_then(|c| c.sgr(mode, false)))
            .chain(self.bg.and_then(|c| c.sgr(mode, true)))
            .collect();

        if !codes.is_empty() {
            out.push_str("\x1b[");
            out.push_str(&codes.join(";"));
            out.push('m');
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sgr(spec: &str, mode: ColorMode) -> Result<String> {
        let mut out = String::new();
        Style::parse(spec, &builtin_palette())?.write_sgr(&mut out, mode);
        Ok(out)
    }

    #[test]
    fn parses_colors() {
        assert_eq!(
            Color::parse("#7aa2f7"),
            Some(Color {
                rgb: Some([0x7a, 0xa2, 0xf7]),
                ansi: None
            })
        );
        assert_eq!(Color::parse("208"), Some(Color::ansi(208)));
        assert_eq!(Color::parse("#12345"), None);
        assert_eq!(Color::parse("256"), None);
        assert_eq!(Color::parse("blue"), None);
        assert_eq!(
            resolve_color("bright_blue", &builtin_palette()),
            Some(Color::ansi(12))
        );
    }

    #[test]
    fn writes_sgr_by_mode() -> Result<()> {
        assert_eq!(sgr("red", ColorMode::TrueColor)?, "\x1b[31m");
        assert_eq!(sgr("bright_red", ColorMode::Ansi256)?, "\x1b[91m");
        assert_eq!(sgr("208", ColorMode::TrueColor)?, "\x1b[38;5;208m");
        assert_eq!(
            sgr("#ff8000", ColorMode::TrueColor)?,
            "\x1b[38;2;255;128;0m"
        );
        assert_eq!(sgr("#ff8700", ColorMode::Ansi256)?, "\x1b[38;5;208m");
        assert_eq!(
            sgr("bold blue on #222222", ColorMode::TrueColor)?,
            "\x1b[1;34;48;2;34;34;34m"
        );
        assert_eq!(sgr("on 4", ColorMode::Ansi256)?, "\x1b[44m");
        assert_eq!(sgr("bold red", ColorMode::None)?, "");
        assert_eq!(sgr("", ColorMode::TrueColor)?, "");
        Ok(())
    }

    #[test]
    fn prefers_ansi_index_in_256_mode() {
        let color = Color {
            rgb: Some([1, 2, 3]),
            ansi: Some(3),
        };
        assert_eq!(color.sgr(ColorMode::Ansi256, false).as_deref(), Some("33"));
        assert_eq!(
            color.sgr(ColorMode::TrueColor, false).as_deref(),
            Some("38;2;1;2;3")
        );
    }

    #[test]
    fn converts_rgb_to_256() {
        assert_eq!(rgb_to_256([0, 0, 0]), 16);
        assert_eq!(rgb_to_256([255, 255, 255]), 231);
        assert_eq!(rgb_to_256([255, 0, 0]), 196);
        assert_eq!(rgb_to_256([128, 128, 128]), 244);
        assert_eq!(rgb_to_256([0x22, 0x22, 0x22]), 235);
    }

    #[test]
    fn rejects_bad_styles() {
        let palette = builtin_palette();
        assert!(Style::parse("blurple", &palette).is_err());
        assert!(Style::parse("red blue", &palette).is_err());
        assert!(Style::parse("red on", &palette).is_err());
        assert!(Style::parse("on nope", &palette).is_err());
    }
}
