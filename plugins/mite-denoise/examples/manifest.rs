//! Prints the parameter manifest, to bless `tests/golden/mite_denoise.params.txt` after a
//! deliberate, migrated parameter change:
//!
//!     cargo run -p mite_denoise --example manifest > tests/golden/mite_denoise.params.txt

fn main() {
    print!("{}", mite_denoise::params::manifest());
}
