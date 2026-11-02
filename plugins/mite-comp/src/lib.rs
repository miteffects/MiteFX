//! Mite Comp: the compressor (BLUEPRINT §6.2). Up to four stages in series on one transfer
//! curve, seven Characters, Smart modes, a sidechain with its own detector EQ.

use std::num::NonZeroU32;
use std::sync::Arc;

use mite_core::presets::CurrentPreset;
use mite_core::snapshots::Snapshots;
use mite_core::{AtomicF32, HistoryTap};
use nih_plug::prelude::*;

pub mod dsp;
pub mod editor;
pub mod migrate;
pub mod params;
pub mod presets;

use dsp::{record, CompEngine, Output, HISTORY_RECORDS};
use params::CompParams;

/// Audio → UI telemetry, and the editor's Character audition (UI → audio). Lock-free and
/// allocation-free on the audio side.
pub struct CompShared {
    /// One record per 5 ms slice (`dsp::record`).
    pub history: HistoryTap<{ record::WIDTH }>,
    pub sample_rate: AtomicF32,
    /// The latency the engine runs with, samples.
    pub latency: AtomicF32,
    /// A Character being tried from the menu (its index), or −1: the engine plays it without the
    /// parameter changing, so hovering the menu never reaches host automation or undo.
    pub audition: AtomicF32,
    /// Snapshots A–D and the current preset (the parameters' stores, shared with the editor).
    pub snapshots: Arc<Snapshots>,
    pub preset: Arc<CurrentPreset>,
    /// The history's level range picked by hand (0: Auto; the parameters' store).
    pub display_range: Arc<AtomicF32>,
}

impl CompShared {
    pub fn new() -> Self {
        Self::with_stores(Arc::new(Snapshots::new()), Arc::new(CurrentPreset::default()))
    }

    pub fn with_stores(snapshots: Arc<Snapshots>, preset: Arc<CurrentPreset>) -> Self {
        Self {
            history: HistoryTap::new(HISTORY_RECORDS),
            sample_rate: AtomicF32::new(48_000.0),
            latency: AtomicF32::new(0.0),
            audition: AtomicF32::new(-1.0),
            snapshots,
            preset,
            display_range: Arc::new(AtomicF32::new(0.0)),
        }
    }

    /// The Character being auditioned, if any.
    pub fn audition(&self) -> Option<usize> {
        let a = self.audition.load();
        (a >= 0.0).then_some(a as usize)
    }
}

impl Default for CompShared {
    fn default() -> Self {
        Self::new()
    }
}

pub struct MiteComp {
    params: Arc<CompParams>,
    engine: CompEngine,
    shared: Arc<CompShared>,
    /// The latency last reported to the host, samples.
    reported_latency: u32,
}

impl MiteComp {
    fn output(&self) -> Output {
        let p = &self.params;
        Output { mix: p.mix.value() / 100.0, delta: p.delta.value(), bypass: p.bypass.value(), gain_db: p.out_gain.value(), pan: p.out_pan.value() / 100.0 }
    }
}

impl Default for MiteComp {
    fn default() -> Self {
        let params = Arc::new(CompParams::default());
        let engine = CompEngine::new(48_000.0, &params.settings(None));
        let mut shared = CompShared::with_stores(params.snapshots.clone(), params.preset.clone());
        shared.display_range = params.display_range.clone();
        let shared = Arc::new(shared);
        Self { params, engine, shared, reported_latency: 0 }
    }
}

impl Plugin for MiteComp {
    const NAME: &'static str = "Mite Comp";
    const VENDOR: &'static str = "MiteFX";
    const URL: &'static str = "https://github.com/ansabic/MiteFX";
    const EMAIL: &'static str = "";
    const VERSION: &'static str = env!("CARGO_PKG_VERSION");

    const AUDIO_IO_LAYOUTS: &'static [AudioIOLayout] = &[
        // The sidechain input feeds the detector when the source is External (silent when
        // unconnected).
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
        mite_plugin::state::migrate(state, params::STATE_VERSION, &[migrate::v1_to_v2, migrate::v2_to_v3]);
    }

    fn params(&self) -> Arc<dyn Params> {
        self.params.clone()
    }

    fn editor(&mut self, _async_executor: AsyncExecutor<Self>) -> Option<Box<dyn Editor>> {
        let shared = self.shared.clone();
        mite_plugin::create_editor(self.params.clone(), "Mite Comp", editor::ACCENT, move || editor::CompView::new(shared.clone()))
    }

    fn initialize(&mut self, _layout: &AudioIOLayout, config: &BufferConfig, context: &mut impl InitContext<Self>) -> bool {
        let settings = self.params.settings(None);
        self.engine.prepare(config.sample_rate, &settings, &self.output());
        self.shared.sample_rate.store(config.sample_rate);
        self.reported_latency = self.engine.latency() as u32;
        context.set_latency_samples(self.reported_latency);
        self.shared.latency.store(self.reported_latency as f32);
        true
    }

    fn reset(&mut self) {
        let settings = self.params.settings(None);
        let output = self.output();
        self.engine.reset(&settings, &output);
    }

    fn process(&mut self, buffer: &mut Buffer, aux: &mut AuxiliaryBuffers, context: &mut impl ProcessContext<Self>) -> ProcessStatus {
        let settings = self.params.settings(self.shared.audition());
        let output = self.output();
        let mut key: [&[f32]; 2] = [&[], &[]];
        let mut key_channels = 0;
        if let Some(sc) = aux.inputs.first_mut() {
            for (slot, ch) in key.iter_mut().zip(sc.as_slice_immutable()) {
                *slot = ch;
                key_channels += 1;
            }
        }
        let key = (key_channels > 0).then_some(&key[..key_channels]);
        self.engine.process(buffer.as_slice(), key, &settings, &output, &self.shared.history);
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

impl ClapPlugin for MiteComp {
    const CLAP_ID: &'static str = "io.github.ansabic.mitefx.comp";
    const CLAP_DESCRIPTION: Option<&'static str> = Some("Compressor");
    const CLAP_MANUAL_URL: Option<&'static str> = Some(Self::URL);
    const CLAP_SUPPORT_URL: Option<&'static str> = Some("https://github.com/ansabic/MiteFX/issues");
    const CLAP_FEATURES: &'static [ClapFeature] = &[ClapFeature::AudioEffect, ClapFeature::Stereo, ClapFeature::Mono, ClapFeature::Compressor];
}

impl Vst3Plugin for MiteComp {
    const VST3_CLASS_ID: [u8; 16] = *b"MiteFX-Comp-0001";
    const VST3_SUBCATEGORIES: &'static [Vst3SubCategory] = &[Vst3SubCategory::Fx, Vst3SubCategory::Dynamics];
}

nih_export_clap!(MiteComp);
nih_export_vst3!(MiteComp);
