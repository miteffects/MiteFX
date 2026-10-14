//! Parameter contract: IDs and ranges are frozen once released (BLUEPRINT §5 rules 5–6).
//!
//! If this fails because you changed a parameter on purpose, the change must come with a state
//! migration; only then update `tests/golden/mite_gain.params.txt`.

use mite_gain::params::{GainParams, SPECS};
use nih_plug::prelude::Params;

const GOLDEN: &str = include_str!("../../../tests/golden/mite_gain.params.txt");

#[test]
fn manifest_matches_golden() {
    assert_eq!(mite_core::manifest(SPECS), GOLDEN);
}

#[test]
fn host_ids_match_specs() {
    let host_ids: Vec<String> = GainParams::default().param_map().into_iter().map(|(id, _, _)| id).collect();
    let spec_ids: Vec<&str> = SPECS.iter().map(|s| s.id).collect();
    assert_eq!(host_ids, spec_ids);
}
