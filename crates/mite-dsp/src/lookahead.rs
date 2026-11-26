//! Lookahead at a constant latency (2026-11-26, user: a lookahead knob "was doing some delays").
//!
//! A dynamics processor with lookahead delays the audio and lets its detector hear the input
//! early. Delaying the audio by the lookahead itself made the latency follow the knob: every turn
//! dipped the sound and made the host realign every track mid-song. Instead, whenever lookahead
//! is on, the audio is delayed by the longest lookahead (the latency, constant) and the detector's
//! key by the rest: [`KeyDelay`]. The knob then moves only the key's delay (gliding, so the
//! detector never jumps); 0 is no latency at all.

/// The latency for a lookahead of `look` samples whose longest is `max`: the longest whenever
/// it is on, else none.
pub fn constant_latency(look: usize, max: usize) -> usize {
    if look > 0 { max } else { 0 }
}

/// The detector's key, delayed by the latency less the lookahead (one or two channels). Its delay
/// glides at most half a sample per sample to a new length. Allocation-free after `new`.
#[derive(Debug, Clone)]
pub struct KeyDelay {
    lines: [Vec<f32>; 2],
    write: usize,
    now: f64,
    target: usize,
}

impl KeyDelay {
    /// Holds up to `max` samples of delay. Allocates.
    pub fn new(max: usize) -> Self {
        let len = max + 2;
        Self { lines: [vec![0.0; len], vec![0.0; len]], write: 0, now: 0.0, target: 0 }
    }

    /// Cleared, at `delay` samples at once.
    pub fn reset(&mut self, delay: usize) {
        for l in &mut self.lines {
            l.fill(0.0);
        }
        self.write = 0;
        self.target = delay.min(self.lines[0].len() - 2);
        self.now = self.target as f64;
    }

    /// The delay to glide to, samples.
    pub fn set(&mut self, delay: usize) {
        self.target = delay.min(self.lines[0].len() - 2);
    }

    /// The delay now (gliding), samples.
    pub fn delay(&self) -> f64 {
        self.now
    }

    /// Channel `c`'s key sample `x` in, its delayed key out (exact on a whole-sample delay). Call
    /// for each channel, then [`Self::advance`] once per sample.
    #[inline]
    pub fn process(&mut self, c: usize, x: f32) -> f32 {
        let line = &mut self.lines[c & 1];
        let n = line.len();
        line[self.write] = x;
        let (d, f) = (self.now.floor() as usize, (self.now - self.now.floor()) as f32);
        let at = |j: usize| line[(self.write + n - j % n) % n];
        if f == 0.0 { at(d) } else { at(d) + (at(d + 1) - at(d)) * f }
    }

    /// On to the next sample: the delay glides a step towards its target.
    #[inline]
    pub fn advance(&mut self) {
        let d = self.target as f64 - self.now;
        self.now += d.clamp(-0.5, 0.5);
        self.write = (self.write + 1) % self.lines[0].len();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn whole_delays_are_exact_and_a_change_glides() {
        let mut k = KeyDelay::new(64);
        k.reset(10);
        let x: Vec<f32> = (0..200).map(|i| (i as f32 * 0.37).sin()).collect();
        let mut y = Vec::new();
        for &v in &x {
            y.push(k.process(0, v));
            k.advance();
        }
        for i in 10..200 {
            assert_eq!(y[i], x[i - 10]);
        }
        // To 20: half a sample a sample, so 20 samples to get there, never a jump.
        k.set(20);
        let mut last = k.delay();
        for _ in 0..19 {
            k.advance();
            assert!((k.delay() - last).abs() <= 0.5 + 1e-12);
            last = k.delay();
        }
        k.advance();
        assert_eq!(k.delay(), 20.0);
        assert_eq!(constant_latency(0, 480), 0);
        assert_eq!(constant_latency(1, 480), 480);
    }
}
