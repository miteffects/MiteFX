//! Mite Verb: an algorithmic reverb (BLUEPRINT §6.5, docs/design/verb-brief.md) with a free decay
//! EQ (decay time per frequency), a free tone EQ, freeze and ducking.

use std::sync::Arc;

use mite_core::presets::CurrentPreset;
use mite_core::snapshots::Snapshots;
use mite_core::{AtomicF32, HistoryTap, SpectrumTap};

pub mod dsp;
pub mod editor;
pub mod migrate;
pub mod params;
pub mod presets;

use dsp::{record, HISTORY_RECORDS};

/// Audio → UI telemetry. Lock-free and allocation-free on the audio side.
pub struct VerbShared {
    pub history: HistoryTap<{ record::WIDTH }>,
    /// The input and the reverb alone (mono), for the decay plot.
    pub spectrum: SpectrumTap,
    pub wet: SpectrumTap,
    pub sample_rate: AtomicF32,
    pub snapshots: Arc<Snapshots>,
    pub preset: Arc<CurrentPreset>,
}

impl VerbShared {
    pub fn new() -> Self {
        Self::with_stores(Arc::new(Snapshots::new()), Arc::new(CurrentPreset::default()))
    }

    pub fn with_stores(snapshots: Arc<Snapshots>, preset: Arc<CurrentPreset>) -> Self {
        Self { history: HistoryTap::new(HISTORY_RECORDS), spectrum: SpectrumTap::new(mite_dsp::analyzer::ANALYZER_SIZE), wet: SpectrumTap::new(mite_dsp::analyzer::ANALYZER_SIZE), sample_rate: AtomicF32::new(48_000.0), snapshots, preset }
    }
}

impl Default for VerbShared {
    fn default() -> Self {
        Self::new()
    }
}
