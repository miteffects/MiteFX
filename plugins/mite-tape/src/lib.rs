//! Mite Tape (BLUEPRINT §6, "Mite Tape"): a tape machine as a plugin of its own (2026-11-26, moved
//! out of Mite Sat, where a deck after the bands sat beside Sat's own Tape curve). Record level into
//! a magnetic core, the record and playback equalisers (NAB or CCIR), speed (head bump, the
//! head's high-frequency loss), bias, hiss, wow and flutter (`mite_dsp::tape`).

use std::sync::Arc;

use mite_core::presets::CurrentPreset;
use mite_core::snapshots::Snapshots;
use mite_core::{AtomicF32, SpectrumTap, StereoMeter};
use mite_dsp::MINUS_INFINITY_DB;
use nih_plug::prelude::*;

pub mod dsp;
pub mod editor;
pub mod params;
pub mod presets;

use dsp::TapeEngine;
use params::TapeParams;

/// Audio → UI telemetry. Lock-free and allocation-free on the audio side.
pub struct TapeShared {
    /// The input and the output (mono), for the spectrum.
    pub spectrum: SpectrumTap,
    pub output: SpectrumTap,
    /// Peak in and out per 10 ms slice, dBFS.
    pub input_meter: StereoMeter,
    pub output_meter: StereoMeter,
    /// The transport's speed against nominal and the peak going into the tape (linear).
    pub speed_dev: AtomicF32,
    pub field_peak: AtomicF32,
    pub sample_rate: AtomicF32,
    /// The latency the engine runs with, samples.
    pub latency: AtomicF32,
    pub snapshots: Arc<Snapshots>,
    pub preset: Arc<CurrentPreset>,
}

impl TapeShared {
    pub fn new() -> Self {
        Self::with_stores(Arc::new(Snapshots::new()), Arc::new(CurrentPreset::default()))
    }

    pub fn with_stores(snapshots: Arc<Snapshots>, preset: Arc<CurrentPreset>) -> Self {
        Self {
            spectrum: SpectrumTap::new(mite_dsp::analyzer::ANALYZER_SIZE),
            output: SpectrumTap::new(mite_dsp::analyzer::ANALYZER_SIZE),
            input_meter: StereoMeter::new(MINUS_INFINITY_DB),
            output_meter: StereoMeter::new(MINUS_INFINITY_DB),
            speed_dev: AtomicF32::new(0.0),
            field_peak: AtomicF32::new(0.0),
            sample_rate: AtomicF32::new(48_000.0),
            latency: AtomicF32::new(0.0),
            snapshots,
            preset,
        }
    }

    /// Runs one block through `engine` and publishes what the editor draws.
    pub fn process(&self, engine: &mut TapeEngine, channels: &mut [&mut [f32]], s: &params::TapeSettings) {
        engine.process(channels, s, Some([&self.input_meter, &self.output_meter]), Some([&self.spectrum, &self.output]));
        let t = engine.telemetry();
        self.speed_dev.store(t.speed_dev);
        self.field_peak.store(t.field_peak);
    }
}

impl Default for TapeShared {
    fn default() -> Self {
        Self::new()
    }
}

pub struct MiteTape {
    params: Arc<TapeParams>,
    engine: TapeEngine,
    shared: Arc<TapeShared>,
}

impl Default for MiteTape {
    fn default() -> Self {
        let params = Arc::new(TapeParams::default());
        let engine = TapeEngine::new(48_000.0, &params.settings());
        let shared = Arc::new(TapeShared::with_stores(params.snapshots.clone(), params.preset.clone()));
        Self { params, engine, shared }
    }
}

impl Plugin for MiteTape {
    const NAME: &'static str = "Mite Tape";
    const VENDOR: &'static str = "MiteFX";
    const URL: &'static str = "https://github.com/miteffects/MiteFX";
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
        mite_plugin::create_editor(self.params.clone(), "Mite Tape", editor::ACCENT, move || editor::TapeView::new(shared.clone()))
    }

    fn initialize(&mut self, _layout: &AudioIOLayout, config: &BufferConfig, context: &mut impl InitContext<Self>) -> bool {
        self.engine.prepare(config.sample_rate, &self.params.settings());
        self.shared.sample_rate.store(config.sample_rate);
        let latency = self.engine.latency() as u32;
        context.set_latency_samples(latency);
        self.shared.latency.store(latency as f32);
        true
    }

    fn reset(&mut self) {
        self.engine.reset(&self.params.settings());
    }

    fn process(&mut self, buffer: &mut Buffer, _aux: &mut AuxiliaryBuffers, _context: &mut impl ProcessContext<Self>) -> ProcessStatus {
        let s = self.params.settings();
        self.shared.process(&mut self.engine, buffer.as_slice(), &s);
        ProcessStatus::Normal
    }
}

impl ClapPlugin for MiteTape {
    const CLAP_ID: &'static str = "io.github.ansabic.mitefx.tape";
    const CLAP_DESCRIPTION: Option<&'static str> = Some("Tape machine");
    const CLAP_MANUAL_URL: Option<&'static str> = Some(Self::URL);
    const CLAP_SUPPORT_URL: Option<&'static str> = Some("https://github.com/miteffects/MiteFX/issues");
    const CLAP_FEATURES: &'static [ClapFeature] = &[ClapFeature::AudioEffect, ClapFeature::Stereo, ClapFeature::Mono, ClapFeature::Distortion];
}

impl Vst3Plugin for MiteTape {
    const VST3_CLASS_ID: [u8; 16] = *b"MiteFX-Tape---01";
    const VST3_SUBCATEGORIES: &'static [Vst3SubCategory] = &[Vst3SubCategory::Fx, Vst3SubCategory::Distortion];
}

nih_export_clap!(MiteTape);
nih_export_vst3!(MiteTape);
