//! Natural Phase correction kernels: what to add after the EQ's zero-latency IIR sections so the
//! whole response matches the analog prototypes in magnitude *and* phase, up to Nyquist, at the
//! cost of a short delay. Runs off the audio thread (allocates).
//!
//! The matched IIR sections already follow the analog magnitude closely and are minimum phase
//! like their prototypes; they part ways near Nyquist (magnitude within about a dB, phase by
//! more). The correction is the ratio analog / digital per frequency, which is 1 almost
//! everywhere, so its impulse response is short: centred on `delay` and windowed to twice that.
//! Bands combine as 2×2 complex matrices in the L/R basis, as in [`crate::linear_phase`].

use realfft::num_complex::Complex;
use realfft::RealFftPlanner;

use crate::conv::{KernelDesign, KernelLayout};
use crate::eq::{analog_response, design as design_band, BandSettings, Placement};

/// The correction's delay at 48 kHz (its kernel is twice as long). Scale it with the sample rate.
pub const DELAY_48K: usize = 128;
/// The correction never boosts or cuts by more than this (linear, 12 dB): where a band drives
/// both responses towards zero (a cut's stopband), their ratio means nothing.
const MAX_CORRECTION: f64 = 4.0;
/// Where the digital response's determinant falls from 1e-6 to 1e-8 (a stereo band about 60 to
/// 80 dB down: a cut's stopband), the correction eases back to none. Both responses are tiny there
/// and their ratio means nothing; letting it drop to zero instead would put a sharp edge in the
/// correction's spectrum, whose long impulse the short window then turns into passband ripple.
const DET_FULL: f64 = 1e-6;
const DET_NONE: f64 = 1e-8;

type C = Complex<f64>;
/// A 2×2 complex matrix in the L/R basis: `[ll, lr, rl, rr]`.
type Matrix = [C; 4];

fn identity() -> Matrix {
    [C::new(1.0, 0.0), C::new(0.0, 0.0), C::new(0.0, 0.0), C::new(1.0, 0.0)]
}

fn multiply(a: &Matrix, b: &Matrix) -> Matrix {
    [a[0] * b[0] + a[1] * b[2], a[0] * b[1] + a[1] * b[3], a[2] * b[0] + a[3] * b[2], a[2] * b[1] + a[3] * b[3]]
}

/// One band's matrix for a complex gain `g`.
fn band_matrix(placement: Placement, g: C) -> Matrix {
    let one = C::new(1.0, 0.0);
    let zero = C::new(0.0, 0.0);
    let (sum, diff) = ((g + one) * 0.5, (g - one) * 0.5);
    match placement {
        Placement::Stereo => [g, zero, zero, g],
        Placement::Left => [g, zero, zero, one],
        Placement::Right => [one, zero, zero, g],
        Placement::Mid => [sum, diff, diff, sum],
        Placement::Side => [sum, -diff, -diff, sum],
    }
}

fn determinant(m: &Matrix) -> C {
    m[0] * m[3] - m[1] * m[2]
}

/// `m⁻¹`. Only used where the determinant is at least [`DET_NONE`].
fn inverse(m: &Matrix) -> Matrix {
    let det = determinant(m);
    let inv_det = det.conj() / det.norm_sqr().max(DET_NONE * DET_NONE);
    [m[3] * inv_det, -m[1] * inv_det, -m[2] * inv_det, m[0] * inv_det]
}

/// The correction for `bands` at `sample_rate`, centred on `delay` samples (a kernel of
/// `2 · delay`).
pub fn design(bands: &[BandSettings], sample_rate: f64, delay: usize) -> KernelDesign {
    let enabled: Vec<&BandSettings> = bands.iter().filter(|s| s.enabled).collect();
    let sections: Vec<_> = enabled.iter().map(|s| design_band(s, sample_rate)).collect();
    // A fine frequency grid, so the windowed impulse has little time aliasing to begin with.
    let len = (16 * delay).next_power_of_two();
    let bins = len / 2 + 1;
    let correction: Vec<Matrix> = (0..bins)
        .map(|k| {
            let w = std::f64::consts::PI * k as f64 / (bins - 1) as f64;
            let f = w / (2.0 * std::f64::consts::PI) * sample_rate;
            let (mut analog, mut digital) = (identity(), identity());
            for (s, sec) in enabled.iter().zip(&sections) {
                analog = multiply(&band_matrix(s.placement, analog_response(s, f)), &analog);
                digital = multiply(&band_matrix(s.placement, sec.response(w)), &digital);
            }
            // analog · digital⁻¹, easing to the identity where the digital response vanishes.
            let det = determinant(&digital).norm();
            let u = ((det.max(1e-300).log10() - DET_NONE.log10()) / (DET_FULL.log10() - DET_NONE.log10())).clamp(0.0, 1.0);
            let t = u * u * (3.0 - 2.0 * u);
            let ratio = if t > 0.0 { multiply(&analog, &inverse(&digital)) } else { identity() };
            let mut c: Matrix = std::array::from_fn(|i| ratio[i] * t + identity()[i] * (1.0 - t));
            for e in &mut c {
                if !e.re.is_finite() || !e.im.is_finite() {
                    *e = C::new(0.0, 0.0);
                } else if e.norm() > MAX_CORRECTION {
                    *e *= MAX_CORRECTION / e.norm();
                }
                // DC and Nyquist bins of a real kernel are real.
                if k == 0 || k == bins - 1 {
                    e.im = 0.0;
                }
            }
            c
        })
        .collect();

    let small = |e: C| e.norm() < 1e-12;
    let off_diagonal = correction.iter().any(|m| !small(m[1]) || !small(m[2]));
    let unequal = correction.iter().any(|m| !small(m[0] - m[3]));
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
    let mut centred = vec![0.0; len];
    let taps = 2 * delay;
    let kernels = entries
        .iter()
        .map(|&e| {
            for (c, m) in spectrum.iter_mut().zip(&correction) {
                *c = m[e];
            }
            // Buffers are sized by the planner and DC/Nyquist are real: this cannot fail.
            let _ = ifft.process_with_scratch(&mut spectrum, &mut centred, &mut scratch);
            // Centre on `delay` and taper with a Blackman window (1 at the centre, 0 at tap 0).
            (0..taps)
                .map(|n| {
                    let h = centred[(n + len - delay) % len] / len as f64;
                    let x = 2.0 * std::f64::consts::PI * n as f64 / taps as f64;
                    h * (0.42 - 0.5 * x.cos() + 0.08 * (2.0 * x).cos())
                })
                .collect()
        })
        .collect();
    KernelDesign { layout, kernels, delay }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::eq::BandType;

    fn band(kind: BandType, freq: f64, gain_db: f64, q: f64) -> BandSettings {
        BandSettings { enabled: true, kind, freq, gain_db, q, ..Default::default() }
    }

    /// IIR sections followed by the correction kernel, at `f`: its response divided by the
    /// analog target, with the correction's pure delay taken out.
    fn error(bands: &[BandSettings], fs: f64, f: f64) -> C {
        let k = design(bands, fs, DELAY_48K);
        let w = 2.0 * std::f64::consts::PI * f / fs;
        let fir: C = k.kernels[0].iter().enumerate().map(|(n, &h)| C::from_polar(h, -w * n as f64)).sum();
        let iir: C = bands.iter().map(|s| design_band(s, fs).response(w)).product();
        let analog: C = bands.iter().map(|s| analog_response(s, f)).product();
        fir * iir * C::from_polar(1.0, w * DELAY_48K as f64) / analog
    }

    #[test]
    fn steep_cuts_stay_finite_and_accurate_in_their_passband() {
        let fs = 48_000.0;
        let mut low = band(BandType::LowCut, 80.0, 0.0, std::f64::consts::FRAC_1_SQRT_2);
        low.slope = 8; // 96 dB/oct
        let mut high = band(BandType::HighCut, 15_000.0, 0.0, std::f64::consts::FRAC_1_SQRT_2);
        high.slope = 3;
        let bands = [low, high];
        let k = design(&bands, fs, DELAY_48K);
        assert!(k.kernels.iter().flatten().all(|v| v.is_finite()));
        for f in [300.0, 1_000.0, 5_000.0, 10_000.0] {
            let db = 20.0 * error(&bands, fs, f).norm().log10();
            assert!(db.abs() < 0.1, "{f} Hz: {db:.3} dB off");
        }
    }

    #[test]
    fn no_bands_is_a_pure_delay() {
        let k = design(&[], 48_000.0, DELAY_48K);
        assert_eq!(k.layout, KernelLayout::Shared);
        for (n, &v) in k.kernels[0].iter().enumerate() {
            let want = if n == DELAY_48K { 1.0 } else { 0.0 };
            assert!((v - want).abs() < 1e-12, "tap {n} = {v}");
        }
    }

    #[test]
    fn matches_the_analog_magnitude_and_phase_up_to_20_khz() {
        // High bands at 44.1 kHz are where the matched IIR alone drifts (about 1 dB).
        let fs = 44_100.0;
        let bands = [band(BandType::Bell, 12_000.0, 6.0, 2.0), band(BandType::HighShelf, 9_000.0, -5.0, 0.7), band(BandType::Bell, 300.0, -4.0, 1.0)];
        for f in [100.0, 1_000.0, 5_000.0, 10_000.0, 12_000.0, 15_000.0, 18_000.0, 20_000.0] {
            let e = error(&bands, fs, f);
            let db = 20.0 * e.norm().log10();
            let deg = e.arg().to_degrees();
            assert!(db.abs() < 0.1, "{f} Hz: {db:.3} dB off the analog magnitude");
            assert!(deg.abs() < 2.0, "{f} Hz: {deg:.2}° off the analog phase");
        }
    }
}
