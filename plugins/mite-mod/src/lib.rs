//! Mite Mod (docs/design/mod-brief.md): chorus, ensemble, flanger (through zero too), phaser and
//! vibrato, a rotary speaker and a micro pitch shifter in one plugin, on one smooth LFO (`mite_dsp::modulation`), each mode with its own view.
//!
//! The plugin itself: everything else is in `mite_mod_core` (also what Mite Rig embeds).

use std::sync::Arc;
use mite_dsp::modulation::Transport;
use nih_plug::prelude::*;
use dsp::ModEngine;
use params::ModParams;

pub use mite_mod_core::*;

pub struct MiteMod {
    params: Arc<ModParams>,
    engine: ModEngine,
    matrix: mite_plugin::matrix::MatrixRunner,
    shared: Arc<ModShared>,
    stereo_in: bool,
    reported_latency: u32,
}

impl Default for MiteMod {
    fn default() -> Self {
        let params = Arc::new(ModParams::default());
        let engine = ModEngine::new(48_000.0, &params.settings(), &params.output());
        let shared = Arc::new(ModShared::with_stores(params.snapshots.clone(), params.preset.clone()));
        Self { params, engine, matrix: Default::default(), shared, stereo_in: true, reported_latency: 0 }
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
        mite_plugin::state::migrate(state, params::STATE_VERSION, &[migrate::v1_to_v2, migrate::v2_to_v3, migrate::v3_to_v4]);
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
        self.matrix.prepare(config.sample_rate, config.max_buffer_size as usize);
        self.shared.sample_rate.store(config.sample_rate);
        self.reported_latency = self.engine.latency() as u32;
        context.set_latency_samples(self.reported_latency);
        true
    }

    fn reset(&mut self) {
        self.engine.reset(&self.params.settings(), &self.params.output());
        self.matrix.reset();
    }

    fn process(&mut self, buffer: &mut Buffer, _aux: &mut AuxiliaryBuffers, context: &mut impl ProcessContext<Self>) -> ProcessStatus {
        let t = context.transport();
        let tempo = t.tempo.filter(|v| v.is_finite() && *v > 0.0);
        let transport = Transport { tempo, beats: t.pos_beats(), playing: t.playing };
        self.shared.tempo.store(tempo.unwrap_or(0.0) as f32);
        let ms = self.params.matrix.settings();
        let fs = f64::from(self.shared.sample_rate.load());
        let (params, engine, shared, stereo_in) = (&self.params, &mut self.engine, &self.shared, self.stereo_in);
        self.matrix.run(&ms, &transport, buffer.as_slice(), None, &self.shared.matrix, params::MOD_TARGETS.len(), |sub, r, offsets| {
            let (s, out) = if offsets.iter().any(|o| *o != 0.0) { params.modulated(offsets) } else { (params.settings(), params.output()) };
            engine.process(sub, stereo_in, &s, &out, &dsp::transport_at(&transport, r.start, fs), &shared.history);
        });
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
    const CLAP_DESCRIPTION: Option<&'static str> = Some("Chorus, ensemble, flanger, phaser, vibrato, rotary speaker and micro pitch shift");
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
