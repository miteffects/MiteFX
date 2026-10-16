//! Streaming stereo STFT with weighted overlap-add, for processing that works per frequency bin
//! (spectral dynamics, resonance suppression). 64-bit, allocation-free in [`Stft::process`].
//!
//! Frames of `size` samples every `size / 4` (75 % overlap), square-root Hann windows for analysis
//! and synthesis (together a Hann window, whose 75 %-overlapped copies sum to a constant), so
//! leaving the spectra alone reconstructs the input exactly, delayed by `size` samples.

use std::sync::Arc;

use realfft::num_complex::Complex;
use realfft::{ComplexToReal, RealFftPlanner, RealToComplex};

pub struct Stft {
    size: usize,
    hop: usize,
    r2c: Arc<dyn RealToComplex<f64>>,
    c2r: Arc<dyn ComplexToReal<f64>>,
    fwd_scratch: Vec<Complex<f64>>,
    inv_scratch: Vec<Complex<f64>>,
    /// Square-root Hann window (analysis and synthesis), and the overlap-add normalisation.
    window: Vec<f64>,
    norm: f64,
    frame: Vec<f64>,
    /// Per channel: the last `size` input samples, the overlap-add accumulator, the hop of output
    /// being played, and the frame's spectrum.
    input: [Vec<f64>; 2],
    acc: [Vec<f64>; 2],
    ready: [Vec<f64>; 2],
    spectra: [Vec<Complex<f64>>; 2],
    pos: usize,
}

impl Stft {
    /// Frames of `size` samples (a power of two, at least 16). Allocates.
    pub fn new(size: usize) -> Self {
        assert!(size.is_power_of_two() && size >= 16, "STFT size must be a power of two");
        let hop = size / 4;
        let mut planner = RealFftPlanner::<f64>::new();
        let r2c = planner.plan_fft_forward(size);
        let c2r = planner.plan_fft_inverse(size);
        let window: Vec<f64> =
            (0..size).map(|n| (0.5 - 0.5 * (2.0 * std::f64::consts::PI * n as f64 / size as f64).cos()).sqrt()).collect();
        // Hann copies at a quarter-frame hop sum to 2; the inverse FFT scales by `size`.
        let norm = 1.0 / (2.0 * size as f64);
        let zeros = |n: usize| vec![0.0; n];
        Self {
            size,
            hop,
            fwd_scratch: r2c.make_scratch_vec(),
            inv_scratch: c2r.make_scratch_vec(),
            spectra: [r2c.make_output_vec(), r2c.make_output_vec()],
            r2c,
            c2r,
            window,
            norm,
            frame: zeros(size),
            input: [zeros(size), zeros(size)],
            acc: [zeros(size), zeros(size)],
            ready: [zeros(hop), zeros(hop)],
            pos: 0,
        }
    }

    pub fn size(&self) -> usize {
        self.size
    }

    /// Samples between frames.
    pub fn hop(&self) -> usize {
        self.hop
    }

    /// Frames per second at `sample_rate`.
    pub fn frame_rate(&self, sample_rate: f64) -> f64 {
        sample_rate / self.hop as f64
    }

    /// Number of bins per spectrum: `size / 2 + 1`.
    pub fn bins(&self) -> usize {
        self.size / 2 + 1
    }

    /// Input-to-output delay, samples.
    pub fn latency(&self) -> usize {
        self.size
    }

    /// The magnitude a full-scale sine centred on a bin has in these spectra, for calibrating
    /// levels to dBFS.
    pub fn full_scale(&self) -> f64 {
        0.5 * self.window.iter().sum::<f64>()
    }

    /// Clears the signal history. Allocation-free.
    pub fn reset(&mut self) {
        for c in 0..2 {
            self.input[c].fill(0.0);
            self.acc[c].fill(0.0);
            self.ready[c].fill(0.0);
        }
        self.pos = 0;
    }

    /// Runs a stereo buffer through, calling `frame` with each frame's left and right spectra to
    /// change in place. Allocation- and lock-free (as long as `frame` is).
    pub fn process(&mut self, left: &mut [f32], right: &mut [f32], frame: &mut impl FnMut(&mut [Complex<f64>], &mut [Complex<f64>])) {
        let base = self.size - self.hop;
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            let finite = |x: f32| if x.is_finite() { f64::from(x) } else { 0.0 };
            self.input[0][base + self.pos] = finite(*l);
            self.input[1][base + self.pos] = finite(*r);
            *l = self.ready[0][self.pos] as f32;
            *r = self.ready[1][self.pos] as f32;
            self.pos += 1;
            if self.pos == self.hop {
                self.pos = 0;
                self.process_frame(frame);
            }
        }
    }

    fn process_frame(&mut self, frame: &mut impl FnMut(&mut [Complex<f64>], &mut [Complex<f64>])) {
        for c in 0..2 {
            for ((f, x), w) in self.frame.iter_mut().zip(&self.input[c]).zip(&self.window) {
                *f = x * w;
            }
            // Buffers are sized by the planner; never panic on the audio thread.
            let _ = self.r2c.process_with_scratch(&mut self.frame, &mut self.spectra[c], &mut self.fwd_scratch);
        }
        let [left, right] = &mut self.spectra;
        frame(left, right);
        for c in 0..2 {
            // DC and Nyquist must be real for the inverse transform.
            let last = self.spectra[c].len() - 1;
            self.spectra[c][0].im = 0.0;
            self.spectra[c][last].im = 0.0;
            let _ = self.c2r.process_with_scratch(&mut self.spectra[c], &mut self.frame, &mut self.inv_scratch);
            for ((a, f), w) in self.acc[c].iter_mut().zip(&self.frame).zip(&self.window) {
                *a += f * w * self.norm;
            }
            // The oldest hop is complete: play it next, and slide everything on.
            self.ready[c].copy_from_slice(&self.acc[c][..self.hop]);
            self.acc[c].copy_within(self.hop.., 0);
            let len = self.acc[c].len();
            self.acc[c][len - self.hop..].fill(0.0);
            self.input[c].copy_within(self.hop.., 0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn noise(n: usize, seed: u32) -> Vec<f32> {
        let mut s = seed;
        (0..n)
            .map(|_| {
                s ^= s << 13;
                s ^= s >> 17;
                s ^= s << 5;
                s as f32 / u32::MAX as f32 - 0.5
            })
            .collect()
    }

    #[test]
    fn untouched_spectra_reconstruct_the_input_after_its_latency() {
        let mut stft = Stft::new(1024);
        let (l0, r0) = (noise(20_000, 1), noise(20_000, 2));
        let (mut l, mut r) = (l0.clone(), r0.clone());
        for (a, b) in l.chunks_mut(100).zip(r.chunks_mut(100)) {
            stft.process(a, b, &mut |_, _| {});
        }
        let d = stft.latency();
        // Once the first frames have filled in, the output is the input delayed by `size`.
        let worst = (2 * d..20_000).map(|n| (l[n] - l0[n - d]).abs().max((r[n] - r0[n - d]).abs())).fold(0.0f32, f32::max);
        assert!(worst < 1e-6, "residual {worst}");
    }

    #[test]
    fn a_bin_gain_scales_a_sine_on_that_bin() {
        let size = 1024;
        let mut stft = Stft::new(size);
        let bin = 64;
        let tone: Vec<f32> = (0..20_000).map(|n| (0.5 * (2.0 * std::f64::consts::PI * bin as f64 * n as f64 / size as f64).sin()) as f32).collect();
        let (mut l, mut r) = (tone.clone(), tone.clone());
        // Halve the tone's bin and its neighbours (a square-root Hann window spreads a sine wide).
        stft.process(&mut l, &mut r, &mut |a, b| {
            for k in bin - 12..=bin + 12 {
                a[k] *= 0.5;
                b[k] *= 0.5;
            }
        });
        let peak = l[10_000..].iter().map(|v| v.abs()).fold(0.0f32, f32::max);
        assert!((peak - 0.25).abs() < 0.002, "peak {peak}");
    }

    #[test]
    fn full_scale_calibrates_a_sine_to_zero_dbfs() {
        let size = 1024;
        let mut stft = Stft::new(size);
        let bin = 100;
        let tone: Vec<f32> = (0..8192).map(|n| (2.0 * std::f64::consts::PI * bin as f64 * n as f64 / size as f64).sin() as f32).collect();
        let (mut l, mut r) = (tone.clone(), tone.clone());
        let mut seen = 0.0;
        let full = stft.full_scale();
        stft.process(&mut l, &mut r, &mut |a, _| seen = a[bin].norm() / full);
        assert!((seen - 1.0).abs() < 1e-3, "{seen}");
    }
}
