//! Mite Mod (docs/design/mod-brief.md): chorus, ensemble, flanger (through zero too), phaser and
//! vibrato, a rotary speaker and a micro pitch shifter in one plugin, on one smooth LFO (`mite_dsp::modulation`), each mode with its own view.

use std::sync::Arc;

use mite_core::presets::CurrentPreset;
use mite_core::snapshots::Snapshots;
use mite_core::modmatrix::MatrixTap;
use mite_core::{AtomicF32, HistoryTap};

pub mod dsp;
pub mod editor;
pub mod migrate;
pub mod params;
pub mod presets;

use dsp::{record, HISTORY_RECORDS};

/// Audio → UI telemetry. Lock-free and allocation-free on the audio side.
pub struct ModShared {
    pub history: HistoryTap<{ record::WIDTH }>,
    pub sample_rate: AtomicF32,
    /// The host's tempo (0: unknown), for a synced rate's reading.
    pub tempo: AtomicF32,
    pub snapshots: Arc<Snapshots>,
    pub preset: Arc<CurrentPreset>,
    /// The modulation matrix now (sources, LFO places, offsets).
    pub matrix: MatrixTap,
}

impl ModShared {
    pub fn new() -> Self {
        Self::with_stores(Arc::new(Snapshots::new()), Arc::new(CurrentPreset::default()))
    }

    pub fn with_stores(snapshots: Arc<Snapshots>, preset: Arc<CurrentPreset>) -> Self {
        Self { history: HistoryTap::new(HISTORY_RECORDS), sample_rate: AtomicF32::new(48_000.0), tempo: AtomicF32::new(0.0), snapshots, preset, matrix: MatrixTap::new() }
    }
}

impl Default for ModShared {
    fn default() -> Self {
        Self::new()
    }
}
