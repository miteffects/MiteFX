//! Golden plugin states (BLUEPRINT §5), checked through the real CLAP entry point; the rig (the
//! loaded files and where they came from) travels with the state.

use std::path::Path;

use mite_amp::params::{RIG_KEY, STATE_VERSION};
use mite_plugin::testing::{check_golden_states, normalized_state_json, ClapInstance};
use nih_plug::prelude::ClapPlugin;
use serde_json::{json, Value};

fn instance() -> ClapInstance {
    ClapInstance::new(&mite_amp::clap_entry, mite_amp::MiteAmp::CLAP_ID)
}

const RIG: &str = r#"[{"path":"/rig/klon.nam"},{"path":"/rig/plexi.nam","source":{"tone_id":27156,"model_id":41,"title":"Plexi 1959","creator":"amp_guy","license":"cc-by","url":"https://www.tone3000.com/tones/plexi-27156"}},{"path":"/rig/v30.wav"}]"#;

/// A dark high-gain setting with a rig, a snapshot and a preset name.
fn configured_state() -> Vec<u8> {
    let plugin = instance();
    let mut state: Value = serde_json::from_slice(&plugin.save()).expect("JSON");
    let params = state["params"].as_object_mut().expect("params");
    for (id, v) in [("input", json!({"f32": 6.0})), ("gate_threshold", json!({"f32": -60.0})), ("size", json!({"i32": 1})), ("bass", json!({"f32": -3.0})), ("low_cut", json!({"f32": 90.0})), ("pedal", json!({"bool": false}))] {
        assert!(params.contains_key(id), "no parameter {id}");
        params.insert(id.into(), v);
    }
    let fields = state["fields"].as_object_mut().expect("fields");
    fields.insert("preset".into(), json!("Tight High Gain"));
    fields.insert("snapshots".into(), json!(r#"{"active":0,"slots":[null,{"bass":0.25},null,null]}"#));
    fields.insert(RIG_KEY.into(), json!(RIG));
    let bytes = serde_json::to_vec(&state).expect("JSON");
    assert!(plugin.load(&bytes));
    plugin.save()
}

#[test]
fn every_golden_state_loads_and_saves_back() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/golden");
    check_golden_states(&mite_amp::clap_entry, mite_amp::MiteAmp::CLAP_ID, &dir, "mite_amp", STATE_VERSION, configured_state);
}

#[test]
fn a_saved_state_carries_the_version_and_the_rig() {
    let state = normalized_state_json(&instance().save());
    assert_eq!(state["fields"]["mitefx_state"], json!(STATE_VERSION.to_string()));
    assert!(state["fields"].get(RIG_KEY).is_none(), "an empty rig saves nothing");
    let saved = normalized_state_json(&configured_state());
    let rig: Value = serde_json::from_str(saved["fields"][RIG_KEY].as_str().expect("saved")).expect("a list");
    assert_eq!(rig[1]["path"], json!("/rig/plexi.nam"));
    assert_eq!(rig[1]["source"]["creator"], json!("amp_guy"));
    assert_eq!(rig[1]["source"]["license"], json!("cc-by"));
}
