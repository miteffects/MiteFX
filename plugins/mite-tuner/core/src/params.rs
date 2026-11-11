//! Mite Tuner parameters (docs/design/tuner-brief.md §3).
//!
//! IDs are frozen once released (`tests/golden/mite_tuner.params.txt`).

use mite_core::presets::CurrentPreset;
use mite_core::snapshots::Snapshots;
use mite_core::{BoolSpec, ChoiceSpec, ParamDisplay, ParamScale, ParamSpec};
use mite_plugin::{bool_param, choice_param, float_param};
use nih_plug::prelude::*;

/// The state format version (`mite_plugin::state`).
pub const STATE_VERSION: u32 = 1;

/// A4, Hz.
pub const REFERENCE: ParamSpec = ParamSpec { id: "reference", name: "Reference", unit: "Hz", min: 420.0, max: 460.0, default: 440.0, scale: ParamScale::Linear, smoothing_ms: 0.0, decimals: 1, wheel_step: 0.5, wheel_step_fine: 0.1, display: ParamDisplay::Number };

/// The strings the tuner points at (it reads every note either way).
pub const TUNINGS: [&str; 9] = ["Chromatic", "Standard", "Drop D", "Half Step Down", "D Standard", "DADGAD", "Open G", "7-String", "Bass"];
/// Each tuning's strings, low to high, as MIDI notes.
pub const STRINGS: [&[i32]; 9] = [
    &[],
    &[40, 45, 50, 55, 59, 64],
    &[38, 45, 50, 55, 59, 64],
    &[39, 44, 49, 54, 58, 63],
    &[38, 43, 48, 53, 57, 62],
    &[38, 45, 50, 55, 57, 62],
    &[38, 43, 50, 55, 59, 62],
    &[35, 40, 45, 50, 55, 59, 64],
    &[28, 33, 38, 43],
];
pub const TUNING: ChoiceSpec = ChoiceSpec { id: "tuning", name: "Tuning", choices: &TUNINGS, default: 1 };

/// Silence the output (tune without being heard).
pub const MUTE: BoolSpec = BoolSpec { id: "mute", name: "Mute", default: false };

/// Every parameter's manifest line, in host order.
pub fn manifest() -> String {
    [REFERENCE.manifest_line(), TUNING.manifest_line(), MUTE.manifest_line()].join("\n") + "\n"
}

pub fn ids() -> Vec<String> {
    manifest().lines().filter_map(|l| l.split(" | ").next()).map(str::to_owned).collect()
}

/// Monitoring, not a setting: never in presets or snapshots.
pub fn is_monitoring(id: &str) -> bool {
    id == MUTE.id
}

pub fn sound_ids() -> Vec<String> {
    ids().into_iter().filter(|id| !is_monitoring(id)).collect()
}

pub fn spec_for(id: &str) -> Option<ParamSpec> {
    (id == REFERENCE.id).then_some(REFERENCE)
}

pub struct TunerParams {
    pub reference: FloatParam,
    pub tuning: IntParam,
    pub mute: BoolParam,
    pub snapshots: std::sync::Arc<Snapshots>,
    pub preset: std::sync::Arc<CurrentPreset>,
}

impl Default for TunerParams {
    fn default() -> Self {
        Self {
            reference: float_param(&REFERENCE),
            tuning: choice_param(&TUNING, TUNING.name.to_owned()),
            mute: bool_param(&MUTE, MUTE.name.to_owned()),
            snapshots: std::sync::Arc::new(Snapshots::new()),
            preset: std::sync::Arc::new(CurrentPreset::default()),
        }
    }
}

// SAFETY: every `ParamPtr` points into `self`, which nih-plug keeps alive (in an `Arc`) for as
// long as it uses the map. The ids match `manifest()` (checked by tests/params.rs).
unsafe impl Params for TunerParams {
    fn serialize_fields(&self) -> std::collections::BTreeMap<String, String> {
        let live = mite_plugin::state::live_values(self, &sound_ids());
        mite_plugin::state::session_fields(STATE_VERSION, &self.snapshots, &self.preset, live)
    }

    fn deserialize_fields(&self, serialized: &std::collections::BTreeMap<String, String>) {
        mite_plugin::state::load_session_fields(serialized, &self.snapshots, &self.preset);
    }

    fn param_map(&self) -> Vec<(String, ParamPtr, String)> {
        vec![
            (REFERENCE.id.to_owned(), self.reference.as_ptr(), String::new()),
            (TUNING.id.to_owned(), self.tuning.as_ptr(), String::new()),
            (MUTE.id.to_owned(), self.mute.as_ptr(), String::new()),
        ]
    }
}
