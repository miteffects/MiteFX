//! The suite's own effects as pedals on the board (docs/design/rig-brief.md §2, slice 2): each a
//! few main knobs over the engine its plugin runs (Mite Comp's compressor, Mite Delay's lines,
//! Mite Verb's room, Mite Mod's modulations, Mite Double's doubler, Mite Tuner's detector), the
//! rest at the plugin's defaults. Plus a graphic EQ and a tremolo of their own.

use mite_core::{ParamDisplay, ParamScale, ParamSpec};
use mite_dsp::delay::{DelayLine, DelaySettings, Mode as DelayMode};
use mite_dsp::doubler::{Doubler, DoublerSettings};
use mite_dsp::dynamics::{CompSettings, Compressor};
use mite_dsp::eq::{BandProcessor, BandSettings, BandType};
use mite_dsp::modulation::{Mod, ModSettings, Mode as ModMode, Transport};
use mite_dsp::pitch::PitchDetector;
use mite_dsp::reverb::{Reverb, ReverbKind, ReverbSettings};
use mite_dsp::{db_to_gain, LinearSmoother};

use crate::blocks::{BlockParam, Unit};
use crate::board::Live;
use crate::parts::SUB;

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

pub const COMP: [BlockParam; 5] = [
    knob(spec("threshold", "Threshold", "dB", -60.0, 0.0, -24.0, LIN, 1, DB, NUM)),
    knob(spec("ratio", "Ratio", ":1", 1.0, 20.0, 4.0, LOG, 1, LOGW, NUM)),
    knob(spec("attack", "Attack", "ms", 0.1, 100.0, 10.0, LOG, 1, LOGW, TIME)),
    knob(spec("release", "Release", "ms", 10.0, 1000.0, 120.0, LOG, 0, LOGW, TIME)),
    knob(spec("makeup", "Level", "dB", -12.0, 24.0, 0.0, LIN, 1, DB, NUM)),
];

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

pub const DELAY: [BlockParam; 6] = [
    choice("mode", "Type", &DelayMode::NAMES, 1),
    knob(spec("time", "Time", "ms", 20.0, 2000.0, 380.0, LOG, 0, LOGW, TIME)),
    knob(spec("feedback", "Feedback", "%", 0.0, 100.0, 35.0, LIN, 0, PCT, NUM)),
    knob(spec("mix", "Mix", "%", 0.0, 100.0, 30.0, LIN, 0, PCT, NUM)),
    knob(spec("tone", "Tone", "Hz", 1000.0, 20_000.0, 6000.0, LOG, 0, LOGW, ParamDisplay::Frequency)),
    switch("pingpong", "Ping-Pong", false),
];

pub const REVERB: [BlockParam; 5] = [
    choice("type", "Type", &ReverbKind::NAMES, 0),
    knob(spec("decay", "Decay", "s", 0.2, 12.0, 1.6, LOG, 2, LOGW, NUM)),
    knob(spec("size", "Size", "%", 0.0, 100.0, 50.0, LIN, 0, PCT, NUM)),
    knob(spec("predelay", "Pre-Delay", "ms", 0.0, 200.0, 10.0, LIN, 0, PCT, TIME)),
    knob(spec("mix", "Mix", "%", 0.0, 100.0, 25.0, LIN, 0, PCT, NUM)),
];

/// The modulations a Mod pedal offers (Mite Mod's, without Micro).
pub const MOD_NAMES: [&str; 6] = ["Chorus", "Ensemble", "Flanger", "Phaser", "Vibrato", "Rotary"];
pub const MOD: [BlockParam; 5] = [
    choice("mode", "Type", &MOD_NAMES, 0),
    knob(spec("rate", "Rate", "Hz", 0.05, 10.0, 0.8, LOG, 2, LOGW, NUM)),
    knob(spec("depth", "Depth", "%", 0.0, 100.0, 50.0, LIN, 0, PCT, NUM)),
    knob(spec("feedback", "Feedback", "%", -95.0, 95.0, 0.0, LIN, 0, PCT, NUM)),
    knob(spec("mix", "Mix", "%", 0.0, 100.0, 50.0, LIN, 0, PCT, NUM)),
];

pub const TREMOLO_NAMES: [&str; 3] = ["Sine", "Square", "Harmonic"];
pub const TREMOLO: [BlockParam; 4] = [
    choice("shape", "Shape", &TREMOLO_NAMES, 0),
    knob(spec("rate", "Rate", "Hz", 0.5, 15.0, 5.0, LOG, 2, LOGW, NUM)),
    knob(spec("depth", "Depth", "%", 0.0, 100.0, 60.0, LIN, 0, PCT, NUM)),
    knob(spec("spread", "Stereo", "%", 0.0, 100.0, 0.0, LIN, 0, PCT, NUM)),
];

pub const DOUBLE: [BlockParam; 4] = [
    knob(spec("amount", "Looseness", "%", 0.0, 100.0, 40.0, LIN, 0, PCT, NUM)),
    knob(spec("delay", "Delay", "ms", 5.0, 40.0, 14.0, LIN, 0, PCT, TIME)),
    knob(spec("width", "Width", "%", 0.0, 100.0, 100.0, LIN, 0, PCT, NUM)),
    knob(spec("level", "Doubles", "dB", -24.0, 6.0, -3.0, LIN, 1, DB, NUM)),
];

pub const TUNER: [BlockParam; 2] = [switch("mute", "Mute", false), knob(spec("reference", "A4", "Hz", 430.0, 450.0, 440.0, LIN, 1, (0.5, 0.1), NUM))];

fn smoother(fs: f32, v: f32) -> LinearSmoother {
    let mut s = LinearSmoother::new(v);
    s.set_ramp(fs, 20.0);
    s
}

fn index(live: &Live, i: usize) -> usize {
    live.value(i).round().max(0.0) as usize
}

// ---- Comp ---------------------------------------------------------------------------------------

pub struct CompUnit {
    comp: Compressor,
}

impl CompUnit {
    pub fn new(fs: f32) -> Self {
        let mut comp = Compressor::new(fs);
        comp.prepare(fs, &Self::settings_of(&[-24.0, 4.0, 10.0, 120.0, 0.0]));
        Self { comp }
    }

    fn settings_of(v: &[f32; 5]) -> CompSettings {
        let mut s = CompSettings::default();
        s.stages[0].on = true;
        s.stages[0].threshold_db = v[0];
        s.stages[0].ratio = v[1];
        s.stages[0].attack_ms = v[2];
        s.stages[0].release_ms = v[3];
        s.makeup_db = v[4];
        s
    }

    fn settings(live: &Live) -> CompSettings {
        Self::settings_of(&std::array::from_fn(|i| live.value(i)))
    }
}

impl Unit for CompUnit {
    fn process(&mut self, l: &mut [f32], r: &mut [f32], live: &Live) {
        let s = Self::settings(live);
        let t = self.comp.process(&mut [l, r], None, &s);
        live.readout[0].store(t.cut_db[0].min(t.cut_db[1]));
    }

    fn reset(&mut self, live: &Live) {
        self.comp.reset(&Self::settings(live));
    }
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

// ---- Delay, Reverb: wet engines, mixed here --------------------------------------------------

fn mix_in(l: &mut [f32], r: &mut [f32], wet: &[Vec<f32>; 2], mix: &mut LinearSmoother) {
    for i in 0..l.len() {
        let m = mix.next_value();
        // Equal-power-ish: the dry stays whole up to half, then gives way.
        let dry = (2.0 * (1.0 - m)).min(1.0);
        let w = (2.0 * m).min(1.0);
        l[i] = l[i] * dry + wet[0][i] * w;
        r[i] = r[i] * dry + wet[1][i] * w;
    }
}

pub struct DelayUnit {
    line: DelayLine,
    wet: [Vec<f32>; 2],
    mix: LinearSmoother,
}

impl DelayUnit {
    pub fn new(fs: f32) -> Self {
        Self { line: DelayLine::new(fs, &DelaySettings::default()), wet: [vec![0.0; SUB], vec![0.0; SUB]], mix: smoother(fs, 0.3) }
    }

    fn settings(live: &Live) -> DelaySettings {
        let mut s = DelaySettings { mode: DelayMode::from_index(index(live, 0)), ..DelaySettings::default() };
        let t = &mut s.taps[0];
        t.on = true;
        t.time_ms = live.value(1);
        t.feedback = live.value(2) / 100.0;
        t.pingpong = live.value(5) >= 0.5;
        // The loop's tone: a high cut in the loop (the first loop band).
        s.eq[0] = BandSettings { enabled: live.value(4) < 19_500.0, kind: BandType::HighCut, freq: f64::from(live.value(4)), slope: 1, ..BandSettings::default() };
        s
    }
}

impl Unit for DelayUnit {
    fn process(&mut self, l: &mut [f32], r: &mut [f32], live: &Live) {
        let n = l.len();
        let s = Self::settings(live);
        let [wl, wr] = &mut self.wet;
        self.line.process([l, r], [&mut wl[..n], &mut wr[..n]], true, &s);
        self.mix.set_target(live.value(3) / 100.0);
        mix_in(l, r, &self.wet, &mut self.mix);
    }

    fn reset(&mut self, live: &Live) {
        self.line.reset(&Self::settings(live));
        self.mix.reset(live.value(3) / 100.0);
    }
}

pub struct ReverbUnit {
    verb: Reverb,
    wet: [Vec<f32>; 2],
    mix: LinearSmoother,
}

impl ReverbUnit {
    pub fn new(fs: f32) -> Self {
        Self { verb: Reverb::new(fs, &ReverbSettings::default()), wet: [vec![0.0; SUB], vec![0.0; SUB]], mix: smoother(fs, 0.25) }
    }

    fn settings(live: &Live) -> ReverbSettings {
        ReverbSettings { kind: ReverbKind::from_index(index(live, 0)), decay_s: live.value(1), size: live.value(2) / 100.0, predelay_ms: live.value(3), ..ReverbSettings::default() }
    }
}

impl Unit for ReverbUnit {
    fn process(&mut self, l: &mut [f32], r: &mut [f32], live: &Live) {
        let n = l.len();
        let s = Self::settings(live);
        let [wl, wr] = &mut self.wet;
        self.verb.process([l, r], [&mut wl[..n], &mut wr[..n]], true, &s);
        self.mix.set_target(live.value(4) / 100.0);
        mix_in(l, r, &self.wet, &mut self.mix);
    }

    fn reset(&mut self, live: &Live) {
        self.verb.reset(&Self::settings(live));
        self.mix.reset(live.value(4) / 100.0);
    }
}

// ---- Mod, Double: engines that mix themselves ---------------------------------------------------

pub struct ModUnit {
    m: Mod,
    out: [Vec<f32>; 2],
}

impl ModUnit {
    pub fn new(fs: f32) -> Self {
        Self { m: Mod::new(fs, &ModSettings::default()), out: [vec![0.0; SUB], vec![0.0; SUB]] }
    }

    fn settings(live: &Live) -> ModSettings {
        ModSettings { mode: ModMode::from_index(index(live, 0)), rate_hz: live.value(1), depth: live.value(2) / 100.0, feedback: live.value(3) / 100.0, mix: live.value(4) / 100.0, ..ModSettings::default() }
    }
}

impl Unit for ModUnit {
    fn process(&mut self, l: &mut [f32], r: &mut [f32], live: &Live) {
        let n = l.len();
        let transport = Transport { tempo: None, beats: None, playing: false };
        let [ol, or] = &mut self.out;
        self.m.process([l, r], [&mut ol[..n], &mut or[..n]], true, &Self::settings(live), &transport);
        l.copy_from_slice(&ol[..n]);
        r.copy_from_slice(&or[..n]);
    }

    fn reset(&mut self, live: &Live) {
        self.m.reset(&Self::settings(live));
    }
}

pub struct DoubleUnit {
    d: Doubler,
    out: [Vec<f32>; 2],
}

impl DoubleUnit {
    pub fn new(fs: f32) -> Self {
        Self { d: Doubler::new(fs), out: [vec![0.0; SUB], vec![0.0; SUB]] }
    }

    fn settings(live: &Live) -> DoublerSettings {
        let amount = live.value(0) / 100.0;
        let width = live.value(2) / 100.0;
        let mut s = DoublerSettings { voices: 2, delay_ms: live.value(1), timing_ms: 2.0 + 18.0 * amount, pitch_cents: 2.0 + 14.0 * amount, ..DoublerSettings::default() };
        for (i, v) in s.voice.iter_mut().enumerate().take(2) {
            v.pan = if i == 0 { -width } else { width };
            v.level_db = live.value(3);
        }
        s
    }
}

impl Unit for DoubleUnit {
    fn process(&mut self, l: &mut [f32], r: &mut [f32], live: &Live) {
        let n = l.len();
        let [ol, or] = &mut self.out;
        self.d.process([l, r], [&mut ol[..n], &mut or[..n]], true, &Self::settings(live));
        l.copy_from_slice(&ol[..n]);
        r.copy_from_slice(&or[..n]);
    }

    fn reset(&mut self, _live: &Live) {
        self.d.reset();
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

// ---- Tuner --------------------------------------------------------------------------------------

/// Samples the tuner listens to (about 85 ms at 48 kHz: low E and below).
const TUNER_WINDOW: usize = 4096;
/// It looks again this often.
const TUNER_HOP: usize = 1024;

pub struct TunerUnit {
    fs: f64,
    detector: PitchDetector,
    ring: Vec<f32>,
    window: Vec<f32>,
    pos: usize,
    since: usize,
    mute: LinearSmoother,
}

impl TunerUnit {
    pub fn new(fs: f32) -> Self {
        Self { fs: f64::from(fs), detector: PitchDetector::new(TUNER_WINDOW), ring: vec![0.0; TUNER_WINDOW], window: vec![0.0; TUNER_WINDOW], pos: 0, since: 0, mute: smoother(fs, 0.0) }
    }
}

impl Unit for TunerUnit {
    fn process(&mut self, l: &mut [f32], r: &mut [f32], live: &Live) {
        let n = l.len();
        for i in 0..n {
            self.ring[self.pos] = 0.5 * (l[i] + r[i]);
            self.pos = (self.pos + 1) % TUNER_WINDOW;
        }
        self.since += n;
        if self.since >= TUNER_HOP {
            self.since = 0;
            let (a, b) = self.ring.split_at(self.pos);
            self.window[..b.len()].copy_from_slice(b);
            self.window[b.len()..].copy_from_slice(a);
            // Readouts: the pitch heard (Hz, 0: none) and how clearly.
            match self.detector.detect(&self.window, self.fs) {
                Some(p) if p.clarity > 0.8 && p.level_db > -60.0 => {
                    live.readout[0].store(p.hz as f32);
                    live.readout[1].store(p.clarity as f32);
                }
                _ => live.readout[0].store(0.0),
            }
        }
        // Muted while tuning, if asked.
        self.mute.set_target(if live.value(0) >= 0.5 { 0.0 } else { 1.0 });
        for i in 0..n {
            let g = self.mute.next_value();
            l[i] *= g;
            r[i] *= g;
        }
    }

    fn reset(&mut self, live: &Live) {
        self.ring.fill(0.0);
        self.mute.reset(if live.value(0) >= 0.5 { 0.0 } else { 1.0 });
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
