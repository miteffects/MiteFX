//! Declarative parameter specs.

/// How a parameter's plain value maps onto the host's 0–1 range.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ParamScale {
    /// Plain value is linear in the range (dB parameters are linear in dB).
    Linear,
    /// Logarithmic, for frequencies and times. `min` must be > 0.
    Log,
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
    /// Mouse-wheel step in plain units, normal and Shift (interaction.md, "Per parameter type").
    /// Linear parameters only; log parameters step multiplicatively (Phase 1).
    pub wheel_step: f32,
    pub wheel_step_fine: f32,
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

    /// Formats a plain value with its unit, e.g. `−0.8 dB`.
    pub fn format(&self, plain: f32) -> String {
        let s = self.format_number(plain);
        if self.unit.is_empty() { s } else { format!("{s} {}", self.unit) }
    }

    /// Parses user input: a number, optionally followed by the unit; accepts `−` or `-`.
    pub fn parse(&self, text: &str) -> Option<f32> {
        let t = text.trim().trim_end_matches(self.unit).trim().replace('−', "-");
        t.parse::<f32>().ok().map(|v| v.clamp(self.min, self.max))
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
        wheel_step: 1.0,
        wheel_step_fine: 0.1,
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
    fn parses_typed_values() {
        assert_eq!(GAIN.parse("−3.5 dB"), Some(-3.5));
        assert_eq!(GAIN.parse("6"), Some(6.0));
        assert_eq!(GAIN.parse("99"), Some(24.0));
        assert_eq!(GAIN.parse("loud"), None);
    }
}
