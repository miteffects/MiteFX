//! The suite's plugins as blocks (docs/design/rig-brief.md §3, slice 3): a Comp, Delay, Reverb,
//! Mod, Double or Tuner block *is* that plugin. It holds the plugin's whole parameter set (saved
//! with the board by the plugin's frozen ids), runs the plugin's own engine with its own telemetry,
//! and opens the plugin's own editor (presets, snapshots, undo and every control) bound to the
//! block. Its simple view shows a few of those same parameters.
//!
//! The plugins' settings functions build ids as they go (they allocate), so the settings are
//! computed off the audio thread whenever a value changes, and the unit copies the newest with a
//! try-lock that never waits.

use std::any::Any;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use mite_dsp::pitch::PitchDetector;
use mite_ui::{MemoryParamHost, View};

use crate::blocks::{BlockParam, Kind, Unit};
use crate::board::Live;

/// The host's tempo (bpm), for synced delays and modulations. One per process: the last Rig to
/// play sets it.
static TEMPO_BITS: AtomicU32 = AtomicU32::new(0x42F0_0000); // 120.0

pub fn set_tempo(bpm: f32) {
    if bpm.is_finite() && bpm > 0.0 {
        TEMPO_BITS.store(bpm.to_bits(), Ordering::Relaxed);
    }
}

pub fn tempo() -> f32 {
    f32::from_bits(TEMPO_BITS.load(Ordering::Relaxed))
}

/// One of the suite's plugins, as a block runs it.
pub trait Embedded: Send + Sync + 'static {
    type Shared: Send + Sync + 'static;
    type Settings: Copy + Send + 'static;
    type Engine: Send + 'static;
    fn manifest() -> String;
    fn host() -> MemoryParamHost;
    /// The plugin's accent (`color.accent` in the tokens), for its full view.
    fn accent() -> &'static str;
    /// Values a pedal starts at when the plugin's own default suits a plugin, not a pedal.
    fn pedal_defaults() -> &'static [(&'static str, f32)] {
        &[]
    }
    /// The parameters the simple view shows (in order).
    fn simple() -> &'static [&'static str];
    fn shared() -> Self::Shared;
    fn set_rate(shared: &Self::Shared, fs: f32);
    fn settings(p: &dyn Fn(&str) -> f32, tempo: f32) -> Self::Settings;
    fn engine(fs: f32, s: &Self::Settings) -> Self::Engine;
    fn process(e: &mut Self::Engine, l: &mut [f32], r: &mut [f32], s: &Self::Settings, shared: &Self::Shared, tempo: f32);
    fn latency(e: &Self::Engine) -> usize;
    fn reset(e: &mut Self::Engine, s: &Self::Settings);
    fn view(shared: Arc<Self::Shared>) -> Box<dyn View>;
    /// A line for the tile (the tuner's note, the comp's reduction), on the UI thread.
    fn summary(_shared: &Self::Shared, _p: &dyn Fn(&str) -> f32, _scratch: &mut Scratch) -> Option<String> {
        None
    }
}

/// UI-thread scratch for readouts.
#[derive(Default)]
pub struct Scratch {
    detector: Option<PitchDetector>,
    buf: Vec<f32>,
    /// The history records read so far (and the last one's values).
    seen: usize,
    last: Vec<f32>,
}

/// A block's plugin, type-erased: what the board, the graph and the editor need.
pub trait Embed: Send + Sync {
    /// Computes the settings from the block's values. UI or loader thread (allocates).
    fn recompute(&self, live: &Live);
    fn unit(&self, fs: f32) -> Box<dyn Unit>;
    fn view(&self) -> Box<dyn View>;
    fn accent(&self) -> &'static str;
    fn summary(&self, live: &Live) -> Option<String>;
    fn as_any(&self) -> &dyn Any;
}

struct State<P: Embedded> {
    shared: Arc<P::Shared>,
    settings: Mutex<P::Settings>,
    version: AtomicU64,
    scratch: Mutex<Scratch>,
    me: std::sync::Weak<State<P>>,
}

fn value_of(live: &Live, id: &str) -> f32 {
    live.kind.param_index(id).map_or(0.0, |i| live.value(i))
}

impl<P: Embedded> Embed for State<P> {
    fn recompute(&self, live: &Live) {
        let p = |id: &str| value_of(live, id);
        let s = P::settings(&p, tempo());
        *self.settings.lock().unwrap_or_else(|e| e.into_inner()) = s;
        self.version.fetch_add(1, Ordering::AcqRel);
    }

    fn unit(&self, fs: f32) -> Box<dyn Unit> {
        let state = self.me.upgrade().expect("the state is alive while it builds");
        P::set_rate(&state.shared, fs);
        let s = *state.settings.lock().unwrap_or_else(|e| e.into_inner());
        Box::new(EmbedUnit::<P> { engine: P::engine(fs, &s), local: s, seen: state.version.load(Ordering::Acquire), state })
    }

    fn view(&self) -> Box<dyn View> {
        P::view(self.shared.clone())
    }

    fn accent(&self) -> &'static str {
        P::accent()
    }

    fn summary(&self, live: &Live) -> Option<String> {
        let mut scratch = self.scratch.lock().unwrap_or_else(|e| e.into_inner());
        P::summary(&self.shared, &|id: &str| value_of(live, id), &mut scratch)
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

struct EmbedUnit<P: Embedded> {
    state: Arc<State<P>>,
    engine: P::Engine,
    local: P::Settings,
    seen: u64,
}

impl<P: Embedded> Unit for EmbedUnit<P> {
    fn process(&mut self, l: &mut [f32], r: &mut [f32], _live: &Live) {
        let v = self.state.version.load(Ordering::Acquire);
        if v != self.seen {
            // Never waits: if the editor is writing right now, the next block takes it.
            if let Ok(s) = self.state.settings.try_lock() {
                self.local = *s;
                self.seen = v;
            }
        }
        P::process(&mut self.engine, l, r, &self.local, &self.state.shared, tempo());
    }

    fn latency(&self) -> usize {
        P::latency(&self.engine)
    }

    fn reset(&mut self, _live: &Live) {
        P::reset(&mut self.engine, &self.local);
    }
}

/// The plugin id of a slice-2 pedal's parameter (boards saved before the pedals were the
/// plugins). Ids without a counterpart (the delay's tone, the doubler's width and level) go.
pub fn renamed(kind: Kind, id: &str) -> Option<&'static str> {
    let map: &[(&str, &'static str)] = match kind {
        Kind::Comp => &[("threshold", "s1_thresh"), ("ratio", "s1_ratio"), ("attack", "s1_attack"), ("release", "s1_release")],
        Kind::Delay => &[("time", "t1_time"), ("feedback", "t1_feedback"), ("pingpong", "t1_pingpong"), ("tone", "-"), ],
        Kind::Reverb => &[("type", "reverb_type")],
        Kind::Double => &[("amount", "variation"), ("delay", "timing"), ("width", "-"), ("level", "-")],
        _ => &[],
    };
    map.iter().find(|(old, _)| *old == id).map(|(_, new)| *new)
}

/// A plugin's parameters as block parameters (from its manifest and its specs), with the simple
/// view's first. Made once per kind; leaked (they live as long as the process).
fn params_of<P: Embedded>() -> Vec<BlockParam> {
    let host = P::host();
    let mut out = Vec::new();
    for line in P::manifest().lines() {
        let f: Vec<&str> = line.split(" | ").collect();
        let Some(spec) = host.spec(f[0]) else { continue };
        let spec = mite_core::ParamSpec { name: Box::leak(friendly(spec.name, f[0]).into_boxed_str()), ..spec };
        let p = match f.get(1) {
            Some(&"bool") => BlockParam { spec, toggle: true, choices: &[] },
            Some(&"choice") => {
                let names: Vec<&'static str> = f[2].split(", ").map(|n| &*Box::leak(n.to_owned().into_boxed_str())).collect();
                BlockParam { spec, toggle: false, choices: Box::leak(names.into_boxed_slice()) }
            }
            _ => BlockParam { spec, toggle: false, choices: &[] },
        };
        out.push(p);
    }
    for (id, v) in P::pedal_defaults() {
        if let Some(p) = out.iter_mut().find(|p| p.spec.id == *id) {
            p.spec.default = *v;
        }
    }
    out
}

/// A spec's name, or its id made readable when the stand-in host named it by id.
fn friendly(name: &str, id: &str) -> String {
    if name != id {
        return name.to_owned();
    }
    let mut s: String = id.replace('_', " ");
    if let Some(c) = s.get(0..1) {
        s.replace_range(0..1, &c.to_uppercase());
    }
    s
}

macro_rules! kinds {
    ($($kind:ident => $p:ty),* $(,)?) => {
        /// The block parameters of an embedded kind.
        pub fn params(kind: Kind) -> Option<&'static [BlockParam]> {
            match kind {
                $(Kind::$kind => {
                    static P: OnceLock<Vec<BlockParam>> = OnceLock::new();
                    Some(P.get_or_init(params_of::<$p>).as_slice())
                })*
                _ => None,
            }
        }

        /// A new block's plugin state (none for blocks that are not a plugin).
        pub fn state(kind: Kind) -> Option<Arc<dyn Embed>> {
            match kind {
                $(Kind::$kind => Some(Arc::new_cyclic(|me| State::<$p> { shared: Arc::new(<$p>::shared()), settings: Mutex::new(<$p>::settings(&|id: &str| params(kind).and_then(|ps| ps.iter().find(|p| p.spec.id == id)).map_or(0.0, |p| p.spec.default), tempo())), version: AtomicU64::new(0), scratch: Mutex::new(Scratch::default()), me: me.clone() }) as Arc<dyn Embed>),)*
                _ => None,
            }
        }

        /// The simple view's parameters of an embedded kind.
        pub fn simple(kind: Kind) -> Option<&'static [&'static str]> {
            match kind {
                $(Kind::$kind => Some(<$p>::simple()),)*
                _ => None,
            }
        }
    };
}

kinds! {
    Comp => CompPlugin,
    Delay => DelayPlugin,
    Reverb => VerbPlugin,
    Mod => ModPlugin,
    Double => DoublePlugin,
    Tuner => TunerPlugin,
}

// ---- the six plugins ---------------------------------------------------------------------------

pub struct CompPlugin;

impl Embedded for CompPlugin {
    type Shared = mite_comp_core::CompShared;
    type Settings = (mite_dsp::dynamics::CompSettings, mite_comp_core::dsp::Output);
    type Engine = mite_comp_core::dsp::CompEngine;
    fn manifest() -> String {
        mite_comp_core::params::manifest()
    }
    fn host() -> MemoryParamHost {
        mite_comp_core::editor::memory_host()
    }
    fn accent() -> &'static str {
        "comp"
    }
    fn simple() -> &'static [&'static str] {
        &["character", "s1_thresh", "s1_ratio", "s1_attack", "s1_release", "makeup", "mix"]
    }
    fn shared() -> Self::Shared {
        mite_comp_core::CompShared::new()
    }
    fn set_rate(shared: &Self::Shared, fs: f32) {
        shared.sample_rate.store(fs);
    }
    fn settings(p: &dyn Fn(&str) -> f32, _tempo: f32) -> Self::Settings {
        let out = mite_comp_core::dsp::Output { mix: p("mix") / 100.0, delta: p("delta") >= 0.5, bypass: p("bypass") >= 0.5, gain_db: p("out_gain"), pan: p("out_pan") / 100.0 };
        (mite_comp_core::params::comp_settings(p), out)
    }
    fn engine(fs: f32, s: &Self::Settings) -> Self::Engine {
        let mut e = mite_comp_core::dsp::CompEngine::new(fs, &s.0);
        e.prepare(fs, &s.0, &s.1);
        e
    }
    fn process(e: &mut Self::Engine, l: &mut [f32], r: &mut [f32], s: &Self::Settings, shared: &Self::Shared, _tempo: f32) {
        e.process(&mut [l, r], None, &s.0, &s.1, &shared.history);
    }
    fn latency(e: &Self::Engine) -> usize {
        e.latency()
    }
    fn reset(e: &mut Self::Engine, s: &Self::Settings) {
        e.reset(&s.0, &s.1);
    }
    fn view(shared: Arc<Self::Shared>) -> Box<dyn View> {
        Box::new(mite_comp_core::editor::CompView::new(shared))
    }
    fn summary(shared: &Self::Shared, _p: &dyn Fn(&str) -> f32, scratch: &mut Scratch) -> Option<String> {
        use mite_comp_core::dsp::record;
        scratch.last.resize(record::WIDTH, 0.0);
        let last = &mut scratch.last;
        shared.history.read_since(&mut scratch.seen, |r| last.copy_from_slice(r));
        let cut = last[record::CUT].min(last[record::CUT + 1]).min(0.0);
        Some(format!("{cut:.1} dB"))
    }
}

pub struct DelayPlugin;

impl Embedded for DelayPlugin {
    type Shared = mite_delay_core::DelayShared;
    type Settings = (mite_dsp::delay::DelaySettings, mite_delay_core::dsp::Output);
    type Engine = mite_delay_core::dsp::DelayEngine;
    fn manifest() -> String {
        mite_delay_core::params::manifest()
    }
    fn host() -> MemoryParamHost {
        mite_delay_core::editor::memory_host()
    }
    fn accent() -> &'static str {
        "delay"
    }
    fn pedal_defaults() -> &'static [(&'static str, f32)] {
        // A pedal's echo starts free and on tape (the slice-2 pedal's), not synced.
        &[("mode", 1.0), ("t1_sync", 0.0), ("t1_time", 380.0), ("t1_feedback", 35.0), ("mix", 30.0)]
    }
    fn simple() -> &'static [&'static str] {
        &["mode", "t1_time", "t1_feedback", "mix", "drive", "width", "t1_sync", "t1_pingpong"]
    }
    fn shared() -> Self::Shared {
        mite_delay_core::DelayShared::new()
    }
    fn set_rate(shared: &Self::Shared, fs: f32) {
        shared.sample_rate.store(fs);
    }
    fn settings(p: &dyn Fn(&str) -> f32, tempo: f32) -> Self::Settings {
        let out = mite_delay_core::dsp::Output { mix: p("mix") / 100.0, gain_db: p("out_gain"), bypass: p("bypass") >= 0.5 };
        (mite_delay_core::params::delay_settings(p, tempo), out)
    }
    fn engine(fs: f32, s: &Self::Settings) -> Self::Engine {
        mite_delay_core::dsp::DelayEngine::new(fs, &s.0, &s.1)
    }
    fn process(e: &mut Self::Engine, l: &mut [f32], r: &mut [f32], s: &Self::Settings, shared: &Self::Shared, tempo: f32) {
        shared.tempo.store(tempo);
        e.process(&mut [l, r], &s.0, &s.1, &shared.history, Some([&shared.spectrum, &shared.wet]));
    }
    fn latency(e: &Self::Engine) -> usize {
        e.latency()
    }
    fn reset(e: &mut Self::Engine, s: &Self::Settings) {
        e.reset(&s.0, &s.1);
    }
    fn view(shared: Arc<Self::Shared>) -> Box<dyn View> {
        Box::new(mite_delay_core::editor::DelayView::new(shared))
    }
}

pub struct VerbPlugin;

impl Embedded for VerbPlugin {
    type Shared = mite_verb_core::VerbShared;
    type Settings = (mite_dsp::reverb::ReverbSettings, mite_verb_core::dsp::Output);
    type Engine = mite_verb_core::dsp::VerbEngine;
    fn manifest() -> String {
        mite_verb_core::params::manifest()
    }
    fn host() -> MemoryParamHost {
        mite_verb_core::editor::memory_host()
    }
    fn accent() -> &'static str {
        "verb"
    }
    fn pedal_defaults() -> &'static [(&'static str, f32)] {
        &[("mix", 25.0)]
    }
    fn simple() -> &'static [&'static str] {
        &["reverb_type", "decay", "size", "predelay", "mix", "width", "freeze"]
    }
    fn shared() -> Self::Shared {
        mite_verb_core::VerbShared::new()
    }
    fn set_rate(shared: &Self::Shared, fs: f32) {
        shared.sample_rate.store(fs);
    }
    fn settings(p: &dyn Fn(&str) -> f32, _tempo: f32) -> Self::Settings {
        let out = mite_verb_core::dsp::Output { mix: p("mix") / 100.0, gain_db: p("out_gain"), bypass: p("bypass") >= 0.5 };
        (mite_verb_core::params::reverb_settings(p), out)
    }
    fn engine(fs: f32, s: &Self::Settings) -> Self::Engine {
        mite_verb_core::dsp::VerbEngine::new(fs, &s.0, &s.1)
    }
    fn process(e: &mut Self::Engine, l: &mut [f32], r: &mut [f32], s: &Self::Settings, shared: &Self::Shared, _tempo: f32) {
        e.process(&mut [l, r], &s.0, &s.1, &shared.history, Some([&shared.spectrum, &shared.wet]));
    }
    fn latency(e: &Self::Engine) -> usize {
        e.latency()
    }
    fn reset(e: &mut Self::Engine, s: &Self::Settings) {
        e.reset(&s.0, &s.1);
    }
    fn view(shared: Arc<Self::Shared>) -> Box<dyn View> {
        Box::new(mite_verb_core::editor::VerbView::new(shared))
    }
}

pub struct ModPlugin;

impl Embedded for ModPlugin {
    type Shared = mite_mod_core::ModShared;
    type Settings = (mite_dsp::modulation::ModSettings, mite_mod_core::dsp::Output);
    type Engine = mite_mod_core::dsp::ModEngine;
    fn manifest() -> String {
        mite_mod_core::params::manifest()
    }
    fn host() -> MemoryParamHost {
        mite_mod_core::editor::memory_host()
    }
    fn accent() -> &'static str {
        "mod"
    }
    fn simple() -> &'static [&'static str] {
        &["mode", "rate", "depth", "feedback", "mix", "width", "sync"]
    }
    fn shared() -> Self::Shared {
        mite_mod_core::ModShared::new()
    }
    fn set_rate(shared: &Self::Shared, fs: f32) {
        shared.sample_rate.store(fs);
    }
    fn settings(p: &dyn Fn(&str) -> f32, _tempo: f32) -> Self::Settings {
        let out = mite_mod_core::dsp::Output { gain_db: p("out_gain"), bypass: p("bypass") >= 0.5 };
        (mite_mod_core::params::mod_settings(p), out)
    }
    fn engine(fs: f32, s: &Self::Settings) -> Self::Engine {
        mite_mod_core::dsp::ModEngine::new(fs, &s.0, &s.1)
    }
    fn process(e: &mut Self::Engine, l: &mut [f32], r: &mut [f32], s: &Self::Settings, shared: &Self::Shared, tempo: f32) {
        shared.tempo.store(tempo);
        let transport = mite_dsp::modulation::Transport { tempo: Some(f64::from(tempo)), beats: None, playing: false };
        e.process(&mut [l, r], true, &s.0, &s.1, &transport, &shared.history);
    }
    fn latency(e: &Self::Engine) -> usize {
        e.latency()
    }
    fn reset(e: &mut Self::Engine, s: &Self::Settings) {
        e.reset(&s.0, &s.1);
    }
    fn view(shared: Arc<Self::Shared>) -> Box<dyn View> {
        Box::new(mite_mod_core::editor::ModView::new(shared))
    }
}

pub struct DoublePlugin;

impl Embedded for DoublePlugin {
    type Shared = mite_double_core::DoubleShared;
    type Settings = (mite_dsp::doubler::DoublerSettings, mite_double_core::dsp::Output);
    type Engine = mite_double_core::dsp::DoubleEngine;
    fn manifest() -> String {
        mite_double_core::params::manifest()
    }
    fn host() -> MemoryParamHost {
        mite_double_core::editor::memory_host()
    }
    fn accent() -> &'static str {
        "double"
    }
    fn simple() -> &'static [&'static str] {
        &["voices", "timing", "pitch", "delay", "variation", "chorus", "tone"]
    }
    fn shared() -> Self::Shared {
        mite_double_core::DoubleShared::new()
    }
    fn set_rate(shared: &Self::Shared, fs: f32) {
        shared.sample_rate.store(fs);
    }
    fn settings(p: &dyn Fn(&str) -> f32, _tempo: f32) -> Self::Settings {
        let out = mite_double_core::dsp::Output { gain_db: p("out_gain"), bypass: p("bypass") >= 0.5 };
        (mite_double_core::params::doubler_settings(p), out)
    }
    fn engine(fs: f32, s: &Self::Settings) -> Self::Engine {
        mite_double_core::dsp::DoubleEngine::new(fs, &s.1)
    }
    fn process(e: &mut Self::Engine, l: &mut [f32], r: &mut [f32], s: &Self::Settings, shared: &Self::Shared, _tempo: f32) {
        e.process(&mut [l, r], true, &s.0, &s.1, &shared.history);
    }
    fn latency(e: &Self::Engine) -> usize {
        e.latency()
    }
    fn reset(e: &mut Self::Engine, s: &Self::Settings) {
        e.reset(&s.1);
    }
    fn view(shared: Arc<Self::Shared>) -> Box<dyn View> {
        Box::new(mite_double_core::editor::DoubleView::new(shared))
    }
}

pub struct TunerPlugin;

impl Embedded for TunerPlugin {
    type Shared = mite_tuner_core::TunerShared;
    /// Mute, and A4's reference (for the tile).
    type Settings = (bool, f32);
    type Engine = mite_tuner_core::dsp::TunerEngine;
    fn manifest() -> String {
        mite_tuner_core::params::manifest()
    }
    fn host() -> MemoryParamHost {
        mite_tuner_core::editor::memory_host()
    }
    fn accent() -> &'static str {
        "tuner"
    }
    fn simple() -> &'static [&'static str] {
        &["reference", "mute"]
    }
    fn shared() -> Self::Shared {
        mite_tuner_core::TunerShared::new()
    }
    fn set_rate(shared: &Self::Shared, fs: f32) {
        shared.sample_rate.store(fs);
    }
    fn settings(p: &dyn Fn(&str) -> f32, _tempo: f32) -> Self::Settings {
        (p("mute") >= 0.5, p("reference"))
    }
    fn engine(fs: f32, s: &Self::Settings) -> Self::Engine {
        mite_tuner_core::dsp::TunerEngine::new(fs, s.0)
    }
    fn process(e: &mut Self::Engine, l: &mut [f32], r: &mut [f32], s: &Self::Settings, shared: &Self::Shared, _tempo: f32) {
        e.process(&mut [l, r], s.0, &shared.history, Some(&shared.input));
    }
    fn latency(_e: &Self::Engine) -> usize {
        0
    }
    fn reset(e: &mut Self::Engine, s: &Self::Settings) {
        e.reset(s.0);
    }
    fn view(shared: Arc<Self::Shared>) -> Box<dyn View> {
        Box::new(mite_tuner_core::editor::TunerView::new(shared))
    }
    fn summary(shared: &Self::Shared, p: &dyn Fn(&str) -> f32, scratch: &mut Scratch) -> Option<String> {
        const LEN: usize = 4096;
        let det = scratch.detector.get_or_insert_with(|| PitchDetector::new(LEN));
        scratch.buf.resize(LEN, 0.0);
        shared.input.read_latest(&mut scratch.buf);
        let fs = f64::from(shared.sample_rate.load());
        match det.detect(&scratch.buf, fs) {
            Some(f) if f.clarity > 0.8 && f.level_db > -60.0 => crate::pedals::note_of(f.hz as f32, p("reference")).map(|(n, c)| format!("{n}  {c:+.0} ¢")),
            _ => Some("—".to_owned()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::Board;
    use crate::parts::SUB;

    const EMBEDDED: [Kind; 6] = [Kind::Comp, Kind::Delay, Kind::Reverb, Kind::Mod, Kind::Double, Kind::Tuner];

    #[test]
    fn every_plugin_block_has_the_plugins_parameters_and_its_simple_view() {
        for kind in EMBEDDED {
            let params = kind.params();
            assert!(params.len() >= 3, "{}", kind.id());
            for id in simple(kind).unwrap() {
                assert!(kind.param_index(id).is_some(), "{}: {id}", kind.id());
            }
            assert!(Live::new(1, kind).embed.is_some());
        }
        assert_eq!(Kind::Delay.params()[Kind::Delay.param_index("t1_sync").unwrap()].spec.default, 0.0);
    }

    #[test]
    fn a_slice_2_board_keeps_its_pedal_settings() {
        let board = Board::new();
        board.load_json(Some(r#"{"items":[{"block":{"id":3,"kind":"comp","values":{"threshold":-30.0,"ratio":8.0}}},{"block":{"id":4,"kind":"delay","values":{"time":250.0,"tone":3000.0}}}]}"#));
        let comp = board.block(3).unwrap();
        assert_eq!(comp.live.value(Kind::Comp.param_index("s1_thresh").unwrap()), -30.0);
        assert_eq!(comp.live.value(Kind::Comp.param_index("s1_ratio").unwrap()), 8.0);
        let delay = board.block(4).unwrap();
        assert_eq!(delay.live.value(Kind::Delay.param_index("t1_time").unwrap()), 250.0);
    }

    fn rms(x: &[f32]) -> f32 {
        (x.iter().map(|v| v * v).sum::<f32>() / x.len() as f32).sqrt()
    }

    /// A value set on the block (as the plugin's editor sets it) reaches the running engine once
    /// it settles.
    #[test]
    fn an_edit_reaches_the_plugins_engine() {
        let live = Live::new(1, Kind::Delay);
        let mut u = crate::blocks::build(&live, 48_000.0);
        u.reset(&live);
        let run = |u: &mut Box<dyn Unit>| {
            let mut out = Vec::new();
            for k in 0..400 {
                let x: Vec<f32> = (0..SUB).map(|i| if k == 0 && i == 0 { 1.0 } else { 0.0 }).collect();
                let (mut l, mut r) = (x.clone(), x);
                u.process(&mut l, &mut r, &live);
                out.extend_from_slice(&l);
            }
            out
        };
        let wet = rms(&run(&mut u)[SUB..]);
        assert!(wet > 1e-4, "the echo plays: {wet}");
        live.set(Kind::Delay.param_index("mix").unwrap(), 0.0);
        assert!(live.settle());
        u.reset(&live);
        let dry = rms(&run(&mut u)[SUB * 4..]);
        assert!(dry < wet * 0.01, "mix 0: {dry} vs {wet}");
    }

    #[test]
    fn the_tuner_names_the_note_it_hears() {
        let live = Live::new(1, Kind::Tuner);
        let mut u = crate::blocks::build(&live, 48_000.0);
        u.reset(&live);
        let mut t = 0usize;
        for _ in 0..80 {
            let x: Vec<f32> = (0..SUB).map(|i| (2.0 * std::f32::consts::PI * 110.0 * (t + i) as f32 / 48_000.0).sin() * 0.5).collect();
            t += SUB;
            let (mut l, mut r) = (x.clone(), x);
            u.process(&mut l, &mut r, &live);
        }
        let line = live.embed.as_ref().unwrap().summary(&live).unwrap();
        assert!(line.starts_with('A'), "{line}");
    }
}
