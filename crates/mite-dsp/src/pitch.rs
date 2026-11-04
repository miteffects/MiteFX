//! Pitch detection for Mite Tuner: the McLeod pitch method (McLeod & Wyvill 2005, "A smarter way
//! to find pitch"). The normalised square difference function of a window, from its
//! autocorrelation (by FFT), then the first key maximum within `KEY` of the highest: the
//! fundamental, not a harmonic (a string's second harmonic is often louder than its first). The
//! peak is placed by a parabola through it and its neighbours, then refined on the NSDF itself
//! (golden-section search over a band-limited interpolation of the window): a decaying string
//! within 0.2 cent from 80 Hz up, 0.5 cent on a bass's lowest strings (tested).
//!
//! Runs off the audio thread (the editor reads the input from a tap), so it may take its time;
//! it never allocates after `new`.

use realfft::num_complex::Complex;
use realfft::{ComplexToReal, RealFftPlanner, RealToComplex};
use std::sync::Arc;

/// A key maximum within this share of the highest is the fundamental.
const KEY: f64 = 0.9;
/// Below this NSDF peak the window holds no clear pitch.
pub const MIN_CLARITY: f64 = 0.8;
/// Quieter than this (RMS, dBFS) is silence.
pub const GATE_DB: f64 = -60.0;
pub const MIN_HZ: f64 = 25.0;
pub const MAX_HZ: f64 = 2000.0;

/// The window for `fs`: about 90 ms (four periods of a low B on a five-string bass), a power of
/// two.
pub fn window_len(fs: f64) -> usize {
    ((0.09 * fs) as usize).next_power_of_two().max(1024)
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pitch {
    pub hz: f64,
    /// The NSDF at the peak, 0…1: how periodic the window is.
    pub clarity: f64,
    /// The window's RMS, dBFS.
    pub level_db: f64,
}

pub struct PitchDetector {
    len: usize,
    fwd: Arc<dyn RealToComplex<f64>>,
    inv: Arc<dyn ComplexToReal<f64>>,
    x: Vec<f64>,
    padded: Vec<f64>,
    spec: Vec<Complex<f64>>,
    acf: Vec<f64>,
    nsdf: Vec<f64>,
    scratch_f: Vec<Complex<f64>>,
    scratch_i: Vec<Complex<f64>>,
}

impl PitchDetector {
    /// A detector for windows of `len` samples (see [`window_len`]).
    pub fn new(len: usize) -> Self {
        let mut planner = RealFftPlanner::<f64>::new();
        let fwd = planner.plan_fft_forward(2 * len);
        let inv = planner.plan_fft_inverse(2 * len);
        let (scratch_f, scratch_i) = (fwd.make_scratch_vec(), inv.make_scratch_vec());
        Self { len, x: vec![0.0; len], padded: vec![0.0; 2 * len], spec: fwd.make_output_vec(), acf: vec![0.0; 2 * len], nsdf: vec![0.0; len], fwd, inv, scratch_f, scratch_i }
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// The pitch of `window` (its last `len` samples) at `fs`, if it has a clear one.
    pub fn detect(&mut self, window: &[f32], fs: f64) -> Option<Pitch> {
        let n = self.len;
        if window.len() < n {
            return None;
        }
        let src = &window[window.len() - n..];
        let mean = src.iter().map(|&v| f64::from(v)).sum::<f64>() / n as f64;
        for (d, &s) in self.x.iter_mut().zip(src) {
            *d = f64::from(s) - mean;
        }
        let energy: f64 = self.x.iter().map(|v| v * v).sum();
        let level_db = 10.0 * (energy / n as f64).max(1e-20).log10();
        if level_db < GATE_DB {
            return None;
        }
        // Autocorrelation by FFT (zero-padded to 2n: no wrap).
        self.padded[..n].copy_from_slice(&self.x);
        self.padded[n..].iter_mut().for_each(|v| *v = 0.0);
        self.fwd.process_with_scratch(&mut self.padded, &mut self.spec, &mut self.scratch_f).ok()?;
        for c in &mut self.spec {
            *c = Complex::new(c.norm_sqr(), 0.0);
        }
        self.inv.process_with_scratch(&mut self.spec, &mut self.acf, &mut self.scratch_i).ok()?;
        let scale = 1.0 / (2 * n) as f64;
        // NSDF: n(τ) = 2 r(τ) / m(τ), m(τ) = Σ x[j]² + x[j+τ]² over the overlap.
        let mut m = 2.0 * energy;
        let tau_max = ((fs / MIN_HZ) as usize).min(n / 2);
        let tau_min = ((fs / MAX_HZ) as usize).max(2);
        for tau in 0..tau_max {
            if tau > 0 {
                m -= self.x[tau - 1] * self.x[tau - 1] + self.x[n - tau] * self.x[n - tau];
            }
            self.nsdf[tau] = if m > 1e-12 { 2.0 * self.acf[tau] * scale / m } else { 0.0 };
        }
        // Key maxima: the highest point between each positive-going zero crossing and the next
        // negative-going one.
        let mut keys: [(usize, f64); 64] = [(0, 0.0); 64];
        let mut count = 0;
        let mut tau = 1;
        while tau < tau_max && self.nsdf[tau] > 0.0 {
            tau += 1;
        }
        while tau < tau_max && count < keys.len() {
            while tau < tau_max && self.nsdf[tau] <= 0.0 {
                tau += 1;
            }
            let mut best = (0, f64::MIN);
            while tau < tau_max && self.nsdf[tau] > 0.0 {
                if self.nsdf[tau] > best.1 && tau >= tau_min {
                    best = (tau, self.nsdf[tau]);
                }
                tau += 1;
            }
            if best.0 > 0 && best.0 + 1 < tau_max {
                keys[count] = best;
                count += 1;
            }
        }
        let highest = keys[..count].iter().map(|k| k.1).fold(0.0, f64::max);
        if highest < MIN_CLARITY {
            return None;
        }
        let &(k, _) = keys[..count].iter().find(|k| k.1 >= KEY * highest)?;
        let (a, b, c) = (self.nsdf[k - 1], self.nsdf[k], self.nsdf[k + 1]);
        let den = a - 2.0 * b + c;
        let shift = if den.abs() > 1e-12 { (0.5 * (a - c) / den).clamp(-0.5, 0.5) } else { 0.0 };
        let (tau, clarity) = self.refine(k as f64 + shift, b);
        Some(Pitch { hz: fs / tau, clarity, level_db })
    }

    /// The NSDF at a fractional lag, the window read between samples by a windowed sinc.
    fn nsdf_at(&self, tau: f64) -> f64 {
        let n = self.len;
        let whole = tau.floor() as usize;
        let frac = tau - whole as f64;
        let half = 8i64;
        let (mut r, mut m) = (0.0, 0.0);
        let mut taps = [0.0f64; 16];
        for (i, t) in taps.iter_mut().enumerate() {
            let k = i as f64 - (half - 1) as f64 - frac;
            let x = std::f64::consts::PI * k;
            let sinc = if x.abs() < 1e-9 { 1.0 } else { x.sin() / x };
            let w = 0.5 + 0.5 * (std::f64::consts::PI * k / half as f64).cos();
            *t = sinc * w;
        }
        let start = half as usize;
        let end = n.saturating_sub(whole + half as usize);
        for j in start..end {
            let mut y = 0.0;
            for (i, t) in taps.iter().enumerate() {
                y += t * self.x[j + whole + i + 1 - half as usize];
            }
            r += self.x[j] * y;
            m += self.x[j] * self.x[j] + y * y;
        }
        if m > 1e-12 {
            2.0 * r / m
        } else {
            0.0
        }
    }

    /// The peak of the NSDF near `tau` (golden-section over ±0.6 samples) and its value.
    fn refine(&self, tau: f64, value: f64) -> (f64, f64) {
        let g = 0.5 * (5f64.sqrt() - 1.0);
        let (mut lo, mut hi) = (tau - 0.6, tau + 0.6);
        let mut c = hi - g * (hi - lo);
        let mut d = lo + g * (hi - lo);
        let (mut fc, mut fd) = (self.nsdf_at(c), self.nsdf_at(d));
        for _ in 0..14 {
            if fc > fd {
                hi = d;
                d = c;
                fd = fc;
                c = hi - g * (hi - lo);
                fc = self.nsdf_at(c);
            } else {
                lo = c;
                c = d;
                fc = fd;
                d = lo + g * (hi - lo);
                fd = self.nsdf_at(d);
            }
        }
        let t = 0.5 * (lo + hi);
        (t, fc.max(fd).max(value.min(1.0)))
    }
}

/// A frequency as a note against `reference` (A4, Hz): the nearest MIDI note and how far off,
/// cents (−50…50).
pub fn note_of(hz: f64, reference: f64) -> (i32, f64) {
    let midi = 69.0 + 12.0 * (hz / reference).log2();
    let note = midi.round();
    (note as i32, 100.0 * (midi - note))
}

/// A MIDI note's frequency against `reference` (A4, Hz).
pub fn note_hz(note: i32, reference: f64) -> f64 {
    reference * 2f64.powf(f64::from(note - 69) / 12.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A plucked string: harmonics falling as 1/k (the second louder than the first when
    /// `strong_second`), a decay, a little noise.
    fn string(hz: f64, fs: f64, n: usize, strong_second: bool) -> Vec<f32> {
        let mut seed = 1u32;
        (0..n)
            .map(|i| {
                let t = i as f64 / fs;
                let mut v = 0.0;
                for k in 1..12 {
                    let a = if strong_second && k == 2 { 1.6 } else { 1.0 / k as f64 };
                    v += a * (std::f64::consts::TAU * hz * k as f64 * t + 0.3 * k as f64).sin() * (-t * (0.6 + 0.4 * k as f64)).exp();
                }
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                (0.2 * v + 0.002 * (seed as f64 / u32::MAX as f64 - 0.5)) as f32
            })
            .collect()
    }

    #[test]
    fn finds_every_string_within_a_fifth_of_a_cent() {
        for fs in [44_100.0, 48_000.0, 96_000.0] {
            let mut d = PitchDetector::new(window_len(fs));
            // A low B (bass), E2…E4 on a guitar, a few detuned, and high on the neck.
            for hz in [30.87, 41.2, 82.41, 110.0 * 1.003, 146.83, 196.0, 246.94 / 1.002, 329.63, 659.26, 1046.5] {
                let x = string(hz, fs, d.len() + 1000, false);
                let p = d.detect(&x, fs).expect("a pitch");
                let cents = 1200.0 * (p.hz / hz).log2();
                // A decaying string biases the peak a little; least where there are fewest periods.
                let tolerance = if hz < 60.0 { 0.5 } else { 0.2 };
                assert!(cents.abs() < tolerance, "{hz} Hz at {fs}: {} Hz ({cents:+.3} cents)", p.hz);
                assert!(p.clarity > 0.9);
            }
        }
    }

    #[test]
    fn a_loud_second_harmonic_is_not_the_note() {
        let fs = 48_000.0;
        let mut d = PitchDetector::new(window_len(fs));
        for hz in [82.41, 110.0, 196.0] {
            let p = d.detect(&string(hz, fs, d.len(), true), fs).expect("a pitch");
            assert!((1200.0 * (p.hz / hz).log2()).abs() < 1.0, "{hz}: {}", p.hz);
        }
    }

    #[test]
    fn silence_and_noise_have_no_pitch() {
        let fs = 48_000.0;
        let mut d = PitchDetector::new(window_len(fs));
        assert!(d.detect(&vec![0.0; d.len()], fs).is_none());
        let mut seed = 9u32;
        let noise: Vec<f32> = (0..d.len())
            .map(|_| {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                seed as f32 / u32::MAX as f32 - 0.5
            })
            .collect();
        assert!(d.detect(&noise, fs).is_none());
    }

    #[test]
    fn notes_and_cents() {
        assert_eq!(note_of(440.0, 440.0), (69, 0.0));
        let (n, c) = note_of(82.41, 440.0);
        assert_eq!(n, 40);
        assert!(c.abs() < 0.1);
        let (n, c) = note_of(445.0, 440.0);
        assert_eq!(n, 69);
        assert!((c - 19.56).abs() < 0.01, "{c}");
        assert!((note_hz(57, 432.0) - 216.0).abs() < 1e-9);
    }
}
