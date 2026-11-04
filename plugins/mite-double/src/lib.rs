//! Mite Double (docs/design/double-brief.md): a smart guitar doubler. Every note of the part is
//! played again by up to three doubles, each landing a little early or late, a few cents off,
//! louder or softer, brighter or duller, differently every note (`mite_dsp::doubler`); the source
//! and each double placed in stereo.

use std::sync::Arc;

use mite_core::presets::CurrentPreset;
use mite_core::snapshots::Snapshots;
use mite_core::{AtomicF32, HistoryTap};
use nih_plug::prelude::*;

pub mod dsp;
pub mod editor;
pub mod params;
pub mod presets;

use dsp::{record, DoubleEngine, HISTORY_RECORDS};
use params::DoubleParams;

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

pub struct MiteDouble {
    params: Arc<DoubleParams>,
    engine: DoubleEngine,
    shared: Arc<DoubleShared>,
    stereo_in: bool,
}

impl Default for MiteDouble {
    fn default() -> Self {
        let params = Arc::new(DoubleParams::default());
        let engine = DoubleEngine::new(48_000.0, &params.output());
        let shared = Arc::new(DoubleShared::with_stores(params.snapshots.clone(), params.preset.clone()));
        Self { params, engine, shared, stereo_in: true }
    }
}

impl Plugin for MiteDouble {
    const NAME: &'static str = "Mite Double";
    const VENDOR: &'static str = "MiteFX";
    const URL: &'static str = "https://github.com/ansabic/MiteFX";
    const EMAIL: &'static str = "";
    const VERSION: &'static str = env!("CARGO_PKG_VERSION");

    const AUDIO_IO_LAYOUTS: &'static [AudioIOLayout] = &[
        AudioIOLayout { main_input_channels: NonZeroU32::new(2), main_output_channels: NonZeroU32::new(2), ..AudioIOLayout::const_default() },
        AudioIOLayout { main_input_channels: NonZeroU32::new(1), main_output_channels: NonZeroU32::new(2), ..AudioIOLayout::const_default() },
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
        mite_plugin::create_editor(self.params.clone(), "Mite Double", editor::ACCENT, move || editor::DoubleView::new(shared.clone()))
    }

    fn initialize(&mut self, layout: &AudioIOLayout, config: &BufferConfig, context: &mut impl InitContext<Self>) -> bool {
        self.stereo_in = layout.main_input_channels.map_or(0, NonZeroU32::get) >= 2;
        self.engine.prepare(config.sample_rate, &self.params.output());
        self.shared.sample_rate.store(config.sample_rate);
        context.set_latency_samples(0);
        true
    }

    fn reset(&mut self) {
        self.engine.reset(&self.params.output());
    }

    fn process(&mut self, buffer: &mut Buffer, _aux: &mut AuxiliaryBuffers, _context: &mut impl ProcessContext<Self>) -> ProcessStatus {
        let (s, out) = (self.params.settings(), self.params.output());
        self.engine.process(buffer.as_slice(), self.stereo_in, &s, &out, &self.shared.history);
        ProcessStatus::Normal
    }
}

impl ClapPlugin for MiteDouble {
    const CLAP_ID: &'static str = "io.github.ansabic.mitefx.double";
    const CLAP_DESCRIPTION: Option<&'static str> = Some("Smart guitar doubler");
    const CLAP_MANUAL_URL: Option<&'static str> = Some(Self::URL);
    const CLAP_SUPPORT_URL: Option<&'static str> = Some("https://github.com/ansabic/MiteFX/issues");
    const CLAP_FEATURES: &'static [ClapFeature] = &[ClapFeature::AudioEffect, ClapFeature::Stereo, ClapFeature::Mono, ClapFeature::Chorus];
}

impl Vst3Plugin for MiteDouble {
    const VST3_CLASS_ID: [u8; 16] = *b"MiteFX-Double-01";
    const VST3_SUBCATEGORIES: &'static [Vst3SubCategory] = &[Vst3SubCategory::Fx, Vst3SubCategory::Modulation];
}

nih_export_clap!(MiteDouble);
nih_export_vst3!(MiteDouble);
