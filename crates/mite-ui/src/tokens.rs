//! Design tokens, read from `docs/design/tokens.json` (schema `mitefx-tokens/3`, extracted
//! verbatim from the Claude Design tokens board).
//!
//! The JSON is embedded at compile time and parsed when a window opens. Only the values the
//! widgets use are pulled out; the file stays the single source of truth.

use serde_json::Value;
use vello::peniko::Color;

const TOKENS_JSON: &str = include_str!("../../../docs/design/tokens.json");

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ThemeKind {
    #[default]
    Dark,
    Light,
    HighContrast,
    ColorBlind,
}

impl ThemeKind {
    /// Key used for ramp, semantic and accent colours.
    fn key(self) -> &'static str {
        match self {
            Self::Dark => "dark",
            Self::Light => "light",
            Self::HighContrast => "hc",
            Self::ColorBlind => "cb",
        }
    }

    /// Key used for neutrals: the colour-blind theme shares the dark neutrals.
    fn neutral_key(self) -> &'static str {
        match self {
            Self::ColorBlind => "dark",
            other => other.key(),
        }
    }
}

/// One resolved theme for one plugin (the accent is per plugin).
#[derive(Debug, Clone)]
pub struct Theme {
    pub kind: ThemeKind,
    pub window: Color,
    pub display: Color,
    pub help_strip: Color,
    pub line: Color,
    pub control: Color,
    pub control_border: Color,
    pub well: Color,
    pub well_border: Color,
    pub knob_disc: Color,
    pub knob_disc_border: Color,
    pub tick_off: Color,
    pub needle: Color,
    pub grid_major: Color,
    pub grid_minor: Color,
    pub grid_zero: Color,
    pub curve: Color,
    pub spectrum_pre: Color,
    /// Second stop of the post-spectrum gradient (first stop is the accent).
    pub spectrum_post_fill: Color,
    pub glass: Color,
    pub glass_border: Color,
    pub panel_control: Color,
    pub panel_control_border: Color,
    pub text_disabled: Color,
    pub text_hi: Color,
    pub text_mid: Color,
    pub text_lo: Color,
    pub text_strong: Color,
    pub accent: Color,
    pub accent_text: Color,
    pub meter_off: Color,
    pub meter_well: Color,
    pub warn: Color,
    pub danger: Color,
    pub gain_reduction: Color,
    pub band_ramp: Vec<Color>,
    /// Drop shadows and glow are off in high contrast (`elevation.*.hc` = none).
    pub shadows: bool,
    /// Control border width: 1 px, 1.5 px in high contrast.
    pub border_width: f64,
    /// Dial disc drop-shadow opacity (`elevation.dialDisc`).
    pub disc_shadow_alpha: f32,
}

impl Theme {
    pub fn dark(accent: &str) -> Self {
        Self::load(ThemeKind::Dark, accent)
    }

    /// Loads `kind` with the accent of `plugin` (`eq`, `comp`, … `utility`, as in
    /// `color.accent`). Panics only if the committed tokens file is malformed or the plugin key
    /// is unknown; the `tokens_parse` test covers every theme and accent.
    pub fn load(kind: ThemeKind, plugin: &str) -> Self {
        let root: Value = serde_json::from_str(TOKENS_JSON).expect("docs/design/tokens.json is valid JSON");
        let colors = &root["color"];
        let n = &colors["neutral"][kind.neutral_key()];
        let t = kind.key();
        let neutral = |name: &str| color(&n[name], &format!("neutral.{}.{name}", kind.neutral_key()));
        let semantic = |name: &str| color(&colors["semantic"][name][t], &format!("semantic.{name}.{t}"));
        let accent = color(&colors["accent"][plugin][t], &format!("accent.{plugin}.{t}"));
        let band_ramp = colors["ramp"][t]
            .as_array()
            .unwrap_or_else(|| panic!("ramp.{t} missing"))
            .iter()
            .enumerate()
            .map(|(i, v)| color(v, &format!("ramp.{t}[{i}]")))
            .collect();
        let hc = kind == ThemeKind::HighContrast;
        Self {
            kind,
            window: neutral("window"),
            display: neutral("display"),
            help_strip: neutral("helpStrip"),
            line: neutral("line"),
            control: neutral("ctlBg"),
            control_border: neutral("ctlBorder"),
            well: neutral("well"),
            well_border: neutral("wellBorder"),
            knob_disc: neutral("dialDisc"),
            knob_disc_border: neutral("dialDiscBorder"),
            tick_off: neutral("tickOff"),
            needle: neutral("needle"),
            grid_major: neutral("gridMajor"),
            grid_minor: neutral("gridMinor"),
            grid_zero: neutral("zeroLine"),
            curve: neutral("curve"),
            spectrum_pre: neutral("preSpectrum"),
            spectrum_post_fill: neutral("postFill2"),
            glass: neutral("glass"),
            glass_border: neutral("glassBorder"),
            panel_control: neutral("panelCtlBg"),
            panel_control_border: neutral("panelCtlBorder"),
            text_disabled: neutral("textDisabled"),
            text_hi: neutral("textHi"),
            text_mid: neutral("textMid"),
            text_lo: neutral("textLo"),
            text_strong: neutral("textStrong"),
            accent,
            accent_text: neutral("accentText"),
            meter_off: neutral("meterOff"),
            meter_well: neutral("meterWell"),
            warn: semantic("warn"),
            danger: semantic("danger"),
            gain_reduction: semantic("gr"),
            band_ramp,
            shadows: !hc,
            border_width: n["borderWidth"].as_f64().unwrap_or(1.0),
            disc_shadow_alpha: {
                let disc = &root["elevation"]["dialDisc"];
                let key = if kind == ThemeKind::Light { "alphaLight" } else { "alphaDark" };
                disc[key].as_f64().expect("elevation.dialDisc alpha") as f32
            },
        }
    }
}

fn color(v: &Value, path: &str) -> Color {
    let raw = v.as_str().unwrap_or_else(|| panic!("token color.{path} missing"));
    parse_color(raw).unwrap_or_else(|| panic!("token color.{path} = {raw:?} is not a colour"))
}

/// Parses `#RRGGBB`, `#RRGGBBAA` and `rgba(r,g,b,a)` — the forms used in `tokens.json`.
pub fn parse_color(s: &str) -> Option<Color> {
    let s = s.trim();
    if let Some(hex) = s.strip_prefix('#') {
        let byte = |i: usize| u8::from_str_radix(hex.get(i..i + 2)?, 16).ok();
        return match hex.len() {
            6 => Some(Color::from_rgba8(byte(0)?, byte(2)?, byte(4)?, 255)),
            8 => Some(Color::from_rgba8(byte(0)?, byte(2)?, byte(4)?, byte(6)?)),
            _ => None,
        };
    }
    let inner = s.strip_prefix("rgba(")?.strip_suffix(')')?;
    let parts: Vec<&str> = inner.split(',').map(str::trim).collect();
    if parts.len() != 4 {
        return None;
    }
    let ch = |p: &str| p.parse::<u8>().ok();
    let a: f32 = parts[3].parse().ok()?;
    Some(Color::from_rgba8(ch(parts[0])?, ch(parts[1])?, ch(parts[2])?, (a.clamp(0.0, 1.0) * 255.0).round() as u8))
}

#[cfg(test)]
mod tests {
    use super::*;

    const PLUGINS: [&str; 15] = [
        "eq", "comp", "limit", "multiband", "verb", "delay", "sat", "deess", "gate", "shape", "soothe", "filter", "mod",
        "meter", "utility",
    ];

    #[test]
    fn tokens_parse() {
        for kind in [ThemeKind::Dark, ThemeKind::Light, ThemeKind::HighContrast, ThemeKind::ColorBlind] {
            for plugin in PLUGINS {
                let t = Theme::load(kind, plugin);
                assert!(t.band_ramp.len() >= 4, "{kind:?}");
            }
        }
    }

    #[test]
    fn accents_never_come_from_the_ramp() {
        for kind in [ThemeKind::Dark, ThemeKind::Light, ThemeKind::HighContrast] {
            for plugin in PLUGINS {
                let t = Theme::load(kind, plugin);
                assert!(!t.band_ramp.contains(&t.accent), "{plugin} accent is a band colour in {kind:?}");
            }
        }
    }

    #[test]
    fn parses_token_colour_forms() {
        assert_eq!(parse_color("#63CBFE"), Some(Color::from_rgba8(0x63, 0xCB, 0xFE, 255)));
        assert_eq!(parse_color("rgba(24,28,35,0.72)"), Some(Color::from_rgba8(24, 28, 35, 184)));
        assert_eq!(parse_color("nope"), None);
    }
}
