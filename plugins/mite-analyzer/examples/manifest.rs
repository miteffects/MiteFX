//! Prints the parameter manifest, to bless `tests/golden/mite_analyzer.params.txt` after a
//! deliberate, migrated parameter change:
//!
//!     cargo run -p mite_analyzer --example manifest > tests/golden/mite_analyzer.params.txt

fn main() {
    print!("{}", mite_analyzer::params::manifest());
}
