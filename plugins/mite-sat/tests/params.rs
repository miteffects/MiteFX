//! Parameter contract: IDs and ranges are frozen once released (BLUEPRINT §5 rules 5–6).
//!
//! If this fails because you changed a parameter on purpose, the change must come with a state
//! migration; only then update `tests/golden/mite_sat.params.txt`.

use mite_sat::params::{ids, manifest, sound_ids, SatParams, MOD_TARGETS, MOD_TARGET_IDS};
use nih_plug::prelude::Params;

const GOLDEN: &str = include_str!("../../../tests/golden/mite_sat.params.txt");
const TARGETS: &str = include_str!("../../../tests/golden/mite_sat.modtargets.txt");

#[test]
fn manifest_matches_golden() {
    assert_eq!(manifest(), GOLDEN.replace("\r\n", "\n"));
}

#[test]
fn modulation_targets_match_golden() {
    // Append only: a slot stores an index into this list.
    let listed: Vec<String> = MOD_TARGET_IDS.iter().map(|s| (*s).to_owned()).collect();
    let golden: Vec<String> = TARGETS.lines().map(str::to_owned).collect();
    assert_eq!(listed, golden);
    assert_eq!(MOD_TARGETS.len(), MOD_TARGET_IDS.len());
    let ids = ids();
    for id in MOD_TARGET_IDS.iter().skip(1) {
        assert!(ids.iter().any(|i| i == id), "target {id} is not a parameter");
    }
}

#[test]
fn host_ids_match_manifest() {
    let host_ids: Vec<String> = SatParams::default().param_map().into_iter().map(|(id, _, _)| id).collect();
    assert_eq!(host_ids, ids());
}

#[test]
fn ids_follow_the_scheme() {
    let ids = ids();
    for id in ["oversampling", "x1_on", "x5_freq", "b1_model", "b6_drive", "b3_bias", "b2_tone", "b4_dyn_thr", "b5_stereo", "b6_solo", "lfo2_shape", "env_src", "m8_dst", "mix", "bypass"] {
        assert!(ids.iter().any(|i| i == id), "{id}");
    }
    assert!(!sound_ids().iter().any(|i| i == "delta" || i.ends_with("_solo")));
    assert!(sound_ids().iter().any(|i| i == "b2_mute" && sound_ids().iter().any(|j| j == "b2_bypass")));
}

#[test]
fn the_default_state_is_one_clean_band() {
    // Init: no crossovers and no drive, so the plugin passes audio unchanged (delayed).
    let p = SatParams::default();
    let s = p.settings();
    assert!(s.crossovers.iter().all(Option::is_none));
    assert!(s.bands.iter().all(|b| b.drive_db == 0.0));
    assert_eq!(p.factor(), 4, "4× by default (2× aliased audibly on Hard)");
}
