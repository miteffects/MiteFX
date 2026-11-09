//! Golden plugin states (BLUEPRINT §5), checked through the real CLAP entry point.

use std::path::Path;

use mite_analyzer::params::STATE_VERSION;
use mite_plugin::testing::{check_golden_states, ClapInstance};
use nih_plug::prelude::ClapPlugin;
use serde_json::{json, Value};

fn instance() -> ClapInstance {
    ClapInstance::new(&mite_analyzer::clap_entry, mite_analyzer::MiteAnalyzer::CLAP_ID)
}

/// Broadcast target, mid/side, slow, no hold, a snapshot and a preset name.
fn configured_state() -> Vec<u8> {
    let plugin = instance();
    let mut state: Value = serde_json::from_slice(&plugin.save()).expect("JSON");
    let params = state["params"].as_object_mut().expect("params");
    for (id, v) in [("target", json!({"i32": 2})), ("view", json!({"i32": 1})), ("speed", json!({"i32": 2})), ("hold", json!({"bool": false}))] {
        assert!(params.contains_key(id), "no parameter {id}");
        params.insert(id.into(), v);
    }
    let fields = state["fields"].as_object_mut().expect("fields");
    fields.insert("preset".into(), json!("Broadcast"));
    fields.insert("snapshots".into(), json!(r#"{"active":0,"slots":[null,{"tilt":0.0},null,null]}"#));
    let bytes = serde_json::to_vec(&state).expect("JSON");
    assert!(plugin.load(&bytes));
    plugin.save()
}

#[test]
fn every_golden_state_loads_and_saves_back() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/golden");
    check_golden_states(&mite_analyzer::clap_entry, mite_analyzer::MiteAnalyzer::CLAP_ID, &dir, "mite_analyzer", STATE_VERSION, configured_state);
}
