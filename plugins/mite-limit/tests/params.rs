//! Parameter contract: IDs and ranges are frozen once released (BLUEPRINT §5 rules 5–6).
//!
//! If this fails because you changed a parameter on purpose, the change must come with a state
//! migration; only then update `tests/golden/mite_limit.params.txt`.

use mite_limit::params::{ids, manifest, sound_ids, LimitParams};
use nih_plug::prelude::Params;

const GOLDEN: &str = include_str!("../../../tests/golden/mite_limit.params.txt");

#[test]
fn manifest_matches_golden() {
    assert_eq!(manifest(), GOLDEN.replace("\r\n", "\n"));
}

#[test]
fn host_ids_match_manifest() {
    let host_ids: Vec<String> = LimitParams::default().param_map().into_iter().map(|(id, _, _)| id).collect();
    assert_eq!(host_ids, ids());
}

#[test]
fn monitoring_stays_out_of_presets() {
    let sound = sound_ids();
    for id in ["unity", "delta", "bypass", "preview", "target", "target_custom"] {
        assert!(!sound.iter().any(|i| i == id), "{id}");
    }
    for id in ["gain", "ceiling", "style", "lookahead", "true_peak", "oversampling", "clip_on", "protect", "dither"] {
        assert!(sound.iter().any(|i| i == id), "{id}");
    }
}

#[test]
fn the_default_state_limits_at_minus_one_with_true_peak_at_2x() {
    let s = LimitParams::default().settings();
    assert_eq!((s.gain_db, s.ceiling_db, s.true_peak, s.oversampling), (0.0, -1.0, true, 2));
}
