//! Prints the parameter manifest, to bless `tests/golden/mite_gate.params.txt` after a
//! deliberate, migrated parameter change:
//!
//!     cargo run -p mite_gate --example manifest > tests/golden/mite_gate.params.txt

fn main() {
    print!("{}", mite_gate::params::manifest());
}
