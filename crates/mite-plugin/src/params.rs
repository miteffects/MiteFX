//! `ParamSpec` → nih-plug parameters.

use mite_core::{BoolSpec, ChoiceSpec, ParamScale, ParamSpec};
use nih_plug::prelude::{BoolParam, FloatParam, FloatRange, IntParam, IntRange};

/// Builds the host-facing parameter for `spec`. The plain value is what the DSP reads (dB for gain
/// parameters, not a linear factor). The DSP smooths it itself, so no nih-plug smoother is set.
pub fn float_param(spec: &'static ParamSpec) -> FloatParam {
    float_param_named(spec, spec.name.to_owned())
}

/// [`float_param`] with a display name other than the spec's (e.g. "Band 3 Frequency").
pub fn float_param_named(spec: &'static ParamSpec, name: String) -> FloatParam {
    let range = match spec.scale {
        ParamScale::Linear => FloatRange::Linear { min: spec.min, max: spec.max },
        ParamScale::Log => FloatRange::Skewed { min: spec.min, max: spec.max, factor: log_like_skew(spec) },
    };
    // The display string carries the unit itself ("−0.8 dB"), so no `with_unit`.
    FloatParam::new(name, spec.default, range)
        .with_value_to_string(std::sync::Arc::new(move |v| spec.format(v)))
        .with_string_to_value(std::sync::Arc::new(move |s| spec.parse(s)))
}

/// A stepped parameter whose value is an index into `spec.choices`.
pub fn choice_param(spec: &'static ChoiceSpec, name: String) -> IntParam {
    IntParam::new(name, spec.default as i32, IntRange::Linear { min: 0, max: spec.choices.len() as i32 - 1 })
        .with_value_to_string(std::sync::Arc::new(move |v| {
            spec.choices.get(v.max(0) as usize).copied().unwrap_or("?").to_owned()
        }))
        .with_string_to_value(std::sync::Arc::new(move |s| {
            spec.choices.iter().position(|c| c.eq_ignore_ascii_case(s.trim())).map(|i| i as i32)
        }))
}

pub fn bool_param(spec: &'static BoolSpec, name: String) -> BoolParam {
    BoolParam::new(name, spec.default)
}

/// A skew whose midpoint is the geometric mean of the range, the closest nih-plug's power-law
/// range gets to a true log mapping. Exact log ranges are a Phase 1 item (EQ frequencies).
fn log_like_skew(spec: &ParamSpec) -> f32 {
    let geometric_mean = (spec.min * spec.max).sqrt();
    let t = (geometric_mean - spec.min) / (spec.max - spec.min);
    0.5_f32.ln() / t.ln()
}
