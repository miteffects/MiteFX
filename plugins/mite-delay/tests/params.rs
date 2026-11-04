//! Parameter contract: IDs and ranges are frozen once released (BLUEPRINT §5 rules 5–6).

use mite_delay::params::{ids, manifest, sound_ids, DelayParams};
use nih_plug::prelude::Params;

const GOLDEN: &str = include_str!("../../../tests/golden/mite_delay.params.txt");

#[test]
fn manifest_matches_golden() {
    assert_eq!(manifest(), GOLDEN.replace("\r\n", "\n"));
}

#[test]
fn host_ids_match_manifest() {
    let host_ids: Vec<String> = DelayParams::default().param_map().into_iter().map(|(id, _, _)| id).collect();
    assert_eq!(host_ids, ids());
}

#[test]
fn bypass_stays_out_of_presets() {
    let sound = sound_ids();
    assert!(!sound.iter().any(|i| i == "bypass"));
    assert_eq!(sound.len(), ids().len() - 1);
}

#[test]
fn the_plugin_settings_follow_the_parameters_and_the_tempo() {
    let p = DelayParams::default();
    let s = p.settings(120.0);
    // One tap by default, a quarter note; nothing else on.
    assert!(s.taps[0].on && s.taps[1..].iter().all(|t| !t.on));
    assert_eq!(s.taps[0].time_ms, 500.0);
    assert_eq!(p.settings(90.0).taps[0].time_ms, 60_000.0 / 90.0);
    assert!(s.eq.iter().all(|b| !b.enabled));
    assert_eq!(s.mode, mite_dsp::delay::Mode::Digital);
    assert_eq!(p.output().mix, 0.3);
}
