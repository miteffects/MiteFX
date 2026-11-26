//! Mite Tape parameters (BLUEPRINT §6, "Mite Tape").
//!
//! IDs are frozen once released (`tests/golden/mite_tape.params.txt`).

use mite_core::presets::CurrentPreset;
use mite_core::snapshots::Snapshots;
use mite_core::{BoolSpec, ChoiceSpec, ParamDisplay, ParamScale, ParamSpec};
use mite_dsp::tape::{MachineSettings, Speed, Standard};
use mite_plugin::{bool_param, choice_param, float_param};
use nih_plug::prelude::*;

/// The state format version (`mite_plugin::state`).
pub const STATE_VERSION: u32 = 1;

#[allow(clippy::too_many_arguments)] // one line per parameter reads best
const fn spec(id: &'static str, name: &'static str, unit: &'static str, min: f32, max: f32, default: f32, scale: ParamScale, decimals: u8, wheel: (f32, f32), display: ParamDisplay) -> ParamSpec {
    ParamSpec { id, name, unit, min, max, default, scale, smoothing_ms: 20.0, decimals, wheel_step: wheel.0, wheel_step_fine: wheel.1, display }
}

/// Record level: how hard the signal hits the tape. It comes back off after the tape, so it moves
/// the saturation, not the volume (0 dB: −18 dBFS is the deck's operating level).
pub const LEVEL: ParamSpec = spec("level", "Level", "dB", mite_dsp::tape::DRIVE_DB.0, mite_dsp::tape::DRIVE_DB.1, 0.0, ParamScale::Linear, 1, (0.5, 0.1), ParamDisplay::Number);
/// −100 % under-biased (brighter, grittier) … +100 % over-biased (cleaner, duller).
pub const BIAS: ParamSpec = spec("bias", "Bias", "%", -100.0, 100.0, 0.0, ParamScale::Linear, 0, (5.0, 1.0), ParamDisplay::Number);
/// By `mite_dsp::tape::Speed` index (append only).
pub const SPEED: ChoiceSpec = ChoiceSpec { id: "speed", name: "Speed", choices: &Speed::NAMES, default: 2 };
/// By `mite_dsp::tape::Standard` index (append only).
pub const EQ: ChoiceSpec = ChoiceSpec { id: "eq", name: "EQ", choices: &Standard::NAMES, default: 0 };
pub const WOW: ParamSpec = spec("wow", "Wow", "%", 0.0, 100.0, 10.0, ParamScale::Linear, 0, (5.0, 1.0), ParamDisplay::Number);
pub const FLUTTER: ParamSpec = spec("flutter", "Flutter", "%", 0.0, 100.0, 10.0, ParamScale::Linear, 0, (5.0, 1.0), ParamDisplay::Number);
pub const HISS: ParamSpec = spec("hiss", "Hiss", "%", 0.0, 100.0, 0.0, ParamScale::Linear, 0, (5.0, 1.0), ParamDisplay::Number);
pub const MIX: ParamSpec = spec("mix", "Mix", "%", 0.0, 100.0, 100.0, ParamScale::Linear, 0, (5.0, 1.0), ParamDisplay::Number);
pub const OUT_GAIN: ParamSpec = spec("out_gain", "Output", "dB", -24.0, 24.0, 0.0, ParamScale::Linear, 1, (0.5, 0.1), ParamDisplay::Number);
pub const BYPASS: BoolSpec = BoolSpec { id: "bypass", name: "Bypass", default: false };

const CONTINUOUS: [ParamSpec; 7] = [LEVEL, BIAS, WOW, FLUTTER, HISS, MIX, OUT_GAIN];

pub fn manifest() -> String {
    [
        LEVEL.manifest_line(),
        BIAS.manifest_line(),
        SPEED.manifest_line(),
        EQ.manifest_line(),
        WOW.manifest_line(),
        FLUTTER.manifest_line(),
        HISS.manifest_line(),
        MIX.manifest_line(),
        OUT_GAIN.manifest_line(),
        BYPASS.manifest_line(),
    ]
    .join("\n")
        + "\n"
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

pub fn spec_for(id: &str) -> Option<ParamSpec> {
    CONTINUOUS.iter().find(|s| s.id == id).copied()
}

/// Everything the engine needs.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TapeSettings {
    pub machine: MachineSettings,
    /// 0…1.
    pub mix: f32,
    pub out_db: f32,
    pub bypass: bool,
}

impl Default for TapeSettings {
    fn default() -> Self {
        tape_settings(|id| match id {
            "level" => LEVEL.default,
            "bias" => BIAS.default,
            "speed" => SPEED.default as f32,
            "eq" => EQ.default as f32,
            "wow" => WOW.default,
            "flutter" => FLUTTER.default,
            "hiss" => HISS.default,
            "mix" => MIX.default,
            "out_gain" => OUT_GAIN.default,
            _ => 0.0,
        })
    }
}

/// The engine's settings from plain values by id.
pub fn tape_settings(p: impl Fn(&str) -> f32) -> TapeSettings {
    let index = |v: f32| v.round().max(0.0) as usize;
    TapeSettings {
        machine: MachineSettings {
            on: true,
            drive_db: p(LEVEL.id),
            bias: p(BIAS.id) / 100.0,
            speed: Speed::from_index(index(p(SPEED.id))),
            standard: Standard::from_index(index(p(EQ.id))),
            wow: p(WOW.id) / 100.0,
            flutter: p(FLUTTER.id) / 100.0,
            hiss: p(HISS.id) / 100.0,
        },
        mix: p(MIX.id) / 100.0,
        out_db: p(OUT_GAIN.id),
        bypass: p(BYPASS.id) >= 0.5,
    }
}

pub struct TapeParams {
    pub level: FloatParam,
    pub bias: FloatParam,
    pub speed: IntParam,
    pub eq: IntParam,
    pub wow: FloatParam,
    pub flutter: FloatParam,
    pub hiss: FloatParam,
    pub mix: FloatParam,
    pub out_gain: FloatParam,
    pub bypass: BoolParam,
    pub snapshots: std::sync::Arc<Snapshots>,
    pub preset: std::sync::Arc<CurrentPreset>,
}

impl Default for TapeParams {
    fn default() -> Self {
        Self {
            level: float_param(&LEVEL),
            bias: float_param(&BIAS),
            speed: choice_param(&SPEED, SPEED.name.to_owned()),
            eq: choice_param(&EQ, EQ.name.to_owned()),
            wow: float_param(&WOW),
            flutter: float_param(&FLUTTER),
            hiss: float_param(&HISS),
            mix: float_param(&MIX),
            out_gain: float_param(&OUT_GAIN),
            // An ordinary parameter, never the host's bypass (REAPER stops processing a
            // host-bypassed plugin, and then the editor could not turn it off).
            bypass: bool_param(&BYPASS, BYPASS.name.to_owned()),
            snapshots: std::sync::Arc::new(Snapshots::new()),
            preset: std::sync::Arc::new(CurrentPreset::default()),
        }
    }
}

impl TapeParams {
    fn plain(&self, id: &str) -> f32 {
        match id {
            "level" => self.level.value(),
            "bias" => self.bias.value(),
            "speed" => self.speed.value() as f32,
            "eq" => self.eq.value() as f32,
            "wow" => self.wow.value(),
            "flutter" => self.flutter.value(),
            "hiss" => self.hiss.value(),
            "mix" => self.mix.value(),
            "out_gain" => self.out_gain.value(),
            "bypass" => f32::from(u8::from(self.bypass.value())),
            _ => 0.0,
        }
    }

    pub fn settings(&self) -> TapeSettings {
        tape_settings(|id| self.plain(id))
    }
}

// SAFETY: every `ParamPtr` points into `self`, which nih-plug keeps alive (in an `Arc`) for as
// long as it uses the map. The ids match `manifest()` (checked by tests/params.rs).
unsafe impl Params for TapeParams {
    fn serialize_fields(&self) -> std::collections::BTreeMap<String, String> {
        let live = mite_plugin::state::live_values(self, &sound_ids());
        mite_plugin::state::session_fields(STATE_VERSION, &self.snapshots, &self.preset, live)
    }

    fn deserialize_fields(&self, serialized: &std::collections::BTreeMap<String, String>) {
        mite_plugin::state::load_session_fields(serialized, &self.snapshots, &self.preset);
    }

    fn param_map(&self) -> Vec<(String, ParamPtr, String)> {
        let p = |id: &str, ptr: ParamPtr, group: &str| (id.to_owned(), ptr, group.to_owned());
        vec![
            p(LEVEL.id, self.level.as_ptr(), "Tape"),
            p(BIAS.id, self.bias.as_ptr(), "Tape"),
            p(SPEED.id, self.speed.as_ptr(), "Tape"),
            p(EQ.id, self.eq.as_ptr(), "Tape"),
            p(WOW.id, self.wow.as_ptr(), "Transport"),
            p(FLUTTER.id, self.flutter.as_ptr(), "Transport"),
            p(HISS.id, self.hiss.as_ptr(), "Tape"),
            p(MIX.id, self.mix.as_ptr(), ""),
            p(OUT_GAIN.id, self.out_gain.as_ptr(), ""),
            p(BYPASS.id, self.bypass.as_ptr(), ""),
        ]
    }
}
