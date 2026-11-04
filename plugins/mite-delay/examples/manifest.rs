//! Prints the parameter manifest, to bless `tests/golden/mite_delay.params.txt` after a
//! deliberate, migrated parameter change:
//!
//!     cargo run -p mite_delay --example manifest > tests/golden/mite_delay.params.txt

fn main() {
    print!("{}", mite_delay::params::manifest());
}
