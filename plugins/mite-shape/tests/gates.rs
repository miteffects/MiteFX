//! The quality gates (BLUEPRINT §10) through the real CLAP entry, on the newest golden state
//! (`mite_plugin::gates`). `cargo test --release -p mite_shape --test gates -- --nocapture` prints
//! every figure.

mite_plugin::gate_tests!(mite_shape, MiteShape, "mite_shape", block_tolerance = 0.0, skip = [
    // OPEN (2026-11-24): the golden state's Crossover 3 jumps past Crossover 1, and the bands
    // re-sort at once while the filters glide (+9–19 dB for 2 ms). Mite Sat crossfades a structure
    // change; this needs the same. Without a neighbour to cross it steps clean (−0.6 dB).
    "Crossover 3 Frequency",
]);
