//! Mite Rig (docs/design/rig-brief.md): a guitar and bass signal chain. A board of blocks read
//! left to right (NAM captures of pedals, amps and full rigs, cabinets, a gate, tone, boost,
//! volume, and the suite's own effects), forked into up to three parallel lanes and merged
//! again wherever wanted; TONE3000's library inside. Rivals: ToneX, the NAM and TONE3000
//! plugins, Helix Native.

use std::sync::Arc;

use mite_core::presets::CurrentPreset;
use mite_core::snapshots::Snapshots;
use mite_core::AtomicF32;
use nih_plug::prelude::*;

pub mod blocks;
pub mod board;
pub mod bracket;
pub mod editor;
pub mod embed;
pub mod engine;
pub mod graph;
pub mod ir;
pub mod loader;
pub mod macros;
pub mod params;
pub mod parts;
pub mod pedals;
pub mod presets;
pub mod rig;
pub mod tone3000;

use board::Board;
use engine::{RigEngine, RigShared};
use loader::{Loader, LoaderCore};
use params::RigParams;

/// What the editor shares with the plugin.
pub struct RigUi {
    pub board: Arc<Board>,
    pub engine: Arc<RigShared>,
    pub tone3000: Arc<tone3000::Session>,
    pub sample_rate: AtomicF32,
    pub latency: AtomicF32,
    pub snapshots: Arc<Snapshots>,
    pub preset: Arc<CurrentPreset>,
}

impl RigUi {
    pub fn new() -> Self {
        Self::with_stores(Arc::new(Snapshots::new()), Arc::new(CurrentPreset::default()), Arc::new(Board::with_default()), Arc::new(RigShared::new()))
    }

    pub fn with_stores(snapshots: Arc<Snapshots>, preset: Arc<CurrentPreset>, board: Arc<Board>, engine: Arc<RigShared>) -> Self {
        Self { board, engine, tone3000: Arc::new(tone3000::Session::new()), sample_rate: AtomicF32::new(48_000.0), latency: AtomicF32::new(0.0), snapshots, preset }
    }
}

impl Default for RigUi {
    fn default() -> Self {
        Self::new()
    }
}

pub struct MiteRig {
    params: Arc<RigParams>,
    engine: RigEngine,
    ui: Arc<RigUi>,
    loader: Loader,
    reported_latency: u32,
    macros: macros::MacroRunner,
}

impl Default for MiteRig {
    fn default() -> Self {
        let params = Arc::new(RigParams::default());
        let shared = Arc::new(RigShared::new());
        let ui = Arc::new(RigUi::with_stores(params.snapshots.clone(), params.preset.clone(), params.board.clone(), shared.clone()));
        let loader = Loader::new(LoaderCore::new(params.board.clone(), shared, params.clone()));
        Self { engine: RigEngine::new(48_000.0, &params.settings()), params, ui, loader, reported_latency: 0, macros: macros::MacroRunner::new() }
    }
}

impl Plugin for MiteRig {
    const NAME: &'static str = "Mite Rig";
    const VENDOR: &'static str = "MiteFX";
    const URL: &'static str = "https://github.com/ansabic/MiteFX";
    const EMAIL: &'static str = "";
    const VERSION: &'static str = env!("CARGO_PKG_VERSION");

    /// MIDI: CCs drive the macros (learned), program changes pick a preset's board.
    const MIDI_INPUT: MidiConfig = MidiConfig::MidiCCs;
    const AUDIO_IO_LAYOUTS: &'static [AudioIOLayout] = &[
        AudioIOLayout { main_input_channels: NonZeroU32::new(2), main_output_channels: NonZeroU32::new(2), ..AudioIOLayout::const_default() },
        AudioIOLayout { main_input_channels: NonZeroU32::new(1), main_output_channels: NonZeroU32::new(2), ..AudioIOLayout::const_default() },
    ];

    const SAMPLE_ACCURATE_AUTOMATION: bool = false;

    type SysExMessage = ();
    type BackgroundTask = ();

    fn filter_state(state: &mut PluginState) {
        mite_plugin::state::migrate(state, params::STATE_VERSION, &[params::v1_to_v2]);
    }

    fn params(&self) -> Arc<dyn Params> {
        self.params.clone()
    }

    fn editor(&mut self, _async_executor: AsyncExecutor<Self>) -> Option<Box<dyn Editor>> {
        let ui = self.ui.clone();
        mite_plugin::create_editor(self.params.clone(), "Mite Rig", editor::ACCENT, move || editor::RigView::new(ui.clone()))
    }

    fn initialize(&mut self, _layout: &AudioIOLayout, config: &BufferConfig, context: &mut impl InitContext<Self>) -> bool {
        let s = self.params.settings();
        self.engine.prepare(config.sample_rate, &s);
        let core = &self.loader.core;
        core.set_rate(config.sample_rate);
        // The board, built now: the first block already plays it.
        core.build_files();
        if let Some(g) = core.build_graph(true) {
            drop(self.engine.install_now(g));
        }
        if let Some(t) = core.build_macros(true) {
            self.ui.engine.macros.send(t);
        }
        self.engine.prime();
        self.engine.reset(&s);
        self.ui.sample_rate.store(config.sample_rate);
        self.reported_latency = self.engine.latency() as u32;
        context.set_latency_samples(self.reported_latency);
        self.ui.latency.store(self.reported_latency as f32);
        true
    }

    fn reset(&mut self) {
        self.engine.reset(&self.params.settings());
    }

    fn process(&mut self, buffer: &mut Buffer, _aux: &mut AuxiliaryBuffers, context: &mut impl ProcessContext<Self>) -> ProcessStatus {
        if let Some(t) = context.transport().tempo {
            embed::set_tempo(t as f32);
        }
        // Macros: the host's values, then MIDI (a CC learned or bound, a program change).
        let board = &self.params.board;
        self.macros.take(&self.ui.engine.macros);
        for (i, m) in self.params.macros.iter().enumerate() {
            self.macros.host(i, m.value() / 100.0);
        }
        for (i, s) in self.params.switches.iter().enumerate() {
            self.macros.host(macros::KNOBS + i, if s.value() { 1.0 } else { 0.0 });
        }
        while let Some(event) = context.next_event() {
            match event {
                NoteEvent::MidiCC { cc, value, .. } => self.macros.cc(&board.macros, cc, value),
                NoteEvent::MidiProgramChange { program, .. } => board.macros.program.store(i32::from(program), std::sync::atomic::Ordering::Release),
                _ => {}
            }
        }
        self.macros.apply(&board.macros);
        let s = self.params.settings();
        self.engine.process(buffer.as_slice(), &s, &self.ui.engine);
        let latency = self.engine.latency() as u32;
        if latency != self.reported_latency {
            self.reported_latency = latency;
            context.set_latency_samples(latency);
            self.ui.latency.store(latency as f32);
        }
        ProcessStatus::Normal
    }
}

impl ClapPlugin for MiteRig {
    const CLAP_ID: &'static str = "io.github.ansabic.mitefx.rig";
    const CLAP_DESCRIPTION: Option<&'static str> = Some("Guitar and bass rig: NAM captures, cabinets and effects on a free board, with TONE3000");
    const CLAP_MANUAL_URL: Option<&'static str> = Some(Self::URL);
    const CLAP_SUPPORT_URL: Option<&'static str> = Some("https://github.com/ansabic/MiteFX/issues");
    const CLAP_FEATURES: &'static [ClapFeature] = &[ClapFeature::AudioEffect, ClapFeature::Stereo, ClapFeature::Mono, ClapFeature::Distortion];
}

impl Vst3Plugin for MiteRig {
    const VST3_CLASS_ID: [u8; 16] = *b"MiteFX-Rig---001";
    const VST3_SUBCATEGORIES: &'static [Vst3SubCategory] = &[Vst3SubCategory::Fx, Vst3SubCategory::Distortion];
}

nih_export_clap!(MiteRig);
nih_export_vst3!(MiteRig);
