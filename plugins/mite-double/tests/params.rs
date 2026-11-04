//! Parameter contract: IDs and ranges are frozen once released (BLUEPRINT §5 rules 5–6).

use mite_double::params::{ids, manifest, sound_ids, DoubleParams};
use nih_plug::prelude::Params;

const GOLDEN: &str = include_str!("../../../tests/golden/mite_double.params.txt");

#[test]
fn manifest_matches_golden() {
    assert_eq!(manifest(), GOLDEN.replace("\r\n", "\n"));
}

#[test]
fn host_ids_match_manifest() {
    let host_ids: Vec<String> = DoubleParams::default().param_map().into_iter().map(|(id, _, _)| id).collect();
    assert_eq!(host_ids, ids());
}

#[test]
fn bypass_stays_out_of_presets() {
    assert!(!sound_ids().iter().any(|i| i == "bypass"));
    assert_eq!(sound_ids().len(), ids().len() - 1);
}

#[test]
fn the_plugin_settings_follow_the_parameters() {
    let p = DoubleParams::default();
    let s = p.settings();
    // One double hard right, the source hard left: the classic double.
    assert_eq!(s.voices, 1);
    assert_eq!(s.source_pan, -1.0);
    assert_eq!(s.voice[0].pan, 1.0);
    assert!(s.follow_notes);
    assert_eq!(s.delay_ms, 16.0);
}
