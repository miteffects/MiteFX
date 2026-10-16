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
pub mod presets;
pub mod snapshots;

use dsp::{EqEngine, Output, PhaseMode};
use params::{EqParams, BANDS};

/// Points of the spectral stage's telemetry: the analyzer's points (log-spaced over the plot's
/// range), which the spectral detector also works on.
pub const SPECTRAL_POINTS: usize = mite_dsp::analyzer::SPECTRUM_POINTS;

/// The frequency of spectral telemetry point `i`.
pub use mite_dsp::analyzer::spectrum_point_freq as spectral_point_freq;

/// Audio → UI telemetry. Everything is lock-free and allocation-free on the audio side.
pub struct EqShared {
    pub meter: StereoMeter,
    pub pre: SpectrumTap,
    pub post: SpectrumTap,
    pub sample_rate: AtomicF32,
    /// Each band's current dynamic gain, dB.
    pub dynamic: [AtomicF32; BANDS],
    /// Each band's detected level, dBFS (dynamic bands only).
    pub level: [AtomicF32; BANDS],
    /// The latency the engine runs with, samples.
    pub latency: AtomicF32,
    /// The spectral stage's total gain at each [`spectral_point_freq`], dB (all 0 while it is off).
    pub spectral: [AtomicF32; SPECTRAL_POINTS],
    /// The band whose per-bin detail the editor shows (the selected one), set by the editor, and
    /// that band's detected level and threshold (dBFS) and applied gain (dB) at each
    /// [`spectral_point_freq`]. Level and threshold are −150 where the band does not listen.
    pub focus: AtomicF32,
    pub focus_level: [AtomicF32; SPECTRAL_POINTS],
    pub focus_threshold: [AtomicF32; SPECTRAL_POINTS],
    pub focus_gain: [AtomicF32; SPECTRAL_POINTS],
    /// Snapshots A–D and the current preset (the parameters' stores, shared with the editor).
    pub snapshots: Arc<snapshots::Snapshots>,
    pub preset: Arc<presets::CurrentPreset>,
}

impl EqShared {
    pub fn new() -> Self {
        Self::with_stores(Arc::new(snapshots::Snapshots::new()), Arc::new(presets::CurrentPreset::default()))
    }

    /// Telemetry around the parameters' snapshot and preset stores.
    pub fn with_stores(snapshots: Arc<snapshots::Snapshots>, preset: Arc<presets::CurrentPreset>) -> Self {
        Self {
            snapshots,
            preset,
            meter: StereoMeter::new(MINUS_INFINITY_DB),
            pre: SpectrumTap::new(editor::ANALYZER_SIZE),
            post: SpectrumTap::new(editor::ANALYZER_SIZE),
            sample_rate: AtomicF32::new(48_000.0),
            dynamic: std::array::from_fn(|_| AtomicF32::new(0.0)),
            level: std::array::from_fn(|_| AtomicF32::new(-120.0)),
            latency: AtomicF32::new(0.0),
            spectral: std::array::from_fn(|_| AtomicF32::new(0.0)),
            focus: AtomicF32::new(-1.0),
            focus_level: std::array::from_fn(|_| AtomicF32::new(-150.0)),
            focus_threshold: std::array::from_fn(|_| AtomicF32::new(-150.0)),
            focus_gain: std::array::from_fn(|_| AtomicF32::new(0.0)),
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
    /// The latency last reported to the host, samples.
    reported_latency: u32,
}

impl MiteEq {
    fn settings(&self) -> [BandSettings; BANDS] {
        std::array::from_fn(|i| self.params.bands[i].settings())
    }

    /// The output stage as the parameters set it; the soloed band is the first with solo on.
    fn output(&self) -> Output {
        let p = &self.params;
        Output {
            mix: p.mix.value() / 100.0,
            delta: p.delta.value(),
            bypass: p.bypass.value(),
            auto_gain: p.auto_gain.value(),
            pan: p.out_pan.value() / 100.0,
            solo: p.bands.iter().position(|b| b.solo.value() && b.on.value()),
        }
    }

    fn set_mode(&mut self) {
        let mode = PhaseMode::from_index(self.params.mode.value().max(0) as usize);
        self.engine.set_mode(mode, self.params.lp_res.value().max(0) as usize);
    }
}

impl Default for MiteEq {
    fn default() -> Self {
        let params = Arc::new(EqParams::default());
        let settings = std::array::from_fn(|i| params.bands[i].settings());
        let engine = EqEngine::new(48_000.0, &settings, params.out_gain.value());
        let shared = Arc::new(EqShared::with_stores(params.snapshots.clone(), params.preset.clone()));
        Self { params, engine, shared, reported_latency: 0 }
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

    fn initialize(&mut self, _layout: &AudioIOLayout, config: &BufferConfig, context: &mut impl InitContext<Self>) -> bool {
        let settings = self.settings();
        self.set_mode();
        self.engine.set_output(self.output());
        self.engine.prepare(config.sample_rate, &settings, self.params.out_gain.value());
        self.shared.sample_rate.store(config.sample_rate);
        self.reported_latency = self.engine.latency() as u32;
        context.set_latency_samples(self.reported_latency);
        self.shared.latency.store(self.reported_latency as f32);
        true
    }

    fn reset(&mut self) {
        let settings = self.settings();
        self.engine.set_output(self.output());
        self.engine.reset(&settings, self.params.out_gain.value());
    }

    fn process(&mut self, buffer: &mut Buffer, _aux: &mut AuxiliaryBuffers, context: &mut impl ProcessContext<Self>) -> ProcessStatus {
        let settings = self.settings();
        self.set_mode();
        self.engine.set_output(self.output());
        let (left, right) = self.engine.process(
            buffer.as_slice(),
            &settings,
            self.params.out_gain.value(),
            Some(&self.shared.pre),
            Some(&self.shared.post),
        );
        self.shared.meter.store(left, right);
        for (slot, db) in self.shared.dynamic.iter().zip(self.engine.dynamic_gains()) {
            slot.store(db);
        }
        for (slot, db) in self.shared.level.iter().zip(self.engine.detector_levels()) {
            slot.store(db);
        }
        for (i, slot) in self.shared.spectral.iter().enumerate() {
            slot.store(self.engine.spectral_gain_db(spectral_point_freq(i)));
        }
        let focus = self.shared.focus.load();
        if focus >= 0.0 && (focus as usize) < BANDS {
            for i in 0..SPECTRAL_POINTS {
                let (level, threshold, gain) = self.engine.spectral_band_db(focus as usize, i);
                self.shared.focus_level[i].store(level);
                self.shared.focus_threshold[i].store(threshold);
                self.shared.focus_gain[i].store(gain);
            }
        }
        // A mode change switched latency behind its dip: tell the host (signalled, not polled).
        let latency = self.engine.latency() as u32;
        if latency != self.reported_latency {
            self.reported_latency = latency;
            context.set_latency_samples(latency);
            self.shared.latency.store(latency as f32);
        }
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
