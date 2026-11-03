//! Golden plugin states (BLUEPRINT §5), checked through the real CLAP entry point by
//! `mite_plugin::testing::check_golden_states`. When `STATE_VERSION` goes up, keep the old file,
//! add its `.expected.json`, and bless the new one with `MITEFX_BLESS=1`.

use std::path::Path;

use mite_limit::params::STATE_VERSION;
use mite_plugin::testing::{check_golden_states, normalized_state_json, ClapInstance};
use nih_plug::prelude::ClapPlugin;
use serde_json::{json, Value};

fn instance() -> ClapInstance {
    ClapInstance::new(&mite_limit::clap_entry, mite_limit::MiteLimit::CLAP_ID)
}

/// Pushed into a lower ceiling, Dense at 8×, half-linked release, a target, snapshots and a
/// preset name.
fn configured_state() -> Vec<u8> {
    let plugin = instance();
    let mut state: Value = serde_json::from_slice(&plugin.save()).expect("JSON");
    let params = state["params"].as_object_mut().expect("params");
    for (id, v) in [
        ("gain", json!({"f32": 9.5})),
        ("ceiling", json!({"f32": -0.3})),
        ("style", json!({"i32": 2})),
        ("lookahead", json!({"f32": 1.25})),
        ("release", json!({"f32": 80.0})),
        ("link_r", json!({"f32": 50.0})),
        ("oversampling", json!({"i32": 3})),
        ("target", json!({"i32": 6})),
    ] {
        assert!(params.contains_key(id), "no parameter {id}");
        params.insert(id.into(), v);
    }
    let fields = state["fields"].as_object_mut().expect("fields");
    fields.insert("preset".into(), json!("Loud and Dense"));
    fields.insert("snapshots".into(), json!(r#"{"active":0,"slots":[null,{"gain":0.2},null,null]}"#));
    let bytes = serde_json::to_vec(&state).expect("JSON");
    assert!(plugin.load(&bytes));
    plugin.save()
}

#[test]
fn every_golden_state_loads_and_saves_back() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/golden");
    check_golden_states(&mite_limit::clap_entry, mite_limit::MiteLimit::CLAP_ID, &dir, "mite_limit", STATE_VERSION, configured_state);
}

#[test]
fn a_saved_state_carries_the_current_version() {
    let state = normalized_state_json(&instance().save());
    assert_eq!(state["fields"]["mitefx_state"], json!(STATE_VERSION.to_string()));
}
