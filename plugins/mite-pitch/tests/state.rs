//! Golden plugin states (BLUEPRINT §5), checked through the real CLAP entry point.

use std::path::Path;

use mite_pitch::params::STATE_VERSION;
use mite_plugin::testing::{ClapInstance, check_golden_states, normalized_state_json};
use nih_plug::prelude::ClapPlugin;
use serde_json::{Value, json};

fn instance() -> ClapInstance {
    ClapInstance::new(&mite_pitch::clap_entry, mite_pitch::MitePitch::CLAP_ID)
}

/// Bypassed, with a snapshot and a preset name.
fn configured_state() -> Vec<u8> {
    let plugin = instance();
    let mut state: Value = serde_json::from_slice(&plugin.save()).expect("JSON");
    let params = state["params"].as_object_mut().expect("params");
    assert!(params.contains_key("bypass"), "no parameter bypass");
    params.insert("bypass".into(), json!({"bool": true}));
    let fields = state["fields"].as_object_mut().expect("fields");
    fields.insert("preset".into(), json!("Init"));
    fields.insert(
        "snapshots".into(),
        json!(r#"{"active":0,"slots":[null,{"bypass":1.0},null,null]}"#),
    );
    let bytes = serde_json::to_vec(&state).expect("JSON");
    assert!(plugin.load(&bytes));
    plugin.save()
}

#[test]
fn every_golden_state_loads_and_saves_back() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/golden");
    check_golden_states(
        &mite_pitch::clap_entry,
        mite_pitch::MitePitch::CLAP_ID,
        &dir,
        "mite_pitch",
        STATE_VERSION,
        configured_state,
    );
}

#[test]
fn a_saved_state_carries_the_current_version() {
    let state = normalized_state_json(&instance().save());
    assert_eq!(
        state["fields"]["mitefx_state"],
        json!(STATE_VERSION.to_string())
    );
}
