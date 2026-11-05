//! Mite Mod (docs/design/mod-brief.md): chorus, ensemble, flanger (through zero too), phaser and
//! vibrato and a rotary speaker in one plugin, on one smooth LFO (`mite_dsp::modulation`), each mode with its own view.

use std::sync::Arc;

use mite_core::presets::CurrentPreset;
use mite_core::snapshots::Snapshots;
use mite_core::{AtomicF32, HistoryTap};
use mite_dsp::modulation::Transport;
use nih_plug::prelude::*;

pub mod dsp;
pub mod editor;
pub mod migrate;
pub mod params;
pub mod presets;

use dsp::{record, ModEngine, HISTORY_RECORDS};
use params::ModParams;

/// Audio → UI telemetry. Lock-free and allocation-free on the audio side.
pub struct ModShared {
    pub history: HistoryTap<{ record::WIDTH }>,
    pub sample_rate: AtomicF32,
    /// The host's tempo (0: unknown), for a synced rate's reading.
    pub tempo: AtomicF32,
    pub snapshots: Arc<Snapshots>,
    pub preset: Arc<CurrentPreset>,
}

impl ModShared {
    pub fn new() -> Self {
        Self::with_stores(Arc::new(Snapshots::new()), Arc::new(CurrentPreset::default()))
    }

    pub fn with_stores(snapshots: Arc<Snapshots>, preset: Arc<CurrentPreset>) -> Self {
        Self { history: HistoryTap::new(HISTORY_RECORDS), sample_rate: AtomicF32::new(48_000.0), tempo: AtomicF32::new(0.0), snapshots, preset }
    }
}

impl Default for ModShared {
    fn default() -> Self {
        Self::new()
    }
}

pub struct MiteMod {
    params: Arc<ModParams>,
    engine: ModEngine,
    shared: Arc<ModShared>,
    stereo_in: bool,
    reported_latency: u32,
}

impl Default for MiteMod {
    fn default() -> Self {
        let params = Arc::new(ModParams::default());
        let engine = ModEngine::new(48_000.0, &params.settings(), &params.output());
        let shared = Arc::new(ModShared::with_stores(params.snapshots.clone(), params.preset.clone()));
        Self { params, engine, shared, stereo_in: true, reported_latency: 0 }
    }
}

impl Plugin for MiteMod {
    const NAME: &'static str = "Mite Mod";
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
        mite_plugin::state::migrate(state, params::STATE_VERSION, &[migrate::v1_to_v2]);
    }

    fn params(&self) -> Arc<dyn Params> {
        self.params.clone()
    }

    fn editor(&mut self, _async_executor: AsyncExecutor<Self>) -> Option<Box<dyn Editor>> {
        let shared = self.shared.clone();
        mite_plugin::create_editor(self.params.clone(), "Mite Mod", editor::ACCENT, move || editor::ModView::new(shared.clone()))
    }

    fn initialize(&mut self, layout: &AudioIOLayout, config: &BufferConfig, context: &mut impl InitContext<Self>) -> bool {
        self.stereo_in = layout.main_input_channels.map_or(0, NonZeroU32::get) >= 2;
        self.engine.prepare(config.sample_rate, &self.params.settings(), &self.params.output());
        self.shared.sample_rate.store(config.sample_rate);
        self.reported_latency = self.engine.latency() as u32;
        context.set_latency_samples(self.reported_latency);
        true
    }

    fn reset(&mut self) {
        self.engine.reset(&self.params.settings(), &self.params.output());
    }

    fn process(&mut self, buffer: &mut Buffer, _aux: &mut AuxiliaryBuffers, context: &mut impl ProcessContext<Self>) -> ProcessStatus {
        let t = context.transport();
        let tempo = t.tempo.filter(|v| v.is_finite() && *v > 0.0);
        let transport = Transport { tempo, beats: t.pos_beats(), playing: t.playing };
        self.shared.tempo.store(tempo.unwrap_or(0.0) as f32);
        let (s, out) = (self.params.settings(), self.params.output());
        self.engine.process(buffer.as_slice(), self.stereo_in, &s, &out, &transport, &self.shared.history);
        // Through zero, vibrato or the rotary in or out: tell the host.
        let latency = self.engine.latency() as u32;
        if latency != self.reported_latency {
            self.reported_latency = latency;
            context.set_latency_samples(latency);
        }
        ProcessStatus::Normal
    }
}

impl ClapPlugin for MiteMod {
    const CLAP_ID: &'static str = "io.github.ansabic.mitefx.mod";
    const CLAP_DESCRIPTION: Option<&'static str> = Some("Chorus, ensemble, flanger, phaser, vibrato and rotary speaker");
    const CLAP_MANUAL_URL: Option<&'static str> = Some(Self::URL);
    const CLAP_SUPPORT_URL: Option<&'static str> = Some("https://github.com/ansabic/MiteFX/issues");
    const CLAP_FEATURES: &'static [ClapFeature] = &[ClapFeature::AudioEffect, ClapFeature::Stereo, ClapFeature::Mono, ClapFeature::Chorus, ClapFeature::Flanger, ClapFeature::Phaser];
}

impl Vst3Plugin for MiteMod {
    const VST3_CLASS_ID: [u8; 16] = *b"MiteFX-Mod----01";
    const VST3_SUBCATEGORIES: &'static [Vst3SubCategory] = &[Vst3SubCategory::Fx, Vst3SubCategory::Modulation];
}

nih_export_clap!(MiteMod);
nih_export_vst3!(MiteMod);
