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
        let transport = Transport {
            playing: t.playing,
            pos: t.pos_samples(),
        };
        self.engine.process(
            buffer.as_slice(),
            transport,
            self.params.bypass.value(),
            &self.shared.session,
        );
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
        ClapFeature::PitchCorrection, ClapFeature::Custom("ara:supported")];

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
    const VST3_SUBCATEGORIES: &'static [Vst3SubCategory] =
        &[Vst3SubCategory::Fx, Vst3SubCategory::PitchShift];
}

nih_export_clap!(MitePitch);
nih_export_vst3!(MitePitch);
