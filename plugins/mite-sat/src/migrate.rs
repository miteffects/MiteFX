//! State migrations (`mite_plugin::state::migrate`, from `Plugin::filter_state`).
//!
//! **1 → 2 (2026-11-10): Left and Right join each band's stereo choices** (user: "i need l/r or
//! m/s control in saturation"). The list grew from 4 (Stereo, Mid, Side, M/S) to 6, appended, so
//! the saved plain index means the same; but a snapshot stores values normalized over the list
//! (i/3 then, i/5 now), so every `b{N}_stereo` in every slot is rescaled.

use mite_plugin::state::SNAPSHOTS_KEY;
use nih_plug::wrapper::state::{ParamValue, PluginState};

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

/// **4 → 5 (2026-11-24): the drive law.** Drive 0 dB used to put a −12 dBFS signal right at the
/// curves' knee, and the first 6 dB faded the saturation in as a mix: no gentle range (measured
/// against Saturn 2, docs/compare/saturn-2.md). The curves now take 24 dB less at the same
/// drive and the range runs to 60 dB, so a setting keeps its push at `d + 24` from 6 dB up; the
/// old fade-in, 0…6, spreads over 0…30 (0 stays 0, the band untouched). A modulation of drive
/// moves a share of the knob's travel, so its amount scales by the old range over the new.
pub fn v4_to_v5(state: &mut PluginState) {
    let drive_targets = drive_target_indices();
    for (id, v) in state.params.iter_mut() {
        if is_drive(id) {
            if let ParamValue::F32(d) = v {
                *d = lift_drive(*d);
            }
        }
    }
    let ids: Vec<String> = state.params.keys().cloned().collect();
    for id in ids.iter().filter(|id| id.starts_with('m') && id.ends_with("_dst")) {
        let target = match state.params.get(id) {
            Some(ParamValue::I32(i)) => *i as usize,
            Some(ParamValue::F32(f)) => f.round() as usize,
            _ => continue,
        };
        if drive_targets.contains(&target) {
            let amt = id.replace("_dst", "_amt");
            if let Some(ParamValue::F32(a)) = state.params.get_mut(&amt) {
                *a *= OLD_DRIVE_MAX / NEW_DRIVE_MAX;
            }
        }
    }
    if let Some(json) = state.fields.get_mut(SNAPSHOTS_KEY) {
        if let Some(lifted) = lift_snapshots(json, &drive_targets) {
            *json = lifted;
        }
    }
}

/// **5 → 6 (2026-11-24): feedback.** Each band's `fb` and `fb_freq` are appended (a state 5
/// gets 0 %, as it had), and so are their twelve modulation targets. A snapshot stores a slot's
/// target normalized over the list (`i/43` then, `i/55` now), so every `m{N}_dst` in every slot
/// is rescaled; the plain index in the parameters means the same.
pub fn v5_to_v6(state: &mut PluginState) {
    if let Some(json) = state.fields.get_mut(SNAPSHOTS_KEY) {
        if let Some(lifted) = rescale_targets(json, 43.0, 55.0) {
            *json = lifted;
        }
    }
}

fn rescale_targets(json: &str, old_last: f64, new_last: f64) -> Option<String> {
    let mut v: serde_json::Value = serde_json::from_str(json).ok()?;
    for slot in v.get_mut("slots")?.as_array_mut()?.iter_mut() {
        let Some(values) = slot.as_object_mut() else { continue };
        for (id, x) in values.iter_mut() {
            if id.starts_with('m') && id.ends_with("_dst") {
                if let Some(n) = x.as_f64() {
                    *x = serde_json::json!(((n * old_last).round() / new_last) as f32);
                }
            }
        }
    }
    serde_json::to_string(&v).ok()
}

/// **6 → 7 (2026-11-24): 16× oversampling appended.** The saved plain index means the same; a
/// snapshot stores it normalized over the list (`i/3` then, `i/4` now), so it is rescaled.
pub fn v6_to_v7(state: &mut PluginState) {
    if let Some(json) = state.fields.get_mut(SNAPSHOTS_KEY) {
        if let Some(lifted) = rescale_choice(json, "oversampling", 3.0, 4.0) {
            *json = lifted;
        }
    }
}

fn rescale_choice(json: &str, id: &str, old_last: f64, new_last: f64) -> Option<String> {
    let mut v: serde_json::Value = serde_json::from_str(json).ok()?;
    for slot in v.get_mut("slots")?.as_array_mut()?.iter_mut() {
        if let Some(x) = slot.as_object_mut().and_then(|o| o.get_mut(id)) {
            if let Some(n) = x.as_f64() {
                *x = serde_json::json!(((n * old_last).round() / new_last) as f32);
            }
        }
    }
    serde_json::to_string(&v).ok()
}

/// **7 → 8 (2026-11-24): Harmonic (drawn harmonics) appended to the models, each band's recipe,
/// Punch and Body appended to the parameters.** A snapshot stores a band's model normalized over
/// the list (`i/7` then, `i/8` now), so every `b{N}_model` is rescaled; the new parameters take
/// their defaults (Punch and Body 0: the band as it was).
pub fn v7_to_v8(state: &mut PluginState) {
    if let Some(json) = state.fields.get_mut(SNAPSHOTS_KEY) {
        for b in 1..=6 {
            if let Some(lifted) = rescale_choice(json, &format!("b{b}_model"), 7.0, 8.0) {
                *json = lifted;
            }
        }
    }
}

const OLD_DRIVE_MAX: f32 = 36.0;
const NEW_DRIVE_MAX: f32 = 60.0;
/// The old fade-in's top, where the old and new laws join (old 6 dB = new 30 dB).
const OLD_ENGAGE: f32 = 6.0;
const OFFSET: f32 = 24.0;

fn is_drive(id: &str) -> bool {
    id.starts_with('b') && id.ends_with("_drive")
}

/// A drive (dB) saved under the old law, under the new one.
pub fn lift_drive(d: f32) -> f32 {
    let d = d.clamp(0.0, OLD_DRIVE_MAX);
    if d < OLD_ENGAGE { d * (OLD_ENGAGE + OFFSET) / OLD_ENGAGE } else { d + OFFSET }.min(NEW_DRIVE_MAX)
}

/// The modulation targets that are a band's drive (indices into the `dst` list).
fn drive_target_indices() -> Vec<usize> {
    crate::params::TARGETS.ids.iter().enumerate().filter(|(_, id)| is_drive(id)).map(|(i, _)| i).collect()
}

fn lift_snapshots(json: &str, drive_targets: &[usize]) -> Option<String> {
    let mut v: serde_json::Value = serde_json::from_str(json).ok()?;
    let last = (crate::params::TARGETS.names.len() - 1) as f64;
    for slot in v.get_mut("slots")?.as_array_mut()?.iter_mut() {
        let Some(values) = slot.as_object_mut() else { continue };
        let modulated: Vec<String> = values
            .iter()
            .filter(|(id, x)| id.starts_with('m') && id.ends_with("_dst") && x.as_f64().is_some_and(|n| drive_targets.contains(&((n * last).round() as usize))))
            .map(|(id, _)| id.replace("_dst", "_amt"))
            .collect();
        for (id, x) in values.iter_mut() {
            let Some(n) = x.as_f64() else { continue };
            if is_drive(id) {
                *x = serde_json::json!(lift_drive(n as f32 * OLD_DRIVE_MAX) / NEW_DRIVE_MAX);
            } else if modulated.contains(id) {
                // −100…100 % over 0…1: scaled about its centre.
                *x = serde_json::json!(0.5 + (n as f32 - 0.5) * OLD_DRIVE_MAX / NEW_DRIVE_MAX);
            }
        }
    }
    serde_json::to_string(&v).ok()
}

/// A preset saved before state 5 (its values are plain), under the drive law of state 5.
pub fn lift_preset(values: &mut mite_core::presets::PresetValues, from: u32) {
    if from >= 5 {
        return;
    }
    let drive_targets = drive_target_indices();
    let modulated: Vec<String> = values
        .iter()
        .filter(|(id, v)| id.starts_with('m') && id.ends_with("_dst") && drive_targets.contains(&(v.round() as usize)))
        .map(|(id, _)| id.replace("_dst", "_amt"))
        .collect();
    for (id, v) in values.iter_mut() {
        if is_drive(id) {
            *v = lift_drive(*v);
        } else if modulated.contains(id) {
            *v *= OLD_DRIVE_MAX / NEW_DRIVE_MAX;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_snapshots_oversampling_keeps_its_factor() {
        // 4× (2 of 3) in a version 6 snapshot is 2 of 4.
        let old = r#"{"active":0,"slots":[{"oversampling":0.6666667,"b1_drive":0.5},null,null,null]}"#;
        let new: serde_json::Value = serde_json::from_str(&rescale_choice(old, "oversampling", 3.0, 4.0).expect("lifted")).expect("JSON");
        assert!((new["slots"][0]["oversampling"].as_f64().unwrap() - 0.5).abs() < 1e-6);
        assert_eq!(new["slots"][0]["b1_drive"].as_f64(), Some(0.5));
    }

    #[test]
    fn a_snapshots_modulation_targets_keep_their_meaning() {
        // Band 1 Drive (1 of 43) and Output (43 of 43) in a version 5 snapshot.
        let old = r#"{"active":0,"slots":[{"m1_dst":0.023255814,"m2_dst":1.0,"b1_drive":0.5},null,null,null]}"#;
        let new: serde_json::Value = serde_json::from_str(&rescale_targets(old, 43.0, 55.0).expect("lifted")).expect("JSON");
        let slot = &new["slots"][0];
        let last = (crate::params::TARGETS.names.len() - 1) as f64;
        assert_eq!(crate::params::TARGETS.names[(slot["m1_dst"].as_f64().unwrap() * last).round() as usize], "Band 1 Drive");
        assert_eq!(crate::params::TARGETS.names[(slot["m2_dst"].as_f64().unwrap() * last).round() as usize], "Output");
        assert_eq!(slot["b1_drive"].as_f64(), Some(0.5), "the rest untouched");
    }

    #[test]
    fn the_drive_law_keeps_off_off_and_a_push_its_push() {
        assert_eq!(lift_drive(0.0), 0.0);
        assert_eq!(lift_drive(3.0), 15.0);
        assert_eq!(lift_drive(6.0), 30.0);
        assert_eq!(lift_drive(14.0), 38.0);
        assert_eq!(lift_drive(36.0), 60.0);
        let old = r#"{"active":0,"slots":[{"b1_drive":0.5,"b2_drive":0.0,"m1_dst":0.023255814,"m1_amt":1.0,"m2_dst":0.04651163,"m2_amt":1.0},null,null,null]}"#;
        let new: serde_json::Value = serde_json::from_str(&lift_snapshots(old, &drive_target_indices()).expect("lifted")).expect("JSON");
        let slot = &new["slots"][0];
        assert!((slot["b1_drive"].as_f64().unwrap() - 42.0 / 60.0).abs() < 1e-6, "18 dB → 42 dB");
        assert_eq!(slot["b2_drive"].as_f64(), Some(0.0));
        assert!((slot["m1_amt"].as_f64().unwrap() - 0.8).abs() < 1e-6, "+100 % on Band 1 Drive → +60 %");
        assert_eq!(slot["m2_amt"].as_f64(), Some(1.0), "Band 1 Bias untouched");
    }

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
