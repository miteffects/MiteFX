//! Parameter contract: IDs and ranges are frozen once released (BLUEPRINT §5 rules 5–6).

use mite_denoise::params::{ids, manifest, sound_ids, DenoiseParams};
use nih_plug::prelude::Params;

const GOLDEN: &str = include_str!("../../../tests/golden/mite_denoise.params.txt");

#[test]
fn manifest_matches_golden() {
    assert_eq!(manifest(), GOLDEN.replace("\r\n", "\n"));
}

#[test]
fn host_ids_match_manifest() {
    let host_ids: Vec<String> = DenoiseParams::default().param_map().into_iter().map(|(id, _, _)| id).collect();
    assert_eq!(host_ids, ids());
}

#[test]
fn listen_and_bypass_stay_out_of_presets() {
    let sound = sound_ids();
    assert!(!sound.iter().any(|i| i == "listen" || i == "bypass"));
    assert_eq!(sound.len(), ids().len() - 2);
}

#[test]
fn the_plugin_settings_are_the_engines_defaults() {
    assert_eq!(DenoiseParams::default().settings(), mite_dsp::denoise::DenoiseSettings::default());
}
