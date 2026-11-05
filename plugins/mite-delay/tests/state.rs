//! Golden plugin states (BLUEPRINT §5), checked through the real CLAP entry point by
//! `mite_plugin::testing::check_golden_states`. When `STATE_VERSION` goes up, keep the old file,
//! add its `.expected.json`, and bless the new one with `MITEFX_BLESS=1`.

use std::path::Path;

use mite_delay::params::STATE_VERSION;
use mite_plugin::testing::{check_golden_states, normalized_state_json, ClapInstance};
use nih_plug::prelude::ClapPlugin;
use serde_json::{json, Value};

fn instance() -> ClapInstance {
    ClapInstance::new(&mite_delay::clap_entry, mite_delay::MiteDelay::CLAP_ID)
}

/// Two taps through tape with a loop EQ, snapshots and a preset name.
fn configured_state() -> Vec<u8> {
    let plugin = instance();
    let mut state: Value = serde_json::from_slice(&plugin.save()).expect("JSON");
    let params = state["params"].as_object_mut().expect("params");
    for (id, v) in [
        ("mode", json!({"i32": 1})),
        ("lfo1_shape", json!({"i32": 1})),
        ("lfo1_sync", json!({"bool": true})),
        ("m1_src", json!({"i32": 1})),
        ("m1_dst", json!({"i32": 9})),
        ("m1_amt", json!({"f32": 12.0})),
        ("drive", json!({"f32": 30.0})),
        ("t1_note", json!({"i32": 10})),
        ("t1_feedback", json!({"f32": 55.0})),
        ("t2_on", json!({"bool": true})),
        ("t2_sync", json!({"bool": false})),
        ("t2_time", json!({"f32": 140.0})),
        ("t2_pingpong", json!({"bool": true})),
        ("e1_on", json!({"bool": true})),
        ("e1_type", json!({"i32": 3})),
        ("e1_freq", json!({"f32": 250.0})),
        ("duck", json!({"f32": 9.0})),
        ("mix", json!({"f32": 40.0})),
    ] {
        assert!(params.contains_key(id), "no parameter {id}");
        params.insert(id.into(), v);
    }
    let fields = state["fields"].as_object_mut().expect("fields");
    fields.insert("preset".into(), json!("Tape Echo"));
    fields.insert("snapshots".into(), json!(r#"{"active":0,"slots":[null,{"drive":0.3},null,null]}"#));
    let bytes = serde_json::to_vec(&state).expect("JSON");
    assert!(plugin.load(&bytes));
    plugin.save()
}

#[test]
fn every_golden_state_loads_and_saves_back() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/golden");
    check_golden_states(&mite_delay::clap_entry, mite_delay::MiteDelay::CLAP_ID, &dir, "mite_delay", STATE_VERSION, configured_state);
}

#[test]
fn a_saved_state_carries_the_current_version() {
    let state = normalized_state_json(&instance().save());
    assert_eq!(state["fields"]["mitefx_state"], json!(STATE_VERSION.to_string()));
}
