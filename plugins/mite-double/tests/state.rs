//! Golden plugin states (BLUEPRINT §5), checked through the real CLAP entry point.

use std::path::Path;

use mite_double::params::STATE_VERSION;
use mite_plugin::testing::{check_golden_states, normalized_state_json, ClapInstance};
use nih_plug::prelude::ClapPlugin;
use serde_json::{json, Value};

fn instance() -> ClapInstance {
    ClapInstance::new(&mite_double::clap_entry, mite_double::MiteDouble::CLAP_ID)
}

/// Three loose doubles, a darker tone, with a snapshot and a preset name.
fn configured_state() -> Vec<u8> {
    let plugin = instance();
    let mut state: Value = serde_json::from_slice(&plugin.save()).expect("JSON");
    let params = state["params"].as_object_mut().expect("params");
    for (id, v) in [("voices", json!({"i32": 2})), ("timing", json!({"f32": 25.0})), ("pitch", json!({"f32": 11.0})), ("tone", json!({"f32": -2.0})), ("v2_pan", json!({"f32": -80.0})), ("follow", json!({"bool": false}))] {
        assert!(params.contains_key(id), "no parameter {id}");
        params.insert(id.into(), v);
    }
    let fields = state["fields"].as_object_mut().expect("fields");
    fields.insert("preset".into(), json!("Quad Tracked"));
    fields.insert("snapshots".into(), json!(r#"{"active":0,"slots":[null,{"timing":0.5},null,null]}"#));
    let bytes = serde_json::to_vec(&state).expect("JSON");
    assert!(plugin.load(&bytes));
    plugin.save()
}

#[test]
fn every_golden_state_loads_and_saves_back() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/golden");
    check_golden_states(&mite_double::clap_entry, mite_double::MiteDouble::CLAP_ID, &dir, "mite_double", STATE_VERSION, configured_state);
}

#[test]
fn a_saved_state_carries_the_current_version() {
    let state = normalized_state_json(&instance().save());
    assert_eq!(state["fields"]["mitefx_state"], json!(STATE_VERSION.to_string()));
}
