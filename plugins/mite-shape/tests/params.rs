//! Parameter contract: IDs and ranges are frozen once released (BLUEPRINT §5 rules 5–6).
//!
//! If this fails because you changed a parameter on purpose, the change must come with a state
//! migration; only then update `tests/golden/mite_shape.params.txt`.

use mite_shape::params::{ids, manifest, sound_ids, ShapeParams};
use nih_plug::prelude::Params;

const GOLDEN: &str = include_str!("../../../tests/golden/mite_shape.params.txt");

#[test]
fn manifest_matches_golden() {
    assert_eq!(manifest(), GOLDEN.replace("\r\n", "\n"));
}

#[test]
fn host_ids_match_manifest() {
    let host_ids: Vec<String> = ShapeParams::default().param_map().into_iter().map(|(id, _, _)| id).collect();
    assert_eq!(host_ids, ids());
}

#[test]
fn ids_follow_the_scheme() {
    let ids = ids();
    for id in ["x1_on", "x4_freq", "b1_attack", "b5_sustain", "b3_time", "b2_trans", "b5_solo", "b4_mute", "stereo", "lookahead", "floor", "det_hp", "listen", "mix", "bypass"] {
        assert!(ids.iter().any(|i| i == id), "{id}");
    }
    assert!(!sound_ids().iter().any(|i| i == "listen" || i.ends_with("_solo")));
    assert!(sound_ids().iter().any(|i| i == "b2_mute"), "mute is sound: throwing a band away is part of a preset");
}

#[test]
fn the_default_state_is_one_neutral_band() {
    // Init: no crossovers and every amount at 0 dB, so the plugin passes audio unchanged.
    let s = ShapeParams::default().settings();
    assert!(s.crossovers.iter().all(Option::is_none));
    assert!(s.bands.iter().all(|b| b.attack_db == 0.0 && b.sustain_db == 0.0));
}
