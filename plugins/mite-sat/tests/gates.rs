//! The quality gates (BLUEPRINT §10) through the real CLAP entry, on the newest golden state
//! (`mite_plugin::gates`). `cargo test --release -p mite_sat --test gates -- --nocapture` prints
//! every figure.

mite_plugin::gate_tests!(mite_sat, MiteSat, "mite_sat", block_tolerance = 0.0, skip = [
    // Not a click (2026-11-26): end to end, Band 2 (Fold, 9 dB) sweeps from everything above
    // 150 Hz to nothing, through the upper mids, where Fold is harsher than at either end. The
    // step test sets "during" against before and after only, so that passage reads +8.5 dB. With
    // drive 0 it measures −14.5 dB, and an 80 ms glide stretches it into a 70 ms plateau of the
    // same height instead of a spike.
    "Crossover 1 Frequency",
], hold = [
    // The golden state runs LFO 1 at 1 Hz into Band 1 Drive: held near still, so each step is
    // judged on its own (at 1 Hz the LFO's swell read as a +10–17 dB "click" on every band 1 step).
    ("LFO 1 Rate", 0.02), ("LFO 2 Rate", 0.02),
]);
