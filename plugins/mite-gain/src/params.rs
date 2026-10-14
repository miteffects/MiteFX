//! Mite Gain parameters. IDs are frozen once released (see `tests/golden/mite_gain.params.txt`).

use mite_core::{ParamScale, ParamSpec};
use mite_plugin::float_param;
use nih_plug::prelude::*;

pub const GAIN: ParamSpec = ParamSpec {
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

/// Every parameter, in host order.
pub const SPECS: &[ParamSpec] = &[GAIN];

#[derive(Params)]
pub struct GainParams {
    #[id = "gain"]
    pub gain: FloatParam,
}

impl Default for GainParams {
    fn default() -> Self {
        Self { gain: float_param(&GAIN) }
    }
}
