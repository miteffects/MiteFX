//! Mite Comp parameters (BLUEPRINT §6.2; docs/design/interaction.md, "Mite Comp").
//!
//! IDs are frozen once released (`tests/golden/mite_comp.params.txt`): globals by name, the four
//! stages as `s{N}_{param}` and the detector EQ bands as `k{N}_{param}`. Stage slots are fixed:
//! the editor numbers stages by threshold, but a slot's parameters never move to another slot,
//! so automation stays on its stage.

use mite_core::presets::CurrentPreset;
use mite_core::snapshots::Snapshots;
use mite_core::{BoolSpec, ChoiceSpec, DialCurve, ParamDisplay, ParamScale, ParamSpec};
use mite_dsp::dynamics::{CompSettings, Detector, Quadrant, Smart, Stage, Style, CHARACTERS, CUSTOM, KEY_BANDS, RATIO_MAX, STAGES, STYLES};
use mite_dsp::eq::{BandSettings, BandType};
use mite_plugin::{bool_param, choice_param, float_param, float_param_named};
use nih_plug::prelude::*;

/// The state format version (`mite_plugin::state`). Bump it, with a migration step in
/// `MiteComp::filter_state` and a new golden state file, when a release changes what a state
/// means.
///
/// - 2 (2026-10-23): thresholds are on the input, so a higher stage never changes a lower one
///   (`crate::migrate::v1_to_v2`).
pub const STATE_VERSION: u32 = 2;

#[allow(clippy::too_many_arguments)] // one line per parameter reads best
const fn spec(id: &'static str, name: &'static str, unit: &'static str, min: f32, max: f32, default: f32, scale: ParamScale, decimals: u8, wheel: (f32, f32), display: ParamDisplay) -> ParamSpec {
    ParamSpec { id, name, unit, min, max, default, scale, smoothing_ms: 20.0, decimals, wheel_step: wheel.0, wheel_step_fine: wheel.1, display }
}

// ---- Character ----------------------------------------------------------------------------

pub const CHARACTER: ChoiceSpec = ChoiceSpec { id: "character", name: "Character", choices: &CHARACTERS, default: 0 };
/// Custom character: the six values every Character is made of (defaults are Clean's).
pub const C_ATTACK: ParamSpec = spec("c_attack", "Custom Attack", "×", 0.1, 4.0, 1.0, ParamScale::Log, 2, (0.25, 1.0 / 16.0), ParamDisplay::Number);
pub const C_RELEASE: ParamSpec = spec("c_release", "Custom Release", "×", 0.1, 4.0, 1.0, ParamScale::Log, 2, (0.25, 1.0 / 16.0), ParamDisplay::Number);
pub const C_DRIVE: ParamSpec = spec("c_drive", "Custom Drive", "dB", -6.0, 12.0, 0.0, ParamScale::Linear, 1, (0.5, 0.1), ParamDisplay::Number);
pub const C_TAIL: ParamSpec = spec("c_tail", "Custom Tail", "%", 0.0, 100.0, 0.0, ParamScale::Linear, 0, (1.0, 0.1), ParamDisplay::Number);
pub const C_PROGRAM: ParamSpec = spec("c_program", "Custom Program", "%", 0.0, 100.0, 0.0, ParamScale::Linear, 0, (1.0, 0.1), ParamDisplay::Number);
pub const C_BEND: ParamSpec = spec("c_bend", "Custom Bend", "%", 0.0, 100.0, 0.0, ParamScale::Linear, 0, (1.0, 0.1), ParamDisplay::Number);

// ---- Smart --------------------------------------------------------------------------------

pub const SMART: ChoiceSpec = ChoiceSpec { id: "smart", name: "Smart", choices: &Smart::NAMES, default: 0 };
pub const SMART_AMOUNT: ParamSpec = spec("smart_amt", "Smart Amount", "%", 0.0, 100.0, 70.0, ParamScale::Linear, 0, (1.0, 0.1), ParamDisplay::Number);
pub const TARGET_GR: ParamSpec = spec("target_gr", "Target GR", "dB", 0.0, 24.0, 6.0, ParamScale::Linear, 1, (0.5, 0.1), ParamDisplay::Number);
pub const SIBILANCE: ParamSpec = spec("sibilance", "Sibilance Guard", "%", 0.0, 100.0, 50.0, ParamScale::Linear, 0, (1.0, 0.1), ParamDisplay::Number);

// ---- Detector & output --------------------------------------------------------------------

pub const TOPOLOGY: ChoiceSpec = ChoiceSpec { id: "topology", name: "Topology", choices: &["Feed-forward", "Feedback"], default: 0 };
pub const STEREO: ChoiceSpec = ChoiceSpec { id: "stereo", name: "Stereo", choices: &["L/R", "M/S"], default: 0 };
pub const LINK: ParamSpec = spec("link", "Stereo Link", "%", 0.0, 100.0, 100.0, ParamScale::Linear, 0, (1.0, 0.1), ParamDisplay::Number);
pub const LOOKAHEAD: ParamSpec = spec("lookahead", "Lookahead", "ms", 0.0, 20.0, 0.0, ParamScale::Linear, 1, (0.5, 0.1), ParamDisplay::Time);
pub const HOLD: ParamSpec = spec("hold", "Hold", "ms", 0.0, 500.0, 0.0, ParamScale::Linear, 0, (5.0, 1.0), ParamDisplay::Time);
pub const MAKEUP: ParamSpec = spec("makeup", "Makeup", "dB", -12.0, 24.0, 0.0, ParamScale::Linear, 1, (0.5, 0.1), ParamDisplay::Number);
pub const AUTO_MAKEUP: BoolSpec = BoolSpec { id: "auto_makeup", name: "Auto Makeup", default: false };
/// Reserved for the oversampled gain cell (Character saturation, a later slice); not drawn yet.
pub const OVERSAMPLING: ChoiceSpec = ChoiceSpec { id: "oversampling", name: "Oversampling", choices: &["1×", "2×", "4×"], default: 0 };

// ---- Sidechain ----------------------------------------------------------------------------

pub const SC_SOURCE: ChoiceSpec = ChoiceSpec { id: "sc_source", name: "Sidechain", choices: &["Internal", "External"], default: 0 };
/// Hear the filtered detector signal instead of the result (monitoring: never in presets).
pub const LISTEN: BoolSpec = BoolSpec { id: "listen", name: "Listen", default: false };
pub const K_ON: BoolSpec = BoolSpec { id: "on", name: "On", default: false };
pub const K_TYPE: ChoiceSpec = ChoiceSpec { id: "type", name: "Type", choices: &BandType::NAMES, default: 0 };
pub const K_FREQ: ParamSpec = spec("freq", "Frequency", "Hz", 20.0, 20_000.0, 1000.0, ParamScale::Log, 0, (1.0 / 6.0, 1.0 / 48.0), ParamDisplay::Frequency);
pub const K_GAIN: ParamSpec = spec("gain", "Gain", "dB", -24.0, 24.0, 0.0, ParamScale::Linear, 1, (0.5, 0.1), ParamDisplay::Number);
pub const K_Q: ParamSpec = spec("q", "Q", "", 0.1, 10.0, std::f32::consts::FRAC_1_SQRT_2, ParamScale::Log, 2, (1.0 / 6.0, 1.0 / 24.0), ParamDisplay::Number);
/// The detector EQ bands' first types and frequencies: a low cut, a bell, a high shelf.
const KEY_DEFAULTS: [(usize, f32); KEY_BANDS] = [(3, 100.0), (0, 1000.0), (2, 6000.0)];
static KEY_TYPES: [ChoiceSpec; KEY_BANDS] = [
    ChoiceSpec { default: KEY_DEFAULTS[0].0, ..K_TYPE },
    ChoiceSpec { default: KEY_DEFAULTS[1].0, ..K_TYPE },
    ChoiceSpec { default: KEY_DEFAULTS[2].0, ..K_TYPE },
];
static KEY_FREQS: [ParamSpec; KEY_BANDS] = [
    ParamSpec { default: KEY_DEFAULTS[0].1, ..K_FREQ },
    ParamSpec { default: KEY_DEFAULTS[1].1, ..K_FREQ },
    ParamSpec { default: KEY_DEFAULTS[2].1, ..K_FREQ },
];

// ---- Output (as the EQ) -------------------------------------------------------------------

pub const MIX: ParamSpec = spec("mix", "Mix", "%", 0.0, 100.0, 100.0, ParamScale::Linear, 0, (5.0, 1.0), ParamDisplay::Number);
pub const OUT_PAN: ParamSpec = spec("out_pan", "Output Pan", "%", -100.0, 100.0, 0.0, ParamScale::Linear, 0, (5.0, 1.0), ParamDisplay::Number);
pub const OUT_GAIN: ParamSpec = spec("out_gain", "Output", "dB", -24.0, 24.0, 0.0, ParamScale::Linear, 1, (0.5, 0.1), ParamDisplay::Number);
pub const DELTA: BoolSpec = BoolSpec { id: "delta", name: "Delta", default: false };
pub const BYPASS: BoolSpec = BoolSpec { id: "bypass", name: "Bypass", default: false };

// ---- Stages -------------------------------------------------------------------------------

pub const S_ON: BoolSpec = BoolSpec { id: "on", name: "On", default: false };
pub const S_QUAD: ChoiceSpec = ChoiceSpec { id: "quad", name: "Quadrant", choices: &Quadrant::NAMES, default: 0 };
pub const S_THRESH: ParamSpec = spec("thresh", "Threshold", "dB", -60.0, 0.0, -18.0, ParamScale::Linear, 1, (0.5, 0.1), ParamDisplay::Number);
pub const S_RATIO: ParamSpec = spec("ratio", "Ratio", ":1", 1.0, RATIO_MAX, 4.0, ParamScale::Log, 1, (1.0 / 6.0, 1.0 / 24.0), ParamDisplay::Ratio);
pub const S_KNEE: ParamSpec = spec("knee", "Knee", "dB", 0.0, 24.0, 6.0, ParamScale::Linear, 1, (0.5, 0.1), ParamDisplay::Number);
pub const S_ATTACK: ParamSpec = spec("attack", "Attack", "ms", 0.05, 250.0, 10.0, ParamScale::Log, 1, (0.25, 1.0 / 16.0), ParamDisplay::Time);
pub const S_RELEASE: ParamSpec = spec("release", "Release", "ms", 5.0, 2500.0, 120.0, ParamScale::Log, 0, (0.25, 1.0 / 16.0), ParamDisplay::Time);
pub const S_RANGE: ParamSpec = spec("range", "Range", "dB", 0.0, 60.0, 60.0, ParamScale::Linear, 0, (1.0, 0.1), ParamDisplay::Number);
pub const S_DET: ChoiceSpec = ChoiceSpec { id: "det", name: "Detector", choices: &Detector::NAMES, default: 1 };
/// Hear only this stage's gain change (monitoring: never in presets).
pub const S_SOLO: BoolSpec = BoolSpec { id: "solo", name: "Solo", default: false };

/// Attack and release dials: common values get more travel (as the EQ's dynamics dials).
pub const ATTACK_DIAL: DialCurve = DialCurve(&[(0.0, 0.05), (0.25, 1.0), (0.5, 10.0), (0.8, 50.0), (1.0, 250.0)]);
pub const RELEASE_DIAL: DialCurve = DialCurve(&[(0.0, 5.0), (0.2, 40.0), (0.45, 100.0), (0.8, 400.0), (1.0, 2500.0)]);

/// A stage parameter's host id, e.g. `s2_thresh` (`stage` from 0).
pub fn stage_id(stage: usize, param: &str) -> String {
    format!("s{}_{param}", stage + 1)
}

/// A detector EQ band parameter's host id, e.g. `k1_freq` (`band` from 0).
pub fn key_id(band: usize, param: &str) -> String {
    format!("k{}_{param}", band + 1)
}

/// Every parameter's manifest line, in host order.
pub fn manifest() -> String {
    let mut lines = vec![CHARACTER.manifest_line()];
    lines.extend([C_ATTACK, C_RELEASE, C_DRIVE, C_TAIL, C_PROGRAM, C_BEND].iter().map(ParamSpec::manifest_line));
    lines.push(SMART.manifest_line());
    lines.extend([SMART_AMOUNT, TARGET_GR, SIBILANCE].iter().map(ParamSpec::manifest_line));
    lines.push(TOPOLOGY.manifest_line());
    lines.push(STEREO.manifest_line());
    lines.extend([LINK, LOOKAHEAD, HOLD, MAKEUP].iter().map(ParamSpec::manifest_line));
    lines.push(AUTO_MAKEUP.manifest_line());
    lines.push(OVERSAMPLING.manifest_line());
    lines.push(SC_SOURCE.manifest_line());
    lines.push(LISTEN.manifest_line());
    for k in 0..KEY_BANDS {
        let prefix = |line: String| format!("k{}_{line}", k + 1);
        lines.push(prefix(K_ON.manifest_line()));
        lines.push(prefix(KEY_TYPES[k].manifest_line()));
        lines.push(prefix(KEY_FREQS[k].manifest_line()));
        lines.push(prefix(K_GAIN.manifest_line()));
        lines.push(prefix(K_Q.manifest_line()));
    }
    lines.extend([MIX, OUT_PAN, OUT_GAIN].iter().map(ParamSpec::manifest_line));
    lines.push(DELTA.manifest_line());
    lines.push(BYPASS.manifest_line());
    for s in 0..STAGES {
        let prefix = |line: String| format!("s{}_{line}", s + 1);
        lines.push(prefix(S_ON.manifest_line()));
        lines.push(prefix(S_QUAD.manifest_line()));
        for spec in [S_THRESH, S_RATIO, S_KNEE, S_ATTACK, S_RELEASE, S_RANGE] {
            lines.push(prefix(spec.manifest_line()));
        }
        lines.push(prefix(S_DET.manifest_line()));
        lines.push(prefix(S_SOLO.manifest_line()));
    }
    lines.join("\n") + "\n"
}

/// Every parameter id, in host order.
pub fn ids() -> Vec<String> {
    manifest().lines().filter_map(|l| l.split(" | ").next()).map(str::to_owned).collect()
}

/// Parameters that are monitoring, not sound: never in presets or snapshots.
pub fn is_monitoring(id: &str) -> bool {
    id == LISTEN.id || id == DELTA.id || id == BYPASS.id || id.ends_with(&format!("_{}", S_SOLO.id))
}

/// The sound parameters: what presets, snapshots and undo cover.
pub fn sound_ids() -> Vec<String> {
    ids().into_iter().filter(|id| !is_monitoring(id)).collect()
}

pub struct StageParams {
    pub on: BoolParam,
    pub quad: IntParam,
    pub thresh: FloatParam,
    pub ratio: FloatParam,
    pub knee: FloatParam,
    pub attack: FloatParam,
    pub release: FloatParam,
    pub range: FloatParam,
    pub det: IntParam,
    pub solo: BoolParam,
}

impl StageParams {
    fn new(stage: usize) -> Self {
        let name = |what: &str| format!("Stage {} {what}", stage + 1);
        Self {
            on: bool_param(&S_ON, name("On")),
            quad: choice_param(&S_QUAD, name("Quadrant")),
            thresh: float_param_named(&S_THRESH, name("Threshold")),
            ratio: float_param_named(&S_RATIO, name("Ratio")),
            knee: float_param_named(&S_KNEE, name("Knee")),
            attack: float_param_named(&S_ATTACK, name("Attack")),
            release: float_param_named(&S_RELEASE, name("Release")),
            range: float_param_named(&S_RANGE, name("Range")),
            det: choice_param(&S_DET, name("Detector")),
            solo: bool_param(&S_SOLO, name("Solo")),
        }
    }

    pub fn stage(&self) -> Stage {
        Stage {
            on: self.on.value(),
            quadrant: Quadrant::from_index(self.quad.value().max(0) as usize),
            threshold_db: self.thresh.value(),
            ratio: self.ratio.value(),
            knee_db: self.knee.value(),
            range_db: self.range.value(),
            attack_ms: self.attack.value(),
            release_ms: self.release.value(),
            detector: Detector::from_index(self.det.value().max(0) as usize),
        }
    }
}

pub struct KeyParams {
    pub on: BoolParam,
    pub kind: IntParam,
    pub freq: FloatParam,
    pub gain: FloatParam,
    pub q: FloatParam,
}

impl KeyParams {
    fn new(band: usize) -> Self {
        let name = |what: &str| format!("Sidechain Band {} {what}", band + 1);
        Self {
            on: bool_param(&K_ON, name("On")),
            kind: choice_param(&KEY_TYPES[band], name("Type")),
            freq: float_param_named(&KEY_FREQS[band], name("Frequency")),
            gain: float_param_named(&K_GAIN, name("Gain")),
            q: float_param_named(&K_Q, name("Q")),
        }
    }

    pub fn band(&self) -> BandSettings {
        BandSettings {
            enabled: self.on.value(),
            kind: BandType::from_index(self.kind.value().max(0) as usize),
            freq: f64::from(self.freq.value()),
            gain_db: f64::from(self.gain.value()),
            q: f64::from(self.q.value()),
            ..BandSettings::default()
        }
    }
}

pub struct CompParams {
    pub character: IntParam,
    pub c_attack: FloatParam,
    pub c_release: FloatParam,
    pub c_drive: FloatParam,
    pub c_tail: FloatParam,
    pub c_program: FloatParam,
    pub c_bend: FloatParam,
    pub smart: IntParam,
    pub smart_amount: FloatParam,
    pub target_gr: FloatParam,
    pub sibilance: FloatParam,
    pub topology: IntParam,
    pub stereo: IntParam,
    pub link: FloatParam,
    pub lookahead: FloatParam,
    pub hold: FloatParam,
    pub makeup: FloatParam,
    pub auto_makeup: BoolParam,
    pub oversampling: IntParam,
    pub sc_source: IntParam,
    pub listen: BoolParam,
    pub keys: [KeyParams; KEY_BANDS],
    pub mix: FloatParam,
    pub out_pan: FloatParam,
    pub out_gain: FloatParam,
    pub delta: BoolParam,
    pub bypass: BoolParam,
    pub stages: [StageParams; STAGES],
    /// Snapshots A–D and the current preset's name (not parameters: saved with the state through
    /// `serialize_fields`).
    pub snapshots: std::sync::Arc<Snapshots>,
    pub preset: std::sync::Arc<CurrentPreset>,
    /// The history's level range picked by hand (0: Auto), saved with the state.
    pub display_range: std::sync::Arc<mite_core::AtomicF32>,
}

impl Default for CompParams {
    fn default() -> Self {
        let named = |spec: &'static ChoiceSpec| choice_param(spec, spec.name.to_owned());
        let flag = |spec: &'static BoolSpec| bool_param(spec, spec.name.to_owned());
        Self {
            character: named(&CHARACTER),
            c_attack: float_param(&C_ATTACK),
            c_release: float_param(&C_RELEASE),
            c_drive: float_param(&C_DRIVE),
            c_tail: float_param(&C_TAIL),
            c_program: float_param(&C_PROGRAM),
            c_bend: float_param(&C_BEND),
            smart: named(&SMART),
            smart_amount: float_param(&SMART_AMOUNT),
            target_gr: float_param(&TARGET_GR),
            sibilance: float_param(&SIBILANCE),
            topology: named(&TOPOLOGY),
            stereo: named(&STEREO),
            link: float_param(&LINK),
            lookahead: float_param(&LOOKAHEAD),
            hold: float_param(&HOLD),
            makeup: float_param(&MAKEUP),
            auto_makeup: flag(&AUTO_MAKEUP),
            oversampling: named(&OVERSAMPLING),
            sc_source: named(&SC_SOURCE),
            listen: flag(&LISTEN),
            keys: std::array::from_fn(KeyParams::new),
            mix: float_param(&MIX),
            out_pan: float_param(&OUT_PAN),
            out_gain: float_param(&OUT_GAIN),
            delta: flag(&DELTA),
            bypass: flag(&BYPASS).make_bypass(),
            stages: std::array::from_fn(StageParams::new),
            snapshots: std::sync::Arc::new(Snapshots::new()),
            preset: std::sync::Arc::new(CurrentPreset::default()),
            display_range: std::sync::Arc::new(mite_core::AtomicF32::new(0.0)),
        }
    }
}

impl CompParams {
    /// The Character in force: a built-in one, or Custom's six values. `audition`, when set, is
    /// a Character being tried from the menu (it never touches the parameter).
    pub fn style(&self, audition: Option<usize>) -> Style {
        let index = audition.unwrap_or(self.character.value().max(0) as usize).min(CUSTOM);
        if index < CUSTOM {
            return STYLES[index];
        }
        Style {
            attack: self.c_attack.value(),
            release: self.c_release.value(),
            drive_db: self.c_drive.value(),
            slow_tail: self.c_tail.value() / 100.0,
            program: self.c_program.value() / 100.0,
            ratio_bend: self.c_bend.value() / 100.0,
        }
    }

    pub fn settings(&self, audition: Option<usize>) -> CompSettings {
        CompSettings {
            stages: std::array::from_fn(|i| self.stages[i].stage()),
            style: self.style(audition),
            feedback: self.topology.value() == 1,
            mid_side: self.stereo.value() == 1,
            link: self.link.value() / 100.0,
            lookahead_ms: self.lookahead.value(),
            hold_ms: self.hold.value(),
            makeup_db: self.makeup.value(),
            auto_makeup: self.auto_makeup.value(),
            smart: Smart::from_index(self.smart.value().max(0) as usize),
            smart_amount: self.smart_amount.value() / 100.0,
            target_gr_db: self.target_gr.value(),
            sibilance: self.sibilance.value() / 100.0,
            external_key: self.sc_source.value() == 1,
            key_bands: std::array::from_fn(|i| self.keys[i].band()),
            listen: self.listen.value(),
            solo: self.stages.iter().position(|s| s.solo.value() && s.on.value()),
        }
    }
}

// SAFETY: every `ParamPtr` points into `self`, which nih-plug keeps alive (in an `Arc`) for as
// long as it uses the map. The ids match `manifest()` (checked by tests/params.rs).
unsafe impl Params for CompParams {
    fn serialize_fields(&self) -> std::collections::BTreeMap<String, String> {
        let live = mite_plugin::state::live_values(self, &sound_ids());
        let mut fields = mite_plugin::state::session_fields(STATE_VERSION, &self.snapshots, &self.preset, live);
        mite_plugin::state::add_display_range(&mut fields, self.display_range.load());
        fields
    }

    fn deserialize_fields(&self, serialized: &std::collections::BTreeMap<String, String>) {
        mite_plugin::state::load_session_fields(serialized, &self.snapshots, &self.preset);
        self.display_range.store(mite_plugin::state::display_range(serialized));
    }

    fn param_map(&self) -> Vec<(String, ParamPtr, String)> {
        let global = |id: &str, ptr: ParamPtr, group: &str| (id.to_owned(), ptr, group.to_owned());
        let mut map = vec![
            global(CHARACTER.id, self.character.as_ptr(), "Character"),
            global(C_ATTACK.id, self.c_attack.as_ptr(), "Character"),
            global(C_RELEASE.id, self.c_release.as_ptr(), "Character"),
            global(C_DRIVE.id, self.c_drive.as_ptr(), "Character"),
            global(C_TAIL.id, self.c_tail.as_ptr(), "Character"),
            global(C_PROGRAM.id, self.c_program.as_ptr(), "Character"),
            global(C_BEND.id, self.c_bend.as_ptr(), "Character"),
            global(SMART.id, self.smart.as_ptr(), "Smart"),
            global(SMART_AMOUNT.id, self.smart_amount.as_ptr(), "Smart"),
            global(TARGET_GR.id, self.target_gr.as_ptr(), "Smart"),
            global(SIBILANCE.id, self.sibilance.as_ptr(), "Smart"),
            global(TOPOLOGY.id, self.topology.as_ptr(), "Detector"),
            global(STEREO.id, self.stereo.as_ptr(), "Detector"),
            global(LINK.id, self.link.as_ptr(), "Detector"),
            global(LOOKAHEAD.id, self.lookahead.as_ptr(), "Detector"),
            global(HOLD.id, self.hold.as_ptr(), "Detector"),
            global(MAKEUP.id, self.makeup.as_ptr(), "Output"),
            global(AUTO_MAKEUP.id, self.auto_makeup.as_ptr(), "Output"),
            global(OVERSAMPLING.id, self.oversampling.as_ptr(), "Detector"),
            global(SC_SOURCE.id, self.sc_source.as_ptr(), "Sidechain"),
            global(LISTEN.id, self.listen.as_ptr(), "Sidechain"),
        ];
        for (i, k) in self.keys.iter().enumerate() {
            let group = "Sidechain".to_owned();
            map.push((key_id(i, K_ON.id), k.on.as_ptr(), group.clone()));
            map.push((key_id(i, K_TYPE.id), k.kind.as_ptr(), group.clone()));
            map.push((key_id(i, K_FREQ.id), k.freq.as_ptr(), group.clone()));
            map.push((key_id(i, K_GAIN.id), k.gain.as_ptr(), group.clone()));
            map.push((key_id(i, K_Q.id), k.q.as_ptr(), group));
        }
        map.extend([
            global(MIX.id, self.mix.as_ptr(), ""),
            global(OUT_PAN.id, self.out_pan.as_ptr(), ""),
            global(OUT_GAIN.id, self.out_gain.as_ptr(), ""),
            global(DELTA.id, self.delta.as_ptr(), ""),
            global(BYPASS.id, self.bypass.as_ptr(), ""),
        ]);
        for (i, s) in self.stages.iter().enumerate() {
            let group = format!("Stage {}", i + 1);
            map.push((stage_id(i, S_ON.id), s.on.as_ptr(), group.clone()));
            map.push((stage_id(i, S_QUAD.id), s.quad.as_ptr(), group.clone()));
            map.push((stage_id(i, S_THRESH.id), s.thresh.as_ptr(), group.clone()));
            map.push((stage_id(i, S_RATIO.id), s.ratio.as_ptr(), group.clone()));
            map.push((stage_id(i, S_KNEE.id), s.knee.as_ptr(), group.clone()));
            map.push((stage_id(i, S_ATTACK.id), s.attack.as_ptr(), group.clone()));
            map.push((stage_id(i, S_RELEASE.id), s.release.as_ptr(), group.clone()));
            map.push((stage_id(i, S_RANGE.id), s.range.as_ptr(), group.clone()));
            map.push((stage_id(i, S_DET.id), s.det.as_ptr(), group.clone()));
            map.push((stage_id(i, S_SOLO.id), s.solo.as_ptr(), group));
        }
        map
    }
}
