//! Mite EQ — the flagship (BLUEPRINT §8). First slice: zero-latency matched bands (Bell,
//! shelves, cuts 6–96 dB/oct, notch, band pass, tilt), 24 bands, analyzer and the EQ editor.

use std::num::NonZeroU32;
use std::sync::Arc;

use mite_core::{AtomicF32, SpectrumTap, StereoMeter};
use mite_dsp::eq::BandSettings;
use mite_dsp::MINUS_INFINITY_DB;
use nih_plug::prelude::*;

pub mod dsp;
pub mod editor;
pub mod params;

use dsp::EqEngine;
use params::{EqParams, BANDS};

/// Audio → UI telemetry. Everything is lock-free and allocation-free on the audio side.
pub struct EqShared {
    pub meter: StereoMeter,
    pub pre: SpectrumTap,
    pub post: SpectrumTap,
    pub sample_rate: AtomicF32,
}

impl EqShared {
    pub fn new() -> Self {
        Self {
            meter: StereoMeter::new(MINUS_INFINITY_DB),
            pre: SpectrumTap::new(editor::ANALYZER_SIZE),
            post: SpectrumTap::new(editor::ANALYZER_SIZE),
            sample_rate: AtomicF32::new(48_000.0),
        }
    }
}

impl Default for EqShared {
    fn default() -> Self {
        Self::new()
    }
}

pub struct MiteEq {
    params: Arc<EqParams>,
    engine: EqEngine,
    shared: Arc<EqShared>,
}

impl MiteEq {
    fn settings(&self) -> [BandSettings; BANDS] {
        std::array::from_fn(|i| self.params.bands[i].settings())
    }
}

impl Default for MiteEq {
    fn default() -> Self {
        let params = Arc::new(EqParams::default());
        let settings = std::array::from_fn(|i| params.bands[i].settings());
        let engine = EqEngine::new(48_000.0, &settings, params.out_gain.value());
        Self { params, engine, shared: Arc::new(EqShared::new()) }
    }
}

impl Plugin for MiteEq {
    const NAME: &'static str = "Mite EQ";
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

    fn params(&self) -> Arc<dyn Params> {
        self.params.clone()
    }

    fn editor(&mut self, _async_executor: AsyncExecutor<Self>) -> Option<Box<dyn Editor>> {
        let shared = self.shared.clone();
        mite_plugin::create_editor(self.params.clone(), "Mite EQ", editor::ACCENT, move || {
            editor::EqView::new(shared.clone())
        })
    }

    fn initialize(&mut self, _layout: &AudioIOLayout, config: &BufferConfig, _context: &mut impl InitContext<Self>) -> bool {
        let settings = self.settings();
        self.engine.prepare(config.sample_rate, &settings, self.params.out_gain.value());
        self.shared.sample_rate.store(config.sample_rate);
        true
    }

    fn reset(&mut self) {
        let settings = self.settings();
        self.engine.reset(&settings, self.params.out_gain.value());
    }

    fn process(&mut self, buffer: &mut Buffer, _aux: &mut AuxiliaryBuffers, _context: &mut impl ProcessContext<Self>) -> ProcessStatus {
        let settings = self.settings();
        let (left, right) = self.engine.process(
            buffer.as_slice(),
            &settings,
            self.params.out_gain.value(),
            Some(&self.shared.pre),
            Some(&self.shared.post),
        );
        self.shared.meter.store(left, right);
        ProcessStatus::Normal
    }
}

impl ClapPlugin for MiteEq {
    const CLAP_ID: &'static str = "io.github.ansabic.mitefx.eq";
    const CLAP_DESCRIPTION: Option<&'static str> = Some("Parametric equaliser");
    const CLAP_MANUAL_URL: Option<&'static str> = Some(Self::URL);
    const CLAP_SUPPORT_URL: Option<&'static str> = Some("https://github.com/ansabic/MiteFX/issues");
    const CLAP_FEATURES: &'static [ClapFeature] =
        &[ClapFeature::AudioEffect, ClapFeature::Stereo, ClapFeature::Mono, ClapFeature::Equalizer];
}

impl Vst3Plugin for MiteEq {
    const VST3_CLASS_ID: [u8; 16] = *b"MiteFX-EQ---0001";
    const VST3_SUBCATEGORIES: &'static [Vst3SubCategory] = &[Vst3SubCategory::Fx, Vst3SubCategory::Eq];
}

nih_export_clap!(MiteEq);
nih_export_vst3!(MiteEq);
