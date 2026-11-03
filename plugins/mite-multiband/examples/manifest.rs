//! Prints the parameter manifest, to bless `tests/golden/mite_multiband.params.txt`:
//!
//!     cargo run -p mite_multiband --example manifest > tests/golden/mite_multiband.params.txt

fn main() {
    print!("{}", mite_multiband::params::manifest());
}
