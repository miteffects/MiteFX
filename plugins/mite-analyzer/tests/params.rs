//! Parameter contract: IDs and ranges are frozen once released (BLUEPRINT §5 rules 5–6).

use mite_analyzer::params::{ids, manifest, AnalyzerParams};
use nih_plug::prelude::Params;

const GOLDEN: &str = include_str!("../../../tests/golden/mite_analyzer.params.txt");

#[test]
fn manifest_matches_golden() {
    assert_eq!(manifest(), GOLDEN.replace("\r\n", "\n"));
}

#[test]
fn host_ids_match_manifest() {
    let host_ids: Vec<String> = AnalyzerParams::default().param_map().into_iter().map(|(id, _, _)| id).collect();
    assert_eq!(host_ids, ids());
}
