//! Parameter contract: IDs and ranges are frozen once released (BLUEPRINT §5 rules 5–6).

use mite_deess::params::{ids, manifest, sound_ids, DeEssParams};
use nih_plug::prelude::Params;

const GOLDEN: &str = include_str!("../../../tests/golden/mite_deess.params.txt");

#[test]
fn manifest_matches_golden() {
    assert_eq!(manifest(), GOLDEN.replace("\r\n", "\n"));
}

#[test]
fn host_ids_match_manifest() {
    let host_ids: Vec<String> = DeEssParams::default().param_map().into_iter().map(|(id, _, _)| id).collect();
    assert_eq!(host_ids, ids());
}

#[test]
fn listen_and_bypass_stay_out_of_presets() {
    let sound = sound_ids();
    assert!(!sound.iter().any(|i| i == "listen" || i == "bypass"));
    assert_eq!(sound.len(), ids().len() - 2);
}

#[test]
fn the_plugin_settings_follow_the_parameters() {
    let s = DeEssParams::default().settings();
    // Auto gain is on in the plugin (the engine leaves it off unless asked).
    assert_eq!(s, mite_dsp::deess::DeEssSettings { auto_gain: true, ..mite_dsp::deess::DeEssSettings::default() });
}
