//! Mite Verb: an algorithmic reverb (BLUEPRINT §6.5, docs/design/verb-brief.md) with a free decay
//! EQ (decay time per frequency), a free tone EQ, freeze and ducking.

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

use dsp::{record, VerbEngine, HISTORY_RECORDS};
use params::VerbParams;

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

pub struct MiteVerb {
    params: Arc<VerbParams>,
    engine: VerbEngine,
    shared: Arc<VerbShared>,
}

impl Default for MiteVerb {
    fn default() -> Self {
        let params = Arc::new(VerbParams::default());
        let engine = VerbEngine::new(48_000.0, &params.settings(), &params.output());
        let shared = Arc::new(VerbShared::with_stores(params.snapshots.clone(), params.preset.clone()));
        Self { params, engine, shared }
    }
}

impl Plugin for MiteVerb {
    const NAME: &'static str = "Mite Verb";
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
        mite_plugin::state::migrate(state, params::STATE_VERSION, &[migrate::v1_to_v2, migrate::v2_to_v3, migrate::v3_to_v4]);
    }

    fn params(&self) -> Arc<dyn Params> {
        self.params.clone()
    }

    fn editor(&mut self, _async_executor: AsyncExecutor<Self>) -> Option<Box<dyn Editor>> {
        let shared = self.shared.clone();
        mite_plugin::create_editor(self.params.clone(), "Mite Verb", editor::ACCENT, move || editor::VerbView::new(shared.clone()))
    }

    fn initialize(&mut self, _layout: &AudioIOLayout, config: &BufferConfig, context: &mut impl InitContext<Self>) -> bool {
        self.engine.prepare(config.sample_rate, &self.params.settings(), &self.params.output());
        self.shared.sample_rate.store(config.sample_rate);
        context.set_latency_samples(0);
        true
    }

    fn reset(&mut self) {
        self.engine.reset(&self.params.settings(), &self.params.output());
    }

    fn process(&mut self, buffer: &mut Buffer, _aux: &mut AuxiliaryBuffers, _context: &mut impl ProcessContext<Self>) -> ProcessStatus {
        let (settings, out) = (self.params.settings(), self.params.output());
        self.engine.process(buffer.as_slice(), &settings, &out, &self.shared.history, Some([&self.shared.spectrum, &self.shared.wet]));
        // A reverb rings on after its input stops.
        ProcessStatus::KeepAlive
    }
}

impl ClapPlugin for MiteVerb {
    const CLAP_ID: &'static str = "io.github.ansabic.mitefx.verb";
    const CLAP_DESCRIPTION: Option<&'static str> = Some("Algorithmic reverb");
    const CLAP_MANUAL_URL: Option<&'static str> = Some(Self::URL);
    const CLAP_SUPPORT_URL: Option<&'static str> = Some("https://github.com/ansabic/MiteFX/issues");
    const CLAP_FEATURES: &'static [ClapFeature] = &[ClapFeature::AudioEffect, ClapFeature::Stereo, ClapFeature::Mono, ClapFeature::Reverb];
}

impl Vst3Plugin for MiteVerb {
    const VST3_CLASS_ID: [u8; 16] = *b"MiteFX-Verb---01";
    const VST3_SUBCATEGORIES: &'static [Vst3SubCategory] = &[Vst3SubCategory::Fx, Vst3SubCategory::Reverb];
}

nih_export_clap!(MiteVerb);
nih_export_vst3!(MiteVerb);
