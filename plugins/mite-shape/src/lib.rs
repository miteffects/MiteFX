//! Mite Shape: the transient designer (BLUEPRINT §6.10). Attack and sustain with no threshold,
//! on one band or split into up to five, each with its own shape.

use std::sync::Arc;

use mite_core::presets::CurrentPreset;
use mite_core::snapshots::Snapshots;
use mite_core::{AtomicF32, HistoryTap, SpectrumTap};
use nih_plug::prelude::*;

pub mod dsp;
pub mod editor;
pub mod params;
pub mod presets;

use dsp::{record, Output, ShapeEngine, HISTORY_RECORDS};
use params::ShapeParams;

/// Audio → UI telemetry. Lock-free and allocation-free on the audio side.
pub struct ShapeShared {
    /// One record per 5 ms slice (`dsp::record`).
    pub history: HistoryTap<{ record::WIDTH }>,
    /// The input and the output (mono), for the spectra under the crossovers.
    pub spectrum: SpectrumTap,
    pub post: SpectrumTap,
    pub sample_rate: AtomicF32,
    /// The latency the engine runs with, samples.
    pub latency: AtomicF32,
    /// Snapshots A–D and the current preset (the parameters' stores, shared with the editor).
    pub snapshots: Arc<Snapshots>,
    pub preset: Arc<CurrentPreset>,
}

impl ShapeShared {
    pub fn new() -> Self {
        Self::with_stores(Arc::new(Snapshots::new()), Arc::new(CurrentPreset::default()))
    }

    pub fn with_stores(snapshots: Arc<Snapshots>, preset: Arc<CurrentPreset>) -> Self {
        Self {
            history: HistoryTap::new(HISTORY_RECORDS),
            spectrum: SpectrumTap::new(editor::ANALYZER_SIZE),
            post: SpectrumTap::new(editor::ANALYZER_SIZE),
            sample_rate: AtomicF32::new(48_000.0),
            latency: AtomicF32::new(0.0),
            snapshots,
            preset,
        }
    }
}

impl Default for ShapeShared {
    fn default() -> Self {
        Self::new()
    }
}

pub struct MiteShape {
    params: Arc<ShapeParams>,
    engine: ShapeEngine,
    shared: Arc<ShapeShared>,
    /// The latency last reported to the host, samples.
    reported_latency: u32,
}

impl MiteShape {
    fn output(&self) -> Output {
        let p = &self.params;
        Output {
            mix: p.mix.value() / 100.0,
            delta: p.delta.value(),
            bypass: p.bypass.value(),
            auto_gain: p.auto_gain.value(),
            gain_db: p.out_gain.value(),
            pan: p.out_pan.value() / 100.0,
            clip: p.clip.value(),
            ceiling_db: p.clip_ceiling.value(),
            softness: p.clip_soft.value() / 100.0,
        }
    }
}

impl Default for MiteShape {
    fn default() -> Self {
        let params = Arc::new(ShapeParams::default());
        let engine = ShapeEngine::new(48_000.0, &params.settings());
        let shared = Arc::new(ShapeShared::with_stores(params.snapshots.clone(), params.preset.clone()));
        Self { params, engine, shared, reported_latency: 0 }
    }
}

impl Plugin for MiteShape {
    const NAME: &'static str = "Mite Shape";
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
        mite_plugin::create_editor(self.params.clone(), "Mite Shape", editor::ACCENT, move || editor::ShapeView::new(shared.clone()))
    }

    fn initialize(&mut self, _layout: &AudioIOLayout, config: &BufferConfig, context: &mut impl InitContext<Self>) -> bool {
        let settings = self.params.settings();
        self.engine.prepare(config.sample_rate, &settings, &self.output());
        self.shared.sample_rate.store(config.sample_rate);
        self.reported_latency = self.engine.latency() as u32;
        context.set_latency_samples(self.reported_latency);
        self.shared.latency.store(self.reported_latency as f32);
        true
    }

    fn reset(&mut self) {
        let settings = self.params.settings();
        let output = self.output();
        self.engine.reset(&settings, &output);
    }

    fn process(&mut self, buffer: &mut Buffer, _aux: &mut AuxiliaryBuffers, context: &mut impl ProcessContext<Self>) -> ProcessStatus {
        let settings = self.params.settings();
        let output = self.output();
        self.engine.process(buffer.as_slice(), &settings, &output, &self.shared.history, Some([&self.shared.spectrum, &self.shared.post]));
        // A lookahead change switched the latency behind its dip: tell the host.
        let latency = self.engine.latency() as u32;
        if latency != self.reported_latency {
            self.reported_latency = latency;
            context.set_latency_samples(latency);
            self.shared.latency.store(latency as f32);
        }
        ProcessStatus::Normal
    }
}

impl ClapPlugin for MiteShape {
    const CLAP_ID: &'static str = "io.github.ansabic.mitefx.shape";
    const CLAP_DESCRIPTION: Option<&'static str> = Some("Transient designer");
    const CLAP_MANUAL_URL: Option<&'static str> = Some(Self::URL);
    const CLAP_SUPPORT_URL: Option<&'static str> = Some("https://github.com/ansabic/MiteFX/issues");
    const CLAP_FEATURES: &'static [ClapFeature] = &[ClapFeature::AudioEffect, ClapFeature::Stereo, ClapFeature::Mono, ClapFeature::TransientShaper];
}

impl Vst3Plugin for MiteShape {
    const VST3_CLASS_ID: [u8; 16] = *b"MiteFX-Shape-001";
    const VST3_SUBCATEGORIES: &'static [Vst3SubCategory] = &[Vst3SubCategory::Fx, Vst3SubCategory::Dynamics];
}

nih_export_clap!(MiteShape);
nih_export_vst3!(MiteShape);
