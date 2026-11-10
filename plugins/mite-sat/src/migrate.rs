//! State migrations (`mite_plugin::state::migrate`, from `Plugin::filter_state`).
//!
//! **1 → 2 (2026-11-10): Left and Right join each band's stereo choices** (user: "i need l/r or
//! m/s control in saturation"). The list grew from 4 (Stereo, Mid, Side, M/S) to 6, appended, so
//! the saved plain index means the same; but a snapshot stores values normalized over the list
//! (i/3 then, i/5 now), so every `b{N}_stereo` in every slot is rescaled.

use mite_plugin::state::SNAPSHOTS_KEY;
use nih_plug::wrapper::state::PluginState;

/// The stereo list's last index, before and after.
const OLD_LAST: f64 = 3.0;
const NEW_LAST: f64 = 5.0;

/// **2 → 3 (2026-11-10):** each band's L/R and M/S balance appended, centred (as it was).
pub fn v2_to_v3(_state: &mut PluginState) {}

pub fn v1_to_v2(state: &mut PluginState) {
    if let Some(json) = state.fields.get_mut(SNAPSHOTS_KEY) {
        if let Some(lifted) = rescale_stereo(json) {
            *json = lifted;
        }
    }
}

fn rescale_stereo(json: &str) -> Option<String> {
    let mut v: serde_json::Value = serde_json::from_str(json).ok()?;
    for slot in v.get_mut("slots")?.as_array_mut()?.iter_mut() {
        let Some(values) = slot.as_object_mut() else { continue };
        for (id, x) in values.iter_mut() {
            if id.starts_with('b') && id.ends_with("_stereo") {
                if let Some(n) = x.as_f64() {
                    *x = serde_json::json!(((n * OLD_LAST).round() / NEW_LAST) as f32);
                }
            }
        }
    }
    serde_json::to_string(&v).ok()
}

/// **3 → 4 (2026-11-10):** `phase` appended; a version 3 state gets Minimum Phase, what it had.
pub fn v3_to_v4(_state: &mut PluginState) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_snapshots_stereo_modes_keep_their_meaning() {
        // Mid (1 of 3) and M/S (3 of 3) in a version 1 snapshot.
        let old = r#"{"active":0,"slots":[{"b1_stereo":0.33333334,"b2_stereo":1.0,"b1_drive":0.5},null,null,null]}"#;
        let new: serde_json::Value = serde_json::from_str(&rescale_stereo(old).expect("lifted")).expect("JSON");
        let slot = &new["slots"][0];
        assert!((slot["b1_stereo"].as_f64().unwrap() - 0.2).abs() < 1e-6, "Mid is 1 of 5");
        assert!((slot["b2_stereo"].as_f64().unwrap() - 0.6).abs() < 1e-6, "M/S is 3 of 5");
        assert_eq!(slot["b1_drive"].as_f64(), Some(0.5), "the rest untouched");
    }
}
