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
    pub sidechain: Color,
    /// Upward gain (upward compression and expansion); rises from the bottom, as `gain_reduction`
    /// hangs from the top.
    pub gain_up: Color,
    pub masking: Color,
    pub band_ramp: Vec<Color>,
    /// Drop shadows and glow are off in high contrast (`elevation.*.hc` = none).
    pub shadows: bool,
    /// Control border width: 1 px, 1.5 px in high contrast.
    pub border_width: f64,
    /// Dial disc drop-shadow opacity (`elevation.dialDisc`).
    pub disc_shadow_alpha: f32,
    /// Backdrop blur σ of glass panels and of menus, logical px (`blur.glassPanel`, `blur.menu`).
    pub blur_panel: f64,
    pub blur_menu: f64,
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
            // A plugin's own text tint when its accent has one (Shape's leaf), else the shared one.
            accent_text: colors["accent"][plugin]["text"].get(t).map_or_else(|| neutral("accentText"), |v| color(v, &format!("accent.{plugin}.text.{t}"))),
            meter_off: neutral("meterOff"),
            meter_well: neutral("meterWell"),
            warn: semantic("warn"),
            danger: semantic("danger"),
            gain_reduction: semantic("gr"),
            sidechain: semantic("sidechain"),
            gain_up: semantic("gainUp"),
            masking: semantic("masking"),
            band_ramp,
            shadows: !hc,
            border_width: n["borderWidth"].as_f64().unwrap_or(1.0),
            disc_shadow_alpha: {
                let disc = &root["elevation"]["dialDisc"];
                let key = if kind == ThemeKind::Light { "alphaLight" } else { "alphaDark" };
                disc[key].as_f64().expect("elevation.dialDisc alpha") as f32
            },
            blur_panel: root["blur"]["glassPanel"].as_f64().expect("blur.glassPanel"),
            blur_menu: root["blur"]["menu"].as_f64().expect("blur.menu"),
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

    const PLUGINS: [&str; 19] = [
        "eq", "comp", "limit", "multiband", "verb", "delay", "sat", "deess", "gate", "shape", "soothe", "filter", "mod",
        "meter", "utility", "tuner", "double", "pitch", "denoise",
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

    /// CIE L*a*b* (D65) of `#RRGGBB`.
    fn lab(hex: &str) -> [f64; 3] {
        let byte = |i: usize| f64::from(u8::from_str_radix(&hex[1 + i..3 + i], 16).unwrap()) / 255.0;
        let lin = |c: f64| if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) };
        let (r, g, b) = (lin(byte(0)), lin(byte(2)), lin(byte(4)));
        let x = (0.4124 * r + 0.3576 * g + 0.1805 * b) / 0.95047;
        let y = 0.2126 * r + 0.7152 * g + 0.0722 * b;
        let z = (0.0193 * r + 0.1192 * g + 0.9505 * b) / 1.08883;
        let f = |t: f64| if t > 216.0 / 24389.0 { t.cbrt() } else { (24389.0 / 27.0 * t + 16.0) / 116.0 };
        let (fx, fy, fz) = (f(x), f(y), f(z));
        [116.0 * fy - 16.0, 500.0 * (fx - fy), 200.0 * (fy - fz)]
    }

    /// CIEDE2000 between two `#RRGGBB` colours (the metric `rules.deltaE` names).
    fn delta_e(c1: &str, c2: &str) -> f64 {
        delta_e_lab(lab(c1), lab(c2))
    }

    fn delta_e_lab([l1, a1, b1]: [f64; 3], [l2, a2, b2]: [f64; 3]) -> f64 {
        let p7 = |c: f64| c.powi(7);
        let cb = (a1.hypot(b1) + a2.hypot(b2)) / 2.0;
        let g = 0.5 * (1.0 - (p7(cb) / (p7(cb) + p7(25.0))).sqrt());
        let (a1p, a2p) = ((1.0 + g) * a1, (1.0 + g) * a2);
        let (c1p, c2p) = (a1p.hypot(b1), a2p.hypot(b2));
        let hue = |b: f64, a: f64| b.atan2(a).to_degrees().rem_euclid(360.0);
        let (h1, h2) = (hue(b1, a1p), hue(b2, a2p));
        let mut dh = h2 - h1;
        if c1p * c2p == 0.0 {
            dh = 0.0;
        } else if dh > 180.0 {
            dh -= 360.0;
        } else if dh < -180.0 {
            dh += 360.0;
        }
        let d_hh = 2.0 * (c1p * c2p).sqrt() * (dh / 2.0).to_radians().sin();
        let lb = (l1 + l2) / 2.0;
        let cbp = (c1p + c2p) / 2.0;
        let hb = if c1p * c2p == 0.0 {
            h1 + h2
        } else if (h1 - h2).abs() <= 180.0 {
            (h1 + h2) / 2.0
        } else if h1 + h2 < 360.0 {
            (h1 + h2 + 360.0) / 2.0
        } else {
            (h1 + h2 - 360.0) / 2.0
        };
        let cos = |d: f64| d.to_radians().cos();
        let t = 1.0 - 0.17 * cos(hb - 30.0) + 0.24 * cos(2.0 * hb) + 0.32 * cos(3.0 * hb + 6.0) - 0.2 * cos(4.0 * hb - 63.0);
        let dth = 30.0 * (-((hb - 275.0) / 25.0).powi(2)).exp();
        let rc = 2.0 * (p7(cbp) / (p7(cbp) + p7(25.0))).sqrt();
        let sl = 1.0 + 0.015 * (lb - 50.0).powi(2) / (20.0 + (lb - 50.0).powi(2)).sqrt();
        let sc = 1.0 + 0.045 * cbp;
        let sh = 1.0 + 0.015 * cbp * t;
        let rt = -(2.0 * dth).to_radians().sin() * rc;
        let (dl, dc, dhh) = ((l2 - l1) / sl, (c2p - c1p) / sc, d_hh / sh);
        (dl * dl + dc * dc + dhh * dhh + rt * dc * dhh).sqrt()
    }

    #[test]
    fn delta_e_matches_the_reference_pairs() {
        // Sharma, Wu and Dalal (2005), pairs 1, 7 and 17.
        for (a, b, want) in [
            ([50.0, 2.6772, -79.7751], [50.0, 0.0, -82.7485], 2.0425),
            ([50.0, 0.0, 0.0], [50.0, -1.0, 2.0], 2.3669),
            ([50.0, 2.5, 0.0], [73.0, 25.0, -18.0], 27.1492),
        ] {
            assert!((delta_e_lab(a, b) - want).abs() < 1e-4, "{a:?} {b:?}");
        }
        assert!((delta_e("#000000", "#FFFFFF") - 100.0).abs() < 0.01);
    }

    #[test]
    fn accents_clear_the_ramp_and_semantics() {
        let json: Value = serde_json::from_str(TOKENS_JSON).unwrap();
        let color = &json["color"];
        let min = color["rules"]["minDeltaE"].as_f64().unwrap();
        let exceptions: Vec<(String, String)> = color["rules"]["accentExceptions"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| (p[0].as_str().unwrap().to_owned(), p[1].as_str().unwrap().to_owned()))
            .collect();
        for theme in ["dark", "light", "hc"] {
            for (plugin, accent) in color["accent"].as_object().unwrap() {
                let a = accent[theme].as_str().unwrap();
                for (i, band) in color["ramp"][theme].as_array().unwrap().iter().enumerate() {
                    let d = delta_e(a, band.as_str().unwrap());
                    assert!(d >= min, "{plugin} accent vs ramp {i} in {theme}: {d:.1}");
                }
                for (name, sem) in color["semantic"].as_object().unwrap() {
                    if name == "focus" || exceptions.contains(&(plugin.clone(), name.clone())) {
                        continue;
                    }
                    let d = delta_e(a, sem[theme].as_str().unwrap());
                    assert!(d >= min, "{plugin} accent vs {name} in {theme}: {d:.1}");
                }
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
