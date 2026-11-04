//! Mite Tuner (docs/design/tuner-brief.md): a big, precise chromatic tuner for guitar and bass.
//! The audio passes untouched (or muted); the editor reads the input from a tap and finds its
//! pitch (`mite_dsp::pitch`, the McLeod pitch method).

use std::sync::Arc;

use mite_core::presets::CurrentPreset;
use mite_core::snapshots::Snapshots;
use mite_core::{AtomicF32, HistoryTap, SpectrumTap};
use nih_plug::prelude::*;

pub mod dsp;
pub mod editor;
pub mod params;
pub mod presets;

use dsp::{record, TunerEngine, HISTORY_RECORDS};
use params::TunerParams;

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

pub struct MiteTuner {
    params: Arc<TunerParams>,
    engine: TunerEngine,
    shared: Arc<TunerShared>,
}

impl Default for MiteTuner {
    fn default() -> Self {
        let params = Arc::new(TunerParams::default());
        let engine = TunerEngine::new(48_000.0, false);
        let shared = Arc::new(TunerShared::with_stores(params.snapshots.clone(), params.preset.clone()));
        Self { params, engine, shared }
    }
}

impl Plugin for MiteTuner {
    const NAME: &'static str = "Mite Tuner";
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
        mite_plugin::create_editor(self.params.clone(), "Mite Tuner", editor::ACCENT, move || editor::TunerView::new(shared.clone()))
    }

    fn initialize(&mut self, _layout: &AudioIOLayout, config: &BufferConfig, context: &mut impl InitContext<Self>) -> bool {
        self.engine.prepare(config.sample_rate, self.params.mute.value());
        self.shared.sample_rate.store(config.sample_rate);
        context.set_latency_samples(0);
        true
    }

    fn reset(&mut self) {
        self.engine.reset(self.params.mute.value());
    }

    fn process(&mut self, buffer: &mut Buffer, _aux: &mut AuxiliaryBuffers, _context: &mut impl ProcessContext<Self>) -> ProcessStatus {
        self.engine.process(buffer.as_slice(), self.params.mute.value(), &self.shared.history, Some(&self.shared.input));
        ProcessStatus::Normal
    }
}

impl ClapPlugin for MiteTuner {
    const CLAP_ID: &'static str = "io.github.ansabic.mitefx.tuner";
    const CLAP_DESCRIPTION: Option<&'static str> = Some("Chromatic tuner for guitar and bass");
    const CLAP_MANUAL_URL: Option<&'static str> = Some(Self::URL);
    const CLAP_SUPPORT_URL: Option<&'static str> = Some("https://github.com/ansabic/MiteFX/issues");
    const CLAP_FEATURES: &'static [ClapFeature] = &[ClapFeature::AudioEffect, ClapFeature::Stereo, ClapFeature::Mono, ClapFeature::Analyzer, ClapFeature::Utility];
}

impl Vst3Plugin for MiteTuner {
    const VST3_CLASS_ID: [u8; 16] = *b"MiteFX-Tuner-01-";
    const VST3_SUBCATEGORIES: &'static [Vst3SubCategory] = &[Vst3SubCategory::Fx, Vst3SubCategory::Analyzer, Vst3SubCategory::Tools];
}

nih_export_clap!(MiteTuner);
nih_export_vst3!(MiteTuner);
