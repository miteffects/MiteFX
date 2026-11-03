//! MiteFX DSP primitives.
//!
//! Rules (BLUEPRINT §5): nothing here allocates, locks or does I/O in a `process`/`next`/`reset`
//! path, and nothing here knows about plugin frameworks or UI.

pub mod analyzer;
pub mod conv;
pub mod crossover;
pub mod dynamics;
pub mod eq;
pub mod filter;
pub mod hysteresis;
pub mod limit;
pub mod gain;
pub mod gate;
pub mod linear_phase;
pub mod loudness;
pub mod matching;
pub mod natural_phase;
pub mod oversample;
pub mod output;
pub mod sat;
pub mod meter;
pub mod min_phase;
pub mod multiband;
pub mod shape;
pub mod spectral;
pub mod smooth;
pub mod stft;

pub use gain::{db_to_gain, gain_to_db, MINUS_INFINITY_DB};
pub use meter::PeakMeter;
pub use smooth::LinearSmoother;
