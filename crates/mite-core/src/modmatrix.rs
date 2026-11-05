//! The modulation matrix's parameters, shared by Mite Delay, Mite Sat and Mite Mod (BLUEPRINT §6;
//! first frozen in Sat's manifest, 2026-10-26): two LFOs (`lfo{N}_rate`, `_sync`, `_div`,
//! `_shape`, `_phase`), an envelope follower (`env_att`, `env_rel`, `env_gain`, and `env_src`
//! where the plugin has a sidechain), and eight slots (`m{N}_src`, `_dst`, `_amt`). A slot's
//! target (`dst`) indexes the plugin's own frozen, append-only target table; `src` and `dst` are
//! not automatable, so a growing table never moves host automation.
//!
//! A slot moves its target as a hand on its knob would: the amount is a share of the knob's whole
//! travel (its normalised range), added to where the knob sits and held inside it
//! ([`modulate`]).

use crate::{BoolSpec, ChoiceSpec, ParamDisplay, ParamScale, ParamSpec};

#[allow(clippy::too_many_arguments)]
const fn spec(id: &'static str, name: &'static str, unit: &'static str, min: f32, max: f32, default: f32, scale: ParamScale, decimals: u8, wheel: (f32, f32), display: ParamDisplay) -> ParamSpec {
    ParamSpec { id, name, unit, min, max, default, scale, smoothing_ms: 20.0, decimals, wheel_step: wheel.0, wheel_step_fine: wheel.1, display }
}

pub const LFOS: usize = 2;
pub const MOD_SLOTS: usize = 8;

/// An LFO's parameters (ids without the `lfo{N}_` prefix).
pub const LFO_RATE: ParamSpec = spec("rate", "Rate", "Hz", 0.02, 40.0, 1.0, ParamScale::Log, 2, (0.25, 1.0 / 16.0), ParamDisplay::Number);
pub const LFO_SYNC: BoolSpec = BoolSpec { id: "sync", name: "Sync", default: false };
pub const LFO_DIV: ChoiceSpec = ChoiceSpec {
    id: "div",
    name: "Division",
    choices: &["1/32", "1/16T", "1/16", "1/16D", "1/8T", "1/8", "1/8D", "1/4T", "1/4", "1/4D", "1/2T", "1/2", "1/2D", "1 bar", "2 bars", "4 bars"],
    default: 8,
};
/// Each division's length, quarter notes (in [`LFO_DIV`]'s order).
pub const DIV_QUARTERS: [f64; 16] = [0.125, 1.0 / 6.0, 0.25, 0.375, 1.0 / 3.0, 0.5, 0.75, 2.0 / 3.0, 1.0, 1.5, 4.0 / 3.0, 2.0, 3.0, 4.0, 8.0, 16.0];
pub const LFO_SHAPE: ChoiceSpec = ChoiceSpec { id: "shape", name: "Shape", choices: &["Sine", "Triangle", "Saw up", "Saw down", "Square", "Smooth random", "Sample & hold"], default: 0 };
pub const LFO_PHASE: ParamSpec = spec("phase", "Phase", "°", 0.0, 360.0, 0.0, ParamScale::Linear, 0, (15.0, 1.0), ParamDisplay::Number);
pub const ENV_ATT: ParamSpec = spec("env_att", "Envelope Attack", "ms", 0.1, 500.0, 10.0, ParamScale::Log, 1, (0.25, 1.0 / 16.0), ParamDisplay::Time);
pub const ENV_REL: ParamSpec = spec("env_rel", "Envelope Release", "ms", 5.0, 2000.0, 200.0, ParamScale::Log, 0, (0.25, 1.0 / 16.0), ParamDisplay::Time);
pub const ENV_GAIN: ParamSpec = spec("env_gain", "Envelope Gain", "dB", -24.0, 24.0, 0.0, ParamScale::Linear, 1, (0.5, 0.1), ParamDisplay::Number);
/// Only where the plugin has a sidechain input.
pub const ENV_SRC: ChoiceSpec = ChoiceSpec { id: "env_src", name: "Envelope Source", choices: &["Input", "Sidechain"], default: 0 };
pub const M_SRC: ChoiceSpec = ChoiceSpec { id: "src", name: "Source", choices: &["Off", "LFO 1", "LFO 2", "Envelope"], default: 0 };
pub const M_AMT: ParamSpec = spec("amt", "Amount", "%", -100.0, 100.0, 0.0, ParamScale::Linear, 0, (5.0, 1.0), ParamDisplay::Number);

/// The continuous matrix parameters by full id (for stand-in hosts and the editor).
pub fn spec_for(id: &str) -> Option<ParamSpec> {
    if let Some(rest) = id.strip_prefix("lfo").and_then(|r| r.split_once('_')).map(|(_, p)| p) {
        return [LFO_RATE, LFO_PHASE].into_iter().find(|s| s.id == rest).map(|s| ParamSpec { id: leak(id), ..s });
    }
    if [ENV_ATT, ENV_REL, ENV_GAIN].iter().any(|s| s.id == id) {
        return [ENV_ATT, ENV_REL, ENV_GAIN].into_iter().find(|s| s.id == id);
    }
    if id.starts_with('m') && id.ends_with("_amt") && id[1..id.len() - 4].parse::<usize>().is_ok() {
        return Some(ParamSpec { id: leak(id), ..M_AMT });
    }
    None
}

fn leak(s: &str) -> &'static str {
    // Ids are few and fixed; interning them once keeps `ParamSpec` 'static.
    use std::sync::{Mutex, OnceLock};
    static IDS: OnceLock<Mutex<Vec<&'static str>>> = OnceLock::new();
    let ids = IDS.get_or_init(|| Mutex::new(Vec::new()));
    let mut ids = ids.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(found) = ids.iter().find(|k| **k == s) {
        return found;
    }
    let leaked: &'static str = Box::leak(s.to_owned().into_boxed_str());
    ids.push(leaked);
    leaked
}

/// An LFO parameter's host id, e.g. `lfo2_rate` (`lfo` from 0).
pub fn lfo_id(lfo: usize, param: &str) -> String {
    format!("lfo{}_{param}", lfo + 1)
}

/// A slot parameter's host id, e.g. `m3_amt` (`slot` from 0).
pub fn mod_id(slot: usize, param: &str) -> String {
    format!("m{}_{param}", slot + 1)
}

/// The matrix's manifest lines, in host order: the LFOs, the envelope (with `env_src` when
/// `sidechain`), the slots with `dst` choosing from `targets`.
pub fn manifest_lines(targets: &ChoiceSpec, sidechain: bool) -> Vec<String> {
    let mut lines = Vec::new();
    for l in 0..LFOS {
        let prefix = |line: String| format!("lfo{}_{line}", l + 1);
        lines.push(prefix(LFO_RATE.manifest_line()));
        lines.push(prefix(LFO_SYNC.manifest_line()));
        lines.push(prefix(LFO_DIV.manifest_line()));
        lines.push(prefix(LFO_SHAPE.manifest_line()));
        lines.push(prefix(LFO_PHASE.manifest_line()));
    }
    lines.extend([ENV_ATT, ENV_REL, ENV_GAIN].iter().map(ParamSpec::manifest_line));
    if sidechain {
        lines.push(ENV_SRC.manifest_line());
    }
    for m in 0..MOD_SLOTS {
        let prefix = |line: String| format!("m{}_{line}", m + 1);
        lines.push(prefix(M_SRC.manifest_line()));
        lines.push(prefix(targets.manifest_line()));
        lines.push(prefix(M_AMT.manifest_line()));
    }
    lines
}

/// `plain` moved by `offset` (a share of the knob's travel, −1…1), held inside the range.
#[inline]
pub fn modulate(spec: &ParamSpec, plain: f32, offset: f32) -> f32 {
    if offset == 0.0 {
        plain
    } else {
        spec.denormalize((spec.normalize(plain) + offset).clamp(0.0, 1.0))
    }
}

/// A plugin's targets: names (the `dst` choices, "None" first) and the parameter each moves.
#[derive(Debug, Clone, Copy)]
pub struct Targets {
    pub names: &'static [&'static str],
    pub ids: &'static [&'static str],
}

impl Targets {
    /// The target index a parameter id is, if any.
    pub fn index(&self, id: &str) -> Option<usize> {
        self.ids.iter().skip(1).position(|t| *t == id).map(|i| i + 1)
    }
}

/// Targets at most (a plugin's table, "None" included).
pub const MAX_TARGETS: usize = 64;

/// The matrix as the audio thread left it, for the editor: each source's value (the LFOs
/// −1…1, the envelope 0…1), each LFO's place (whole cycles mod 2²⁰ and the fraction: Random's
/// values are the cycle's own), and each target's offset (a share of its knob's travel).
pub struct MatrixTap {
    pub sources: [crate::AtomicF32; 3],
    pub cycles: [crate::AtomicF32; LFOS],
    pub phases: [crate::AtomicF32; LFOS],
    pub rates: [crate::AtomicF32; LFOS],
    pub offsets: [crate::AtomicF32; MAX_TARGETS],
}

impl MatrixTap {
    pub fn new() -> Self {
        let z = || crate::AtomicF32::new(0.0);
        Self { sources: std::array::from_fn(|_| z()), cycles: std::array::from_fn(|_| z()), phases: std::array::from_fn(|_| z()), rates: std::array::from_fn(|_| z()), offsets: std::array::from_fn(|_| z()) }
    }

    pub fn offset(&self, target: usize) -> f32 {
        self.offsets.get(target).map_or(0.0, crate::AtomicF32::load)
    }
}

impl Default for MatrixTap {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modulation_moves_along_the_knob_and_stays_inside() {
        let s = spec("x", "X", "Hz", 20.0, 20_000.0, 1000.0, ParamScale::Log, 0, (0.1, 0.01), ParamDisplay::Frequency);
        assert_eq!(modulate(&s, 1000.0, 0.0), 1000.0);
        // A tenth of the travel up a log knob: ×2 for 20…20k's three decades… ×10^0.3.
        assert!((modulate(&s, 1000.0, 0.1) - 1000.0 * 10f32.powf(0.3)).abs() < 1.0);
        assert_eq!(modulate(&s, 1000.0, 1.0), 20_000.0);
        assert_eq!(modulate(&s, 1000.0, -1.0), 20.0);
        assert_eq!(spec_for("lfo2_rate").map(|s| s.id), Some("lfo2_rate"));
        assert_eq!(spec_for("m8_amt").map(|s| s.max), Some(100.0));
        assert_eq!(spec_for("env_gain").map(|s| s.id), Some("env_gain"));
        assert!(spec_for("lfo1_sync").is_none());
    }
}
