//! Prints the parameter manifest, to bless `tests/golden/mite_verb.params.txt` after a
//! deliberate, migrated parameter change:
//!
//!     cargo run -p mite_verb --example manifest > tests/golden/mite_verb.params.txt

fn main() {
    print!("{}", mite_verb::params::manifest());
}
