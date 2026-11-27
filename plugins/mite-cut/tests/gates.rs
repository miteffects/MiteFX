//! The quality gates (BLUEPRINT §10) through the real CLAP entry, on the newest golden state
//! (`mite_plugin::gates`). `cargo test --release -p mite_cut --test gates -- --nocapture` prints
//! every figure.

mite_plugin::gate_tests!(mite_cut, MiteCut, "mite_cut", block_tolerance = 0.0, skip = []);
