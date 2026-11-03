//! Parameter contract: IDs and ranges are frozen once released (BLUEPRINT §5 rules 5–6).

use mite_multiband::params::{ids, manifest, sound_ids, MbParams};
use nih_plug::prelude::Params;

const GOLDEN: &str = include_str!("../../../tests/golden/mite_multiband.params.txt");

#[test]
fn manifest_matches_golden() {
    assert_eq!(manifest(), GOLDEN.replace("\r\n", "\n"));
}

#[test]
fn host_ids_match_manifest() {
    let host_ids: Vec<String> = MbParams::default().param_map().into_iter().map(|(id, _, _)| id).collect();
    assert_eq!(host_ids, ids());
}

#[test]
fn monitoring_stays_out_of_presets_and_the_sidechain_eq_is_gates() {
    let sound = sound_ids();
    for id in ["delta", "bypass", "listen", "b1_solo", "b6_delta"] {
        assert!(!sound.iter().any(|i| i == id), "{id}");
    }
    assert!(sound.iter().any(|i| i == "b3_mute"));
    let gate = include_str!("../../../tests/golden/mite_gate.params.txt");
    for line in manifest().lines().filter(|l| l.starts_with('k')) {
        assert!(gate.lines().any(|g| g == line), "{line} differs from Gate's");
    }
}
