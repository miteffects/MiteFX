//! Mite Limit: a brickwall limiter (BLUEPRINT §6.3, docs/design/limit-brief.md). The ceiling
//! holds in sample peak always (proved) and in true peak when True Peak is on; oversampled up
//! to 16×, six styles, EBU R128 loudness.

use std::num::NonZeroU32;
use std::sync::Arc;

use mite_core::presets::CurrentPreset;
use mite_core::snapshots::Snapshots;
use mite_core::{AtomicF32, HistoryTap};
use nih_plug::prelude::*;

pub mod dsp;
pub mod editor;
pub mod params;
pub mod presets;

use dsp::{record, LimitEngine, LoudnessTap, HISTORY_RECORDS};
use params::LimitParams;

/// Audio → UI telemetry (and the loudness reset, UI → audio). Lock-free and allocation-free on
/// the audio side.
pub struct LimitShared {
    /// One record per 5 ms slice (`dsp::record`).
    pub history: HistoryTap<{ record::WIDTH }>,
    pub loudness: LoudnessTap,
    pub sample_rate: AtomicF32,
    /// The latency the engine runs with, samples.
    pub latency: AtomicF32,
    /// Snapshots A–D and the current preset (the parameters' stores, shared with the editor).
    pub snapshots: Arc<Snapshots>,
    pub preset: Arc<CurrentPreset>,
    /// The history's level range picked by hand (0: Auto; the parameters' store).
    pub display_range: Arc<AtomicF32>,
}

impl LimitShared {
    pub fn new() -> Self {
        Self::with_stores(Arc::new(Snapshots::new()), Arc::new(CurrentPreset::default()), Arc::new(AtomicF32::new(0.0)))
    }

    pub fn with_stores(snapshots: Arc<Snapshots>, preset: Arc<CurrentPreset>, display_range: Arc<AtomicF32>) -> Self {
        Self {
            history: HistoryTap::new(HISTORY_RECORDS),
            loudness: LoudnessTap::new(),
            sample_rate: AtomicF32::new(48_000.0),
            latency: AtomicF32::new(dsp::latency_for(&dsp::LimitSettings::default(), 48_000.0) as f32),
            snapshots,
            preset,
            display_range,
        }
    }
}

impl Default for LimitShared {
    fn default() -> Self {
        Self::new()
    }
}

pub struct MiteLimit {
    params: Arc<LimitParams>,
    engine: LimitEngine,
    shared: Arc<LimitShared>,
    /// The latency last reported to the host, samples.
    reported_latency: u32,
}

impl Default for MiteLimit {
    fn default() -> Self {
        let params = Arc::new(LimitParams::default());
        let engine = LimitEngine::new(48_000.0, &params.settings());
        let shared = Arc::new(LimitShared::with_stores(params.snapshots.clone(), params.preset.clone(), params.display_range.clone()));
        Self { params, engine, shared, reported_latency: 0 }
    }
}

impl Plugin for MiteLimit {
    const NAME: &'static str = "Mite Limit";
    const VENDOR: &'static str = "MiteFX";
    const URL: &'static str = "https://github.com/ansabic/MiteFX";
    const EMAIL: &'static str = "";
    const VERSION: &'static str = env!("CARGO_PKG_VERSION");

    const AUDIO_IO_LAYOUTS: &'static [AudioIOLayout] = &[
        AudioIOLayout { main_input_channels: NonZeroU32::new(2), main_output_channels: NonZeroU32::new(2), ..AudioIOLayout::const_default() },
        AudioIOLayout { main_input_channels: NonZeroU32::new(1), main_output_channels: NonZeroU32::new(1), ..AudioIOLayout::const_default() },
    ];

    const SAMPLE_ACCURATE_AUTOMATION: bool = true;

    type SysExMessage = ();
    type BackgroundTask = ();

    fn filter_state(state: &mut PluginState) {
        mite_plugin::state::migrate(state, params::STATE_VERSION, &[]);
    }

    fn params(&self) -> Arc<dyn Params> {
        self.params.clone()
    }

    fn editor(&mut self, _async_executor: AsyncExecutor<Self>) -> Option<Box<dyn Editor>> {
        let shared = self.shared.clone();
        mite_plugin::create_editor(self.params.clone(), "Mite Limit", editor::ACCENT, move || editor::LimitView::new(shared.clone()))
    }

    fn initialize(&mut self, _layout: &AudioIOLayout, config: &BufferConfig, context: &mut impl InitContext<Self>) -> bool {
        let settings = self.params.settings();
        self.engine.prepare(config.sample_rate, &settings);
        self.shared.sample_rate.store(config.sample_rate);
        self.reported_latency = self.engine.latency() as u32;
        context.set_latency_samples(self.reported_latency);
        self.shared.latency.store(self.reported_latency as f32);
        true
    }

    fn reset(&mut self) {
        let settings = self.params.settings();
        self.engine.reset(&settings);
    }

    fn process(&mut self, buffer: &mut Buffer, _aux: &mut AuxiliaryBuffers, context: &mut impl ProcessContext<Self>) -> ProcessStatus {
        let settings = self.params.settings();
        self.engine.process(buffer.as_slice(), &settings, &self.shared.history, &self.shared.loudness);
        // A lookahead, true peak or oversampling change switched the latency behind its dip.
        let latency = self.engine.latency() as u32;
        if latency != self.reported_latency {
            self.reported_latency = latency;
            context.set_latency_samples(latency);
            self.shared.latency.store(latency as f32);
        }
        ProcessStatus::Normal
    }
}

impl ClapPlugin for MiteLimit {
    const CLAP_ID: &'static str = "io.github.ansabic.mitefx.limit";
    const CLAP_DESCRIPTION: Option<&'static str> = Some("Brickwall limiter");
    const CLAP_MANUAL_URL: Option<&'static str> = Some(Self::URL);
    const CLAP_SUPPORT_URL: Option<&'static str> = Some("https://github.com/ansabic/MiteFX/issues");
    const CLAP_FEATURES: &'static [ClapFeature] = &[ClapFeature::AudioEffect, ClapFeature::Stereo, ClapFeature::Mono, ClapFeature::Limiter, ClapFeature::Mastering];
}

impl Vst3Plugin for MiteLimit {
    const VST3_CLASS_ID: [u8; 16] = *b"MiteFX-Limit-001";
    const VST3_SUBCATEGORIES: &'static [Vst3SubCategory] = &[Vst3SubCategory::Fx, Vst3SubCategory::Dynamics];
}

nih_export_clap!(MiteLimit);
nih_export_vst3!(MiteLimit);
