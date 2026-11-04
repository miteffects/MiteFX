//! Mite Verb parameters (docs/design/verb-brief.md §3).
//!
//! IDs are frozen once released (`tests/golden/mite_verb.params.txt`).

use mite_core::presets::CurrentPreset;
use mite_core::snapshots::Snapshots;
use mite_core::{BoolSpec, DialCurve, ParamDisplay, ParamScale, ParamSpec};
use mite_dsp::reverb::{DecayBand, ReverbSettings, DECAY_BANDS};
use mite_plugin::{bool_param, float_param};
use nih_plug::prelude::*;

use crate::dsp::Output;

/// The state format version (`mite_plugin::state`).
///
/// - 2 (2026-11-04): the early field's size and send, and the decay EQ's four bells
///   (`d1…d4_{on,freq,mult,q}`), appended; a version 1 state gets their defaults (bells off).
///   The algorithm changed the same day (32 lines, diffusers in the loop), so a version 1
///   state sounds different anyway; nothing to migrate.
pub const STATE_VERSION: u32 = 2;

/// Version 1 → 2: the new parameters arrive at their defaults.
pub fn v1_to_v2(_state: &mut nih_plug::wrapper::state::PluginState) {}

#[allow(clippy::too_many_arguments)] // one line per parameter reads best
const fn spec(id: &'static str, name: &'static str, unit: &'static str, min: f32, max: f32, default: f32, scale: ParamScale, decimals: u8, wheel: (f32, f32), display: ParamDisplay) -> ParamSpec {
    ParamSpec { id, name, unit, min, max, default, scale, smoothing_ms: 20.0, decimals, wheel_step: wheel.0, wheel_step_fine: wheel.1, display }
}

// ---- The space -------------------------------------------------------------------------------

pub const PREDELAY: ParamSpec = spec("predelay", "Pre-delay", "ms", 0.0, 500.0, 10.0, ParamScale::Linear, 0, (2.0, 0.5), ParamDisplay::Time);
pub const SIZE: ParamSpec = spec("size", "Size", "%", 0.0, 100.0, 50.0, ParamScale::Linear, 0, (2.0, 0.5), ParamDisplay::Number);
pub const DECAY: ParamSpec = spec("decay", "Decay", "s", 0.1, 30.0, 2.0, ParamScale::Log, 2, (1.0 / 24.0, 1.0 / 96.0), ParamDisplay::Number);
pub const LO_MULT: ParamSpec = spec("lo_mult", "Low Decay", "×", 0.25, 4.0, 1.2, ParamScale::Log, 2, (1.0 / 24.0, 1.0 / 96.0), ParamDisplay::Number);
pub const LO_FREQ: ParamSpec = spec("lo_freq", "Low Frequency", "Hz", 50.0, 2000.0, 250.0, ParamScale::Log, 0, (1.0 / 24.0, 1.0 / 96.0), ParamDisplay::Frequency);
pub const HI_MULT: ParamSpec = spec("hi_mult", "High Decay", "×", 0.1, 4.0, 0.5, ParamScale::Log, 2, (1.0 / 24.0, 1.0 / 96.0), ParamDisplay::Number);
pub const HI_FREQ: ParamSpec = spec("hi_freq", "High Frequency", "Hz", 1000.0, 16_000.0, 5000.0, ParamScale::Log, 0, (1.0 / 24.0, 1.0 / 96.0), ParamDisplay::Frequency);
pub const DIFFUSION: ParamSpec = spec("diffusion", "Diffusion", "%", 0.0, 100.0, 70.0, ParamScale::Linear, 0, (2.0, 0.5), ParamDisplay::Number);
pub const EARLY: ParamSpec = spec("early", "Early Reflections", "%", 0.0, 100.0, 50.0, ParamScale::Linear, 0, (2.0, 0.5), ParamDisplay::Number);
pub const MOD_DEPTH: ParamSpec = spec("mod_depth", "Modulation", "%", 0.0, 100.0, 30.0, ParamScale::Linear, 0, (2.0, 0.5), ParamDisplay::Number);
pub const MOD_RATE: ParamSpec = spec("mod_rate", "Modulation Rate", "Hz", 0.05, 5.0, 0.6, ParamScale::Log, 2, (1.0 / 24.0, 1.0 / 96.0), ParamDisplay::Number);
pub const WIDTH: ParamSpec = spec("width", "Width", "%", 0.0, 150.0, 100.0, ParamScale::Linear, 0, (2.0, 0.5), ParamDisplay::Number);

// ---- The reverb's EQ ---------------------------------------------------------------------------

pub const LO_CUT: ParamSpec = spec("lo_cut", "Low Cut", "Hz", 10.0, 1000.0, 20.0, ParamScale::Log, 0, (1.0 / 24.0, 1.0 / 96.0), ParamDisplay::Frequency);
pub const HI_CUT: ParamSpec = spec("hi_cut", "High Cut", "Hz", 1000.0, 20_000.0, 20_000.0, ParamScale::Log, 0, (1.0 / 24.0, 1.0 / 96.0), ParamDisplay::Frequency);

// ---- Freeze and ducking --------------------------------------------------------------------------

pub const FREEZE: BoolSpec = BoolSpec { id: "freeze", name: "Freeze", default: false };
pub const DUCK: ParamSpec = spec("duck", "Ducking", "dB", 0.0, 24.0, 0.0, ParamScale::Linear, 1, (0.5, 0.1), ParamDisplay::Number);
pub const DUCK_THRESH: ParamSpec = spec("duck_thresh", "Ducking Threshold", "dB", -60.0, 0.0, -30.0, ParamScale::Linear, 1, (0.5, 0.1), ParamDisplay::Number);
pub const DUCK_RELEASE: ParamSpec = spec("duck_release", "Ducking Release", "ms", 20.0, 2000.0, 300.0, ParamScale::Log, 0, (0.25, 1.0 / 16.0), ParamDisplay::Time);

// ---- Output -------------------------------------------------------------------------------------

pub const MIX: ParamSpec = spec("mix", "Mix", "%", 0.0, 100.0, 30.0, ParamScale::Linear, 0, (2.0, 0.5), ParamDisplay::Number);
pub const OUT_GAIN: ParamSpec = spec("out_gain", "Output", "dB", -24.0, 24.0, 0.0, ParamScale::Linear, 1, (0.5, 0.1), ParamDisplay::Number);
pub const BYPASS: BoolSpec = BoolSpec { id: "bypass", name: "Bypass", default: false };

// ---- State 2: the early field and the decay EQ's bells ---------------------------------------

pub const EARLY_SIZE: ParamSpec = spec("early_size", "Early Size", "ms", 2.0, 200.0, 35.0, ParamScale::Log, 0, (1.0 / 24.0, 1.0 / 96.0), ParamDisplay::Time);
pub const EARLY_SEND: ParamSpec = spec("early_send", "Early Send", "%", 0.0, 100.0, 0.0, ParamScale::Linear, 0, (2.0, 0.5), ParamDisplay::Number);
pub const D_ON: BoolSpec = BoolSpec { id: "on", name: "On", default: false };
pub const D_FREQ: ParamSpec = spec("freq", "Frequency", "Hz", 50.0, 16_000.0, 1000.0, ParamScale::Log, 0, (1.0 / 24.0, 1.0 / 96.0), ParamDisplay::Frequency);
pub const D_MULT: ParamSpec = spec("mult", "Decay", "×", 0.1, 4.0, 1.0, ParamScale::Log, 2, (1.0 / 24.0, 1.0 / 96.0), ParamDisplay::Number);
pub const D_Q: ParamSpec = spec("q", "Q", "", 0.3, 6.0, 1.0, ParamScale::Log, 2, (1.0 / 12.0, 1.0 / 48.0), ParamDisplay::Number);
/// The bells' first frequencies (each in its own part of the range).
pub const D_FREQS: [f32; DECAY_BANDS] = [150.0, 600.0, 2500.0, 8000.0];
static D_FREQ_SPECS: [ParamSpec; DECAY_BANDS] = [
    ParamSpec { default: D_FREQS[0], ..D_FREQ },
    ParamSpec { default: D_FREQS[1], ..D_FREQ },
    ParamSpec { default: D_FREQS[2], ..D_FREQ },
    ParamSpec { default: D_FREQS[3], ..D_FREQ },
];

/// A decay bell's parameter id, e.g. `d2_mult` (`band` from 0).
pub fn band_id(band: usize, param: &str) -> String {
    format!("d{}_{param}", band + 1)
}

/// Dials: common values get more travel.
pub const PREDELAY_DIAL: DialCurve = DialCurve(&[(0.0, 0.0), (0.4, 20.0), (0.7, 80.0), (1.0, 500.0)]);

const CONTINUOUS: [ParamSpec; 19] = [PREDELAY, SIZE, DECAY, LO_MULT, LO_FREQ, HI_MULT, HI_FREQ, DIFFUSION, EARLY, MOD_DEPTH, MOD_RATE, WIDTH, LO_CUT, HI_CUT, DUCK, DUCK_THRESH, DUCK_RELEASE, MIX, OUT_GAIN];

/// Every parameter's manifest line, in host order.
pub fn manifest() -> String {
    let mut lines: Vec<String> = CONTINUOUS[..14].iter().map(ParamSpec::manifest_line).collect();
    lines.push(FREEZE.manifest_line());
    lines.extend(CONTINUOUS[14..].iter().map(ParamSpec::manifest_line));
    lines.push(BYPASS.manifest_line());
    // State 2.
    lines.extend([EARLY_SIZE, EARLY_SEND].iter().map(ParamSpec::manifest_line));
    for (b, freq) in D_FREQ_SPECS.iter().enumerate() {
        let prefix = |line: String| format!("d{}_{line}", b + 1);
        lines.push(prefix(D_ON.manifest_line()));
        lines.push(prefix(freq.manifest_line()));
        lines.push(prefix(D_MULT.manifest_line()));
        lines.push(prefix(D_Q.manifest_line()));
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

/// Every continuous spec by its id, for stand-in hosts.
pub fn spec_for(id: &str) -> Option<ParamSpec> {
    if let Some(s) = CONTINUOUS.into_iter().chain([EARLY_SIZE, EARLY_SEND]).find(|s| s.id == id) {
        return Some(s);
    }
    let (slot, short) = id.split_once('_')?;
    let b: usize = slot.strip_prefix('d')?.parse::<usize>().ok()?.checked_sub(1)?;
    match short {
        "freq" => Some(D_FREQ_SPECS[b.min(DECAY_BANDS - 1)]),
        "mult" => Some(D_MULT),
        "q" => Some(D_Q),
        _ => None,
    }
}

pub struct BandParams {
    pub on: BoolParam,
    pub freq: FloatParam,
    pub mult: FloatParam,
    pub q: FloatParam,
}

impl BandParams {
    fn new(band: usize) -> Self {
        let name = |what: &str| format!("Decay Band {} {what}", band + 1);
        Self { on: bool_param(&D_ON, name("On")), freq: mite_plugin::float_param_named(&D_FREQ_SPECS[band], name("Frequency")), mult: mite_plugin::float_param_named(&D_MULT, name("Decay")), q: mite_plugin::float_param_named(&D_Q, name("Q")) }
    }
}

pub struct VerbParams {
    pub predelay: FloatParam,
    pub size: FloatParam,
    pub decay: FloatParam,
    pub lo_mult: FloatParam,
    pub lo_freq: FloatParam,
    pub hi_mult: FloatParam,
    pub hi_freq: FloatParam,
    pub diffusion: FloatParam,
    pub early: FloatParam,
    pub mod_depth: FloatParam,
    pub mod_rate: FloatParam,
    pub width: FloatParam,
    pub lo_cut: FloatParam,
    pub hi_cut: FloatParam,
    pub freeze: BoolParam,
    pub duck: FloatParam,
    pub duck_thresh: FloatParam,
    pub duck_release: FloatParam,
    pub mix: FloatParam,
    pub out_gain: FloatParam,
    pub bypass: BoolParam,
    pub early_size: FloatParam,
    pub early_send: FloatParam,
    pub bands: [BandParams; DECAY_BANDS],
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
            lo_mult: float_param(&LO_MULT),
            lo_freq: float_param(&LO_FREQ),
            hi_mult: float_param(&HI_MULT),
            hi_freq: float_param(&HI_FREQ),
            diffusion: float_param(&DIFFUSION),
            early: float_param(&EARLY),
            mod_depth: float_param(&MOD_DEPTH),
            mod_rate: float_param(&MOD_RATE),
            width: float_param(&WIDTH),
            lo_cut: float_param(&LO_CUT),
            hi_cut: float_param(&HI_CUT),
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
            bands: std::array::from_fn(BandParams::new),
            snapshots: std::sync::Arc::new(Snapshots::new()),
            preset: std::sync::Arc::new(CurrentPreset::default()),
        }
    }
}

/// The reverb's settings from plain values in [`CONTINUOUS`] terms (the plugin and the editor's
/// stand-in host share it).
pub fn reverb_settings(p: impl Fn(&str) -> f32) -> ReverbSettings {
    ReverbSettings {
        predelay_ms: p(PREDELAY.id),
        size: p(SIZE.id) / 100.0,
        decay_s: p(DECAY.id),
        lo_mult: p(LO_MULT.id),
        lo_freq: p(LO_FREQ.id),
        hi_mult: p(HI_MULT.id),
        hi_freq: p(HI_FREQ.id),
        diffusion: p(DIFFUSION.id) / 100.0,
        early: p(EARLY.id) / 100.0,
        mod_depth: p(MOD_DEPTH.id) / 100.0,
        mod_rate: p(MOD_RATE.id),
        width: p(WIDTH.id) / 100.0,
        lo_cut: p(LO_CUT.id),
        hi_cut: p(HI_CUT.id),
        freeze: p(FREEZE.id) >= 0.5,
        duck_db: p(DUCK.id),
        duck_thresh_db: p(DUCK_THRESH.id),
        duck_release_ms: p(DUCK_RELEASE.id),
        early_size_ms: p(EARLY_SIZE.id),
        early_send: p(EARLY_SEND.id) / 100.0,
        bands: std::array::from_fn(|b| DecayBand { on: p(&band_id(b, D_ON.id)) >= 0.5, freq: p(&band_id(b, D_FREQ.id)), mult: p(&band_id(b, D_MULT.id)), q: p(&band_id(b, D_Q.id)) }),
    }
}

impl VerbParams {
    fn plain(&self, id: &str) -> f32 {
        let f = |p: &FloatParam| p.value();
        match id {
            "predelay" => f(&self.predelay),
            "size" => f(&self.size),
            "decay" => f(&self.decay),
            "lo_mult" => f(&self.lo_mult),
            "lo_freq" => f(&self.lo_freq),
            "hi_mult" => f(&self.hi_mult),
            "hi_freq" => f(&self.hi_freq),
            "diffusion" => f(&self.diffusion),
            "early" => f(&self.early),
            "mod_depth" => f(&self.mod_depth),
            "mod_rate" => f(&self.mod_rate),
            "width" => f(&self.width),
            "lo_cut" => f(&self.lo_cut),
            "hi_cut" => f(&self.hi_cut),
            "freeze" => f32::from(u8::from(self.freeze.value())),
            "duck" => f(&self.duck),
            "duck_thresh" => f(&self.duck_thresh),
            "duck_release" => f(&self.duck_release),
            "early_size" => f(&self.early_size),
            "early_send" => f(&self.early_send),
            _ => {
                let Some((slot, short)) = id.split_once('_') else { return 0.0 };
                let Some(b) = slot.strip_prefix('d').and_then(|n| n.parse::<usize>().ok()).and_then(|n| n.checked_sub(1)).filter(|&b| b < DECAY_BANDS) else { return 0.0 };
                let band = &self.bands[b];
                match short {
                    "on" => f32::from(u8::from(band.on.value())),
                    "freq" => f(&band.freq),
                    "mult" => f(&band.mult),
                    "q" => f(&band.q),
                    _ => 0.0,
                }
            }
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
            p(LO_MULT.id, self.lo_mult.as_ptr(), "Decay"),
            p(LO_FREQ.id, self.lo_freq.as_ptr(), "Decay"),
            p(HI_MULT.id, self.hi_mult.as_ptr(), "Decay"),
            p(HI_FREQ.id, self.hi_freq.as_ptr(), "Decay"),
            p(DIFFUSION.id, self.diffusion.as_ptr(), "Space"),
            p(EARLY.id, self.early.as_ptr(), "Space"),
            p(MOD_DEPTH.id, self.mod_depth.as_ptr(), "Space"),
            p(MOD_RATE.id, self.mod_rate.as_ptr(), "Space"),
            p(WIDTH.id, self.width.as_ptr(), "Space"),
            p(LO_CUT.id, self.lo_cut.as_ptr(), "EQ"),
            p(HI_CUT.id, self.hi_cut.as_ptr(), "EQ"),
            p(FREEZE.id, self.freeze.as_ptr(), ""),
            p(DUCK.id, self.duck.as_ptr(), "Ducking"),
            p(DUCK_THRESH.id, self.duck_thresh.as_ptr(), "Ducking"),
            p(DUCK_RELEASE.id, self.duck_release.as_ptr(), "Ducking"),
            p(MIX.id, self.mix.as_ptr(), ""),
            p(OUT_GAIN.id, self.out_gain.as_ptr(), ""),
            p(BYPASS.id, self.bypass.as_ptr(), ""),
        ];
        // State 2.
        map.push(p(EARLY_SIZE.id, self.early_size.as_ptr(), "Space"));
        map.push(p(EARLY_SEND.id, self.early_send.as_ptr(), "Space"));
        for (b, band) in self.bands.iter().enumerate() {
            for (id, ptr) in [(D_ON.id, band.on.as_ptr()), (D_FREQ.id, band.freq.as_ptr()), (D_MULT.id, band.mult.as_ptr()), (D_Q.id, band.q.as_ptr())] {
                map.push((band_id(b, id), ptr, "Decay".to_owned()));
            }
        }
        map
    }
}
