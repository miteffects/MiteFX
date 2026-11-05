//! Prints the parameter manifest, to bless `tests/golden/mite_pitch.params.txt` after a
//! deliberate, migrated parameter change:
//!
//!     cargo run -p mite_pitch --example manifest > tests/golden/mite_pitch.params.txt

fn main() {
    print!("{}", mite_pitch::params::manifest());
}
