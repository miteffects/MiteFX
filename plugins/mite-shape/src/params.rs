//! Mite Shape parameters (BLUEPRINT §6.10; docs/design/mockups/shape-main.dc.html).
//!
//! IDs are frozen once released (`tests/golden/mite_shape.params.txt`): globals by name, the four
//! crossovers as `x{N}_{param}` and the five bands as `b{N}_{param}`. Slots are fixed: crossover
//! slot N, when on, owns band N + 1 (the region above it); band 1 is always the lowest region.
//! The editor numbers bands by frequency, but a slot's parameters never move to another slot, so
//! automation stays on its band.

use mite_core::presets::CurrentPreset;
use mite_core::snapshots::Snapshots;
use mite_core::{BoolSpec, ChoiceSpec, DialCurve, ParamDisplay, ParamScale, ParamSpec};
use mite_dsp::shape::{BandShape, ShapeSettings, AMOUNT_DB, BANDS, CROSSOVERS, DEFAULT_CROSSOVERS, FREQ_MAX, FREQ_MIN, MAX_LOOKAHEAD_MS};
use mite_plugin::{bool_param, choice_param, float_param, float_param_named};
use nih_plug::prelude::*;

/// The state format version (`mite_plugin::state`). Bump it, with a migration step in
/// `MiteShape::filter_state` and a new golden state file, when a release changes what a state
/// means.
pub const STATE_VERSION: u32 = 1;

#[allow(clippy::too_many_arguments)] // one line per parameter reads best
const fn spec(id: &'static str, name: &'static str, unit: &'static str, min: f32, max: f32, default: f32, scale: ParamScale, decimals: u8, wheel: (f32, f32), display: ParamDisplay) -> ParamSpec {
    ParamSpec { id, name, unit, min, max, default, scale, smoothing_ms: 20.0, decimals, wheel_step: wheel.0, wheel_step_fine: wheel.1, display }
}

// ---- Detector & stereo --------------------------------------------------------------------

pub const STEREO: ChoiceSpec = ChoiceSpec { id: "stereo", name: "Stereo", choices: &["L/R", "M/S"], default: 0 };
pub const LINK: ParamSpec = spec("link", "Stereo Link", "%", 0.0, 100.0, 100.0, ParamScale::Linear, 0, (1.0, 0.1), ParamDisplay::Number);
pub const LOOKAHEAD: ParamSpec = spec("lookahead", "Lookahead", "ms", 0.0, MAX_LOOKAHEAD_MS, 0.0, ParamScale::Linear, 1, (0.1, 0.01), ParamDisplay::Time);
pub const FLOOR: ParamSpec = spec("floor", "Floor", "dBFS", -80.0, -20.0, -60.0, ParamScale::Linear, 0, (1.0, 0.1), ParamDisplay::Number);
/// What the detector hears with one band (open at the ends).
pub const DET_HP: ParamSpec = spec("det_hp", "Detector High-pass", "Hz", FREQ_MIN, 2000.0, FREQ_MIN, ParamScale::Log, 0, (1.0 / 6.0, 1.0 / 48.0), ParamDisplay::Frequency);
pub const DET_LP: ParamSpec = spec("det_lp", "Detector Low-pass", "Hz", 200.0, FREQ_MAX, FREQ_MAX, ParamScale::Log, 0, (1.0 / 6.0, 1.0 / 48.0), ParamDisplay::Frequency);
/// Hear what the detector hears instead of the result (monitoring: never in presets).
pub const LISTEN: BoolSpec = BoolSpec { id: "listen", name: "Listen", default: false };

// ---- Output (as the EQ and Comp) ----------------------------------------------------------

pub const AUTO_GAIN: BoolSpec = BoolSpec { id: "auto_gain", name: "Auto Gain", default: false };
pub const MIX: ParamSpec = spec("mix", "Mix", "%", 0.0, 100.0, 100.0, ParamScale::Linear, 0, (5.0, 1.0), ParamDisplay::Number);
pub const OUT_PAN: ParamSpec = spec("out_pan", "Output Pan", "%", -100.0, 100.0, 0.0, ParamScale::Linear, 0, (5.0, 1.0), ParamDisplay::Number);
pub const OUT_GAIN: ParamSpec = spec("out_gain", "Output", "dB", -24.0, 24.0, 0.0, ParamScale::Linear, 1, (0.5, 0.1), ParamDisplay::Number);
pub const DELTA: BoolSpec = BoolSpec { id: "delta", name: "Delta", default: false };
pub const BYPASS: BoolSpec = BoolSpec { id: "bypass", name: "Bypass", default: false };

// ---- Crossovers ---------------------------------------------------------------------------

pub const X_ON: BoolSpec = BoolSpec { id: "on", name: "On", default: false };
pub const X_FREQ: ParamSpec = spec("freq", "Frequency", "Hz", FREQ_MIN, FREQ_MAX, 1000.0, ParamScale::Log, 0, (1.0 / 6.0, 1.0 / 48.0), ParamDisplay::Frequency);
static X_FREQS: [ParamSpec; CROSSOVERS] = [
    ParamSpec { default: DEFAULT_CROSSOVERS[0], ..X_FREQ },
    ParamSpec { default: DEFAULT_CROSSOVERS[1], ..X_FREQ },
    ParamSpec { default: DEFAULT_CROSSOVERS[2], ..X_FREQ },
    ParamSpec { default: DEFAULT_CROSSOVERS[3], ..X_FREQ },
];

// ---- Bands --------------------------------------------------------------------------------

pub const B_ATTACK: ParamSpec = spec("attack", "Attack", "dB", -AMOUNT_DB, AMOUNT_DB, 0.0, ParamScale::Linear, 1, (0.5, 0.1), ParamDisplay::Number);
pub const B_SUSTAIN: ParamSpec = spec("sustain", "Sustain", "dB", -AMOUNT_DB, AMOUNT_DB, 0.0, ParamScale::Linear, 1, (0.5, 0.1), ParamDisplay::Number);
pub const B_TIME: ParamSpec = spec("time", "Transient Time", "ms", 5.0, 200.0, 25.0, ParamScale::Log, 0, (0.25, 1.0 / 16.0), ParamDisplay::Time);
pub const B_TRANSITION: ParamSpec = spec("trans", "Transition", "%", 0.0, 200.0, 100.0, ParamScale::Linear, 0, (5.0, 1.0), ParamDisplay::Number);
pub const B_MIX: ParamSpec = spec("mix", "Band Mix", "%", 0.0, 100.0, 100.0, ParamScale::Linear, 0, (5.0, 1.0), ParamDisplay::Number);
pub const B_OUT: ParamSpec = spec("out", "Band Output", "dB", -24.0, 24.0, 0.0, ParamScale::Linear, 1, (0.5, 0.1), ParamDisplay::Number);
/// A muted band drops out of the sum (sound: kept in presets, as throwing a band away is).
pub const B_MUTE: BoolSpec = BoolSpec { id: "mute", name: "Mute", default: false };
/// Hear only this band (monitoring: never in presets).
pub const B_SOLO: BoolSpec = BoolSpec { id: "solo", name: "Solo", default: false };

/// Attack and sustain dials: ±6 dB takes 60 % of the sweep (the design's S curve).
pub const AMOUNT_DIAL: DialCurve = DialCurve(&[(0.0, -AMOUNT_DB), (0.2, -6.0), (0.5, 0.0), (0.8, 6.0), (1.0, AMOUNT_DB)]);
/// Transient time: 10–50 ms takes half the travel.
pub const TIME_DIAL: DialCurve = DialCurve(&[(0.0, 5.0), (0.2, 10.0), (0.7, 50.0), (1.0, 200.0)]);

/// A crossover parameter's host id, e.g. `x2_freq` (`slot` from 0).
pub fn cross_id(slot: usize, param: &str) -> String {
    format!("x{}_{param}", slot + 1)
}

/// A band parameter's host id, e.g. `b3_attack` (`band` from 0).
pub fn band_id(band: usize, param: &str) -> String {
    format!("b{}_{param}", band + 1)
}

/// Every parameter's manifest line, in host order.
pub fn manifest() -> String {
    let mut lines = vec![STEREO.manifest_line()];
    lines.extend([LINK, LOOKAHEAD, FLOOR, DET_HP, DET_LP].iter().map(ParamSpec::manifest_line));
    lines.push(LISTEN.manifest_line());
    lines.push(AUTO_GAIN.manifest_line());
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
        for spec in [B_ATTACK, B_SUSTAIN, B_TIME, B_TRANSITION, B_MIX, B_OUT] {
            lines.push(prefix(spec.manifest_line()));
        }
        lines.push(prefix(B_MUTE.manifest_line()));
        lines.push(prefix(B_SOLO.manifest_line()));
    }
    lines.join("\n") + "\n"
}

/// Every parameter id, in host order.
pub fn ids() -> Vec<String> {
    manifest().lines().filter_map(|l| l.split(" | ").next()).map(str::to_owned).collect()
}

/// Parameters that are monitoring, not sound: never in presets or snapshots.
pub fn is_monitoring(id: &str) -> bool {
    id == LISTEN.id || id == DELTA.id || id == BYPASS.id || id.ends_with(&format!("_{}", B_SOLO.id))
}

/// The sound parameters: what presets, snapshots and undo cover.
pub fn sound_ids() -> Vec<String> {
    ids().into_iter().filter(|id| !is_monitoring(id)).collect()
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
    pub attack: FloatParam,
    pub sustain: FloatParam,
    pub time: FloatParam,
    pub transition: FloatParam,
    pub mix: FloatParam,
    pub out: FloatParam,
    pub mute: BoolParam,
    pub solo: BoolParam,
}

impl BandParams {
    fn new(band: usize) -> Self {
        let name = |what: &str| format!("Band {} {what}", band + 1);
        Self {
            attack: float_param_named(&B_ATTACK, name("Attack")),
            sustain: float_param_named(&B_SUSTAIN, name("Sustain")),
            time: float_param_named(&B_TIME, name("Transient Time")),
            transition: float_param_named(&B_TRANSITION, name("Transition")),
            mix: float_param_named(&B_MIX, name("Mix")),
            out: float_param_named(&B_OUT, name("Output")),
            mute: bool_param(&B_MUTE, name("Mute")),
            solo: bool_param(&B_SOLO, name("Solo")),
        }
    }

    pub fn shape(&self) -> BandShape {
        BandShape {
            attack_db: self.attack.value(),
            sustain_db: self.sustain.value(),
            transient_ms: self.time.value(),
            transition: self.transition.value() / 100.0,
            mix: self.mix.value() / 100.0,
            out_db: self.out.value(),
            mute: self.mute.value(),
            solo: self.solo.value(),
        }
    }
}

pub struct ShapeParams {
    pub stereo: IntParam,
    pub link: FloatParam,
    pub lookahead: FloatParam,
    pub floor: FloatParam,
    pub det_hp: FloatParam,
    pub det_lp: FloatParam,
    pub listen: BoolParam,
    pub auto_gain: BoolParam,
    pub mix: FloatParam,
    pub out_pan: FloatParam,
    pub out_gain: FloatParam,
    pub delta: BoolParam,
    pub bypass: BoolParam,
    pub crossovers: [CrossParams; CROSSOVERS],
    pub bands: [BandParams; BANDS],
    /// Snapshots A–D and the current preset's name (not parameters: saved with the state through
    /// `serialize_fields`).
    pub snapshots: std::sync::Arc<Snapshots>,
    pub preset: std::sync::Arc<CurrentPreset>,
}

impl Default for ShapeParams {
    fn default() -> Self {
        let flag = |spec: &'static BoolSpec| bool_param(spec, spec.name.to_owned());
        Self {
            stereo: choice_param(&STEREO, STEREO.name.to_owned()),
            link: float_param(&LINK),
            lookahead: float_param(&LOOKAHEAD),
            floor: float_param(&FLOOR),
            det_hp: float_param(&DET_HP),
            det_lp: float_param(&DET_LP),
            listen: flag(&LISTEN),
            auto_gain: flag(&AUTO_GAIN),
            mix: float_param(&MIX),
            out_pan: float_param(&OUT_PAN),
            out_gain: float_param(&OUT_GAIN),
            delta: flag(&DELTA),
            bypass: flag(&BYPASS).make_bypass(),
            crossovers: std::array::from_fn(CrossParams::new),
            bands: std::array::from_fn(BandParams::new),
            snapshots: std::sync::Arc::new(Snapshots::new()),
            preset: std::sync::Arc::new(CurrentPreset::default()),
        }
    }
}

impl ShapeParams {
    pub fn settings(&self) -> ShapeSettings {
        ShapeSettings {
            crossovers: std::array::from_fn(|k| self.crossovers[k].on.value().then(|| self.crossovers[k].freq.value())),
            bands: std::array::from_fn(|b| self.bands[b].shape()),
            detector_hp: self.det_hp.value(),
            detector_lp: self.det_lp.value(),
            listen: self.listen.value(),
            lookahead_ms: self.lookahead.value(),
            mid_side: self.stereo.value() == 1,
            link: self.link.value() / 100.0,
            floor_db: self.floor.value(),
        }
    }
}

// SAFETY: every `ParamPtr` points into `self`, which nih-plug keeps alive (in an `Arc`) for as
// long as it uses the map. The ids match `manifest()` (checked by tests/params.rs).
unsafe impl Params for ShapeParams {
    fn serialize_fields(&self) -> std::collections::BTreeMap<String, String> {
        let live = mite_plugin::state::live_values(self, &sound_ids());
        mite_plugin::state::session_fields(STATE_VERSION, &self.snapshots, &self.preset, live)
    }

    fn deserialize_fields(&self, serialized: &std::collections::BTreeMap<String, String>) {
        mite_plugin::state::load_session_fields(serialized, &self.snapshots, &self.preset);
    }

    fn param_map(&self) -> Vec<(String, ParamPtr, String)> {
        let global = |id: &str, ptr: ParamPtr, group: &str| (id.to_owned(), ptr, group.to_owned());
        let mut map = vec![
            global(STEREO.id, self.stereo.as_ptr(), "Detector"),
            global(LINK.id, self.link.as_ptr(), "Detector"),
            global(LOOKAHEAD.id, self.lookahead.as_ptr(), "Detector"),
            global(FLOOR.id, self.floor.as_ptr(), "Detector"),
            global(DET_HP.id, self.det_hp.as_ptr(), "Detector"),
            global(DET_LP.id, self.det_lp.as_ptr(), "Detector"),
            global(LISTEN.id, self.listen.as_ptr(), "Detector"),
            global(AUTO_GAIN.id, self.auto_gain.as_ptr(), ""),
            global(MIX.id, self.mix.as_ptr(), ""),
            global(OUT_PAN.id, self.out_pan.as_ptr(), ""),
            global(OUT_GAIN.id, self.out_gain.as_ptr(), ""),
            global(DELTA.id, self.delta.as_ptr(), ""),
            global(BYPASS.id, self.bypass.as_ptr(), ""),
        ];
        for (i, x) in self.crossovers.iter().enumerate() {
            let group = "Crossovers".to_owned();
            map.push((cross_id(i, X_ON.id), x.on.as_ptr(), group.clone()));
            map.push((cross_id(i, X_FREQ.id), x.freq.as_ptr(), group));
        }
        for (i, b) in self.bands.iter().enumerate() {
            let group = format!("Band {}", i + 1);
            map.push((band_id(i, B_ATTACK.id), b.attack.as_ptr(), group.clone()));
            map.push((band_id(i, B_SUSTAIN.id), b.sustain.as_ptr(), group.clone()));
            map.push((band_id(i, B_TIME.id), b.time.as_ptr(), group.clone()));
            map.push((band_id(i, B_TRANSITION.id), b.transition.as_ptr(), group.clone()));
            map.push((band_id(i, B_MIX.id), b.mix.as_ptr(), group.clone()));
            map.push((band_id(i, B_OUT.id), b.out.as_ptr(), group.clone()));
            map.push((band_id(i, B_MUTE.id), b.mute.as_ptr(), group.clone()));
            map.push((band_id(i, B_SOLO.id), b.solo.as_ptr(), group));
        }
        map
    }
}
