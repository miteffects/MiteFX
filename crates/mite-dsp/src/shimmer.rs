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
    /// Per side: each bin's last analysis phase; the last frame's moved peaks (their synthesis
    /// phase and frequency in bins, by target bin; NaN: none there) and this frame's.
    last: [Vec<f64>; 2],
    synth: [Vec<[f64; 2]>; 2],
    fresh: Vec<[f64; 2]>,
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
        let none = || vec![[f64::NAN; 2]; bins];
        Self { stft, last: [zeros(), zeros()], synth: [none(), none()], fresh: none(), mag: zeros(), phase: zeros() }
    }

    /// The delay through it, samples.
    pub fn latency(&self) -> usize {
        self.stft.latency()
    }

    pub fn reset(&mut self) {
        self.stft.reset();
        for v in &mut self.last {
            v.fill(0.0);
        }
        for v in &mut self.synth {
            v.fill([f64::NAN; 2]);
        }
    }

    /// Shifts `left` and `right` in place by `ratio` (2: an octave up), delayed by
    /// [`Self::latency`].
    pub fn process(&mut self, left: &mut [f32], right: &mut [f32], ratio: f32) {
        let ratio = f64::from(ratio.clamp(0.125, 8.0));
        let (n, hop) = (self.stft.size() as f64, self.stft.hop() as f64);
        let (last, synth, fresh, mag, phase) = (&mut self.last, &mut self.synth, &mut self.fresh, &mut self.mag, &mut self.phase);
        self.stft.process(left, right, &mut |l: &mut [Complex<f64>], r: &mut [Complex<f64>]| {
            for (c, spec) in [l, r].into_iter().enumerate() {
                shift_frame(spec, &mut last[c], &mut synth[c], fresh, mag, phase, ratio, n, hop);
            }
        });
    }
}

/// One frame of one side, in place: each spectral peak moves with the bins around it (to the
/// valleys either side) as one piece, so a partial keeps its shape and level, and those bins
/// keep their phases relative to the peak (identity phase locking: no phasiness).
#[allow(clippy::too_many_arguments)]
fn shift_frame(spec: &mut [Complex<f64>], last: &mut [f64], synth: &mut [[f64; 2]], fresh: &mut [[f64; 2]], mag: &mut [f64], phase: &mut [f64], ratio: f64, n: f64, hop: f64) {
    use std::f64::consts::TAU;
    let bins = spec.len();
    // The phase a bin's centre frequency advances over a hop, per bin.
    let expect = TAU * hop / n;
    // Phases as seen from the frame's centre (bin k turned by k·π, as a half-frame rotation
    // would): a partial's bins then share one phase instead of alternating, and its phase does
    // not flip by π when its frequency crosses a bin edge (it did: a drifting tone dipped 12 dB
    // at every crossing).
    for k in 0..bins {
        let (m, ph) = spec[k].to_polar();
        mag[k] = m;
        phase[k] = ph + std::f64::consts::PI * (k % 2) as f64;
        spec[k] = Complex::new(0.0, 0.0);
    }
    fresh.fill([f64::NAN; 2]);
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
        let target = peak as isize + (true_bin * ratio - true_bin).round() as isize;
        if target > 0 && (target as usize) < bins {
            let t = target as usize;
            let f = true_bin * ratio;
            // The phase carries on from the same partial last frame (the moved peak nearest,
            // within two bins: a drifting partial crosses bins without its phase restarting),
            // advancing over the hop at the mean of its old and new frequency.
            let prev = [0isize, -1, 1, -2, 2].iter().map(|d| t as isize + d).filter(|&b| b >= 0 && (b as usize) < bins).map(|b| synth[b as usize]).find(|p| !p[0].is_nan());
            let ph = match prev {
                Some([p, pf]) => (p + 0.5 * (pf + f) * expect) % TAU,
                None => phase[peak],
            };
            fresh[t] = [ph, f];
            // The neighbourhood moves by the shift rounded to whole bins (rounding the shift,
            // not the target, keeps the shape within half a bin of its frequency).
            let offset = (f - true_bin).round() as isize;
            let rot = ph - phase[peak];
            for b in start..end {
                let j = b as isize + offset;
                if j >= 0 && (j as usize) < bins {
                    let back = std::f64::consts::PI * (j % 2) as f64;
                    spec[j as usize] += Complex::from_polar(mag[b], phase[b] + rot - back);
                }
            }
        }
        start = end;
        k = end;
    }
    last.copy_from_slice(&phase[..bins]);
    synth.copy_from_slice(fresh);
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

#[cfg(test)]
mod purity {
    use super::*;
    use realfft::RealFftPlanner;

    /// The share of the output's energy within ±`width` Hz of the wanted partials.
    fn purity(v: &[f32], fs: f32, want: &[f64], width: f64) -> f64 {
        let n = v.len();
        let mut p = RealFftPlanner::<f64>::new();
        let fft = p.plan_fft_forward(n);
        let mut x: Vec<f64> = v.iter().enumerate().map(|(i, &s)| f64::from(s) * (0.5 - 0.5 * (std::f64::consts::TAU * i as f64 / n as f64).cos())).collect();
        let mut out = fft.make_output_vec();
        fft.process(&mut x, &mut out).unwrap();
        let hz = |k: usize| k as f64 * f64::from(fs) / n as f64;
        let tot: f64 = out.iter().map(|c| c.norm_sqr()).sum();
        let hit: f64 = out.iter().enumerate().filter(|(k, _)| want.iter().any(|w| (hz(*k) - w).abs() < width)).map(|(_, c)| c.norm_sqr()).sum();
        hit / tot
    }

    /// An octave up of a tone, a chord and a drifting tone (as a modulated tail is) comes out
    /// clean (little energy off the moved partials) and steady (the level does not beat).
    #[test]
    fn the_shift_is_clean_and_steady_even_on_a_drifting_tone() {
        let fs = 48_000.0f32;
        let cases: [(&str, Vec<f64>); 3] = [("tone", vec![440.0]), ("chord", vec![261.6, 329.6, 392.0, 523.2]), ("glide", vec![])];
        for (name, freqs) in cases {
            let mut sh = SpectralShifter::new(fs);
            let n = 4 * 48_000;
            let mut ph = 0.0f64;
            let mut l: Vec<f32> = (0..n).map(|i| {
                if freqs.is_empty() {
                    let f = 440.0 + 30.0 * i as f64 / n as f64;
                    ph += std::f64::consts::TAU * f / f64::from(fs);
                    (0.5 * ph.sin()) as f32
                } else {
                    let t = i as f64 / f64::from(fs);
                    freqs.iter().map(|f| 0.2 * (std::f64::consts::TAU * f * t).sin()).sum::<f64>() as f32
                }
            }).collect();
            let mut r = l.clone();
            for (a, b) in l.chunks_mut(1).zip(r.chunks_mut(1)) { sh.process(a, b, 2.0); }
            let tail = &l[48_000..];
            let want: Vec<f64> = if freqs.is_empty() { vec![] } else { freqs.iter().map(|f| f * 2.0).collect() };
            let p = if want.is_empty() {
                // The glide: 2·(447.5…470) over the tail; count ±40 Hz around the middle.
                purity(tail, fs, &[2.0 * 458.75], 40.0)
            } else { purity(tail, fs, &want, 6.0) };
            let env: Vec<f32> = tail.chunks(480).map(|c| (c.iter().map(|v| v * v).sum::<f32>() / c.len() as f32).sqrt()).collect();
            let (lo, hi) = env.iter().fold((f32::MAX, 0.0f32), |(a, b), &e| (a.min(e), b.max(e)));
            let (junk, swing) = (10.0 * (1.0 - p).max(1e-12).log10(), 10.0 * (hi / lo).log10());
            // Measured 2026-11-09: junk −40/−41/−47 dB, swing ±0.10/±0.98/±0.73 dB.
            assert!(junk < -35.0 && swing < 1.5, "{name}: junk {junk:.1} dB, envelope ±{swing:.2} dB");
        }
    }
}
