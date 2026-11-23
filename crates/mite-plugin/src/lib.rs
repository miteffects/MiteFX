//! nih-plug adapter.
//!
//! The rest of MiteFX is framework-free: DSP in `mite-dsp`, parameters and telemetry in
//! `mite-core`, the UI in `mite-ui`. This crate is the single place that turns those into a
//! CLAP/VST3 plugin, so swapping the framework later only touches this crate and the thin plugin
//! shells in `plugins/*`.

mod editor;
pub mod gates;
pub mod matrix;
mod params;
pub mod state;
pub mod testing;

pub use editor::{create_editor, create_editor_with, SharedParamHost};
pub use params::{bool_param, choice_param, float_param, float_param_named};
