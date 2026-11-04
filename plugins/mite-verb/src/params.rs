//! Mite Verb parameters (docs/design/verb-brief.md §3).
//!
//! IDs are frozen once released (`tests/golden/mite_verb.params.txt`).

use mite_core::presets::CurrentPreset;
use mite_core::snapshots::Snapshots;
use mite_core::{BoolSpec, ChoiceSpec, DialCurve, ParamDisplay, ParamScale, ParamSpec};
use mite_dsp::eq::{BandSettings, BandType, DEFAULT_SLOPE};
use mite_dsp::reverb::{DecayBand, DecayShape, ReverbSettings, DECAY_BANDS, TONE_BANDS};
use mite_plugin::{bool_param, choice_param, float_param, float_param_named};
use nih_plug::prelude::*;

use crate::dsp::Output;

/// The state format version (`mite_plugin::state`).
///
/// - 2 (2026-11-04): the early field's size and send, and the decay EQ's four bells
///   (`d1…d4_{on,freq,mult,q}`), appended; a version 1 state gets their defaults (bells off).
/// - 3 (2026-11-04): both EQs made free (user: "fully flexible eq like eq plugin … no default
///   bands"): the decay EQ is eight bands `d1…d8_{on,type,freq,mult,q}` (each a bell, a shelf or
///   a tilt), the tone EQ eight of Mite EQ's bands `e1…e8_{on,type,freq,gain,q,slope}`. The fixed
///   low and high decay shelves (`lo_mult`, `lo_freq`, `hi_mult`, `hi_freq`) and the cuts
///   (`lo_cut`, `hi_cut`) are gone: [`crate::migrate::v2_to_v3`] turns them into bands.
pub const STATE_VERSION: u32 = 3;

#[allow(clippy::too_many_arguments)] // one line per parameter reads best
const fn spec(id: &'static str, name: &'static str, unit: &'static str, min: f32, max: f32, default: f32, scale: ParamScale, decimals: u8, wheel: (f32, f32), display: ParamDisplay) -> ParamSpec {
    ParamSpec { id, name, unit, min, max, default, scale, smoothing_ms: 20.0, decimals, wheel_step: wheel.0, wheel_step_fine: wheel.1, display }
}

// ---- The space -------------------------------------------------------------------------------

pub const PREDELAY: ParamSpec = spec("predelay", "Pre-delay", "ms", 0.0, 500.0, 10.0, ParamScale::Linear, 0, (2.0, 0.5), ParamDisplay::Time);
pub const SIZE: ParamSpec = spec("size", "Size", "%", 0.0, 100.0, 50.0, ParamScale::Linear, 0, (2.0, 0.5), ParamDisplay::Number);
pub const DECAY: ParamSpec = spec("decay", "Decay", "s", 0.1, 30.0, 2.0, ParamScale::Log, 2, (1.0 / 24.0, 1.0 / 96.0), ParamDisplay::Number);
pub const DIFFUSION: ParamSpec = spec("diffusion", "Diffusion", "%", 0.0, 100.0, 70.0, ParamScale::Linear, 0, (2.0, 0.5), ParamDisplay::Number);
pub const EARLY: ParamSpec = spec("early", "Early Reflections", "%", 0.0, 100.0, 50.0, ParamScale::Linear, 0, (2.0, 0.5), ParamDisplay::Number);
pub const MOD_DEPTH: ParamSpec = spec("mod_depth", "Modulation", "%", 0.0, 100.0, 30.0, ParamScale::Linear, 0, (2.0, 0.5), ParamDisplay::Number);
pub const MOD_RATE: ParamSpec = spec("mod_rate", "Modulation Rate", "Hz", 0.05, 5.0, 0.6, ParamScale::Log, 2, (1.0 / 24.0, 1.0 / 96.0), ParamDisplay::Number);
pub const WIDTH: ParamSpec = spec("width", "Width", "%", 0.0, 150.0, 100.0, ParamScale::Linear, 0, (2.0, 0.5), ParamDisplay::Number);
pub const EARLY_SIZE: ParamSpec = spec("early_size", "Early Size", "ms", 2.0, 200.0, 35.0, ParamScale::Log, 0, (1.0 / 24.0, 1.0 / 96.0), ParamDisplay::Time);
pub const EARLY_SEND: ParamSpec = spec("early_send", "Early Send", "%", 0.0, 100.0, 0.0, ParamScale::Linear, 0, (2.0, 0.5), ParamDisplay::Number);

// ---- Freeze and ducking --------------------------------------------------------------------------

pub const FREEZE: BoolSpec = BoolSpec { id: "freeze", name: "Freeze", default: false };
pub const DUCK: ParamSpec = spec("duck", "Ducking", "dB", 0.0, 24.0, 0.0, ParamScale::Linear, 1, (0.5, 0.1), ParamDisplay::Number);
pub const DUCK_THRESH: ParamSpec = spec("duck_thresh", "Ducking Threshold", "dB", -60.0, 0.0, -30.0, ParamScale::Linear, 1, (0.5, 0.1), ParamDisplay::Number);
pub const DUCK_RELEASE: ParamSpec = spec("duck_release", "Ducking Release", "ms", 20.0, 2000.0, 300.0, ParamScale::Log, 0, (0.25, 1.0 / 16.0), ParamDisplay::Time);

// ---- Output -------------------------------------------------------------------------------------

pub const MIX: ParamSpec = spec("mix", "Mix", "%", 0.0, 100.0, 30.0, ParamScale::Linear, 0, (2.0, 0.5), ParamDisplay::Number);
pub const OUT_GAIN: ParamSpec = spec("out_gain", "Output", "dB", -24.0, 24.0, 0.0, ParamScale::Linear, 1, (0.5, 0.1), ParamDisplay::Number);
pub const BYPASS: BoolSpec = BoolSpec { id: "bypass", name: "Bypass", default: false };

// ---- The decay EQ: `d{N}_…` ---------------------------------------------------------------------

pub const D_ON: BoolSpec = BoolSpec { id: "on", name: "On", default: false };
pub const D_TYPE: ChoiceSpec = ChoiceSpec { id: "type", name: "Shape", choices: &DecayShape::NAMES, default: 0 };
pub const D_FREQ: ParamSpec = spec("freq", "Frequency", "Hz", 20.0, 20_000.0, 1000.0, ParamScale::Log, 0, (1.0 / 24.0, 1.0 / 96.0), ParamDisplay::Frequency);
pub const D_MULT: ParamSpec = spec("mult", "Decay", "×", 0.1, 4.0, 1.0, ParamScale::Log, 2, (1.0 / 24.0, 1.0 / 96.0), ParamDisplay::Number);
pub const D_Q: ParamSpec = spec("q", "Q", "", 0.3, 6.0, 1.0, ParamScale::Log, 2, (1.0 / 12.0, 1.0 / 48.0), ParamDisplay::Number);

// ---- The tone EQ: `e{N}_…` (Mite EQ's bands on the reverb) -------------------------------------

pub const E_ON: BoolSpec = BoolSpec { id: "on", name: "On", default: false };
pub const E_TYPE: ChoiceSpec = ChoiceSpec { id: "type", name: "Type", choices: &BandType::NAMES, default: 0 };
pub const E_FREQ: ParamSpec = spec("freq", "Frequency", "Hz", 20.0, 20_000.0, 1000.0, ParamScale::Log, 0, (1.0 / 6.0, 1.0 / 48.0), ParamDisplay::Frequency);
pub const E_GAIN: ParamSpec = spec("gain", "Gain", "dB", -24.0, 24.0, 0.0, ParamScale::Linear, 1, (0.5, 0.1), ParamDisplay::Number);
pub const E_Q: ParamSpec = spec("q", "Q", "", 0.1, 10.0, std::f32::consts::FRAC_1_SQRT_2, ParamScale::Log, 2, (1.0 / 6.0, 1.0 / 24.0), ParamDisplay::Number);
/// A cut's steepness: the EQ's IIR slopes (`mite_dsp::eq::SLOPES_DB`), 24 dB/oct by default.
pub const E_SLOPE: ChoiceSpec = ChoiceSpec { id: "slope", name: "Slope", choices: &["6 dB/oct", "12 dB/oct", "18 dB/oct", "24 dB/oct", "30 dB/oct", "36 dB/oct", "48 dB/oct", "72 dB/oct", "96 dB/oct"], default: DEFAULT_SLOPE };

/// A decay band parameter's id, e.g. `d2_mult` (`band` from 0).
pub fn band_id(band: usize, param: &str) -> String {
    format!("d{}_{param}", band + 1)
}

/// A tone band parameter's id, e.g. `e2_gain` (`band` from 0).
pub fn tone_id(band: usize, param: &str) -> String {
    format!("e{}_{param}", band + 1)
}

/// Dials: common values get more travel.
pub const PREDELAY_DIAL: DialCurve = DialCurve(&[(0.0, 0.0), (0.4, 20.0), (0.7, 80.0), (1.0, 500.0)]);

const CONTINUOUS: [ParamSpec; 16] = [PREDELAY, SIZE, DECAY, DIFFUSION, EARLY, MOD_DEPTH, MOD_RATE, WIDTH, DUCK, DUCK_THRESH, DUCK_RELEASE, MIX, OUT_GAIN, EARLY_SIZE, EARLY_SEND, D_FREQ];

/// Every parameter's manifest line, in host order.
pub fn manifest() -> String {
    let mut lines: Vec<String> = CONTINUOUS[..8].iter().map(ParamSpec::manifest_line).collect();
    lines.push(FREEZE.manifest_line());
    lines.extend(CONTINUOUS[8..13].iter().map(ParamSpec::manifest_line));
    lines.push(BYPASS.manifest_line());
    lines.extend(CONTINUOUS[13..15].iter().map(ParamSpec::manifest_line));
    for b in 0..DECAY_BANDS {
        let prefix = |line: String| format!("d{}_{line}", b + 1);
        lines.extend([D_ON.manifest_line(), D_TYPE.manifest_line(), D_FREQ.manifest_line(), D_MULT.manifest_line(), D_Q.manifest_line()].map(prefix));
    }
    for b in 0..TONE_BANDS {
        let prefix = |line: String| format!("e{}_{line}", b + 1);
        lines.extend([E_ON.manifest_line(), E_TYPE.manifest_line(), E_FREQ.manifest_line(), E_GAIN.manifest_line(), E_Q.manifest_line(), E_SLOPE.manifest_line()].map(prefix));
    }
    lines.join("\n") + "\n"
}

/// Every parameter id, in host order.
pub fn ids() -> Vec<String> {
    manifest().lines().filter_map(|l| l.split(" | ").next()).map(str::to_owned).collect()
}

/// Performance and monitoring, not sound: never in presets or snapshots.
pub fn is_monitoring(id: &str) -> bool {
    [FREEZE.id, BYPASS.id].contains(&id)
}

pub fn sound_ids() -> Vec<String> {
    ids().into_iter().filter(|id| !is_monitoring(id)).collect()
}

/// A band parameter's bank (`'d'` or `'e'`), band (from 0) and short id, e.g. `e3_q` → ('e', 2, "q").
fn split_band_id(id: &str) -> Option<(char, usize, &str)> {
    let (slot, short) = id.split_once('_')?;
    let bank = slot.chars().next()?;
    let b = slot[1..].parse::<usize>().ok()?.checked_sub(1)?;
    let count = match bank {
        'd' => DECAY_BANDS,
        'e' => TONE_BANDS,
        _ => return None,
    };
    (b < count).then_some((bank, b, short))
}

/// Every continuous spec by its id, for stand-in hosts.
pub fn spec_for(id: &str) -> Option<ParamSpec> {
    if let Some(s) = CONTINUOUS[..15].iter().find(|s| s.id == id) {
        return Some(*s);
    }
    match split_band_id(id)? {
        ('d', _, "freq") => Some(D_FREQ),
        ('d', _, "mult") => Some(D_MULT),
        ('d', _, "q") => Some(D_Q),
        ('e', _, "freq") => Some(E_FREQ),
        ('e', _, "gain") => Some(E_GAIN),
        ('e', _, "q") => Some(E_Q),
        _ => None,
    }
}

/// Every choice spec by its id, for stand-in hosts.
pub fn choice_for(id: &str) -> Option<ChoiceSpec> {
    match split_band_id(id)? {
        ('d', _, "type") => Some(D_TYPE),
        ('e', _, "type") => Some(E_TYPE),
        ('e', _, "slope") => Some(E_SLOPE),
        _ => None,
    }
}

pub struct DecayBandParams {
    pub on: BoolParam,
    pub kind: IntParam,
    pub freq: FloatParam,
    pub mult: FloatParam,
    pub q: FloatParam,
}

impl DecayBandParams {
    fn new(band: usize) -> Self {
        let name = |what: &str| format!("Decay Band {} {what}", band + 1);
        Self { on: bool_param(&D_ON, name("On")), kind: choice_param(&D_TYPE, name("Shape")), freq: float_param_named(&D_FREQ, name("Frequency")), mult: float_param_named(&D_MULT, name("Decay")), q: float_param_named(&D_Q, name("Q")) }
    }
}

pub struct ToneBandParams {
    pub on: BoolParam,
    pub kind: IntParam,
    pub freq: FloatParam,
    pub gain: FloatParam,
    pub q: FloatParam,
    pub slope: IntParam,
}

impl ToneBandParams {
    fn new(band: usize) -> Self {
        let name = |what: &str| format!("Tone Band {} {what}", band + 1);
        Self {
            on: bool_param(&E_ON, name("On")),
            kind: choice_param(&E_TYPE, name("Type")),
            freq: float_param_named(&E_FREQ, name("Frequency")),
            gain: float_param_named(&E_GAIN, name("Gain")),
            q: float_param_named(&E_Q, name("Q")),
            slope: choice_param(&E_SLOPE, name("Slope")),
        }
    }
}

pub struct VerbParams {
    pub predelay: FloatParam,
    pub size: FloatParam,
    pub decay: FloatParam,
    pub diffusion: FloatParam,
    pub early: FloatParam,
    pub mod_depth: FloatParam,
    pub mod_rate: FloatParam,
    pub width: FloatParam,
    pub freeze: BoolParam,
    pub duck: FloatParam,
    pub duck_thresh: FloatParam,
    pub duck_release: FloatParam,
    pub mix: FloatParam,
    pub out_gain: FloatParam,
    pub bypass: BoolParam,
    pub early_size: FloatParam,
    pub early_send: FloatParam,
    pub bands: [DecayBandParams; DECAY_BANDS],
    pub tone: [ToneBandParams; TONE_BANDS],
    pub snapshots: std::sync::Arc<Snapshots>,
    pub preset: std::sync::Arc<CurrentPreset>,
}

impl Default for VerbParams {
    fn default() -> Self {
        let flag = |spec: &'static BoolSpec| bool_param(spec, spec.name.to_owned());
        Self {
            predelay: float_param(&PREDELAY),
            size: float_param(&SIZE),
            decay: float_param(&DECAY),
            diffusion: float_param(&DIFFUSION),
            early: float_param(&EARLY),
            mod_depth: float_param(&MOD_DEPTH),
            mod_rate: float_param(&MOD_RATE),
            width: float_param(&WIDTH),
            freeze: flag(&FREEZE),
            duck: float_param(&DUCK),
            duck_thresh: float_param(&DUCK_THRESH),
            duck_release: float_param(&DUCK_RELEASE),
            mix: float_param(&MIX),
            out_gain: float_param(&OUT_GAIN),
            // An ordinary parameter, never the host's bypass (REAPER stops processing a
            // host-bypassed plugin, and then the editor could not turn it off).
            bypass: flag(&BYPASS),
            early_size: float_param(&EARLY_SIZE),
            early_send: float_param(&EARLY_SEND),
            bands: std::array::from_fn(DecayBandParams::new),
            tone: std::array::from_fn(ToneBandParams::new),
            snapshots: std::sync::Arc::new(Snapshots::new()),
            preset: std::sync::Arc::new(CurrentPreset::default()),
        }
    }
}

/// The reverb's settings from plain values by id (the plugin and the editor's stand-in host
/// share it).
pub fn reverb_settings(p: impl Fn(&str) -> f32) -> ReverbSettings {
    let index = |v: f32| v.round().max(0.0) as usize;
    ReverbSettings {
        predelay_ms: p(PREDELAY.id),
        size: p(SIZE.id) / 100.0,
        decay_s: p(DECAY.id),
        diffusion: p(DIFFUSION.id) / 100.0,
        early: p(EARLY.id) / 100.0,
        mod_depth: p(MOD_DEPTH.id) / 100.0,
        mod_rate: p(MOD_RATE.id),
        width: p(WIDTH.id) / 100.0,
        freeze: p(FREEZE.id) >= 0.5,
        duck_db: p(DUCK.id),
        duck_thresh_db: p(DUCK_THRESH.id),
        duck_release_ms: p(DUCK_RELEASE.id),
        early_size_ms: p(EARLY_SIZE.id),
        early_send: p(EARLY_SEND.id) / 100.0,
        bands: std::array::from_fn(|b| {
            let v = |id: &str| p(&band_id(b, id));
            DecayBand { on: v(D_ON.id) >= 0.5, shape: DecayShape::from_index(index(v(D_TYPE.id))), freq: v(D_FREQ.id), mult: v(D_MULT.id), q: v(D_Q.id) }
        }),
        eq: std::array::from_fn(|b| {
            let v = |id: &str| p(&tone_id(b, id));
            BandSettings {
                enabled: v(E_ON.id) >= 0.5,
                kind: BandType::from_index(index(v(E_TYPE.id))),
                freq: f64::from(v(E_FREQ.id)),
                gain_db: f64::from(v(E_GAIN.id)),
                q: f64::from(v(E_Q.id)),
                slope: index(v(E_SLOPE.id)).min(E_SLOPE.choices.len() - 1),
                ..BandSettings::default()
            }
        }),
    }
}

impl VerbParams {
    fn plain(&self, id: &str) -> f32 {
        let f = |p: &FloatParam| p.value();
        let flag = |p: &BoolParam| f32::from(u8::from(p.value()));
        let int = |p: &IntParam| p.value() as f32;
        match id {
            "predelay" => f(&self.predelay),
            "size" => f(&self.size),
            "decay" => f(&self.decay),
            "diffusion" => f(&self.diffusion),
            "early" => f(&self.early),
            "mod_depth" => f(&self.mod_depth),
            "mod_rate" => f(&self.mod_rate),
            "width" => f(&self.width),
            "freeze" => flag(&self.freeze),
            "duck" => f(&self.duck),
            "duck_thresh" => f(&self.duck_thresh),
            "duck_release" => f(&self.duck_release),
            "early_size" => f(&self.early_size),
            "early_send" => f(&self.early_send),
            _ => match split_band_id(id) {
                Some(('d', b, short)) => {
                    let band = &self.bands[b];
                    match short {
                        "on" => flag(&band.on),
                        "type" => int(&band.kind),
                        "freq" => f(&band.freq),
                        "mult" => f(&band.mult),
                        "q" => f(&band.q),
                        _ => 0.0,
                    }
                }
                Some(('e', b, short)) => {
                    let band = &self.tone[b];
                    match short {
                        "on" => flag(&band.on),
                        "type" => int(&band.kind),
                        "freq" => f(&band.freq),
                        "gain" => f(&band.gain),
                        "q" => f(&band.q),
                        "slope" => int(&band.slope),
                        _ => 0.0,
                    }
                }
                _ => 0.0,
            },
        }
    }

    pub fn settings(&self) -> ReverbSettings {
        reverb_settings(|id| self.plain(id))
    }

    pub fn output(&self) -> Output {
        Output { mix: self.mix.value() / 100.0, gain_db: self.out_gain.value(), bypass: self.bypass.value() }
    }
}

// SAFETY: every `ParamPtr` points into `self`, which nih-plug keeps alive (in an `Arc`) for as
// long as it uses the map. The ids match `manifest()` (checked by tests/params.rs).
unsafe impl Params for VerbParams {
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
            p(PREDELAY.id, self.predelay.as_ptr(), "Space"),
            p(SIZE.id, self.size.as_ptr(), "Space"),
            p(DECAY.id, self.decay.as_ptr(), "Decay"),
            p(DIFFUSION.id, self.diffusion.as_ptr(), "Space"),
            p(EARLY.id, self.early.as_ptr(), "Early"),
            p(MOD_DEPTH.id, self.mod_depth.as_ptr(), "Modulation"),
            p(MOD_RATE.id, self.mod_rate.as_ptr(), "Modulation"),
            p(WIDTH.id, self.width.as_ptr(), "Space"),
            p(FREEZE.id, self.freeze.as_ptr(), ""),
            p(DUCK.id, self.duck.as_ptr(), "Ducking"),
            p(DUCK_THRESH.id, self.duck_thresh.as_ptr(), "Ducking"),
            p(DUCK_RELEASE.id, self.duck_release.as_ptr(), "Ducking"),
            p(MIX.id, self.mix.as_ptr(), ""),
            p(OUT_GAIN.id, self.out_gain.as_ptr(), ""),
            p(BYPASS.id, self.bypass.as_ptr(), ""),
            p(EARLY_SIZE.id, self.early_size.as_ptr(), "Early"),
            p(EARLY_SEND.id, self.early_send.as_ptr(), "Early"),
        ];
        for (b, band) in self.bands.iter().enumerate() {
            for (id, ptr) in [(D_ON.id, band.on.as_ptr()), (D_TYPE.id, band.kind.as_ptr()), (D_FREQ.id, band.freq.as_ptr()), (D_MULT.id, band.mult.as_ptr()), (D_Q.id, band.q.as_ptr())] {
                map.push((band_id(b, id), ptr, "Decay EQ".to_owned()));
            }
        }
        for (b, band) in self.tone.iter().enumerate() {
            for (id, ptr) in [(E_ON.id, band.on.as_ptr()), (E_TYPE.id, band.kind.as_ptr()), (E_FREQ.id, band.freq.as_ptr()), (E_GAIN.id, band.gain.as_ptr()), (E_Q.id, band.q.as_ptr()), (E_SLOPE.id, band.slope.as_ptr())] {
                map.push((tone_id(b, id), ptr, "Tone EQ".to_owned()));
            }
        }
        map
    }
}
