//! Mite Analyzer (docs/design/analyzer-brief.md): the master's meter. Spectrum, spectrogram,
//! loudness (EBU R128 / BS.1770 with a history and a target), true peak, and the stereo field
//! (vectorscope, correlation, balance, width per band). The audio passes through untouched.

use std::sync::Arc;

use mite_core::presets::CurrentPreset;
use mite_core::snapshots::Snapshots;
use mite_core::{AtomicF32, Registry};
use std::sync::atomic::AtomicU32;
use nih_plug::prelude::*;

pub mod dsp;
pub mod editor;
pub mod params;
pub mod presets;

use dsp::{AnalyzerEngine, Telemetry};
use params::AnalyzerParams;

/// Every Mite Analyzer in the process: one placed before a chain is the reference another one
/// after it measures the chain against (the phase page).
pub static INSTANCES: Registry<AnalyzerShared> = Registry::new();

/// The phase page's reference: automatic, the sidechain, or Analyzer `n` (`ANALYZER_SOURCE + n`).
pub const SOURCE_AUTO: u32 = 0;
pub const SOURCE_SIDECHAIN: u32 = 1;
pub const ANALYZER_SOURCE: u32 = 1;

/// Audio ↔ UI. Lock-free and allocation-free on the audio side.
pub struct AnalyzerShared {
    pub tel: Telemetry,
    pub sample_rate: AtomicF32,
    /// The phase page's reference ([`SOURCE_AUTO`], …), kept while the instance lives.
    pub phase_source: AtomicU32,
    pub snapshots: Arc<Snapshots>,
    pub preset: Arc<CurrentPreset>,
}

impl AnalyzerShared {
    pub fn new() -> Self {
        Self::with_stores(Arc::new(Snapshots::new()), Arc::new(CurrentPreset::default()))
    }

    pub fn with_stores(snapshots: Arc<Snapshots>, preset: Arc<CurrentPreset>) -> Self {
        Self { tel: Telemetry::new(), sample_rate: AtomicF32::new(48_000.0), phase_source: AtomicU32::new(SOURCE_AUTO), snapshots, preset }
    }
}

impl Default for AnalyzerShared {
    fn default() -> Self {
        Self::new()
    }
}

pub struct MiteAnalyzer {
    params: Arc<AnalyzerParams>,
    engine: AnalyzerEngine,
    shared: Arc<AnalyzerShared>,
}

impl Default for MiteAnalyzer {
    fn default() -> Self {
        let params = Arc::new(AnalyzerParams::default());
        let shared = Arc::new(AnalyzerShared::with_stores(params.snapshots.clone(), params.preset.clone()));
        INSTANCES.register(&shared);
        Self { params, engine: AnalyzerEngine::new(48_000.0), shared }
    }
}

impl Plugin for MiteAnalyzer {
    const NAME: &'static str = "Mite Analyzer";
    const VENDOR: &'static str = "MiteFX";
    const URL: &'static str = "https://github.com/ansabic/MiteFX";
    const EMAIL: &'static str = "";
    const VERSION: &'static str = env!("CARGO_PKG_VERSION");

    const AUDIO_IO_LAYOUTS: &'static [AudioIOLayout] = &[
        AudioIOLayout {
            main_input_channels: NonZeroU32::new(2),
            main_output_channels: NonZeroU32::new(2),
            aux_input_ports: &[new_nonzero_u32(2)],
            names: PortNames { aux_inputs: &["Reference"], ..PortNames::const_default() },
            ..AudioIOLayout::const_default()
        },
        AudioIOLayout {
            main_input_channels: NonZeroU32::new(1),
            main_output_channels: NonZeroU32::new(1),
            aux_input_ports: &[new_nonzero_u32(1)],
            names: PortNames { aux_inputs: &["Reference"], ..PortNames::const_default() },
            ..AudioIOLayout::const_default()
        },
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
        mite_plugin::create_editor(self.params.clone(), "Mite Analyzer", editor::ACCENT, move || editor::AnalyzerView::new(shared.clone()))
    }

    fn initialize(&mut self, _layout: &AudioIOLayout, config: &BufferConfig, _context: &mut impl InitContext<Self>) -> bool {
        self.engine.prepare(config.sample_rate);
        self.shared.sample_rate.store(config.sample_rate);
        true
    }

    fn reset(&mut self) {
        self.engine.reset();
    }

    fn process(&mut self, buffer: &mut Buffer, aux: &mut AuxiliaryBuffers, _context: &mut impl ProcessContext<Self>) -> ProcessStatus {
        let mut sc: [&[f32]; 2] = [&[], &[]];
        let mut n = 0;
        if let Some(input) = aux.inputs.first_mut() {
            for (slot, ch) in sc.iter_mut().zip(input.as_slice_immutable()) {
                *slot = ch;
                n += 1;
            }
        }
        self.engine.process_with(buffer.as_slice(), (n > 0).then_some(&sc[..n]), &self.shared.tel);
        ProcessStatus::Normal
    }
}

impl ClapPlugin for MiteAnalyzer {
    const CLAP_ID: &'static str = "io.github.ansabic.mitefx.analyzer";
    const CLAP_DESCRIPTION: Option<&'static str> = Some("Spectrum, loudness, true peak, stereo and a chain's phase response");
    const CLAP_MANUAL_URL: Option<&'static str> = Some(Self::URL);
    const CLAP_SUPPORT_URL: Option<&'static str> = Some("https://github.com/ansabic/MiteFX/issues");
    const CLAP_FEATURES: &'static [ClapFeature] = &[ClapFeature::AudioEffect, ClapFeature::Stereo, ClapFeature::Mono, ClapFeature::Analyzer];
}

impl Vst3Plugin for MiteAnalyzer {
    const VST3_CLASS_ID: [u8; 16] = *b"MiteFX-Analyzr01";
    const VST3_SUBCATEGORIES: &'static [Vst3SubCategory] = &[Vst3SubCategory::Fx, Vst3SubCategory::Analyzer];
}

nih_export_clap!(MiteAnalyzer);
nih_export_vst3!(MiteAnalyzer);
