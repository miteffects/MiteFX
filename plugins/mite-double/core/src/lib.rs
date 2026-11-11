//! Mite Double (docs/design/double-brief.md): a smart guitar doubler. Every note of the part is
//! played again by up to three doubles, each landing a little early or late, a few cents off,
//! louder or softer, brighter or duller, differently every note (`mite_dsp::doubler`); the source
//! and each double placed in stereo.

use std::sync::Arc;

use mite_core::presets::CurrentPreset;
use mite_core::snapshots::Snapshots;
use mite_core::{AtomicF32, HistoryTap};

pub mod dsp;
pub mod editor;
pub mod params;
pub mod presets;

use dsp::{record, HISTORY_RECORDS};

/// Audio → UI telemetry. Lock-free and allocation-free on the audio side.
pub struct DoubleShared {
    pub history: HistoryTap<{ record::WIDTH }>,
    pub sample_rate: AtomicF32,
    pub snapshots: Arc<Snapshots>,
    pub preset: Arc<CurrentPreset>,
}

impl DoubleShared {
    pub fn new() -> Self {
        Self::with_stores(Arc::new(Snapshots::new()), Arc::new(CurrentPreset::default()))
    }

    pub fn with_stores(snapshots: Arc<Snapshots>, preset: Arc<CurrentPreset>) -> Self {
        Self { history: HistoryTap::new(HISTORY_RECORDS), sample_rate: AtomicF32::new(48_000.0), snapshots, preset }
    }
}

impl Default for DoubleShared {
    fn default() -> Self {
        Self::new()
    }
}
