//! The parts of Mite Verb's Plate and Shimmer types: Jon Dattorro's plate constants ("Effect
//! Design, Part 1: Reverberator and Other Filters", JAES 1997, figure 1, his delays and taps at
//! 29 761 Hz), delay lines, allpasses, and the two-grain pitch shifter the Shimmer type puts where
//! the tank's halves cross (`crate::reverb::PlateTank`): every pass round the tank moves part of
//! the tail by the interval, and the tank's modulated allpasses smear it on the way.
//!
//! Before this (2026-11-09) the shifted tail went back through the room's 32 lines: a two-grain
//! shifter was "grainy", a bin-shifting phase vocoder "metallic" (a comb at half a frame), a
//! stretch-and-resample vocoder "still metallic". The user asked for the most popular design.
//!
//! Everything is sized when made; processing never allocates.

/// Dattorro's rate: his delays are in samples at it.
pub(crate) const DATTORRO_FS: f32 = 29_761.0;
/// The input diffusers (samples at `DATTORRO_FS`) and their coefficients.
pub(crate) const IN_DIFF: [(f32, f32); 4] = [(142.0, 0.75), (107.0, 0.75), (379.0, 0.625), (277.0, 0.625)];
/// Each tank half: the modulated allpass, the first delay, the second allpass, the second delay.
pub(crate) const TANK: [[f32; 4]; 2] = [[672.0, 4453.0, 1800.0, 3720.0], [908.0, 4217.0, 2656.0, 3163.0]];
/// Decay diffusion 1 (the modulated allpasses) and 2.
pub(crate) const DECAY_DIFF: (f32, f32) = (0.70, 0.50);
/// The modulated allpasses' excursion (samples at `DATTORRO_FS`).
pub(crate) const EXCURSION: f32 = 16.0;
/// The output taps (Dattorro's table 2): (node, delay at `DATTORRO_FS`, sign). Nodes: 0 left
/// first delay, 1 left second allpass, 2 left second delay, 3–5 the same on the right.
pub(crate) const TAPS: [[(usize, f32, f32); 7]; 2] = [
    [(3, 266.0, 1.0), (3, 2974.0, 1.0), (4, 1913.0, -1.0), (5, 1996.0, 1.0), (0, 1990.0, -1.0), (1, 187.0, -1.0), (2, 1066.0, -1.0)],
    [(0, 353.0, 1.0), (0, 3627.0, 1.0), (1, 1228.0, -1.0), (2, 2673.0, 1.0), (3, 2111.0, -1.0), (4, 335.0, -1.0), (5, 121.0, -1.0)],
];

/// A delay line read whole or fractional samples back.
pub(crate) struct Line {
    buf: Vec<f32>,
    pos: usize,
}

impl Line {
    pub(crate) fn new(len: usize) -> Self {
        Self { buf: vec![0.0; len.max(4) + 4], pos: 0 }
    }

    pub(crate) fn reset(&mut self) {
        self.buf.fill(0.0);
        self.pos = 0;
    }

    pub(crate) fn write(&mut self, x: f32) {
        self.pos = (self.pos + 1) % self.buf.len();
        self.buf[self.pos] = x;
    }

    /// `d` samples back (0: the last written).
    pub(crate) fn at(&self, d: usize) -> f32 {
        let n = self.buf.len();
        self.buf[(self.pos + n - d.min(n - 1)) % n]
    }

    pub(crate) fn frac(&self, d: f32) -> f32 {
        let i = d.floor();
        let t = d - i;
        let a = self.at(i as usize);
        a + (self.at(i as usize + 1) - a) * t
    }
}

/// A Schroeder allpass over a [`Line`] of `len` samples (modulated by reading it fractionally).
pub(crate) struct Allpass {
    pub(crate) line: Line,
    /// Its length, samples (settable up to what it was made for).
    pub(crate) len: f32,
}

impl Allpass {
    pub(crate) fn new(len: f32, excursion: f32) -> Self {
        Self { line: Line::new((len + excursion).ceil() as usize + 2), len }
    }

    pub(crate) fn process(&mut self, x: f32, g: f32, offset: f32) -> f32 {
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

    pub fn reset(&mut self) {
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
}
