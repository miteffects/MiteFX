//! Linear-phase kernels for the EQ: the bands' analog magnitude, with zero phase, made causal by
//! a delay of half the kernel length. Runs off the audio thread (allocates).
//!
//! Each band is a 2×2 matrix per frequency in the L/R basis (a stereo band scales both channels,
//! a mid or side band scales M or S), so stereo placements combine exactly; the result comes back
//! as one shared kernel, a kernel per channel, or a full matrix, whichever the bands need.

use realfft::num_complex::Complex;
use realfft::RealFftPlanner;

use crate::conv::KernelLayout;
use crate::eq::{analog_response_db, BandSettings, Placement};

/// Kernels for [`crate::conv::KernelSet::new`]: their layout, the kernels (1, 2 or 4, each `len`
/// samples), and their delay (`len / 2`).
pub struct LinearPhaseKernels {
    pub layout: KernelLayout,
    pub kernels: Vec<Vec<f64>>,
    pub delay: usize,
}

/// A 2×2 real matrix in the L/R basis: `[ll, lr, rl, rr]`.
type Matrix = [f64; 4];

const IDENTITY: Matrix = [1.0, 0.0, 0.0, 1.0];

fn multiply(a: Matrix, b: Matrix) -> Matrix {
    [a[0] * b[0] + a[1] * b[2], a[0] * b[1] + a[1] * b[3], a[2] * b[0] + a[3] * b[2], a[2] * b[1] + a[3] * b[3]]
}

/// One band's matrix for a linear gain `g`.
fn band_matrix(placement: Placement, g: f64) -> Matrix {
    let (sum, diff) = (0.5 * (g + 1.0), 0.5 * (g - 1.0));
    match placement {
        Placement::Stereo => [g, 0.0, 0.0, g],
        Placement::Left => [g, 0.0, 0.0, 1.0],
        Placement::Right => [1.0, 0.0, 0.0, g],
        // M' = g·M, S' = S, with M = (L + R)/2 and S = (L − R)/2.
        Placement::Mid => [sum, diff, diff, sum],
        Placement::Side => [sum, -diff, -diff, sum],
    }
}

/// The EQ's linear-phase kernels, `len` samples long (a power of two), at `sample_rate`.
pub fn design(bands: &[BandSettings], sample_rate: f64, len: usize) -> LinearPhaseKernels {
    assert!(len.is_power_of_two() && len >= 4, "kernel length must be a power of two");
    let bins = len / 2 + 1;
    // The whole EQ's matrix at every bin, bands applied in order.
    let mut response = vec![IDENTITY; bins];
    for s in bands.iter().filter(|s| s.enabled) {
        for (k, m) in response.iter_mut().enumerate() {
            let f = k as f64 * sample_rate / len as f64;
            let db = analog_response_db(s, f);
            let g = if db.is_finite() { 10f64.powf(db / 20.0) } else { 0.0 };
            *m = multiply(band_matrix(s.placement, g), *m);
        }
    }
    let off_diagonal = response.iter().any(|m| m[1].abs() > 1e-12 || m[2].abs() > 1e-12);
    let unequal = response.iter().any(|m| (m[0] - m[3]).abs() > 1e-12);
    let (layout, entries): (KernelLayout, &[usize]) = if off_diagonal {
        (KernelLayout::Matrix, &[0, 1, 2, 3])
    } else if unequal {
        (KernelLayout::Diagonal, &[0, 3])
    } else {
        (KernelLayout::Shared, &[0])
    };

    let ifft = RealFftPlanner::<f64>::new().plan_fft_inverse(len);
    let mut spectrum = ifft.make_input_vec();
    let mut scratch = ifft.make_scratch_vec();
    let mut zero_phase = vec![0.0; len];
    let half = len / 2;
    let kernels = entries
        .iter()
        .map(|&e| {
            for (c, m) in spectrum.iter_mut().zip(&response) {
                *c = Complex::new(m[e], 0.0);
            }
            ifft.process_with_scratch(&mut spectrum, &mut zero_phase, &mut scratch).expect("buffers sized by the planner");
            // Centre the zero-phase response at `half` and taper it with a Blackman window, which
            // is 1 at the centre and 0 at sample 0: the kernel is exactly symmetric about `half`.
            (0..len)
                .map(|n| {
                    let h = zero_phase[(n + len - half) % len] / len as f64;
                    let x = 2.0 * std::f64::consts::PI * n as f64 / len as f64;
                    h * (0.42 - 0.5 * x.cos() + 0.08 * (2.0 * x).cos())
                })
                .collect()
        })
        .collect();
    LinearPhaseKernels { layout, kernels, delay: half }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::eq::BandType;

    fn band(kind: BandType, freq: f64, gain_db: f64, q: f64, placement: Placement) -> BandSettings {
        BandSettings { enabled: true, kind, freq, gain_db, q, placement, ..Default::default() }
    }

    /// The kernel's magnitude at `f`, dB.
    fn magnitude_db(h: &[f64], f: f64, fs: f64) -> f64 {
        let w = 2.0 * std::f64::consts::PI * f / fs;
        let (re, im) = h.iter().enumerate().fold((0.0, 0.0), |(re, im), (n, &v)| (re + v * (w * n as f64).cos(), im - v * (w * n as f64).sin()));
        10.0 * (re * re + im * im).log10()
    }

    #[test]
    fn no_bands_is_a_pure_delay() {
        let k = design(&[], 48_000.0, 1024);
        assert_eq!(k.layout, KernelLayout::Shared);
        assert_eq!(k.delay, 512);
        for (n, &v) in k.kernels[0].iter().enumerate() {
            let want = if n == 512 { 1.0 } else { 0.0 };
            assert!((v - want).abs() < 1e-12, "tap {n} = {v}");
        }
    }

    #[test]
    fn kernels_are_symmetric_and_follow_the_analog_curve() {
        let bands = [band(BandType::Bell, 1000.0, 6.0, 1.0, Placement::Stereo), band(BandType::HighShelf, 6000.0, -4.0, 0.7, Placement::Stereo)];
        let fs = 48_000.0;
        let k = design(&bands, fs, 4096);
        let h = &k.kernels[0];
        for i in 1..2048 {
            assert!((h[2048 - i] - h[2048 + i]).abs() < 1e-12, "asymmetric at ±{i}");
        }
        for f in [200.0, 1000.0, 3000.0, 10_000.0, 18_000.0] {
            let want: f64 = bands.iter().map(|b| analog_response_db(b, f)).sum();
            let got = magnitude_db(h, f, fs);
            assert!((got - want).abs() < 0.1, "{f} Hz: {got:.3} dB, analog {want:.3} dB");
        }
    }

    #[test]
    fn stereo_placements_pick_the_right_layout() {
        let fs = 48_000.0;
        let left = band(BandType::Bell, 1000.0, 6.0, 1.0, Placement::Left);
        assert_eq!(design(&[left], fs, 1024).layout, KernelLayout::Diagonal);
        let mid = band(BandType::Bell, 1000.0, 6.0, 1.0, Placement::Mid);
        let k = design(&[mid], fs, 1024);
        assert_eq!(k.layout, KernelLayout::Matrix);
        // A mid boost leaves the side alone: L − R passes unchanged.
        let side_gain = magnitude_db(&k.kernels[0].iter().zip(&k.kernels[1]).map(|(a, b)| a - b).collect::<Vec<_>>(), 1000.0, fs);
        assert!(side_gain.abs() < 0.05, "side moved by {side_gain} dB");
        let mid_gain = magnitude_db(&k.kernels[0].iter().zip(&k.kernels[1]).map(|(a, b)| a + b).collect::<Vec<_>>(), 1000.0, fs);
        assert!((mid_gain - 6.0).abs() < 0.1, "mid {mid_gain} dB");
    }
}
