//! Golden plugin states (BLUEPRINT §5), checked through the real CLAP entry point by
//! `mite_plugin::testing::check_golden_states`. When `STATE_VERSION` goes up, keep the old file,
//! add its `.expected.json`, and bless the new one with `MITEFX_BLESS=1`.

use std::path::Path;

use mite_gate::params::STATE_VERSION;
use mite_plugin::testing::{check_golden_states, normalized_state_json, ClapInstance};
use nih_plug::prelude::ClapPlugin;
use serde_json::{json, Value};

fn instance() -> ClapInstance {
    ClapInstance::new(&mite_gate::clap_entry, mite_gate::MiteGate::CLAP_ID)
}

/// An expander keyed from the sidechain through two EQ bands, with lookahead, snapshots and a
/// preset name.
fn configured_state() -> Vec<u8> {
    let plugin = instance();
    let mut state: Value = serde_json::from_slice(&plugin.save()).expect("JSON");
    let params = state["params"].as_object_mut().expect("params");
    for (id, v) in [
        ("mode", json!({"i32": 1})),
        ("threshold", json!({"f32": -33.5})),
        ("range", json!({"f32": -20.0})),
        ("ratio", json!({"f32": 2.5})),
        ("hysteresis", json!({"f32": 7.0})),
        ("lookahead", json!({"f32": 1.5})),
        ("sc_source", json!({"i32": 1})),
        ("k1_on", json!({"bool": true})),
        ("k2_on", json!({"bool": true})),
        ("k2_gain", json!({"f32": 6.0})),
        ("midi_on", json!({"bool": true})),
    ] {
        assert!(params.contains_key(id), "no parameter {id}");
        params.insert(id.into(), v);
    }
    let fields = state["fields"].as_object_mut().expect("fields");
    fields.insert("preset".into(), json!("Vocal Cleanup"));
    fields.insert("snapshots".into(), json!(r#"{"active":0,"slots":[null,{"range":0.4},null,null]}"#));
    let bytes = serde_json::to_vec(&state).expect("JSON");
    assert!(plugin.load(&bytes));
    plugin.save()
}

#[test]
fn every_golden_state_loads_and_saves_back() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/golden");
    check_golden_states(&mite_gate::clap_entry, mite_gate::MiteGate::CLAP_ID, &dir, "mite_gate", STATE_VERSION, configured_state);
}

#[test]
fn a_saved_state_carries_the_current_version() {
    let state = normalized_state_json(&instance().save());
    assert_eq!(state["fields"]["mitefx_state"], json!(STATE_VERSION.to_string()));
}
