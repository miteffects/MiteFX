//! MiteFX DSP primitives.
//!
//! Rules (BLUEPRINT §5): nothing here allocates, locks or does I/O in a `process`/`next`/`reset`
//! path, and nothing here knows about plugin frameworks or UI.

pub mod gain;
pub mod meter;
pub mod smooth;

pub use gain::{db_to_gain, gain_to_db, MINUS_INFINITY_DB};
pub use meter::PeakMeter;
pub use smooth::LinearSmoother;
