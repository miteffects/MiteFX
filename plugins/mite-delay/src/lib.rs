//! Mite Delay (BLUEPRINT §6.6, docs/design/delay-brief.md): up to four echo taps, each synced to
//! the host's tempo or free, normal or ping-pong; Digital, Tape and BBD machines inside the loop
//! with a free loop EQ, drive, modulation and diffusion; width and ducking.
//!
//! The plugin itself: everything else is in `mite_delay_core` (also what Mite Rig embeds).

use std::sync::Arc;
use nih_plug::prelude::*;
use dsp::DelayEngine;
use params::DelayParams;

pub use mite_delay_core::*;

pub struct MiteDelay {
    params: Arc<DelayParams>,
    engine: DelayEngine,
    matrix: mite_plugin::matrix::MatrixRunner,
    shared: Arc<DelayShared>,
}

impl Default for MiteDelay {
    fn default() -> Self {
        let params = Arc::new(DelayParams::default());
        let engine = DelayEngine::new(48_000.0, &params.settings(DEFAULT_TEMPO), &params.output());
        let shared = Arc::new(DelayShared::with_stores(params.snapshots.clone(), params.preset.clone()));
        Self { params, engine, matrix: Default::default(), shared }
    }
}

/// 1 → 2 (2026-11-05): the modulation matrix, new and empty by default: nothing to change.
fn migrate_v1_to_v2(_state: &mut PluginState) {}

impl MiteDelay {
    fn tempo(&self) -> f32 {
        self.shared.tempo.load()
    }
}

impl Plugin for MiteDelay {
    const NAME: &'static str = "Mite Delay";
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
        mite_plugin::state::migrate(state, params::STATE_VERSION, &[migrate_v1_to_v2]);
    }

    fn params(&self) -> Arc<dyn Params> {
        self.params.clone()
    }

    fn editor(&mut self, _async_executor: AsyncExecutor<Self>) -> Option<Box<dyn Editor>> {
        let shared = self.shared.clone();
        mite_plugin::create_editor(self.params.clone(), "Mite Delay", editor::ACCENT, move || editor::DelayView::new(shared.clone()))
    }

    fn initialize(&mut self, _layout: &AudioIOLayout, config: &BufferConfig, context: &mut impl InitContext<Self>) -> bool {
        self.engine.prepare(config.sample_rate, &self.params.settings(self.tempo()), &self.params.output());
        self.shared.sample_rate.store(config.sample_rate);
        self.matrix.prepare(config.sample_rate, config.max_buffer_size as usize);
        context.set_latency_samples(0);
        true
    }

    fn reset(&mut self) {
        self.engine.reset(&self.params.settings(self.tempo()), &self.params.output());
        self.matrix.reset();
    }

    fn process(&mut self, buffer: &mut Buffer, _aux: &mut AuxiliaryBuffers, context: &mut impl ProcessContext<Self>) -> ProcessStatus {
        let t = context.transport();
        let host_tempo = t.tempo.filter(|v| v.is_finite() && *v > 0.0);
        if let Some(bpm) = host_tempo {
            self.shared.tempo.store(bpm as f32);
        }
        let transport = mite_dsp::modulation::Transport { tempo: host_tempo, beats: t.pos_beats(), playing: t.playing };
        let tempo = self.tempo();
        let ms = self.params.matrix.settings();
        let (params, engine, shared) = (&self.params, &mut self.engine, &self.shared);
        self.matrix.run(&ms, &transport, buffer.as_slice(), None, &self.shared.matrix, params::MOD_TARGETS.len(), |sub, _, offsets| {
            let (settings, out) = if offsets.iter().any(|o| *o != 0.0) { params.modulated(tempo, offsets) } else { (params.settings(tempo), params.output()) };
            engine.process(sub, &settings, &out, &shared.history, Some([&shared.spectrum, &shared.wet]));
        });
        // Echoes ring on after the input stops.
        ProcessStatus::KeepAlive
    }
}

impl ClapPlugin for MiteDelay {
    const CLAP_ID: &'static str = "io.github.ansabic.mitefx.delay";
    const CLAP_DESCRIPTION: Option<&'static str> = Some("Tempo-synced delay with tape and BBD machines");
    const CLAP_MANUAL_URL: Option<&'static str> = Some(Self::URL);
    const CLAP_SUPPORT_URL: Option<&'static str> = Some("https://github.com/ansabic/MiteFX/issues");
    const CLAP_FEATURES: &'static [ClapFeature] = &[ClapFeature::AudioEffect, ClapFeature::Stereo, ClapFeature::Mono, ClapFeature::Delay];
}

impl Vst3Plugin for MiteDelay {
    const VST3_CLASS_ID: [u8; 16] = *b"MiteFX-Delay--01";
    const VST3_SUBCATEGORIES: &'static [Vst3SubCategory] = &[Vst3SubCategory::Fx, Vst3SubCategory::Delay];
}

nih_export_clap!(MiteDelay);
nih_export_vst3!(MiteDelay);
