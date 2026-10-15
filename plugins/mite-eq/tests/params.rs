//! Parameter contract: IDs and ranges are frozen once released (BLUEPRINT §5 rules 5–6).
//!
//! If this fails because you changed a parameter on purpose, the change must come with a state
//! migration; only then update `tests/golden/mite_eq.params.txt`.

use mite_eq::params::{manifest, EqParams};
use nih_plug::prelude::Params;

const GOLDEN: &str = include_str!("../../../tests/golden/mite_eq.params.txt");

#[test]
fn manifest_matches_golden() {
    assert_eq!(manifest(), GOLDEN.replace("\r\n", "\n"));
}

#[test]
fn host_ids_match_manifest() {
    let host_ids: Vec<String> = EqParams::default().param_map().into_iter().map(|(id, _, _)| id).collect();
    let manifest_ids: Vec<String> =
        manifest().lines().map(|l| l.split(" | ").next().unwrap_or_default().to_owned()).collect();
    assert_eq!(host_ids, manifest_ids);
}

#[test]
fn ids_follow_the_blueprint_scheme() {
    let ids: Vec<String> = EqParams::default().param_map().into_iter().map(|(id, _, _)| id).collect();
    assert_eq!(ids.len(), 3 + 24 * 18);
    assert!(ids.contains(&"b01_freq".to_owned()));
    assert!(ids.contains(&"b24_slope".to_owned()));
    assert!(ids.contains(&"b24_stereo".to_owned()));
    assert!(ids.contains(&"b24_ratio".to_owned()));
    assert!(ids.contains(&"b24_dmode".to_owned()));
    assert!(ids.contains(&"b24_sustain".to_owned()));
    assert!(ids.contains(&"b24_transient".to_owned()));
    assert!(ids.contains(&"b24_ttime".to_owned()));
    assert!(ids.contains(&"b24_transition".to_owned()));
}
