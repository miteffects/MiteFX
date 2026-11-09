//! Golden plugin states (BLUEPRINT §5), checked through the real CLAP entry point; the learned
//! noise profile travels with the state.

use std::path::Path;

use mite_denoise::params::{PROFILE_KEY, STATE_VERSION};
use mite_dsp::denoise::PROFILE_POINTS;
use mite_plugin::testing::{check_golden_states, normalized_state_json, ClapInstance};
use nih_plug::prelude::ClapPlugin;
use serde_json::{json, Value};

fn instance() -> ClapInstance {
    ClapInstance::new(&mite_denoise::clap_entry, mite_denoise::MiteDenoise::CLAP_ID)
}

/// A strong setting with a learned profile (a hiss rising 3 dB an octave), a snapshot and a
/// preset name.
fn configured_state() -> Vec<u8> {
    let plugin = instance();
    let mut state: Value = serde_json::from_slice(&plugin.save()).expect("JSON");
    let params = state["params"].as_object_mut().expect("params");
    for (id, v) in [("threshold", json!({"f32": 9.0})), ("reduction", json!({"f32": 24.0})), ("adaptive", json!({"bool": false})), ("emphasis", json!({"f32": 3.0}))] {
        assert!(params.contains_key(id), "no parameter {id}");
        params.insert(id.into(), v);
    }
    let profile: Vec<f32> = (0..PROFILE_POINTS).map(|i| -110.0 + 30.0 * i as f32 / PROFILE_POINTS as f32).collect();
    let fields = state["fields"].as_object_mut().expect("fields");
    fields.insert("preset".into(), json!("Strong"));
    fields.insert("snapshots".into(), json!(r#"{"active":0,"slots":[null,{"reduction":0.25},null,null]}"#));
    fields.insert(PROFILE_KEY.into(), json!(serde_json::to_string(&profile).expect("JSON")));
    let bytes = serde_json::to_vec(&state).expect("JSON");
    assert!(plugin.load(&bytes));
    plugin.save()
}

#[test]
fn every_golden_state_loads_and_saves_back() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/golden");
    check_golden_states(&mite_denoise::clap_entry, mite_denoise::MiteDenoise::CLAP_ID, &dir, "mite_denoise", STATE_VERSION, configured_state);
}

#[test]
fn a_saved_state_carries_the_version_and_the_profile() {
    let state = normalized_state_json(&instance().save());
    assert_eq!(state["fields"]["mitefx_state"], json!(STATE_VERSION.to_string()));
    assert!(state["fields"].get(PROFILE_KEY).is_none(), "nothing learned, nothing saved");
    let saved = normalized_state_json(&configured_state());
    let profile: Vec<f32> = serde_json::from_str(saved["fields"][PROFILE_KEY].as_str().expect("saved")).expect("a list");
    assert_eq!(profile.len(), PROFILE_POINTS);
    assert!((profile[0] + 110.0).abs() < 0.01);
}
