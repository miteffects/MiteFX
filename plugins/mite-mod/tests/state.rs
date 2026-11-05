//! Golden plugin states (BLUEPRINT §5), checked through the real CLAP entry point.

use std::path::Path;

use mite_mod::params::STATE_VERSION;
use mite_plugin::testing::{check_golden_states, normalized_state_json, ClapInstance};
use nih_plug::prelude::ClapPlugin;
use serde_json::{json, Value};

fn instance() -> ClapInstance {
    ClapInstance::new(&mite_mod::clap_entry, mite_mod::MiteMod::CLAP_ID)
}

/// A deep phaser, through-zero on, synced, the rotary's settings moved, with a snapshot and a
/// preset name.
fn configured_state() -> Vec<u8> {
    let plugin = instance();
    let mut state: Value = serde_json::from_slice(&plugin.save()).expect("JSON");
    let params = state["params"].as_object_mut().expect("params");
    for (id, v) in [("mode", json!({"i32": 3})), ("stages", json!({"i32": 4})), ("feedback", json!({"f32": 60.0})), ("thru_zero", json!({"bool": true})), ("sync", json!({"bool": true})), ("centre", json!({"f32": 1200.0})), ("rotor", json!({"i32": 2})), ("balance", json!({"f32": -20.0})), ("inertia", json!({"f32": 70.0}))] {
        assert!(params.contains_key(id), "no parameter {id}");
        params.insert(id.into(), v);
    }
    let fields = state["fields"].as_object_mut().expect("fields");
    fields.insert("preset".into(), json!("Deep Phaser"));
    fields.insert("snapshots".into(), json!(r#"{"active":0,"slots":[null,{"depth":0.25},null,null]}"#));
    let bytes = serde_json::to_vec(&state).expect("JSON");
    assert!(plugin.load(&bytes));
    plugin.save()
}

#[test]
fn every_golden_state_loads_and_saves_back() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/golden");
    check_golden_states(&mite_mod::clap_entry, mite_mod::MiteMod::CLAP_ID, &dir, "mite_mod", STATE_VERSION, configured_state);
}

#[test]
fn a_saved_state_carries_the_current_version() {
    let state = normalized_state_json(&instance().save());
    assert_eq!(state["fields"]["mitefx_state"], json!(STATE_VERSION.to_string()));
}
