//! A pitch shifter for a reverb's shimmer: a phase vocoder with peak-locked shifting (Laroche &
//! Dolson). Each frame (75 % overlap) every spectral peak's true frequency is measured from its
//! phase advance, and the peak with its neighbourhood moves to `ratio` times it, resynthesised
//! with its phase advancing at the new frequency. No grains, no splices: the
//! output is continuous (soft and a little smeared in time, which a reverb tail hides), where a
//! two-grain time-domain shifter crossfades twenty times a second and is heard as grain
//! (user, 2026-11-09: "still sounds grainy").
//!
//! Sized in [`SpectralShifter::new`]; `process` never allocates.

use realfft::num_complex::Complex;

use crate::stft::Stft;

/// The frame, about 85 ms at any rate (4096 at 44.1/48 kHz): fine enough in frequency that the
/// low notes' harmonics land in their own bins.
pub fn frame_size(fs: f32) -> usize {
    let target = 0.085 * fs;
    let mut n = 1024;
    while (n as f32) < target * 0.75 {
        n *= 2;
    }
    n
}

pub struct SpectralShifter {
    stft: Stft,
    /// Per side: each bin's last analysis phase and its running synthesis phase.
    last: [Vec<f64>; 2],
    synth: [Vec<f64>; 2],
    /// Scratch per frame: the magnitudes and phases.
    mag: Vec<f64>,
    phase: Vec<f64>,
}

impl SpectralShifter {
    /// Allocates.
    pub fn new(fs: f32) -> Self {
        let stft = Stft::new(frame_size(fs));
        let bins = stft.bins();
        let zeros = || vec![0.0; bins];
        Self { stft, last: [zeros(), zeros()], synth: [zeros(), zeros()], mag: zeros(), phase: zeros() }
    }

    /// The delay through it, samples.
    pub fn latency(&self) -> usize {
        self.stft.latency()
    }

    pub fn reset(&mut self) {
        self.stft.reset();
        for v in self.last.iter_mut().chain(self.synth.iter_mut()) {
            v.fill(0.0);
        }
    }

    /// Shifts `left` and `right` in place by `ratio` (2: an octave up), delayed by
    /// [`Self::latency`].
    pub fn process(&mut self, left: &mut [f32], right: &mut [f32], ratio: f32) {
        let ratio = f64::from(ratio.clamp(0.125, 8.0));
        let (n, hop) = (self.stft.size() as f64, self.stft.hop() as f64);
        let (last, synth, mag, phase) = (&mut self.last, &mut self.synth, &mut self.mag, &mut self.phase);
        self.stft.process(left, right, &mut |l: &mut [Complex<f64>], r: &mut [Complex<f64>]| {
            for (c, spec) in [l, r].into_iter().enumerate() {
                shift_frame(spec, &mut last[c], &mut synth[c], mag, phase, ratio, n, hop);
            }
        });
    }
}

/// One frame of one side, in place: each spectral peak moves with the bins around it (to the
/// valleys either side) as one piece, so a partial keeps its shape and level, and those bins
/// keep their phases relative to the peak (identity phase locking: no phasiness).
#[allow(clippy::too_many_arguments)]
fn shift_frame(spec: &mut [Complex<f64>], last: &mut [f64], synth: &mut [f64], mag: &mut [f64], phase: &mut [f64], ratio: f64, n: f64, hop: f64) {
    use std::f64::consts::TAU;
    let bins = spec.len();
    // The phase a bin's centre frequency advances over a hop, per bin.
    let expect = TAU * hop / n;
    for k in 0..bins {
        let (m, ph) = spec[k].to_polar();
        mag[k] = m;
        phase[k] = ph;
        spec[k] = Complex::new(0.0, 0.0);
    }
    let mut k = 1;
    let mut start = 0;
    while k < bins {
        // The next peak, and the valley after it.
        let is_peak = mag[k] > mag[k - 1] && (k + 1 >= bins || mag[k] >= mag[k + 1]);
        if !is_peak {
            k += 1;
            continue;
        }
        let peak = k;
        let mut end = peak + 1;
        while end < bins && mag[end] < mag[end - 1] {
            end += 1;
        }
        let mut d = phase[peak] - last[peak] - peak as f64 * expect;
        d -= TAU * (d / TAU).round();
        let true_bin = peak as f64 + d / expect;
        let target = (true_bin * ratio).round() as isize;
        let offset = target - peak as isize;
        if target > 0 && (target as usize) < bins {
            let t = target as usize;
            // The target bin's running phase advances at the moved frequency.
            synth[t] = (synth[t] + true_bin * ratio * expect) % TAU;
            for b in start..end {
                let j = b as isize + offset;
                if j >= 0 && (j as usize) < bins {
                    let rot = synth[t] - phase[peak];
                    spec[j as usize] += Complex::from_polar(mag[b], phase[b] + rot);
                }
            }
        }
        start = end;
        k = end;
    }
    last.copy_from_slice(&phase[..bins]);
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
    fn it_moves_a_tone_by_the_interval_smoothly() {
        let fs = 48_000.0;
        for (semis, want) in [(12.0f32, 880.0f32), (-12.0, 220.0), (7.0, 659.255)] {
            let mut sh = SpectralShifter::new(fs);
            let r = (semis / 12.0).exp2();
            let mut l: Vec<f32> = (0..96_000).map(|i| 0.5 * (std::f32::consts::TAU * 440.0 * i as f32 / fs).sin()).collect();
            let mut rr = l.clone();
            for (a, b) in l.chunks_mut(64).zip(rr.chunks_mut(64)) {
                sh.process(a, b, r);
            }
            let tail = &l[24_000..];
            let (hit, orig) = (level(tail, want, fs), level(tail, 440.0, fs));
            assert!(hit > 0.35 && orig < 0.02, "{semis}: {hit} at the interval, {orig} left");
            // Smooth: the envelope (10 ms RMS) varies under ±1 dB on a steady tone (two grains
            // dip several dB at every crossfade).
            let env: Vec<f32> = tail.chunks(480).map(|c| (c.iter().map(|v| v * v).sum::<f32>() / c.len() as f32).sqrt()).collect();
            let (lo, hi) = env.iter().fold((f32::MAX, 0.0f32), |(a, b), &e| (a.min(e), b.max(e)));
            assert!(20.0 * (hi / lo).log10() < 2.0, "{semis}: envelope {lo}…{hi}");
        }
    }
}
