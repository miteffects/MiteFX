//! Golden plugin states (BLUEPRINT §5), checked through the real CLAP entry point.

use std::path::Path;

use mite_deess::params::STATE_VERSION;
use mite_plugin::testing::{check_golden_states, normalized_state_json, ClapInstance};
use nih_plug::prelude::ClapPlugin;
use serde_json::{json, Value};

fn instance() -> ClapInstance {
    ClapInstance::new(&mite_deess::clap_entry, mite_deess::MiteDeEss::CLAP_ID)
}

/// A band de-esser on a bright voice, with a snapshot and a preset name.
fn configured_state() -> Vec<u8> {
    let plugin = instance();
    let mut state: Value = serde_json::from_slice(&plugin.save()).expect("JSON");
    let params = state["params"].as_object_mut().expect("params");
    for (id, v) in [("threshold", json!({"f32": -18.0})), ("reduce", json!({"i32": 1})), ("lo", json!({"f32": 6500.0})), ("hi", json!({"f32": 14000.0})), ("range", json!({"f32": 9.0})), ("lookahead", json!({"bool": false}))] {
        assert!(params.contains_key(id), "no parameter {id}");
        params.insert(id.into(), v);
    }
    let fields = state["fields"].as_object_mut().expect("fields");
    fields.insert("preset".into(), json!("Keep the Air"));
    fields.insert("snapshots".into(), json!(r#"{"active":0,"slots":[null,{"range":0.25},null,null]}"#));
    let bytes = serde_json::to_vec(&state).expect("JSON");
    assert!(plugin.load(&bytes));
    plugin.save()
}

#[test]
fn every_golden_state_loads_and_saves_back() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/golden");
    check_golden_states(&mite_deess::clap_entry, mite_deess::MiteDeEss::CLAP_ID, &dir, "mite_deess", STATE_VERSION, configured_state);
}

#[test]
fn a_saved_state_carries_the_current_version() {
    let state = normalized_state_json(&instance().save());
    assert_eq!(state["fields"]["mitefx_state"], json!(STATE_VERSION.to_string()));
}
