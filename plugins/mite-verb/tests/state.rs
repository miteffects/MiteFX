//! Golden plugin states (BLUEPRINT §5), checked through the real CLAP entry point by
//! `mite_plugin::testing::check_golden_states`. When `STATE_VERSION` goes up, keep the old file,
//! add its `.expected.json`, and bless the new one with `MITEFX_BLESS=1`.

use std::path::Path;

use mite_plugin::testing::{check_golden_states, normalized_state_json, ClapInstance};
use mite_verb::params::STATE_VERSION;
use nih_plug::prelude::ClapPlugin;
use serde_json::{json, Value};

fn instance() -> ClapInstance {
    ClapInstance::new(&mite_verb::clap_entry, mite_verb::MiteVerb::CLAP_ID)
}

/// A large, dark, ducked hall with snapshots and a preset name.
fn configured_state() -> Vec<u8> {
    let plugin = instance();
    let mut state: Value = serde_json::from_slice(&plugin.save()).expect("JSON");
    let params = state["params"].as_object_mut().expect("params");
    for (id, v) in [
        ("predelay", json!({"f32": 35.0})),
        ("size", json!({"f32": 80.0})),
        ("decay", json!({"f32": 3.5})),
        ("lo_mult", json!({"f32": 1.5})),
        ("hi_mult", json!({"f32": 0.4})),
        ("hi_cut", json!({"f32": 9000.0})),
        ("duck", json!({"f32": 9.0})),
        ("mix", json!({"f32": 40.0})),
    ] {
        assert!(params.contains_key(id), "no parameter {id}");
        params.insert(id.into(), v);
    }
    let fields = state["fields"].as_object_mut().expect("fields");
    fields.insert("preset".into(), json!("Large Hall"));
    fields.insert("snapshots".into(), json!(r#"{"active":0,"slots":[null,{"size":0.3},null,null]}"#));
    let bytes = serde_json::to_vec(&state).expect("JSON");
    assert!(plugin.load(&bytes));
    plugin.save()
}

#[test]
fn every_golden_state_loads_and_saves_back() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/golden");
    check_golden_states(&mite_verb::clap_entry, mite_verb::MiteVerb::CLAP_ID, &dir, "mite_verb", STATE_VERSION, configured_state);
}

#[test]
fn a_saved_state_carries_the_current_version() {
    let state = normalized_state_json(&instance().save());
    assert_eq!(state["fields"]["mitefx_state"], json!(STATE_VERSION.to_string()));
}
