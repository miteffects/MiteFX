//! Parameter contract: IDs and ranges are frozen once released (BLUEPRINT §5 rules 5–6).
//!
//! If this fails because you changed a parameter on purpose, the change must come with a state
//! migration; only then update `tests/golden/mite_comp.params.txt`.

use mite_comp::params::{ids, manifest, sound_ids, CompParams};
use nih_plug::prelude::Params;

const GOLDEN: &str = include_str!("../../../tests/golden/mite_comp.params.txt");

#[test]
fn manifest_matches_golden() {
    assert_eq!(manifest(), GOLDEN.replace("\r\n", "\n"));
}

#[test]
fn host_ids_match_manifest() {
    let host_ids: Vec<String> = CompParams::default().param_map().into_iter().map(|(id, _, _)| id).collect();
    assert_eq!(host_ids, ids());
}

#[test]
fn ids_follow_the_scheme() {
    let ids = ids();
    for id in ["s1_thresh", "s4_ratio", "s4_solo", "k3_q", "character", "smart", "lookahead", "listen", "mix", "bypass"] {
        assert!(ids.iter().any(|i| i == id), "{id}");
    }
    assert!(!sound_ids().iter().any(|i| i == "listen" || i.ends_with("_solo")));
}

#[test]
fn the_default_state_has_no_stages() {
    // Init: the curve on the diagonal, so the plugin passes audio unchanged.
    let p = CompParams::default();
    assert!(p.stages.iter().all(|s| !s.on.value()));
}
