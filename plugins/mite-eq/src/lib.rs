//! Mite EQ — the flagship (BLUEPRINT §8). First slice: zero-latency matched bands (Bell,
//! shelves, cuts 6–96 dB/oct, notch, band pass, tilt), 24 bands, analyzer and the EQ editor.

use std::num::NonZeroU32;
use std::sync::Arc;

use mite_core::presets::CurrentPreset;
use mite_core::snapshots::Snapshots;
use mite_core::{AtomicF32, Registry, SpectrumTap, StereoMeter};
use mite_dsp::eq::BandSettings;
use mite_dsp::MINUS_INFINITY_DB;
use nih_plug::prelude::*;

pub mod dsp;
pub mod editor;
pub mod params;
pub mod presets;
pub mod reference;

use dsp::{EqEngine, Output, PhaseMode};
use params::{EqParams, BANDS};

/// Points of the spectral stage's telemetry: the analyzer's points (log-spaced over the plot's
/// range), which the spectral detector also works on.
pub const SPECTRAL_POINTS: usize = mite_dsp::analyzer::SPECTRUM_POINTS;

/// The frequency of spectral telemetry point `i`.
pub use mite_dsp::analyzer::spectrum_point_freq as spectral_point_freq;

/// Every live Mite EQ in this process: editors compare their track with the others' (masking)
/// and can carve on them.
pub static INSTANCES: Registry<EqShared> = Registry::new();

/// Audio → UI telemetry. Everything is lock-free and allocation-free on the audio side.
pub struct EqShared {
    pub meter: StereoMeter,
    pub pre: SpectrumTap,
    pub post: SpectrumTap,
    /// The sidechain input (mono sum; silence while unconnected), for the masking view and EQ
    /// Match's reference.
    pub sidechain: SpectrumTap,
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
    pub snapshots: Arc<Snapshots>,
    /// The plot's dB range picked by hand (0: Auto; the parameters' store, saved with the state).
    pub display_range: Arc<AtomicF32>,
    /// Whether the masking regions are shown (the parameters' store, saved with the state).
    pub show_masking: Arc<std::sync::atomic::AtomicBool>,
    /// Focus mode (the parameters' store, saved with the state).
    pub focus_mode: Arc<std::sync::atomic::AtomicBool>,
    pub preset: Arc<CurrentPreset>,
    /// This instance's parameters for other instances' editors to write (a carve), once its own
    /// editor has opened (UI threads only).
    pub remote: std::sync::Mutex<Option<mite_plugin::SharedParamHost>>,
}

impl EqShared {
    pub fn new() -> Self {
        Self::with_stores(Arc::new(Snapshots::new()), Arc::new(CurrentPreset::default()))
    }

    /// Telemetry around the parameters' snapshot and preset stores.
    pub fn with_stores(snapshots: Arc<Snapshots>, preset: Arc<CurrentPreset>) -> Self {
        Self {
            snapshots,
            preset,
            meter: StereoMeter::new(MINUS_INFINITY_DB),
            pre: SpectrumTap::new(editor::ANALYZER_SIZE),
            post: SpectrumTap::new(editor::ANALYZER_SIZE),
            sidechain: SpectrumTap::new(editor::ANALYZER_SIZE),
            sample_rate: AtomicF32::new(48_000.0),
            dynamic: std::array::from_fn(|_| AtomicF32::new(0.0)),
            level: std::array::from_fn(|_| AtomicF32::new(-120.0)),
            latency: AtomicF32::new(0.0),
            spectral: std::array::from_fn(|_| AtomicF32::new(0.0)),
            focus: AtomicF32::new(-1.0),
            focus_level: std::array::from_fn(|_| AtomicF32::new(-150.0)),
            focus_threshold: std::array::from_fn(|_| AtomicF32::new(-150.0)),
            focus_gain: std::array::from_fn(|_| AtomicF32::new(0.0)),
            remote: std::sync::Mutex::new(None),
            display_range: Arc::new(AtomicF32::new(0.0)),
            show_masking: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            focus_mode: Arc::new(std::sync::atomic::AtomicBool::new(false)),
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
        let mut shared = EqShared::with_stores(params.snapshots.clone(), params.preset.clone());
        shared.display_range = params.display_range.clone();
        shared.show_masking = params.show_masking.clone();
        shared.focus_mode = params.focus_mode.clone();
        let shared = Arc::new(shared);
        INSTANCES.register(&shared);
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
        // A sidechain input for dynamic bands that detect from it (silent when unconnected).
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
        let own = self.shared.clone();
        mite_plugin::create_editor_with(
            self.params.clone(),
            "Mite EQ",
            editor::ACCENT,
            move || editor::EqView::new(shared.clone()),
            move |host| *own.remote.lock().unwrap_or_else(|e| e.into_inner()) = Some(host),
        )
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

    fn process(&mut self, buffer: &mut Buffer, aux: &mut AuxiliaryBuffers, context: &mut impl ProcessContext<Self>) -> ProcessStatus {
        let settings = self.settings();
        self.set_mode();
        self.engine.set_output(self.output());
        let mut sidechain: [&[f32]; 2] = [&[], &[]];
        let mut sc_channels = 0;
        if let Some(sc) = aux.inputs.first_mut() {
            for (slot, ch) in sidechain.iter_mut().zip(sc.as_slice_immutable()) {
                *slot = ch;
                sc_channels += 1;
            }
        }
        match &sidechain[..sc_channels] {
            [l, r] => self.shared.sidechain.push(l.iter().zip(r.iter()).map(|(a, b)| 0.5 * (a + b))),
            [m] => self.shared.sidechain.push(m.iter().copied()),
            _ => self.shared.sidechain.push(std::iter::repeat_n(0.0, buffer.samples())),
        }
        let (left, right) = self.engine.process_sidechain(
            buffer.as_slice(),
            (sc_channels > 0).then_some(&sidechain[..sc_channels]),
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
