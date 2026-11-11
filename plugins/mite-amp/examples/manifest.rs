//! Prints the parameter manifest, to bless `tests/golden/mite_amp.params.txt` after a
//! deliberate, migrated parameter change:
//!
//!     cargo run -p mite_amp --example manifest > tests/golden/mite_amp.params.txt

fn main() {
    print!("{}", mite_amp::params::manifest());
}
