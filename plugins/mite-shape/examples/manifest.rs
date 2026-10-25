//! Prints the parameter manifest, to bless `tests/golden/mite_shape.params.txt` after a
//! deliberate, migrated parameter change:
//!
//!     cargo run -p mite_shape --example manifest > tests/golden/mite_shape.params.txt

fn main() {
    print!("{}", mite_shape::params::manifest());
}
