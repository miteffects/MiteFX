//! Golden plugin states (BLUEPRINT §5), checked through the real CLAP entry point by
//! `mite_plugin::testing::check_golden_states`. When `STATE_VERSION` goes up, keep the old file,
//! add its `.expected.json`, and bless the new one with `MITEFX_BLESS=1`.

use std::path::Path;

use mite_multiband::params::STATE_VERSION;
use mite_plugin::testing::{check_golden_states, normalized_state_json, ClapInstance};
use nih_plug::prelude::ClapPlugin;
use serde_json::{json, Value};

fn instance() -> ClapInstance {
    ClapInstance::new(&mite_multiband::clap_entry, mite_multiband::MiteMultiband::CLAP_ID)
}

/// Three bands with their own dynamics, a muted one, lookahead, the sidechain EQ, snapshots and
/// a preset name.
fn configured_state() -> Vec<u8> {
    let plugin = instance();
    let mut state: Value = serde_json::from_slice(&plugin.save()).expect("JSON");
    let params = state["params"].as_object_mut().expect("params");
    for (id, v) in [
        ("x1_on", json!({"bool": true})),
        ("x1_freq", json!({"f32": 180.0})),
        ("x3_on", json!({"bool": true})),
        ("x3_freq", json!({"f32": 3200.0})),
        ("b1_thresh", json!({"f32": -26.5})),
        ("b1_ratio", json!({"f32": 3.0})),
        ("b2_quad", json!({"i32": 2})),
        ("b4_mute", json!({"bool": true})),
        ("b4_gain", json!({"f32": -2.5})),
        ("lookahead", json!({"f32": 4.0})),
        ("k2_on", json!({"bool": true})),
    ] {
        assert!(params.contains_key(id), "no parameter {id}");
        params.insert(id.into(), v);
    }
    let fields = state["fields"].as_object_mut().expect("fields");
    fields.insert("preset".into(), json!("Mix Glue"));
    fields.insert("snapshots".into(), json!(r#"{"active":0,"slots":[null,{"b1_ratio":0.4},null,null]}"#));
    let bytes = serde_json::to_vec(&state).expect("JSON");
    assert!(plugin.load(&bytes));
    plugin.save()
}

#[test]
fn every_golden_state_loads_and_saves_back() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/golden");
    check_golden_states(&mite_multiband::clap_entry, mite_multiband::MiteMultiband::CLAP_ID, &dir, "mite_multiband", STATE_VERSION, configured_state);
}

#[test]
fn a_saved_state_carries_the_current_version() {
    let state = normalized_state_json(&instance().save());
    assert_eq!(state["fields"]["mitefx_state"], json!(STATE_VERSION.to_string()));
}
