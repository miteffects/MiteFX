//! State migrations (`mite_plugin::state::migrate`, from `Plugin::filter_state`).
//!
//! **1 → 2 (2026-11-05): the rotary.** `mode` gained a sixth choice. A parameter keeps its index,
//! but the snapshots store a choice normalised over its choices (`index / (count − 1)`):
//! version 1's Vibrato (4 of 5: 1.0) would read as 5 of 6, the rotary. Every stored snapshot's
//! `mode` is re-normalised (× 4 / 5). The rotary's own parameters are new: older states get
//! their defaults, which nothing else hears.
//!
//! **2 → 3 (2026-11-05): the micro pitch shifter.** `mode` gained a seventh choice: the same
//! re-normalisation (× 5 / 6); `detune` is new.

use nih_plug::wrapper::state::PluginState;

const MODE: &str = "mode";

pub fn v1_to_v2(state: &mut PluginState) {
    lift_mode(state, 5);
}

pub fn v2_to_v3(state: &mut PluginState) {
    lift_mode(state, 6);
}

/// Every stored snapshot's `mode`, normalised over `count` choices, normalised over one more.
fn lift_mode(state: &mut PluginState, count: usize) {
    if let Some(json) = state.fields.get_mut(mite_plugin::state::SNAPSHOTS_KEY) {
        if let Some(lifted) = lift_snapshots(json, count) {
            *json = lifted;
        }
    }
}

fn lift_snapshots(json: &str, count: usize) -> Option<String> {
    let mut state: serde_json::Value = serde_json::from_str(json).ok()?;
    for slot in state.get_mut("slots")?.as_array_mut()?.iter_mut() {
        let Some(values) = slot.as_object_mut() else { continue };
        if let Some(n) = values.get(MODE).and_then(serde_json::Value::as_f64) {
            // The old index (of `count`), then normalised over one more.
            let index = (n * (count - 1) as f64).round();
            values.insert(MODE.to_owned(), (index / count as f64).into());
        }
    }
    serde_json::to_string(&state).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_snapshot_keeps_its_mode() {
        let lifted: serde_json::Value = serde_json::from_str(&lift_snapshots(r#"{"active":0,"slots":[{"mode":1.0,"depth":0.25},{"mode":0.75},{"depth":0.1},null]}"#, 5).expect("lifted")).expect("JSON");
        // Vibrato (4 of 5) stays Vibrato (4 of 6), Phaser stays Phaser; a slot without it is left alone.
        assert!((lifted["slots"][0]["mode"].as_f64().expect("vibrato") - 0.8).abs() < 1e-9);
        assert!((lifted["slots"][1]["mode"].as_f64().expect("phaser") - 0.6).abs() < 1e-9);
        assert!(lifted["slots"][2].get("mode").is_none());
        assert_eq!(lifted["slots"][0]["depth"], 0.25);
        // 2 → 3: Rotary (5 of 6) stays Rotary (5 of 7).
        let lifted: serde_json::Value = serde_json::from_str(&lift_snapshots(r#"{"active":0,"slots":[{"mode":1.0}]}"#, 6).expect("lifted")).expect("JSON");
        assert!((lifted["slots"][0]["mode"].as_f64().expect("rotary") - 5.0 / 6.0).abs() < 1e-9);
    }
}
