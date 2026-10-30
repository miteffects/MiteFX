//! Mite EQ parameters (BLUEPRINT §8.4).
//!
//! IDs are frozen once released (`tests/golden/mite_eq.params.txt`): globals by name, bands as
//! `b{NN}_{param}` with all 24 bands pre-declared because hosts need static parameter lists.

use mite_core::{BoolSpec, ChoiceSpec, DialCurve, ParamDisplay, ParamScale, ParamSpec};
use mite_dsp::eq::{BandSettings, BandType, DetectSource, Dynamics, DynamicsMode, Placement, DEFAULT_SLOPE, DEFAULT_TRANSIENT_MS, DEFAULT_TRANSITION, SLOPE_NAMES};
use mite_plugin::{bool_param, choice_param, float_param, float_param_named};
use nih_plug::prelude::*;

pub const BANDS: usize = 24;

pub const OUT_GAIN: ParamSpec = ParamSpec {
    id: "out_gain",
    name: "Output",
    unit: "dB",
    min: -24.0,
    max: 24.0,
    default: 0.0,
    scale: ParamScale::Linear,
    smoothing_ms: 20.0,
    decimals: 1,
    wheel_step: 0.5,
    wheel_step_fine: 0.1,
    display: ParamDisplay::Number,
};

/// Output balance, % (−100 left … 100 right).
pub const OUT_PAN: ParamSpec = ParamSpec {
    id: "out_pan",
    name: "Output Pan",
    unit: "%",
    min: -100.0,
    max: 100.0,
    default: 0.0,
    scale: ParamScale::Linear,
    smoothing_ms: 20.0,
    decimals: 0,
    wheel_step: 5.0,
    wheel_step_fine: 1.0,
    display: ParamDisplay::Number,
};

/// Global dry/wet, %. The dry path is delayed by the latency, so any mix is phase-aligned.
pub const MIX: ParamSpec = ParamSpec {
    id: "mix",
    name: "Mix",
    unit: "%",
    min: 0.0,
    max: 100.0,
    default: 100.0,
    scale: ParamScale::Linear,
    smoothing_ms: 20.0,
    decimals: 0,
    wheel_step: 5.0,
    wheel_step_fine: 1.0,
    display: ParamDisplay::Number,
};

/// Loudness-based gain compensation for the static curve.
pub const AUTO_GAIN: BoolSpec = BoolSpec { id: "auto_gain", name: "Auto Gain", default: false };
/// Listen to what the EQ changes (wet minus dry) instead of the result.
pub const DELTA: BoolSpec = BoolSpec { id: "delta", name: "Delta", default: false };
/// The whole plugin, latency kept (the host's bypass maps here).
pub const BYPASS: BoolSpec = BoolSpec { id: "bypass", name: "Bypass", default: false };

pub const ON: BoolSpec = BoolSpec { id: "on", name: "On", default: false };
pub const TYPE: ChoiceSpec = ChoiceSpec { id: "type", name: "Type", choices: &BandType::NAMES, default: 0 };

pub const FREQ: ParamSpec = ParamSpec {
    id: "freq",
    name: "Frequency",
    unit: "Hz",
    min: 10.0,
    max: 30_000.0,
    default: 1000.0,
    scale: ParamScale::Log,
    smoothing_ms: 20.0,
    decimals: 0,
    wheel_step: 1.0 / 6.0,
    wheel_step_fine: 1.0 / 48.0,
    display: ParamDisplay::Frequency,
};

pub const GAIN: ParamSpec = ParamSpec {
    id: "gain",
    name: "Gain",
    unit: "dB",
    min: -30.0,
    max: 30.0,
    default: 0.0,
    scale: ParamScale::Linear,
    smoothing_ms: 20.0,
    decimals: 1,
    wheel_step: 0.5,
    wheel_step_fine: 0.1,
    display: ParamDisplay::Number,
};

pub const Q: ParamSpec = ParamSpec {
    id: "q",
    name: "Q",
    unit: "",
    min: 0.025,
    max: 40.0,
    default: std::f32::consts::FRAC_1_SQRT_2,
    scale: ParamScale::Log,
    smoothing_ms: 20.0,
    decimals: 2,
    wheel_step: 1.0 / 6.0,
    wheel_step_fine: 1.0 / 24.0,
    display: ParamDisplay::Number,
};

pub const SLOPE: ChoiceSpec = ChoiceSpec { id: "slope", name: "Slope", choices: &SLOPE_NAMES, default: DEFAULT_SLOPE };

pub const STEREO: ChoiceSpec = ChoiceSpec { id: "stereo", name: "Stereo", choices: &Placement::NAMES, default: 0 };

// Dynamic EQ (BLUEPRINT §8.1). Defaults match `mite_dsp::eq::Dynamics::default()`.
pub const DYN: BoolSpec = BoolSpec { id: "dyn", name: "Dynamic", default: false };

pub const RANGE: ParamSpec = ParamSpec {
    id: "range",
    name: "Dynamic Range",
    unit: "dB",
    min: -30.0,
    max: 30.0,
    default: -6.0,
    scale: ParamScale::Linear,
    smoothing_ms: 0.0,
    decimals: 1,
    wheel_step: 0.5,
    wheel_step_fine: 0.1,
    display: ParamDisplay::Number,
};

pub const THRESH: ParamSpec = ParamSpec {
    id: "thresh",
    name: "Threshold",
    unit: "dB",
    min: -80.0,
    max: 0.0,
    default: -30.0,
    scale: ParamScale::Linear,
    smoothing_ms: 0.0,
    decimals: 1,
    wheel_step: 1.0,
    wheel_step_fine: 0.1,
    display: ParamDisplay::Number,
};

pub const ATTACK: ParamSpec = ParamSpec {
    id: "attack",
    name: "Attack",
    unit: "ms",
    min: 0.5,
    max: 200.0,
    default: 10.0,
    scale: ParamScale::Log,
    smoothing_ms: 0.0,
    decimals: 1,
    wheel_step: 1.0 / 12.0,
    wheel_step_fine: 1.0 / 48.0,
    display: ParamDisplay::Time,
};

pub const RELEASE: ParamSpec = ParamSpec {
    id: "release",
    name: "Release",
    unit: "ms",
    min: 10.0,
    max: 2500.0,
    default: 150.0,
    scale: ParamScale::Log,
    smoothing_ms: 0.0,
    decimals: 0,
    wheel_step: 1.0 / 12.0,
    wheel_step_fine: 1.0 / 48.0,
    display: ParamDisplay::Time,
};

pub const DMODE: ChoiceSpec = ChoiceSpec { id: "dmode", name: "Dynamics Mode", choices: &DynamicsMode::NAMES, default: 0 };

/// Transient mode: how far the band moves on attacks.
pub const TRANSIENT: ParamSpec = ParamSpec {
    id: "transient",
    name: "Transient Attack",
    unit: "dB",
    min: -12.0,
    max: 12.0,
    default: 0.0,
    scale: ParamScale::Linear,
    smoothing_ms: 0.0,
    decimals: 1,
    wheel_step: 0.5,
    wheel_step_fine: 0.1,
    display: ParamDisplay::Number,
};

/// Transient mode: how far the band moves in the tails after attacks.
pub const SUSTAIN: ParamSpec = ParamSpec {
    id: "sustain",
    name: "Sustain",
    unit: "dB",
    min: -12.0,
    max: 12.0,
    default: 0.0,
    scale: ParamScale::Linear,
    smoothing_ms: 0.0,
    decimals: 1,
    wheel_step: 0.5,
    wheel_step_fine: 0.1,
    display: ParamDisplay::Number,
};

/// Transient mode: how long attack lasts.
pub const TTIME: ParamSpec = ParamSpec {
    id: "ttime",
    name: "Transient Time",
    unit: "ms",
    min: 5.0,
    max: 200.0,
    default: DEFAULT_TRANSIENT_MS as f32,
    scale: ParamScale::Log,
    smoothing_ms: 0.0,
    decimals: 0,
    wheel_step: 1.0 / 12.0,
    wheel_step_fine: 1.0 / 48.0,
    display: ParamDisplay::Time,
};

/// Transient mode: how wide the crossfade from attack to sustain is, % of the transient time.
pub const TRANSITION: ParamSpec = ParamSpec {
    id: "transition",
    name: "Transition",
    unit: "%",
    min: 0.0,
    max: 200.0,
    default: (DEFAULT_TRANSITION * 100.0) as f32,
    scale: ParamScale::Linear,
    smoothing_ms: 0.0,
    decimals: 0,
    wheel_step: 5.0,
    wheel_step_fine: 1.0,
    display: ParamDisplay::Number,
};

/// What a dynamic band's detector listens to: its own region, the sidechain, or another band's
/// region (`mite_dsp::eq::DetectSource` order).
pub const DETECT_NAMES: [&str; 26] = ["Own", "Sidechain", "Band 1", "Band 2", "Band 3", "Band 4", "Band 5", "Band 6", "Band 7", "Band 8", "Band 9", "Band 10", "Band 11", "Band 12", "Band 13", "Band 14", "Band 15", "Band 16", "Band 17", "Band 18", "Band 19", "Band 20", "Band 21", "Band 22", "Band 23", "Band 24"];
pub const DSRC: ChoiceSpec = ChoiceSpec { id: "dsrc", name: "Detection", choices: &DETECT_NAMES, default: 0 };

/// Listen to this band's region alone (one band at a time).
pub const SOLO: BoolSpec = BoolSpec { id: "solo", name: "Solo", default: false };

/// Compressor mode: work per frequency bin inside the band's range.
pub const SPECTRAL: BoolSpec = BoolSpec { id: "spectral", name: "Spectral", default: false };

/// Spectral compressor: compare each frequency with its surroundings instead of a fixed level,
/// and the threshold then (dB above the surroundings).
pub const SREL: BoolSpec = BoolSpec { id: "srel", name: "Spectral Relative", default: false };

pub const RTHRESH: ParamSpec = ParamSpec {
    id: "rthresh",
    name: "Relative Threshold",
    unit: "dB",
    min: 0.0,
    max: 24.0,
    default: 6.0,
    scale: ParamScale::Linear,
    smoothing_ms: 0.0,
    decimals: 1,
    wheel_step: 0.5,
    wheel_step_fine: 0.1,
    display: ParamDisplay::Number,
};

/// Spectral compressor: how far each frequency's gain spreads to its neighbours.
pub const SSMOOTH: ParamSpec = ParamSpec {
    id: "ssmooth",
    name: "Spectral Smoothing",
    unit: "%",
    min: 0.0,
    max: 100.0,
    default: 25.0,
    scale: ParamScale::Linear,
    smoothing_ms: 0.0,
    decimals: 0,
    wheel_step: 5.0,
    wheel_step_fine: 1.0,
    display: ParamDisplay::Number,
};

/// How the attack and release dials feel: common times get most of the travel (attack up to
/// 50 ms, release 100–300 ms); the rarely used ends move faster.
pub const ATTACK_DIAL: DialCurve = DialCurve(&[(0.0, 0.5), (0.12, 2.0), (0.3, 10.0), (0.75, 50.0), (1.0, 200.0)]);
pub const RELEASE_DIAL: DialCurve = DialCurve(&[(0.0, 10.0), (0.15, 40.0), (0.3, 100.0), (0.75, 300.0), (1.0, 2500.0)]);

pub const RATIO: ParamSpec = ParamSpec {
    id: "ratio",
    name: "Ratio",
    unit: ":1",
    min: 1.0,
    max: 20.0,
    default: 3.0,
    scale: ParamScale::Log,
    smoothing_ms: 0.0,
    decimals: 1,
    wheel_step: 1.0 / 12.0,
    wheel_step_fine: 1.0 / 48.0,
    display: ParamDisplay::Number,
};

/// Processing mode (BLUEPRINT §8.2): Zero Latency, Natural Phase or Linear Phase.
pub const MODE: ChoiceSpec = ChoiceSpec { id: "mode", name: "Processing Mode", choices: &crate::dsp::PhaseMode::NAMES, default: 0 };

/// Linear-phase resolution: kernel length, and with it latency and low-frequency precision.
pub const LP_RES: ChoiceSpec = ChoiceSpec { id: "lp_res", name: "Linear Phase Resolution", choices: &crate::dsp::LP_RESOLUTIONS, default: 2 };

/// The state format version (`mite_plugin::state`). Bump it, with a migration step in
/// `MiteEq::filter_state` and a new golden state file, when a release changes what a state means.
pub const STATE_VERSION: u32 = 1;

/// The per-band parameters a snapshot holds (every one but solo, which is an audition).
pub const SNAPSHOT_PARAMS: [&str; 23] = [
    ON.id, TYPE.id, FREQ.id, GAIN.id, Q.id, SLOPE.id, STEREO.id, DYN.id, RANGE.id, THRESH.id, ATTACK.id, RELEASE.id, RATIO.id, DMODE.id, SUSTAIN.id,
    TRANSIENT.id, TTIME.id, TRANSITION.id, SPECTRAL.id, SSMOOTH.id, SREL.id, RTHRESH.id, DSRC.id,
];

/// `b03_freq` for band index 2.
pub fn band_id(band: usize, param: &str) -> String {
    format!("b{:02}_{param}", band + 1)
}

/// The frozen parameter manifest, in host order.
/// Every snapshot parameter id, in order (the bands' sound settings).
pub fn snapshot_ids() -> Vec<String> {
    (0..BANDS).flat_map(|b| SNAPSHOT_PARAMS.iter().map(move |p| band_id(b, p))).collect()
}

pub fn manifest() -> String {
    let mut lines = vec![
        OUT_GAIN.manifest_line(),
        MODE.manifest_line(),
        LP_RES.manifest_line(),
        OUT_PAN.manifest_line(),
        MIX.manifest_line(),
        AUTO_GAIN.manifest_line(),
        DELTA.manifest_line(),
        BYPASS.manifest_line(),
    ];
    for b in 0..BANDS {
        let prefix = |line: String| format!("b{:02}_{line}", b + 1);
        lines.push(prefix(ON.manifest_line()));
        lines.push(prefix(TYPE.manifest_line()));
        lines.push(prefix(FREQ.manifest_line()));
        lines.push(prefix(GAIN.manifest_line()));
        lines.push(prefix(Q.manifest_line()));
        lines.push(prefix(SLOPE.manifest_line()));
        lines.push(prefix(STEREO.manifest_line()));
        lines.push(prefix(DYN.manifest_line()));
        for spec in [RANGE, THRESH, ATTACK, RELEASE, RATIO] {
            lines.push(prefix(spec.manifest_line()));
        }
        lines.push(prefix(DMODE.manifest_line()));
        lines.push(prefix(SUSTAIN.manifest_line()));
        lines.push(prefix(TRANSIENT.manifest_line()));
        lines.push(prefix(TTIME.manifest_line()));
        lines.push(prefix(TRANSITION.manifest_line()));
        lines.push(prefix(SPECTRAL.manifest_line()));
        lines.push(prefix(SSMOOTH.manifest_line()));
        lines.push(prefix(SREL.manifest_line()));
        lines.push(prefix(RTHRESH.manifest_line()));
        lines.push(prefix(SOLO.manifest_line()));
        lines.push(prefix(DSRC.manifest_line()));
    }
    lines.join("\n") + "\n"
}

pub struct BandParams {
    pub on: BoolParam,
    pub kind: IntParam,
    pub freq: FloatParam,
    pub gain: FloatParam,
    pub q: FloatParam,
    pub slope: IntParam,
    pub stereo: IntParam,
    pub dyn_on: BoolParam,
    pub range: FloatParam,
    pub thresh: FloatParam,
    pub attack: FloatParam,
    pub release: FloatParam,
    pub ratio: FloatParam,
    pub dmode: IntParam,
    pub sustain: FloatParam,
    pub transient: FloatParam,
    pub ttime: FloatParam,
    pub transition: FloatParam,
    pub spectral: BoolParam,
    pub ssmooth: FloatParam,
    pub srel: BoolParam,
    pub rthresh: FloatParam,
    pub solo: BoolParam,
    pub dsrc: IntParam,
}

impl BandParams {
    fn new(band: usize) -> Self {
        let name = |what: &str| format!("Band {} {what}", band + 1);
        Self {
            on: bool_param(&ON, name("On")),
            kind: choice_param(&TYPE, name("Type")),
            freq: float_param_named(&FREQ, name("Frequency")),
            gain: float_param_named(&GAIN, name("Gain")),
            q: float_param_named(&Q, name("Q")),
            slope: choice_param(&SLOPE, name("Slope")),
            stereo: choice_param(&STEREO, name("Stereo")),
            dyn_on: bool_param(&DYN, name("Dynamic")),
            range: float_param_named(&RANGE, name("Dynamic Range")),
            thresh: float_param_named(&THRESH, name("Threshold")),
            attack: float_param_named(&ATTACK, name("Attack")),
            release: float_param_named(&RELEASE, name("Release")),
            ratio: float_param_named(&RATIO, name("Ratio")),
            dmode: choice_param(&DMODE, name("Dynamics Mode")),
            sustain: float_param_named(&SUSTAIN, name("Sustain")),
            transient: float_param_named(&TRANSIENT, name("Transient Attack")),
            ttime: float_param_named(&TTIME, name("Transient Time")),
            transition: float_param_named(&TRANSITION, name("Transition")),
            spectral: bool_param(&SPECTRAL, name("Spectral")),
            ssmooth: float_param_named(&SSMOOTH, name("Spectral Smoothing")),
            srel: bool_param(&SREL, name("Spectral Relative")),
            rthresh: float_param_named(&RTHRESH, name("Relative Threshold")),
            solo: bool_param(&SOLO, name("Solo")),
            dsrc: choice_param(&DSRC, name("Detection")),
        }
    }

    /// The band as the DSP sees it. Audio-thread safe (atomic reads only).
    pub fn settings(&self) -> BandSettings {
        BandSettings {
            enabled: self.on.value(),
            kind: BandType::from_index(self.kind.value().max(0) as usize),
            freq: f64::from(self.freq.value()),
            gain_db: f64::from(self.gain.value()),
            q: f64::from(self.q.value()),
            slope: self.slope.value().max(0) as usize,
            placement: Placement::from_index(self.stereo.value().max(0) as usize),
            dynamics: Dynamics {
                enabled: self.dyn_on.value(),
                mode: DynamicsMode::from_index(self.dmode.value().max(0) as usize),
                range_db: f64::from(self.range.value()),
                transient_db: f64::from(self.transient.value()),
                sustain_db: f64::from(self.sustain.value()),
                transient_ms: f64::from(self.ttime.value()),
                transition: f64::from(self.transition.value()) / 100.0,
                threshold_db: f64::from(self.thresh.value()),
                attack_ms: f64::from(self.attack.value()),
                release_ms: f64::from(self.release.value()),
                ratio: f64::from(self.ratio.value()),
                spectral: self.spectral.value(),
                smoothing: f64::from(self.ssmooth.value()) / 100.0,
                relative: self.srel.value(),
                relative_db: f64::from(self.rthresh.value()),
                source: DetectSource::from_index(self.dsrc.value().max(0) as usize),
            },
        }
    }
}

pub struct EqParams {
    pub out_gain: FloatParam,
    pub mode: IntParam,
    pub lp_res: IntParam,
    pub out_pan: FloatParam,
    pub mix: FloatParam,
    pub auto_gain: BoolParam,
    pub delta: BoolParam,
    pub bypass: BoolParam,
    /// Snapshots A–D and the current preset's name (not parameters: saved with the state
    /// through `serialize_fields`).
    pub snapshots: std::sync::Arc<mite_core::snapshots::Snapshots>,
    pub preset: std::sync::Arc<mite_core::presets::CurrentPreset>,
    /// The display's dB range picked by hand (0: Auto), saved with the state.
    pub display_range: std::sync::Arc<mite_core::AtomicF32>,
    pub bands: [BandParams; BANDS],
}

impl Default for EqParams {
    fn default() -> Self {
        Self {
            out_gain: float_param(&OUT_GAIN),
            mode: choice_param(&MODE, MODE.name.to_owned()),
            lp_res: choice_param(&LP_RES, LP_RES.name.to_owned()),
            out_pan: float_param(&OUT_PAN),
            mix: float_param(&MIX),
            auto_gain: bool_param(&AUTO_GAIN, AUTO_GAIN.name.to_owned()),
            delta: bool_param(&DELTA, DELTA.name.to_owned()),
            bypass: bool_param(&BYPASS, BYPASS.name.to_owned()).make_bypass(),
            snapshots: std::sync::Arc::new(mite_core::snapshots::Snapshots::new()),
            preset: std::sync::Arc::new(mite_core::presets::CurrentPreset::default()),
            display_range: std::sync::Arc::new(mite_core::AtomicF32::new(0.0)),
            bands: std::array::from_fn(BandParams::new),
        }
    }
}

// SAFETY: every `ParamPtr` points into `self`, which nih-plug keeps alive (in an `Arc`) for as
// long as it uses the map. The ids match `manifest()` (checked by tests/params.rs).
unsafe impl Params for EqParams {
    /// The snapshots (A–D), with the band parameters now as the active slot.
    fn serialize_fields(&self) -> std::collections::BTreeMap<String, String> {
        let live = mite_plugin::state::live_values(self, &snapshot_ids());
        let mut fields = mite_plugin::state::session_fields(STATE_VERSION, &self.snapshots, &self.preset, live);
        mite_plugin::state::add_display_range(&mut fields, self.display_range.load());
        fields
    }

    fn deserialize_fields(&self, serialized: &std::collections::BTreeMap<String, String>) {
        mite_plugin::state::load_session_fields(serialized, &self.snapshots, &self.preset);
        self.display_range.store(mite_plugin::state::display_range(serialized));
    }

    fn param_map(&self) -> Vec<(String, ParamPtr, String)> {
        let mut map = vec![
            (OUT_GAIN.id.to_owned(), self.out_gain.as_ptr(), String::new()),
            (MODE.id.to_owned(), self.mode.as_ptr(), String::new()),
            (LP_RES.id.to_owned(), self.lp_res.as_ptr(), String::new()),
            (OUT_PAN.id.to_owned(), self.out_pan.as_ptr(), String::new()),
            (MIX.id.to_owned(), self.mix.as_ptr(), String::new()),
            (AUTO_GAIN.id.to_owned(), self.auto_gain.as_ptr(), String::new()),
            (DELTA.id.to_owned(), self.delta.as_ptr(), String::new()),
            (BYPASS.id.to_owned(), self.bypass.as_ptr(), String::new()),
        ];
        for (i, b) in self.bands.iter().enumerate() {
            let group = format!("Band {}", i + 1);
            map.push((band_id(i, ON.id), b.on.as_ptr(), group.clone()));
            map.push((band_id(i, TYPE.id), b.kind.as_ptr(), group.clone()));
            map.push((band_id(i, FREQ.id), b.freq.as_ptr(), group.clone()));
            map.push((band_id(i, GAIN.id), b.gain.as_ptr(), group.clone()));
            map.push((band_id(i, Q.id), b.q.as_ptr(), group.clone()));
            map.push((band_id(i, SLOPE.id), b.slope.as_ptr(), group.clone()));
            map.push((band_id(i, STEREO.id), b.stereo.as_ptr(), group.clone()));
            map.push((band_id(i, DYN.id), b.dyn_on.as_ptr(), group.clone()));
            map.push((band_id(i, RANGE.id), b.range.as_ptr(), group.clone()));
            map.push((band_id(i, THRESH.id), b.thresh.as_ptr(), group.clone()));
            map.push((band_id(i, ATTACK.id), b.attack.as_ptr(), group.clone()));
            map.push((band_id(i, RELEASE.id), b.release.as_ptr(), group.clone()));
            map.push((band_id(i, RATIO.id), b.ratio.as_ptr(), group.clone()));
            map.push((band_id(i, DMODE.id), b.dmode.as_ptr(), group.clone()));
            map.push((band_id(i, SUSTAIN.id), b.sustain.as_ptr(), group.clone()));
            map.push((band_id(i, TRANSIENT.id), b.transient.as_ptr(), group.clone()));
            map.push((band_id(i, TTIME.id), b.ttime.as_ptr(), group.clone()));
            map.push((band_id(i, TRANSITION.id), b.transition.as_ptr(), group.clone()));
            map.push((band_id(i, SPECTRAL.id), b.spectral.as_ptr(), group.clone()));
            map.push((band_id(i, SSMOOTH.id), b.ssmooth.as_ptr(), group.clone()));
            map.push((band_id(i, SREL.id), b.srel.as_ptr(), group.clone()));
            map.push((band_id(i, RTHRESH.id), b.rthresh.as_ptr(), group.clone()));
            map.push((band_id(i, SOLO.id), b.solo.as_ptr(), group.clone()));
            map.push((band_id(i, DSRC.id), b.dsrc.as_ptr(), group));
        }
        map
    }
}
