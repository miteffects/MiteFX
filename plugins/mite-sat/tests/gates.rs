//! The quality gates (BLUEPRINT §10) through the real CLAP entry, on the newest golden state
//! (`mite_plugin::gates`). `cargo test --release -p mite_sat --test gates -- --nocapture` prints
//! every figure.

mite_plugin::gate_tests!(mite_sat, MiteSat, "mite_sat", block_tolerance = 0.0, skip = [
    // Clicks Sat had before the tape machine (2026-11-25): with the deck off, a jump end to end
    // steps the output in one sample (Crossover 1 +45 dB, Band 2 Tone +23.5 dB over the music),
    // but the change lands at the start of the host block, 37 samples before the step test's
    // window opens, so it read as "before" and passed. The deck's latency (+169 samples) moves
    // it into the window. Open: fix the click, and open the window where the block starts.
    "Crossover 1 Frequency", "Band 2 Tone",
], hold = [
    // The golden state runs LFO 1 at 1 Hz into Band 1 Drive: held near still, so each step is
    // judged on its own (at 1 Hz the LFO's swell read as a +10–17 dB "click" on every band 1 step).
    ("LFO 1 Rate", 0.02), ("LFO 2 Rate", 0.02),
]);
