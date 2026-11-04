//! Prints the parameter manifest, to bless `tests/golden/mite_deess.params.txt` after a
//! deliberate, migrated parameter change:
//!
//!     cargo run -p mite_deess --example manifest > tests/golden/mite_deess.params.txt

fn main() {
    print!("{}", mite_deess::params::manifest());
}
