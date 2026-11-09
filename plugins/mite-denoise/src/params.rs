//! Mite DeNoise parameters (docs/design/denoise-brief.md §3).
//!
//! IDs are frozen once released (`tests/golden/mite_denoise.params.txt`). The learned noise
//! profile is not a parameter: it is saved with the state as a field (`noise_profile`).

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use mite_core::presets::CurrentPreset;
use mite_core::snapshots::Snapshots;
use mite_core::{AtomicF32, BoolSpec, DialCurve, ParamDisplay, ParamScale, ParamSpec};
use mite_dsp::denoise::{DenoiseSettings, PROFILE_POINTS, SILENT_DB};
use mite_plugin::{bool_param, float_param};
use nih_plug::prelude::*;

/// The state format version (`mite_plugin::state`).
pub const STATE_VERSION: u32 = 1;

/// The state field holding the learned profile.
pub const PROFILE_KEY: &str = "noise_profile";

#[allow(clippy::too_many_arguments)] // one line per parameter reads best
const fn spec(id: &'static str, name: &'static str, unit: &'static str, min: f32, max: f32, default: f32, scale: ParamScale, decimals: u8, wheel: (f32, f32), display: ParamDisplay) -> ParamSpec {
    ParamSpec { id, name, unit, min, max, default, scale, smoothing_ms: 0.0, decimals, wheel_step: wheel.0, wheel_step_fine: wheel.1, display }
}

/// How far above the noise floor a sound passes untouched.
pub const THRESHOLD: ParamSpec = spec("threshold", "Threshold", "dB", -10.0, 30.0, 6.0, ParamScale::Linear, 1, (0.5, 0.1), ParamDisplay::Number);
/// The most any frequency is turned down.
pub const REDUCTION: ParamSpec = spec("reduction", "Reduction", "dB", 0.0, 60.0, 12.0, ParamScale::Linear, 1, (0.5, 0.1), ParamDisplay::Number);
pub const ATTACK: ParamSpec = spec("attack", "Attack", "ms", 1.0, 200.0, 10.0, ParamScale::Log, 0, (1.0 / 24.0, 1.0 / 96.0), ParamDisplay::Time);
pub const RELEASE: ParamSpec = spec("release", "Release", "ms", 10.0, 2000.0, 150.0, ParamScale::Log, 0, (1.0 / 24.0, 1.0 / 96.0), ParamDisplay::Time);
/// Across frequencies: 0 each bin alone, 100 a third of an octave.
pub const SMOOTHING: ParamSpec = spec("smoothing", "Smoothing", "%", 0.0, 100.0, 40.0, ParamScale::Linear, 0, (1.0, 0.1), ParamDisplay::Number);
/// The threshold's tilt about 1 kHz, dB at 10 kHz.
pub const EMPHASIS: ParamSpec = spec("emphasis", "Emphasis", "dB", -12.0, 12.0, 0.0, ParamScale::Linear, 1, (0.5, 0.1), ParamDisplay::Number);
pub const ADAPTIVE: BoolSpec = BoolSpec { id: "adaptive", name: "Adaptive", default: true };
pub const LISTEN: BoolSpec = BoolSpec { id: "listen", name: "Noise Only", default: false };
pub const OUT_GAIN: ParamSpec = ParamSpec { smoothing_ms: 20.0, ..spec("out_gain", "Output", "dB", -24.0, 24.0, 0.0, ParamScale::Linear, 1, (0.5, 0.1), ParamDisplay::Number) };
pub const BYPASS: BoolSpec = BoolSpec { id: "bypass", name: "Bypass", default: false };

pub const ATTACK_DIAL: DialCurve = DialCurve(&[(0.0, 1.0), (0.4, 10.0), (0.75, 50.0), (1.0, 200.0)]);
pub const RELEASE_DIAL: DialCurve = DialCurve(&[(0.0, 10.0), (0.35, 100.0), (0.6, 250.0), (1.0, 2000.0)]);

const CONTINUOUS: [ParamSpec; 7] = [THRESHOLD, REDUCTION, ATTACK, RELEASE, SMOOTHING, EMPHASIS, OUT_GAIN];

pub fn manifest() -> String {
    [
        THRESHOLD.manifest_line(),
        REDUCTION.manifest_line(),
        ATTACK.manifest_line(),
        RELEASE.manifest_line(),
        SMOOTHING.manifest_line(),
        EMPHASIS.manifest_line(),
        ADAPTIVE.manifest_line(),
        LISTEN.manifest_line(),
        OUT_GAIN.manifest_line(),
        BYPASS.manifest_line(),
    ]
    .join("\n")
        + "\n"
}

pub fn ids() -> Vec<String> {
    manifest().lines().filter_map(|l| l.split(" | ").next()).map(str::to_owned).collect()
}

/// Monitoring, not sound: never in presets or snapshots.
pub fn is_monitoring(id: &str) -> bool {
    [LISTEN.id, BYPASS.id].contains(&id)
}

pub fn sound_ids() -> Vec<String> {
    ids().into_iter().filter(|id| !is_monitoring(id)).collect()
}

pub fn spec_for(id: &str) -> Option<ParamSpec> {
    CONTINUOUS.iter().find(|s| s.id == id).copied()
}

/// The learned noise profile, shared by the engine (which learns it and reads it back after a
/// state load), the editor and the state: dB per Hz at `PROFILE_POINTS` log-spaced frequencies
/// (all `SILENT_DB`: none). `version` moves on every change, so each reader knows when to look.
pub struct NoiseProfile {
    pub points: Vec<AtomicF32>,
    pub version: AtomicU64,
}

impl NoiseProfile {
    pub fn new() -> Self {
        Self { points: (0..PROFILE_POINTS).map(|_| AtomicF32::new(SILENT_DB)).collect(), version: AtomicU64::new(0) }
    }

    pub fn is_learned(&self) -> bool {
        self.points.iter().any(|p| p.load() > SILENT_DB + 1.0)
    }

    /// Stores `values` and moves the version on. Returns the new version.
    pub fn store(&self, values: &[f32]) -> u64 {
        for (p, &v) in self.points.iter().zip(values) {
            p.store(v);
        }
        self.version.fetch_add(1, Ordering::AcqRel) + 1
    }

    pub fn to_json(&self) -> Option<String> {
        if !self.is_learned() {
            return None;
        }
        let rounded: Vec<f32> = self.points.iter().map(|p| (p.load() * 100.0).round() / 100.0).collect();
        serde_json::to_string(&rounded).ok()
    }

    pub fn load_json(&self, json: &str) {
        if let Ok(v) = serde_json::from_str::<Vec<f32>>(json) {
            if v.len() == PROFILE_POINTS && v.iter().all(|x| x.is_finite()) {
                self.store(&v);
            }
        }
    }

    pub fn clear(&self) {
        self.store(&[SILENT_DB; PROFILE_POINTS]);
    }
}

impl Default for NoiseProfile {
    fn default() -> Self {
        Self::new()
    }
}

pub struct DenoiseParams {
    pub threshold: FloatParam,
    pub reduction: FloatParam,
    pub attack: FloatParam,
    pub release: FloatParam,
    pub smoothing: FloatParam,
    pub emphasis: FloatParam,
    pub adaptive: BoolParam,
    pub listen: BoolParam,
    pub out_gain: FloatParam,
    pub bypass: BoolParam,
    pub snapshots: Arc<Snapshots>,
    pub preset: Arc<CurrentPreset>,
    pub profile: Arc<NoiseProfile>,
}

impl Default for DenoiseParams {
    fn default() -> Self {
        let flag = |s: &'static BoolSpec| bool_param(s, s.name.to_owned());
        Self {
            threshold: float_param(&THRESHOLD),
            reduction: float_param(&REDUCTION),
            attack: float_param(&ATTACK),
            release: float_param(&RELEASE),
            smoothing: float_param(&SMOOTHING),
            emphasis: float_param(&EMPHASIS),
            adaptive: flag(&ADAPTIVE),
            listen: flag(&LISTEN),
            out_gain: float_param(&OUT_GAIN),
            // An ordinary parameter, never the host's bypass (REAPER stops processing a
            // host-bypassed plugin, and then the editor could not turn it off).
            bypass: flag(&BYPASS),
            snapshots: Arc::new(Snapshots::new()),
            preset: Arc::new(CurrentPreset::default()),
            profile: Arc::new(NoiseProfile::new()),
        }
    }
}

/// The engine's settings from plain values by id.
pub fn denoise_settings(p: impl Fn(&str) -> f32) -> DenoiseSettings {
    DenoiseSettings {
        threshold_db: p(THRESHOLD.id),
        reduction_db: p(REDUCTION.id),
        attack_ms: p(ATTACK.id),
        release_ms: p(RELEASE.id),
        smoothing: p(SMOOTHING.id) / 100.0,
        emphasis_db: p(EMPHASIS.id),
        adaptive: p(ADAPTIVE.id) >= 0.5,
        listen: p(LISTEN.id) >= 0.5,
        bypass: p(BYPASS.id) >= 0.5,
    }
}

impl DenoiseParams {
    fn plain(&self, id: &str) -> f32 {
        let flag = |p: &BoolParam| f32::from(u8::from(p.value()));
        match id {
            "threshold" => self.threshold.value(),
            "reduction" => self.reduction.value(),
            "attack" => self.attack.value(),
            "release" => self.release.value(),
            "smoothing" => self.smoothing.value(),
            "emphasis" => self.emphasis.value(),
            "adaptive" => flag(&self.adaptive),
            "listen" => flag(&self.listen),
            "bypass" => flag(&self.bypass),
            _ => 0.0,
        }
    }

    pub fn settings(&self) -> DenoiseSettings {
        denoise_settings(|id| self.plain(id))
    }
}

// SAFETY: every `ParamPtr` points into `self`, which nih-plug keeps alive (in an `Arc`) for as
// long as it uses the map. The ids match `manifest()` (checked by tests/params.rs).
unsafe impl Params for DenoiseParams {
    fn serialize_fields(&self) -> std::collections::BTreeMap<String, String> {
        let live = mite_plugin::state::live_values(self, &sound_ids());
        let mut fields = mite_plugin::state::session_fields(STATE_VERSION, &self.snapshots, &self.preset, live);
        if let Some(json) = self.profile.to_json() {
            fields.insert(PROFILE_KEY.to_owned(), json);
        }
        fields
    }

    fn deserialize_fields(&self, serialized: &std::collections::BTreeMap<String, String>) {
        mite_plugin::state::load_session_fields(serialized, &self.snapshots, &self.preset);
        match serialized.get(PROFILE_KEY) {
            Some(json) => self.profile.load_json(json),
            None => self.profile.clear(),
        }
    }

    fn param_map(&self) -> Vec<(String, ParamPtr, String)> {
        let p = |id: &str, ptr: ParamPtr, group: &str| (id.to_owned(), ptr, group.to_owned());
        vec![
            p(THRESHOLD.id, self.threshold.as_ptr(), "Detect"),
            p(REDUCTION.id, self.reduction.as_ptr(), "Reduce"),
            p(ATTACK.id, self.attack.as_ptr(), "Reduce"),
            p(RELEASE.id, self.release.as_ptr(), "Reduce"),
            p(SMOOTHING.id, self.smoothing.as_ptr(), "Reduce"),
            p(EMPHASIS.id, self.emphasis.as_ptr(), "Detect"),
            p(ADAPTIVE.id, self.adaptive.as_ptr(), "Detect"),
            p(LISTEN.id, self.listen.as_ptr(), ""),
            p(OUT_GAIN.id, self.out_gain.as_ptr(), ""),
            p(BYPASS.id, self.bypass.as_ptr(), ""),
        ]
    }
}
