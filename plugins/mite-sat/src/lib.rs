//! Mite Sat: saturation (BLUEPRINT §6.7, docs/design/sat-brief.md). One band or up to six, each
//! with its own model, drive, bias and tone, oversampled up to 8×.

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

use dsp::{record, Output, SatEngine, HISTORY_RECORDS};
use params::SatParams;

/// Audio → UI telemetry. Lock-free and allocation-free on the audio side.
pub struct SatShared {
    /// One record per 5 ms slice (`dsp::record`).
    pub history: HistoryTap<{ record::WIDTH }>,
    /// The input and the output (mono), for the spectrum and the harmonics it adds.
    pub spectrum: SpectrumTap,
    pub post: SpectrumTap,
    pub sample_rate: AtomicF32,
    /// The latency the engine runs with, samples.
    pub latency: AtomicF32,
    /// Snapshots A–D and the current preset (the parameters' stores, shared with the editor).
    pub snapshots: Arc<Snapshots>,
    pub preset: Arc<CurrentPreset>,
    /// The modulation matrix now (sources, LFO places, offsets).
    pub matrix: mite_core::modmatrix::MatrixTap,
}

impl SatShared {
    pub fn new() -> Self {
        Self::with_stores(Arc::new(Snapshots::new()), Arc::new(CurrentPreset::default()))
    }

    pub fn with_stores(snapshots: Arc<Snapshots>, preset: Arc<CurrentPreset>) -> Self {
        Self {
            history: HistoryTap::new(HISTORY_RECORDS),
            spectrum: SpectrumTap::new(editor::ANALYZER_SIZE),
            post: SpectrumTap::new(editor::ANALYZER_SIZE),
            sample_rate: AtomicF32::new(48_000.0),
            latency: AtomicF32::new(mite_dsp::oversample::Oversampler::latency_for(2) as f32),
            snapshots,
            preset,
            matrix: mite_core::modmatrix::MatrixTap::new(),
        }
    }
}

impl Default for SatShared {
    fn default() -> Self {
        Self::new()
    }
}

pub struct MiteSat {
    params: Arc<SatParams>,
    engine: SatEngine,
    matrix: mite_plugin::matrix::MatrixRunner,
    shared: Arc<SatShared>,
    /// The latency last reported to the host, samples.
    reported_latency: u32,
}

impl MiteSat {
    fn output(&self) -> Output {
        let p = &self.params;
        Output { mix: p.mix.value() / 100.0, delta: p.delta.value(), bypass: p.bypass.value(), gain_db: p.out_gain.value(), pan: p.out_pan.value() / 100.0 }
    }
}

impl Default for MiteSat {
    fn default() -> Self {
        let params = Arc::new(SatParams::default());
        let engine = SatEngine::new(48_000.0, params.factor(), &params.settings());
        let shared = Arc::new(SatShared::with_stores(params.snapshots.clone(), params.preset.clone()));
        Self { params, engine, matrix: Default::default(), shared, reported_latency: 0 }
    }
}

impl Plugin for MiteSat {
    const NAME: &'static str = "Mite Sat";
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
        mite_plugin::create_editor(self.params.clone(), "Mite Sat", editor::ACCENT, move || editor::SatView::new(shared.clone()))
    }

    fn initialize(&mut self, _layout: &AudioIOLayout, config: &BufferConfig, context: &mut impl InitContext<Self>) -> bool {
        let settings = self.params.settings();
        self.engine.prepare(config.sample_rate, self.params.factor(), &settings, &self.output(), self.params.linear());
        self.shared.sample_rate.store(config.sample_rate);
        self.matrix.prepare(config.sample_rate, config.max_buffer_size as usize);
        self.reported_latency = self.engine.latency() as u32;
        context.set_latency_samples(self.reported_latency);
        self.shared.latency.store(self.reported_latency as f32);
        true
    }

    fn reset(&mut self) {
        let settings = self.params.settings();
        let output = self.output();
        self.engine.reset(&settings, &output);
        self.matrix.reset();
    }

    fn process(&mut self, buffer: &mut Buffer, _aux: &mut AuxiliaryBuffers, context: &mut impl ProcessContext<Self>) -> ProcessStatus {
        let t = context.transport();
        let transport = mite_dsp::modulation::Transport { tempo: t.tempo.filter(|v| v.is_finite() && *v > 0.0), beats: t.pos_beats(), playing: t.playing };
        let (settings, output, factor, linear) = (self.params.settings(), self.output(), self.params.factor(), self.params.linear());
        let ms = self.params.matrix.settings();
        let (engine, shared) = (&mut self.engine, &self.shared);
        self.matrix.run(&ms, &transport, buffer.as_slice(), None, &self.shared.matrix, params::MOD_TARGETS.len(), |sub, _, offsets| {
            if offsets.iter().any(|o| *o != 0.0) {
                let (mut s, mut out) = (settings, output);
                params::modulate_settings(&mut s, &mut out, offsets);
                engine.process(sub, &s, factor, linear, &out, &shared.history, Some([&shared.spectrum, &shared.post]));
            } else {
                engine.process(sub, &settings, factor, linear, &output, &shared.history, Some([&shared.spectrum, &shared.post]));
            }
        });
        // An oversampling or phase change switched the latency behind its dip: tell the host.
        let latency = self.engine.latency() as u32;
        if latency != self.reported_latency {
            self.reported_latency = latency;
            context.set_latency_samples(latency);
            self.shared.latency.store(latency as f32);
        }
        ProcessStatus::Normal
    }
}

impl ClapPlugin for MiteSat {
    const CLAP_ID: &'static str = "io.github.ansabic.mitefx.sat";
    const CLAP_DESCRIPTION: Option<&'static str> = Some("Saturation");
    const CLAP_MANUAL_URL: Option<&'static str> = Some(Self::URL);
    const CLAP_SUPPORT_URL: Option<&'static str> = Some("https://github.com/ansabic/MiteFX/issues");
    const CLAP_FEATURES: &'static [ClapFeature] = &[ClapFeature::AudioEffect, ClapFeature::Stereo, ClapFeature::Mono, ClapFeature::Distortion];
}

impl Vst3Plugin for MiteSat {
    const VST3_CLASS_ID: [u8; 16] = *b"MiteFX-Sat---001";
    const VST3_SUBCATEGORIES: &'static [Vst3SubCategory] = &[Vst3SubCategory::Fx, Vst3SubCategory::Distortion];
}

nih_export_clap!(MiteSat);
nih_export_vst3!(MiteSat);
