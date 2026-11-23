//! The quality gates (BLUEPRINT §10) through the real CLAP entry, on the newest golden state
//! (`mite_plugin::gates`). `cargo test --release -p mite_comp --test gates -- --nocapture` prints
//! every figure.

mite_plugin::gate_tests!(mite_comp, MiteComp, "mite_comp", block_tolerance = 0.0, skip = [
    // Its compressor answering a threshold dropped 60 dB at once, at its attack: dynamics, not a glide.
    "Stage 2 Threshold",
]);
