//! Parameter contract: IDs and ranges are frozen once released (BLUEPRINT §5 rules 5–6).
//!
//! If this fails because you changed a parameter on purpose, the change must come with a state
//! migration; only then update `tests/golden/mite_eq.params.txt`.

use mite_eq::params::{manifest, EqParams};
use nih_plug::prelude::{Param, Params};

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
    assert_eq!(ids.len(), 8 + 24 * 23);
    assert!(ids.contains(&"b01_freq".to_owned()));
    assert!(ids.contains(&"b24_slope".to_owned()));
    assert!(ids.contains(&"b24_stereo".to_owned()));
    assert!(ids.contains(&"b24_ratio".to_owned()));
    assert!(ids.contains(&"b24_dmode".to_owned()));
    assert!(ids.contains(&"b24_sustain".to_owned()));
    assert!(ids.contains(&"b24_transient".to_owned()));
    assert!(ids.contains(&"b24_ttime".to_owned()));
    assert!(ids.contains(&"b24_transition".to_owned()));
    assert!(ids.contains(&"b24_spectral".to_owned()));
    assert!(ids.contains(&"b24_rthresh".to_owned()));
    assert!(ids.contains(&"b24_solo".to_owned()));
    for id in ["out_pan", "mix", "auto_gain", "delta", "bypass"] {
        assert!(ids.contains(&id.to_owned()), "{id}");
    }
}

#[test]
fn snapshots_are_saved_with_the_state_and_restored() {
    use mite_eq::snapshots::Values;
    let params = EqParams::default();
    let stored: Values = [("b01_gain".to_owned(), 0.8_f32)].into_iter().collect();
    params.snapshots.store(2, stored.clone());
    params.snapshots.set_active(1);
    let saved = params.serialize_fields();
    let restored = EqParams::default();
    restored.deserialize_fields(&saved);
    assert_eq!(restored.snapshots.active(), 1);
    assert_eq!(restored.snapshots.slot(2), Some(stored));
    // The active slot is saved as the parameters were (every band parameter, normalized).
    let active = restored.snapshots.slot(1).expect("the active slot is saved");
    assert_eq!(active.len(), 24 * 22);
    assert_eq!(active.get("b01_freq").copied(), Some(params.bands[0].freq.default_normalized_value()));
}
