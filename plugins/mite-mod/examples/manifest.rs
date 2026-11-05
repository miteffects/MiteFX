//! Prints the parameter manifest, to bless `tests/golden/mite_mod.params.txt` after a
//! deliberate, migrated parameter change:
//!
//!     cargo run -p mite_mod --example manifest > tests/golden/mite_mod.params.txt

fn main() {
    print!("{}", mite_mod::params::manifest());
}
