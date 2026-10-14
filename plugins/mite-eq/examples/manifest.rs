//! Prints the parameter manifest. Only for creating `tests/golden/mite_eq.params.txt` the first
//! time, or after a deliberate, migrated parameter change.
//!
//!     cargo run -p mite_eq --example manifest > tests/golden/mite_eq.params.txt

fn main() {
    print!("{}", mite_eq::params::manifest());
}
