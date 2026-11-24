//! Mite Rig's host parameters (docs/design/rig-brief.md §4): the few that stay put whatever the
//! board holds. The board itself (its blocks, their values and files) is the state's `board`
//! field. IDs are frozen once released (`tests/golden/mite_rig.params.txt`).

use std::sync::Arc;

use mite_core::presets::CurrentPreset;
use mite_core::snapshots::Snapshots;
use mite_core::{BoolSpec, ChoiceSpec, ParamDisplay, ParamScale, ParamSpec};
use mite_plugin::{bool_param, choice_param, float_param};
use nih_plug::prelude::*;

use crate::board::Board;
use crate::engine::RigSettings;

/// The state format version (`mite_plugin::state`). 2 (2026-11-11): the macro knobs and switches
/// (new parameters at their defaults; the board's mapping is an optional field of the board).
pub const STATE_VERSION: u32 = 2;

/// The state field holding the board.
pub const BOARD_KEY: &str = "board";

#[allow(clippy::too_many_arguments)] // one line per parameter reads best
const fn spec(id: &'static str, name: &'static str, unit: &'static str, min: f32, max: f32, default: f32, scale: ParamScale, decimals: u8, wheel: (f32, f32), display: ParamDisplay) -> ParamSpec {
    ParamSpec { id, name, unit, min, max, default, scale, smoothing_ms: 20.0, decimals, wheel_step: wheel.0, wheel_step_fine: wheel.1, display }
}

pub const INPUT: ParamSpec = spec("input", "Input", "dB", -24.0, 24.0, 0.0, ParamScale::Linear, 1, (0.5, 0.1), ParamDisplay::Number);
/// Captures that come in sizes (A2 Full and Lite) play this one, everywhere on the board.
pub const SIZE_NAMES: [&str; 2] = ["Full", "Lite"];
pub const SIZE: ChoiceSpec = ChoiceSpec { id: "size", name: "Model Size", choices: &SIZE_NAMES, default: 0 };
pub const MIX: ParamSpec = spec("mix", "Mix", "%", 0.0, 100.0, 100.0, ParamScale::Linear, 0, (1.0, 0.1), ParamDisplay::Number);
pub const OUT_GAIN: ParamSpec = spec("out_gain", "Output", "dB", -24.0, 24.0, 0.0, ParamScale::Linear, 1, (0.5, 0.1), ParamDisplay::Number);
pub const BYPASS: BoolSpec = BoolSpec { id: "bypass", name: "Bypass", default: false };

pub const CONTINUOUS: [ParamSpec; 3] = [INPUT, MIX, OUT_GAIN];

/// The macro knobs (0…100 %) and switches the host sees (`crate::macros`).
pub const MACRO_KNOBS: [ParamSpec; crate::macros::KNOBS] = [
    spec("macro1", "Macro 1", "%", 0.0, 100.0, 0.0, ParamScale::Linear, 0, (1.0, 0.1), ParamDisplay::Number),
    spec("macro2", "Macro 2", "%", 0.0, 100.0, 0.0, ParamScale::Linear, 0, (1.0, 0.1), ParamDisplay::Number),
    spec("macro3", "Macro 3", "%", 0.0, 100.0, 0.0, ParamScale::Linear, 0, (1.0, 0.1), ParamDisplay::Number),
    spec("macro4", "Macro 4", "%", 0.0, 100.0, 0.0, ParamScale::Linear, 0, (1.0, 0.1), ParamDisplay::Number),
    spec("macro5", "Macro 5", "%", 0.0, 100.0, 0.0, ParamScale::Linear, 0, (1.0, 0.1), ParamDisplay::Number),
    spec("macro6", "Macro 6", "%", 0.0, 100.0, 0.0, ParamScale::Linear, 0, (1.0, 0.1), ParamDisplay::Number),
    spec("macro7", "Macro 7", "%", 0.0, 100.0, 0.0, ParamScale::Linear, 0, (1.0, 0.1), ParamDisplay::Number),
    spec("macro8", "Macro 8", "%", 0.0, 100.0, 0.0, ParamScale::Linear, 0, (1.0, 0.1), ParamDisplay::Number),
];
pub const MACRO_SWITCHES: [BoolSpec; crate::macros::SWITCHES] = [
    BoolSpec { id: "switch1", name: "Switch 1", default: false },
    BoolSpec { id: "switch2", name: "Switch 2", default: false },
    BoolSpec { id: "switch3", name: "Switch 3", default: false },
    BoolSpec { id: "switch4", name: "Switch 4", default: false },
];

pub fn spec_for(id: &str) -> Option<ParamSpec> {
    CONTINUOUS.iter().chain(MACRO_KNOBS.iter()).find(|s| s.id == id).copied()
}

pub fn manifest() -> String {
    let mut lines = vec![INPUT.manifest_line(), SIZE.manifest_line(), MIX.manifest_line(), OUT_GAIN.manifest_line(), BYPASS.manifest_line()];
    lines.extend(MACRO_KNOBS.iter().map(ParamSpec::manifest_line));
    lines.extend(MACRO_SWITCHES.iter().map(BoolSpec::manifest_line));
    lines.join("\n") + "\n"
}

pub fn ids() -> Vec<String> {
    manifest().lines().filter_map(|l| l.split(" | ").next()).map(str::to_owned).collect()
}

/// Monitoring, not sound: never in presets or snapshots.
pub fn is_monitoring(id: &str) -> bool {
    id == BYPASS.id
}

pub fn sound_ids() -> Vec<String> {
    ids().into_iter().filter(|id| !is_monitoring(id)).collect()
}

/// The container size a choice index plays (Lite: the smallest member, Full: the largest).
pub fn size_value(index: usize) -> f64 {
    if index == 1 { 0.0 } else { 1.0 }
}

pub struct RigParams {
    pub input: FloatParam,
    pub size: IntParam,
    pub mix: FloatParam,
    pub out_gain: FloatParam,
    pub bypass: BoolParam,
    pub macros: [FloatParam; crate::macros::KNOBS],
    pub switches: [BoolParam; crate::macros::SWITCHES],
    pub snapshots: Arc<Snapshots>,
    pub preset: Arc<CurrentPreset>,
    /// The board (saved with the state).
    pub board: Arc<Board>,
    /// A host without parameters (Miteboard) picks the model size here: -1 follows `size`,
    /// else the choice index (0 Full, 1 Lite).
    pub size_override: std::sync::atomic::AtomicI8,
}

impl Default for RigParams {
    fn default() -> Self {
        Self {
            input: float_param(&INPUT),
            size: choice_param(&SIZE, SIZE.name.to_owned()),
            mix: float_param(&MIX),
            out_gain: float_param(&OUT_GAIN),
            // An ordinary parameter, never the host's bypass (REAPER stops processing a
            // host-bypassed plugin, and then the editor could not turn it off).
            bypass: bool_param(&BYPASS, BYPASS.name.to_owned()),
            macros: std::array::from_fn(|i| float_param(&MACRO_KNOBS[i])),
            switches: std::array::from_fn(|i| bool_param(&MACRO_SWITCHES[i], MACRO_SWITCHES[i].name.to_owned())),
            snapshots: Arc::new(Snapshots::new()),
            preset: Arc::new(CurrentPreset::default()),
            board: Arc::new(Board::with_default()),
            size_override: std::sync::atomic::AtomicI8::new(-1),
        }
    }
}

/// Version 1 → 2: nothing to change (the macros start at their defaults, unmapped).
pub fn v1_to_v2(_state: &mut nih_plug::prelude::PluginState) {}

/// The engine's settings from plain values by id.
pub fn rig_settings(p: impl Fn(&str) -> f32) -> RigSettings {
    RigSettings { input_db: p(INPUT.id), out_db: p(OUT_GAIN.id), mix: p(MIX.id) / 100.0, bypass: p(BYPASS.id) >= 0.5 }
}

impl RigParams {
    pub fn settings(&self) -> RigSettings {
        RigSettings { input_db: self.input.value(), out_db: self.out_gain.value(), mix: self.mix.value() / 100.0, bypass: self.bypass.value() }
    }

    pub fn model_size(&self) -> f64 {
        let o = self.size_override.load(std::sync::atomic::Ordering::Relaxed);
        size_value(if o >= 0 { o as usize } else { self.size.value().max(0) as usize })
    }
}

// SAFETY: every `ParamPtr` points into `self`, which nih-plug keeps alive (in an `Arc`) for as
// long as it uses the map. The ids match `manifest()` (checked by tests/params.rs).
unsafe impl Params for RigParams {
    fn serialize_fields(&self) -> std::collections::BTreeMap<String, String> {
        let live = mite_plugin::state::live_values(self, &sound_ids());
        let mut fields = mite_plugin::state::session_fields(STATE_VERSION, &self.snapshots, &self.preset, live);
        fields.insert(BOARD_KEY.to_owned(), self.board.to_json());
        fields
    }

    fn deserialize_fields(&self, serialized: &std::collections::BTreeMap<String, String>) {
        mite_plugin::state::load_session_fields(serialized, &self.snapshots, &self.preset);
        self.board.load_json(serialized.get(BOARD_KEY).map(String::as_str));
    }

    fn param_map(&self) -> Vec<(String, ParamPtr, String)> {
        let p = |id: &str, ptr: ParamPtr| (id.to_owned(), ptr, String::new());
        let mut v = vec![p(INPUT.id, self.input.as_ptr()), p(SIZE.id, self.size.as_ptr()), p(MIX.id, self.mix.as_ptr()), p(OUT_GAIN.id, self.out_gain.as_ptr()), p(BYPASS.id, self.bypass.as_ptr())];
        v.extend(self.macros.iter().zip(&MACRO_KNOBS).map(|(m, s)| p(s.id, m.as_ptr())));
        v.extend(self.switches.iter().zip(&MACRO_SWITCHES).map(|(m, s)| p(s.id, m.as_ptr())));
        v
    }
}
