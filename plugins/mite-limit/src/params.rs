//! Mite Limit parameters (docs/design/limit-brief.md §4).
//!
//! IDs are frozen once released (`tests/golden/mite_limit.params.txt`). Every parameter of the
//! full design exists from slice 1, so the state stays at version 1 while the features arrive; a
//! parameter whose feature is not built yet sits at its neutral default and the editor does not
//! show it.

use mite_core::presets::CurrentPreset;
use mite_core::snapshots::Snapshots;
use mite_core::{BoolSpec, ChoiceSpec, DialCurve, ParamDisplay, ParamScale, ParamSpec};
use mite_plugin::{bool_param, choice_param, float_param};
use nih_plug::prelude::*;

use crate::dsp::LimitSettings;

/// The state format version (`mite_plugin::state`).
pub const STATE_VERSION: u32 = 1;

#[allow(clippy::too_many_arguments)] // one line per parameter reads best
const fn spec(id: &'static str, name: &'static str, unit: &'static str, min: f32, max: f32, default: f32, scale: ParamScale, decimals: u8, wheel: (f32, f32), display: ParamDisplay) -> ParamSpec {
    ParamSpec { id, name, unit, min, max, default, scale, smoothing_ms: 20.0, decimals, wheel_step: wheel.0, wheel_step_fine: wheel.1, display }
}

// ---- Core ---------------------------------------------------------------------------------

pub const GAIN: ParamSpec = spec("gain", "Gain", "dB", 0.0, 30.0, 0.0, ParamScale::Linear, 1, (0.5, 0.1), ParamDisplay::Number);
/// −1 dBTP: what the streaming platforms ask for, so a lossy encode does not clip.
pub const CEILING: ParamSpec = spec("ceiling", "Ceiling", "dB", -30.0, 0.0, -1.0, ParamScale::Linear, 1, (0.1, 0.01), ParamDisplay::Number);
pub const STYLE_NAMES: [&str; 6] = ["Transparent", "Punch", "Dense", "Bus", "Safe", "Custom"];
pub const STYLE: ChoiceSpec = ChoiceSpec { id: "style", name: "Style", choices: &STYLE_NAMES, default: 0 };
/// The ramp before a peak (and the latency). Not smoothed: the latency follows its exact value.
pub const LOOKAHEAD: ParamSpec = ParamSpec { smoothing_ms: 0.0, ..spec("lookahead", "Lookahead", "ms", 0.0, 10.0, 2.0, ParamScale::Linear, 2, (0.25, 0.05), ParamDisplay::Time) };
/// How fast sustained limiting builds the slow envelope.
pub const ATTACK: ParamSpec = spec("attack", "Attack", "ms", 0.1, 100.0, 30.0, ParamScale::Log, 1, (0.25, 1.0 / 16.0), ParamDisplay::Time);
pub const RELEASE: ParamSpec = spec("release", "Release", "ms", 1.0, 2500.0, 100.0, ParamScale::Log, 0, (0.25, 1.0 / 16.0), ParamDisplay::Time);
/// Custom's slow envelope share.
pub const SLOW: ParamSpec = spec("slow", "Slow", "%", 0.0, 100.0, 50.0, ParamScale::Linear, 0, (1.0, 0.1), ParamDisplay::Number);
pub const LINK_T: ParamSpec = spec("link_t", "Transient Link", "%", 0.0, 100.0, 100.0, ParamScale::Linear, 0, (1.0, 0.1), ParamDisplay::Number);
pub const LINK_R: ParamSpec = spec("link_r", "Release Link", "%", 0.0, 100.0, 100.0, ParamScale::Linear, 0, (1.0, 0.1), ParamDisplay::Number);
pub const TRUE_PEAK: BoolSpec = BoolSpec { id: "true_peak", name: "True Peak", default: true };
pub const OVERSAMPLING: ChoiceSpec = ChoiceSpec { id: "oversampling", name: "Oversampling", choices: &["1×", "2×", "4×", "8×", "16×"], default: 1 };

// ---- Clipper (slice 5) --------------------------------------------------------------------

pub const CLIP_ON: BoolSpec = BoolSpec { id: "clip_on", name: "Clipper", default: false };
pub const CLIP_LEVEL: ParamSpec = spec("clip_level", "Clip Level", "dB", 0.0, 12.0, 3.0, ParamScale::Linear, 1, (0.25, 0.05), ParamDisplay::Number);
pub const CLIP_CURVE: ChoiceSpec = ChoiceSpec { id: "clip_curve", name: "Clip Curve", choices: &["Hard", "Soft", "Round"], default: 1 };

// ---- Transient protection (slice 6) -------------------------------------------------------

pub const PROTECT: ParamSpec = spec("protect", "Protect", "%", 0.0, 100.0, 0.0, ParamScale::Linear, 0, (5.0, 1.0), ParamDisplay::Number);

// ---- Output -------------------------------------------------------------------------------

/// Monitoring: the output turned to the input's loudness (slice 4).
pub const UNITY: BoolSpec = BoolSpec { id: "unity", name: "Unity Gain", default: false };
pub const DELTA: BoolSpec = BoolSpec { id: "delta", name: "Delta", default: false };
pub const BYPASS: BoolSpec = BoolSpec { id: "bypass", name: "Bypass", default: false };
pub const DITHER: ChoiceSpec = ChoiceSpec { id: "dither", name: "Dither", choices: &["Off", "24 bit", "20 bit", "16 bit"], default: 0 };
pub const SHAPING: ChoiceSpec = ChoiceSpec { id: "shaping", name: "Noise Shaping", choices: &["Off", "Light", "Strong"], default: 0 };
pub const DC_FILTER: BoolSpec = BoolSpec { id: "dc_filter", name: "DC Filter", default: false };

// ---- Loudness (slice 2) -------------------------------------------------------------------

/// The platform targets (frozen order, append only; brief §3.3).
pub const TARGETS: [&str; 9] = ["Spotify", "Apple Music", "YouTube", "Tidal", "Amazon Music", "Deezer", "EBU R128", "ATSC A/85", "Custom"];
pub const TARGET: ChoiceSpec = ChoiceSpec { id: "target", name: "Target", choices: &TARGETS, default: 0 };
pub const TARGET_CUSTOM: ParamSpec = spec("target_custom", "Custom Target", "LUFS", -30.0, -5.0, -14.0, ParamScale::Linear, 1, (0.5, 0.1), ParamDisplay::Number);
/// Monitoring: hear what the target platform will do.
pub const PREVIEW: BoolSpec = BoolSpec { id: "preview", name: "Platform Preview", default: false };

/// Dials: common values get more travel.
pub const LOOKAHEAD_DIAL: DialCurve = DialCurve(&[(0.0, 0.0), (0.3, 1.0), (0.6, 3.0), (0.85, 6.0), (1.0, 10.0)]);
pub const ATTACK_DIAL: DialCurve = DialCurve(&[(0.0, 0.1), (0.25, 3.0), (0.55, 30.0), (1.0, 100.0)]);
pub const RELEASE_DIAL: DialCurve = DialCurve(&[(0.0, 1.0), (0.2, 20.0), (0.45, 100.0), (0.8, 600.0), (1.0, 2500.0)]);

/// Every parameter's manifest line, in host order.
pub fn manifest() -> String {
    let mut lines = vec![GAIN.manifest_line(), CEILING.manifest_line(), STYLE.manifest_line()];
    lines.extend([LOOKAHEAD, ATTACK, RELEASE, SLOW, LINK_T, LINK_R].iter().map(ParamSpec::manifest_line));
    lines.push(TRUE_PEAK.manifest_line());
    lines.push(OVERSAMPLING.manifest_line());
    lines.push(CLIP_ON.manifest_line());
    lines.push(CLIP_LEVEL.manifest_line());
    lines.push(CLIP_CURVE.manifest_line());
    lines.push(PROTECT.manifest_line());
    lines.extend([UNITY, DELTA, BYPASS].iter().map(BoolSpec::manifest_line));
    lines.push(DITHER.manifest_line());
    lines.push(SHAPING.manifest_line());
    lines.push(DC_FILTER.manifest_line());
    lines.push(TARGET.manifest_line());
    lines.push(TARGET_CUSTOM.manifest_line());
    lines.push(PREVIEW.manifest_line());
    lines.join("\n") + "\n"
}

/// Every parameter id, in host order.
pub fn ids() -> Vec<String> {
    manifest().lines().filter_map(|l| l.split(" | ").next()).map(str::to_owned).collect()
}

/// Parameters that are monitoring or metering, not sound: never in presets or snapshots.
pub fn is_monitoring(id: &str) -> bool {
    [UNITY.id, DELTA.id, BYPASS.id, PREVIEW.id, TARGET.id, TARGET_CUSTOM.id].contains(&id)
}

/// The sound parameters: what presets, snapshots and undo cover.
pub fn sound_ids() -> Vec<String> {
    ids().into_iter().filter(|id| !is_monitoring(id)).collect()
}

/// The oversampling factor of a choice index.
pub fn factor(index: i32) -> usize {
    1 << index.clamp(0, 4)
}

pub struct LimitParams {
    pub gain: FloatParam,
    pub ceiling: FloatParam,
    pub style: IntParam,
    pub lookahead: FloatParam,
    pub attack: FloatParam,
    pub release: FloatParam,
    pub slow: FloatParam,
    pub link_t: FloatParam,
    pub link_r: FloatParam,
    pub true_peak: BoolParam,
    pub oversampling: IntParam,
    pub clip_on: BoolParam,
    pub clip_level: FloatParam,
    pub clip_curve: IntParam,
    pub protect: FloatParam,
    pub unity: BoolParam,
    pub delta: BoolParam,
    pub bypass: BoolParam,
    pub dither: IntParam,
    pub shaping: IntParam,
    pub dc_filter: BoolParam,
    pub target: IntParam,
    pub target_custom: FloatParam,
    pub preview: BoolParam,
    /// Snapshots A–D and the current preset's name (saved with the state).
    pub snapshots: std::sync::Arc<Snapshots>,
    pub preset: std::sync::Arc<CurrentPreset>,
    /// The history's level range picked by hand (0: Auto), saved with the state.
    pub display_range: std::sync::Arc<mite_core::AtomicF32>,
}

impl Default for LimitParams {
    fn default() -> Self {
        let named = |spec: &'static ChoiceSpec| choice_param(spec, spec.name.to_owned());
        let flag = |spec: &'static BoolSpec| bool_param(spec, spec.name.to_owned());
        Self {
            gain: float_param(&GAIN),
            ceiling: float_param(&CEILING),
            style: named(&STYLE),
            lookahead: float_param(&LOOKAHEAD),
            attack: float_param(&ATTACK),
            release: float_param(&RELEASE),
            slow: float_param(&SLOW),
            link_t: float_param(&LINK_T),
            link_r: float_param(&LINK_R),
            true_peak: flag(&TRUE_PEAK),
            oversampling: named(&OVERSAMPLING),
            clip_on: flag(&CLIP_ON),
            clip_level: float_param(&CLIP_LEVEL),
            clip_curve: named(&CLIP_CURVE),
            protect: float_param(&PROTECT),
            unity: flag(&UNITY),
            delta: flag(&DELTA),
            // Not the host's bypass (`make_bypass`): REAPER stops processing a bypassed plugin, so
            // a change from the editor never reached it and bypass could not be turned off (user,
            // 2026-11-03). An ordinary parameter keeps processing, so every change lands.
            bypass: flag(&BYPASS),
            dither: named(&DITHER),
            shaping: named(&SHAPING),
            dc_filter: flag(&DC_FILTER),
            target: named(&TARGET).non_automatable(),
            target_custom: float_param(&TARGET_CUSTOM),
            preview: flag(&PREVIEW),
            snapshots: std::sync::Arc::new(Snapshots::new()),
            preset: std::sync::Arc::new(CurrentPreset::default()),
            display_range: std::sync::Arc::new(mite_core::AtomicF32::new(0.0)),
        }
    }
}

impl LimitParams {
    pub fn settings(&self) -> LimitSettings {
        LimitSettings {
            gain_db: self.gain.value(),
            ceiling_db: self.ceiling.value(),
            style: self.style.value().max(0) as usize,
            lookahead_ms: self.lookahead.value(),
            attack_ms: self.attack.value(),
            release_ms: self.release.value(),
            slow: self.slow.value() / 100.0,
            link_transients: self.link_t.value() / 100.0,
            link_release: self.link_r.value() / 100.0,
            true_peak: self.true_peak.value(),
            oversampling: factor(self.oversampling.value()),
            delta: self.delta.value(),
            bypass: self.bypass.value(),
            target: self.target.value().max(0) as usize,
            target_custom: self.target_custom.value(),
            preview: self.preview.value(),
        }
    }
}

// SAFETY: every `ParamPtr` points into `self`, which nih-plug keeps alive (in an `Arc`) for as
// long as it uses the map. The ids match `manifest()` (checked by tests/params.rs).
unsafe impl Params for LimitParams {
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
        vec![
            p(GAIN.id, self.gain.as_ptr(), ""),
            p(CEILING.id, self.ceiling.as_ptr(), ""),
            p(STYLE.id, self.style.as_ptr(), "Limiter"),
            p(LOOKAHEAD.id, self.lookahead.as_ptr(), "Limiter"),
            p(ATTACK.id, self.attack.as_ptr(), "Limiter"),
            p(RELEASE.id, self.release.as_ptr(), "Limiter"),
            p(SLOW.id, self.slow.as_ptr(), "Limiter"),
            p(LINK_T.id, self.link_t.as_ptr(), "Limiter"),
            p(LINK_R.id, self.link_r.as_ptr(), "Limiter"),
            p(TRUE_PEAK.id, self.true_peak.as_ptr(), "Limiter"),
            p(OVERSAMPLING.id, self.oversampling.as_ptr(), "Limiter"),
            p(CLIP_ON.id, self.clip_on.as_ptr(), "Clipper"),
            p(CLIP_LEVEL.id, self.clip_level.as_ptr(), "Clipper"),
            p(CLIP_CURVE.id, self.clip_curve.as_ptr(), "Clipper"),
            p(PROTECT.id, self.protect.as_ptr(), "Limiter"),
            p(UNITY.id, self.unity.as_ptr(), "Output"),
            p(DELTA.id, self.delta.as_ptr(), "Output"),
            p(BYPASS.id, self.bypass.as_ptr(), ""),
            p(DITHER.id, self.dither.as_ptr(), "Output"),
            p(SHAPING.id, self.shaping.as_ptr(), "Output"),
            p(DC_FILTER.id, self.dc_filter.as_ptr(), "Output"),
            p(TARGET.id, self.target.as_ptr(), "Loudness"),
            p(TARGET_CUSTOM.id, self.target_custom.as_ptr(), "Loudness"),
            p(PREVIEW.id, self.preview.as_ptr(), "Loudness"),
        ]
    }
}
