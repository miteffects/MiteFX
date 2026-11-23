//! The quality gates (BLUEPRINT §10) through the real CLAP entry (`mite_plugin::gates`), on the
//! golden state: bands of several kinds on, one dynamic, so the parameters have work to do.
//!
//!     cargo test --release -p mite_eq --test gates -- --nocapture   (prints every figure)

use std::path::Path;

use mite_eq::params::STATE_VERSION;
use mite_plugin::gates::Gates;
use nih_plug::prelude::ClapPlugin;

fn gates() -> Gates {
    let golden = Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("../../tests/golden/mite_eq.state.v{STATE_VERSION}.json"));
    Gates::new(&mite_eq::clap_entry, mite_eq::MiteEq::CLAP_ID).with_state(std::fs::read(golden).expect("golden state"))
}

#[test]
fn the_output_is_the_same_at_every_block_size() {
    for (blocks, diff) in gates().block_sizes(1.0) {
        println!("blocks {blocks}: max difference {diff:e}");
        assert!(diff < 1e-6, "blocks {blocks}: {diff}");
    }
}

#[test]
fn every_sample_rate_from_44_1_to_384_khz_works() {
    for (rate, r) in gates().sample_rates() {
        let (peak, latency) = r.unwrap_or_else(|e| panic!("{rate} Hz: {e}"));
        println!("{rate} Hz: peak {peak:.3}, latency {latency}");
        assert!(peak > 0.05 && peak < 4.0, "{rate} Hz: peak {peak}");
        assert!(f64::from(latency) < 0.5 * rate, "{rate} Hz: latency {latency}");
    }
}

#[test]
fn nan_and_inf_recover_through_reset() {
    let (finite, peak) = gates().nan_recovery();
    assert!(finite, "non-finite output after reset");
    assert!(peak > 0.05, "silent after reset: {peak}");
}

#[test]
fn no_continuous_parameter_clicks_when_it_jumps_end_to_end() {
    let g = gates();
    let mut worst = Vec::new();
    for p in g.continuous(&[]) {
        let s = g.step(&p);
        worst.push(s);
    }
    worst.sort_by(|a, b| b.worst_db().total_cmp(&a.worst_db()));
    for s in worst.iter().take(15) {
        println!("{:<28} up {:+6.1} dB  down {:+6.1} dB", s.param, s.up_db, s.down_db);
    }
    let clicks: Vec<_> = worst.iter().filter(|s| s.worst_db() > 6.0).map(|s| format!("{} ({:+.1} dB)", s.param, s.worst_db())).collect();
    assert!(clicks.is_empty(), "{} of {} click: {}", clicks.len(), worst.len(), clicks.join(", "));
}
