//! Mite Comp: the compressor (BLUEPRINT §6.2). Up to four stages in series on one transfer
//! curve, seven Characters, Smart modes, a sidechain with its own detector EQ.

use std::sync::Arc;

use mite_core::presets::CurrentPreset;
use mite_core::snapshots::Snapshots;
use mite_core::{AtomicF32, HistoryTap};

pub mod dsp;
pub mod editor;
pub mod migrate;
pub mod params;
pub mod presets;

use dsp::{record, HISTORY_RECORDS};

/// Audio → UI telemetry, and the editor's Character audition (UI → audio). Lock-free and
/// allocation-free on the audio side.
pub struct CompShared {
    /// One record per 5 ms slice (`dsp::record`).
    pub history: HistoryTap<{ record::WIDTH }>,
    pub sample_rate: AtomicF32,
    /// The latency the engine runs with, samples.
    pub latency: AtomicF32,
    /// A Character being tried from the menu (its index), or −1: the engine plays it without the
    /// parameter changing, so hovering the menu never reaches host automation or undo.
    pub audition: AtomicF32,
    /// Snapshots A–D and the current preset (the parameters' stores, shared with the editor).
    pub snapshots: Arc<Snapshots>,
    pub preset: Arc<CurrentPreset>,
    /// The history's level range picked by hand (0: Auto; the parameters' store).
    pub display_range: Arc<AtomicF32>,
    /// The editor shows every stage (the strip, pulling the line makes more); off, it is one
    /// compressor (the parameters' store, saved with the session).
    pub multi_stage: Arc<std::sync::atomic::AtomicBool>,
}

impl CompShared {
    pub fn new() -> Self {
        Self::with_stores(Arc::new(Snapshots::new()), Arc::new(CurrentPreset::default()))
    }

    pub fn with_stores(snapshots: Arc<Snapshots>, preset: Arc<CurrentPreset>) -> Self {
        Self {
            history: HistoryTap::new(HISTORY_RECORDS),
            sample_rate: AtomicF32::new(48_000.0),
            latency: AtomicF32::new(0.0),
            audition: AtomicF32::new(-1.0),
            snapshots,
            preset,
            display_range: Arc::new(AtomicF32::new(0.0)),
            multi_stage: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        }
    }

    /// The Character being auditioned, if any.
    pub fn audition(&self) -> Option<usize> {
        let a = self.audition.load();
        (a >= 0.0).then_some(a as usize)
    }
}

impl Default for CompShared {
    fn default() -> Self {
        Self::new()
    }
}
