//! Parameter contract: IDs and ranges are frozen once released (BLUEPRINT §5 rules 5–6).

use mite_tape::params::{ids, manifest, sound_ids, TapeParams, TapeSettings};
use nih_plug::prelude::Params;

const GOLDEN: &str = include_str!("../../../tests/golden/mite_tape.params.txt");

#[test]
fn manifest_matches_golden() {
    assert_eq!(manifest(), GOLDEN.replace("\r\n", "\n"));
}

#[test]
fn host_ids_match_manifest() {
    let host_ids: Vec<String> = TapeParams::default().param_map().into_iter().map(|(id, _, _)| id).collect();
    assert_eq!(host_ids, ids());
}

#[test]
fn bypass_stays_out_of_presets() {
    let sound = sound_ids();
    assert!(!sound.iter().any(|i| i == "bypass"));
    assert_eq!(sound.len(), ids().len() - 1);
}

#[test]
fn the_plugin_settings_follow_the_parameters() {
    assert_eq!(TapeParams::default().settings(), TapeSettings::default());
}
