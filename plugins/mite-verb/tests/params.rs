//! Parameter contract: IDs and ranges are frozen once released (BLUEPRINT §5 rules 5–6).

use mite_verb::params::{ids, manifest, sound_ids, VerbParams};
use nih_plug::prelude::Params;

const GOLDEN: &str = include_str!("../../../tests/golden/mite_verb.params.txt");

#[test]
fn manifest_matches_golden() {
    assert_eq!(manifest(), GOLDEN.replace("\r\n", "\n"));
}

#[test]
fn host_ids_match_manifest() {
    let host_ids: Vec<String> = VerbParams::default().param_map().into_iter().map(|(id, _, _)| id).collect();
    assert_eq!(host_ids, ids());
}

#[test]
fn freeze_and_bypass_stay_out_of_presets() {
    let sound = sound_ids();
    for id in ["freeze", "bypass"] {
        assert!(!sound.iter().any(|i| i == id), "{id}");
    }
    assert_eq!(sound.len(), ids().len() - 2);
}

#[test]
fn the_plugin_settings_follow_the_parameters() {
    let p = VerbParams::default();
    let s = p.settings();
    assert_eq!(s, mite_dsp::reverb::ReverbSettings { size: 0.5, diffusion: 0.7, early: 0.5, mod_depth: 0.3, width: 1.0, ..s });
    assert_eq!(s.decay_s, 2.0);
    assert_eq!(p.output().mix, 0.3);
}
