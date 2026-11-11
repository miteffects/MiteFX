//! Mite Amp (docs/design/amp-brief.md): a Neural Amp Modeler player. Two captures in a row (a
//! pedal into an amp), a tone stack, a cabinet impulse response; A1 and A2 captures, their sizes
//! and calibration; TONE3000's library one click away. Rival: the NAM and TONE3000 plugins.

use std::sync::Arc;

use mite_core::presets::CurrentPreset;
use mite_core::snapshots::Snapshots;
use mite_core::AtomicF32;
use nih_plug::prelude::*;

pub mod bracket;
pub mod dsp;
pub mod editor;
pub mod ir;
pub mod loader;
pub mod migrate;
pub mod params;
pub mod presets;
pub mod rig;
pub mod tone3000;

use dsp::{AmpEngine, AmpShared};
use loader::{Loader, LoaderCore};
use params::AmpParams;
use rig::Rig;

/// What the editor shares with the plugin.
pub struct AmpUi {
    pub rig: Arc<Rig>,
    pub engine: Arc<AmpShared>,
    pub tone3000: Arc<tone3000::Session>,
    pub sample_rate: AtomicF32,
    pub latency: AtomicF32,
    pub snapshots: Arc<Snapshots>,
    pub preset: Arc<CurrentPreset>,
}

impl AmpUi {
    pub fn new() -> Self {
        Self::with_stores(Arc::new(Snapshots::new()), Arc::new(CurrentPreset::default()), Arc::new(Rig::new()), Arc::new(AmpShared::new()))
    }

    pub fn with_stores(snapshots: Arc<Snapshots>, preset: Arc<CurrentPreset>, rig: Arc<Rig>, engine: Arc<AmpShared>) -> Self {
        Self { rig, engine, tone3000: Arc::new(tone3000::Session::new()), sample_rate: AtomicF32::new(48_000.0), latency: AtomicF32::new(0.0), snapshots, preset }
    }
}

impl Default for AmpUi {
    fn default() -> Self {
        Self::new()
    }
}

pub struct MiteAmp {
    params: Arc<AmpParams>,
    engine: AmpEngine,
    ui: Arc<AmpUi>,
    loader: Loader,
    reported_latency: u32,
}

impl Default for MiteAmp {
    fn default() -> Self {
        let params = Arc::new(AmpParams::default());
        let shared = Arc::new(AmpShared::new());
        let ui = Arc::new(AmpUi::with_stores(params.snapshots.clone(), params.preset.clone(), params.rig.clone(), shared.clone()));
        let loader = Loader::new(LoaderCore::new(params.rig.clone(), shared, params.clone()));
        Self { engine: AmpEngine::new(48_000.0, &params.settings()), params, ui, loader, reported_latency: 0 }
    }
}

impl Plugin for MiteAmp {
    const NAME: &'static str = "Mite Amp";
    const VENDOR: &'static str = "MiteFX";
    const URL: &'static str = "https://github.com/ansabic/MiteFX";
    const EMAIL: &'static str = "";
    const VERSION: &'static str = env!("CARGO_PKG_VERSION");

    const AUDIO_IO_LAYOUTS: &'static [AudioIOLayout] = &[
        AudioIOLayout { main_input_channels: NonZeroU32::new(2), main_output_channels: NonZeroU32::new(2), ..AudioIOLayout::const_default() },
        AudioIOLayout { main_input_channels: NonZeroU32::new(1), main_output_channels: NonZeroU32::new(1), ..AudioIOLayout::const_default() },
    ];

    const SAMPLE_ACCURATE_AUTOMATION: bool = false;

    type SysExMessage = ();
    type BackgroundTask = ();

    fn filter_state(state: &mut PluginState) {
        mite_plugin::state::migrate(state, params::STATE_VERSION, &[migrate::v1_to_v2]);
    }

    fn params(&self) -> Arc<dyn Params> {
        self.params.clone()
    }

    fn editor(&mut self, _async_executor: AsyncExecutor<Self>) -> Option<Box<dyn Editor>> {
        let ui = self.ui.clone();
        mite_plugin::create_editor(self.params.clone(), "Mite Amp", editor::ACCENT, move || editor::AmpView::new(ui.clone()))
    }

    fn initialize(&mut self, _layout: &AudioIOLayout, config: &BufferConfig, context: &mut impl InitContext<Self>) -> bool {
        let s = self.params.settings();
        self.engine.prepare(config.sample_rate, &s);
        self.loader.core.set_rate(config.sample_rate);
        // The rig, built now: the first block already plays it.
        let (models, cab) = self.loader.core.build_all();
        drop(self.engine.install_now(models, cab));
        self.engine.reset(&s);
        self.ui.sample_rate.store(config.sample_rate);
        self.reported_latency = self.engine.latency() as u32;
        context.set_latency_samples(self.reported_latency);
        self.ui.latency.store(self.reported_latency as f32);
        true
    }

    fn reset(&mut self) {
        self.engine.reset(&self.params.settings());
    }

    fn process(&mut self, buffer: &mut Buffer, _aux: &mut AuxiliaryBuffers, context: &mut impl ProcessContext<Self>) -> ProcessStatus {
        let s = self.params.settings();
        self.engine.process(buffer.as_slice(), &s, &self.ui.engine);
        // A capture of another rate (or none any more) moved the latency behind its dip.
        let latency = self.engine.latency() as u32;
        if latency != self.reported_latency {
            self.reported_latency = latency;
            context.set_latency_samples(latency);
            self.ui.latency.store(latency as f32);
        }
        ProcessStatus::Normal
    }
}

impl ClapPlugin for MiteAmp {
    const CLAP_ID: &'static str = "io.github.ansabic.mitefx.amp";
    const CLAP_DESCRIPTION: Option<&'static str> = Some("Neural amp captures (NAM A1 and A2), tone stack and cabinet");
    const CLAP_MANUAL_URL: Option<&'static str> = Some(Self::URL);
    const CLAP_SUPPORT_URL: Option<&'static str> = Some("https://github.com/ansabic/MiteFX/issues");
    const CLAP_FEATURES: &'static [ClapFeature] = &[ClapFeature::AudioEffect, ClapFeature::Stereo, ClapFeature::Mono, ClapFeature::Distortion];
}

impl Vst3Plugin for MiteAmp {
    const VST3_CLASS_ID: [u8; 16] = *b"MiteFX-Amp---001";
    const VST3_SUBCATEGORIES: &'static [Vst3SubCategory] = &[Vst3SubCategory::Fx, Vst3SubCategory::Distortion];
}

nih_export_clap!(MiteAmp);
nih_export_vst3!(MiteAmp);
