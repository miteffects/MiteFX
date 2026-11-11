//! Prints the parameter manifest, to bless `tests/golden/mite_rig.params.txt` after a
//! deliberate, migrated parameter change:
//!
//!     cargo run -p mite_rig --example manifest > tests/golden/mite_rig.params.txt

fn main() {
    print!("{}", mite_rig::params::manifest());
}
