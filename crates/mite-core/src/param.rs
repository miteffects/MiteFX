//! Declarative parameter specs.

/// How a parameter's plain value maps onto the host's 0–1 range.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ParamScale {
    /// Plain value is linear in the range (dB parameters are linear in dB).
    Linear,
    /// Logarithmic, for frequencies and times. `min` must be > 0.
    Log,
}

/// How a value is shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParamDisplay {
    /// `decimals` places, then the unit.
    Number,
    /// Hz below 1 kHz (no decimals), kHz with two decimals above: `160 Hz`, `2.80 kHz`.
    Frequency,
}

/// One parameter, independent of any plugin framework.
///
/// `id` is part of the saved-state contract: once released it never changes (BLUEPRINT §5.5).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ParamSpec {
    pub id: &'static str,
    pub name: &'static str,
    pub unit: &'static str,
    pub min: f32,
    pub max: f32,
    pub default: f32,
    pub scale: ParamScale,
    /// Ramp time used by the DSP smoother.
    pub smoothing_ms: f32,
    /// Decimal places in the displayed value.
    pub decimals: u8,
    /// Mouse-wheel step, normal and Shift (interaction.md, "Per parameter type"): plain units for
    /// linear parameters, octaves (a ×2^step factor) for log parameters.
    pub wheel_step: f32,
    pub wheel_step_fine: f32,
    pub display: ParamDisplay,
}

impl ParamSpec {
    pub fn normalize(&self, plain: f32) -> f32 {
        let v = plain.clamp(self.min, self.max);
        match self.scale {
            ParamScale::Linear => (v - self.min) / (self.max - self.min),
            ParamScale::Log => (v / self.min).ln() / (self.max / self.min).ln(),
        }
    }

    pub fn denormalize(&self, normalized: f32) -> f32 {
        let n = normalized.clamp(0.0, 1.0);
        match self.scale {
            ParamScale::Linear => self.min + n * (self.max - self.min),
            ParamScale::Log => self.min * (self.max / self.min).powf(n),
        }
    }

    /// Formats a plain value as (number, unit), e.g. `("−0.8", "dB")` or `("2.80", "kHz")`.
    pub fn format_parts(&self, plain: f32) -> (String, &'static str) {
        match self.display {
            ParamDisplay::Frequency if plain >= 1000.0 => (format!("{:.2}", plain / 1000.0), "kHz"),
            ParamDisplay::Frequency => (format!("{:.0}", plain), "Hz"),
            ParamDisplay::Number => (self.format_number(plain), self.unit),
        }
    }

    /// Formats a plain value without its unit, e.g. `−0.8`. Uses a true minus sign like the
    /// design, and never shows `−0.0`.
    pub fn format_number(&self, plain: f32) -> String {
        let s = format!("{:.*}", self.decimals as usize, plain);
        match s.strip_prefix('-') {
            Some(rest) if rest.chars().all(|c| c == '0' || c == '.') => rest.to_string(),
            Some(rest) => format!("−{rest}"),
            None => s,
        }
    }

    /// Formats a plain value with its unit, e.g. `−0.8 dB`, `2.80 kHz`.
    pub fn format(&self, plain: f32) -> String {
        let (n, unit) = self.format_parts(plain);
        if unit.is_empty() { n } else { format!("{n} {unit}") }
    }

    /// Parses user input: a number with an optional unit or `k` suffix (`3.5k`, `350 Hz`,
    /// `−3 dB`); accepts `−` or `-`.
    pub fn parse(&self, text: &str) -> Option<f32> {
        let t = text.trim().replace('−', "-");
        let lower = t.to_ascii_lowercase();
        let (num, factor) = if let Some(n) = lower.strip_suffix("khz").or_else(|| lower.strip_suffix('k')) {
            (n.trim().to_owned(), 1000.0)
        } else {
            let unit = self.unit.to_ascii_lowercase();
            let n = if unit.is_empty() { lower.as_str() } else { lower.trim_end_matches(unit.as_str()) };
            (n.trim().to_owned(), 1.0)
        };
        num.parse::<f32>().ok().map(|v| (v * factor).clamp(self.min, self.max))
    }

    /// The value one wheel notch away (interaction.md): additive for linear parameters,
    /// multiplicative (octaves) for log ones.
    pub fn wheel(&self, plain: f32, notches: f32, fine: bool) -> f32 {
        let step = if fine { self.wheel_step_fine } else { self.wheel_step };
        let v = match self.scale {
            ParamScale::Linear => plain + notches * step,
            ParamScale::Log => plain * 2f32.powf(notches * step),
        };
        v.clamp(self.min, self.max)
    }

    /// One line of the golden manifest. Any change to this line for a released parameter is a
    /// breaking change to saved state and must go through a versioned migration.
    pub fn manifest_line(&self) -> String {
        format!(
            "{} | {} | {} | {} | {} | {:?}",
            self.id, self.unit, self.min, self.max, self.default, self.scale
        )
    }
}

/// A parameter that picks one of a fixed list. The stored value is the index, so the list is
/// append-only once released.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ChoiceSpec {
    pub id: &'static str,
    pub name: &'static str,
    pub choices: &'static [&'static str],
    pub default: usize,
}

impl ChoiceSpec {
    pub fn manifest_line(&self) -> String {
        format!("{} | choice | {} | {}", self.id, self.choices.join(", "), self.default)
    }
}

/// An on/off parameter.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BoolSpec {
    pub id: &'static str,
    pub name: &'static str,
    pub default: bool,
}

impl BoolSpec {
    pub fn manifest_line(&self) -> String {
        format!("{} | bool | {}", self.id, self.default)
    }
}

/// The golden manifest for a plugin: one [`ParamSpec::manifest_line`] per parameter, in order.
pub fn manifest(specs: &[ParamSpec]) -> String {
    let mut out = String::new();
    for spec in specs {
        out.push_str(&spec.manifest_line());
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const GAIN: ParamSpec = ParamSpec {
        id: "gain",
        name: "Gain",
        unit: "dB",
        min: -48.0,
        max: 24.0,
        default: 0.0,
        scale: ParamScale::Linear,
        smoothing_ms: 20.0,
        decimals: 1,
        wheel_step: 0.5,
        wheel_step_fine: 0.1,
        display: ParamDisplay::Number,
    };

    const FREQ: ParamSpec = ParamSpec {
        id: "freq",
        name: "Frequency",
        unit: "Hz",
        min: 10.0,
        max: 30_000.0,
        default: 1_000.0,
        scale: ParamScale::Log,
        smoothing_ms: 20.0,
        decimals: 0,
        wheel_step: 1.0 / 6.0,
        wheel_step_fine: 1.0 / 48.0,
        display: ParamDisplay::Frequency,
    };

    #[test]
    fn round_trips() {
        for spec in [GAIN, FREQ] {
            for i in 0..=20 {
                let n = i as f32 / 20.0;
                let back = spec.normalize(spec.denormalize(n));
                assert!((back - n).abs() < 1e-5, "{} {n} -> {back}", spec.id);
            }
        }
    }

    #[test]
    fn formats_like_the_design() {
        assert_eq!(GAIN.format(-0.8), "−0.8 dB");
        assert_eq!(GAIN.format(3.5), "3.5 dB");
        assert_eq!(GAIN.format(-0.01), "0.0 dB");
    }

    #[test]
    fn frequencies_read_like_the_design() {
        assert_eq!(FREQ.format(160.0), "160 Hz");
        assert_eq!(FREQ.format(2800.0), "2.80 kHz");
        assert_eq!(FREQ.parse("3.5k"), Some(3500.0));
        assert_eq!(FREQ.parse("350 Hz"), Some(350.0));
    }

    #[test]
    fn log_wheel_steps_in_octaves() {
        let up = FREQ.wheel(1000.0, 6.0, false);
        assert!((up - 2000.0).abs() < 0.5, "{up}");
    }

    #[test]
    fn parses_typed_values() {
        assert_eq!(GAIN.parse("−3.5 dB"), Some(-3.5));
        assert_eq!(GAIN.parse("6"), Some(6.0));
        assert_eq!(GAIN.parse("99"), Some(24.0));
        assert_eq!(GAIN.parse("loud"), None);
    }
}
