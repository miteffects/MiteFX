//! Mite Gain — the Phase 0 plugin. One parameter, an output meter and a Vello UI: small enough
//! to prove the whole pipeline (DSP → params → state → UI → CLAP/VST3 bundles → validators) on
//! every platform before the EQ is built on it.

use std::num::NonZeroU32;
use std::sync::Arc;

use mite_core::StereoMeter;
use mite_dsp::MINUS_INFINITY_DB;
use nih_plug::prelude::*;

pub mod dsp;
pub mod editor;
pub mod params;

use dsp::GainDsp;
use params::GainParams;

pub struct MiteGain {
    params: Arc<GainParams>,
    dsp: GainDsp,
    meter: Arc<StereoMeter>,
}

impl Default for MiteGain {
    fn default() -> Self {
        let params = Arc::new(GainParams::default());
        let dsp = GainDsp::new(48_000.0, params.gain.value());
        Self { params, dsp, meter: Arc::new(StereoMeter::new(MINUS_INFINITY_DB)) }
    }
}

impl Plugin for MiteGain {
    const NAME: &'static str = "Mite Gain";
    const VENDOR: &'static str = "MiteFX";
    const URL: &'static str = "https://github.com/ansabic/MiteFX";
    const EMAIL: &'static str = "";
    const VERSION: &'static str = env!("CARGO_PKG_VERSION");

    const AUDIO_IO_LAYOUTS: &'static [AudioIOLayout] = &[
        AudioIOLayout {
            main_input_channels: NonZeroU32::new(2),
            main_output_channels: NonZeroU32::new(2),
            ..AudioIOLayout::const_default()
        },
        AudioIOLayout {
            main_input_channels: NonZeroU32::new(1),
            main_output_channels: NonZeroU32::new(1),
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
        let meter = self.meter.clone();
        mite_plugin::create_editor(self.params.clone(), "Mite Gain", editor::ACCENT, move || editor::GainView::new(meter.clone()))
    }

    fn initialize(&mut self, _layout: &AudioIOLayout, config: &BufferConfig, _context: &mut impl InitContext<Self>) -> bool {
        self.dsp.prepare(config.sample_rate, self.params.gain.value());
        true
    }

    fn reset(&mut self) {
        self.dsp.reset(self.params.gain.value());
    }

    fn process(&mut self, buffer: &mut Buffer, _aux: &mut AuxiliaryBuffers, _context: &mut impl ProcessContext<Self>) -> ProcessStatus {
        // nih-plug splits the buffer at automation events (SAMPLE_ACCURATE_AUTOMATION), so the
        // parameter value is exact for this sub-block; the DSP smooths between values.
        let (left, right) = self.dsp.process(buffer.as_slice(), self.params.gain.value());
        self.meter.store(left, right);
        ProcessStatus::Normal
    }
}

impl ClapPlugin for MiteGain {
    const CLAP_ID: &'static str = "io.github.ansabic.mitefx.gain";
    const CLAP_DESCRIPTION: Option<&'static str> = Some("Clean gain with an output meter");
    const CLAP_MANUAL_URL: Option<&'static str> = Some(Self::URL);
    const CLAP_SUPPORT_URL: Option<&'static str> = Some("https://github.com/ansabic/MiteFX/issues");
    const CLAP_FEATURES: &'static [ClapFeature] =
        &[ClapFeature::AudioEffect, ClapFeature::Stereo, ClapFeature::Mono, ClapFeature::Utility];
}

impl Vst3Plugin for MiteGain {
    const VST3_CLASS_ID: [u8; 16] = *b"MiteFX-Gain-0001";
    const VST3_SUBCATEGORIES: &'static [Vst3SubCategory] = &[Vst3SubCategory::Fx, Vst3SubCategory::Tools];
}

nih_export_clap!(MiteGain);
nih_export_vst3!(MiteGain);
