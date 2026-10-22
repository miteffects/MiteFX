//! Golden plugin states (BLUEPRINT §5), checked through the real CLAP entry point by
//! `mite_plugin::testing::check_golden_states`. When `STATE_VERSION` goes up, keep the old file,
//! add its `.expected.json`, and bless the new one with `MITEFX_BLESS=1`.

use std::path::Path;

use mite_comp::params::STATE_VERSION;
use mite_plugin::testing::{check_golden_states, normalized_state_json, ClapInstance};
use nih_plug::prelude::ClapPlugin;
use serde_json::{json, Value};

fn instance() -> ClapInstance {
    ClapInstance::new(&mite_comp::clap_entry, mite_comp::MiteComp::CLAP_ID)
}

/// Two stages, a Character, Smart, a sidechain band, M/S with link, snapshots and a preset name.
fn configured_state() -> Vec<u8> {
    let plugin = instance();
    let mut state: Value = serde_json::from_slice(&plugin.save()).expect("JSON");
    let params = state["params"].as_object_mut().expect("params");
    for (id, v) in [
        ("s1_on", json!({"bool": true})),
        ("s1_quad", json!({"i32": 2})),
        ("s1_thresh", json!({"f32": -40.0})),
        ("s1_ratio", json!({"f32": 2.0})),
        ("s2_on", json!({"bool": true})),
        ("s2_thresh", json!({"f32": -20.0})),
        ("s2_ratio", json!({"f32": 3.0})),
        ("s2_attack", json!({"f32": 8.0})),
        ("character", json!({"i32": 3})),
        ("smart", json!({"i32": 2})),
        ("stereo", json!({"i32": 1})),
        ("link", json!({"f32": 50.0})),
        ("k1_on", json!({"bool": true})),
        ("k1_freq", json!({"f32": 150.0})),
        ("lookahead", json!({"f32": 5.0})),
        ("auto_makeup", json!({"bool": true})),
    ] {
        assert!(params.contains_key(id), "no parameter {id}");
        params.insert(id.into(), v);
    }
    let fields = state["fields"].as_object_mut().expect("fields");
    fields.insert("preset".into(), json!("Vocal Leveller"));
    fields.insert("snapshots".into(), json!(r#"{"active":0,"slots":[null,{"s1_ratio":0.3},null,null]}"#));
    let bytes = serde_json::to_vec(&state).expect("JSON");
    assert!(plugin.load(&bytes));
    plugin.save()
}

#[test]
fn every_golden_state_loads_and_saves_back() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/golden");
    check_golden_states(&mite_comp::clap_entry, mite_comp::MiteComp::CLAP_ID, &dir, "mite_comp", STATE_VERSION, configured_state);
}

#[test]
fn a_saved_state_carries_the_current_version() {
    let state = normalized_state_json(&instance().save());
    assert_eq!(state["fields"]["mitefx_state"], json!(STATE_VERSION.to_string()));
}
