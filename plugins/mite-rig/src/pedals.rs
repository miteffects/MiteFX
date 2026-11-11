//! The suite's own effects as pedals on the board (docs/design/rig-brief.md §2, slice 2): each a
//! few main knobs over the engine its plugin runs (Mite Comp's compressor, Mite Delay's lines,
//! Mite Verb's room, Mite Mod's modulations, Mite Double's doubler, Mite Tuner's detector), the
//! rest at the plugin's defaults. Plus a graphic EQ and a tremolo of their own.

use mite_core::{ParamDisplay, ParamScale, ParamSpec};
use mite_dsp::eq::{BandProcessor, BandSettings, BandType};
use mite_dsp::{db_to_gain, LinearSmoother};

use crate::blocks::{BlockParam, Unit};
use crate::board::Live;

#[allow(clippy::too_many_arguments)] // one line per parameter reads best
const fn spec(id: &'static str, name: &'static str, unit: &'static str, min: f32, max: f32, default: f32, scale: ParamScale, decimals: u8, wheel: (f32, f32), display: ParamDisplay) -> ParamSpec {
    ParamSpec { id, name, unit, min, max, default, scale, smoothing_ms: 20.0, decimals, wheel_step: wheel.0, wheel_step_fine: wheel.1, display }
}

const fn knob(spec: ParamSpec) -> BlockParam {
    BlockParam { spec, toggle: false, choices: &[] }
}

const fn choice(id: &'static str, name: &'static str, choices: &'static [&'static str], default: usize) -> BlockParam {
    BlockParam { spec: spec(id, name, "", 0.0, (choices.len() - 1) as f32, default as f32, ParamScale::Linear, 0, (1.0, 1.0), ParamDisplay::Number), toggle: false, choices }
}

const fn switch(id: &'static str, name: &'static str, default: bool) -> BlockParam {
    BlockParam { spec: spec(id, name, "", 0.0, 1.0, if default { 1.0 } else { 0.0 }, ParamScale::Linear, 0, (1.0, 1.0), ParamDisplay::Number), toggle: true, choices: &[] }
}

const LIN: ParamScale = ParamScale::Linear;
const LOG: ParamScale = ParamScale::Log;
const NUM: ParamDisplay = ParamDisplay::Number;
const TIME: ParamDisplay = ParamDisplay::Time;
const DB: (f32, f32) = (0.5, 0.1);
const PCT: (f32, f32) = (1.0, 0.1);
const LOGW: (f32, f32) = (1.0 / 24.0, 1.0 / 96.0);

/// The graphic EQ's bands (a classic seven-band guitar EQ), Hz.
pub const EQ_HZ: [f64; 7] = [100.0, 200.0, 400.0, 800.0, 1600.0, 3200.0, 6400.0];
pub const EQ: [BlockParam; 8] = [
    knob(spec("b100", "100", "dB", -15.0, 15.0, 0.0, LIN, 1, DB, NUM)),
    knob(spec("b200", "200", "dB", -15.0, 15.0, 0.0, LIN, 1, DB, NUM)),
    knob(spec("b400", "400", "dB", -15.0, 15.0, 0.0, LIN, 1, DB, NUM)),
    knob(spec("b800", "800", "dB", -15.0, 15.0, 0.0, LIN, 1, DB, NUM)),
    knob(spec("b1k6", "1.6k", "dB", -15.0, 15.0, 0.0, LIN, 1, DB, NUM)),
    knob(spec("b3k2", "3.2k", "dB", -15.0, 15.0, 0.0, LIN, 1, DB, NUM)),
    knob(spec("b6k4", "6.4k", "dB", -15.0, 15.0, 0.0, LIN, 1, DB, NUM)),
    knob(spec("level", "Level", "dB", -15.0, 15.0, 0.0, LIN, 1, DB, NUM)),
];

pub const TREMOLO_NAMES: [&str; 3] = ["Sine", "Square", "Harmonic"];
pub const TREMOLO: [BlockParam; 4] = [
    choice("shape", "Shape", &TREMOLO_NAMES, 0),
    knob(spec("rate", "Rate", "Hz", 0.5, 15.0, 5.0, LOG, 2, LOGW, NUM)),
    knob(spec("depth", "Depth", "%", 0.0, 100.0, 60.0, LIN, 0, PCT, NUM)),
    knob(spec("spread", "Stereo", "%", 0.0, 100.0, 0.0, LIN, 0, PCT, NUM)),
];


fn smoother(fs: f32, v: f32) -> LinearSmoother {
    let mut s = LinearSmoother::new(v);
    s.set_ramp(fs, 20.0);
    s
}

fn index(live: &Live, i: usize) -> usize {
    live.value(i).round().max(0.0) as usize
}

// ---- EQ -----------------------------------------------------------------------------------------

pub struct EqUnit {
    bands: [BandProcessor; 7],
    level: LinearSmoother,
}

/// The graphic EQ's bands from its values (also what the editor draws).
pub fn eq_bands(gains: &[f32; 7]) -> [BandSettings; 7] {
    std::array::from_fn(|i| BandSettings { enabled: gains[i].abs() > 1e-3, kind: BandType::Bell, freq: EQ_HZ[i], gain_db: f64::from(gains[i]), q: 1.1, ..BandSettings::default() })
}

impl EqUnit {
    pub fn new(fs: f32) -> Self {
        Self { bands: std::array::from_fn(|_| BandProcessor::new(f64::from(fs), 20.0)), level: smoother(fs, 1.0) }
    }

    fn gains(live: &Live) -> [f32; 7] {
        std::array::from_fn(|i| live.value(i))
    }
}

impl Unit for EqUnit {
    fn process(&mut self, l: &mut [f32], r: &mut [f32], live: &Live) {
        for (b, s) in self.bands.iter_mut().zip(eq_bands(&Self::gains(live))) {
            b.set_target(&s);
            b.process(&mut [&mut *l, &mut *r]);
        }
        self.level.set_target(db_to_gain(live.value(7)));
        for i in 0..l.len() {
            let g = self.level.next_value();
            l[i] *= g;
            r[i] *= g;
        }
    }

    fn reset(&mut self, live: &Live) {
        for (b, s) in self.bands.iter_mut().zip(eq_bands(&Self::gains(live))) {
            b.reset(&s);
        }
        self.level.reset(db_to_gain(live.value(7)));
    }
}

// ---- Tremolo ------------------------------------------------------------------------------------

pub struct TremoloUnit {
    fs: f32,
    phase: f64,
    rate: LinearSmoother,
    depth: LinearSmoother,
    /// Harmonic: the low and high halves (a crossover at 800 Hz), moving in opposite phase.
    split: [[f32; 2]; 2],
    split_a: f32,
}

impl TremoloUnit {
    pub fn new(fs: f32) -> Self {
        Self { fs, phase: 0.0, rate: smoother(fs, 5.0), depth: smoother(fs, 0.6), split: [[0.0; 2]; 2], split_a: 1.0 - (-std::f32::consts::TAU * 800.0 / fs).exp() }
    }

    /// The gain curve over one cycle, 0…1 (1: full).
    fn wave(shape: usize, phase: f64) -> f32 {
        let s = (std::f64::consts::TAU * phase).sin() as f32;
        match shape {
            // A square with soft edges (no clicks).
            1 => 0.5 + 0.5 * (s * 6.0).tanh(),
            _ => 0.5 + 0.5 * s,
        }
    }
}

impl Unit for TremoloUnit {
    fn process(&mut self, l: &mut [f32], r: &mut [f32], live: &Live) {
        let shape = index(live, 0);
        self.rate.set_target(live.value(1));
        self.depth.set_target(live.value(2) / 100.0);
        let spread = f64::from(live.value(3) / 100.0) * 0.5;
        for i in 0..l.len() {
            let depth = self.depth.next_value();
            self.phase = (self.phase + f64::from(self.rate.next_value()) / f64::from(self.fs)).fract();
            let gain = |ph: f64| 1.0 - depth * (1.0 - Self::wave(shape, ph.fract()));
            let (pl, pr) = (self.phase, self.phase + spread);
            if shape == 2 {
                // Harmonic: the lows and the highs move in turn (half a cycle apart).
                let a = self.split_a;
                for (c, (x, ph)) in [(l[i], pl), (r[i], pr)].into_iter().enumerate() {
                    let lp = &mut self.split[c];
                    lp[0] += a * (x - lp[0]);
                    lp[1] += a * (lp[0] - lp[1]);
                    let (low, high) = (lp[1], x - lp[1]);
                    let y = low * gain(ph) + high * gain(ph + 0.5);
                    if c == 0 { l[i] = y } else { r[i] = y }
                }
            } else {
                l[i] *= gain(pl);
                r[i] *= gain(pr);
            }
        }
    }

    fn reset(&mut self, live: &Live) {
        self.phase = 0.0;
        self.split = [[0.0; 2]; 2];
        self.rate.reset(live.value(1));
        self.depth.reset(live.value(2) / 100.0);
    }
}

/// A pitch as a note name and its cents off, against `reference` (A4, Hz).
pub fn note_of(hz: f32, reference: f32) -> Option<(String, f32)> {
    if hz <= 0.0 {
        return None;
    }
    let names = ["C", "C♯", "D", "D♯", "E", "F", "F♯", "G", "G♯", "A", "A♯", "B"];
    let midi = 69.0 + 12.0 * (hz / reference).log2();
    let nearest = midi.round();
    let cents = (midi - nearest) * 100.0;
    let n = nearest as i32;
    Some((format!("{}{}", names[n.rem_euclid(12) as usize], n.div_euclid(12) - 1), cents))
}
