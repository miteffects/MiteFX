//! State migrations (`mite_plugin::state::migrate`, from `Plugin::filter_state`).
//!
//! **1 → 2 (2026-11-05): the rotary.** `mode` gained a sixth choice. A parameter keeps its index,
//! but the snapshots store a choice normalised over its choices (`index / (count − 1)`):
//! version 1's Vibrato (4 of 5: 1.0) would read as 5 of 6, the rotary. Every stored snapshot's
//! `mode` is re-normalised (× 4 / 5). The rotary's own parameters are new: older states get
//! their defaults, which nothing else hears.

use nih_plug::wrapper::state::PluginState;

const MODE: &str = "mode";

pub fn v1_to_v2(state: &mut PluginState) {
    if let Some(json) = state.fields.get_mut(mite_plugin::state::SNAPSHOTS_KEY) {
        if let Some(lifted) = lift_snapshots(json) {
            *json = lifted;
        }
    }
}

fn lift_snapshots(json: &str) -> Option<String> {
    let mut state: serde_json::Value = serde_json::from_str(json).ok()?;
    for slot in state.get_mut("slots")?.as_array_mut()?.iter_mut() {
        let Some(values) = slot.as_object_mut() else { continue };
        if let Some(n) = values.get(MODE).and_then(serde_json::Value::as_f64) {
            // The old index (of 5), then normalised over 6.
            let index = (n * 4.0).round();
            values.insert(MODE.to_owned(), (index / 5.0).into());
        }
    }
    serde_json::to_string(&state).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_snapshot_keeps_its_mode() {
        let lifted: serde_json::Value = serde_json::from_str(&lift_snapshots(r#"{"active":0,"slots":[{"mode":1.0,"depth":0.25},{"mode":0.75},{"depth":0.1},null]}"#).expect("lifted")).expect("JSON");
        // Vibrato (4 of 5) stays Vibrato (4 of 6), Phaser stays Phaser; a slot without it is left alone.
        assert!((lifted["slots"][0]["mode"].as_f64().expect("vibrato") - 0.8).abs() < 1e-9);
        assert!((lifted["slots"][1]["mode"].as_f64().expect("phaser") - 0.6).abs() < 1e-9);
        assert!(lifted["slots"][2].get("mode").is_none());
        assert_eq!(lifted["slots"][0]["depth"], 0.25);
    }
}
