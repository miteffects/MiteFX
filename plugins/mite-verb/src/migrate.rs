//! State migrations (`mite_plugin::state::migrate`, from `Plugin::filter_state`).
//!
//! **1 → 2 (2026-11-04):** the early field's size and send and four decay bells appended; a
//! version 1 state gets their defaults (bells off). Nothing to change.
//!
//! **2 → 3 (2026-11-04): both EQs are free bands.** Version 2 had a fixed low and a high decay
//! shelf (`lo_mult`/`lo_freq`, `hi_mult`/`hi_freq`, on by default at ×1.2 and ×0.5) and a fixed low
//! and high cut (`lo_cut`, `hi_cut`, second order). Version 3 has none: a state keeps its sound by
//! turning them into bands. The low shelf becomes decay band 5, the high shelf band 6 (bands 1–4
//! are version 2's bells, unchanged: a bell is shape 0). A cut that did something (above 20 Hz,
//! under 20 kHz) becomes tone band 1 (low) or 2 (high) at 12 dB/oct. The parameters and every
//! stored snapshot are lifted.

use std::collections::BTreeMap;

use mite_core::{ParamDisplay, ParamScale, ParamSpec};
use mite_dsp::reverb::DecayShape;
use nih_plug::wrapper::state::{ParamValue, PluginState};

use crate::params::{band_id, tone_id, D_FREQ, D_MULT, D_ON, D_Q, D_TYPE, E_FREQ, E_ON, E_Q, E_SLOPE, E_TYPE};

pub fn v1_to_v2(_state: &mut PluginState) {}

const fn old(id: &'static str, min: f32, max: f32, default: f32) -> ParamSpec {
    ParamSpec { id, name: id, unit: "", min, max, default, scale: ParamScale::Log, smoothing_ms: 20.0, decimals: 0, wheel_step: 0.0, wheel_step_fine: 0.0, display: ParamDisplay::Number }
}

/// Version 2's parameters that version 3 no longer has (and the bells' old frequency range).
const LO_MULT: ParamSpec = old("lo_mult", 0.25, 4.0, 1.2);
const LO_FREQ: ParamSpec = old("lo_freq", 50.0, 2000.0, 250.0);
const HI_MULT: ParamSpec = old("hi_mult", 0.1, 4.0, 0.5);
const HI_FREQ: ParamSpec = old("hi_freq", 1000.0, 16_000.0, 5000.0);
const LO_CUT: ParamSpec = old("lo_cut", 10.0, 1000.0, 20.0);
const HI_CUT: ParamSpec = old("hi_cut", 1000.0, 20_000.0, 20_000.0);
const OLD_D_FREQ: ParamSpec = old("freq", 50.0, 16_000.0, 1000.0);
const GONE: [ParamSpec; 6] = [LO_MULT, LO_FREQ, HI_MULT, HI_FREQ, LO_CUT, HI_CUT];
/// The bells version 2 had (`d1…d4`).
const V2_BANDS: usize = 4;

/// A new value: a switch, a choice (index of how many) or a number.
#[derive(Debug, Clone, Copy, PartialEq)]
enum New {
    Flag(bool),
    Choice(usize, usize),
    Plain(ParamSpec, f32),
}

/// What version 3 sets, from version 2's plain values (`get`: `None` where the state has none).
fn lifted(get: impl Fn(&ParamSpec) -> Option<f32>) -> Vec<(String, New)> {
    let v = |spec: &ParamSpec| get(spec).unwrap_or(spec.default);
    let mut out = Vec::new();
    let q = std::f32::consts::FRAC_1_SQRT_2;
    for (band, shape, mult, freq) in [(4, DecayShape::LowShelf, v(&LO_MULT), v(&LO_FREQ)), (5, DecayShape::HighShelf, v(&HI_MULT), v(&HI_FREQ))] {
        if (mult - 1.0).abs() > 1e-4 {
            out.push((band_id(band, D_ON.id), New::Flag(true)));
            out.push((band_id(band, D_TYPE.id), New::Choice(shape.index(), D_TYPE.choices.len())));
            out.push((band_id(band, D_FREQ.id), New::Plain(D_FREQ, freq)));
            out.push((band_id(band, D_MULT.id), New::Plain(D_MULT, mult)));
            out.push((band_id(band, D_Q.id), New::Plain(D_Q, q.max(D_Q.min))));
        }
    }
    for (band, kind, freq, acts) in [(0, 3, v(&LO_CUT), v(&LO_CUT) > 20.5), (1, 4, v(&HI_CUT), v(&HI_CUT) < 19_900.0)] {
        if acts {
            out.push((tone_id(band, E_ON.id), New::Flag(true)));
            out.push((tone_id(band, E_TYPE.id), New::Choice(kind, E_TYPE.choices.len())));
            out.push((tone_id(band, E_FREQ.id), New::Plain(E_FREQ, freq.clamp(E_FREQ.min, E_FREQ.max))));
            out.push((tone_id(band, E_Q.id), New::Plain(E_Q, q)));
            out.push((tone_id(band, E_SLOPE.id), New::Choice(1, E_SLOPE.choices.len())));
        }
    }
    out
}

pub fn v2_to_v3(state: &mut PluginState) {
    let params = &mut state.params;
    let get = |spec: &ParamSpec| match params.get(spec.id) {
        Some(ParamValue::F32(v)) => Some(*v),
        Some(ParamValue::I32(v)) => Some(*v as f32),
        _ => None,
    };
    let writes = lifted(get);
    for (id, value) in writes {
        params.insert(
            id,
            match value {
                New::Flag(on) => ParamValue::Bool(on),
                New::Choice(i, _) => ParamValue::I32(i as i32),
                New::Plain(_, v) => ParamValue::F32(v),
            },
        );
    }
    for spec in GONE {
        params.remove(spec.id);
    }
    if let Some(json) = state.fields.get_mut(mite_plugin::state::SNAPSHOTS_KEY) {
        if let Some(lifted) = lift_snapshots(json) {
            *json = lifted;
        }
    }
}

/// The snapshots' saved form with every slot lifted (its values are normalized). A slot keeps
/// only what it stored: one that stored none of the old EQ gets no bands.
fn lift_snapshots(json: &str) -> Option<String> {
    let mut state: serde_json::Value = serde_json::from_str(json).ok()?;
    for slot in state.get_mut("slots")?.as_array_mut()?.iter_mut() {
        let Some(values) = slot.as_object_mut() else { continue };
        // The bells' frequencies: the range grew (50…16 000 Hz → 20…20 000 Hz).
        for b in 0..V2_BANDS {
            let id = band_id(b, D_FREQ.id);
            if let Some(n) = values.get(&id).and_then(serde_json::Value::as_f64) {
                values.insert(id, D_FREQ.normalize(OLD_D_FREQ.denormalize(n as f32)).into());
            }
        }
        if !GONE.iter().any(|s| values.contains_key(s.id)) {
            continue;
        }
        let snapshot: BTreeMap<String, f32> = values.iter().filter_map(|(k, v)| Some((k.clone(), v.as_f64()? as f32))).collect();
        for (id, value) in lifted(|spec| snapshot.get(spec.id).map(|&n| spec.denormalize(n))) {
            let n = match value {
                New::Flag(on) => f32::from(u8::from(on)),
                New::Choice(i, count) => i as f32 / (count - 1) as f32,
                New::Plain(spec, v) => spec.normalize(v),
            };
            values.insert(id, n.into());
        }
        for spec in GONE {
            values.remove(spec.id);
        }
    }
    serde_json::to_string(&state).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A saved value as a number (a switch: 0 or 1).
    fn val(params: &BTreeMap<String, ParamValue>, id: &str) -> Option<f32> {
        match params.get(id)? {
            ParamValue::F32(v) => Some(*v),
            ParamValue::I32(v) => Some(*v as f32),
            ParamValue::Bool(v) => Some(f32::from(u8::from(*v))),
            ParamValue::String(_) => None,
        }
    }

    fn state(params: &[(&str, ParamValue)]) -> PluginState {
        PluginState { version: String::new(), params: params.iter().map(|(k, v)| (k.to_string(), v.clone())).collect(), fields: BTreeMap::new() }
    }

    #[test]
    fn the_old_shelves_and_cuts_become_bands() {
        let mut s = state(&[("lo_mult", ParamValue::F32(1.5)), ("lo_freq", ParamValue::F32(300.0)), ("hi_mult", ParamValue::F32(0.4)), ("hi_freq", ParamValue::F32(6000.0)), ("lo_cut", ParamValue::F32(20.0)), ("hi_cut", ParamValue::F32(9000.0))]);
        v2_to_v3(&mut s);
        let p = &s.params;
        assert_eq!(val(p, "d5_on"), Some(1.0));
        assert_eq!(val(p, "d5_type"), Some(1.0));
        assert_eq!(val(p, "d5_mult"), Some(1.5));
        assert_eq!(val(p, "d6_type"), Some(2.0));
        assert_eq!(val(p, "d6_freq"), Some(6000.0));
        // The low cut at 20 Hz did nothing: no band.
        assert_eq!(val(p, "e1_on"), None);
        assert_eq!(val(p, "e2_type"), Some(4.0));
        assert_eq!(val(p, "e2_freq"), Some(9000.0));
        assert!(GONE.iter().all(|g| !p.contains_key(g.id)));
    }

    #[test]
    fn a_state_without_them_had_the_old_defaults() {
        let mut s = state(&[]);
        v2_to_v3(&mut s);
        assert_eq!(val(&s.params, "d5_mult"), Some(1.2));
        assert_eq!(val(&s.params, "d6_mult"), Some(0.5));
        assert_eq!(val(&s.params, "e1_on"), None);
    }

    #[test]
    fn snapshots_are_lifted_and_keep_only_what_they_stored() {
        let json = format!(r#"{{"active":0,"slots":[{{"lo_mult":{},"d1_freq":0.5,"size":0.3}},{{"size":0.3}},null,null]}}"#, LO_MULT.normalize(2.0));
        let lifted: serde_json::Value = serde_json::from_str(&lift_snapshots(&json).expect("lifted")).expect("JSON");
        let a = &lifted["slots"][0];
        assert!((D_MULT.denormalize(a["d5_mult"].as_f64().expect("d5") as f32) - 2.0).abs() < 1e-3);
        assert!((D_FREQ.denormalize(a["d1_freq"].as_f64().expect("d1") as f32) - OLD_D_FREQ.denormalize(0.5)).abs() < 0.1);
        assert!(a.get("lo_mult").is_none());
        // Slot 1 stored none of it.
        assert!(lifted["slots"][1].get("d5_on").is_none());
    }
}
