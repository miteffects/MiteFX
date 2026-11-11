//! The Mite Amp engine (docs/design/amp-brief.md §2): input (and its calibration) → the gate's
//! detector → Pedal capture → Amp capture → loudness normalisation and the gate → tone (bass,
//! middle, treble, low and high cut) → Cab (zero-latency convolution) → output, mix, bypass.
//!
//! Captures are mono: the input is summed, the output is the same on both sides. A capture
//! trained at another rate runs inside a [`Bracket`] (resampled in and out), whose delay is the
//! plugin's latency; the dry path waits for it.
//!
//! Models and cabinets arrive from the loader through `Handover`s and swap in with a crossfade
//! (or, when the latency changes, behind a short dip). The audio thread never allocates or frees:
//! whatever it lets go of is retired to the loader.

use std::sync::atomic::{AtomicU64, Ordering};

use mite_core::handover::Handover;
use mite_core::AtomicF32;
use mite_dsp::conv::Convolver;
use mite_dsp::eq::{BandProcessor, BandSettings, BandType};
use mite_dsp::{db_to_gain, gain_to_db, LinearSmoother};
use mite_nam::NamModel;

use crate::bracket::{Bracket, MAX_CALL};
use crate::ir::{self, Cab, HEAD};

/// Engine work happens in pieces of at most this many samples.
const SUB: usize = 256;
/// Longest latency the delay lines hold (two brackets at the most extreme rates fit easily).
const MAX_LATENCY: usize = 1024;
/// Captures and cabinets crossfade over this, ms.
const SWAP_MS: f32 = 30.0;
/// The dip around a latency change, ms each way.
const DIP_MS: f32 = 10.0;
/// What a normalised capture plays at (NAM's convention), dB.
pub const NORMALIZE_TARGET_DB: f32 = -18.0;
/// Tone stack: bass shelf, middle bell and treble shelf frequencies (NAM's), Hz.
pub const BASS_HZ: f64 = 150.0;
pub const MIDDLE_HZ: f64 = 425.0;
pub const TREBLE_HZ: f64 = 1800.0;
pub const MIDDLE_Q: f64 = 0.7;
/// The cuts' slope index into `mite_dsp::eq::SLOPES_DB` (12 dB/oct).
pub const CUT_SLOPE: usize = 1;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AmpSettings {
    pub input_db: f32,
    pub gate: bool,
    pub gate_db: f32,
    pub calibrate: bool,
    pub interface_dbu: f32,
    pub pedal: bool,
    pub amp: bool,
    pub cab: bool,
    pub normalize: bool,
    pub bass_db: f32,
    pub middle_db: f32,
    pub treble_db: f32,
    pub low_cut_hz: f32,
    pub high_cut_hz: f32,
    pub mix: f32,
    pub out_db: f32,
    pub bypass: bool,
    pub pedal_gain_db: f32,
    pub pedal_level_db: f32,
    pub pedal_bass_db: f32,
    pub pedal_middle_db: f32,
    pub pedal_treble_db: f32,
    pub amp_gain_db: f32,
    pub amp_level_db: f32,
    pub cab_level_db: f32,
}

impl Default for AmpSettings {
    fn default() -> Self {
        Self {
            input_db: 0.0,
            gate: true,
            gate_db: -80.0,
            calibrate: false,
            interface_dbu: 12.0,
            pedal: true,
            amp: true,
            cab: true,
            normalize: true,
            bass_db: 0.0,
            middle_db: 0.0,
            treble_db: 0.0,
            low_cut_hz: 20.0,
            high_cut_hz: 20_000.0,
            mix: 1.0,
            out_db: 0.0,
            bypass: false,
            pedal_gain_db: 0.0,
            pedal_level_db: 0.0,
            pedal_bass_db: 0.0,
            pedal_middle_db: 0.0,
            pedal_treble_db: 0.0,
            amp_gain_db: 0.0,
            amp_level_db: 0.0,
            cab_level_db: 0.0,
        }
    }
}

/// The pedal's tone (after it): the same three bands as the amp's.
/// A switched-off pedal takes its tone with it (the bands fade out).
pub fn pedal_bands(s: &AmpSettings) -> [BandSettings; 3] {
    let t = tone_bands(&AmpSettings { bass_db: s.pedal_bass_db, middle_db: s.pedal_middle_db, treble_db: s.pedal_treble_db, ..*s });
    [t[0], t[1], t[2]].map(|b| BandSettings { enabled: b.enabled && s.pedal, ..b })
}

/// A slot's gain or level: as set while the slot is on, unity while it is off (it passes the
/// signal untouched).
fn slot_gain(on: bool, db: f32) -> f32 {
    if on { db_to_gain(db) } else { 1.0 }
}

/// The amp's tone and the cuts (before the cabinet) as `mite_dsp::eq` settings (also what the
/// editor draws).
pub fn tone_bands(s: &AmpSettings) -> [BandSettings; 5] {
    let band = |kind, freq: f64, gain: f32, q: f64, enabled: bool| BandSettings { enabled, kind, freq, gain_db: f64::from(gain), q, slope: CUT_SLOPE, ..BandSettings::default() };
    let q = std::f64::consts::FRAC_1_SQRT_2;
    [
        band(BandType::LowShelf, BASS_HZ, s.bass_db, q, s.bass_db.abs() > 1e-3),
        band(BandType::Bell, MIDDLE_HZ, s.middle_db, MIDDLE_Q, s.middle_db.abs() > 1e-3),
        band(BandType::HighShelf, TREBLE_HZ, s.treble_db, q, s.treble_db.abs() > 1e-3),
        band(BandType::LowCut, f64::from(s.low_cut_hz), 0.0, q, s.low_cut_hz > 20.5),
        band(BandType::HighCut, f64::from(s.high_cut_hz), 0.0, q, s.high_cut_hz < 19_900.0),
    ]
}

/// A capture ready to play at the host's rate.
pub struct Loaded {
    pub model: NamModel,
    /// An amp and its cabinet in one capture: the Cab slot steps aside.
    pub full_rig: bool,
    pub bracket: Option<Bracket>,
    pub loudness_db: Option<f32>,
    pub input_dbu: Option<f32>,
    pub output_dbu: Option<f32>,
}

impl Loaded {
    pub fn new(model: NamModel, host_rate: f64) -> Self {
        let info = model.info();
        let rate = info.sample_rate.unwrap_or(48_000.0);
        let (loudness_db, input_dbu, output_dbu) = (info.loudness.map(|v| v as f32), info.input_level_dbu.map(|v| v as f32), info.output_level_dbu.map(|v| v as f32));
        let bracket = (Bracket::latency_for(host_rate, rate) > 0).then(|| Bracket::new(host_rate, rate, model.max_block()));
        Self { model, bracket, loudness_db, input_dbu, output_dbu, full_rig: false }
    }

    pub fn latency(&self) -> usize {
        self.bracket.as_ref().map_or(0, Bracket::latency)
    }

    fn process(&mut self, x: &[f32], y: &mut [f32]) {
        match &mut self.bracket {
            Some(b) => b.process(&mut self.model, x, y),
            None => self.model.process(x, y),
        }
    }

    fn reset(&mut self) {
        self.model.reset();
        if let Some(b) = &mut self.bracket {
            b.reset();
        }
    }
}

/// What the loader sends a model slot: a capture, or nothing (the slot emptied).
pub struct ModelLoad {
    pub model: Option<Loaded>,
}

/// What it sends the cab slot. Also the box a kernel the convolver let go of travels back in.
pub struct CabLoad {
    pub cab: Option<Cab>,
}

/// The handovers from the loader, and the telemetry for the editor.
pub struct AmpShared {
    pub models: [Handover<ModelLoad>; 2],
    pub cab: Handover<CabLoad>,
    pub in_peak: AtomicF32,
    pub out_peak: AtomicF32,
    pub gate_db: AtomicF32,
    /// Moves when the engine takes something from a handover (the loader's tests wait on it).
    pub installed: AtomicU64,
}

impl Default for AmpShared {
    fn default() -> Self {
        Self::new()
    }
}

impl AmpShared {
    pub fn new() -> Self {
        Self { models: [Handover::new(), Handover::new()], cab: Handover::new(), in_peak: AtomicF32::new(-150.0), out_peak: AtomicF32::new(-150.0), gate_db: AtomicF32::new(0.0), installed: AtomicU64::new(0) }
    }

    /// The meters' peaks since the last call, dB (IN, OUT).
    pub fn take_peaks(&self) -> (f32, f32) {
        let peaks = (self.in_peak.load(), self.out_peak.load());
        self.in_peak.store(-150.0);
        self.out_peak.store(-150.0);
        peaks
    }
}

fn fade_curve(t: f32) -> f32 {
    0.5 - 0.5 * (std::f32::consts::PI * t.clamp(0.0, 1.0)).cos()
}

/// A mono delay line (a slot's off path, the gate's gain).
struct Delay {
    buf: Vec<f32>,
    pos: usize,
    delay: usize,
}

impl Delay {
    fn new(max: usize) -> Self {
        Self { buf: vec![0.0; (max + SUB).next_power_of_two()], pos: 0, delay: 0 }
    }

    fn tick(&mut self, x: f32) -> f32 {
        let mask = self.buf.len() - 1;
        self.buf[self.pos & mask] = x;
        let y = self.buf[(self.pos + self.buf.len() - self.delay) & mask];
        self.pos = self.pos.wrapping_add(1);
        y
    }

    fn reset(&mut self) {
        self.buf.fill(0.0);
    }
}

/// Values the audio thread is done with, waiting for a free retire slot (never freed here).
struct Stash<T> {
    items: [Option<Box<T>>; 4],
}

impl<T> Stash<T> {
    fn new() -> Self {
        Self { items: [None, None, None, None] }
    }

    fn retire(&mut self, handover: &Handover<T>, value: Box<T>) {
        if let Err(v) = handover.retire(value) {
            match self.items.iter_mut().find(|s| s.is_none()) {
                Some(slot) => *slot = Some(v),
                // The loader has stopped collecting: leaking beats freeing on the audio thread.
                None => std::mem::forget(v),
            }
        }
    }

    fn flush(&mut self, handover: &Handover<T>) {
        for slot in &mut self.items {
            if let Some(v) = slot.take() {
                if let Err(v) = handover.retire(v) {
                    *slot = Some(v);
                    return;
                }
            }
        }
    }
}

struct ModelSlot {
    cur: Box<ModelLoad>,
    old: Option<Box<ModelLoad>>,
    fade: usize,
    on: LinearSmoother,
    off_path: Delay,
    stash: Stash<ModelLoad>,
    old_out: Vec<f32>,
}

impl ModelSlot {
    fn new() -> Self {
        Self { cur: Box::new(ModelLoad { model: None }), old: None, fade: 0, on: LinearSmoother::new(1.0), off_path: Delay::new(MAX_LATENCY), stash: Stash::new(), old_out: vec![0.0; SUB] }
    }

    fn latency(&self) -> usize {
        self.cur.model.as_ref().map_or(0, Loaded::latency)
    }

    fn loaded(&self) -> Option<&Loaded> {
        self.cur.model.as_ref()
    }

    fn active(&self, on: bool) -> Option<&Loaded> {
        if on { self.loaded() } else { None }
    }

    /// `x` through the capture into `y` (an empty slot passes `x`, `latency` late either way).
    fn process(&mut self, x: &[f32], y: &mut [f32], fade_len: usize) {
        let n = x.len();
        match &mut self.cur.model {
            Some(m) => m.process(x, y),
            None => y[..n].copy_from_slice(x),
        }
        if let Some(old) = &mut self.old {
            match &mut old.model {
                Some(m) => m.process(x, &mut self.old_out[..n]),
                None => self.old_out[..n].copy_from_slice(x),
            }
            for (v, &o) in y[..n].iter_mut().zip(&self.old_out[..n]) {
                let w = fade_curve(self.fade as f32 / fade_len as f32);
                *v = o + w * (*v - o);
                self.fade += 1;
            }
        }
        // Off: the input, as late as the capture would have made it.
        self.off_path.delay = self.latency();
        for i in 0..n {
            let off = self.off_path.tick(x[i]);
            let g = self.on.next_value();
            y[i] = off + g * (y[i] - off);
        }
    }
}

/// A cabinet: the head taps run directly, the tail in the FFT convolver one block (= HEAD) late.
struct CabEngine {
    head: [f32; HEAD],
    hist: [f32; 2 * HEAD],
    hpos: usize,
    conv: Convolver,
    /// How many partitions the convolver holds.
    capacity: usize,
    has_tail: bool,
    loaded: bool,
    zeros: Vec<f32>,
    tail_out: Vec<f32>,
}

impl CabEngine {
    fn new(max_len: usize) -> Self {
        let max_len = max_len.max(HEAD);
        Self { head: [0.0; HEAD], hist: [0.0; 2 * HEAD], hpos: 0, conv: Convolver::new(HEAD, max_len, 64), capacity: max_len.div_ceil(HEAD), has_tail: false, loaded: false, zeros: vec![0.0; SUB], tail_out: vec![0.0; SUB] }
    }

    /// Installs `cab` (taking its tail; the convolver's previous kernel goes back into the box).
    fn install(&mut self, load: &mut CabLoad) {
        self.reset();
        match load.cab.as_mut() {
            // A tail made for another rate would not fit: refuse it (the loader rebuilds).
            Some(cab) if cab.tail.as_ref().is_some_and(|t| t.block() != HEAD || t.partitions() > self.capacity) => self.loaded = false,
            Some(cab) => {
                self.head = cab.head;
                self.has_tail = cab.tail.is_some();
                let old = cab.tail.take().and_then(|t| self.conv.replace(t));
                cab.tail = old;
                self.loaded = true;
            }
            None => self.loaded = false,
        }
    }

    fn reset(&mut self) {
        self.hist = [0.0; 2 * HEAD];
        self.hpos = 0;
        self.conv.reset();
    }

    fn process(&mut self, x: &[f32], y: &mut [f32]) {
        let n = x.len();
        if !self.loaded {
            y[..n].copy_from_slice(x);
            return;
        }
        for i in 0..n {
            self.hpos = (self.hpos + HEAD - 1) % HEAD;
            self.hist[self.hpos] = x[i];
            self.hist[self.hpos + HEAD] = x[i];
            let w = &self.hist[self.hpos..self.hpos + HEAD];
            y[i] = self.head.iter().zip(w).map(|(a, b)| a * b).sum();
        }
        if self.has_tail {
            self.tail_out[..n].copy_from_slice(x);
            let (t, z) = (&mut self.tail_out[..n], &mut self.zeros[..n]);
            self.conv.process(t, z);
            for (v, t) in y[..n].iter_mut().zip(&self.tail_out[..n]) {
                *v += t;
            }
            self.zeros[..n].fill(0.0);
        }
    }
}

/// The NAM-style gate: the input's power over 10 ms against the threshold, open in 5 ms after
/// 10 ms hold, closed in 50 ms; its gain lands on the captures' output.
struct Gate {
    power: f32,
    a_power: f32,
    hold: usize,
    hold_len: usize,
    gain_db: f32,
    a_open: f32,
    a_close: f32,
}

impl Gate {
    fn new(fs: f32) -> Self {
        let a = |ms: f32| (-1.0 / (ms * 0.001 * fs)).exp();
        Self { power: 0.0, a_power: a(10.0), hold: 0, hold_len: (0.01 * fs) as usize, gain_db: 0.0, a_open: a(5.0), a_close: a(50.0) }
    }

    /// Open, holding: a note right after a reset is not cut.
    fn reset(&mut self) {
        self.power = 0.0;
        self.hold = self.hold_len;
        self.gain_db = 0.0;
    }

    fn next(&mut self, x: f32, on: bool, threshold_db: f32) -> f32 {
        self.power = self.a_power * self.power + (1.0 - self.a_power) * x * x;
        let level = 10.0 * self.power.max(1e-20).log10();
        let target = if !on || level >= threshold_db {
            self.hold = self.hold_len;
            0.0
        } else if self.hold > 0 {
            self.hold -= 1;
            0.0
        } else {
            ((level - threshold_db) * 10.0).max(-90.0)
        };
        let a = if target > self.gain_db { self.a_open } else { self.a_close };
        self.gain_db = target + a * (self.gain_db - target);
        self.gain_db
    }
}

enum Dip {
    Steady,
    /// Fading out, `n` samples in.
    Out(usize),
    /// Fading back in after the swap.
    In(usize),
}

pub struct AmpEngine {
    fs: f32,
    slots: [ModelSlot; 2],
    cabs: [CabEngine; 2],
    cab_active: usize,
    cab_fade: Option<usize>,
    cab_stash: Stash<CabLoad>,
    cab_on: LinearSmoother,
    tone: [BandProcessor; 5],
    pedal_tone: [BandProcessor; 3],
    /// Each slot's gain in and level out, and the cabinet's level.
    pedal_gain: LinearSmoother,
    pedal_level: LinearSmoother,
    amp_gain: LinearSmoother,
    amp_level: LinearSmoother,
    cab_level: LinearSmoother,
    gate: Gate,
    gate_delay: Delay,
    input: LinearSmoother,
    between: LinearSmoother,
    normal: LinearSmoother,
    out: LinearSmoother,
    mix: LinearSmoother,
    bypass: LinearSmoother,
    dry: [Delay; 2],
    dip: Dip,
    pending: [Option<Box<ModelLoad>>; 2],
    fade_len: usize,
    dip_len: usize,
    buf: [Vec<f32>; 4],
}

impl AmpEngine {
    pub fn new(fs: f32, s: &AmpSettings) -> Self {
        let max_len = (ir::MAX_SECONDS * f64::from(fs)) as usize + HEAD;
        let mut e = Self {
            fs,
            slots: [ModelSlot::new(), ModelSlot::new()],
            cabs: [CabEngine::new(max_len), CabEngine::new(max_len)],
            cab_active: 0,
            cab_fade: None,
            cab_stash: Stash::new(),
            cab_on: LinearSmoother::new(1.0),
            tone: std::array::from_fn(|_| BandProcessor::new(f64::from(fs), 20.0)),
            pedal_tone: std::array::from_fn(|_| BandProcessor::new(f64::from(fs), 20.0)),
            pedal_gain: LinearSmoother::new(1.0),
            pedal_level: LinearSmoother::new(1.0),
            amp_gain: LinearSmoother::new(1.0),
            amp_level: LinearSmoother::new(1.0),
            cab_level: LinearSmoother::new(1.0),
            gate: Gate::new(fs),
            gate_delay: Delay::new(MAX_LATENCY),
            input: LinearSmoother::new(1.0),
            between: LinearSmoother::new(1.0),
            normal: LinearSmoother::new(1.0),
            out: LinearSmoother::new(1.0),
            mix: LinearSmoother::new(1.0),
            bypass: LinearSmoother::new(0.0),
            dry: [Delay::new(MAX_LATENCY), Delay::new(MAX_LATENCY)],
            dip: Dip::Steady,
            pending: [None, None],
            fade_len: 1,
            dip_len: 1,
            buf: std::array::from_fn(|_| vec![0.0; SUB]),
        };
        e.prepare(fs, s);
        e
    }

    /// Sizes everything for `fs`. Allocates. The slots keep what they hold (the loader rebuilds
    /// captures for a new rate).
    pub fn prepare(&mut self, fs: f32, s: &AmpSettings) {
        if (fs - self.fs).abs() > 0.5 {
            let max_len = (ir::MAX_SECONDS * f64::from(fs)) as usize + HEAD;
            self.cabs = [CabEngine::new(max_len), CabEngine::new(max_len)];
        }
        self.fs = fs;
        self.gate = Gate::new(fs);
        for b in self.tone.iter_mut().chain(self.pedal_tone.iter_mut()) {
            *b = BandProcessor::new(f64::from(fs), 20.0);
        }
        for sm in [&mut self.pedal_gain, &mut self.pedal_level, &mut self.amp_gain, &mut self.amp_level, &mut self.cab_level] {
            sm.set_ramp(fs, 20.0);
        }
        let [s0, s1] = &mut self.slots;
        for sm in [&mut self.input, &mut self.out, &mut self.mix, &mut self.bypass, &mut self.cab_on, &mut s0.on, &mut s1.on] {
            sm.set_ramp(fs, 20.0);
        }
        for sm in [&mut self.between, &mut self.normal] {
            sm.set_ramp(fs, 50.0);
        }
        self.fade_len = ((SWAP_MS * 0.001 * fs) as usize).max(1);
        self.dip_len = ((DIP_MS * 0.001 * fs) as usize).max(1);
        self.reset(s);
    }

    /// Clears all signal state and jumps to `s`. Allocation-free (pre-warms the captures).
    pub fn reset(&mut self, s: &AmpSettings) {
        for slot in &mut self.slots {
            if let Some(m) = &mut slot.cur.model {
                m.reset();
            }
            slot.off_path.reset();
        }
        for c in &mut self.cabs {
            c.reset();
        }
        self.gate.reset();
        self.gate_delay.reset();
        for d in &mut self.dry {
            d.reset();
        }
        for (b, t) in self.tone.iter_mut().zip(tone_bands(s)) {
            b.reset(&t);
        }
        for (b, t) in self.pedal_tone.iter_mut().zip(pedal_bands(s)) {
            b.reset(&t);
        }
        self.pedal_gain.reset(slot_gain(s.pedal, s.pedal_gain_db));
        self.pedal_level.reset(slot_gain(s.pedal, s.pedal_level_db));
        self.amp_gain.reset(slot_gain(s.amp, s.amp_gain_db));
        self.amp_level.reset(slot_gain(s.amp, s.amp_level_db));
        self.cab_level.reset(db_to_gain(s.cab_level_db));
        let (between, normal) = self.model_gains(s);
        self.input.reset(self.input_gain(s));
        self.between.reset(between);
        self.normal.reset(normal);
        self.out.reset(db_to_gain(s.out_db));
        self.mix.reset(s.mix);
        self.bypass.reset(if s.bypass { 1.0 } else { 0.0 });
        self.cab_on.reset(if self.cab_engaged(s) { 1.0 } else { 0.0 });
        self.slots[0].on.reset(if s.pedal { 1.0 } else { 0.0 });
        self.slots[1].on.reset(if s.amp { 1.0 } else { 0.0 });
    }

    /// The plugin's latency, samples.
    pub fn latency(&self) -> usize {
        self.slots[0].latency() + self.slots[1].latency()
    }

    /// Installs captures and a cabinet at once (prepare, offline). Whatever they replace comes
    /// back for the caller to drop (not on the audio thread).
    pub fn install_now(&mut self, models: [Option<Box<ModelLoad>>; 2], cab: Option<Box<CabLoad>>) -> Vec<Box<ModelLoad>> {
        let mut gone = Vec::new();
        for (slot, m) in self.slots.iter_mut().zip(models) {
            if let Some(mut m) = m {
                if let Some(l) = &mut m.model {
                    l.reset();
                }
                gone.push(std::mem::replace(&mut slot.cur, m));
                if let Some(old) = slot.old.take() {
                    gone.push(old);
                }
            }
        }
        if let Some(mut c) = cab {
            self.cabs[self.cab_active].install(&mut c);
            self.cab_fade = None;
        }
        gone
    }

    /// The cabinet plays when it is on, holds a response, and the amp is not a full rig (which
    /// has its own). Otherwise the stage, its level with it, passes the signal.
    pub fn cab_engaged(&self, s: &AmpSettings) -> bool {
        let loaded = self.cabs[self.cab_active].loaded || self.cab_fade.is_some();
        s.cab && loaded && !self.slots[1].active(s.amp).is_some_and(|m| m.full_rig)
    }

    fn input_gain(&self, s: &AmpSettings) -> f32 {
        let first = self.slots[0].active(s.pedal).or_else(|| self.slots[1].active(s.amp));
        let cal = match (s.calibrate, first.and_then(|m| m.input_dbu)) {
            (true, Some(level)) => s.interface_dbu - level,
            _ => 0.0,
        };
        db_to_gain(s.input_db + cal)
    }

    /// The gain between the captures (calibrated pedal → amp) and the normalisation after them.
    fn model_gains(&self, s: &AmpSettings) -> (f32, f32) {
        let (pedal, amp) = (self.slots[0].active(s.pedal), self.slots[1].active(s.amp));
        let between = match (s.calibrate, pedal.and_then(|p| p.output_dbu), amp.and_then(|a| a.input_dbu)) {
            (true, Some(out), Some(inp)) => db_to_gain(out - inp),
            _ => 1.0,
        };
        let last = amp.or(pedal);
        let normal = match (s.normalize, last.and_then(|m| m.loudness_db)) {
            (true, Some(l)) => db_to_gain(NORMALIZE_TARGET_DB - l),
            _ => 1.0,
        };
        (between, normal)
    }

    /// Takes what the loader sent, starting crossfades (or a dip, when the latency changes).
    fn take_new(&mut self, shared: &AmpShared) {
        for (i, slot) in self.slots.iter_mut().enumerate() {
            slot.stash.flush(&shared.models[i]);
        }
        self.cab_stash.flush(&shared.cab);
        if !matches!(self.dip, Dip::Steady) {
            return;
        }
        let mut latency_changes = false;
        for i in 0..2 {
            if self.slots[i].old.is_some() || self.pending[i].is_some() {
                continue;
            }
            if let Some(new) = shared.models[i].take() {
                shared.installed.fetch_add(1, Ordering::AcqRel);
                let lat = new.model.as_ref().map_or(0, Loaded::latency);
                if lat != self.slots[i].latency() {
                    latency_changes = true;
                }
                self.pending[i] = Some(new);
            }
        }
        if latency_changes {
            self.dip = Dip::Out(0);
        } else {
            for i in 0..2 {
                if let Some(new) = self.pending[i].take() {
                    let slot = &mut self.slots[i];
                    slot.old = Some(std::mem::replace(&mut slot.cur, new));
                    slot.fade = 0;
                }
            }
        }
        if self.cab_fade.is_none() {
            if let Some(mut c) = shared.cab.take() {
                shared.installed.fetch_add(1, Ordering::AcqRel);
                let next = 1 - self.cab_active;
                self.cabs[next].install(&mut c);
                self.cab_stash.retire(&shared.cab, c);
                self.cab_fade = Some(0);
            }
        }
    }

    /// The dip's swap point: everything pending goes in at once, the delays follow.
    fn swap_behind_dip(&mut self, shared: &AmpShared) {
        for i in 0..2 {
            if let Some(mut new) = self.pending[i].take() {
                if let Some(m) = &mut new.model {
                    m.reset();
                }
                let slot = &mut self.slots[i];
                let gone = std::mem::replace(&mut slot.cur, new);
                slot.stash.retire(&shared.models[i], gone);
                if let Some(old) = slot.old.take() {
                    slot.stash.retire(&shared.models[i], old);
                }
                slot.off_path.reset();
            }
        }
        for d in &mut self.dry {
            d.reset();
        }
        self.gate_delay.reset();
    }

    pub fn process(&mut self, channels: &mut [&mut [f32]], s: &AmpSettings, shared: &AmpShared) {
        if channels.is_empty() {
            return;
        }
        self.take_new(shared);
        let len = channels.iter().map(|c| c.len()).min().unwrap_or(0);
        let mut at = 0;
        while at < len {
            let n = (len - at).min(SUB).min(MAX_CALL);
            self.block(channels, at, n, s, shared);
            at += n;
        }
    }

    fn block(&mut self, channels: &mut [&mut [f32]], at: usize, n: usize, s: &AmpSettings, shared: &AmpShared) {
        let stereo = channels.len() > 1;
        let latency = self.latency();
        self.input.set_target(self.input_gain(s));
        let (between, normal) = self.model_gains(s);
        self.between.set_target(between);
        self.normal.set_target(normal);
        self.out.set_target(db_to_gain(s.out_db));
        self.mix.set_target(s.mix);
        self.bypass.set_target(if s.bypass { 1.0 } else { 0.0 });
        self.cab_on.set_target(if self.cab_engaged(s) { 1.0 } else { 0.0 });
        self.pedal_gain.set_target(slot_gain(s.pedal, s.pedal_gain_db));
        self.pedal_level.set_target(slot_gain(s.pedal, s.pedal_level_db));
        self.amp_gain.set_target(slot_gain(s.amp, s.amp_gain_db));
        self.amp_level.set_target(slot_gain(s.amp, s.amp_level_db));
        self.cab_level.set_target(db_to_gain(s.cab_level_db));
        for (b, t) in self.pedal_tone.iter_mut().zip(pedal_bands(s)) {
            b.set_target(&t);
        }
        self.slots[0].on.set_target(if s.pedal { 1.0 } else { 0.0 });
        self.slots[1].on.set_target(if s.amp { 1.0 } else { 0.0 });
        for (b, t) in self.tone.iter_mut().zip(tone_bands(s)) {
            b.set_target(&t);
        }
        for d in &mut self.dry {
            d.delay = latency;
        }
        self.gate_delay.delay = latency;
        // The scratch buffers, moved out for the block (so `self` stays free to borrow).
        let mut bufs = std::mem::take(&mut self.buf);
        let [a, b, c, d] = &mut bufs;
        let (x, y, z, gates) = (&mut a[..n], &mut b[..n], &mut c[..n], &mut d[..n]);
        // In: mono, input gain, the gate's detector (its gain waits for the captures' delay).
        let mut in_peak = 0.0f32;
        for i in 0..n {
            let l = channels[0][at + i];
            let m = if stereo { 0.5 * (l + channels[1][at + i]) } else { l };
            let v = m * self.input.next_value();
            in_peak = in_peak.max(v.abs());
            x[i] = v;
            let g = self.gate.next(v, s.gate, s.gate_db);
            gates[i] = self.gate_delay.tick(g);
        }
        // The captures, each with its gain in and level out (the pedal's tone after it). A
        // slot switched off passes the signal untouched: its gain, level and tone go with it.
        let fade = self.fade_len;
        for v in x.iter_mut() {
            *v *= self.pedal_gain.next_value();
        }
        self.slots[0].process(x, y, fade);
        for band in &mut self.pedal_tone {
            band.process(&mut [&mut *y]);
        }
        for v in y.iter_mut() {
            *v *= self.pedal_level.next_value() * self.between.next_value() * self.amp_gain.next_value();
        }
        self.slots[1].process(y, z, fade);
        let mut gate_db = 0.0f32;
        for i in 0..n {
            gate_db = gate_db.min(gates[i]);
            z[i] *= self.normal.next_value() * db_to_gain(gates[i]) * self.amp_level.next_value();
        }
        // Tone.
        for band in &mut self.tone {
            band.process(&mut [&mut *z]);
        }
        // The cabinet (crossfading to a new one), and its switch.
        let active = self.cab_active;
        let (first, second) = self.cabs.split_at_mut(1);
        let (cur, other) = if active == 0 { (&mut first[0], &mut second[0]) } else { (&mut second[0], &mut first[0]) };
        cur.process(z, x);
        if let Some(f) = self.cab_fade {
            other.process(z, y);
            for i in 0..n {
                let w = fade_curve((f + i) as f32 / fade as f32);
                x[i] += w * (y[i] - x[i]);
            }
            let f = f + n;
            if f >= fade {
                self.cab_fade = None;
                self.cab_active = 1 - active;
            } else {
                self.cab_fade = Some(f);
            }
        }
        for i in 0..n {
            let g = self.cab_on.next_value();
            let level = self.cab_level.next_value();
            x[i] = z[i] + g * (x[i] * level - z[i]);
        }
        // Out: gain, mix against the waiting dry, bypass, the dip.
        let mut out_peak = 0.0f32;
        for i in 0..n {
            let wet = x[i] * self.out.next_value();
            let (mix, by) = (self.mix.next_value(), self.bypass.next_value());
            let dip = match self.dip {
                Dip::Steady => 1.0,
                Dip::Out(k) => 1.0 - fade_curve(k as f32 / self.dip_len as f32),
                Dip::In(k) => fade_curve(k as f32 / self.dip_len as f32),
            };
            for (ch, buf) in channels.iter_mut().enumerate().take(2) {
                let dry = self.dry[ch].tick(buf[at + i]);
                let mixed = dry + mix * (wet - dry);
                let v = (mixed + by * (dry - mixed)) * dip;
                out_peak = out_peak.max(v.abs());
                buf[at + i] = v;
            }
            self.dip = match self.dip {
                Dip::Steady => Dip::Steady,
                Dip::Out(k) if k + 1 >= self.dip_len => Dip::In(0),
                Dip::Out(k) => Dip::Out(k + 1),
                Dip::In(k) if k + 1 >= self.dip_len => Dip::Steady,
                Dip::In(k) => Dip::In(k + 1),
            };
            if matches!(self.dip, Dip::In(0)) {
                self.swap_behind_dip(shared);
            }
        }
        // Crossfades that ended: their outgoing captures go back to the loader.
        for (i, slot) in self.slots.iter_mut().enumerate() {
            if slot.old.is_some() && slot.fade >= fade {
                if let Some(old) = slot.old.take() {
                    slot.stash.retire(&shared.models[i], old);
                }
            }
        }
        self.buf = bufs;
        shared.in_peak.store(shared.in_peak.load().max(gain_to_db(in_peak)));
        shared.out_peak.store(shared.out_peak.load().max(gain_to_db(out_peak)));
        shared.gate_db.store(gate_db);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(e: &mut AmpEngine, s: &AmpSettings, shared: &AmpShared, x: &[f32]) -> Vec<f32> {
        let (mut l, mut r) = (x.to_vec(), x.to_vec());
        for (a, b) in l.chunks_mut(100).zip(r.chunks_mut(100)) {
            e.process(&mut [a, b], s, shared);
        }
        l
    }

    #[test]
    fn an_empty_rig_at_neutral_settings_is_the_input() {
        let s = AmpSettings { gate: false, ..AmpSettings::default() };
        let shared = AmpShared::new();
        let mut e = AmpEngine::new(48_000.0, &s);
        let x: Vec<f32> = (0..4800).map(|i| 0.3 * (i as f32 * 0.05).sin()).collect();
        let y = run(&mut e, &s, &shared, &x);
        for (a, b) in x.iter().zip(&y) {
            assert!((a - b).abs() < 1e-6);
        }
    }
}
