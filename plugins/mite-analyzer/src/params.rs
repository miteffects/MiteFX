//! Mite Analyzer parameters (docs/design/analyzer-brief.md §3). None touches the sound: the
//! analyzer passes the audio through unchanged; these are how it reads.
//!
//! IDs are frozen once released (`tests/golden/mite_analyzer.params.txt`).

use std::sync::Arc;

use mite_core::presets::CurrentPreset;
use mite_core::snapshots::Snapshots;
use mite_core::{BoolSpec, ChoiceSpec};
use mite_plugin::{bool_param, choice_param};
use nih_plug::prelude::*;

/// The state format version (`mite_plugin::state`).
pub const STATE_VERSION: u32 = 1;

/// The loudness target the integrated reading is held against (LUFS in [`TARGET_LUFS`]).
pub const TARGET: ChoiceSpec = ChoiceSpec { id: "target", name: "Target", choices: &["Streaming −14", "Apple Music −16", "EBU R128 −23", "ATSC A/85 −24", "Club −9"], default: 0 };
pub const TARGET_LUFS: [f32; 5] = [-14.0, -16.0, -23.0, -24.0, -9.0];
/// What the spectrum shows: the sum, mid and side, or left and right.
pub const VIEW: ChoiceSpec = ChoiceSpec { id: "view", name: "View", choices: &["Stereo", "Mid / Side", "Left / Right"], default: 0 };
/// The spectrum's tilt, so a balanced mix reads level (4.5 dB/oct: pink noise flat).
pub const TILT: ChoiceSpec = ChoiceSpec { id: "tilt", name: "Tilt", choices: &["Flat", "3 dB/oct", "4.5 dB/oct"], default: 2 };
pub const TILT_DB_OCT: [f64; 3] = [0.0, 3.0, 4.5];
/// The spectrum's ballistics.
pub const SPEED: ChoiceSpec = ChoiceSpec { id: "speed", name: "Speed", choices: &["Fast", "Normal", "Slow"], default: 1 };
/// The spectrum's peak hold line.
pub const HOLD: BoolSpec = BoolSpec { id: "hold", name: "Peak Hold", default: true };

pub fn manifest() -> String {
    [TARGET.manifest_line(), VIEW.manifest_line(), TILT.manifest_line(), SPEED.manifest_line(), HOLD.manifest_line()].join("\n") + "\n"
}

pub fn ids() -> Vec<String> {
    manifest().lines().filter_map(|l| l.split(" | ").next()).map(str::to_owned).collect()
}

/// What presets and snapshots keep: all of it (the analyzer has no monitoring switches).
pub fn sound_ids() -> Vec<String> {
    ids()
}

pub struct AnalyzerParams {
    pub target: IntParam,
    pub view: IntParam,
    pub tilt: IntParam,
    pub speed: IntParam,
    pub hold: BoolParam,
    pub snapshots: Arc<Snapshots>,
    pub preset: Arc<CurrentPreset>,
}

impl Default for AnalyzerParams {
    fn default() -> Self {
        let choice = |s: &'static ChoiceSpec| choice_param(s, s.name.to_owned());
        Self {
            target: choice(&TARGET),
            view: choice(&VIEW),
            tilt: choice(&TILT),
            speed: choice(&SPEED),
            hold: bool_param(&HOLD, HOLD.name.to_owned()),
            snapshots: Arc::new(Snapshots::new()),
            preset: Arc::new(CurrentPreset::default()),
        }
    }
}

// SAFETY: every `ParamPtr` points into `self`, which nih-plug keeps alive (in an `Arc`) for as
// long as it uses the map. The ids match `manifest()` (checked by tests/params.rs).
unsafe impl Params for AnalyzerParams {
    fn serialize_fields(&self) -> std::collections::BTreeMap<String, String> {
        let live = mite_plugin::state::live_values(self, &sound_ids());
        mite_plugin::state::session_fields(STATE_VERSION, &self.snapshots, &self.preset, live)
    }

    fn deserialize_fields(&self, serialized: &std::collections::BTreeMap<String, String>) {
        mite_plugin::state::load_session_fields(serialized, &self.snapshots, &self.preset);
    }

    fn param_map(&self) -> Vec<(String, ParamPtr, String)> {
        let p = |id: &str, ptr: ParamPtr| (id.to_owned(), ptr, String::new());
        vec![p(TARGET.id, self.target.as_ptr()), p(VIEW.id, self.view.as_ptr()), p(TILT.id, self.tilt.as_ptr()), p(SPEED.id, self.speed.as_ptr()), p(HOLD.id, self.hold.as_ptr())]
    }
}
