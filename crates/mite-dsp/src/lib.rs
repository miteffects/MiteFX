//! MiteFX DSP primitives.
//!
//! Rules (BLUEPRINT §5): nothing here allocates, locks or does I/O in a `process`/`next`/`reset`
//! path, and nothing here knows about plugin frameworks or UI.

pub mod analyzer;
pub mod conv;
pub mod crossover;
pub mod deess;
pub mod denoise;
pub mod dither;
pub mod delay;
pub mod doubler;
pub mod dynamics;
pub mod eq;
pub mod filter;
pub mod gain;
pub mod gate;
pub mod hysteresis;
pub mod limit;
pub mod linear_phase;
pub mod loudness;
pub mod matching;
pub mod melody;
pub mod meter;
pub mod min_phase;
pub mod modmatrix;
pub mod modulation;
pub mod multiband;
pub mod natural_phase;
pub mod output;
pub mod oversample;
pub mod pitch;
pub mod resample;
pub mod reverb;
pub mod reverb_quality;
pub mod sat;
pub mod shape;
pub mod smooth;
pub mod spectral;
pub mod stft;

pub use gain::{MINUS_INFINITY_DB, db_to_gain, gain_to_db};
pub use meter::PeakMeter;
pub use smooth::LinearSmoother;
