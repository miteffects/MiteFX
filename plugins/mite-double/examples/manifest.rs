//! Prints the parameter manifest, to bless `tests/golden/mite_double.params.txt` after a
//! deliberate, migrated parameter change:
//!
//!     cargo run -p mite_double --example manifest > tests/golden/mite_double.params.txt

fn main() {
    print!("{}", mite_double::params::manifest());
}
