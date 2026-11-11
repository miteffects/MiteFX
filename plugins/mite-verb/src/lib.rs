//! Mite Verb: an algorithmic reverb (BLUEPRINT §6.5, docs/design/verb-brief.md) with a free decay
//! EQ (decay time per frequency), a free tone EQ, freeze and ducking.
//!
//! The plugin itself: everything else is in `mite_verb_core` (also what Mite Rig embeds).

use std::sync::Arc;
use nih_plug::prelude::*;
use dsp::VerbEngine;
use params::VerbParams;

pub use mite_verb_core::*;

pub struct MiteVerb {
    params: Arc<VerbParams>,
    engine: VerbEngine,
    shared: Arc<VerbShared>,
}

impl Default for MiteVerb {
    fn default() -> Self {
        let params = Arc::new(VerbParams::default());
        let engine = VerbEngine::new(48_000.0, &params.settings(), &params.output());
        let shared = Arc::new(VerbShared::with_stores(params.snapshots.clone(), params.preset.clone()));
        Self { params, engine, shared }
    }
}

impl Plugin for MiteVerb {
    const NAME: &'static str = "Mite Verb";
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
        mite_plugin::state::migrate(state, params::STATE_VERSION, &[migrate::v1_to_v2, migrate::v2_to_v3, migrate::v3_to_v4, migrate::v4_to_v5, migrate::v5_to_v6, migrate::v6_to_v7, migrate::v7_to_v8]);
    }

    fn params(&self) -> Arc<dyn Params> {
        self.params.clone()
    }

    fn editor(&mut self, _async_executor: AsyncExecutor<Self>) -> Option<Box<dyn Editor>> {
        let shared = self.shared.clone();
        mite_plugin::create_editor(self.params.clone(), "Mite Verb", editor::ACCENT, move || editor::VerbView::new(shared.clone()))
    }

    fn initialize(&mut self, _layout: &AudioIOLayout, config: &BufferConfig, context: &mut impl InitContext<Self>) -> bool {
        self.engine.prepare(config.sample_rate, &self.params.settings(), &self.params.output());
        self.shared.sample_rate.store(config.sample_rate);
        context.set_latency_samples(0);
        true
    }

    fn reset(&mut self) {
        self.engine.reset(&self.params.settings(), &self.params.output());
    }

    fn process(&mut self, buffer: &mut Buffer, _aux: &mut AuxiliaryBuffers, _context: &mut impl ProcessContext<Self>) -> ProcessStatus {
        let (settings, out) = (self.params.settings(), self.params.output());
        self.engine.process(buffer.as_slice(), &settings, &out, &self.shared.history, Some([&self.shared.spectrum, &self.shared.wet]));
        // A reverb rings on after its input stops.
        ProcessStatus::KeepAlive
    }
}

impl ClapPlugin for MiteVerb {
    const CLAP_ID: &'static str = "io.github.ansabic.mitefx.verb";
    const CLAP_DESCRIPTION: Option<&'static str> = Some("Algorithmic reverb");
    const CLAP_MANUAL_URL: Option<&'static str> = Some(Self::URL);
    const CLAP_SUPPORT_URL: Option<&'static str> = Some("https://github.com/ansabic/MiteFX/issues");
    const CLAP_FEATURES: &'static [ClapFeature] = &[ClapFeature::AudioEffect, ClapFeature::Stereo, ClapFeature::Mono, ClapFeature::Reverb];
}

impl Vst3Plugin for MiteVerb {
    const VST3_CLASS_ID: [u8; 16] = *b"MiteFX-Verb---01";
    const VST3_SUBCATEGORIES: &'static [Vst3SubCategory] = &[Vst3SubCategory::Fx, Vst3SubCategory::Reverb];
}

nih_export_clap!(MiteVerb);
nih_export_vst3!(MiteVerb);
