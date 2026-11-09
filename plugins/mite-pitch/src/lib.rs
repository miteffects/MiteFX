//! Mite Pitch (docs/design/pitch-brief.md): a monophonic take edited note by note (pitch, drift and
//! vibrato, timing and length, formant, level; `mite_dsp::melody`). The take comes in by transfer:
//! armed, the next stretch the transport plays is recorded against the timeline, analysed on a
//! worker thread, and from then on plays back edited in that stretch.

use std::sync::Arc;

use mite_core::AtomicF32;
use nih_plug::prelude::*;

pub mod ara;
pub mod dsp;
pub mod editor;
pub mod grid;
pub mod params;
pub mod presets;
pub mod session;

use dsp::{PitchEngine, Transport};
use params::PitchParams;
use session::Session;

/// What the editor reads besides the session.
pub struct PitchShared {
    pub session: Arc<Session>,
    pub sample_rate: AtomicF32,
}

pub struct MitePitch {
    params: Arc<PitchParams>,
    engine: PitchEngine,
    shared: Arc<PitchShared>,
    /// MITE_PITCH_DIAG set: each block's input and output peaks go to the ARA log.
    diag: bool,
}

impl Default for MitePitch {
    fn default() -> Self {
        let params = Arc::new(PitchParams::default());
        let shared = Arc::new(PitchShared {
            session: params.session.clone(),
            sample_rate: AtomicF32::new(48_000.0),
        });
        Self {
            params,
            engine: PitchEngine::new(48_000.0),
            shared,
            diag: std::env::var_os("MITE_PITCH_DIAG").is_some(),
        }
    }
}

impl Plugin for MitePitch {
    const NAME: &'static str = "Mite Pitch";
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
        let shared = self.shared.clone();
        mite_plugin::create_editor(
            self.params.clone(),
            "Mite Pitch",
            editor::ACCENT,
            move || editor::PitchView::new(shared.clone()),
        )
    }

    fn initialize(
        &mut self,
        _layout: &AudioIOLayout,
        config: &BufferConfig,
        context: &mut impl InitContext<Self>,
    ) -> bool {
        self.engine.prepare(config.sample_rate);
        self.shared.sample_rate.store(config.sample_rate);
        self.shared.session.set_sample_rate(config.sample_rate);
        context.set_latency_samples(0);
        true
    }

    fn reset(&mut self) {
        self.engine.reset();
    }

    fn process(
        &mut self,
        buffer: &mut Buffer,
        _aux: &mut AuxiliaryBuffers,
        context: &mut impl ProcessContext<Self>,
    ) -> ProcessStatus {
        let t = context.transport();
        // The song's time, for the editor's bars and beats.
        if let (Some(bpm), Some(pos), Some(beats)) = (t.tempo, t.pos_samples(), t.pos_beats()) {
            let sig = (t.time_sig_numerator.unwrap_or(4), t.time_sig_denominator.unwrap_or(4));
            self.shared.session.set_song_time(bpm, sig, f64::from(t.sample_rate), pos, beats, t.bar_start_pos_beats());
        }
        let transport = Transport {
            playing: t.playing,
            pos: t.pos_samples(),
        };
        let s = &self.shared.session;
        if !self.diag {
            self.engine.process(buffer.as_slice(), transport, self.params.bypass.value(), s);
            return ProcessStatus::Normal;
        }
        let peak = |b: &mut Buffer| b.as_slice().iter().flat_map(|c| c.iter()).fold(0.0f32, |m, v| m.max(v.abs()));
        let pin = peak(buffer);
        self.engine.process(buffer.as_slice(), transport, self.params.bypass.value(), s);
        let pout = peak(buffer);
        // Diagnostics: atomics only.
        use std::sync::atomic::Ordering::Relaxed;
        if pin > f32::from_bits(s.diag_in.load(Relaxed)) {
            s.diag_in.store(pin.to_bits(), Relaxed);
        }
        if pout > f32::from_bits(s.diag_out.load(Relaxed)) {
            s.diag_out.store(pout.to_bits(), Relaxed);
        }
        s.diag_blocks.fetch_add(1, Relaxed);
        ProcessStatus::Normal
    }
}

impl ClapPlugin for MitePitch {
    const CLAP_ID: &'static str = "io.github.ansabic.mitefx.pitch";
    const CLAP_DESCRIPTION: Option<&'static str> =
        Some("Monophonic note editing: pitch, drift, timing, formant and level");
    const CLAP_MANUAL_URL: Option<&'static str> = Some(Self::URL);
    const CLAP_SUPPORT_URL: Option<&'static str> = Some("https://github.com/ansabic/MiteFX/issues");
    const CLAP_FEATURES: &'static [ClapFeature] = &[
        ClapFeature::AudioEffect,
        ClapFeature::Stereo,
        ClapFeature::Mono,
        ClapFeature::PitchCorrection];

    fn clap_factory(id: &std::ffi::CStr) -> *const std::ffi::c_void {
        ara::clap_factory(id)
    }

    fn clap_extension(id: &std::ffi::CStr) -> *const std::ffi::c_void {
        ara::clap_extension(id)
    }

    fn clap_extension_context(&self) -> *const std::ffi::c_void {
        Arc::as_ptr(&self.shared).cast()
    }
}

impl Vst3Plugin for MitePitch {
    const VST3_CLASS_ID: [u8; 16] = *b"MiteFX-Pitch-01-";
    const VST3_SUBCATEGORIES: &'static [Vst3SubCategory] = &[Vst3SubCategory::Fx, Vst3SubCategory::PitchShift];

    fn vst3_ara_factory() -> *const std::ffi::c_void {
        mite_ara::factory(&ara::INFO).cast()
    }

    fn vst3_ara_bind(context: *const std::ffi::c_void, document_controller: *mut std::ffi::c_void, known_roles: i32, assigned_roles: i32) -> *const std::ffi::c_void {
        // SAFETY: `context` is this instance's `vst3_extension_context`; the host passes a
        // document controller made by our factory.
        unsafe { ara::bind(context.cast(), document_controller.cast(), known_roles, assigned_roles) }.cast()
    }

    fn vst3_extension_context(&self) -> *const std::ffi::c_void {
        Arc::as_ptr(&self.shared).cast()
    }
}

nih_export_clap!(MitePitch);
nih_export_vst3!(MitePitch);
