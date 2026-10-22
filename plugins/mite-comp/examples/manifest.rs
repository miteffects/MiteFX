//! Prints the parameter manifest, to bless `tests/golden/mite_comp.params.txt` after a deliberate,
//! migrated parameter change:
//!
//!     cargo run -p mite_comp --example manifest > tests/golden/mite_comp.params.txt

fn main() {
    print!("{}", mite_comp::params::manifest());
}
