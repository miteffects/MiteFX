//! Mite DeEss parameters (docs/design/deess-brief.md §3).
//!
//! IDs are frozen once released (`tests/golden/mite_deess.params.txt`).

use mite_core::presets::CurrentPreset;
use mite_core::snapshots::Snapshots;
use mite_core::{BoolSpec, ChoiceSpec, ParamDisplay, ParamScale, ParamSpec};
use mite_dsp::deess::{DeEssSettings, Detection, Reduce};
use mite_plugin::{bool_param, choice_param, float_param};
use nih_plug::prelude::*;

/// The state format version (`mite_plugin::state`).
///
/// - 2 (2026-11-05): REDUCE gained Spectral; [`crate::migrate::v1_to_v2`] re-normalises the
///   choice in stored snapshots.
pub const STATE_VERSION: u32 = 2;

#[allow(clippy::too_many_arguments)] // one line per parameter reads best
const fn spec(id: &'static str, name: &'static str, unit: &'static str, min: f32, max: f32, default: f32, scale: ParamScale, decimals: u8, wheel: (f32, f32), display: ParamDisplay) -> ParamSpec {
    ParamSpec { id, name, unit, min, max, default, scale, smoothing_ms: 20.0, decimals, wheel_step: wheel.0, wheel_step_fine: wheel.1, display }
}

pub const THRESHOLD: ParamSpec = spec("threshold", "Threshold", "dB", -60.0, 0.0, -15.0, ParamScale::Linear, 1, (0.5, 0.1), ParamDisplay::Number);
pub const DETECTION: ChoiceSpec = ChoiceSpec { id: "detection", name: "Detection", choices: &Detection::NAMES, default: 0 };
pub const RANGE: ParamSpec = spec("range", "Range", "dB", 0.0, 24.0, 12.0, ParamScale::Linear, 1, (0.5, 0.1), ParamDisplay::Number);
pub const RELEASE: ParamSpec = spec("release", "Release", "ms", 10.0, 500.0, 60.0, ParamScale::Log, 0, (1.0 / 24.0, 1.0 / 96.0), ParamDisplay::Time);
pub const LO: ParamSpec = spec("lo", "Band Low", "Hz", 1000.0, 16_000.0, 5000.0, ParamScale::Log, 0, (1.0 / 24.0, 1.0 / 96.0), ParamDisplay::Frequency);
pub const HI: ParamSpec = spec("hi", "Band High", "Hz", 2000.0, 20_000.0, 12_000.0, ParamScale::Log, 0, (1.0 / 24.0, 1.0 / 96.0), ParamDisplay::Frequency);
pub const REDUCE: ChoiceSpec = ChoiceSpec { id: "reduce", name: "Reduce", choices: &Reduce::NAMES, default: 0 };
pub const LOOKAHEAD: BoolSpec = BoolSpec { id: "lookahead", name: "Lookahead", default: true };
pub const LISTEN: BoolSpec = BoolSpec { id: "listen", name: "Listen", default: false };
pub const OUT_GAIN: ParamSpec = spec("out_gain", "Output", "dB", -24.0, 24.0, 0.0, ParamScale::Linear, 1, (0.5, 0.1), ParamDisplay::Number);
pub const BYPASS: BoolSpec = BoolSpec { id: "bypass", name: "Bypass", default: false };

const CONTINUOUS: [ParamSpec; 6] = [THRESHOLD, RANGE, RELEASE, LO, HI, OUT_GAIN];

pub fn manifest() -> String {
    [
        THRESHOLD.manifest_line(),
        DETECTION.manifest_line(),
        RANGE.manifest_line(),
        RELEASE.manifest_line(),
        LO.manifest_line(),
        HI.manifest_line(),
        REDUCE.manifest_line(),
        LOOKAHEAD.manifest_line(),
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

pub struct DeEssParams {
    pub threshold: FloatParam,
    pub detection: IntParam,
    pub range: FloatParam,
    pub release: FloatParam,
    pub lo: FloatParam,
    pub hi: FloatParam,
    pub reduce: IntParam,
    pub lookahead: BoolParam,
    pub listen: BoolParam,
    pub out_gain: FloatParam,
    pub bypass: BoolParam,
    pub snapshots: std::sync::Arc<Snapshots>,
    pub preset: std::sync::Arc<CurrentPreset>,
}

impl Default for DeEssParams {
    fn default() -> Self {
        Self {
            threshold: float_param(&THRESHOLD),
            detection: choice_param(&DETECTION, DETECTION.name.to_owned()),
            range: float_param(&RANGE),
            release: float_param(&RELEASE),
            lo: float_param(&LO),
            hi: float_param(&HI),
            reduce: choice_param(&REDUCE, REDUCE.name.to_owned()),
            lookahead: bool_param(&LOOKAHEAD, LOOKAHEAD.name.to_owned()),
            listen: bool_param(&LISTEN, LISTEN.name.to_owned()),
            out_gain: float_param(&OUT_GAIN),
            // An ordinary parameter, never the host's bypass (REAPER stops processing a
            // host-bypassed plugin, and then the editor could not turn it off).
            bypass: bool_param(&BYPASS, BYPASS.name.to_owned()),
            snapshots: std::sync::Arc::new(Snapshots::new()),
            preset: std::sync::Arc::new(CurrentPreset::default()),
        }
    }
}

/// The engine's settings from plain values by id.
pub fn deess_settings(p: impl Fn(&str) -> f32) -> DeEssSettings {
    let index = |v: f32| v.round().max(0.0) as usize;
    DeEssSettings {
        threshold_db: p(THRESHOLD.id),
        detection: Detection::from_index(index(p(DETECTION.id))),
        range_db: p(RANGE.id),
        release_ms: p(RELEASE.id),
        lo_hz: p(LO.id),
        hi_hz: p(HI.id),
        reduce: Reduce::from_index(index(p(REDUCE.id))),
        lookahead: p(LOOKAHEAD.id) >= 0.5,
        listen: p(LISTEN.id) >= 0.5,
        bypass: p(BYPASS.id) >= 0.5,
    }
}

impl DeEssParams {
    fn plain(&self, id: &str) -> f32 {
        let flag = |p: &BoolParam| f32::from(u8::from(p.value()));
        match id {
            "threshold" => self.threshold.value(),
            "detection" => self.detection.value() as f32,
            "range" => self.range.value(),
            "release" => self.release.value(),
            "lo" => self.lo.value(),
            "hi" => self.hi.value(),
            "reduce" => self.reduce.value() as f32,
            "lookahead" => flag(&self.lookahead),
            "listen" => flag(&self.listen),
            "bypass" => flag(&self.bypass),
            _ => 0.0,
        }
    }

    pub fn settings(&self) -> DeEssSettings {
        deess_settings(|id| self.plain(id))
    }
}

// SAFETY: every `ParamPtr` points into `self`, which nih-plug keeps alive (in an `Arc`) for as
// long as it uses the map. The ids match `manifest()` (checked by tests/params.rs).
unsafe impl Params for DeEssParams {
    fn serialize_fields(&self) -> std::collections::BTreeMap<String, String> {
        let live = mite_plugin::state::live_values(self, &sound_ids());
        mite_plugin::state::session_fields(STATE_VERSION, &self.snapshots, &self.preset, live)
    }

    fn deserialize_fields(&self, serialized: &std::collections::BTreeMap<String, String>) {
        mite_plugin::state::load_session_fields(serialized, &self.snapshots, &self.preset);
    }

    fn param_map(&self) -> Vec<(String, ParamPtr, String)> {
        let p = |id: &str, ptr: ParamPtr, group: &str| (id.to_owned(), ptr, group.to_owned());
        vec![
            p(THRESHOLD.id, self.threshold.as_ptr(), "Detect"),
            p(DETECTION.id, self.detection.as_ptr(), "Detect"),
            p(RANGE.id, self.range.as_ptr(), "Reduce"),
            p(RELEASE.id, self.release.as_ptr(), "Reduce"),
            p(LO.id, self.lo.as_ptr(), "Band"),
            p(HI.id, self.hi.as_ptr(), "Band"),
            p(REDUCE.id, self.reduce.as_ptr(), "Reduce"),
            p(LOOKAHEAD.id, self.lookahead.as_ptr(), ""),
            p(LISTEN.id, self.listen.as_ptr(), ""),
            p(OUT_GAIN.id, self.out_gain.as_ptr(), ""),
            p(BYPASS.id, self.bypass.as_ptr(), ""),
        ]
    }
}
