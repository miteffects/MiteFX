//! Golden plugin states (BLUEPRINT §5), checked through the real CLAP entry point by
//! `mite_plugin::testing::check_golden_states`. When `STATE_VERSION` goes up, keep the old file,
//! add its `.expected.json`, and bless the new one with `MITEFX_BLESS=1`.

use std::path::Path;

use mite_plugin::testing::{check_golden_states, normalized_state_json, ClapInstance};
use mite_shape::params::STATE_VERSION;
use nih_plug::prelude::ClapPlugin;
use serde_json::{json, Value};

fn instance() -> ClapInstance {
    ClapInstance::new(&mite_shape::clap_entry, mite_shape::MiteShape::CLAP_ID)
}

/// Three bands with their own shapes, a muted one, M/S with link, lookahead, snapshots and a
/// preset name.
fn configured_state() -> Vec<u8> {
    let plugin = instance();
    let mut state: Value = serde_json::from_slice(&plugin.save()).expect("JSON");
    let params = state["params"].as_object_mut().expect("params");
    for (id, v) in [
        ("x1_on", json!({"bool": true})),
        ("x1_freq", json!({"f32": 150.0})),
        ("x3_on", json!({"bool": true})),
        ("x3_freq", json!({"f32": 3500.0})),
        ("b1_attack", json!({"f32": 5.0})),
        ("b1_sustain", json!({"f32": -3.0})),
        ("b2_time", json!({"f32": 40.0})),
        ("b4_attack", json!({"f32": -3.0})),
        ("b4_trans", json!({"f32": 120.0})),
        ("b4_mute", json!({"bool": true})),
        ("stereo", json!({"i32": 1})),
        ("link", json!({"f32": 50.0})),
        ("lookahead", json!({"f32": 2.0})),
        ("floor", json!({"f32": -50.0})),
        ("auto_gain", json!({"bool": true})),
    ] {
        assert!(params.contains_key(id), "no parameter {id}");
        params.insert(id.into(), v);
    }
    let fields = state["fields"].as_object_mut().expect("fields");
    fields.insert("preset".into(), json!("Kit Split"));
    fields.insert("snapshots".into(), json!(r#"{"active":0,"slots":[null,{"b1_attack":0.7},null,null]}"#));
    let bytes = serde_json::to_vec(&state).expect("JSON");
    assert!(plugin.load(&bytes));
    plugin.save()
}

#[test]
fn every_golden_state_loads_and_saves_back() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/golden");
    check_golden_states(&mite_shape::clap_entry, mite_shape::MiteShape::CLAP_ID, &dir, "mite_shape", STATE_VERSION, configured_state);
}

#[test]
fn a_saved_state_carries_the_current_version() {
    let state = normalized_state_json(&instance().save());
    assert_eq!(state["fields"]["mitefx_state"], json!(STATE_VERSION.to_string()));
}
