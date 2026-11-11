//! The Mite Rig engine: input gain → the board → output gain, mix against the dry input (in
//! time), bypass. A new graph (a structural edit) swaps in behind a short dip; the latency
//! follows the longest path through the board and is reported.

use std::sync::atomic::{AtomicU64, Ordering};

use mite_core::handover::Handover;
use mite_core::AtomicF32;
use mite_dsp::{db_to_gain, gain_to_db, LinearSmoother};

use crate::graph::Graph;
use crate::parts::{fade_curve, Delay, Stash, MAX_LATENCY, SUB};

/// The dip around a graph swap, ms each way.
const DIP_MS: f32 = 10.0;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RigSettings {
    pub input_db: f32,
    pub out_db: f32,
    pub mix: f32,
    pub bypass: bool,
}

impl Default for RigSettings {
    fn default() -> Self {
        Self { input_db: 0.0, out_db: 0.0, mix: 1.0, bypass: false }
    }
}

/// The loader's handover and the meters.
pub struct RigShared {
    pub graph: Handover<Graph>,
    /// The macros' mapping, compiled (`crate::macros`).
    pub macros: Handover<crate::macros::MacroTable>,
    pub in_peak: AtomicF32,
    pub out_peak: AtomicF32,
    /// Graphs swapped in so far (tests and the loader wait on it).
    pub swaps: AtomicU64,
}

impl Default for RigShared {
    fn default() -> Self {
        Self::new()
    }
}

impl RigShared {
    pub fn new() -> Self {
        Self { graph: Handover::new(), macros: Handover::new(), in_peak: AtomicF32::new(-150.0), out_peak: AtomicF32::new(-150.0), swaps: AtomicU64::new(0) }
    }

    /// The meters' peaks since the last call, dB (IN, OUT).
    pub fn take_peaks(&self) -> (f32, f32) {
        let peaks = (self.in_peak.load(), self.out_peak.load());
        self.in_peak.store(-150.0);
        self.out_peak.store(-150.0);
        peaks
    }
}

enum Dip {
    Steady,
    Out(usize),
    In(usize),
}

pub struct RigEngine {
    fs: f32,
    graph: Box<Graph>,
    pending: Option<Box<Graph>>,
    stash: Stash<Graph>,
    dip: Dip,
    dip_len: usize,
    input: LinearSmoother,
    out: LinearSmoother,
    mix: LinearSmoother,
    bypass: LinearSmoother,
    dry: [Delay; 2],
    buf: [Vec<f32>; 2],
}

impl RigEngine {
    pub fn new(fs: f32, s: &RigSettings) -> Self {
        let mut e = Self {
            fs,
            graph: Box::new(Graph { stages: Vec::new() }),
            pending: None,
            stash: Stash::new(),
            dip: Dip::Steady,
            dip_len: 1,
            input: LinearSmoother::new(1.0),
            out: LinearSmoother::new(1.0),
            mix: LinearSmoother::new(1.0),
            bypass: LinearSmoother::new(0.0),
            dry: [Delay::new(MAX_LATENCY), Delay::new(MAX_LATENCY)],
            buf: [vec![0.0; SUB], vec![0.0; SUB]],
        };
        e.prepare(fs, s);
        e
    }

    pub fn prepare(&mut self, fs: f32, s: &RigSettings) {
        self.fs = fs;
        for sm in [&mut self.input, &mut self.out, &mut self.mix, &mut self.bypass] {
            sm.set_ramp(fs, 20.0);
        }
        self.dip_len = ((DIP_MS * 0.001 * fs) as usize).max(1);
        self.reset(s);
    }

    pub fn sample_rate(&self) -> f32 {
        self.fs
    }

    pub fn reset(&mut self, s: &RigSettings) {
        self.graph.reset();
        self.dry.iter_mut().for_each(Delay::reset);
        self.input.reset(db_to_gain(s.input_db));
        self.out.reset(db_to_gain(s.out_db));
        self.mix.reset(s.mix);
        self.bypass.reset(if s.bypass { 1.0 } else { 0.0 });
    }

    pub fn latency(&self) -> usize {
        self.graph.latency().min(MAX_LATENCY)
    }

    /// Puts `graph` in at once (initialize, offline). The one it replaces is returned.
    pub fn install_now(&mut self, mut graph: Box<Graph>) -> Box<Graph> {
        graph.adopt(&mut self.graph);
        std::mem::replace(&mut self.graph, graph)
    }

    /// Lets every block take what the loader has sent it (a zero-length pass), so the latency
    /// is right before the first block of audio. Allocation-free.
    pub fn prime(&mut self) {
        self.graph.process(&mut [], &mut []);
    }

    /// Whether block `id` plays a full rig.
    pub fn full_rig(&mut self, id: u64) -> bool {
        self.graph.full_rig(id)
    }

    fn swap(&mut self, shared: &RigShared) {
        if let Some(mut new) = self.pending.take() {
            new.adopt(&mut self.graph);
            let old = std::mem::replace(&mut self.graph, new);
            self.stash.retire(&shared.graph, old);
            self.dry.iter_mut().for_each(Delay::reset);
            shared.swaps.fetch_add(1, Ordering::AcqRel);
        }
    }

    pub fn process(&mut self, channels: &mut [&mut [f32]], s: &RigSettings, shared: &RigShared) {
        if channels.is_empty() {
            return;
        }
        self.stash.flush(&shared.graph);
        if matches!(self.dip, Dip::Steady) && self.pending.is_none() {
            if let Some(g) = shared.graph.take() {
                self.pending = Some(g);
                self.dip = Dip::Out(0);
            }
        }
        self.input.set_target(db_to_gain(s.input_db));
        self.out.set_target(db_to_gain(s.out_db));
        self.mix.set_target(s.mix);
        self.bypass.set_target(if s.bypass { 1.0 } else { 0.0 });
        let len = channels.iter().map(|c| c.len()).min().unwrap_or(0);
        let stereo = channels.len() > 1;
        let mut at = 0;
        let (mut in_peak, mut out_peak) = (0.0f32, 0.0f32);
        while at < len {
            let n = (len - at).min(SUB);
            let lat = self.latency();
            self.dry[0].delay = lat;
            self.dry[1].delay = lat;
            // The work buffers, moved out for the block (so `self` stays free to borrow).
            let mut buf = std::mem::take(&mut self.buf);
            let [bl, br] = &mut buf;
            for i in 0..n {
                let g = self.input.next_value();
                let l = channels[0][at + i];
                let r = if stereo { channels[1][at + i] } else { l };
                bl[i] = l * g;
                br[i] = r * g;
                in_peak = in_peak.max(bl[i].abs()).max(br[i].abs());
            }
            self.graph.process(&mut bl[..n], &mut br[..n]);
            for i in 0..n {
                let (o, mix, by) = (self.out.next_value(), self.mix.next_value(), self.bypass.next_value());
                let dip = match self.dip {
                    Dip::Steady => 1.0,
                    Dip::Out(k) => 1.0 - fade_curve(k as f32 / self.dip_len as f32),
                    Dip::In(k) => fade_curve(k as f32 / self.dip_len as f32),
                };
                let wet = [bl[i] * o, br[i] * o];
                for (ch, buf) in channels.iter_mut().enumerate().take(2) {
                    let dry = self.dry[ch].tick(buf[at + i]);
                    let mixed = dry + mix * (wet[ch] - dry);
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
                if matches!(self.dip, Dip::In(0)) && self.pending.is_some() {
                    self.swap(shared);
                }
            }
            self.buf = buf;
            at += n;
        }
        shared.in_peak.store(shared.in_peak.load().max(gain_to_db(in_peak)));
        shared.out_peak.store(shared.out_peak.load().max(gain_to_db(out_peak)));
    }
}
