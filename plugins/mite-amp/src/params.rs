//! Mite Amp parameters (docs/design/amp-brief.md §3).
//!
//! IDs are frozen once released (`tests/golden/mite_amp.params.txt`). The loaded captures and
//! cabinet are not parameters: their files (and where they came from) travel with the state as
//! the `rig` field.

use std::sync::Arc;

use mite_core::presets::CurrentPreset;
use mite_core::snapshots::Snapshots;
use mite_core::{BoolSpec, ChoiceSpec, ParamDisplay, ParamScale, ParamSpec};
use mite_plugin::{bool_param, choice_param, float_param};
use nih_plug::prelude::*;

use crate::dsp::AmpSettings;
use crate::rig::Rig;

/// The state format version (`mite_plugin::state`).
/// 2 (2026-11-11): each slot's gain, level and (pedal) tone appended.
pub const STATE_VERSION: u32 = 2;

/// The state field holding the rig (files and their sources).
pub const RIG_KEY: &str = "rig";

#[allow(clippy::too_many_arguments)] // one line per parameter reads best
const fn spec(id: &'static str, name: &'static str, unit: &'static str, min: f32, max: f32, default: f32, scale: ParamScale, decimals: u8, wheel: (f32, f32), display: ParamDisplay) -> ParamSpec {
    ParamSpec { id, name, unit, min, max, default, scale, smoothing_ms: 20.0, decimals, wheel_step: wheel.0, wheel_step_fine: wheel.1, display }
}

// ---- Input ------------------------------------------------------------------------------

pub const INPUT: ParamSpec = spec("input", "Input", "dB", -24.0, 24.0, 0.0, ParamScale::Linear, 1, (0.5, 0.1), ParamDisplay::Number);
pub const GATE: BoolSpec = BoolSpec { id: "gate", name: "Gate", default: true };
/// Below this (the input, before the models) the gate closes on the models' output.
pub const GATE_THRESHOLD: ParamSpec = spec("gate_threshold", "Gate Threshold", "dB", -100.0, -20.0, -80.0, ParamScale::Linear, 1, (1.0, 0.1), ParamDisplay::Number);
/// Input calibration: the level the interface's 0 dBFS stands for, matched to what the capture
/// was made at (when the file says).
pub const CALIBRATE: BoolSpec = BoolSpec { id: "calibrate", name: "Calibrate Input", default: false };
pub const INTERFACE_LEVEL: ParamSpec = spec("interface_level", "Interface Level", "dBu", -10.0, 30.0, 12.0, ParamScale::Linear, 1, (0.5, 0.1), ParamDisplay::Number);

// ---- The rig ----------------------------------------------------------------------------

pub const PEDAL: BoolSpec = BoolSpec { id: "pedal", name: "Pedal", default: true };
pub const AMP: BoolSpec = BoolSpec { id: "amp", name: "Amp", default: true };
pub const CAB: BoolSpec = BoolSpec { id: "cab", name: "Cab", default: true };
/// Captures that come in sizes (A2 Full and Lite) play this one.
pub const SIZE_NAMES: [&str; 2] = ["Full", "Lite"];
pub const SIZE: ChoiceSpec = ChoiceSpec { id: "size", name: "Model Size", choices: &SIZE_NAMES, default: 0 };
/// Every capture played at the same loudness (when the file says how loud it is).
pub const NORMALIZE: BoolSpec = BoolSpec { id: "normalize", name: "Normalize", default: true };

// ---- Tone (after the amp, before the cab) -----------------------------------------------

pub const BASS: ParamSpec = spec("bass", "Bass", "dB", -12.0, 12.0, 0.0, ParamScale::Linear, 1, (0.5, 0.1), ParamDisplay::Number);
pub const MIDDLE: ParamSpec = spec("middle", "Middle", "dB", -12.0, 12.0, 0.0, ParamScale::Linear, 1, (0.5, 0.1), ParamDisplay::Number);
pub const TREBLE: ParamSpec = spec("treble", "Treble", "dB", -12.0, 12.0, 0.0, ParamScale::Linear, 1, (0.5, 0.1), ParamDisplay::Number);
/// At its lowest it is off.
pub const LOW_CUT: ParamSpec = spec("low_cut", "Low Cut", "Hz", 20.0, 500.0, 20.0, ParamScale::Log, 0, (1.0 / 24.0, 1.0 / 96.0), ParamDisplay::Frequency);
/// At its highest it is off.
pub const HIGH_CUT: ParamSpec = spec("high_cut", "High Cut", "Hz", 2000.0, 20_000.0, 20_000.0, ParamScale::Log, 0, (1.0 / 24.0, 1.0 / 96.0), ParamDisplay::Frequency);

// ---- Output -----------------------------------------------------------------------------

pub const MIX: ParamSpec = spec("mix", "Mix", "%", 0.0, 100.0, 100.0, ParamScale::Linear, 0, (1.0, 0.1), ParamDisplay::Number);
pub const OUT_GAIN: ParamSpec = spec("out_gain", "Output", "dB", -24.0, 24.0, 0.0, ParamScale::Linear, 1, (0.5, 0.1), ParamDisplay::Number);
pub const BYPASS: BoolSpec = BoolSpec { id: "bypass", name: "Bypass", default: false };

// ---- Each slot's own (state 2) ----------------------------------------------------------

/// Into the pedal (how hard it is hit), out of it (into the amp), and its tone after it.
pub const PEDAL_GAIN: ParamSpec = spec("pedal_gain", "Pedal Gain", "dB", -24.0, 24.0, 0.0, ParamScale::Linear, 1, (0.5, 0.1), ParamDisplay::Number);
pub const PEDAL_LEVEL: ParamSpec = spec("pedal_level", "Pedal Level", "dB", -24.0, 24.0, 0.0, ParamScale::Linear, 1, (0.5, 0.1), ParamDisplay::Number);
pub const PEDAL_BASS: ParamSpec = spec("pedal_bass", "Pedal Bass", "dB", -12.0, 12.0, 0.0, ParamScale::Linear, 1, (0.5, 0.1), ParamDisplay::Number);
pub const PEDAL_MIDDLE: ParamSpec = spec("pedal_middle", "Pedal Middle", "dB", -12.0, 12.0, 0.0, ParamScale::Linear, 1, (0.5, 0.1), ParamDisplay::Number);
pub const PEDAL_TREBLE: ParamSpec = spec("pedal_treble", "Pedal Treble", "dB", -12.0, 12.0, 0.0, ParamScale::Linear, 1, (0.5, 0.1), ParamDisplay::Number);
/// Into the amp and out of it (its tone is BASS, MIDDLE, TREBLE).
pub const AMP_GAIN: ParamSpec = spec("amp_gain", "Amp Gain", "dB", -24.0, 24.0, 0.0, ParamScale::Linear, 1, (0.5, 0.1), ParamDisplay::Number);
pub const AMP_LEVEL: ParamSpec = spec("amp_level", "Amp Level", "dB", -24.0, 24.0, 0.0, ParamScale::Linear, 1, (0.5, 0.1), ParamDisplay::Number);
/// The cabinet's level (its cuts are LOW_CUT and HIGH_CUT).
pub const CAB_LEVEL: ParamSpec = spec("cab_level", "Cab Level", "dB", -24.0, 24.0, 0.0, ParamScale::Linear, 1, (0.5, 0.1), ParamDisplay::Number);

/// The continuous parameters.
pub const CONTINUOUS: [ParamSpec; 18] = [INPUT, GATE_THRESHOLD, INTERFACE_LEVEL, BASS, MIDDLE, TREBLE, LOW_CUT, HIGH_CUT, MIX, OUT_GAIN, PEDAL_GAIN, PEDAL_LEVEL, PEDAL_BASS, PEDAL_MIDDLE, PEDAL_TREBLE, AMP_GAIN, AMP_LEVEL, CAB_LEVEL];

pub fn spec_for(id: &str) -> Option<ParamSpec> {
    CONTINUOUS.iter().find(|s| s.id == id).copied()
}

/// Every parameter's manifest line, in host order.
pub fn manifest() -> String {
    [
        INPUT.manifest_line(),
        GATE.manifest_line(),
        GATE_THRESHOLD.manifest_line(),
        CALIBRATE.manifest_line(),
        INTERFACE_LEVEL.manifest_line(),
        PEDAL.manifest_line(),
        AMP.manifest_line(),
        CAB.manifest_line(),
        SIZE.manifest_line(),
        NORMALIZE.manifest_line(),
        BASS.manifest_line(),
        MIDDLE.manifest_line(),
        TREBLE.manifest_line(),
        LOW_CUT.manifest_line(),
        HIGH_CUT.manifest_line(),
        MIX.manifest_line(),
        OUT_GAIN.manifest_line(),
        BYPASS.manifest_line(),
        PEDAL_GAIN.manifest_line(),
        PEDAL_LEVEL.manifest_line(),
        PEDAL_BASS.manifest_line(),
        PEDAL_MIDDLE.manifest_line(),
        PEDAL_TREBLE.manifest_line(),
        AMP_GAIN.manifest_line(),
        AMP_LEVEL.manifest_line(),
        CAB_LEVEL.manifest_line(),
    ]
    .join("\n")
        + "\n"
}

pub fn ids() -> Vec<String> {
    manifest().lines().filter_map(|l| l.split(" | ").next()).map(str::to_owned).collect()
}

/// Monitoring, not sound: never in presets or snapshots. The interface level describes the
/// studio, not the sound, so it stays put across presets too.
pub fn is_monitoring(id: &str) -> bool {
    [BYPASS.id, INTERFACE_LEVEL.id, CALIBRATE.id].contains(&id)
}

pub fn sound_ids() -> Vec<String> {
    ids().into_iter().filter(|id| !is_monitoring(id)).collect()
}

pub struct AmpParams {
    pub input: FloatParam,
    pub gate: BoolParam,
    pub gate_threshold: FloatParam,
    pub calibrate: BoolParam,
    pub interface_level: FloatParam,
    pub pedal: BoolParam,
    pub amp: BoolParam,
    pub cab: BoolParam,
    pub size: IntParam,
    pub normalize: BoolParam,
    pub bass: FloatParam,
    pub middle: FloatParam,
    pub treble: FloatParam,
    pub low_cut: FloatParam,
    pub high_cut: FloatParam,
    pub mix: FloatParam,
    pub out_gain: FloatParam,
    pub bypass: BoolParam,
    pub pedal_gain: FloatParam,
    pub pedal_level: FloatParam,
    pub pedal_bass: FloatParam,
    pub pedal_middle: FloatParam,
    pub pedal_treble: FloatParam,
    pub amp_gain: FloatParam,
    pub amp_level: FloatParam,
    pub cab_level: FloatParam,
    pub snapshots: Arc<Snapshots>,
    pub preset: Arc<CurrentPreset>,
    /// The loaded files (saved with the state).
    pub rig: Arc<Rig>,
}

impl Default for AmpParams {
    fn default() -> Self {
        let flag = |s: &'static BoolSpec| bool_param(s, s.name.to_owned());
        Self {
            input: float_param(&INPUT),
            gate: flag(&GATE),
            gate_threshold: float_param(&GATE_THRESHOLD),
            calibrate: flag(&CALIBRATE),
            interface_level: float_param(&INTERFACE_LEVEL),
            pedal: flag(&PEDAL),
            amp: flag(&AMP),
            cab: flag(&CAB),
            size: choice_param(&SIZE, SIZE.name.to_owned()),
            normalize: flag(&NORMALIZE),
            bass: float_param(&BASS),
            middle: float_param(&MIDDLE),
            treble: float_param(&TREBLE),
            low_cut: float_param(&LOW_CUT),
            high_cut: float_param(&HIGH_CUT),
            mix: float_param(&MIX),
            out_gain: float_param(&OUT_GAIN),
            // An ordinary parameter, never the host's bypass (REAPER stops processing a
            // host-bypassed plugin, and then the editor could not turn it off).
            bypass: flag(&BYPASS),
            pedal_gain: float_param(&PEDAL_GAIN),
            pedal_level: float_param(&PEDAL_LEVEL),
            pedal_bass: float_param(&PEDAL_BASS),
            pedal_middle: float_param(&PEDAL_MIDDLE),
            pedal_treble: float_param(&PEDAL_TREBLE),
            amp_gain: float_param(&AMP_GAIN),
            amp_level: float_param(&AMP_LEVEL),
            cab_level: float_param(&CAB_LEVEL),
            snapshots: Arc::new(Snapshots::new()),
            preset: Arc::new(CurrentPreset::default()),
            rig: Arc::new(Rig::new()),
        }
    }
}

/// The engine's settings from plain values by id.
pub fn amp_settings(p: impl Fn(&str) -> f32) -> AmpSettings {
    let on = |id: &str| p(id) >= 0.5;
    AmpSettings {
        input_db: p(INPUT.id),
        gate: on(GATE.id),
        gate_db: p(GATE_THRESHOLD.id),
        calibrate: on(CALIBRATE.id),
        interface_dbu: p(INTERFACE_LEVEL.id),
        pedal: on(PEDAL.id),
        amp: on(AMP.id),
        cab: on(CAB.id),
        normalize: on(NORMALIZE.id),
        bass_db: p(BASS.id),
        middle_db: p(MIDDLE.id),
        treble_db: p(TREBLE.id),
        low_cut_hz: p(LOW_CUT.id),
        high_cut_hz: p(HIGH_CUT.id),
        mix: p(MIX.id) / 100.0,
        out_db: p(OUT_GAIN.id),
        bypass: on(BYPASS.id),
        pedal_gain_db: p(PEDAL_GAIN.id),
        pedal_level_db: p(PEDAL_LEVEL.id),
        pedal_bass_db: p(PEDAL_BASS.id),
        pedal_middle_db: p(PEDAL_MIDDLE.id),
        pedal_treble_db: p(PEDAL_TREBLE.id),
        amp_gain_db: p(AMP_GAIN.id),
        amp_level_db: p(AMP_LEVEL.id),
        cab_level_db: p(CAB_LEVEL.id),
    }
}

impl AmpParams {
    fn plain(&self, id: &str) -> f32 {
        let flag = |p: &BoolParam| f32::from(u8::from(p.value()));
        match id {
            "input" => self.input.value(),
            "gate" => flag(&self.gate),
            "gate_threshold" => self.gate_threshold.value(),
            "calibrate" => flag(&self.calibrate),
            "interface_level" => self.interface_level.value(),
            "pedal" => flag(&self.pedal),
            "amp" => flag(&self.amp),
            "cab" => flag(&self.cab),
            "normalize" => flag(&self.normalize),
            "bass" => self.bass.value(),
            "middle" => self.middle.value(),
            "treble" => self.treble.value(),
            "low_cut" => self.low_cut.value(),
            "high_cut" => self.high_cut.value(),
            "mix" => self.mix.value(),
            "out_gain" => self.out_gain.value(),
            "bypass" => flag(&self.bypass),
            "pedal_gain" => self.pedal_gain.value(),
            "pedal_level" => self.pedal_level.value(),
            "pedal_bass" => self.pedal_bass.value(),
            "pedal_middle" => self.pedal_middle.value(),
            "pedal_treble" => self.pedal_treble.value(),
            "amp_gain" => self.amp_gain.value(),
            "amp_level" => self.amp_level.value(),
            "cab_level" => self.cab_level.value(),
            _ => 0.0,
        }
    }

    pub fn settings(&self) -> AmpSettings {
        amp_settings(|id| self.plain(id))
    }

    /// The model size asked for: 1 (Full) or a value inside A2's Lite range.
    pub fn model_size(&self) -> f64 {
        size_value(self.size.value().max(0) as usize)
    }
}

/// The container size a choice index plays (Lite: the smallest member, Full: the largest).
pub fn size_value(index: usize) -> f64 {
    if index == 1 { 0.0 } else { 1.0 }
}

// SAFETY: every `ParamPtr` points into `self`, which nih-plug keeps alive (in an `Arc`) for as
// long as it uses the map. The ids match `manifest()` (checked by tests/params.rs).
unsafe impl Params for AmpParams {
    fn serialize_fields(&self) -> std::collections::BTreeMap<String, String> {
        let live = mite_plugin::state::live_values(self, &sound_ids());
        let mut fields = mite_plugin::state::session_fields(STATE_VERSION, &self.snapshots, &self.preset, live);
        if let Some(json) = self.rig.to_json() {
            fields.insert(RIG_KEY.to_owned(), json);
        }
        fields
    }

    fn deserialize_fields(&self, serialized: &std::collections::BTreeMap<String, String>) {
        mite_plugin::state::load_session_fields(serialized, &self.snapshots, &self.preset);
        self.rig.load_json(serialized.get(RIG_KEY).map(String::as_str));
    }

    fn param_map(&self) -> Vec<(String, ParamPtr, String)> {
        let p = |id: &str, ptr: ParamPtr, group: &str| (id.to_owned(), ptr, group.to_owned());
        vec![
            p(INPUT.id, self.input.as_ptr(), "Input"),
            p(GATE.id, self.gate.as_ptr(), "Input"),
            p(GATE_THRESHOLD.id, self.gate_threshold.as_ptr(), "Input"),
            p(CALIBRATE.id, self.calibrate.as_ptr(), "Input"),
            p(INTERFACE_LEVEL.id, self.interface_level.as_ptr(), "Input"),
            p(PEDAL.id, self.pedal.as_ptr(), "Rig"),
            p(AMP.id, self.amp.as_ptr(), "Rig"),
            p(CAB.id, self.cab.as_ptr(), "Rig"),
            p(SIZE.id, self.size.as_ptr(), "Rig"),
            p(NORMALIZE.id, self.normalize.as_ptr(), "Rig"),
            p(BASS.id, self.bass.as_ptr(), "Tone"),
            p(MIDDLE.id, self.middle.as_ptr(), "Tone"),
            p(TREBLE.id, self.treble.as_ptr(), "Tone"),
            p(LOW_CUT.id, self.low_cut.as_ptr(), "Tone"),
            p(HIGH_CUT.id, self.high_cut.as_ptr(), "Tone"),
            p(MIX.id, self.mix.as_ptr(), ""),
            p(OUT_GAIN.id, self.out_gain.as_ptr(), ""),
            p(BYPASS.id, self.bypass.as_ptr(), ""),
            p(PEDAL_GAIN.id, self.pedal_gain.as_ptr(), "Pedal"),
            p(PEDAL_LEVEL.id, self.pedal_level.as_ptr(), "Pedal"),
            p(PEDAL_BASS.id, self.pedal_bass.as_ptr(), "Pedal"),
            p(PEDAL_MIDDLE.id, self.pedal_middle.as_ptr(), "Pedal"),
            p(PEDAL_TREBLE.id, self.pedal_treble.as_ptr(), "Pedal"),
            p(AMP_GAIN.id, self.amp_gain.as_ptr(), "Amp"),
            p(AMP_LEVEL.id, self.amp_level.as_ptr(), "Amp"),
            p(CAB_LEVEL.id, self.cab_level.as_ptr(), "Cab"),
        ]
    }
}
