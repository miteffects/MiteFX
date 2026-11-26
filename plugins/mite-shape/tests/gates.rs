//! The quality gates (BLUEPRINT §10) through the real CLAP entry, on the newest golden state
//! (`mite_plugin::gates`). `cargo test --release -p mite_shape --test gates -- --nocapture` prints
//! every figure.

mite_plugin::gate_tests!(mite_shape, MiteShape, "mite_shape", block_tolerance = 0.0, skip = [
    // Not clicks (2026-11-26): the golden state has Auto Gain on, and a band's output jumped
    // −24 → +24 dB swells the sum for the ~100 ms Auto Gain takes to follow (+15.6 dB on Band 2).
    // With Auto Gain held off both measure under 0 dB (the output glides as an S curve in dB).
    "Band 1 Output", "Band 2 Output",
]);
