//! Mite Double parameters (docs/design/double-brief.md §3).
//!
//! IDs are frozen once released (`tests/golden/mite_double.params.txt`).

use mite_core::presets::CurrentPreset;
use mite_core::snapshots::Snapshots;
use mite_core::{BoolSpec, ChoiceSpec, ParamDisplay, ParamScale, ParamSpec};
use mite_dsp::doubler::{DoublerSettings, VoiceSettings, MAX_DELAY_MS, MAX_PITCH_CENTS, MAX_TIMING_MS, MAX_VOICES, MIN_DELAY_MS};
use mite_plugin::{bool_param, choice_param, float_param, float_param_named};
use nih_plug::prelude::*;

use crate::dsp::Output;

/// The state format version (`mite_plugin::state`).
pub const STATE_VERSION: u32 = 1;

#[allow(clippy::too_many_arguments)] // one line per parameter reads best
const fn spec(id: &'static str, name: &'static str, unit: &'static str, min: f32, max: f32, default: f32, scale: ParamScale, decimals: u8, wheel: (f32, f32), display: ParamDisplay) -> ParamSpec {
    ParamSpec { id, name, unit, min, max, default, scale, smoothing_ms: 20.0, decimals, wheel_step: wheel.0, wheel_step_fine: wheel.1, display }
}

// ---- The take -----------------------------------------------------------------------------------

pub const DELAY: ParamSpec = spec("delay", "Delay", "ms", MIN_DELAY_MS, MAX_DELAY_MS, 16.0, ParamScale::Log, 1, (1.0 / 24.0, 1.0 / 96.0), ParamDisplay::Time);
pub const TIMING: ParamSpec = spec("timing", "Timing", "ms", 0.0, MAX_TIMING_MS, 14.0, ParamScale::Linear, 1, (0.5, 0.1), ParamDisplay::Time);
pub const PITCH: ParamSpec = spec("pitch", "Pitch", "ct", 0.0, MAX_PITCH_CENTS, 7.0, ParamScale::Linear, 1, (0.5, 0.1), ParamDisplay::Number);
pub const VARIATION: ParamSpec = spec("variation", "Variation", "%", 0.0, 100.0, 50.0, ParamScale::Linear, 0, (2.0, 0.5), ParamDisplay::Number);
pub const CHORUS: ParamSpec = spec("chorus", "Chorus", "%", 0.0, 100.0, 15.0, ParamScale::Linear, 0, (2.0, 0.5), ParamDisplay::Number);
pub const CHORUS_RATE: ParamSpec = spec("chorus_rate", "Chorus Rate", "Hz", 0.1, 5.0, 0.6, ParamScale::Log, 2, (1.0 / 24.0, 1.0 / 96.0), ParamDisplay::Number);
pub const TONE: ParamSpec = spec("tone", "Tone", "dB", -6.0, 6.0, 0.0, ParamScale::Linear, 1, (0.25, 0.05), ParamDisplay::Number);
pub const FOLLOW: BoolSpec = BoolSpec { id: "follow", name: "Follow Notes", default: true };
pub const VOICES: ChoiceSpec = ChoiceSpec { id: "voices", name: "Doubles", choices: &["1", "2", "3"], default: 0 };

// ---- Placement ----------------------------------------------------------------------------------

pub const SOURCE_PAN: ParamSpec = spec("source_pan", "Source Pan", "%", -100.0, 100.0, -100.0, ParamScale::Linear, 0, (5.0, 1.0), ParamDisplay::Number);
pub const SOURCE_LEVEL: ParamSpec = spec("source_level", "Source Level", "dB", -48.0, 6.0, 0.0, ParamScale::Linear, 1, (0.5, 0.1), ParamDisplay::Number);
pub const V_PAN: ParamSpec = spec("pan", "Pan", "%", -100.0, 100.0, 100.0, ParamScale::Linear, 0, (5.0, 1.0), ParamDisplay::Number);
pub const V_LEVEL: ParamSpec = spec("level", "Level", "dB", -48.0, 6.0, 0.0, ParamScale::Linear, 1, (0.5, 0.1), ParamDisplay::Number);
/// Each double's place by default: the first opposite the source, the others between.
pub const V_DEFAULTS: [(f32, f32); MAX_VOICES] = [(100.0, 0.0), (-60.0, -3.0), (60.0, -3.0)];

// ---- Output -------------------------------------------------------------------------------------

pub const OUT_GAIN: ParamSpec = spec("out_gain", "Output", "dB", -24.0, 24.0, 0.0, ParamScale::Linear, 1, (0.5, 0.1), ParamDisplay::Number);
pub const BYPASS: BoolSpec = BoolSpec { id: "bypass", name: "Bypass", default: false };
/// Keep the output as loud as the input, whatever the doubles add (user, 2026-11-05: "give it
/// auto gain too").
pub const AUTO_GAIN: BoolSpec = BoolSpec { id: "auto_gain", name: "Auto Gain", default: true };

/// A double's parameter id, e.g. `v2_pan` (`voice` from 0).
pub fn voice_id(voice: usize, param: &str) -> String {
    format!("v{}_{param}", voice + 1)
}

/// A double's spec with its own default.
pub fn voice_spec(voice: usize, param: &str) -> Option<ParamSpec> {
    let (pan, level) = V_DEFAULTS[voice];
    match param {
        "pan" => Some(ParamSpec { default: pan, ..V_PAN }),
        "level" => Some(ParamSpec { default: level, ..V_LEVEL }),
        _ => None,
    }
}

const CONTINUOUS: [ParamSpec; 10] = [DELAY, TIMING, PITCH, VARIATION, CHORUS, CHORUS_RATE, TONE, SOURCE_PAN, SOURCE_LEVEL, OUT_GAIN];

pub fn manifest() -> String {
    let mut lines: Vec<String> = CONTINUOUS[..7].iter().map(ParamSpec::manifest_line).collect();
    lines.push(FOLLOW.manifest_line());
    lines.push(VOICES.manifest_line());
    lines.extend(CONTINUOUS[7..].iter().map(ParamSpec::manifest_line));
    lines.push(BYPASS.manifest_line());
    for v in 0..MAX_VOICES {
        for p in ["pan", "level"] {
            lines.push(format!("v{}_{}", v + 1, voice_spec(v, p).expect("spec").manifest_line()));
        }
    }
    lines.push(AUTO_GAIN.manifest_line());
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

/// A double's index (from 0) and short id, e.g. `v2_pan` → (1, "pan").
fn split_voice(id: &str) -> Option<(usize, &str)> {
    let (slot, short) = id.split_once('_')?;
    let v = slot.strip_prefix('v')?.parse::<usize>().ok()?.checked_sub(1)?;
    (v < MAX_VOICES).then_some((v, short))
}

/// Every continuous spec by its id, for stand-in hosts.
pub fn spec_for(id: &str) -> Option<ParamSpec> {
    if let Some(s) = CONTINUOUS.iter().find(|s| s.id == id) {
        return Some(*s);
    }
    let (v, short) = split_voice(id)?;
    voice_spec(v, short)
}

pub struct VoiceParams {
    pub pan: FloatParam,
    pub level: FloatParam,
}

pub struct DoubleParams {
    pub delay: FloatParam,
    pub timing: FloatParam,
    pub pitch: FloatParam,
    pub variation: FloatParam,
    pub chorus: FloatParam,
    pub chorus_rate: FloatParam,
    pub tone: FloatParam,
    pub follow: BoolParam,
    pub voices: IntParam,
    pub source_pan: FloatParam,
    pub source_level: FloatParam,
    pub out_gain: FloatParam,
    pub bypass: BoolParam,
    pub auto_gain: BoolParam,
    pub voice: [VoiceParams; MAX_VOICES],
    pub snapshots: std::sync::Arc<Snapshots>,
    pub preset: std::sync::Arc<CurrentPreset>,
}

/// Each double's specs live as long as the plugin (nih-plug wants `'static`).
fn leaked(voice: usize, param: &str) -> &'static ParamSpec {
    Box::leak(Box::new(voice_spec(voice, param).expect("spec")))
}

impl Default for DoubleParams {
    fn default() -> Self {
        Self {
            delay: float_param(&DELAY),
            timing: float_param(&TIMING),
            pitch: float_param(&PITCH),
            variation: float_param(&VARIATION),
            chorus: float_param(&CHORUS),
            chorus_rate: float_param(&CHORUS_RATE),
            tone: float_param(&TONE),
            follow: bool_param(&FOLLOW, FOLLOW.name.to_owned()),
            voices: choice_param(&VOICES, VOICES.name.to_owned()),
            source_pan: float_param(&SOURCE_PAN),
            source_level: float_param(&SOURCE_LEVEL),
            out_gain: float_param(&OUT_GAIN),
            // An ordinary parameter, never the host's bypass (REAPER stops processing a
            // host-bypassed plugin, and then the editor could not turn it off).
            bypass: bool_param(&BYPASS, BYPASS.name.to_owned()),
            auto_gain: bool_param(&AUTO_GAIN, AUTO_GAIN.name.to_owned()),
            voice: std::array::from_fn(|v| VoiceParams {
                pan: float_param_named(leaked(v, "pan"), format!("Double {} Pan", v + 1)),
                level: float_param_named(leaked(v, "level"), format!("Double {} Level", v + 1)),
            }),
            snapshots: std::sync::Arc::new(Snapshots::new()),
            preset: std::sync::Arc::new(CurrentPreset::default()),
        }
    }
}

/// The engine's settings from plain values by id.
pub fn doubler_settings(p: impl Fn(&str) -> f32) -> DoublerSettings {
    DoublerSettings {
        voices: (p(VOICES.id).round().max(0.0) as usize + 1).min(MAX_VOICES),
        voice: std::array::from_fn(|v| VoiceSettings { pan: p(&voice_id(v, "pan")) / 100.0, level_db: p(&voice_id(v, "level")) }),
        source_pan: p(SOURCE_PAN.id) / 100.0,
        source_db: p(SOURCE_LEVEL.id),
        delay_ms: p(DELAY.id),
        timing_ms: p(TIMING.id),
        pitch_cents: p(PITCH.id),
        variation: p(VARIATION.id) / 100.0,
        chorus: p(CHORUS.id) / 100.0,
        chorus_rate: p(CHORUS_RATE.id),
        tone_db: p(TONE.id),
        follow_notes: p(FOLLOW.id) >= 0.5,
    }
}

impl DoubleParams {
    fn plain(&self, id: &str) -> f32 {
        let f = |p: &FloatParam| p.value();
        match id {
            "delay" => f(&self.delay),
            "timing" => f(&self.timing),
            "pitch" => f(&self.pitch),
            "variation" => f(&self.variation),
            "chorus" => f(&self.chorus),
            "chorus_rate" => f(&self.chorus_rate),
            "tone" => f(&self.tone),
            "follow" => f32::from(u8::from(self.follow.value())),
            "voices" => self.voices.value() as f32,
            "source_pan" => f(&self.source_pan),
            "source_level" => f(&self.source_level),
            _ => match split_voice(id) {
                Some((v, "pan")) => f(&self.voice[v].pan),
                Some((v, "level")) => f(&self.voice[v].level),
                _ => 0.0,
            },
        }
    }

    pub fn settings(&self) -> DoublerSettings {
        doubler_settings(|id| self.plain(id))
    }

    pub fn output(&self) -> Output {
        Output { gain_db: self.out_gain.value(), bypass: self.bypass.value(), auto_gain: self.auto_gain.value() }
    }
}

// SAFETY: every `ParamPtr` points into `self`, which nih-plug keeps alive (in an `Arc`) for as
// long as it uses the map. The ids match `manifest()` (checked by tests/params.rs).
unsafe impl Params for DoubleParams {
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
            p(DELAY.id, self.delay.as_ptr(), "Take"),
            p(TIMING.id, self.timing.as_ptr(), "Take"),
            p(PITCH.id, self.pitch.as_ptr(), "Take"),
            p(VARIATION.id, self.variation.as_ptr(), "Take"),
            p(CHORUS.id, self.chorus.as_ptr(), "Chorus"),
            p(CHORUS_RATE.id, self.chorus_rate.as_ptr(), "Chorus"),
            p(TONE.id, self.tone.as_ptr(), "Take"),
            p(FOLLOW.id, self.follow.as_ptr(), "Take"),
            p(VOICES.id, self.voices.as_ptr(), "Doubles"),
            p(SOURCE_PAN.id, self.source_pan.as_ptr(), "Source"),
            p(SOURCE_LEVEL.id, self.source_level.as_ptr(), "Source"),
            p(OUT_GAIN.id, self.out_gain.as_ptr(), ""),
            p(BYPASS.id, self.bypass.as_ptr(), ""),
        ];
        for (v, voice) in self.voice.iter().enumerate() {
            map.push((voice_id(v, "pan"), voice.pan.as_ptr(), "Doubles".to_owned()));
            map.push((voice_id(v, "level"), voice.level.as_ptr(), "Doubles".to_owned()));
        }
        map.push((AUTO_GAIN.id.to_owned(), self.auto_gain.as_ptr(), String::new()));
        map
    }
}
