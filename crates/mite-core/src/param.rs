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
    /// Milliseconds, one decimal below 10 ms and none from there (`4.5 ms`, `15 ms`), seconds
    /// with two decimals from 1 s (`1.50 s`). Values snap to that precision (see
    /// [`ParamSpec::snap`]), so a drag lands on 15 ms exactly.
    Time,
    /// A ratio `n:1`: one decimal under 10, none from there, and `∞` at the top of the range.
    Ratio,
    /// As [`ParamDisplay::Time`], with `∞` at the top of the range (a length that can be endless).
    TimeOrForever,
}

/// The precision a [`ParamDisplay::Time`] value is shown and snapped to, ms.
fn time_step(ms: f32) -> f32 {
    if ms < 10.0 {
        0.1
    } else if ms < 1000.0 {
        1.0
    } else {
        10.0
    }
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
            // Follows the rounded value too: 999.97 Hz reads "1.00 kHz", not "1000 Hz" (which
            // would read back as 1000 and then show as kHz: not a stable round trip).
            ParamDisplay::Frequency if plain.round() >= 1000.0 => (format!("{:.2}", plain / 1000.0), "kHz"),
            ParamDisplay::Frequency => (format!("{:.0}", plain), "Hz"),
            // The format follows the *rounded* value, so a time just under a boundary (9.97 ms)
            // reads like the boundary does ("10 ms") and survives a text round trip.
            ParamDisplay::TimeOrForever if plain >= self.max * 0.995 => ("∞".to_owned(), ""),
            ParamDisplay::Time | ParamDisplay::TimeOrForever if (plain * 10.0).round() < 100.0 => (format!("{:.1}", plain), "ms"),
            ParamDisplay::Time | ParamDisplay::TimeOrForever if plain.round() < 1000.0 => (format!("{:.0}", plain), "ms"),
            ParamDisplay::Time | ParamDisplay::TimeOrForever => (format!("{:.2}", plain / 1000.0), "s"),
            ParamDisplay::Ratio if plain >= self.max * 0.995 => ("∞".to_owned(), ":1"),
            ParamDisplay::Ratio if plain < 10.0 => (format!("{plain:.1}"), ":1"),
            ParamDisplay::Ratio => (format!("{plain:.0}"), ":1"),
            ParamDisplay::Number => (self.format_number(plain), self.unit),
        }
    }

    /// The value as it is shown: [`ParamDisplay::Time`] values round to their displayed
    /// precision; every other display is returned unchanged.
    pub fn snap(&self, plain: f32) -> f32 {
        match self.display {
            ParamDisplay::Time | ParamDisplay::TimeOrForever => {
                let step = time_step(plain);
                ((plain / step).round() * step).clamp(self.min, self.max)
            }
            _ => plain,
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
        if matches!(self.display, ParamDisplay::Ratio | ParamDisplay::TimeOrForever) && (lower.contains('∞') || lower.starts_with("inf")) {
            return Some(self.max);
        }
        let seconds = matches!(self.display, ParamDisplay::Time | ParamDisplay::TimeOrForever) && lower.ends_with('s') && !lower.ends_with("ms");
        let (num, factor) = if let Some(n) = lower.strip_suffix("khz").or_else(|| lower.strip_suffix('k')) {
            (n.trim().to_owned(), 1000.0)
        } else if seconds {
            (lower.trim_end_matches('s').trim().to_owned(), 1000.0)
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
        let v = v.clamp(self.min, self.max);
        // Snapped values must still move: a notch too small to reach the next shown value takes
        // one display step.
        let (from, to) = (self.snap(plain), self.snap(v));
        if to == from && notches != 0.0 && matches!(self.display, ParamDisplay::Time | ParamDisplay::TimeOrForever) {
            return self.snap(from + time_step(from) * notches.signum());
        }
        to
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

/// How a dial feels, separate from the host's normalisation: breakpoints of (dial position
/// 0…1, plain value), interpolated logarithmically between them where both ends are positive
/// (times) and linearly otherwise (signed amounts). It gives common values more travel (release
/// 100–300 ms, shaping ±6 dB) and rare ones less. Plain values must be increasing.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DialCurve(pub &'static [(f32, f32)]);

impl DialCurve {
    /// The plain value at dial position `pos`.
    pub fn to_plain(&self, pos: f32) -> f32 {
        let pos = pos.clamp(0.0, 1.0);
        for w in self.0.windows(2) {
            let ((p0, v0), (p1, v1)) = (w[0], w[1]);
            if pos <= p1 {
                let t = if p1 > p0 { (pos - p0) / (p1 - p0) } else { 1.0 };
                return if v0 > 0.0 && v1 > 0.0 { v0 * (v1 / v0).powf(t) } else { v0 + (v1 - v0) * t };
            }
        }
        self.0.last().map_or(0.0, |&(_, v)| v)
    }

    /// The dial position of plain value `plain`.
    pub fn to_position(&self, plain: f32) -> f32 {
        for w in self.0.windows(2) {
            let ((p0, v0), (p1, v1)) = (w[0], w[1]);
            if plain <= v1 {
                let t = if v0 > 0.0 && v1 > 0.0 { (plain.max(v0) / v0).ln() / (v1 / v0).ln() } else { (plain.max(v0) - v0) / (v1 - v0) };
                return p0 + t.clamp(0.0, 1.0) * (p1 - p0);
            }
        }
        1.0
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

    const ATTACK: ParamSpec = ParamSpec {
        id: "attack",
        name: "Attack",
        unit: "ms",
        min: 0.5,
        max: 200.0,
        default: 10.0,
        scale: ParamScale::Log,
        smoothing_ms: 0.0,
        decimals: 1,
        wheel_step: 1.0 / 12.0,
        wheel_step_fine: 1.0 / 48.0,
        display: ParamDisplay::Time,
    };

    #[test]
    fn dial_curves_round_trip_and_hit_their_breakpoints() {
        let release = DialCurve(&[(0.0, 10.0), (0.3, 100.0), (0.75, 300.0), (1.0, 2500.0)]);
        assert!((release.to_plain(0.3) - 100.0).abs() < 1e-3);
        assert!((release.to_plain(0.75) - 300.0).abs() < 1e-2);
        for i in 0..=40 {
            let pos = i as f32 / 40.0;
            let back = release.to_position(release.to_plain(pos));
            assert!((back - pos).abs() < 1e-4, "{pos} -> {back}");
        }
    }

    #[test]
    fn times_show_and_snap_to_readable_values() {
        assert_eq!(ATTACK.format(4.53), "4.5 ms");
        assert_eq!(ATTACK.format(15.3), "15 ms");
        assert_eq!(ATTACK.snap(15.3), 15.0);
        assert_eq!(ATTACK.snap(4.53), 4.5);
        let release = ParamSpec { max: 2500.0, ..ATTACK };
        assert_eq!(release.format(1500.0), "1.50 s");
        assert_eq!(release.parse("1.5 s"), Some(1500.0));
        assert_eq!(release.parse("15 ms"), Some(15.0));
        // Values just under a format boundary read like the boundary and round-trip as text.
        let release = ParamSpec { max: 2500.0, ..ATTACK };
        for v in [9.96f32, 9.9999, 999.6, 999.9999] {
            let text = release.format(v);
            let back = release.parse(&text).expect("parses");
            assert_eq!(release.format(back), text, "{v} -> {text}");
        }
        assert_eq!(release.format(999.7), "1.00 s");
        // A fine wheel notch is smaller than 1 ms at 15 ms, but still moves by one shown step.
        assert_eq!(ATTACK.wheel(15.0, 1.0, true), 16.0);
        assert_eq!(ATTACK.wheel(15.0, -1.0, true), 14.0);
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
        // Just under the unit boundary: shown as it reads back, so text round-trips.
        assert_eq!(FREQ.format(999.97), "1.00 kHz");
        let back = FREQ.parse(&FREQ.format(999.97)).expect("parses");
        assert_eq!(FREQ.format(back), FREQ.format(999.97));
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
