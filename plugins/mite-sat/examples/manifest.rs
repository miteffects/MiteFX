//! Prints the parameter manifest, to bless `tests/golden/mite_sat.params.txt` after a
//! deliberate, migrated parameter change:
//!
//!     cargo run -p mite_sat --example manifest > tests/golden/mite_sat.params.txt

fn main() {
    print!("{}", mite_sat::params::manifest());
}
