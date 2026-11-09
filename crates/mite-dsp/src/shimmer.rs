//! Mite Verb's shimmer: the classic recipe. Jon Dattorro's plate ("Effect Design, Part 1:
//! Reverberator and Other Filters", JAES 1997, figure 1, his delays and taps scaled from
//! 29 761 Hz) with a two-grain pitch shifter where each half of the tank crosses into the other.
//! Every pass round the tank moves the tail by the interval, and the tank's modulated
//! allpasses and damping smear and darken it on the way: what climbs is part of the plate, not a
//! copy laid on top.
//!
//! Before this (2026-11-09) the shifted tail went back through the room's 32 lines: a two-grain
//! shifter was "grainy", a bin-shifting phase vocoder "metallic" (a comb at half a frame), a
//! stretch-and-resample vocoder "still metallic". The user asked for the most popular design.
//!
//! Sized in [`ShimmerPlate::new`]; `process` never allocates.

/// Dattorro's rate: his delays are in samples at it.
const DATTORRO_FS: f32 = 29_761.0;
/// The input diffusers (samples at `DATTORRO_FS`) and their coefficients.
const IN_DIFF: [(f32, f32); 4] = [(142.0, 0.75), (107.0, 0.75), (379.0, 0.625), (277.0, 0.625)];
/// Each tank half: the modulated allpass, the first delay, the second allpass, the second delay.
const TANK: [[f32; 4]; 2] = [[672.0, 4453.0, 1800.0, 3720.0], [908.0, 4217.0, 2656.0, 3163.0]];
/// Decay diffusion 1 (the modulated allpasses) and 2.
const DECAY_DIFF: (f32, f32) = (0.70, 0.50);
/// The modulated allpasses' excursion (samples at `DATTORRO_FS`) and rate, Hz.
const EXCURSION: f32 = 16.0;
const MOD_HZ: f32 = 0.5;
/// The pitch shifters' grain windows per half, s (different, so their grains never line up).
const GRAIN_S: [f32; 2] = [0.093, 0.107];
/// The share of the power crossing between the halves that is shifted (the rest crosses as it
/// is): every pass all of it moved an octave a third of a second and climbed out of the top in
/// 1.5 s; a share lets the note ring on while part of it climbs (equal power: the grains and the
/// rest are uncorrelated).
const SHIFT_SHARE: f32 = 0.35;
/// The halves' two voices sit this far either side of the interval, cents.
const DETUNE_CENTS: f32 = 6.0;
/// The output taps (Dattorro's table 2): (node, delay at `DATTORRO_FS`, sign). Nodes: 0 left
/// first delay, 1 left second allpass, 2 left second delay, 3–5 the same on the right.
const TAPS: [[(usize, f32, f32); 7]; 2] = [
    [(3, 266.0, 1.0), (3, 2974.0, 1.0), (4, 1913.0, -1.0), (5, 1996.0, 1.0), (0, 1990.0, -1.0), (1, 187.0, -1.0), (2, 1066.0, -1.0)],
    [(0, 353.0, 1.0), (0, 3627.0, 1.0), (1, 1228.0, -1.0), (2, 2673.0, 1.0), (3, 2111.0, -1.0), (4, 335.0, -1.0), (5, 121.0, -1.0)],
];

/// A delay line read whole or fractional samples back.
struct Line {
    buf: Vec<f32>,
    pos: usize,
}

impl Line {
    fn new(len: usize) -> Self {
        Self { buf: vec![0.0; len.max(4) + 4], pos: 0 }
    }

    fn reset(&mut self) {
        self.buf.fill(0.0);
        self.pos = 0;
    }

    fn write(&mut self, x: f32) {
        self.pos = (self.pos + 1) % self.buf.len();
        self.buf[self.pos] = x;
    }

    /// `d` samples back (0: the last written).
    fn at(&self, d: usize) -> f32 {
        let n = self.buf.len();
        self.buf[(self.pos + n - d.min(n - 1)) % n]
    }

    fn frac(&self, d: f32) -> f32 {
        let i = d.floor();
        let t = d - i;
        let a = self.at(i as usize);
        a + (self.at(i as usize + 1) - a) * t
    }
}

/// A Schroeder allpass over a [`Line`] of `len` samples (modulated by reading it fractionally).
struct Allpass {
    line: Line,
    len: f32,
}

impl Allpass {
    fn new(len: f32, excursion: f32) -> Self {
        Self { line: Line::new((len + excursion).ceil() as usize + 2), len }
    }

    fn process(&mut self, x: f32, g: f32, offset: f32) -> f32 {
        let d = self.line.frac(self.len + offset - 1.0);
        let w = x + g * d;
        self.line.write(w);
        d - g * w
    }
}

/// A two-grain pitch shifter: two heads sweep a delay line at the rate the interval asks for and
/// crossfade (sin² and cos²) where each jumps back. Grainy alone; smooth inside the tank.
pub struct Shifter {
    line: Line,
    phase: f32,
    window: f32,
}

impl Shifter {
    pub fn new(fs: f32, window_s: f32) -> Self {
        let window = (window_s * fs).max(16.0);
        Self { line: Line::new(window as usize + 4), phase: 0.0, window }
    }

    fn reset(&mut self) {
        self.line.reset();
        self.phase = 0.0;
    }

    /// One sample in, the sample shifted by `ratio` (2: an octave up) out.
    pub fn process(&mut self, x: f32, ratio: f32) -> f32 {
        self.line.write(x);
        // The delay shrinks by (ratio − 1) a sample: read faster than written, the pitch rises.
        self.phase -= (ratio - 1.0) / self.window;
        self.phase -= self.phase.floor();
        let mut y = 0.0;
        for h in [self.phase, (self.phase + 0.5).fract()] {
            let w = (std::f32::consts::PI * h).sin();
            y += w * w * self.line.frac(h * self.window);
        }
        y
    }
}

/// What the plate does, from the editor's dials.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlateSettings {
    /// The tank's gain per half pass, 0…<1.
    pub decay: f32,
    /// The interval as a speed ratio.
    pub ratio: f32,
    /// The tank's damping: a low cut and a high cut, Hz.
    pub low_hz: f32,
    pub high_hz: f32,
}

pub struct ShimmerPlate {
    fs: f32,
    k: f32,
    diff: [Allpass; 4],
    /// Per half: the modulated allpass, the first delay, the second allpass, the second delay;
    /// the damping states (low cut, high cut); the shifter.
    ap1: [Allpass; 2],
    d1: [Line; 2],
    ap2: [Allpass; 2],
    d2: [Line; 2],
    damp: [[f32; 2]; 2],
    shift: [Shifter; 2],
    lfo: f32,
    /// Each half's last output, waiting to cross into the other.
    cross: [f32; 2],
}

impl ShimmerPlate {
    /// Allocates.
    pub fn new(fs: f32) -> Self {
        let k = fs / DATTORRO_FS;
        let exc = EXCURSION * k;
        Self {
            fs,
            k,
            diff: IN_DIFF.map(|(len, _)| Allpass::new(len * k, 0.0)),
            ap1: [Allpass::new(TANK[0][0] * k, exc), Allpass::new(TANK[1][0] * k, exc)],
            d1: [Line::new((TANK[0][1] * k) as usize + 2), Line::new((TANK[1][1] * k) as usize + 2)],
            ap2: [Allpass::new(TANK[0][2] * k, 0.0), Allpass::new(TANK[1][2] * k, 0.0)],
            d2: [Line::new((TANK[0][3] * k) as usize + 2), Line::new((TANK[1][3] * k) as usize + 2)],
            damp: [[0.0; 2]; 2],
            shift: [Shifter::new(fs, GRAIN_S[0]), Shifter::new(fs, GRAIN_S[1])],
            lfo: 0.0,
            cross: [0.0; 2],
        }
    }

    pub fn reset(&mut self) {
        for a in self.diff.iter_mut().chain(self.ap1.iter_mut()).chain(self.ap2.iter_mut()) {
            a.line.reset();
        }
        for l in self.d1.iter_mut().chain(self.d2.iter_mut()) {
            l.reset();
        }
        for s in &mut self.shift {
            s.reset();
        }
        self.damp = [[0.0; 2]; 2];
        self.lfo = 0.0;
        self.cross = [0.0; 2];
    }

    /// The decay per half pass for a T60 of `t60` s (a half pass is about 0.36 s at any rate).
    pub fn decay_for(t60: f32) -> f32 {
        let half = (TANK[0].iter().sum::<f32>() + TANK[1].iter().sum::<f32>()) / 2.0 / DATTORRO_FS;
        10f32.powf(-3.0 * half / t60.max(0.05)).min(0.98)
    }

    /// One sample of (mono) input in; the plate's left and right out.
    pub fn process(&mut self, x: f32, s: &PlateSettings) -> [f32; 2] {
        let k = self.k;
        let mut u = x;
        for (a, &(_, g)) in self.diff.iter_mut().zip(&IN_DIFF) {
            u = a.process(u, g, 0.0);
        }
        self.lfo = (self.lfo + MOD_HZ / self.fs).fract();
        let lfo = (std::f32::consts::TAU * self.lfo).sin();
        let lc = 1.0 - (-std::f32::consts::TAU * s.low_hz / self.fs).exp();
        let hc = 1.0 - (-std::f32::consts::TAU * s.high_hz.min(0.45 * self.fs) / self.fs).exp();
        let spread = (DETUNE_CENTS / 1200.0).exp2();
        let ratios = [s.ratio * spread, s.ratio / spread];
        let mut out_cross = [0.0f32; 2];
        for h in 0..2 {
            // This half takes the input and the other half's tail, shifted by the interval.
            let c = self.cross[1 - h];
            let from = (1.0 - SHIFT_SHARE).sqrt() * c + SHIFT_SHARE.sqrt() * self.shift[h].process(c, ratios[h]);
            let mut v = u + from;
            let offset = EXCURSION * k * if h == 0 { lfo } else { -lfo };
            v = self.ap1[h].process(v, -DECAY_DIFF.0, offset);
            self.d1[h].write(v);
            let mut t = self.d1[h].at((TANK[h][1] * k) as usize);
            // Damping: the high cut and the low cut, then the decay.
            let st = &mut self.damp[h];
            st[1] += (t - st[1]) * hc;
            t = st[1];
            st[0] += (t - st[0]) * lc;
            t = (t - st[0]) * s.decay;
            t = self.ap2[h].process(t, DECAY_DIFF.1, 0.0);
            self.d2[h].write(t);
            out_cross[h] = self.d2[h].at((TANK[h][3] * k) as usize) * s.decay;
        }
        self.cross = out_cross;
        let node = |n: usize, d: f32| -> f32 {
            let d = (d * k) as usize;
            match n {
                0 => self.d1[0].at(d),
                1 => self.ap2[0].line.at(d),
                2 => self.d2[0].at(d),
                3 => self.d1[1].at(d),
                4 => self.ap2[1].line.at(d),
                _ => self.d2[1].at(d),
            }
        };
        let mut y = [0.0f32; 2];
        for (side, taps) in TAPS.iter().enumerate() {
            y[side] = 0.6 * taps.iter().map(|&(n, d, sign)| sign * node(n, d)).sum::<f32>();
        }
        y
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn level(v: &[f32], f: f32, fs: f32) -> f32 {
        let (mut c, mut s) = (0.0f64, 0.0f64);
        for (i, &x) in v.iter().enumerate() {
            let w = std::f64::consts::TAU * f64::from(f) * i as f64 / f64::from(fs);
            c += f64::from(x) * w.cos();
            s += f64::from(x) * w.sin();
        }
        (2.0 * (c * c + s * s).sqrt() / v.len() as f64) as f32
    }

    #[test]
    fn the_shifter_moves_a_tone_by_the_interval() {
        let fs = 48_000.0;
        for (semis, want) in [(12.0f32, 880.0f32), (-12.0, 220.0), (7.0, 659.255)] {
            let mut sh = Shifter::new(fs, 0.1);
            let r = (semis / 12.0).exp2();
            let out: Vec<f32> = (0..48_000).map(|i| sh.process((std::f32::consts::TAU * 440.0 * i as f32 / fs).sin(), r)).collect();
            let tail = &out[9600..];
            let (hit, orig) = (level(tail, want, fs), level(tail, 440.0, fs));
            assert!(hit > 0.5 && orig < 0.05, "{semis}: {hit} at the interval, {orig} left");
        }
    }

    #[test]
    fn the_plate_decays_and_its_tail_climbs() {
        let fs = 48_000.0;
        let mut p = ShimmerPlate::new(fs);
        let s = PlateSettings { decay: ShimmerPlate::decay_for(6.0), ratio: 2.0, low_hz: 150.0, high_hz: 8000.0 };
        let n = 8 * 48_000;
        let out: Vec<f32> = (0..n)
            .map(|i| {
                let x = if i < 24_000 { 0.3 * (std::f32::consts::TAU * 440.0 * i as f32 / fs).sin() } else { 0.0 };
                p.process(x, &s)[0]
            })
            .collect();
        let win = |a: usize| &out[a * 48_000..(a + 1) * 48_000];
        // The octave above grows into the tail.
        assert!(level(win(2), 880.0, fs) > 0.5 * level(win(2), 440.0, fs), "{} vs {}", level(win(2), 880.0, fs), level(win(2), 440.0, fs));
        // And it all dies away.
        let rms = |v: &[f32]| (v.iter().map(|x| x * x).sum::<f32>() / v.len() as f32).sqrt();
        assert!(rms(win(7)) < 0.1 * rms(win(1)), "{} → {}", rms(win(1)), rms(win(7)));
    }
}
