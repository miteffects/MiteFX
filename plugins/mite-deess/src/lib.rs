//! Mite DeEss (BLUEPRINT §6.8, docs/design/deess-brief.md): a de-esser that hears a sibilant as
//! the top of the voice loud against the voice itself (or past a level), and turns it down with a
//! dynamic shelf, a dynamic band or the whole signal, 3 ms ahead.

use std::sync::Arc;

use mite_core::presets::CurrentPreset;
use mite_core::snapshots::Snapshots;
use mite_core::{AtomicF32, HistoryTap, SpectrumTap};
use nih_plug::prelude::*;

pub mod dsp;
pub mod editor;
pub mod migrate;
pub mod params;
pub mod presets;

use dsp::{record, DeEssEngine, GainsTap, HISTORY_RECORDS};
use params::DeEssParams;

/// Audio → UI telemetry. Lock-free and allocation-free on the audio side.
pub struct DeEssShared {
    pub history: HistoryTap<{ record::WIDTH }>,
    /// The input and the output (mono), for the spectrum.
    pub spectrum: SpectrumTap,
    pub output: SpectrumTap,
    /// The spectral reduction per bin (spectral mode).
    pub gains: GainsTap,
    pub sample_rate: AtomicF32,
    /// The latency the engine runs with, samples.
    pub latency: AtomicF32,
    pub snapshots: Arc<Snapshots>,
    pub preset: Arc<CurrentPreset>,
}

impl DeEssShared {
    pub fn new() -> Self {
        Self::with_stores(Arc::new(Snapshots::new()), Arc::new(CurrentPreset::default()))
    }

    pub fn with_stores(snapshots: Arc<Snapshots>, preset: Arc<CurrentPreset>) -> Self {
        Self {
            history: HistoryTap::new(HISTORY_RECORDS),
            spectrum: SpectrumTap::new(mite_dsp::analyzer::ANALYZER_SIZE),
            output: SpectrumTap::new(mite_dsp::analyzer::ANALYZER_SIZE),
            gains: GainsTap::new(),
            sample_rate: AtomicF32::new(48_000.0),
            latency: AtomicF32::new(0.0),
            snapshots,
            preset,
        }
    }
}

impl Default for DeEssShared {
    fn default() -> Self {
        Self::new()
    }
}

pub struct MiteDeEss {
    params: Arc<DeEssParams>,
    engine: DeEssEngine,
    shared: Arc<DeEssShared>,
    /// The latency last reported to the host, samples.
    reported_latency: u32,
}

impl Default for MiteDeEss {
    fn default() -> Self {
        let params = Arc::new(DeEssParams::default());
        let engine = DeEssEngine::new(48_000.0, &params.settings(), params.out_gain.value());
        let shared = Arc::new(DeEssShared::with_stores(params.snapshots.clone(), params.preset.clone()));
        Self { params, engine, shared, reported_latency: 0 }
    }
}

impl Plugin for MiteDeEss {
    const NAME: &'static str = "Mite DeEss";
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
        mite_plugin::state::migrate(state, params::STATE_VERSION, &[migrate::v1_to_v2]);
    }

    fn params(&self) -> Arc<dyn Params> {
        self.params.clone()
    }

    fn editor(&mut self, _async_executor: AsyncExecutor<Self>) -> Option<Box<dyn Editor>> {
        let shared = self.shared.clone();
        mite_plugin::create_editor(self.params.clone(), "Mite DeEss", editor::ACCENT, move || editor::DeEssView::new(shared.clone()))
    }

    fn initialize(&mut self, _layout: &AudioIOLayout, config: &BufferConfig, context: &mut impl InitContext<Self>) -> bool {
        self.engine.prepare(config.sample_rate, &self.params.settings(), self.params.out_gain.value());
        self.shared.sample_rate.store(config.sample_rate);
        self.reported_latency = self.engine.latency() as u32;
        context.set_latency_samples(self.reported_latency);
        self.shared.latency.store(self.reported_latency as f32);
        true
    }

    fn reset(&mut self) {
        self.engine.reset(&self.params.settings(), self.params.out_gain.value());
    }

    fn process(&mut self, buffer: &mut Buffer, _aux: &mut AuxiliaryBuffers, context: &mut impl ProcessContext<Self>) -> ProcessStatus {
        let s = self.params.settings();
        self.engine.process(buffer.as_slice(), &s, self.params.out_gain.value(), &self.shared.history, Some([&self.shared.spectrum, &self.shared.output]), Some(&self.shared.gains));
        // Lookahead switched (behind a dip): tell the host.
        let latency = self.engine.latency() as u32;
        if latency != self.reported_latency {
            self.reported_latency = latency;
            context.set_latency_samples(latency);
            self.shared.latency.store(latency as f32);
        }
        ProcessStatus::Normal
    }
}

impl ClapPlugin for MiteDeEss {
    const CLAP_ID: &'static str = "io.github.ansabic.mitefx.deess";
    const CLAP_DESCRIPTION: Option<&'static str> = Some("De-esser");
    const CLAP_MANUAL_URL: Option<&'static str> = Some(Self::URL);
    const CLAP_SUPPORT_URL: Option<&'static str> = Some("https://github.com/ansabic/MiteFX/issues");
    const CLAP_FEATURES: &'static [ClapFeature] = &[ClapFeature::AudioEffect, ClapFeature::Stereo, ClapFeature::Mono, ClapFeature::Compressor];
}

impl Vst3Plugin for MiteDeEss {
    const VST3_CLASS_ID: [u8; 16] = *b"MiteFX-DeEss--01";
    const VST3_SUBCATEGORIES: &'static [Vst3SubCategory] = &[Vst3SubCategory::Fx, Vst3SubCategory::Dynamics];
}

nih_export_clap!(MiteDeEss);
nih_export_vst3!(MiteDeEss);
