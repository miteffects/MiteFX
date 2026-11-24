//! Mite Gate parameters (docs/design/gate-brief.md §4).
//!
//! IDs are frozen once released (`tests/golden/mite_gate.params.txt`). Every parameter of the
//! baseline exists from slice 1; the key bands are Comp's (`k{N}_{param}`, the same ranges).

use mite_core::presets::CurrentPreset;
use mite_core::snapshots::Snapshots;
use mite_core::{BoolSpec, ChoiceSpec, DialCurve, ParamDisplay, ParamScale, ParamSpec};
use mite_dsp::eq::{BandSettings, BandType};
use mite_dsp::gate::{Detector, GateSettings, Mode, KEY_BANDS};
use mite_plugin::{bool_param, choice_param, float_param, float_param_named};
use nih_plug::prelude::*;

use crate::dsp::Output;

/// The state format version (`mite_plugin::state`).
///
/// - 2 (2026-11-03): three more key bands and every band's slope (`k1…k3_slope`, `k4…k6_*`),
///   appended; a version 1 state has its three bands at the default slope (24 dB/oct, what they
///   played) and the new ones off, so it sounds as before: nothing to migrate.
pub const STATE_VERSION: u32 = 2;
/// The bands version 1 had.
pub const V1_KEY_BANDS: usize = 3;

#[allow(clippy::too_many_arguments)] // one line per parameter reads best
const fn spec(id: &'static str, name: &'static str, unit: &'static str, min: f32, max: f32, default: f32, scale: ParamScale, decimals: u8, wheel: (f32, f32), display: ParamDisplay) -> ParamSpec {
    ParamSpec { id, name, unit, min, max, default, scale, smoothing_ms: 20.0, decimals, wheel_step: wheel.0, wheel_step_fine: wheel.1, display }
}

// ---- The gate -------------------------------------------------------------------------------

pub const MODE: ChoiceSpec = ChoiceSpec { id: "mode", name: "Mode", choices: &Mode::NAMES, default: 0 };
pub const THRESHOLD: ParamSpec = spec("threshold", "Threshold", "dB", -80.0, 0.0, -40.0, ParamScale::Linear, 1, (0.5, 0.1), ParamDisplay::Number);
pub const RANGE: ParamSpec = spec("range", "Range", "dB", -80.0, 0.0, -80.0, ParamScale::Linear, 1, (1.0, 0.1), ParamDisplay::Number);
pub const RATIO: ParamSpec = spec("ratio", "Ratio", ":1", 1.0, 10.0, 4.0, ParamScale::Log, 1, (1.0 / 6.0, 1.0 / 24.0), ParamDisplay::Ratio);
pub const HYSTERESIS: ParamSpec = spec("hysteresis", "Hysteresis", "dB", 0.0, 24.0, 4.0, ParamScale::Linear, 1, (0.5, 0.1), ParamDisplay::Number);
pub const ATTACK: ParamSpec = spec("attack", "Attack", "ms", 0.01, 100.0, 0.5, ParamScale::Log, 2, (0.25, 1.0 / 16.0), ParamDisplay::Time);
pub const HOLD: ParamSpec = spec("hold", "Hold", "ms", 0.0, 1000.0, 30.0, ParamScale::Linear, 0, (5.0, 1.0), ParamDisplay::Time);
pub const RELEASE: ParamSpec = spec("release", "Release", "ms", 1.0, 2500.0, 100.0, ParamScale::Log, 0, (0.25, 1.0 / 16.0), ParamDisplay::Time);
/// Not smoothed: the latency follows its exact value.
pub const LOOKAHEAD: ParamSpec = ParamSpec { smoothing_ms: 0.0, ..spec("lookahead", "Lookahead", "ms", 0.0, 10.0, 0.0, ParamScale::Linear, 2, (0.25, 0.05), ParamDisplay::Time) };
pub const DETECTOR: ChoiceSpec = ChoiceSpec { id: "detector", name: "Detector", choices: &Detector::NAMES, default: 0 };
pub const LINK: ParamSpec = spec("link", "Stereo Link", "%", 0.0, 100.0, 100.0, ParamScale::Linear, 0, (1.0, 0.1), ParamDisplay::Number);

// ---- Sidechain (Comp's) -----------------------------------------------------------------------

pub const SC_SOURCE: ChoiceSpec = ChoiceSpec { id: "sc_source", name: "Sidechain", choices: &["Internal", "External"], default: 0 };
/// Hear the key as the detector does (monitoring).
pub const LISTEN: BoolSpec = BoolSpec { id: "listen", name: "Listen", default: false };
pub const K_ON: BoolSpec = BoolSpec { id: "on", name: "On", default: false };
pub const K_TYPE: ChoiceSpec = ChoiceSpec { id: "type", name: "Type", choices: &BandType::NAMES, default: 0 };
pub const K_FREQ: ParamSpec = spec("freq", "Frequency", "Hz", 20.0, 20_000.0, 1000.0, ParamScale::Log, 0, (1.0 / 6.0, 1.0 / 48.0), ParamDisplay::Frequency);
pub const K_GAIN: ParamSpec = spec("gain", "Gain", "dB", -24.0, 24.0, 0.0, ParamScale::Linear, 1, (0.5, 0.1), ParamDisplay::Number);
pub const K_Q: ParamSpec = spec("q", "Q", "", 0.1, 10.0, std::f32::consts::FRAC_1_SQRT_2, ParamScale::Log, 2, (1.0 / 6.0, 1.0 / 24.0), ParamDisplay::Number);
/// A cut's steepness: the EQ's IIR slopes (`mite_dsp::eq::SLOPES_DB`), 24 dB/oct by default.
pub const K_SLOPE: ChoiceSpec = ChoiceSpec { id: "slope", name: "Slope", choices: &["6 dB/oct", "12 dB/oct", "18 dB/oct", "24 dB/oct", "30 dB/oct", "36 dB/oct", "48 dB/oct", "72 dB/oct", "96 dB/oct"], default: mite_dsp::eq::DEFAULT_SLOPE };
/// The key bands' first types and frequencies: a low cut, a bell, a high shelf (as Comp's), then
/// a low shelf, a bell and a high cut.
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

// ---- Output -------------------------------------------------------------------------------------

pub const MIX: ParamSpec = spec("mix", "Mix", "%", 0.0, 100.0, 100.0, ParamScale::Linear, 0, (5.0, 1.0), ParamDisplay::Number);
pub const OUT_GAIN: ParamSpec = spec("out_gain", "Output", "dB", -24.0, 24.0, 0.0, ParamScale::Linear, 1, (0.5, 0.1), ParamDisplay::Number);
pub const DELTA: BoolSpec = BoolSpec { id: "delta", name: "Delta", default: false };
pub const BYPASS: BoolSpec = BoolSpec { id: "bypass", name: "Bypass", default: false };

// ---- MIDI trigger (slice 3) ----------------------------------------------------------------------

pub const MIDI_ON: BoolSpec = BoolSpec { id: "midi_on", name: "MIDI Trigger", default: false };
pub const MIDI_NOTE: ParamSpec = spec("midi_note", "MIDI Note", "", 0.0, 127.0, 36.0, ParamScale::Linear, 0, (1.0, 1.0), ParamDisplay::Number);
pub const MIDI_CHANNEL: ParamSpec = spec("midi_channel", "MIDI Channel", "", 1.0, 16.0, 1.0, ParamScale::Linear, 0, (1.0, 1.0), ParamDisplay::Number);
pub const MIDI_VELOCITY: ChoiceSpec = ChoiceSpec { id: "midi_velocity", name: "MIDI Velocity", choices: &["Fixed", "Level"], default: 0 };
pub const MIDI_VEL_FIXED: ParamSpec = spec("midi_vel_fixed", "Fixed Velocity", "", 1.0, 127.0, 100.0, ParamScale::Linear, 0, (1.0, 1.0), ParamDisplay::Number);

/// Dials: common values get more travel.
pub const ATTACK_DIAL: DialCurve = DialCurve(&[(0.0, 0.01), (0.3, 0.5), (0.6, 5.0), (1.0, 100.0)]);
pub const HOLD_DIAL: DialCurve = DialCurve(&[(0.0, 0.0), (0.4, 30.0), (0.75, 200.0), (1.0, 1000.0)]);
pub const RELEASE_DIAL: DialCurve = DialCurve(&[(0.0, 1.0), (0.2, 20.0), (0.45, 100.0), (0.8, 600.0), (1.0, 2500.0)]);

/// A key band parameter's host id, e.g. `k1_freq` (`band` from 0).
pub fn key_id(band: usize, param: &str) -> mite_core::ParamId {
    mite_core::ParamId::indexed("k", band + 1, 0, param)
}

/// Every parameter's manifest line, in host order.
pub fn manifest() -> String {
    let mut lines = vec![MODE.manifest_line()];
    lines.extend([THRESHOLD, RANGE, RATIO, HYSTERESIS, ATTACK, HOLD, RELEASE, LOOKAHEAD].iter().map(ParamSpec::manifest_line));
    lines.push(DETECTOR.manifest_line());
    lines.push(LINK.manifest_line());
    lines.push(SC_SOURCE.manifest_line());
    lines.push(LISTEN.manifest_line());
    for k in 0..V1_KEY_BANDS {
        let prefix = |line: String| format!("k{}_{line}", k + 1);
        lines.push(prefix(K_ON.manifest_line()));
        lines.push(prefix(KEY_TYPES[k].manifest_line()));
        lines.push(prefix(KEY_FREQS[k].manifest_line()));
        lines.push(prefix(K_GAIN.manifest_line()));
        lines.push(prefix(K_Q.manifest_line()));
    }
    lines.extend([MIX, OUT_GAIN].iter().map(ParamSpec::manifest_line));
    lines.push(DELTA.manifest_line());
    lines.push(BYPASS.manifest_line());
    lines.push(MIDI_ON.manifest_line());
    lines.extend([MIDI_NOTE, MIDI_CHANNEL].iter().map(ParamSpec::manifest_line));
    lines.push(MIDI_VELOCITY.manifest_line());
    lines.push(MIDI_VEL_FIXED.manifest_line());
    // State 2: every band's slope, then the new bands.
    for k in 0..V1_KEY_BANDS {
        lines.push(format!("k{}_{}", k + 1, K_SLOPE.manifest_line()));
    }
    for k in V1_KEY_BANDS..KEY_BANDS {
        let prefix = |line: String| format!("k{}_{line}", k + 1);
        lines.push(prefix(K_ON.manifest_line()));
        lines.push(prefix(KEY_TYPES[k].manifest_line()));
        lines.push(prefix(KEY_FREQS[k].manifest_line()));
        lines.push(prefix(K_GAIN.manifest_line()));
        lines.push(prefix(K_Q.manifest_line()));
        lines.push(prefix(K_SLOPE.manifest_line()));
    }
    lines.join("\n") + "\n"
}

/// Every parameter id, in host order.
pub fn ids() -> Vec<String> {
    manifest().lines().filter_map(|l| l.split(" | ").next()).map(str::to_owned).collect()
}

/// Monitoring, not sound: never in presets or snapshots.
pub fn is_monitoring(id: &str) -> bool {
    [LISTEN.id, DELTA.id, BYPASS.id].contains(&id)
}

pub fn sound_ids() -> Vec<String> {
    ids().into_iter().filter(|id| !is_monitoring(id)).collect()
}

/// Every continuous spec by its id (key bands by the part after the slot), for stand-in hosts.
pub fn spec_for(id: &str) -> Option<ParamSpec> {
    let globals = [THRESHOLD, RANGE, RATIO, HYSTERESIS, ATTACK, HOLD, RELEASE, LOOKAHEAD, LINK, MIX, OUT_GAIN, MIDI_NOTE, MIDI_CHANNEL, MIDI_VEL_FIXED];
    if let Some(s) = globals.into_iter().find(|s| s.id == id) {
        return Some(s);
    }
    let (slot, short) = id.split_once('_')?;
    let band: usize = slot.strip_prefix('k')?.parse().ok()?;
    let b = band.checked_sub(1)?;
    match short {
        "freq" => Some(KEY_FREQS[b.min(KEY_BANDS - 1)]),
        "gain" => Some(K_GAIN),
        "q" => Some(K_Q),
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

pub struct GateParams {
    pub mode: IntParam,
    pub threshold: FloatParam,
    pub range: FloatParam,
    pub ratio: FloatParam,
    pub hysteresis: FloatParam,
    pub attack: FloatParam,
    pub hold: FloatParam,
    pub release: FloatParam,
    pub lookahead: FloatParam,
    pub detector: IntParam,
    pub link: FloatParam,
    pub sc_source: IntParam,
    pub listen: BoolParam,
    pub keys: [KeyParams; KEY_BANDS],
    pub mix: FloatParam,
    pub out_gain: FloatParam,
    pub delta: BoolParam,
    pub bypass: BoolParam,
    pub midi_on: BoolParam,
    pub midi_note: FloatParam,
    pub midi_channel: FloatParam,
    pub midi_velocity: IntParam,
    pub midi_vel_fixed: FloatParam,
    pub snapshots: std::sync::Arc<Snapshots>,
    pub preset: std::sync::Arc<CurrentPreset>,
    /// The history's level range picked by hand (0: Auto), saved with the state.
    pub display_range: std::sync::Arc<mite_core::AtomicF32>,
}

impl Default for GateParams {
    fn default() -> Self {
        let named = |spec: &'static ChoiceSpec| choice_param(spec, spec.name.to_owned());
        let flag = |spec: &'static BoolSpec| bool_param(spec, spec.name.to_owned());
        Self {
            mode: named(&MODE),
            threshold: float_param(&THRESHOLD),
            range: float_param(&RANGE),
            ratio: float_param(&RATIO),
            hysteresis: float_param(&HYSTERESIS),
            attack: float_param(&ATTACK),
            hold: float_param(&HOLD),
            release: float_param(&RELEASE),
            lookahead: float_param(&LOOKAHEAD),
            detector: named(&DETECTOR),
            link: float_param(&LINK),
            sc_source: named(&SC_SOURCE),
            listen: flag(&LISTEN),
            keys: std::array::from_fn(KeyParams::new),
            mix: float_param(&MIX),
            out_gain: float_param(&OUT_GAIN),
            delta: flag(&DELTA),
            // An ordinary parameter, never the host's bypass (REAPER stops processing a
            // host-bypassed plugin, and then the editor could not turn it off).
            bypass: flag(&BYPASS),
            midi_on: flag(&MIDI_ON),
            midi_note: float_param(&MIDI_NOTE),
            midi_channel: float_param(&MIDI_CHANNEL),
            midi_velocity: named(&MIDI_VELOCITY),
            midi_vel_fixed: float_param(&MIDI_VEL_FIXED),
            snapshots: std::sync::Arc::new(Snapshots::new()),
            preset: std::sync::Arc::new(CurrentPreset::default()),
            display_range: std::sync::Arc::new(mite_core::AtomicF32::new(0.0)),
        }
    }
}

impl GateParams {
    pub fn settings(&self) -> GateSettings {
        GateSettings {
            mode: Mode::from_index(self.mode.value().max(0) as usize),
            threshold_db: self.threshold.value(),
            range_db: self.range.value(),
            ratio: self.ratio.value(),
            hysteresis_db: self.hysteresis.value(),
            attack_ms: self.attack.value(),
            hold_ms: self.hold.value(),
            release_ms: self.release.value(),
            lookahead_ms: self.lookahead.value(),
            detector: Detector::from_index(self.detector.value().max(0) as usize),
            link: self.link.value() / 100.0,
            external_key: self.sc_source.value() == 1,
            key_bands: std::array::from_fn(|i| self.keys[i].band()),
            listen: self.listen.value(),
        }
    }

    pub fn midi(&self) -> crate::dsp::MidiSettings {
        crate::dsp::MidiSettings {
            on: self.midi_on.value(),
            note: self.midi_note.value().round().clamp(0.0, 127.0) as u8,
            channel: (self.midi_channel.value().round().clamp(1.0, 16.0) - 1.0) as u8,
            from_level: self.midi_velocity.value() == 1,
            fixed: self.midi_vel_fixed.value().round().clamp(1.0, 127.0) as u8,
        }
    }

    pub fn output(&self) -> Output {
        Output { mix: self.mix.value() / 100.0, gain_db: self.out_gain.value(), delta: self.delta.value(), bypass: self.bypass.value() }
    }
}

// SAFETY: every `ParamPtr` points into `self`, which nih-plug keeps alive (in an `Arc`) for as
// long as it uses the map. The ids match `manifest()` (checked by tests/params.rs).
unsafe impl Params for GateParams {
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
        let p = |id: &str, ptr: ParamPtr, group: &str| (id.to_owned(), ptr, group.to_owned());
        let mut map = vec![
            p(MODE.id, self.mode.as_ptr(), "Gate"),
            p(THRESHOLD.id, self.threshold.as_ptr(), "Gate"),
            p(RANGE.id, self.range.as_ptr(), "Gate"),
            p(RATIO.id, self.ratio.as_ptr(), "Gate"),
            p(HYSTERESIS.id, self.hysteresis.as_ptr(), "Gate"),
            p(ATTACK.id, self.attack.as_ptr(), "Gate"),
            p(HOLD.id, self.hold.as_ptr(), "Gate"),
            p(RELEASE.id, self.release.as_ptr(), "Gate"),
            p(LOOKAHEAD.id, self.lookahead.as_ptr(), "Gate"),
            p(DETECTOR.id, self.detector.as_ptr(), "Gate"),
            p(LINK.id, self.link.as_ptr(), "Gate"),
            p(SC_SOURCE.id, self.sc_source.as_ptr(), "Sidechain"),
            p(LISTEN.id, self.listen.as_ptr(), "Sidechain"),
        ];
        for (i, k) in self.keys.iter().enumerate().take(V1_KEY_BANDS) {
            map.push((String::from(key_id(i, K_ON.id)), k.on.as_ptr(), "Sidechain".to_owned()));
            map.push((String::from(key_id(i, K_TYPE.id)), k.kind.as_ptr(), "Sidechain".to_owned()));
            map.push((String::from(key_id(i, K_FREQ.id)), k.freq.as_ptr(), "Sidechain".to_owned()));
            map.push((String::from(key_id(i, K_GAIN.id)), k.gain.as_ptr(), "Sidechain".to_owned()));
            map.push((String::from(key_id(i, K_Q.id)), k.q.as_ptr(), "Sidechain".to_owned()));
        }
        map.extend([
            p(MIX.id, self.mix.as_ptr(), ""),
            p(OUT_GAIN.id, self.out_gain.as_ptr(), ""),
            p(DELTA.id, self.delta.as_ptr(), ""),
            p(BYPASS.id, self.bypass.as_ptr(), ""),
            p(MIDI_ON.id, self.midi_on.as_ptr(), "MIDI"),
            p(MIDI_NOTE.id, self.midi_note.as_ptr(), "MIDI"),
            p(MIDI_CHANNEL.id, self.midi_channel.as_ptr(), "MIDI"),
            p(MIDI_VELOCITY.id, self.midi_velocity.as_ptr(), "MIDI"),
            p(MIDI_VEL_FIXED.id, self.midi_vel_fixed.as_ptr(), "MIDI"),
        ]);
        // State 2.
        for (i, k) in self.keys.iter().enumerate().take(V1_KEY_BANDS) {
            map.push((String::from(key_id(i, K_SLOPE.id)), k.slope.as_ptr(), "Sidechain".to_owned()));
        }
        for (i, k) in self.keys.iter().enumerate().skip(V1_KEY_BANDS) {
            for (id, ptr) in [(K_ON.id, k.on.as_ptr()), (K_TYPE.id, k.kind.as_ptr()), (K_FREQ.id, k.freq.as_ptr()), (K_GAIN.id, k.gain.as_ptr()), (K_Q.id, k.q.as_ptr()), (K_SLOPE.id, k.slope.as_ptr())] {
                map.push((String::from(key_id(i, id)), ptr, "Sidechain".to_owned()));
            }
        }
        map
    }
}
