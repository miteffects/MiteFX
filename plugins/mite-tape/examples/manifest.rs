//! Prints the parameter manifest, to bless `tests/golden/mite_tape.params.txt` after a
//! deliberate, migrated parameter change:
//!
//!     cargo run -p mite_tape --example manifest > tests/golden/mite_tape.params.txt

fn main() {
    print!("{}", mite_tape::params::manifest());
}
