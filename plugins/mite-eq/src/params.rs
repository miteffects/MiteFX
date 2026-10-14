//! Mite EQ parameters (BLUEPRINT §8.4).
//!
//! IDs are frozen once released (`tests/golden/mite_eq.params.txt`): globals by name, bands as
//! `b{NN}_{param}` with all 24 bands pre-declared because hosts need static parameter lists.

use mite_core::{BoolSpec, ChoiceSpec, ParamDisplay, ParamScale, ParamSpec};
use mite_dsp::eq::{BandSettings, BandType, Placement, DEFAULT_SLOPE, SLOPE_NAMES};
use mite_plugin::{bool_param, choice_param, float_param, float_param_named};
use nih_plug::prelude::*;

pub const BANDS: usize = 24;

pub const OUT_GAIN: ParamSpec = ParamSpec {
    id: "out_gain",
    name: "Output",
    unit: "dB",
    min: -24.0,
    max: 24.0,
    default: 0.0,
    scale: ParamScale::Linear,
    smoothing_ms: 20.0,
    decimals: 1,
    wheel_step: 0.5,
    wheel_step_fine: 0.1,
    display: ParamDisplay::Number,
};

pub const ON: BoolSpec = BoolSpec { id: "on", name: "On", default: false };
pub const TYPE: ChoiceSpec = ChoiceSpec { id: "type", name: "Type", choices: &BandType::NAMES, default: 0 };

pub const FREQ: ParamSpec = ParamSpec {
    id: "freq",
    name: "Frequency",
    unit: "Hz",
    min: 10.0,
    max: 30_000.0,
    default: 1000.0,
    scale: ParamScale::Log,
    smoothing_ms: 20.0,
    decimals: 0,
    wheel_step: 1.0 / 6.0,
    wheel_step_fine: 1.0 / 48.0,
    display: ParamDisplay::Frequency,
};

pub const GAIN: ParamSpec = ParamSpec {
    id: "gain",
    name: "Gain",
    unit: "dB",
    min: -30.0,
    max: 30.0,
    default: 0.0,
    scale: ParamScale::Linear,
    smoothing_ms: 20.0,
    decimals: 1,
    wheel_step: 0.5,
    wheel_step_fine: 0.1,
    display: ParamDisplay::Number,
};

pub const Q: ParamSpec = ParamSpec {
    id: "q",
    name: "Q",
    unit: "",
    min: 0.025,
    max: 40.0,
    default: std::f32::consts::FRAC_1_SQRT_2,
    scale: ParamScale::Log,
    smoothing_ms: 20.0,
    decimals: 2,
    wheel_step: 1.0 / 6.0,
    wheel_step_fine: 1.0 / 24.0,
    display: ParamDisplay::Number,
};

pub const SLOPE: ChoiceSpec = ChoiceSpec { id: "slope", name: "Slope", choices: &SLOPE_NAMES, default: DEFAULT_SLOPE };

pub const STEREO: ChoiceSpec = ChoiceSpec { id: "stereo", name: "Stereo", choices: &Placement::NAMES, default: 0 };

/// `b03_freq` for band index 2.
pub fn band_id(band: usize, param: &str) -> String {
    format!("b{:02}_{param}", band + 1)
}

/// The frozen parameter manifest, in host order.
pub fn manifest() -> String {
    let mut lines = vec![OUT_GAIN.manifest_line()];
    for b in 0..BANDS {
        let prefix = |line: String| format!("b{:02}_{line}", b + 1);
        lines.push(prefix(ON.manifest_line()));
        lines.push(prefix(TYPE.manifest_line()));
        lines.push(prefix(FREQ.manifest_line()));
        lines.push(prefix(GAIN.manifest_line()));
        lines.push(prefix(Q.manifest_line()));
        lines.push(prefix(SLOPE.manifest_line()));
        lines.push(prefix(STEREO.manifest_line()));
    }
    lines.join("\n") + "\n"
}

pub struct BandParams {
    pub on: BoolParam,
    pub kind: IntParam,
    pub freq: FloatParam,
    pub gain: FloatParam,
    pub q: FloatParam,
    pub slope: IntParam,
    pub stereo: IntParam,
}

impl BandParams {
    fn new(band: usize) -> Self {
        let name = |what: &str| format!("Band {} {what}", band + 1);
        Self {
            on: bool_param(&ON, name("On")),
            kind: choice_param(&TYPE, name("Type")),
            freq: float_param_named(&FREQ, name("Frequency")),
            gain: float_param_named(&GAIN, name("Gain")),
            q: float_param_named(&Q, name("Q")),
            slope: choice_param(&SLOPE, name("Slope")),
            stereo: choice_param(&STEREO, name("Stereo")),
        }
    }

    /// The band as the DSP sees it. Audio-thread safe (atomic reads only).
    pub fn settings(&self) -> BandSettings {
        BandSettings {
            enabled: self.on.value(),
            kind: BandType::from_index(self.kind.value().max(0) as usize),
            freq: f64::from(self.freq.value()),
            gain_db: f64::from(self.gain.value()),
            q: f64::from(self.q.value()),
            slope: self.slope.value().max(0) as usize,
            placement: Placement::from_index(self.stereo.value().max(0) as usize),
        }
    }
}

pub struct EqParams {
    pub out_gain: FloatParam,
    pub bands: [BandParams; BANDS],
}

impl Default for EqParams {
    fn default() -> Self {
        Self { out_gain: float_param(&OUT_GAIN), bands: std::array::from_fn(BandParams::new) }
    }
}

// SAFETY: every `ParamPtr` points into `self`, which nih-plug keeps alive (in an `Arc`) for as
// long as it uses the map. The ids match `manifest()` (checked by tests/params.rs).
unsafe impl Params for EqParams {
    fn param_map(&self) -> Vec<(String, ParamPtr, String)> {
        let mut map = vec![(OUT_GAIN.id.to_owned(), self.out_gain.as_ptr(), String::new())];
        for (i, b) in self.bands.iter().enumerate() {
            let group = format!("Band {}", i + 1);
            map.push((band_id(i, ON.id), b.on.as_ptr(), group.clone()));
            map.push((band_id(i, TYPE.id), b.kind.as_ptr(), group.clone()));
            map.push((band_id(i, FREQ.id), b.freq.as_ptr(), group.clone()));
            map.push((band_id(i, GAIN.id), b.gain.as_ptr(), group.clone()));
            map.push((band_id(i, Q.id), b.q.as_ptr(), group.clone()));
            map.push((band_id(i, SLOPE.id), b.slope.as_ptr(), group.clone()));
            map.push((band_id(i, STEREO.id), b.stereo.as_ptr(), group));
        }
        map
    }
}
