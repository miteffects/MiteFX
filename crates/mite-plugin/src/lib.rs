//! nih-plug adapter.
//!
//! The rest of MiteFX is framework-free: DSP in `mite-dsp`, parameters and telemetry in
//! `mite-core`, the UI in `mite-ui`. This crate is the single place that turns those into a
//! CLAP/VST3 plugin, so swapping the framework later only touches this crate and the thin plugin
//! shells in `plugins/*`.

mod editor;
mod params;

pub use editor::create_editor;
pub use params::float_param;
