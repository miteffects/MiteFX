//! Mite Multiband: up to six bands of dynamics (BLUEPRINT §6.4, docs/design/multiband-brief.md)
//! on the shared crossover, with a sidechain and its own EQ.

use std::num::NonZeroU32;
use std::sync::Arc;

use mite_core::presets::CurrentPreset;
use mite_core::snapshots::Snapshots;
use mite_core::{AtomicF32, HistoryTap, SpectrumTap};
use nih_plug::prelude::*;

pub mod dsp;
pub mod editor;
pub mod params;
pub mod presets;

use dsp::{record, MbEngine, HISTORY_RECORDS};
use params::MbParams;

/// Audio → UI telemetry. Lock-free and allocation-free on the audio side.
pub struct MbShared {
    pub history: HistoryTap<{ record::WIDTH }>,
    /// The input and the output (mono), for the spectrum.
    pub spectrum: SpectrumTap,
    pub post: SpectrumTap,
    pub sample_rate: AtomicF32,
    pub latency: AtomicF32,
    pub snapshots: Arc<Snapshots>,
    pub preset: Arc<CurrentPreset>,
    }

impl MbShared {
    pub fn new() -> Self {
        Self::with_stores(Arc::new(Snapshots::new()), Arc::new(CurrentPreset::default()))
    }

    pub fn with_stores(snapshots: Arc<Snapshots>, preset: Arc<CurrentPreset>) -> Self {
        Self { history: HistoryTap::new(HISTORY_RECORDS), spectrum: SpectrumTap::new(mite_dsp::analyzer::ANALYZER_SIZE), post: SpectrumTap::new(mite_dsp::analyzer::ANALYZER_SIZE), sample_rate: AtomicF32::new(48_000.0), latency: AtomicF32::new(0.0), snapshots, preset }
    }
}

impl Default for MbShared {
    fn default() -> Self {
        Self::new()
    }
}

pub struct MiteMultiband {
    params: Arc<MbParams>,
    engine: MbEngine,
    shared: Arc<MbShared>,
    reported_latency: u32,
}

impl Default for MiteMultiband {
    fn default() -> Self {
        let params = Arc::new(MbParams::default());
        let engine = MbEngine::new(48_000.0, &params.settings());
        let shared = Arc::new(MbShared::with_stores(params.snapshots.clone(), params.preset.clone()));
        Self { params, engine, shared, reported_latency: 0 }
    }
}

impl Plugin for MiteMultiband {
    const NAME: &'static str = "Mite Multiband";
    const VENDOR: &'static str = "MiteFX";
    const URL: &'static str = "https://github.com/ansabic/MiteFX";
    const EMAIL: &'static str = "";
    const VERSION: &'static str = env!("CARGO_PKG_VERSION");

    const AUDIO_IO_LAYOUTS: &'static [AudioIOLayout] = &[
        // The sidechain feeds the detector when the source is External (silent when unconnected).
        AudioIOLayout {
            main_input_channels: NonZeroU32::new(2),
            main_output_channels: NonZeroU32::new(2),
            aux_input_ports: &[new_nonzero_u32(2)],
            names: PortNames { aux_inputs: &["Sidechain"], ..PortNames::const_default() },
            ..AudioIOLayout::const_default()
        },
        AudioIOLayout {
            main_input_channels: NonZeroU32::new(1),
            main_output_channels: NonZeroU32::new(1),
            aux_input_ports: &[new_nonzero_u32(1)],
            names: PortNames { aux_inputs: &["Sidechain"], ..PortNames::const_default() },
            ..AudioIOLayout::const_default()
        },
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
        mite_plugin::create_editor(self.params.clone(), "Mite Multiband", editor::ACCENT, move || editor::MbView::new(shared.clone()))
    }

    fn initialize(&mut self, _layout: &AudioIOLayout, config: &BufferConfig, context: &mut impl InitContext<Self>) -> bool {
        self.engine.prepare(config.sample_rate, &self.params.settings());
        self.shared.sample_rate.store(config.sample_rate);
        self.reported_latency = self.engine.latency() as u32;
        context.set_latency_samples(self.reported_latency);
        self.shared.latency.store(self.reported_latency as f32);
        true
    }

    fn reset(&mut self) {
        self.engine.reset(&self.params.settings());
    }

    fn process(&mut self, buffer: &mut Buffer, aux: &mut AuxiliaryBuffers, context: &mut impl ProcessContext<Self>) -> ProcessStatus {
        let settings = self.params.settings();
        let mut key: [&[f32]; 2] = [&[], &[]];
        let mut key_channels = 0;
        if let Some(sc) = aux.inputs.first_mut() {
            for (slot, ch) in key.iter_mut().zip(sc.as_slice_immutable()) {
                *slot = ch;
                key_channels += 1;
            }
        }
        let key = (key_channels > 0).then_some(&key[..key_channels]);
        self.engine.process(buffer.as_slice(), key, &settings, &self.shared.history, Some([&self.shared.spectrum, &self.shared.post]));
        let latency = self.engine.latency() as u32;
        if latency != self.reported_latency {
            self.reported_latency = latency;
            context.set_latency_samples(latency);
            self.shared.latency.store(latency as f32);
        }
        ProcessStatus::Normal
    }
}

impl ClapPlugin for MiteMultiband {
    const CLAP_ID: &'static str = "io.github.ansabic.mitefx.multiband";
    const CLAP_DESCRIPTION: Option<&'static str> = Some("Multiband dynamics");
    const CLAP_MANUAL_URL: Option<&'static str> = Some(Self::URL);
    const CLAP_SUPPORT_URL: Option<&'static str> = Some("https://github.com/ansabic/MiteFX/issues");
    const CLAP_FEATURES: &'static [ClapFeature] = &[ClapFeature::AudioEffect, ClapFeature::Stereo, ClapFeature::Mono, ClapFeature::Compressor, ClapFeature::Expander];
}

impl Vst3Plugin for MiteMultiband {
    const VST3_CLASS_ID: [u8; 16] = *b"MiteFX-Multib-01";
    const VST3_SUBCATEGORIES: &'static [Vst3SubCategory] = &[Vst3SubCategory::Fx, Vst3SubCategory::Dynamics];
}

nih_export_clap!(MiteMultiband);
nih_export_vst3!(MiteMultiband);
