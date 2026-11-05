//! Parameter contract: IDs and ranges are frozen once released (BLUEPRINT §5 rules 5–6).

use mite_dsp::modulation::{Mode, ModSettings};
use mite_mod::params::{ids, manifest, sound_ids, ModParams};
use nih_plug::prelude::Params;

const GOLDEN: &str = include_str!("../../../tests/golden/mite_mod.params.txt");

#[test]
fn manifest_matches_golden() {
    assert_eq!(manifest(), GOLDEN.replace("\r\n", "\n"));
}

#[test]
fn host_ids_match_manifest() {
    let host_ids: Vec<String> = ModParams::default().param_map().into_iter().map(|(id, _, _)| id).collect();
    assert_eq!(host_ids, ids());
}

#[test]
fn bypass_stays_out_of_presets() {
    assert!(!sound_ids().iter().any(|i| i == "bypass"));
    assert_eq!(sound_ids().len(), ids().len() - 1);
}

#[test]
fn the_plugin_settings_follow_the_parameters() {
    let s = ModParams::default().settings();
    // A two-voice chorus, the sides 90° apart.
    assert_eq!(s.mode, Mode::Chorus);
    assert_eq!(s.voices, 2);
    assert_eq!(s.stages, 4);
    assert!((s.spread - 0.5).abs() < 1e-6);
    assert_eq!(s, ModSettings { spread: 0.5, ..ModSettings::default() });
}
