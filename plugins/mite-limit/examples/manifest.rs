//! Prints the parameter manifest, to bless `tests/golden/mite_limit.params.txt` after a
//! deliberate, migrated parameter change:
//!
//!     cargo run -p mite_limit --example manifest > tests/golden/mite_limit.params.txt

fn main() {
    print!("{}", mite_limit::params::manifest());
}
