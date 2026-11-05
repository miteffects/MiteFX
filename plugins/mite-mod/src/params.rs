//! Mite Mod parameters (docs/design/mod-brief.md §3).
//!
//! IDs are frozen once released (`tests/golden/mite_mod.params.txt`).

use mite_core::presets::CurrentPreset;
use mite_core::snapshots::Snapshots;
use mite_core::{BoolSpec, ChoiceSpec, ParamDisplay, ParamScale, ParamSpec};
use mite_dsp::modulation::{ModSettings, Mode, Rotor, Shape, DIVISIONS, MAX_DELAY_MS, MICRO_MAX_CENTS, MIN_DELAY_MS, MIN_MANUAL_MS, STAGE_COUNTS, TZ_MAX_MS};
use mite_plugin::{bool_param, choice_param, float_param};
use nih_plug::prelude::*;

use crate::dsp::Output;

/// The state format version (`mite_plugin::state`).
///
/// - 1 (2026-11-05): the five modes.
/// - 2 (2026-11-05): the rotary, appended to `mode` (snapshots re-normalised: `migrate`), and its
///   `rotor`, `balance` and `inertia`, appended.
/// - 3 (2026-11-05): the micro pitch shifter, appended to `mode` (snapshots re-normalised again),
///   and its `detune`, appended.
/// - 4 (2026-11-05): the modulation matrix (`mite_core::modmatrix`: `lfo{N}_*`, `env_*`,
///   `m{N}_*`), appended; older states get it empty.
pub const STATE_VERSION: u32 = 4;

#[allow(clippy::too_many_arguments)] // one line per parameter reads best
const fn spec(id: &'static str, name: &'static str, unit: &'static str, min: f32, max: f32, default: f32, scale: ParamScale, decimals: u8, wheel: (f32, f32), display: ParamDisplay) -> ParamSpec {
    ParamSpec { id, name, unit, min, max, default, scale, smoothing_ms: 20.0, decimals, wheel_step: wheel.0, wheel_step_fine: wheel.1, display }
}

const LOG_WHEEL: (f32, f32) = (1.0 / 24.0, 1.0 / 96.0);

const DIVISION_NAMES: [&str; DIVISIONS.len()] = {
    let mut names = [""; DIVISIONS.len()];
    let mut i = 0;
    while i < DIVISIONS.len() {
        names[i] = DIVISIONS[i].0;
        i += 1;
    }
    names
};

pub const MODE: ChoiceSpec = ChoiceSpec { id: "mode", name: "Mode", choices: &Mode::NAMES, default: 0 };

// ---- Motion -------------------------------------------------------------------------------------

pub const RATE: ParamSpec = spec("rate", "Rate", "Hz", 0.02, 20.0, 0.6, ParamScale::Log, 2, LOG_WHEEL, ParamDisplay::Number);
pub const SYNC: BoolSpec = BoolSpec { id: "sync", name: "Sync", default: false };
pub const DIVISION: ChoiceSpec = ChoiceSpec { id: "division", name: "Division", choices: &DIVISION_NAMES, default: 6 };
pub const SHAPE: ChoiceSpec = ChoiceSpec { id: "shape", name: "Shape", choices: &Shape::NAMES, default: 0 };
pub const DEPTH: ParamSpec = spec("depth", "Depth", "%", 0.0, 100.0, 50.0, ParamScale::Linear, 0, (2.0, 0.5), ParamDisplay::Number);
pub const SPREAD: ParamSpec = spec("spread", "Spread", "°", 0.0, 180.0, 90.0, ParamScale::Linear, 0, (5.0, 1.0), ParamDisplay::Number);

// ---- Per mode -----------------------------------------------------------------------------------

pub const FEEDBACK: ParamSpec = spec("feedback", "Feedback", "%", -95.0, 95.0, 0.0, ParamScale::Linear, 0, (2.0, 0.5), ParamDisplay::Number);
pub const DELAY: ParamSpec = spec("delay", "Delay", "ms", MIN_DELAY_MS, MAX_DELAY_MS, 12.0, ParamScale::Log, 1, LOG_WHEEL, ParamDisplay::Time);
pub const VOICES: ChoiceSpec = ChoiceSpec { id: "voices", name: "Voices", choices: &["1", "2", "3", "4"], default: 1 };
pub const MANUAL: ParamSpec = spec("manual", "Manual", "ms", MIN_MANUAL_MS, TZ_MAX_MS, 1.5, ParamScale::Log, 2, LOG_WHEEL, ParamDisplay::Time);
pub const THRU_ZERO: BoolSpec = BoolSpec { id: "thru_zero", name: "Through Zero", default: false };
pub const CENTRE: ParamSpec = spec("centre", "Centre", "Hz", 50.0, 8000.0, 800.0, ParamScale::Log, 0, LOG_WHEEL, ParamDisplay::Frequency);
pub const STAGES: ChoiceSpec = ChoiceSpec { id: "stages", name: "Stages", choices: &["2", "4", "6", "8", "12", "16", "24"], default: 1 };
pub const ROTOR: ChoiceSpec = ChoiceSpec { id: "rotor", name: "Rotor Speed", choices: &Rotor::NAMES, default: 0 };
pub const BALANCE: ParamSpec = spec("balance", "Balance", "%", -100.0, 100.0, 0.0, ParamScale::Linear, 0, (2.0, 0.5), ParamDisplay::Number);
pub const DETUNE: ParamSpec = spec("detune", "Detune", "ct", 0.0, MICRO_MAX_CENTS, 9.0, ParamScale::Linear, 1, (0.5, 0.1), ParamDisplay::Number);
pub const INERTIA: ParamSpec = spec("inertia", "Inertia", "%", 0.0, 100.0, 50.0, ParamScale::Linear, 0, (2.0, 0.5), ParamDisplay::Number);

// ---- Sound --------------------------------------------------------------------------------------

pub const MIX: ParamSpec = spec("mix", "Mix", "%", 0.0, 100.0, 50.0, ParamScale::Linear, 0, (2.0, 0.5), ParamDisplay::Number);
pub const WIDTH: ParamSpec = spec("width", "Width", "%", 0.0, 200.0, 100.0, ParamScale::Linear, 0, (5.0, 1.0), ParamDisplay::Number);
pub const LOW_CUT: ParamSpec = spec("low_cut", "Low Cut", "Hz", 20.0, 2000.0, 20.0, ParamScale::Log, 0, LOG_WHEEL, ParamDisplay::Frequency);
pub const HIGH_CUT: ParamSpec = spec("high_cut", "High Cut", "Hz", 1000.0, 20_000.0, 20_000.0, ParamScale::Log, 0, LOG_WHEEL, ParamDisplay::Frequency);

// ---- Output -------------------------------------------------------------------------------------

pub const OUT_GAIN: ParamSpec = spec("out_gain", "Output", "dB", -24.0, 24.0, 0.0, ParamScale::Linear, 1, (0.5, 0.1), ParamDisplay::Number);
pub const BYPASS: BoolSpec = BoolSpec { id: "bypass", name: "Bypass", default: false };

/// What the modulation matrix can move (`m{N}_dst`): frozen, append only.
pub static MOD_TARGETS: [&str; 16] = ["None", "Rate", "Depth", "Spread", "Feedback", "Delay", "Manual", "Centre", "Mix", "Width", "Low Cut", "High Cut", "Output", "Balance", "Inertia", "Detune"];
pub static MOD_TARGET_IDS: [&str; 16] = ["none", "rate", "depth", "spread", "feedback", "delay", "manual", "centre", "mix", "width", "low_cut", "high_cut", "out_gain", "balance", "inertia", "detune"];
pub const TARGETS: mite_core::modmatrix::Targets = mite_core::modmatrix::Targets { names: &MOD_TARGETS, ids: &MOD_TARGET_IDS };
pub const M_DST: ChoiceSpec = ChoiceSpec { id: "dst", name: "Target", choices: &MOD_TARGETS, default: 0 };

const CONTINUOUS: [ParamSpec; 15] = [RATE, DEPTH, SPREAD, FEEDBACK, DELAY, MANUAL, CENTRE, MIX, WIDTH, LOW_CUT, HIGH_CUT, OUT_GAIN, BALANCE, INERTIA, DETUNE];

pub fn manifest() -> String {
    let mut lines = vec![
        MODE.manifest_line(),
        RATE.manifest_line(),
        SYNC.manifest_line(),
        DIVISION.manifest_line(),
        SHAPE.manifest_line(),
        DEPTH.manifest_line(),
        SPREAD.manifest_line(),
        FEEDBACK.manifest_line(),
        DELAY.manifest_line(),
        VOICES.manifest_line(),
        MANUAL.manifest_line(),
        THRU_ZERO.manifest_line(),
        CENTRE.manifest_line(),
        STAGES.manifest_line(),
        MIX.manifest_line(),
        WIDTH.manifest_line(),
        LOW_CUT.manifest_line(),
        HIGH_CUT.manifest_line(),
        OUT_GAIN.manifest_line(),
        BYPASS.manifest_line(),
        ROTOR.manifest_line(),
        BALANCE.manifest_line(),
        INERTIA.manifest_line(),
        DETUNE.manifest_line(),
    ];
    lines.extend(mite_core::modmatrix::manifest_lines(&M_DST, false));
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

/// Every continuous spec by its id, for stand-in hosts.
pub fn spec_for(id: &str) -> Option<ParamSpec> {
    CONTINUOUS.iter().find(|s| s.id == id).copied().or_else(|| mite_core::modmatrix::spec_for(id))
}

pub struct ModParams {
    pub mode: IntParam,
    pub rate: FloatParam,
    pub sync: BoolParam,
    pub division: IntParam,
    pub shape: IntParam,
    pub depth: FloatParam,
    pub spread: FloatParam,
    pub feedback: FloatParam,
    pub delay: FloatParam,
    pub voices: IntParam,
    pub manual: FloatParam,
    pub thru_zero: BoolParam,
    pub centre: FloatParam,
    pub stages: IntParam,
    pub mix: FloatParam,
    pub width: FloatParam,
    pub low_cut: FloatParam,
    pub high_cut: FloatParam,
    pub out_gain: FloatParam,
    pub bypass: BoolParam,
    pub rotor: IntParam,
    pub balance: FloatParam,
    pub inertia: FloatParam,
    pub detune: FloatParam,
    pub matrix: mite_plugin::matrix::MatrixParams,
    pub snapshots: std::sync::Arc<Snapshots>,
    pub preset: std::sync::Arc<CurrentPreset>,
}

impl Default for ModParams {
    fn default() -> Self {
        let choice = |c: &'static ChoiceSpec| choice_param(c, c.name.to_owned());
        let flag = |b: &'static BoolSpec| bool_param(b, b.name.to_owned());
        Self {
            mode: choice(&MODE),
            rate: float_param(&RATE),
            sync: flag(&SYNC),
            division: choice(&DIVISION),
            shape: choice(&SHAPE),
            depth: float_param(&DEPTH),
            spread: float_param(&SPREAD),
            feedback: float_param(&FEEDBACK),
            delay: float_param(&DELAY),
            voices: choice(&VOICES),
            manual: float_param(&MANUAL),
            thru_zero: flag(&THRU_ZERO),
            centre: float_param(&CENTRE),
            stages: choice(&STAGES),
            mix: float_param(&MIX),
            width: float_param(&WIDTH),
            low_cut: float_param(&LOW_CUT),
            high_cut: float_param(&HIGH_CUT),
            out_gain: float_param(&OUT_GAIN),
            // An ordinary parameter, never the host's bypass (REAPER stops processing a
            // host-bypassed plugin, and then the editor could not turn it off).
            bypass: flag(&BYPASS),
            rotor: choice(&ROTOR),
            balance: float_param(&BALANCE),
            inertia: float_param(&INERTIA),
            detune: float_param(&DETUNE),
            matrix: mite_plugin::matrix::MatrixParams::new(&M_DST, false),
            snapshots: std::sync::Arc::new(Snapshots::new()),
            preset: std::sync::Arc::new(CurrentPreset::default()),
        }
    }
}

/// The engine's settings from plain values by id.
pub fn mod_settings(p: impl Fn(&str) -> f32) -> ModSettings {
    let index = |id: &str| p(id).round().max(0.0) as usize;
    ModSettings {
        mode: Mode::from_index(index(MODE.id)),
        rate_hz: p(RATE.id),
        sync: p(SYNC.id) >= 0.5,
        division: index(DIVISION.id).min(DIVISIONS.len() - 1),
        shape: Shape::from_index(index(SHAPE.id)),
        depth: p(DEPTH.id) / 100.0,
        spread: p(SPREAD.id) / 180.0,
        feedback: p(FEEDBACK.id) / 100.0,
        mix: p(MIX.id) / 100.0,
        width: p(WIDTH.id) / 100.0,
        low_cut_hz: p(LOW_CUT.id),
        high_cut_hz: p(HIGH_CUT.id),
        delay_ms: p(DELAY.id),
        voices: index(VOICES.id) + 1,
        manual_ms: p(MANUAL.id),
        thru_zero: p(THRU_ZERO.id) >= 0.5,
        centre_hz: p(CENTRE.id),
        stages: STAGE_COUNTS[index(STAGES.id).min(STAGE_COUNTS.len() - 1)],
        rotor: Rotor::from_index(index(ROTOR.id)),
        balance: p(BALANCE.id) / 100.0,
        inertia: p(INERTIA.id) / 100.0,
        detune_cents: p(DETUNE.id),
    }
}

/// The settings and output from plain values by id, with the matrix's `offsets` (one per target)
/// applied: each target moved along its knob (`mite_core::modmatrix::modulate`).
pub fn modulated_settings(p: impl Fn(&str) -> f32, offsets: &[f32]) -> (ModSettings, Output) {
    let at = |id: &str| {
        let v = p(id);
        match TARGETS.index(id) {
            Some(t) if offsets.get(t).is_some_and(|o| *o != 0.0) => spec_for(id).map_or(v, |spec| mite_core::modmatrix::modulate(&spec, v, offsets[t])),
            _ => v,
        }
    };
    (mod_settings(at), Output { gain_db: at(OUT_GAIN.id), bypass: p(BYPASS.id) >= 0.5 })
}

impl ModParams {
    pub fn plain(&self, id: &str) -> f32 {
        let flag = |p: &BoolParam| f32::from(u8::from(p.value()));
        match id {
            "mode" => self.mode.value() as f32,
            "rate" => self.rate.value(),
            "sync" => flag(&self.sync),
            "division" => self.division.value() as f32,
            "shape" => self.shape.value() as f32,
            "depth" => self.depth.value(),
            "spread" => self.spread.value(),
            "feedback" => self.feedback.value(),
            "delay" => self.delay.value(),
            "voices" => self.voices.value() as f32,
            "manual" => self.manual.value(),
            "thru_zero" => flag(&self.thru_zero),
            "centre" => self.centre.value(),
            "stages" => self.stages.value() as f32,
            "mix" => self.mix.value(),
            "width" => self.width.value(),
            "low_cut" => self.low_cut.value(),
            "high_cut" => self.high_cut.value(),
            "rotor" => self.rotor.value() as f32,
            "balance" => self.balance.value(),
            "inertia" => self.inertia.value(),
            "detune" => self.detune.value(),
            "out_gain" => self.out_gain.value(),
            "bypass" => f32::from(u8::from(self.bypass.value())),
            _ => 0.0,
        }
    }

    pub fn settings(&self) -> ModSettings {
        mod_settings(|id| self.plain(id))
    }

    pub fn output(&self) -> Output {
        Output { gain_db: self.out_gain.value(), bypass: self.bypass.value() }
    }

    /// The settings and output with the matrix's `offsets` (one per target) applied.
    pub fn modulated(&self, offsets: &[f32]) -> (ModSettings, Output) {
        modulated_settings(|id| self.plain(id), offsets)
    }
}

// SAFETY: every `ParamPtr` points into `self`, which nih-plug keeps alive (in an `Arc`) for as
// long as it uses the map. The ids match `manifest()` (checked by tests/params.rs).
unsafe impl Params for ModParams {
    fn serialize_fields(&self) -> std::collections::BTreeMap<String, String> {
        let live = mite_plugin::state::live_values(self, &sound_ids());
        mite_plugin::state::session_fields(STATE_VERSION, &self.snapshots, &self.preset, live)
    }

    fn deserialize_fields(&self, serialized: &std::collections::BTreeMap<String, String>) {
        mite_plugin::state::load_session_fields(serialized, &self.snapshots, &self.preset);
    }

    fn param_map(&self) -> Vec<(String, ParamPtr, String)> {
        let p = |id: &str, ptr: ParamPtr, group: &str| (id.to_owned(), ptr, group.to_owned());
        let mut map = vec![
            p(MODE.id, self.mode.as_ptr(), ""),
            p(RATE.id, self.rate.as_ptr(), "Motion"),
            p(SYNC.id, self.sync.as_ptr(), "Motion"),
            p(DIVISION.id, self.division.as_ptr(), "Motion"),
            p(SHAPE.id, self.shape.as_ptr(), "Motion"),
            p(DEPTH.id, self.depth.as_ptr(), "Motion"),
            p(SPREAD.id, self.spread.as_ptr(), "Motion"),
            p(FEEDBACK.id, self.feedback.as_ptr(), "Mode"),
            p(DELAY.id, self.delay.as_ptr(), "Mode"),
            p(VOICES.id, self.voices.as_ptr(), "Mode"),
            p(MANUAL.id, self.manual.as_ptr(), "Mode"),
            p(THRU_ZERO.id, self.thru_zero.as_ptr(), "Mode"),
            p(CENTRE.id, self.centre.as_ptr(), "Mode"),
            p(STAGES.id, self.stages.as_ptr(), "Mode"),
            p(MIX.id, self.mix.as_ptr(), "Sound"),
            p(WIDTH.id, self.width.as_ptr(), "Sound"),
            p(LOW_CUT.id, self.low_cut.as_ptr(), "Sound"),
            p(HIGH_CUT.id, self.high_cut.as_ptr(), "Sound"),
            p(OUT_GAIN.id, self.out_gain.as_ptr(), ""),
            p(BYPASS.id, self.bypass.as_ptr(), ""),
            p(ROTOR.id, self.rotor.as_ptr(), "Mode"),
            p(BALANCE.id, self.balance.as_ptr(), "Mode"),
            p(INERTIA.id, self.inertia.as_ptr(), "Mode"),
            p(DETUNE.id, self.detune.as_ptr(), "Mode"),
        ];
        self.matrix.param_map(&mut map);
        map
    }
}
