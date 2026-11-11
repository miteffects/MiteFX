//! Golden plugin states (BLUEPRINT §5), checked through the real CLAP entry point; the board (its
//! blocks, their values, bypass, files and sources, splits with their lanes) travels with it.

use std::path::Path;

use mite_plugin::testing::{check_golden_states, normalized_state_json, ClapInstance};
use mite_rig::params::{BOARD_KEY, STATE_VERSION};
use nih_plug::prelude::ClapPlugin;
use serde_json::{json, Value};

fn instance() -> ClapInstance {
    ClapInstance::new(&mite_rig::clap_entry, mite_rig::MiteRig::CLAP_ID)
}

const BOARD: &str = r#"{"items":[{"block":{"id":1,"kind":"gate","values":{"threshold":-60.0}}},{"block":{"id":2,"kind":"capture","values":{"gain":3.0},"file":{"path":"/rig/klon.nam","source":{"tone_id":9,"model_id":4,"title":"Klon","creator":"pedal_guy","license":"cc-by","url":"https://www.tone3000.com/tones/klon-9","gear":"pedal"}}}},{"split":{"id":3,"lanes":[{"level_db":-3.0,"pan":-0.5,"blocks":[{"id":4,"kind":"capture","file":{"path":"/rig/plexi.nam"}}]},{"level_db":0.0,"pan":0.5,"blocks":[{"id":5,"kind":"capture","bypass":true,"file":{"path":"/rig/ac30.nam"}},{"id":6,"kind":"tone","values":{"treble":-4.0}}]}]}},{"block":{"id":7,"kind":"cab","file":{"path":"/rig/v30.wav"}}}]}"#;

/// A two-amp board with a pedal, a snapshot and a preset name.
fn configured_state() -> Vec<u8> {
    let plugin = instance();
    let mut state: Value = serde_json::from_slice(&plugin.save()).expect("JSON");
    let params = state["params"].as_object_mut().expect("params");
    for (id, v) in [("input", json!({"f32": 3.0})), ("size", json!({"i32": 1})), ("out_gain", json!({"f32": -2.0}))] {
        assert!(params.contains_key(id), "no parameter {id}");
        params.insert(id.into(), v);
    }
    let fields = state["fields"].as_object_mut().expect("fields");
    fields.insert("preset".into(), json!("Init"));
    fields.insert("snapshots".into(), json!(r#"{"active":0,"slots":[null,{"input":0.25},null,null]}"#));
    fields.insert(BOARD_KEY.into(), json!(BOARD));
    let bytes = serde_json::to_vec(&state).expect("JSON");
    assert!(plugin.load(&bytes));
    plugin.save()
}

#[test]
fn every_golden_state_loads_and_saves_back() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/golden");
    check_golden_states(&mite_rig::clap_entry, mite_rig::MiteRig::CLAP_ID, &dir, "mite_rig", STATE_VERSION, configured_state);
}

#[test]
fn a_saved_state_carries_the_board_as_it_was_given() {
    let state = normalized_state_json(&instance().save());
    assert_eq!(state["fields"]["mitefx_state"], json!(STATE_VERSION.to_string()));
    let default: Value = serde_json::from_str(state["fields"][BOARD_KEY].as_str().expect("a board")).unwrap();
    assert_eq!(default["items"].as_array().unwrap().len(), 3, "gate, capture, cab");
    let saved = normalized_state_json(&configured_state());
    let board: Value = serde_json::from_str(saved["fields"][BOARD_KEY].as_str().unwrap()).unwrap();
    let given: Value = serde_json::from_str(BOARD).unwrap();
    assert_eq!(board, given);
}
