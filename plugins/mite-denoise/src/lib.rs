//! Mite DeNoise (docs/design/denoise-brief.md): spectral noise reduction. It learns the noise
//! from a stretch of noise only, or tracks the floor all the time, and turns each frequency down
//! while it sits under a threshold over that floor. Rival: Acon Digital DeNoise.

use std::sync::Arc;

use mite_core::presets::CurrentPreset;
use mite_core::snapshots::Snapshots;
use mite_core::AtomicF32;
use nih_plug::prelude::*;

pub mod dsp;
pub mod editor;
pub mod params;
pub mod presets;

use dsp::{DenoiseEngine, DisplayTap};
use params::{DenoiseParams, NoiseProfile};

/// Audio ↔ UI. Lock-free and allocation-free on the audio side.
pub struct DenoiseShared {
    pub tap: DisplayTap,
    pub profile: Arc<NoiseProfile>,
    pub sample_rate: AtomicF32,
    pub latency: AtomicF32,
    pub snapshots: Arc<Snapshots>,
    pub preset: Arc<CurrentPreset>,
}

impl DenoiseShared {
    pub fn new() -> Self {
        Self::with_stores(Arc::new(Snapshots::new()), Arc::new(CurrentPreset::default()), Arc::new(NoiseProfile::new()))
    }

    pub fn with_stores(snapshots: Arc<Snapshots>, preset: Arc<CurrentPreset>, profile: Arc<NoiseProfile>) -> Self {
        Self { tap: DisplayTap::new(), profile, sample_rate: AtomicF32::new(48_000.0), latency: AtomicF32::new(2048.0), snapshots, preset }
    }
}

impl Default for DenoiseShared {
    fn default() -> Self {
        Self::new()
    }
}

pub struct MiteDenoise {
    params: Arc<DenoiseParams>,
    engine: DenoiseEngine,
    shared: Arc<DenoiseShared>,
}

impl Default for MiteDenoise {
    fn default() -> Self {
        let params = Arc::new(DenoiseParams::default());
        let shared = Arc::new(DenoiseShared::with_stores(params.snapshots.clone(), params.preset.clone(), params.profile.clone()));
        Self { params, engine: DenoiseEngine::new(48_000.0), shared }
    }
}

impl Plugin for MiteDenoise {
    const NAME: &'static str = "Mite DeNoise";
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
        mite_plugin::state::migrate(state, params::STATE_VERSION, &[]);
    }

    fn params(&self) -> Arc<dyn Params> {
        self.params.clone()
    }

    fn editor(&mut self, _async_executor: AsyncExecutor<Self>) -> Option<Box<dyn Editor>> {
        let shared = self.shared.clone();
        mite_plugin::create_editor(self.params.clone(), "Mite DeNoise", editor::ACCENT, move || editor::DenoiseView::new(shared.clone()))
    }

    fn initialize(&mut self, _layout: &AudioIOLayout, config: &BufferConfig, context: &mut impl InitContext<Self>) -> bool {
        self.engine.prepare(config.sample_rate, self.params.out_gain.value());
        self.shared.sample_rate.store(config.sample_rate);
        let latency = self.engine.latency() as u32;
        context.set_latency_samples(latency);
        self.shared.latency.store(latency as f32);
        true
    }

    fn reset(&mut self) {
        self.engine.reset(self.params.out_gain.value());
    }

    fn process(&mut self, buffer: &mut Buffer, _aux: &mut AuxiliaryBuffers, _context: &mut impl ProcessContext<Self>) -> ProcessStatus {
        let s = self.params.settings();
        self.engine.process(buffer.as_slice(), &s, self.params.out_gain.value(), &self.shared.profile, &self.shared.tap);
        ProcessStatus::Normal
    }
}

impl ClapPlugin for MiteDenoise {
    const CLAP_ID: &'static str = "io.github.ansabic.mitefx.denoise";
    const CLAP_DESCRIPTION: Option<&'static str> = Some("Spectral noise reduction");
    const CLAP_MANUAL_URL: Option<&'static str> = Some(Self::URL);
    const CLAP_SUPPORT_URL: Option<&'static str> = Some("https://github.com/ansabic/MiteFX/issues");
    const CLAP_FEATURES: &'static [ClapFeature] = &[ClapFeature::AudioEffect, ClapFeature::Stereo, ClapFeature::Mono, ClapFeature::Restoration];
}

impl Vst3Plugin for MiteDenoise {
    const VST3_CLASS_ID: [u8; 16] = *b"MiteFX-DeNoise01";
    const VST3_SUBCATEGORIES: &'static [Vst3SubCategory] = &[Vst3SubCategory::Fx, Vst3SubCategory::Restoration];
}

nih_export_clap!(MiteDenoise);
nih_export_vst3!(MiteDenoise);
