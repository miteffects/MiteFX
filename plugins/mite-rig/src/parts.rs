//! The pieces blocks are made of, from Mite Amp: a capture at the host's rate (resampled when
//! it was trained at another), a cabinet (direct head + FFT tail, zero latency), the NAM-style
//! gate, a delay line, and the stash that hands finished objects back to the loader.

use mite_core::handover::Handover;
use mite_dsp::conv::Convolver;
use mite_nam::NamModel;

use crate::bracket::Bracket;
use crate::ir::{Cab, HEAD};

/// Work happens in pieces of at most this many samples.
pub const SUB: usize = 256;
/// Longest latency a delay line holds.
pub const MAX_LATENCY: usize = 2048;

/// A capture ready to play at the host's rate.
pub struct Loaded {
    pub model: NamModel,
    /// An amp and its cabinet in one capture.
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

    pub fn process(&mut self, x: &[f32], y: &mut [f32]) {
        match &mut self.bracket {
            Some(b) => b.process(&mut self.model, x, y),
            None => self.model.process(x, y),
        }
    }

    pub fn reset(&mut self) {
        self.model.reset();
        if let Some(b) = &mut self.bracket {
            b.reset();
        }
    }
}

/// What the loader sends a capture block: a capture, or nothing (the block emptied).
pub struct ModelLoad {
    pub model: Option<Loaded>,
}

/// What it sends a cabinet block. Also the box a kernel the convolver let go of travels back in.
pub struct CabLoad {
    pub cab: Option<Cab>,
}

pub fn fade_curve(t: f32) -> f32 {
    0.5 - 0.5 * (std::f32::consts::PI * t.clamp(0.0, 1.0)).cos()
}

/// A mono delay line (a slot's off path, the gate's gain).
pub struct Delay {
    buf: Vec<f32>,
    pos: usize,
    pub delay: usize,
}

impl Delay {
    pub fn new(max: usize) -> Self {
        Self { buf: vec![0.0; (max + SUB).next_power_of_two()], pos: 0, delay: 0 }
    }

    pub fn tick(&mut self, x: f32) -> f32 {
        let mask = self.buf.len() - 1;
        self.buf[self.pos & mask] = x;
        let y = self.buf[(self.pos + self.buf.len() - self.delay) & mask];
        self.pos = self.pos.wrapping_add(1);
        y
    }

    pub fn reset(&mut self) {
        self.buf.fill(0.0);
    }
}

/// Values the audio thread is done with, waiting for a free retire slot (never freed here).
pub struct Stash<T> {
    items: [Option<Box<T>>; 4],
}

impl<T> Default for Stash<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> Stash<T> {
    pub fn new() -> Self {
        Self { items: [None, None, None, None] }
    }

    pub fn retire(&mut self, handover: &Handover<T>, value: Box<T>) {
        if let Err(v) = handover.retire(value) {
            match self.items.iter_mut().find(|s| s.is_none()) {
                Some(slot) => *slot = Some(v),
                // The loader has stopped collecting: leaking beats freeing on the audio thread.
                None => std::mem::forget(v),
            }
        }
    }

    pub fn flush(&mut self, handover: &Handover<T>) {
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

/// A cabinet: the head taps run directly, the tail in the FFT convolver one block (= HEAD) late.
pub struct CabEngine {
    head: [f32; HEAD],
    hist: [f32; 2 * HEAD],
    hpos: usize,
    conv: Convolver,
    /// How many partitions the convolver holds.
    capacity: usize,
    has_tail: bool,
    pub loaded: bool,
    zeros: Vec<f32>,
    tail_out: Vec<f32>,
}

impl CabEngine {
    pub fn new(max_len: usize) -> Self {
        let max_len = max_len.max(HEAD);
        Self { head: [0.0; HEAD], hist: [0.0; 2 * HEAD], hpos: 0, conv: Convolver::new(HEAD, max_len, 64), capacity: max_len.div_ceil(HEAD), has_tail: false, loaded: false, zeros: vec![0.0; SUB], tail_out: vec![0.0; SUB] }
    }

    /// Installs `cab` (taking its tail; the convolver's previous kernel goes back into the box).
    pub fn install(&mut self, load: &mut CabLoad) {
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

    pub fn reset(&mut self) {
        self.hist = [0.0; 2 * HEAD];
        self.hpos = 0;
        self.conv.reset();
    }

    pub fn process(&mut self, x: &[f32], y: &mut [f32]) {
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
/// 10 ms hold, closed in 50 ms; its gain lands on the signal.
pub struct Gate {
    power: f32,
    a_power: f32,
    hold: usize,
    hold_len: usize,
    gain_db: f32,
    a_open: f32,
    a_close: f32,
}

impl Gate {
    pub fn new(fs: f32) -> Self {
        let a = |ms: f32| (-1.0 / (ms * 0.001 * fs)).exp();
        Self { power: 0.0, a_power: a(10.0), hold: 0, hold_len: (0.01 * fs) as usize, gain_db: 0.0, a_open: a(5.0), a_close: a(50.0) }
    }

    /// Open, holding: a note right after a reset is not cut.
    pub fn reset(&mut self) {
        self.power = 0.0;
        self.hold = self.hold_len;
        self.gain_db = 0.0;
    }

    pub fn next(&mut self, x: f32, on: bool, threshold_db: f32) -> f32 {
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

