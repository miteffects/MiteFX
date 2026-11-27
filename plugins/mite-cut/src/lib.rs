//! Mite Cut: drum hits found across the kit's tracks, the silences between them cut
//! (`mite_dsp::hits`). An ARA plugin (over VST3): it reads every track's clips from the host,
//! judges each track against the others (a tom mic hears the snare as loudly as a soft tom),
//! and plays each track back cut; the project's items are never touched.

use std::sync::Arc;

use nih_plug::prelude::*;

pub mod ara;
pub mod dsp;
pub mod editor;
pub mod params;
pub mod presets;
pub mod session;

use dsp::{CutEngine, Transport};
use params::CutParams;
use session::Session;

/// What the editor reads.
pub struct CutShared {
    pub session: Arc<Session>,
}

pub struct MiteCut {
    params: Arc<CutParams>,
    engine: CutEngine,
    shared: Arc<CutShared>,
}

impl Default for MiteCut {
    fn default() -> Self {
        Self { params: Arc::new(CutParams::default()), engine: CutEngine::new(48_000.0), shared: Arc::new(CutShared { session: Session::new() }) }
    }
}

impl Plugin for MiteCut {
    const NAME: &'static str = "Mite Cut";
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
        mite_plugin::state::migrate(state, params::STATE_VERSION, &[]);
    }

    fn params(&self) -> Arc<dyn Params> {
        self.params.clone()
    }

    fn editor(&mut self, _async_executor: AsyncExecutor<Self>) -> Option<Box<dyn Editor>> {
        let shared = self.shared.clone();
        mite_plugin::create_editor(self.params.clone(), "Mite Cut", editor::ACCENT, move || editor::CutView::new(shared.clone()))
    }

    fn initialize(&mut self, _layout: &AudioIOLayout, config: &BufferConfig, context: &mut impl InitContext<Self>) -> bool {
        self.engine.prepare(config.sample_rate);
        self.shared.session.set_sample_rate(config.sample_rate);
        context.set_latency_samples(0);
        true
    }

    fn reset(&mut self) {
        self.engine.reset();
    }

    fn process(&mut self, buffer: &mut Buffer, _aux: &mut AuxiliaryBuffers, context: &mut impl ProcessContext<Self>) -> ProcessStatus {
        let t = context.transport();
        let transport = Transport { playing: t.playing, pos: t.pos_samples() };
        self.engine.process(buffer.as_slice(), transport, self.params.bypass.value(), self.params.removed.value(), &self.shared.session);
        ProcessStatus::Normal
    }
}

impl ClapPlugin for MiteCut {
    const CLAP_ID: &'static str = "io.github.ansabic.mitefx.cut";
    const CLAP_DESCRIPTION: Option<&'static str> = Some("Drum hits found across the kit's tracks, the silences between them cut");
    const CLAP_MANUAL_URL: Option<&'static str> = Some(Self::URL);
    const CLAP_SUPPORT_URL: Option<&'static str> = Some("https://github.com/ansabic/MiteFX/issues");
    const CLAP_FEATURES: &'static [ClapFeature] = &[ClapFeature::AudioEffect, ClapFeature::Stereo, ClapFeature::Mono, ClapFeature::Gate];

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

impl Vst3Plugin for MiteCut {
    const VST3_CLASS_ID: [u8; 16] = *b"MiteFX-Cut---01-";
    const VST3_SUBCATEGORIES: &'static [Vst3SubCategory] = &[Vst3SubCategory::Fx, Vst3SubCategory::Dynamics];

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

nih_export_clap!(MiteCut);
nih_export_vst3!(MiteCut);
