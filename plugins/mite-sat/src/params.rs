//! Mite Sat parameters (BLUEPRINT §6.7; docs/design/sat-brief.md §4).
//!
//! IDs are frozen once released (`tests/golden/mite_sat.params.txt`): globals by name, the five
//! crossovers as `x{N}_{param}`, the six bands as `b{N}_{param}`, the modulation sources as
//! `lfo{N}_*` and `env_*`, and the eight modulation slots as `m{N}_*`. Slots are fixed:
//! crossover slot N, when on, owns band N + 1 (the region above it); band 1 is always the lowest
//! region. Every parameter of the full design exists from the first release, at a neutral
//! default until its feature is built, so the state stays at version 1 as features arrive.

use mite_core::presets::CurrentPreset;
use mite_core::snapshots::Snapshots;
use mite_core::{BoolSpec, ChoiceSpec, DialCurve, ParamDisplay, ParamScale, ParamSpec};
use mite_dsp::sat::{BandSat, Model, SatSettings, BANDS, CROSSOVERS, DEFAULT_CROSSOVERS, DRIVE_MAX_DB, FACTORS, FREQ_MAX, FREQ_MIN, TONE_MAX_DB};
use mite_plugin::{bool_param, choice_param, float_param, float_param_named};
use nih_plug::prelude::*;

/// The state format version (`mite_plugin::state`). Bump it, with a migration step in
/// `MiteSat::filter_state` and a new golden state file, when a release changes what a state
/// means.
pub const STATE_VERSION: u32 = 1;

#[allow(clippy::too_many_arguments)] // one line per parameter reads best
const fn spec(id: &'static str, name: &'static str, unit: &'static str, min: f32, max: f32, default: f32, scale: ParamScale, decimals: u8, wheel: (f32, f32), display: ParamDisplay) -> ParamSpec {
    ParamSpec { id, name, unit, min, max, default, scale, smoothing_ms: 20.0, decimals, wheel_step: wheel.0, wheel_step_fine: wheel.1, display }
}

// ---- Global -------------------------------------------------------------------------------

pub const OVERSAMPLING: ChoiceSpec = ChoiceSpec { id: "oversampling", name: "Oversampling", choices: &["1×", "2×", "4×", "8×"], default: 1 };
/// Each band matches its loudness to what goes in (sat-brief §1.1).
pub const AUTO_GAIN: BoolSpec = BoolSpec { id: "auto_gain", name: "Auto Gain", default: true };
pub const MIX: ParamSpec = spec("mix", "Mix", "%", 0.0, 100.0, 100.0, ParamScale::Linear, 0, (5.0, 1.0), ParamDisplay::Number);
pub const OUT_PAN: ParamSpec = spec("out_pan", "Output Pan", "%", -100.0, 100.0, 0.0, ParamScale::Linear, 0, (5.0, 1.0), ParamDisplay::Number);
pub const OUT_GAIN: ParamSpec = spec("out_gain", "Output", "dB", -24.0, 24.0, 0.0, ParamScale::Linear, 1, (0.5, 0.1), ParamDisplay::Number);
pub const DELTA: BoolSpec = BoolSpec { id: "delta", name: "Delta", default: false };
pub const BYPASS: BoolSpec = BoolSpec { id: "bypass", name: "Bypass", default: false };

// ---- Crossovers ---------------------------------------------------------------------------

pub const X_ON: BoolSpec = BoolSpec { id: "on", name: "On", default: false };
pub const X_FREQ: ParamSpec = spec("freq", "Frequency", "Hz", FREQ_MIN, FREQ_MAX, 1000.0, ParamScale::Log, 0, (1.0 / 6.0, 1.0 / 48.0), ParamDisplay::Frequency);
pub static X_FREQS: [ParamSpec; CROSSOVERS] = [
    ParamSpec { default: DEFAULT_CROSSOVERS[0], ..X_FREQ },
    ParamSpec { default: DEFAULT_CROSSOVERS[1], ..X_FREQ },
    ParamSpec { default: DEFAULT_CROSSOVERS[2], ..X_FREQ },
    ParamSpec { default: DEFAULT_CROSSOVERS[3], ..X_FREQ },
    ParamSpec { default: DEFAULT_CROSSOVERS[4], ..X_FREQ },
];

// ---- Bands --------------------------------------------------------------------------------

/// The model, by `Model` index (append only).
pub const B_MODEL: ChoiceSpec = ChoiceSpec { id: "model", name: "Model", choices: &Model::NAMES, default: 1 };
pub const B_DRIVE: ParamSpec = spec("drive", "Drive", "dB", 0.0, DRIVE_MAX_DB, 0.0, ParamScale::Linear, 1, (0.5, 0.1), ParamDisplay::Number);
pub const B_BIAS: ParamSpec = spec("bias", "Bias", "%", -100.0, 100.0, 0.0, ParamScale::Linear, 0, (5.0, 1.0), ParamDisplay::Number);
pub const B_TONE: ParamSpec = spec("tone", "Tone", "dB", -TONE_MAX_DB, TONE_MAX_DB, 0.0, ParamScale::Linear, 1, (0.5, 0.1), ParamDisplay::Number);
pub const B_MIX: ParamSpec = spec("mix", "Band Mix", "%", 0.0, 100.0, 100.0, ParamScale::Linear, 0, (5.0, 1.0), ParamDisplay::Number);
pub const B_OUT: ParamSpec = spec("out", "Band Output", "dB", -24.0, 24.0, 0.0, ParamScale::Linear, 1, (0.5, 0.1), ParamDisplay::Number);
/// Per-band dynamics after the model: below 0 compresses, above expands (slice 6).
pub const B_DYN: ParamSpec = spec("dyn", "Dynamics", "%", -100.0, 100.0, 0.0, ParamScale::Linear, 0, (5.0, 1.0), ParamDisplay::Number);
pub const B_DYN_THR: ParamSpec = spec("dyn_thr", "Dynamics Threshold", "dBFS", -60.0, 0.0, -24.0, ParamScale::Linear, 1, (0.5, 0.1), ParamDisplay::Number);
pub const B_DYN_ATT: ParamSpec = spec("dyn_att", "Dynamics Attack", "ms", 0.1, 100.0, 10.0, ParamScale::Log, 1, (0.25, 1.0 / 16.0), ParamDisplay::Time);
pub const B_DYN_REL: ParamSpec = spec("dyn_rel", "Dynamics Release", "ms", 10.0, 1000.0, 150.0, ParamScale::Log, 0, (0.25, 1.0 / 16.0), ParamDisplay::Time);
/// What part of the stereo image the band saturates (slice 5).
pub const B_STEREO: ChoiceSpec = ChoiceSpec { id: "stereo", name: "Stereo", choices: &["Stereo", "Mid", "Side", "M/S"], default: 0 };
/// Clean, but still heard.
pub const B_BYPASS: BoolSpec = BoolSpec { id: "bypass", name: "Bypass", default: false };
/// A muted band drops out of the sum (sound: kept in presets).
pub const B_MUTE: BoolSpec = BoolSpec { id: "mute", name: "Mute", default: false };
/// Hear only this band (monitoring: never in presets).
pub const B_SOLO: BoolSpec = BoolSpec { id: "solo", name: "Solo", default: false };

/// Drive: 0–12 dB takes half the travel.
pub const DRIVE_DIAL: DialCurve = DialCurve(&[(0.0, 0.0), (0.5, 12.0), (0.8, 24.0), (1.0, DRIVE_MAX_DB)]);

// ---- Modulation (slice 7) -----------------------------------------------------------------

pub const LFO_RATE: ParamSpec = spec("rate", "Rate", "Hz", 0.02, 40.0, 1.0, ParamScale::Log, 2, (0.25, 1.0 / 16.0), ParamDisplay::Number);
pub const LFO_SYNC: BoolSpec = BoolSpec { id: "sync", name: "Sync", default: false };
pub const LFO_DIV: ChoiceSpec = ChoiceSpec {
    id: "div",
    name: "Division",
    choices: &["1/32", "1/16T", "1/16", "1/16D", "1/8T", "1/8", "1/8D", "1/4T", "1/4", "1/4D", "1/2T", "1/2", "1/2D", "1 bar", "2 bars", "4 bars"],
    default: 8,
};
pub const LFO_SHAPE: ChoiceSpec = ChoiceSpec { id: "shape", name: "Shape", choices: &["Sine", "Triangle", "Saw up", "Saw down", "Square", "Smooth random", "Sample & hold"], default: 0 };
pub const LFO_PHASE: ParamSpec = spec("phase", "Phase", "°", 0.0, 360.0, 0.0, ParamScale::Linear, 0, (15.0, 1.0), ParamDisplay::Number);
pub const ENV_ATT: ParamSpec = spec("env_att", "Envelope Attack", "ms", 0.1, 500.0, 10.0, ParamScale::Log, 1, (0.25, 1.0 / 16.0), ParamDisplay::Time);
pub const ENV_REL: ParamSpec = spec("env_rel", "Envelope Release", "ms", 5.0, 2000.0, 200.0, ParamScale::Log, 0, (0.25, 1.0 / 16.0), ParamDisplay::Time);
pub const ENV_GAIN: ParamSpec = spec("env_gain", "Envelope Gain", "dB", -24.0, 24.0, 0.0, ParamScale::Linear, 1, (0.5, 0.1), ParamDisplay::Number);
pub const ENV_SRC: ChoiceSpec = ChoiceSpec { id: "env_src", name: "Envelope Source", choices: &["Input", "Sidechain"], default: 0 };
pub const MOD_SLOTS: usize = 8;
pub const M_SRC: ChoiceSpec = ChoiceSpec { id: "src", name: "Source", choices: &["Off", "LFO 1", "LFO 2", "Envelope"], default: 0 };
/// What a slot moves: an index into [`MOD_TARGET_IDS`] (frozen, append only; not automatable, so
/// growing the list never moves host automation).
pub const M_DST: ChoiceSpec = ChoiceSpec { id: "dst", name: "Target", choices: &MOD_TARGETS, default: 0 };
pub const M_AMT: ParamSpec = spec("amt", "Amount", "%", -100.0, 100.0, 0.0, ParamScale::Linear, 0, (5.0, 1.0), ParamDisplay::Number);

pub static MOD_TARGETS: [&str; 44] = [
    "None",
    "Band 1 Drive",
    "Band 1 Bias",
    "Band 1 Tone",
    "Band 1 Mix",
    "Band 1 Output",
    "Band 1 Dynamics",
    "Band 2 Drive",
    "Band 2 Bias",
    "Band 2 Tone",
    "Band 2 Mix",
    "Band 2 Output",
    "Band 2 Dynamics",
    "Band 3 Drive",
    "Band 3 Bias",
    "Band 3 Tone",
    "Band 3 Mix",
    "Band 3 Output",
    "Band 3 Dynamics",
    "Band 4 Drive",
    "Band 4 Bias",
    "Band 4 Tone",
    "Band 4 Mix",
    "Band 4 Output",
    "Band 4 Dynamics",
    "Band 5 Drive",
    "Band 5 Bias",
    "Band 5 Tone",
    "Band 5 Mix",
    "Band 5 Output",
    "Band 5 Dynamics",
    "Band 6 Drive",
    "Band 6 Bias",
    "Band 6 Tone",
    "Band 6 Mix",
    "Band 6 Output",
    "Band 6 Dynamics",
    "Crossover 1",
    "Crossover 2",
    "Crossover 3",
    "Crossover 4",
    "Crossover 5",
    "Mix",
    "Output",
];
/// The parameter each modulation target moves (`none` for the first).
pub static MOD_TARGET_IDS: [&str; 44] = [
    "none",
    "b1_drive",
    "b1_bias",
    "b1_tone",
    "b1_mix",
    "b1_out",
    "b1_dyn",
    "b2_drive",
    "b2_bias",
    "b2_tone",
    "b2_mix",
    "b2_out",
    "b2_dyn",
    "b3_drive",
    "b3_bias",
    "b3_tone",
    "b3_mix",
    "b3_out",
    "b3_dyn",
    "b4_drive",
    "b4_bias",
    "b4_tone",
    "b4_mix",
    "b4_out",
    "b4_dyn",
    "b5_drive",
    "b5_bias",
    "b5_tone",
    "b5_mix",
    "b5_out",
    "b5_dyn",
    "b6_drive",
    "b6_bias",
    "b6_tone",
    "b6_mix",
    "b6_out",
    "b6_dyn",
    "x1_freq",
    "x2_freq",
    "x3_freq",
    "x4_freq",
    "x5_freq",
    "mix",
    "out_gain",
];

/// A crossover parameter's host id, e.g. `x2_freq` (`slot` from 0).
pub fn cross_id(slot: usize, param: &str) -> String {
    format!("x{}_{param}", slot + 1)
}

/// A band parameter's host id, e.g. `b3_drive` (`band` from 0).
pub fn band_id(band: usize, param: &str) -> String {
    format!("b{}_{param}", band + 1)
}

/// An LFO parameter's host id, e.g. `lfo2_rate` (`lfo` from 0).
pub fn lfo_id(lfo: usize, param: &str) -> String {
    format!("lfo{}_{param}", lfo + 1)
}

/// A modulation slot parameter's host id, e.g. `m3_amt` (`slot` from 0).
pub fn mod_id(slot: usize, param: &str) -> String {
    format!("m{}_{param}", slot + 1)
}

/// The continuous band parameters, in host order.
pub const BAND_SPECS: [ParamSpec; 9] = [B_DRIVE, B_BIAS, B_TONE, B_MIX, B_OUT, B_DYN, B_DYN_THR, B_DYN_ATT, B_DYN_REL];

/// Every parameter's manifest line, in host order.
pub fn manifest() -> String {
    let mut lines = vec![OVERSAMPLING.manifest_line(), AUTO_GAIN.manifest_line()];
    lines.extend([MIX, OUT_PAN, OUT_GAIN].iter().map(ParamSpec::manifest_line));
    lines.push(DELTA.manifest_line());
    lines.push(BYPASS.manifest_line());
    for (x, freq) in X_FREQS.iter().enumerate() {
        let prefix = |line: String| format!("x{}_{line}", x + 1);
        lines.push(prefix(X_ON.manifest_line()));
        lines.push(prefix(freq.manifest_line()));
    }
    for b in 0..BANDS {
        let prefix = |line: String| format!("b{}_{line}", b + 1);
        lines.push(prefix(B_MODEL.manifest_line()));
        for spec in BAND_SPECS {
            lines.push(prefix(spec.manifest_line()));
        }
        lines.push(prefix(B_STEREO.manifest_line()));
        for spec in [B_BYPASS, B_MUTE, B_SOLO] {
            lines.push(prefix(spec.manifest_line()));
        }
    }
    for l in 0..2 {
        let prefix = |line: String| format!("lfo{}_{line}", l + 1);
        lines.push(prefix(LFO_RATE.manifest_line()));
        lines.push(prefix(LFO_SYNC.manifest_line()));
        lines.push(prefix(LFO_DIV.manifest_line()));
        lines.push(prefix(LFO_SHAPE.manifest_line()));
        lines.push(prefix(LFO_PHASE.manifest_line()));
    }
    lines.extend([ENV_ATT, ENV_REL, ENV_GAIN].iter().map(ParamSpec::manifest_line));
    lines.push(ENV_SRC.manifest_line());
    for m in 0..MOD_SLOTS {
        let prefix = |line: String| format!("m{}_{line}", m + 1);
        lines.push(prefix(M_SRC.manifest_line()));
        lines.push(prefix(M_DST.manifest_line()));
        lines.push(prefix(M_AMT.manifest_line()));
    }
    lines.join("\n") + "\n"
}

/// Every parameter id, in host order.
pub fn ids() -> Vec<String> {
    manifest().lines().filter_map(|l| l.split(" | ").next()).map(str::to_owned).collect()
}

/// Parameters that are monitoring, not sound: never in presets or snapshots.
pub fn is_monitoring(id: &str) -> bool {
    id == DELTA.id || id == BYPASS.id || id.ends_with(&format!("_{}", B_SOLO.id))
}

/// The sound parameters: what presets, snapshots and undo cover.
pub fn sound_ids() -> Vec<String> {
    ids().into_iter().filter(|id| !is_monitoring(id)).collect()
}

/// The oversampling factor a choice index means.
pub fn factor(index: i32) -> usize {
    FACTORS[(index.max(0) as usize).min(FACTORS.len() - 1)]
}

pub struct CrossParams {
    pub on: BoolParam,
    pub freq: FloatParam,
}

impl CrossParams {
    fn new(slot: usize) -> Self {
        let name = |what: &str| format!("Crossover {} {what}", slot + 1);
        Self { on: bool_param(&X_ON, name("On")), freq: float_param_named(&X_FREQS[slot], name("Frequency")) }
    }
}

pub struct BandParams {
    pub model: IntParam,
    /// Drive, bias, tone, mix, output, dynamics and its threshold, attack and release
    /// ([`BAND_SPECS`] order).
    pub floats: [FloatParam; 9],
    pub stereo: IntParam,
    pub bypass: BoolParam,
    pub mute: BoolParam,
    pub solo: BoolParam,
}

impl BandParams {
    fn new(band: usize) -> Self {
        let name = |what: &str| format!("Band {} {what}", band + 1);
        static SPECS: [ParamSpec; 9] = BAND_SPECS;
        let names = ["Drive", "Bias", "Tone", "Mix", "Output", "Dynamics", "Dynamics Threshold", "Dynamics Attack", "Dynamics Release"];
        Self {
            model: choice_param(&B_MODEL, name("Model")),
            floats: std::array::from_fn(|k| float_param_named(&SPECS[k], name(names[k]))),
            stereo: choice_param(&B_STEREO, name("Stereo")),
            bypass: bool_param(&B_BYPASS, name("Bypass")),
            mute: bool_param(&B_MUTE, name("Mute")),
            solo: bool_param(&B_SOLO, name("Solo")),
        }
    }

    pub fn sat(&self) -> BandSat {
        let f = |k: usize| self.floats[k].value();
        BandSat {
            model: Model::from_index(self.model.value().max(0) as usize),
            drive_db: f(0),
            bias: f(1) / 100.0,
            tone_db: f(2),
            mix: f(3) / 100.0,
            out_db: f(4),
            stereo: mite_dsp::sat::StereoMode::from_index(self.stereo.value().max(0) as usize),
            bypass: self.bypass.value(),
            mute: self.mute.value(),
            solo: self.solo.value(),
        }
    }
}

pub struct LfoParams {
    pub rate: FloatParam,
    pub sync: BoolParam,
    pub div: IntParam,
    pub shape: IntParam,
    pub phase: FloatParam,
}

impl LfoParams {
    fn new(lfo: usize) -> Self {
        let name = |what: &str| format!("LFO {} {what}", lfo + 1);
        Self {
            rate: float_param_named(&LFO_RATE, name("Rate")),
            sync: bool_param(&LFO_SYNC, name("Sync")),
            div: choice_param(&LFO_DIV, name("Division")),
            shape: choice_param(&LFO_SHAPE, name("Shape")),
            phase: float_param_named(&LFO_PHASE, name("Phase")),
        }
    }
}

pub struct ModParams {
    pub src: IntParam,
    pub dst: IntParam,
    pub amt: FloatParam,
}

impl ModParams {
    fn new(slot: usize) -> Self {
        let name = |what: &str| format!("Mod {} {what}", slot + 1);
        Self {
            src: choice_param(&M_SRC, name("Source")).non_automatable(),
            dst: choice_param(&M_DST, name("Target")).non_automatable(),
            amt: float_param_named(&M_AMT, name("Amount")),
        }
    }
}

pub struct SatParams {
    pub oversampling: IntParam,
    pub auto_gain: BoolParam,
    pub mix: FloatParam,
    pub out_pan: FloatParam,
    pub out_gain: FloatParam,
    pub delta: BoolParam,
    pub bypass: BoolParam,
    pub crossovers: [CrossParams; CROSSOVERS],
    pub bands: [BandParams; BANDS],
    pub lfos: [LfoParams; 2],
    pub env_att: FloatParam,
    pub env_rel: FloatParam,
    pub env_gain: FloatParam,
    pub env_src: IntParam,
    pub mods: [ModParams; MOD_SLOTS],
    /// Snapshots A–D and the current preset's name (not parameters: saved with the state through
    /// `serialize_fields`).
    pub snapshots: std::sync::Arc<Snapshots>,
    pub preset: std::sync::Arc<CurrentPreset>,
}

impl Default for SatParams {
    fn default() -> Self {
        let flag = |spec: &'static BoolSpec| bool_param(spec, spec.name.to_owned());
        Self {
            oversampling: choice_param(&OVERSAMPLING, OVERSAMPLING.name.to_owned()),
            auto_gain: flag(&AUTO_GAIN),
            mix: float_param(&MIX),
            out_pan: float_param(&OUT_PAN),
            out_gain: float_param(&OUT_GAIN),
            delta: flag(&DELTA),
            bypass: flag(&BYPASS).make_bypass(),
            crossovers: std::array::from_fn(CrossParams::new),
            bands: std::array::from_fn(BandParams::new),
            lfos: std::array::from_fn(LfoParams::new),
            env_att: float_param(&ENV_ATT),
            env_rel: float_param(&ENV_REL),
            env_gain: float_param(&ENV_GAIN),
            env_src: choice_param(&ENV_SRC, ENV_SRC.name.to_owned()),
            mods: std::array::from_fn(ModParams::new),
            snapshots: std::sync::Arc::new(Snapshots::new()),
            preset: std::sync::Arc::new(CurrentPreset::default()),
        }
    }
}

impl SatParams {
    pub fn settings(&self) -> SatSettings {
        SatSettings {
            crossovers: std::array::from_fn(|k| self.crossovers[k].on.value().then(|| self.crossovers[k].freq.value())),
            bands: std::array::from_fn(|b| self.bands[b].sat()),
            auto_gain: self.auto_gain.value(),
        }
    }

    pub fn factor(&self) -> usize {
        factor(self.oversampling.value())
    }
}

// SAFETY: every `ParamPtr` points into `self`, which nih-plug keeps alive (in an `Arc`) for as
// long as it uses the map. The ids match `manifest()` (checked by tests/params.rs).
unsafe impl Params for SatParams {
    fn serialize_fields(&self) -> std::collections::BTreeMap<String, String> {
        let live = mite_plugin::state::live_values(self, &sound_ids());
        mite_plugin::state::session_fields(STATE_VERSION, &self.snapshots, &self.preset, live)
    }

    fn deserialize_fields(&self, serialized: &std::collections::BTreeMap<String, String>) {
        mite_plugin::state::load_session_fields(serialized, &self.snapshots, &self.preset);
    }

    fn param_map(&self) -> Vec<(String, ParamPtr, String)> {
        let global = |id: &str, ptr: ParamPtr| (id.to_owned(), ptr, String::new());
        let mut map = vec![
            global(OVERSAMPLING.id, self.oversampling.as_ptr()),
            global(AUTO_GAIN.id, self.auto_gain.as_ptr()),
            global(MIX.id, self.mix.as_ptr()),
            global(OUT_PAN.id, self.out_pan.as_ptr()),
            global(OUT_GAIN.id, self.out_gain.as_ptr()),
            global(DELTA.id, self.delta.as_ptr()),
            global(BYPASS.id, self.bypass.as_ptr()),
        ];
        for (i, x) in self.crossovers.iter().enumerate() {
            let group = "Crossovers".to_owned();
            map.push((cross_id(i, X_ON.id), x.on.as_ptr(), group.clone()));
            map.push((cross_id(i, X_FREQ.id), x.freq.as_ptr(), group));
        }
        for (i, b) in self.bands.iter().enumerate() {
            let group = format!("Band {}", i + 1);
            map.push((band_id(i, B_MODEL.id), b.model.as_ptr(), group.clone()));
            for (spec, p) in BAND_SPECS.iter().zip(&b.floats) {
                map.push((band_id(i, spec.id), p.as_ptr(), group.clone()));
            }
            map.push((band_id(i, B_STEREO.id), b.stereo.as_ptr(), group.clone()));
            map.push((band_id(i, B_BYPASS.id), b.bypass.as_ptr(), group.clone()));
            map.push((band_id(i, B_MUTE.id), b.mute.as_ptr(), group.clone()));
            map.push((band_id(i, B_SOLO.id), b.solo.as_ptr(), group));
        }
        for (i, l) in self.lfos.iter().enumerate() {
            let group = format!("LFO {}", i + 1);
            map.push((lfo_id(i, LFO_RATE.id), l.rate.as_ptr(), group.clone()));
            map.push((lfo_id(i, LFO_SYNC.id), l.sync.as_ptr(), group.clone()));
            map.push((lfo_id(i, LFO_DIV.id), l.div.as_ptr(), group.clone()));
            map.push((lfo_id(i, LFO_SHAPE.id), l.shape.as_ptr(), group.clone()));
            map.push((lfo_id(i, LFO_PHASE.id), l.phase.as_ptr(), group));
        }
        let group = "Envelope".to_owned();
        map.push((ENV_ATT.id.to_owned(), self.env_att.as_ptr(), group.clone()));
        map.push((ENV_REL.id.to_owned(), self.env_rel.as_ptr(), group.clone()));
        map.push((ENV_GAIN.id.to_owned(), self.env_gain.as_ptr(), group.clone()));
        map.push((ENV_SRC.id.to_owned(), self.env_src.as_ptr(), group));
        for (i, m) in self.mods.iter().enumerate() {
            let group = "Modulation".to_owned();
            map.push((mod_id(i, M_SRC.id), m.src.as_ptr(), group.clone()));
            map.push((mod_id(i, M_DST.id), m.dst.as_ptr(), group.clone()));
            map.push((mod_id(i, M_AMT.id), m.amt.as_ptr(), group));
        }
        map
    }
}
