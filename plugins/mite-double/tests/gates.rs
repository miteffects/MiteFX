//! The quality gates (BLUEPRINT §10) through the real CLAP entry, on the newest golden state
//! (`mite_plugin::gates`). `cargo test --release -p mite_double --test gates -- --nocapture` prints
//! every figure.

mite_plugin::gate_tests!(mite_double, MiteDouble, "mite_double", block_tolerance = 0.0, skip = [
    // The mono-bass split swept across the 220 Hz test tone (an allpass turning it), as a crossover.
    "Mono Bass Frequency",
]);
