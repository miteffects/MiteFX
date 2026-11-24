//! Mite Multiband parameters (docs/design/multiband-brief.md §3).
//!
//! IDs are frozen once released (`tests/golden/mite_multiband.params.txt`): globals by name, the
//! crossovers `x{K}_{param}`, the bands `b{N}_{param}`, the sidechain EQ `k{N}_{param}` (Gate's).
//! Crossover slot k owns band slot k + 1; band 1 is always the lowest region; slots never swap.

use mite_core::presets::CurrentPreset;
use mite_core::snapshots::Snapshots;
use mite_core::{BoolSpec, ChoiceSpec, DialCurve, ParamDisplay, ParamScale, ParamSpec};
use mite_dsp::dynamics::{Detector, Quadrant, Stage, RATIO_MAX};
use mite_dsp::eq::{BandSettings, BandType};
use mite_dsp::multiband::{MbBand, MbSettings, BANDS, CROSSOVERS, DEFAULT_CROSSOVERS, KEY_BANDS};
use mite_plugin::{bool_param, choice_param, float_param, float_param_named};
use nih_plug::prelude::*;

/// The state format version (`mite_plugin::state`).
///
/// - 2 (2026-11-04): auto gain (`auto_gain`, appended; off in older states, so they sound as
///   before: [`v1_to_v2`] has nothing to do).
pub const STATE_VERSION: u32 = 2;

/// State 1 → 2: auto gain is new. Absent from an older state, it reads as off.
pub fn v1_to_v2(_state: &mut nih_plug::wrapper::state::PluginState) {}

#[allow(clippy::too_many_arguments)] // one line per parameter reads best
const fn spec(id: &'static str, name: &'static str, unit: &'static str, min: f32, max: f32, default: f32, scale: ParamScale, decimals: u8, wheel: (f32, f32), display: ParamDisplay) -> ParamSpec {
    ParamSpec { id, name, unit, min, max, default, scale, smoothing_ms: 20.0, decimals, wheel_step: wheel.0, wheel_step_fine: wheel.1, display }
}

// ---- Globals ----------------------------------------------------------------------------------

/// Not smoothed: the latency follows its exact value.
pub const LOOKAHEAD: ParamSpec = ParamSpec { smoothing_ms: 0.0, ..spec("lookahead", "Lookahead", "ms", 0.0, 20.0, 0.0, ParamScale::Linear, 1, (0.5, 0.1), ParamDisplay::Time) };
pub const LINK: ParamSpec = spec("link", "Stereo Link", "%", 0.0, 100.0, 100.0, ParamScale::Linear, 0, (1.0, 0.1), ParamDisplay::Number);
pub const MIX: ParamSpec = spec("mix", "Mix", "%", 0.0, 100.0, 100.0, ParamScale::Linear, 0, (5.0, 1.0), ParamDisplay::Number);
pub const OUT_GAIN: ParamSpec = spec("out_gain", "Output", "dB", -24.0, 24.0, 0.0, ParamScale::Linear, 1, (0.5, 0.1), ParamDisplay::Number);
pub const DELTA: BoolSpec = BoolSpec { id: "delta", name: "Delta", default: false };
pub const BYPASS: BoolSpec = BoolSpec { id: "bypass", name: "Bypass", default: false };
pub const SC_SOURCE: ChoiceSpec = ChoiceSpec { id: "sc_source", name: "Sidechain", choices: &["Internal", "External"], default: 0 };
/// Each band's loudness out matched to its loudness in (Comp's auto makeup per band). Appended
/// in state 2.
pub const AUTO_GAIN: BoolSpec = BoolSpec { id: "auto_gain", name: "Auto Gain", default: false };
/// Hear the key (slice 2; monitoring).
pub const LISTEN: BoolSpec = BoolSpec { id: "listen", name: "Listen", default: false };

// ---- Sidechain EQ (Gate's) ----------------------------------------------------------------------

pub const K_ON: BoolSpec = BoolSpec { id: "on", name: "On", default: false };
pub const K_TYPE: ChoiceSpec = ChoiceSpec { id: "type", name: "Type", choices: &BandType::NAMES, default: 0 };
pub const K_FREQ: ParamSpec = spec("freq", "Frequency", "Hz", 20.0, 20_000.0, 1000.0, ParamScale::Log, 0, (1.0 / 6.0, 1.0 / 48.0), ParamDisplay::Frequency);
pub const K_GAIN: ParamSpec = spec("gain", "Gain", "dB", -24.0, 24.0, 0.0, ParamScale::Linear, 1, (0.5, 0.1), ParamDisplay::Number);
pub const K_Q: ParamSpec = spec("q", "Q", "", 0.1, 10.0, std::f32::consts::FRAC_1_SQRT_2, ParamScale::Log, 2, (1.0 / 6.0, 1.0 / 24.0), ParamDisplay::Number);
pub const K_SLOPE: ChoiceSpec = ChoiceSpec { id: "slope", name: "Slope", choices: &["6 dB/oct", "12 dB/oct", "18 dB/oct", "24 dB/oct", "30 dB/oct", "36 dB/oct", "48 dB/oct", "72 dB/oct", "96 dB/oct"], default: mite_dsp::eq::DEFAULT_SLOPE };
const KEY_DEFAULTS: [(usize, f32); KEY_BANDS] = [(3, 100.0), (0, 1000.0), (2, 6000.0), (1, 250.0), (0, 3000.0), (4, 12000.0)];
static KEY_TYPES: [ChoiceSpec; KEY_BANDS] = [
    ChoiceSpec { default: KEY_DEFAULTS[0].0, ..K_TYPE },
    ChoiceSpec { default: KEY_DEFAULTS[1].0, ..K_TYPE },
    ChoiceSpec { default: KEY_DEFAULTS[2].0, ..K_TYPE },
    ChoiceSpec { default: KEY_DEFAULTS[3].0, ..K_TYPE },
    ChoiceSpec { default: KEY_DEFAULTS[4].0, ..K_TYPE },
    ChoiceSpec { default: KEY_DEFAULTS[5].0, ..K_TYPE },
];
static KEY_FREQS: [ParamSpec; KEY_BANDS] = [
    ParamSpec { default: KEY_DEFAULTS[0].1, ..K_FREQ },
    ParamSpec { default: KEY_DEFAULTS[1].1, ..K_FREQ },
    ParamSpec { default: KEY_DEFAULTS[2].1, ..K_FREQ },
    ParamSpec { default: KEY_DEFAULTS[3].1, ..K_FREQ },
    ParamSpec { default: KEY_DEFAULTS[4].1, ..K_FREQ },
    ParamSpec { default: KEY_DEFAULTS[5].1, ..K_FREQ },
];

// ---- Crossovers -----------------------------------------------------------------------------------

pub const X_ON: BoolSpec = BoolSpec { id: "on", name: "On", default: false };
pub const X_FREQ: ParamSpec = spec("freq", "Frequency", "Hz", 20.0, 20_000.0, 1000.0, ParamScale::Log, 0, (1.0 / 6.0, 1.0 / 48.0), ParamDisplay::Frequency);
static X_FREQS: [ParamSpec; CROSSOVERS] = [
    ParamSpec { default: DEFAULT_CROSSOVERS[0], ..X_FREQ },
    ParamSpec { default: DEFAULT_CROSSOVERS[1], ..X_FREQ },
    ParamSpec { default: DEFAULT_CROSSOVERS[2], ..X_FREQ },
    ParamSpec { default: DEFAULT_CROSSOVERS[3], ..X_FREQ },
    ParamSpec { default: DEFAULT_CROSSOVERS[4], ..X_FREQ },
];

// ---- Bands --------------------------------------------------------------------------------------

/// A fresh band compresses gently, as a fresh Comp does.
pub const B_ON: BoolSpec = BoolSpec { id: "on", name: "Dynamics", default: true };
pub const B_QUAD: ChoiceSpec = ChoiceSpec { id: "quad", name: "Quadrant", choices: &Quadrant::NAMES, default: 0 };
pub const B_THRESH: ParamSpec = spec("thresh", "Threshold", "dB", -60.0, 0.0, -20.0, ParamScale::Linear, 1, (0.5, 0.1), ParamDisplay::Number);
pub const B_RATIO: ParamSpec = spec("ratio", "Ratio", ":1", 1.0, RATIO_MAX, 2.0, ParamScale::Log, 1, (1.0 / 6.0, 1.0 / 24.0), ParamDisplay::Ratio);
pub const B_KNEE: ParamSpec = spec("knee", "Knee", "dB", 0.0, 24.0, 6.0, ParamScale::Linear, 1, (0.5, 0.1), ParamDisplay::Number);
pub const B_RANGE: ParamSpec = spec("range", "Range", "dB", 0.0, 60.0, 24.0, ParamScale::Linear, 0, (1.0, 0.1), ParamDisplay::Number);
pub const B_ATTACK: ParamSpec = spec("attack", "Attack", "ms", 0.05, 250.0, 10.0, ParamScale::Log, 1, (0.25, 1.0 / 16.0), ParamDisplay::Time);
pub const B_RELEASE: ParamSpec = spec("release", "Release", "ms", 5.0, 2500.0, 120.0, ParamScale::Log, 0, (0.25, 1.0 / 16.0), ParamDisplay::Time);
pub const B_DET: ChoiceSpec = ChoiceSpec { id: "det", name: "Detector", choices: &Detector::NAMES, default: 1 };
pub const B_GAIN: ParamSpec = spec("gain", "Gain", "dB", -24.0, 24.0, 0.0, ParamScale::Linear, 1, (0.5, 0.1), ParamDisplay::Number);
pub const B_BYPASS: BoolSpec = BoolSpec { id: "bypass", name: "Bypass", default: false };
pub const B_MUTE: BoolSpec = BoolSpec { id: "mute", name: "Mute", default: false };
/// Monitoring.
pub const B_SOLO: BoolSpec = BoolSpec { id: "solo", name: "Solo", default: false };
/// Monitoring (slice 2): hear only what this band's dynamics change.
pub const B_DELTA: BoolSpec = BoolSpec { id: "delta", name: "Band Delta", default: false };
/// Detect on the external sidechain (slice 2).
pub const B_EXTERNAL: BoolSpec = BoolSpec { id: "external", name: "External Key", default: false };

pub const ATTACK_DIAL: DialCurve = DialCurve(&[(0.0, 0.05), (0.25, 1.0), (0.5, 10.0), (0.8, 50.0), (1.0, 250.0)]);
pub const RELEASE_DIAL: DialCurve = DialCurve(&[(0.0, 5.0), (0.2, 40.0), (0.45, 100.0), (0.8, 400.0), (1.0, 2500.0)]);

pub fn key_id(band: usize, param: &str) -> mite_core::ParamId {
    mite_core::ParamId::indexed("k", band + 1, 0, param)
}

pub fn cross_id(k: usize, param: &str) -> mite_core::ParamId {
    mite_core::ParamId::indexed("x", k + 1, 0, param)
}

pub fn band_id(b: usize, param: &str) -> mite_core::ParamId {
    mite_core::ParamId::indexed("b", b + 1, 0, param)
}

/// Every parameter's manifest line, in host order.
pub fn manifest() -> String {
    let mut lines = vec![LOOKAHEAD.manifest_line(), LINK.manifest_line(), MIX.manifest_line(), OUT_GAIN.manifest_line(), DELTA.manifest_line(), BYPASS.manifest_line(), SC_SOURCE.manifest_line(), LISTEN.manifest_line()];
    for k in 0..KEY_BANDS {
        let prefix = |line: String| format!("k{}_{line}", k + 1);
        lines.push(prefix(K_ON.manifest_line()));
        lines.push(prefix(KEY_TYPES[k].manifest_line()));
        lines.push(prefix(KEY_FREQS[k].manifest_line()));
        lines.push(prefix(K_GAIN.manifest_line()));
        lines.push(prefix(K_Q.manifest_line()));
        lines.push(prefix(K_SLOPE.manifest_line()));
    }
    for (k, freq) in X_FREQS.iter().enumerate() {
        lines.push(format!("x{}_{}", k + 1, X_ON.manifest_line()));
        lines.push(format!("x{}_{}", k + 1, freq.manifest_line()));
    }
    for b in 0..BANDS {
        let prefix = |line: String| format!("b{}_{line}", b + 1);
        lines.push(prefix(B_ON.manifest_line()));
        lines.push(prefix(B_QUAD.manifest_line()));
        for s in [B_THRESH, B_RATIO, B_KNEE, B_RANGE, B_ATTACK, B_RELEASE] {
            lines.push(prefix(s.manifest_line()));
        }
        lines.push(prefix(B_DET.manifest_line()));
        lines.push(prefix(B_GAIN.manifest_line()));
        for s in [B_BYPASS, B_MUTE, B_SOLO, B_DELTA, B_EXTERNAL] {
            lines.push(prefix(s.manifest_line()));
        }
    }
    // Appended in state 2.
    lines.push(AUTO_GAIN.manifest_line());
    lines.join("\n") + "\n"
}

pub fn ids() -> Vec<String> {
    manifest().lines().filter_map(|l| l.split(" | ").next()).map(str::to_owned).collect()
}

/// Monitoring, not sound: never in presets or snapshots.
pub fn is_monitoring(id: &str) -> bool {
    [DELTA.id, BYPASS.id, LISTEN.id].contains(&id) || id.ends_with("_solo") || (id.starts_with('b') && id.ends_with("_delta"))
}

pub fn sound_ids() -> Vec<String> {
    ids().into_iter().filter(|id| !is_monitoring(id)).collect()
}

/// Every continuous spec by its id, for stand-in hosts.
pub fn spec_for(id: &str) -> Option<ParamSpec> {
    if let Some(s) = [LOOKAHEAD, LINK, MIX, OUT_GAIN].into_iter().find(|s| s.id == id) {
        return Some(s);
    }
    let (slot, short) = id.split_once('_')?;
    let n: usize = slot[1..].parse().ok()?;
    let i = n.checked_sub(1)?;
    match (&slot[..1], short) {
        ("k", "freq") => Some(KEY_FREQS[i.min(KEY_BANDS - 1)]),
        ("k", "gain") => Some(K_GAIN),
        ("k", "q") => Some(K_Q),
        ("x", "freq") => Some(X_FREQS[i.min(CROSSOVERS - 1)]),
        ("b", _) => [B_THRESH, B_RATIO, B_KNEE, B_RANGE, B_ATTACK, B_RELEASE, B_GAIN].into_iter().find(|s| s.id == short),
        _ => None,
    }
}

pub struct KeyParams {
    pub on: BoolParam,
    pub kind: IntParam,
    pub freq: FloatParam,
    pub gain: FloatParam,
    pub q: FloatParam,
    pub slope: IntParam,
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
            slope: choice_param(&K_SLOPE, name("Slope")),
        }
    }

    pub fn band(&self) -> BandSettings {
        BandSettings {
            enabled: self.on.value(),
            kind: BandType::from_index(self.kind.value().max(0) as usize),
            freq: f64::from(self.freq.value()),
            gain_db: f64::from(self.gain.value()),
            q: f64::from(self.q.value()),
            slope: self.slope.value().max(0) as usize,
            ..BandSettings::default()
        }
    }
}

pub struct CrossParams {
    pub on: BoolParam,
    pub freq: FloatParam,
}

pub struct BandParams {
    pub on: BoolParam,
    pub quad: IntParam,
    pub thresh: FloatParam,
    pub ratio: FloatParam,
    pub knee: FloatParam,
    pub range: FloatParam,
    pub attack: FloatParam,
    pub release: FloatParam,
    pub det: IntParam,
    pub gain: FloatParam,
    pub bypass: BoolParam,
    pub mute: BoolParam,
    pub solo: BoolParam,
    pub delta: BoolParam,
    pub external: BoolParam,
}

impl BandParams {
    fn new(b: usize) -> Self {
        let name = |what: &str| format!("Band {} {what}", b + 1);
        Self {
            on: bool_param(&B_ON, name("Dynamics")),
            quad: choice_param(&B_QUAD, name("Quadrant")),
            thresh: float_param_named(&B_THRESH, name("Threshold")),
            ratio: float_param_named(&B_RATIO, name("Ratio")),
            knee: float_param_named(&B_KNEE, name("Knee")),
            range: float_param_named(&B_RANGE, name("Range")),
            attack: float_param_named(&B_ATTACK, name("Attack")),
            release: float_param_named(&B_RELEASE, name("Release")),
            det: choice_param(&B_DET, name("Detector")),
            gain: float_param_named(&B_GAIN, name("Gain")),
            bypass: bool_param(&B_BYPASS, name("Bypass")),
            mute: bool_param(&B_MUTE, name("Mute")),
            solo: bool_param(&B_SOLO, name("Solo")),
            delta: bool_param(&B_DELTA, name("Delta")),
            external: bool_param(&B_EXTERNAL, name("External Key")),
        }
    }

    pub fn band(&self) -> MbBand {
        MbBand {
            stage: Stage {
                on: self.on.value(),
                quadrant: Quadrant::from_index(self.quad.value().max(0) as usize),
                threshold_db: self.thresh.value(),
                ratio: self.ratio.value(),
                knee_db: self.knee.value(),
                range_db: self.range.value(),
                attack_ms: self.attack.value(),
                release_ms: self.release.value(),
                auto_release: false,
                detector: Detector::from_index(self.det.value().max(0) as usize),
            },
            gain_db: self.gain.value(),
            bypass: self.bypass.value(),
            mute: self.mute.value(),
            solo: self.solo.value(),
            external: self.external.value(),
            delta: self.delta.value(),
        }
    }
}

pub struct MbParams {
    pub lookahead: FloatParam,
    pub link: FloatParam,
    pub mix: FloatParam,
    pub out_gain: FloatParam,
    pub delta: BoolParam,
    pub bypass: BoolParam,
    pub sc_source: IntParam,
    pub listen: BoolParam,
    pub auto_gain: BoolParam,
    pub keys: [KeyParams; KEY_BANDS],
    pub cross: [CrossParams; CROSSOVERS],
    pub bands: [BandParams; BANDS],
    pub snapshots: std::sync::Arc<Snapshots>,
    pub preset: std::sync::Arc<CurrentPreset>,
}

impl Default for MbParams {
    fn default() -> Self {
        let flag = |spec: &'static BoolSpec| bool_param(spec, spec.name.to_owned());
        Self {
            lookahead: float_param(&LOOKAHEAD),
            link: float_param(&LINK),
            mix: float_param(&MIX),
            out_gain: float_param(&OUT_GAIN),
            delta: flag(&DELTA),
            // An ordinary parameter, never the host's bypass (REAPER stops processing).
            bypass: flag(&BYPASS),
            sc_source: choice_param(&SC_SOURCE, SC_SOURCE.name.to_owned()),
            listen: flag(&LISTEN),
            auto_gain: flag(&AUTO_GAIN),
            keys: std::array::from_fn(KeyParams::new),
            cross: std::array::from_fn(|k| CrossParams { on: bool_param(&X_ON, format!("Crossover {} On", k + 1)), freq: float_param_named(&X_FREQS[k], format!("Crossover {} Frequency", k + 1)) }),
            bands: std::array::from_fn(BandParams::new),
            snapshots: std::sync::Arc::new(Snapshots::new()),
            preset: std::sync::Arc::new(CurrentPreset::default()),
        }
    }
}

impl MbParams {
    pub fn settings(&self) -> MbSettings {
        let external = self.sc_source.value() == 1;
        MbSettings {
            crossovers: std::array::from_fn(|k| self.cross[k].on.value().then(|| self.cross[k].freq.value())),
            bands: std::array::from_fn(|b| {
                let mut band = self.bands[b].band();
                band.external &= external;
                band
            }),
            lookahead_ms: self.lookahead.value(),
            link: self.link.value() / 100.0,
            key_eq: std::array::from_fn(|i| self.keys[i].band()),
            mix: self.mix.value() / 100.0,
            out_db: self.out_gain.value(),
            delta: self.delta.value(),
            bypass: self.bypass.value(),
            listen: external && self.listen.value(),
            auto_gain: self.auto_gain.value(),
        }
    }
}

// SAFETY: every `ParamPtr` points into `self`, which nih-plug keeps alive (in an `Arc`) for as
// long as it uses the map. The ids match `manifest()` (checked by tests/params.rs).
unsafe impl Params for MbParams {
    fn serialize_fields(&self) -> std::collections::BTreeMap<String, String> {
        let live = mite_plugin::state::live_values(self, &sound_ids());
        mite_plugin::state::session_fields(STATE_VERSION, &self.snapshots, &self.preset, live)
    }

    fn deserialize_fields(&self, serialized: &std::collections::BTreeMap<String, String>) {
        mite_plugin::state::load_session_fields(serialized, &self.snapshots, &self.preset);
    }

    fn param_map(&self) -> Vec<(String, ParamPtr, String)> {
        let p = |id: String, ptr: ParamPtr, group: &str| (id, ptr, group.to_owned());
        let mut map = vec![
            p(LOOKAHEAD.id.into(), self.lookahead.as_ptr(), ""),
            p(LINK.id.into(), self.link.as_ptr(), ""),
            p(MIX.id.into(), self.mix.as_ptr(), ""),
            p(OUT_GAIN.id.into(), self.out_gain.as_ptr(), ""),
            p(DELTA.id.into(), self.delta.as_ptr(), ""),
            p(BYPASS.id.into(), self.bypass.as_ptr(), ""),
            p(SC_SOURCE.id.into(), self.sc_source.as_ptr(), "Sidechain"),
            p(LISTEN.id.into(), self.listen.as_ptr(), "Sidechain"),
        ];
        for (i, k) in self.keys.iter().enumerate() {
            for (id, ptr) in [(K_ON.id, k.on.as_ptr()), (K_TYPE.id, k.kind.as_ptr()), (K_FREQ.id, k.freq.as_ptr()), (K_GAIN.id, k.gain.as_ptr()), (K_Q.id, k.q.as_ptr()), (K_SLOPE.id, k.slope.as_ptr())] {
                map.push(p(String::from(key_id(i, id)), ptr, "Sidechain"));
            }
        }
        for (k, x) in self.cross.iter().enumerate() {
            map.push(p(String::from(cross_id(k, X_ON.id)), x.on.as_ptr(), "Crossovers"));
            map.push(p(String::from(cross_id(k, X_FREQ.id)), x.freq.as_ptr(), "Crossovers"));
        }
        for (b, s) in self.bands.iter().enumerate() {
            let g = format!("Band {}", b + 1);
            for (id, ptr) in [
                (B_ON.id, s.on.as_ptr()),
                (B_QUAD.id, s.quad.as_ptr()),
                (B_THRESH.id, s.thresh.as_ptr()),
                (B_RATIO.id, s.ratio.as_ptr()),
                (B_KNEE.id, s.knee.as_ptr()),
                (B_RANGE.id, s.range.as_ptr()),
                (B_ATTACK.id, s.attack.as_ptr()),
                (B_RELEASE.id, s.release.as_ptr()),
                (B_DET.id, s.det.as_ptr()),
                (B_GAIN.id, s.gain.as_ptr()),
                (B_BYPASS.id, s.bypass.as_ptr()),
                (B_MUTE.id, s.mute.as_ptr()),
                (B_SOLO.id, s.solo.as_ptr()),
                (B_DELTA.id, s.delta.as_ptr()),
                (B_EXTERNAL.id, s.external.as_ptr()),
            ] {
                map.push(p(String::from(band_id(b, id)), ptr, &g));
            }
        }
        map.push(p(AUTO_GAIN.id.into(), self.auto_gain.as_ptr(), ""));
        map
    }
}
