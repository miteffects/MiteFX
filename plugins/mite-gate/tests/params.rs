//! Parameter contract: IDs and ranges are frozen once released (BLUEPRINT §5 rules 5–6).

use mite_gate::params::{ids, manifest, sound_ids, GateParams};
use nih_plug::prelude::Params;

const GOLDEN: &str = include_str!("../../../tests/golden/mite_gate.params.txt");

#[test]
fn manifest_matches_golden() {
    assert_eq!(manifest(), GOLDEN.replace("\r\n", "\n"));
}

#[test]
fn host_ids_match_manifest() {
    let host_ids: Vec<String> = GateParams::default().param_map().into_iter().map(|(id, _, _)| id).collect();
    assert_eq!(host_ids, ids());
}

#[test]
fn monitoring_stays_out_of_presets_and_the_key_bands_are_comps() {
    let sound = sound_ids();
    for id in ["listen", "delta", "bypass"] {
        assert!(!sound.iter().any(|i| i == id), "{id}");
    }
    let comp = include_str!("../../../tests/golden/mite_comp.params.txt");
    for line in manifest().lines().filter(|l| l.starts_with('k')) {
        assert!(comp.lines().any(|c| c == line), "{line} differs from Comp's");
    }
}
