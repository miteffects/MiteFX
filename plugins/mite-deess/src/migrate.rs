//! State migrations (`mite_plugin::state::migrate`, from `Plugin::filter_state`).
//!
//! **1 → 2 (2026-11-05): the spectral mode.** REDUCE gained a fourth choice (Shelf, Band, Wide,
//! Spectral). A parameter keeps its index, but the snapshots store a choice normalised over its
//! choices (`index / (count − 1)`): version 1's Band (1 of 3: 0.5) would now read as 1.5 of 4 and
//! round to Wide. Every stored snapshot's `reduce` is re-normalised (× 2 / 3).

//!
//! **2 → 3 (2026-11-05): auto gain.** A new parameter, on by default; sessions saved before it
//! get it off, so they sound as they did.
//!
//! **3 → 4 (2026-11-05): the lookahead as a time.** `lookahead` (on/off) became `lookahead_ms`
//! (0…10 ms): on is 3 ms (what it was), off 0, in the parameters and in every stored snapshot.

use nih_plug::wrapper::state::{ParamValue, PluginState};

const REDUCE: &str = "reduce";

pub fn v1_to_v2(state: &mut PluginState) {
    if let Some(json) = state.fields.get_mut(mite_plugin::state::SNAPSHOTS_KEY) {
        if let Some(lifted) = lift_snapshots(json) {
            *json = lifted;
        }
    }
}

pub fn v2_to_v3(state: &mut PluginState) {
    state.params.entry(crate::params::AUTO_GAIN.id.to_owned()).or_insert(ParamValue::Bool(false));
}

pub fn v3_to_v4(state: &mut PluginState) {
    let spec = crate::params::LOOKAHEAD;
    if let Some(old) = state.params.remove(OLD_LOOKAHEAD) {
        let on = matches!(old, ParamValue::Bool(true));
        state.params.insert(spec.id.to_owned(), ParamValue::F32(if on { OLD_LOOKAHEAD_MS } else { 0.0 }));
    }
    if let Some(json) = state.fields.get_mut(mite_plugin::state::SNAPSHOTS_KEY) {
        if let Some(lifted) = lookahead_in_snapshots(json) {
            *json = lifted;
        }
    }
}

const OLD_LOOKAHEAD: &str = "lookahead";
const OLD_LOOKAHEAD_MS: f32 = 3.0;

fn lookahead_in_snapshots(json: &str) -> Option<String> {
    let spec = crate::params::LOOKAHEAD;
    let mut state: serde_json::Value = serde_json::from_str(json).ok()?;
    for slot in state.get_mut("slots")?.as_array_mut()?.iter_mut() {
        let Some(values) = slot.as_object_mut() else { continue };
        if let Some(n) = values.remove(OLD_LOOKAHEAD).and_then(|v| v.as_f64()) {
            let ms = if n >= 0.5 { OLD_LOOKAHEAD_MS } else { 0.0 };
            values.insert(spec.id.to_owned(), f64::from(spec.normalize(ms)).into());
        }
    }
    serde_json::to_string(&state).ok()
}

fn lift_snapshots(json: &str) -> Option<String> {
    let mut state: serde_json::Value = serde_json::from_str(json).ok()?;
    for slot in state.get_mut("slots")?.as_array_mut()?.iter_mut() {
        let Some(values) = slot.as_object_mut() else { continue };
        if let Some(n) = values.get(REDUCE).and_then(serde_json::Value::as_f64) {
            // The old index (of 3), then normalised over 4.
            let index = (n * 2.0).round();
            values.insert(REDUCE.to_owned(), (index / 3.0).into());
        }
    }
    serde_json::to_string(&state).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_snapshot_keeps_its_way_of_reducing() {
        let lifted: serde_json::Value = serde_json::from_str(&lift_snapshots(r#"{"active":0,"slots":[{"reduce":0.5,"range":0.25},{"reduce":1.0},{"range":0.1},null]}"#).expect("lifted")).expect("JSON");
        // Band (1 of 3) stays Band (1 of 4), Wide stays Wide; a slot without it is left alone.
        assert!((lifted["slots"][0]["reduce"].as_f64().expect("band") - 1.0 / 3.0).abs() < 1e-9);
        assert!((lifted["slots"][1]["reduce"].as_f64().expect("wide") - 2.0 / 3.0).abs() < 1e-9);
        assert!(lifted["slots"][2].get("reduce").is_none());
        assert_eq!(lifted["slots"][0]["range"], 0.25);
    }

    #[test]
    fn lookahead_on_becomes_three_ms_and_off_none() {
        let lifted: serde_json::Value = serde_json::from_str(&lookahead_in_snapshots(r#"{"active":0,"slots":[{"lookahead":1.0,"range":0.25},{"lookahead":0.0},{"range":0.1},null]}"#).expect("lifted")).expect("JSON");
        assert!((lifted["slots"][0]["lookahead_ms"].as_f64().expect("on") - 0.3).abs() < 1e-6);
        assert_eq!(lifted["slots"][1]["lookahead_ms"], 0.0);
        assert!(lifted["slots"][0].get("lookahead").is_none());
        assert!(lifted["slots"][2].get("lookahead_ms").is_none());
    }
}
