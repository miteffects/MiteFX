//! A pitch shifter for a reverb's shimmer: two read heads sweep a delay line at the rate the
//! interval asks for and crossfade (sin² and cos², summing to one) where each one jumps back;
//! cubic (Hermite) reads. Grainy on dry material, smooth on a dense reverb tail, where it runs.
//!
//! Sized in [`Shifter::new`]; `process` never allocates.

pub struct Shifter {
    buf: Vec<f32>,
    pos: usize,
    /// The heads' place in the window, 0…1 (the second half a window on).
    phase: f32,
    window: f32,
}

impl Shifter {
    /// `window_s`: the grain window, s. Allocates.
    pub fn new(fs: f32, window_s: f32) -> Self {
        let window = (window_s * fs).max(16.0);
        Self { buf: vec![0.0; window as usize + 8], pos: 0, phase: 0.0, window }
    }

    pub fn reset(&mut self) {
        self.buf.fill(0.0);
        self.pos = 0;
        self.phase = 0.0;
    }

    /// The sample `delay` samples back (≥ 1), Hermite between its neighbours.
    fn read(&self, delay: f32) -> f32 {
        let n = self.buf.len();
        let d = delay.max(1.0);
        let i = d as usize;
        let t = d - i as f32;
        let at = |k: usize| self.buf[(self.pos + 2 * n - k) % n];
        // Newer to older: x₋₁ is one sample later than x₀.
        let (xm1, x0, x1, x2) = (at(i - 1), at(i), at(i + 1), at(i + 2));
        let c1 = 0.5 * (x1 - xm1);
        let c2 = xm1 - 2.5 * x0 + 2.0 * x1 - 0.5 * x2;
        let c3 = 0.5 * (x2 - xm1) + 1.5 * (x0 - x1);
        ((c3 * t + c2) * t + c1) * t + x0
    }

    /// One sample in, the sample shifted by `ratio` (2: an octave up) out.
    pub fn process(&mut self, x: f32, ratio: f32) -> f32 {
        let n = self.buf.len();
        self.pos = (self.pos + 1) % n;
        self.buf[self.pos] = x;
        // The delay shrinks by (ratio − 1) a sample: read faster than written, the pitch rises.
        self.phase -= (ratio - 1.0) / self.window;
        self.phase -= self.phase.floor();
        let mut y = 0.0;
        for h in [self.phase, (self.phase + 0.5).fract()] {
            let w = (std::f32::consts::PI * h).sin();
            y += w * w * self.read(1.0 + h * self.window);
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
    fn it_moves_a_tone_by_the_interval() {
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
