//! Mite Delay (BLUEPRINT §6.6, docs/design/delay-brief.md): up to four echo taps, each synced to
//! the host's tempo or free, normal or ping-pong; Digital, Tape and BBD machines inside the loop
//! with a free loop EQ, drive, modulation and diffusion; width and ducking.

use std::sync::Arc;

use mite_core::presets::CurrentPreset;
use mite_core::snapshots::Snapshots;
use mite_core::{AtomicF32, HistoryTap, SpectrumTap};

pub mod dsp;
pub mod editor;
pub mod params;
pub mod presets;

use dsp::{record, HISTORY_RECORDS};

/// Audio → UI telemetry. Lock-free and allocation-free on the audio side.
pub struct DelayShared {
    pub history: HistoryTap<{ record::WIDTH }>,
    /// The input and the echoes alone (mono), for the loop EQ's plot.
    pub spectrum: SpectrumTap,
    pub wet: SpectrumTap,
    pub sample_rate: AtomicF32,
    /// The host's tempo (bpm; 120 until a host says), for synced taps in the editor.
    pub tempo: AtomicF32,
    pub snapshots: Arc<Snapshots>,
    /// The modulation matrix now (sources, LFO places, offsets).
    pub matrix: mite_core::modmatrix::MatrixTap,
    pub preset: Arc<CurrentPreset>,
}

impl DelayShared {
    pub fn new() -> Self {
        Self::with_stores(Arc::new(Snapshots::new()), Arc::new(CurrentPreset::default()))
    }

    pub fn with_stores(snapshots: Arc<Snapshots>, preset: Arc<CurrentPreset>) -> Self {
        Self { history: HistoryTap::new(HISTORY_RECORDS), spectrum: SpectrumTap::new(mite_dsp::analyzer::ANALYZER_SIZE), wet: SpectrumTap::new(mite_dsp::analyzer::ANALYZER_SIZE), sample_rate: AtomicF32::new(48_000.0), tempo: AtomicF32::new(DEFAULT_TEMPO), snapshots, preset, matrix: mite_core::modmatrix::MatrixTap::new() }
    }
}

impl Default for DelayShared {
    fn default() -> Self {
        Self::new()
    }
}

/// The tempo synced taps follow until a host gives one (bpm).
pub const DEFAULT_TEMPO: f32 = 120.0;
