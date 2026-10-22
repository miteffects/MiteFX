//! Golden plugin states (BLUEPRINT §5: state is versioned and migrated, with golden tests for
//! every version): `tests/golden/mite_eq.state.v<N>.json`, checked through the real CLAP entry
//! point by `mite_plugin::testing::check_golden_states`.
//!
//! When `STATE_VERSION` goes up: keep the old file, add its `.expected.json` (what it migrates
//! to), and bless a file for the new version with `MITEFX_BLESS=1 cargo test -p mite_eq --test
//! state`.

use std::path::{Path, PathBuf};

use mite_eq::params::STATE_VERSION;
use mite_plugin::testing::{check_golden_states, normalized_state_json, ClapInstance};
use nih_plug::prelude::ClapPlugin;
use serde_json::{json, Value};

fn golden_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/golden")
}

fn instance() -> ClapInstance {
    ClapInstance::new(&mite_eq::clap_entry, mite_eq::MiteEq::CLAP_ID)
}

/// A state that exercises everything a state holds: bands of several kinds (dynamic, mid/side,
/// a cut), the global settings, snapshots A and B, and a preset name.
fn configured_state() -> Vec<u8> {
    let plugin = instance();
    let mut state: Value = serde_json::from_slice(&plugin.save()).expect("JSON");
    let params = state["params"].as_object_mut().expect("params");
    for (id, v) in [
        ("b01_on", json!({"bool": true})),
        ("b01_type", json!({"i32": 3})),
        ("b01_freq", json!({"f32": 80.0})),
        ("b01_slope", json!({"i32": 4})),
        ("b02_on", json!({"bool": true})),
        ("b02_freq", json!({"f32": 2800.0})),
        ("b02_gain", json!({"f32": -3.0})),
        ("b02_q", json!({"f32": 2.6})),
        ("b02_dyn", json!({"bool": true})),
        ("b02_range", json!({"f32": -6.0})),
        ("b02_thresh", json!({"f32": -30.0})),
        ("b02_stereo", json!({"i32": 3})),
        ("mode", json!({"i32": 2})),
        ("lp_res", json!({"i32": 1})),
        ("out_gain", json!({"f32": -1.5})),
        ("mix", json!({"f32": 80.0})),
    ] {
        assert!(params.contains_key(id), "no parameter {id}");
        params.insert(id.into(), v);
    }
    let fields = state["fields"].as_object_mut().expect("fields");
    fields.insert("preset".into(), json!("Kick tight"));
    fields.insert(
        "snapshots".into(),
        json!(r#"{"active":1,"slots":[{"b01_gain":0.6},null,{"b03_on":1.0},null]}"#),
    );
    let bytes = serde_json::to_vec(&state).expect("JSON");
    assert!(plugin.load(&bytes));
    plugin.save()
}

#[test]
fn a_saved_state_carries_the_current_version() {
    let state = normalized_state_json(&instance().save());
    assert_eq!(state["fields"]["mitefx_state"], json!(STATE_VERSION.to_string()));
}

#[test]
fn every_golden_state_loads_and_saves_back() {
    check_golden_states(&mite_eq::clap_entry, mite_eq::MiteEq::CLAP_ID, &golden_dir(), "mite_eq", STATE_VERSION, configured_state);
}

#[test]
fn the_golden_state_is_not_the_default() {
    // A golden file that only held defaults would prove nothing about loading.
    let golden = std::fs::read(golden_dir().join(format!("mite_eq.state.v{STATE_VERSION}.json")));
    let Ok(golden) = golden else { return };
    let golden = normalized_state_json(&golden);
    assert_ne!(golden["params"], normalized_state_json(&instance().save())["params"]);
    assert_eq!(golden["fields"]["preset"], json!("Kick tight"));
}
