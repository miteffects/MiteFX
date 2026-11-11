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

/// The wah: a resonant band-pass swept between a heel and a toe frequency, by the pedal's
/// position or (Auto) by the playing's envelope on top of it.
pub const WAH_MODES: [&str; 2] = ["Pedal", "Auto"];
pub const WAH: [BlockParam; 9] = [
    choice("mode", "Mode", &WAH_MODES, 0),
    knob(spec("position", "Pedal", "%", 0.0, 100.0, 50.0, LIN, 0, PCT, NUM)),
    knob(spec("sens", "Sensitivity", "%", 0.0, 100.0, 60.0, LIN, 0, PCT, NUM)),
    knob(spec("speed", "Speed", "ms", 20.0, 1000.0, 120.0, LOG, 0, LOGW, TIME)),
    knob(spec("q", "Q", "", 1.0, 12.0, 5.0, LOG, 1, LOGW, NUM)),
    knob(spec("low", "Heel", "Hz", 200.0, 800.0, 400.0, LOG, 0, LOGW, ParamDisplay::Frequency)),
    knob(spec("high", "Toe", "Hz", 1000.0, 4000.0, 2200.0, LOG, 0, LOGW, ParamDisplay::Frequency)),
    knob(spec("mix", "Mix", "%", 0.0, 100.0, 100.0, LIN, 0, PCT, NUM)),
    knob(spec("level", "Level", "dB", -12.0, 12.0, 0.0, LIN, 1, DB, NUM)),
];

/// The octaver: an octave and two below, one above, and a free interval, each with its level
/// (−60 dB: off), over the dry signal. Poly shifts the pitch (chords too); Mono is the classic
/// analog way, tight on single notes: a flip-flop on the zero crossings flips the signal's polarity
/// (an octave down, and its own flip-flop two), a full-wave rectifier doubles it (up).
pub const OCTAVER_MODES: [&str; 2] = ["Poly", "Mono"];
pub const OCTAVER: [BlockParam; 8] = [
    choice("mode", "Mode", &OCTAVER_MODES, 0),
    knob(spec("dry", "Dry", "dB", -60.0, 6.0, 0.0, LIN, 1, DB, NUM)),
    knob(spec("sub1", "−1 Oct", "dB", -60.0, 6.0, -3.0, LIN, 1, DB, NUM)),
    knob(spec("sub2", "−2 Oct", "dB", -60.0, 6.0, -60.0, LIN, 1, DB, NUM)),
    knob(spec("up", "+1 Oct", "dB", -60.0, 6.0, -60.0, LIN, 1, DB, NUM)),
    knob(spec("shift", "Pitch", "st", -24.0, 24.0, 7.0, LIN, 0, (1.0, 1.0), NUM)),
    knob(spec("shift_level", "Pitch Level", "dB", -60.0, 6.0, -60.0, LIN, 1, DB, NUM)),
    knob(spec("tone", "Tone", "Hz", 500.0, 12_000.0, 5000.0, LOG, 0, LOGW, ParamDisplay::Frequency)),
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

// ---- Wah ----------------------------------------------------------------------------------------

pub struct WahUnit {
    fs: f32,
    position: LinearSmoother,
    sens: LinearSmoother,
    q: LinearSmoother,
    low: LinearSmoother,
    high: LinearSmoother,
    mix: LinearSmoother,
    level: LinearSmoother,
    /// Auto's envelope (linear peak), and its attack.
    env: f32,
    attack: f32,
    /// The filter's two states per channel (a TPT state-variable filter: stable when swept).
    s: [[f32; 2]; 2],
}

impl WahUnit {
    pub fn new(fs: f32) -> Self {
        let sm = |ms: f32| smoother(fs, ms);
        Self { fs, position: sm(20.0), sens: sm(20.0), q: sm(20.0), low: sm(20.0), high: sm(20.0), mix: sm(20.0), level: sm(20.0), env: 0.0, attack: 1.0 - (-1.0 / (0.004 * fs)).exp(), s: [[0.0; 2]; 2] }
    }

    fn targets(&mut self, live: &Live) -> [f32; 7] {
        [live.value(1) / 100.0, if index(live, 0) == 1 { live.value(2) / 100.0 } else { 0.0 }, live.value(4), live.value(5), live.value(6), live.value(7) / 100.0, db_to_gain(live.value(8))]
    }
}

impl Unit for WahUnit {
    fn process(&mut self, l: &mut [f32], r: &mut [f32], live: &Live) {
        let t = self.targets(live);
        for (sm, v) in [&mut self.position, &mut self.sens, &mut self.q, &mut self.low, &mut self.high, &mut self.mix, &mut self.level].into_iter().zip(t) {
            sm.set_target(v);
        }
        let release = 1.0 - (-1.0 / (live.value(3).max(1.0) * 0.001 * self.fs)).exp();
        for i in 0..l.len() {
            let (pos, sens, q, low, high, mix, level) = (self.position.next_value(), self.sens.next_value(), self.q.next_value(), self.low.next_value(), self.high.next_value(), self.mix.next_value(), self.level.next_value());
            // Auto: the envelope (−48…−6 dBFS → 0…1) pushes the pedal towards the toe.
            let x = 0.5 * (l[i].abs() + r[i].abs());
            self.env += (if x > self.env { self.attack } else { release }) * (x - self.env);
            let e = ((20.0 * self.env.max(1e-6).log10() + 48.0) / 42.0).clamp(0.0, 1.0);
            let p = (pos + sens * e).clamp(0.0, 1.0);
            let f = (low * (high / low).powf(p)).min(0.45 * self.fs);
            let g = (std::f32::consts::PI * f / self.fs).tan();
            let k = 1.0 / q;
            let h = 1.0 / (1.0 + k * g + g * g);
            for (c, v) in [&mut l[i], &mut r[i]].into_iter().enumerate() {
                let [s1, s2] = &mut self.s[c];
                let hp = (*v - (k + g) * *s1 - *s2) * h;
                let bp = g * hp + *s1;
                *s1 = g * hp + bp;
                let lp = g * bp + *s2;
                *s2 = g * bp + lp;
                // Unity at the peak, with a little of the low end a real wah lets through.
                let wet = k * bp * 1.6 + 0.12 * lp;
                *v = (*v + mix * (wet - *v)) * level;
            }
        }
    }

    fn reset(&mut self, live: &Live) {
        let t = self.targets(live);
        for (sm, v) in [&mut self.position, &mut self.sens, &mut self.q, &mut self.low, &mut self.high, &mut self.mix, &mut self.level].into_iter().zip(t) {
            sm.reset(v);
        }
        self.env = 0.0;
        self.s = [[0.0; 2]; 2];
    }
}

// ---- Octaver -------------------------------------------------------------------------------------

/// A level knob's gain (−60 dB and below: none).
fn level_gain(db: f32) -> f32 {
    if db <= -59.9 { 0.0 } else { db_to_gain(db) }
}

/// A pitch shifter that splices where the signal repeats: one head reads a delay line at the
/// interval's speed; when its delay leaves its range it jumps by the lag (about one span, a whole
/// number of the note's periods) whose stretch of signal matches best, crossfading 10 ms. Clean
/// on single notes, fair on chords (the two-grain shifter cancelled itself on held notes).
pub struct Splicer {
    buf: Vec<f32>,
    w: usize,
    delay: f32,
    /// The head fading out after a jump, and the fade's progress (1: done).
    old: f32,
    fade: f32,
    fade_step: f32,
}

const SPLICE_MIN: f32 = 256.0;
const SPLICE_MAX: f32 = 2048.0;
/// A jump's lag is searched between these (samples).
const LAG: (usize, usize) = (1024, 1792);
/// The stretch compared at each lag.
const MATCH: usize = 256;

impl Splicer {
    pub fn new(fs: f32) -> Self {
        Self { buf: vec![0.0; 4096], w: 0, delay: 1024.0, old: 0.0, fade: 1.0, fade_step: 1.0 / (0.01 * fs).max(1.0) }
    }

    pub fn reset(&mut self) {
        self.buf.fill(0.0);
        self.delay = 1024.0;
        self.fade = 1.0;
    }

    fn read(&self, d: f32) -> f32 {
        let mask = self.buf.len() - 1;
        let p = self.w as f32 - d;
        let i = p.floor();
        let f = p - i;
        let i = i as isize as usize;
        let a = self.buf[i & mask];
        let b = self.buf[i.wrapping_add(1) & mask];
        a + f * (b - a)
    }

    /// The lag (`LAG`) whose signal matches the head's best: the head moves by it at a jump.
    fn best_lag(&self, sign: f32) -> f32 {
        let mask = self.buf.len() - 1;
        let at = |d: usize| self.buf[(self.w.wrapping_sub(d)) & mask];
        let d0 = self.delay.round() as usize;
        let score = |lag: usize| {
            let d1 = if sign < 0.0 { d0 - lag } else { d0 + lag };
            let (mut r, mut e) = (0.0f32, 1e-9f32);
            for k in 0..MATCH {
                let (a, b) = (at(d0 - k.min(d0)), at(d1 - k.min(d1)));
                r += a * b;
                e += b * b;
            }
            r / e.sqrt()
        };
        // Every other lag, then the neighbours of the best.
        let mut best = (LAG.0, f32::MIN);
        for lag in (LAG.0..=LAG.1).step_by(2) {
            let v = score(lag);
            if v > best.1 {
                best = (lag, v);
            }
        }
        for lag in [best.0.saturating_sub(1), best.0 + 1] {
            let v = score(lag);
            if v > best.1 {
                best = (lag, v);
            }
        }
        best.0 as f32
    }

    /// One sample in, the sample shifted by `ratio` (0.5: an octave down) out.
    pub fn process(&mut self, x: f32, ratio: f32) -> f32 {
        let mask = self.buf.len() - 1;
        self.buf[self.w & mask] = x;
        self.w = self.w.wrapping_add(1);
        let step = 1.0 - ratio;
        self.delay += step;
        self.old += step;
        if self.fade >= 1.0 {
            if self.delay > SPLICE_MAX {
                self.old = self.delay;
                self.delay -= self.best_lag(-1.0);
                self.fade = 0.0;
            } else if self.delay < SPLICE_MIN {
                self.old = self.delay;
                self.delay += self.best_lag(1.0);
                self.fade = 0.0;
            }
        }
        let y = self.read(self.delay.clamp(1.0, (self.buf.len() - 2) as f32));
        if self.fade < 1.0 {
            let o = self.read(self.old.clamp(1.0, (self.buf.len() - 2) as f32));
            let t = self.fade;
            self.fade = (self.fade + self.fade_step).min(1.0);
            return o + t * (y - o);
        }
        y
    }
}

pub struct OctaverUnit {
    fs: f32,
    /// Poly: −1, −2, +1 octave and the free interval (mono in: a guitar).
    shifters: [Splicer; 4],
    /// The voices' and the dry levels, the interval, the tone.
    gains: [LinearSmoother; 5],
    ratio: LinearSmoother,
    tone: LinearSmoother,
    /// Mono: the tracking filter's two states, the flip-flops, the last sign, the envelope, the
    /// rectifier's DC blocker.
    track: [f32; 2],
    flip: [f32; 2],
    above: bool,
    env: f32,
    dc: [f32; 2],
    /// The wet's tone (a two-pole low-pass).
    lp: [f32; 2],
    mode: usize,
}

impl OctaverUnit {
    pub fn new(fs: f32) -> Self {
        Self {
            fs,
            shifters: std::array::from_fn(|_| Splicer::new(fs)),
            gains: std::array::from_fn(|_| smoother(fs, 20.0)),
            ratio: smoother(fs, 20.0),
            tone: smoother(fs, 20.0),
            track: [0.0; 2],
            flip: [1.0; 2],
            above: false,
            env: 0.0,
            dc: [0.0; 2],
            lp: [0.0; 2],
            mode: 0,
        }
    }

    fn targets(live: &Live) -> ([f32; 5], f32, f32) {
        ([level_gain(live.value(2)), level_gain(live.value(3)), level_gain(live.value(4)), level_gain(live.value(6)), level_gain(live.value(1))], (live.value(5).round() / 12.0).exp2(), live.value(7))
    }
}

impl Unit for OctaverUnit {
    fn process(&mut self, l: &mut [f32], r: &mut [f32], live: &Live) {
        let (g, ratio, tone) = Self::targets(live);
        for (sm, v) in self.gains.iter_mut().zip(g) {
            sm.set_target(v);
        }
        self.ratio.set_target(ratio);
        self.tone.set_target(tone);
        let mode = index(live, 0);
        if mode != self.mode {
            self.mode = mode;
            self.shifters.iter_mut().for_each(Splicer::reset);
        }
        let fs = self.fs;
        let track_a = 1.0 - (-std::f32::consts::TAU * 700.0 / fs).exp();
        let env_a = 1.0 - (-1.0 / (0.01 * fs)).exp();
        let dc_a = 1.0 - (-std::f32::consts::TAU * 30.0 / fs).exp();
        for i in 0..l.len() {
            let x = 0.5 * (l[i] + r[i]);
            let [g1, g2, gu, gp, gd] = [0, 1, 2, 3, 4].map(|k| self.gains[k].next_value());
            let ratio = self.ratio.next_value();
            let tone_a = 1.0 - (-std::f32::consts::TAU * self.tone.next_value() / fs).exp();
            let (sub1, sub2, up) = if self.mode == 1 {
                // The tracking filter keeps the fundamental; its zero crossings flip the flops.
                self.track[0] += track_a * (x - self.track[0]);
                self.track[1] += track_a * (self.track[0] - self.track[1]);
                let t = self.track[1];
                self.env += env_a * (t.abs() - self.env);
                let hyst = 0.05 * self.env;
                if !self.above && t > hyst {
                    self.above = true;
                    self.flip[0] = -self.flip[0];
                    if self.flip[0] > 0.0 {
                        self.flip[1] = -self.flip[1];
                    }
                } else if self.above && t < -hyst {
                    self.above = false;
                }
                let rect = t.abs();
                self.dc[0] += dc_a * (rect - self.dc[0]);
                (t * self.flip[0], t * self.flip[1], (rect - self.dc[0]) * 1.4)
            } else {
                let [a, b, c, _] = &mut self.shifters;
                (a.process(x, 0.5), b.process(x, 0.25), c.process(x, 2.0))
            };
            let shifted = if gp > 0.0 || self.gains[3].is_smoothing() { self.shifters[3].process(x, ratio) } else { 0.0 };
            let wet = g1 * sub1 + g2 * sub2 + gu * up + gp * shifted;
            self.lp[0] += tone_a * (wet - self.lp[0]);
            self.lp[1] += tone_a * (self.lp[0] - self.lp[1]);
            let w = self.lp[1];
            l[i] = gd * l[i] + w;
            r[i] = gd * r[i] + w;
        }
    }

    fn reset(&mut self, live: &Live) {
        let (g, ratio, tone) = Self::targets(live);
        for (sm, v) in self.gains.iter_mut().zip(g) {
            sm.reset(v);
        }
        self.ratio.reset(ratio);
        self.tone.reset(tone);
        self.shifters.iter_mut().for_each(Splicer::reset);
        self.mode = index(live, 0);
        self.track = [0.0; 2];
        self.flip = [1.0; 2];
        self.above = false;
        self.env = 0.0;
        self.dc = [0.0; 2];
        self.lp = [0.0; 2];
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

#[cfg(test)]
mod wah_tests {
    use super::*;
    use crate::blocks::{build, Kind};

    /// Energy of a block's output for a tone at `hz`, with the pedal at `pos` (0…100).
    fn level(hz: f32, pos: f32) -> f32 {
        let fs = 48_000.0;
        let live = Live::new(1, Kind::Wah);
        live.set(1, pos);
        let mut u = build(&live, fs);
        u.reset(&live);
        let mut e = 0.0;
        for k in 0..40 {
            let x: Vec<f32> = (0..256).map(|i| (std::f32::consts::TAU * hz * (k * 256 + i) as f32 / fs).sin() * 0.3).collect();
            let (mut l, mut r) = (x.clone(), x);
            u.process(&mut l, &mut r, &live);
            if k >= 20 {
                e += l.iter().map(|v| v * v).sum::<f32>();
            }
        }
        e
    }

    #[test]
    fn the_wah_sweeps_from_heel_to_toe() {
        // Heel down: the lows through, the highs cut; toe down: the other way round.
        assert!(level(400.0, 0.0) > 4.0 * level(400.0, 100.0));
        assert!(level(2200.0, 100.0) > 4.0 * level(2200.0, 0.0));
    }
}

#[cfg(test)]
mod octaver_tests {
    use super::*;
    use crate::blocks::{build, Kind};

    fn level(v: &[f32], f: f32, fs: f32) -> f32 {
        let (mut c, mut s) = (0.0f64, 0.0f64);
        for (i, &x) in v.iter().enumerate() {
            let w = std::f64::consts::TAU * f64::from(f) * i as f64 / f64::from(fs);
            c += f64::from(x) * w.cos();
            s += f64::from(x) * w.sin();
        }
        (2.0 * (c * c + s * s).sqrt() / v.len() as f64) as f32
    }

    /// Both modes put a tone an octave down, with the dry off.
    #[test]
    fn the_octaver_plays_an_octave_down() {
        let fs = 48_000.0;
        for mode in [0.0, 1.0] {
            let live = Live::new(1, Kind::Octaver);
            live.set(0, mode);
            live.set(1, -60.0);
            live.set(2, 0.0);
            live.set(7, 12_000.0);
            let mut u = build(&live, fs);
            u.reset(&live);
            let mut out = Vec::new();
            for k in 0..100 {
                let x: Vec<f32> = (0..480).map(|i| (std::f32::consts::TAU * 220.0 * (k * 480 + i) as f32 / fs).sin() * 0.5).collect();
                let (mut l, mut r) = (x.clone(), x);
                u.process(&mut l, &mut r, &live);
                out.extend_from_slice(&l);
            }
            let tail = &out[24_000..];
            let (sub, orig) = (level(tail, 110.0, fs), level(tail, 220.0, fs));
            // Poly keeps the note's level (0.5); Mono's flipped signal carries part of it.
            let want = if mode == 0.0 { 0.35 } else { 0.1 };
            assert!(sub > want && sub > 2.0 * orig, "mode {mode}: {sub} at 110 Hz, {orig} at 220 Hz");
        }
    }

    /// Poly's octave up and free interval land where they should, at about the note's level.
    #[test]
    fn poly_shifts_up_and_by_an_interval() {
        let fs = 48_000.0;
        for (param, value, want_hz) in [(4usize, 0.0f32, 440.0f32), (6, 0.0, 220.0 * 1.4983)] {
            let live = Live::new(1, Kind::Octaver);
            live.set(1, -60.0);
            live.set(2, -60.0);
            live.set(param, value);
            live.set(7, 12_000.0);
            let mut u = build(&live, fs);
            u.reset(&live);
            let mut out = Vec::new();
            for k in 0..100 {
                let x: Vec<f32> = (0..480).map(|i| (std::f32::consts::TAU * 220.0 * (k * 480 + i) as f32 / fs).sin() * 0.5).collect();
                let (mut l, mut r) = (x.clone(), x);
                u.process(&mut l, &mut r, &live);
                out.extend_from_slice(&l);
            }
            let tail = &out[24_000..];
            let (hit, orig) = (level(tail, want_hz, fs), level(tail, 220.0, fs));
            assert!(hit > 0.3 && orig < 0.05, "{want_hz} Hz: {hit}, {orig} left at 220");
        }
    }
}
