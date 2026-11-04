//! Prints the parameter manifest, to bless `tests/golden/mite_tuner.params.txt` after a
//! deliberate, migrated parameter change:
//!
//!     cargo run -p mite_tuner --example manifest > tests/golden/mite_tuner.params.txt

fn main() {
    print!("{}", mite_tuner::params::manifest());
}
