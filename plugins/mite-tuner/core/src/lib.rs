//! Mite Tuner (docs/design/tuner-brief.md): a big, precise chromatic tuner for guitar and bass.
//! The audio passes untouched (or muted); the editor reads the input from a tap and finds its
//! pitch (`mite_dsp::pitch`, the McLeod pitch method).

use std::sync::Arc;

use mite_core::presets::CurrentPreset;
use mite_core::snapshots::Snapshots;
use mite_core::{AtomicF32, HistoryTap, SpectrumTap};

pub mod dsp;
pub mod editor;
pub mod params;
pub mod presets;

use dsp::{record, HISTORY_RECORDS};

/// The input tap's length: a detection window at up to 192 kHz.
pub const TAP_LEN: usize = 16_384;

/// Audio → UI telemetry. Lock-free and allocation-free on the audio side.
pub struct TunerShared {
    pub history: HistoryTap<{ record::WIDTH }>,
    /// The input, mono, for the pitch detector.
    pub input: SpectrumTap,
    pub sample_rate: AtomicF32,
    pub snapshots: Arc<Snapshots>,
    pub preset: Arc<CurrentPreset>,
}

impl TunerShared {
    pub fn new() -> Self {
        Self::with_stores(Arc::new(Snapshots::new()), Arc::new(CurrentPreset::default()))
    }

    pub fn with_stores(snapshots: Arc<Snapshots>, preset: Arc<CurrentPreset>) -> Self {
        Self { history: HistoryTap::new(HISTORY_RECORDS), input: SpectrumTap::new(TAP_LEN), sample_rate: AtomicF32::new(48_000.0), snapshots, preset }
    }
}

impl Default for TunerShared {
    fn default() -> Self {
        Self::new()
    }
}
