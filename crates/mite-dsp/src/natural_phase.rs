//! Natural Phase correction kernels: what to add after the EQ's zero-latency IIR sections so the
//! whole response matches the analog prototypes in magnitude *and* phase across the audible band,
//! at the cost of a short delay. Runs off the audio thread (allocates).
//!
//! The matched IIR sections already follow the analog magnitude closely and are minimum phase
//! like their prototypes; they part ways near Nyquist (magnitude within about a dB, phase by
//! more). The correction is the ratio analog / digital per frequency, which is 1 almost
//! everywhere. Matching it right up to Nyquist would cost pre-ringing: a real kernel's phase at
//! Nyquist is 0 or 180°, the analog phase there is anything, and that step rings on both sides.
//! So the kernel is fitted only up to 20 kHz (a weighted least-squares fit, reweighted towards
//! the smallest worst error), with few taps before its peak, and is left free above.
//! Bands combine as 2×2 complex matrices in the L/R basis, as in [`crate::linear_phase`].

use realfft::num_complex::Complex;
use realfft::RealFftPlanner;

use crate::conv::{KernelDesign, KernelLayout};
use crate::eq::{analog_response, design as design_band, BandSettings, Placement};

/// Taps before the correction's peak at 48 kHz: its delay, and all the pre-ringing it can add.
/// Scale it with the sample rate.
pub const DELAY_48K: usize = 11;
/// The correction's length at 48 kHz. Scale it with the sample rate.
pub const TAPS_48K: usize = 256;
/// The fit holds up to here; above, the kernel only loosely follows the correction.
const CARE_HZ: f64 = 20_000.0;
/// How much the fit weighs a bin above [`CARE_HZ`], against one below.
const FREE_WEIGHT: f64 = 1e-4;
/// Reweighting passes towards the smallest worst error (Lawson's algorithm).
const LAWSON_PASSES: usize = 12;
/// A bin's error counts as at least this share of the worst while reweighting.
const LAWSON_FLOOR: f64 = 1e-3;
/// The correction never boosts or cuts by more than this (linear, 12 dB): where a band drives
/// both responses towards zero (a cut's stopband), their ratio means nothing.
const MAX_CORRECTION: f64 = 4.0;
/// Where the digital response's determinant falls from 1e-6 to 1e-8 (a stereo band about 60 to
/// 80 dB down: a cut's stopband), the correction eases back to none. Both responses are tiny there
/// and their ratio means nothing; letting it drop to zero instead would put a sharp edge in the
/// correction's spectrum, which the short kernel would turn into passband ripple.
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

/// The correction for `bands` at `sample_rate`: `taps` long, peaking at tap `delay`.
pub fn design(bands: &[BandSettings], sample_rate: f64, delay: usize, taps: usize) -> KernelDesign {
    assert!(delay < taps, "the peak must lie inside the kernel");
    let enabled: Vec<&BandSettings> = bands.iter().filter(|s| s.enabled).collect();
    let sections: Vec<_> = enabled.iter().map(|s| design_band(s, sample_rate)).collect();
    // A frequency grid much finer than the kernel, so the fit holds between bins too.
    let len = (8 * taps).next_power_of_two();
    let bins = len / 2 + 1;
    let freq = |k: usize| k as f64 * sample_rate / len as f64;
    let correction: Vec<Matrix> = (0..bins)
        .map(|k| {
            let w = std::f64::consts::TAU * k as f64 / len as f64;
            let (mut analog, mut digital) = (identity(), identity());
            for (s, sec) in enabled.iter().zip(&sections) {
                analog = multiply(&band_matrix(s.placement, analog_response(s, freq(k))), &analog);
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

    // Nothing to correct: a pure delay, exactly.
    if correction.iter().all(|m| m.iter().zip(identity()).all(|(a, b)| small(a - b))) {
        let mut delta = vec![0.0; taps];
        delta[delay] = 1.0;
        return KernelDesign { layout: KernelLayout::Shared, kernels: vec![delta], delay };
    }

    // The error each bin is weighed by is relative to the correction there (the measured error
    // is relative to the analog response), taken over the entries so they share one fit.
    let scale: Vec<f64> = correction.iter().map(|m| entries.iter().map(|&e| m[e].norm_sqr()).fold(1e-6, f64::max)).collect();
    let care: Vec<bool> = (0..bins).map(|k| freq(k) <= CARE_HZ).collect();
    let fit = Fit::new(len);
    let targets: Vec<Vec<C>> = entries.iter().map(|&e| correction.iter().map(|m| m[e]).collect()).collect();
    let mut lawson: Vec<f64> = care.iter().map(|&c| if c { 1.0 } else { 0.0 }).collect();
    let mut kernels: Vec<Vec<f64>> = Vec::new();
    for pass in 0..=LAWSON_PASSES {
        let care_total: f64 = lawson.iter().sum();
        let weight: Vec<f64> = (0..bins)
            .map(|k| {
                let w = if care[k] { lawson[k] / care_total * bins as f64 } else { FREE_WEIGHT };
                w / scale[k]
            })
            .collect();
        let next: Vec<Vec<f64>> = targets.iter().map(|t| fit.solve(&weight, t, delay, taps)).collect();
        // A pass that went numerically wrong keeps the last good one (the first pass is plain
        // least squares and stays well conditioned).
        if next.iter().flatten().any(|v| !v.is_finite()) {
            break;
        }
        kernels = next;
        if pass == LAWSON_PASSES {
            break;
        }
        // Lawson: weigh each bin by its worst relative error so far, so the largest shrink.
        let mut errors = vec![0.0f64; bins];
        for (h, t) in kernels.iter().zip(&targets) {
            for (k, (r, c)) in fit.response(h, delay).iter().zip(t).enumerate() {
                errors[k] = errors[k].max((r - c).norm() / scale[k].sqrt());
            }
        }
        let worst = errors.iter().zip(&care).filter(|(_, c)| **c).map(|(e, _)| *e).fold(0.0, f64::max);
        // Already exact (bands the IIR matches, as low ones at high rates): nothing to even out.
        if worst < 1e-9 {
            break;
        }
        // Bins already far better than the worst keep a floor, so the weights' spread (and the
        // system's conditioning) stays bounded.
        for (l, (e, &c)) in lawson.iter_mut().zip(errors.iter().zip(&care)) {
            if c {
                *l *= e.max(LAWSON_FLOOR * worst) / worst;
            }
        }
    }
    if kernels.is_empty() {
        // Nothing solved cleanly: no correction, just the delay.
        let mut delta = vec![0.0; taps];
        delta[delay] = 1.0;
        return KernelDesign { layout: KernelLayout::Shared, kernels: vec![delta], delay };
    }
    KernelDesign { layout, kernels, delay }
}

/// Weighted least-squares fits of a kernel to a response on an `len`-point grid.
struct Fit {
    len: usize,
    forward: std::sync::Arc<dyn realfft::RealToComplex<f64>>,
    inverse: std::sync::Arc<dyn realfft::ComplexToReal<f64>>,
}

impl Fit {
    fn new(len: usize) -> Self {
        let mut planner = RealFftPlanner::<f64>::new();
        Self { len, forward: planner.plan_fft_forward(len), inverse: planner.plan_fft_inverse(len) }
    }

    /// `Σₖ weightₖ · ½ (W(ωₖ) e^{jωₖn} + conj)` for every lag `n`: the inverse transform of a
    /// half spectrum, DC and Nyquist counted once and the rest twice.
    fn correlate(&self, spectrum: &[C]) -> Vec<f64> {
        let mut input = spectrum.to_vec();
        input[0].im = 0.0;
        input[self.len / 2].im = 0.0;
        let mut out = self.inverse.make_output_vec();
        // Buffers are sized by the planner and DC/Nyquist are real: this cannot fail.
        let _ = self.inverse.process(&mut input, &mut out);
        out
    }

    /// The kernel (`taps` long, its tap `delay` at time 0) minimising
    /// `Σₖ weightₖ |H(ωₖ) − targetₖ|²`.
    fn solve(&self, weight: &[f64], target: &[C], delay: usize, taps: usize) -> Vec<f64> {
        // Normal equations: a symmetric Toeplitz matrix of the weights' correlation, and the
        // target's correlation with each tap's delay.
        let r = self.correlate(&weight.iter().map(|&w| C::new(w, 0.0)).collect::<Vec<_>>());
        let p = self.correlate(&weight.iter().zip(target).map(|(&w, &t)| t * w).collect::<Vec<_>>());
        let mut r = r[..taps].to_vec();
        // A whisper of ridge regression: the free band leaves the system nearly singular.
        r[0] *= 1.0 + 1e-9;
        let b: Vec<f64> = (0..taps).map(|n| p[(n + self.len - delay) % self.len]).collect();
        levinson(&r, &b)
    }

    /// The kernel's response on the grid, with the pure delay of `delay` taken out.
    fn response(&self, h: &[f64], delay: usize) -> Vec<C> {
        let mut frame = self.forward.make_input_vec();
        for (n, &v) in h.iter().enumerate() {
            frame[(n + self.len - delay) % self.len] = v;
        }
        let mut out = self.forward.make_output_vec();
        let _ = self.forward.process(&mut frame, &mut out);
        out
    }
}

/// Solves `T x = b` for the symmetric Toeplitz `T` whose first row is `r` (Levinson).
fn levinson(r: &[f64], b: &[f64]) -> Vec<f64> {
    let n = b.len();
    // `f` solves T f = e₀ at each order; its reverse solves T g = e_last.
    let mut f = vec![0.0; n];
    let mut next = vec![0.0; n];
    let mut x = vec![0.0; n];
    f[0] = 1.0 / r[0];
    x[0] = b[0] / r[0];
    for k in 1..n {
        let ef: f64 = (0..k).map(|i| r[k - i] * f[i]).sum();
        let denom = 1.0 - ef * ef;
        for i in 0..=k {
            let fe = if i < k { f[i] } else { 0.0 };
            let be = if i > 0 { f[k - i] } else { 0.0 };
            next[i] = (fe - ef * be) / denom;
        }
        f[..=k].copy_from_slice(&next[..=k]);
        let ex: f64 = (0..k).map(|i| r[k - i] * x[i]).sum();
        let g = b[k] - ex;
        for i in 0..=k {
            x[i] += g * f[k - i];
        }
    }
    x
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
        let k = design(bands, fs, DELAY_48K, TAPS_48K);
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
        let k = design(&bands, fs, DELAY_48K, TAPS_48K);
        assert!(k.kernels.iter().flatten().all(|v| v.is_finite()));
        for f in [300.0, 1_000.0, 5_000.0, 10_000.0] {
            let db = 20.0 * error(&bands, fs, f).norm().log10();
            assert!(db.abs() < 0.1, "{f} Hz: {db:.3} dB off");
        }
    }

    #[test]
    fn no_bands_is_a_pure_delay() {
        let k = design(&[], 48_000.0, DELAY_48K, TAPS_48K);
        assert_eq!(k.layout, KernelLayout::Shared);
        for (n, &v) in k.kernels[0].iter().enumerate() {
            let want = if n == DELAY_48K { 1.0 } else { 0.0 };
            assert!((v - want).abs() < 1e-9, "tap {n} = {v}");
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

    #[test]
    fn low_and_high_bands_stay_finite_and_accurate_at_every_rate() {
        // At high rates a low band's correction is all but 1: the reweighting must not run away.
        for fs in [44_100.0, 48_000.0, 96_000.0, 192_000.0] {
            let ratio = ((fs / 48_000.0f64).round().max(1.0) as usize).next_power_of_two();
            for b in [band(BandType::Bell, 100.0, 12.0, 8.0), band(BandType::LowShelf, 100.0, 6.0, 0.7), band(BandType::Bell, 10_000.0, 12.0, 2.0)] {
                let k = design(&[b], fs, DELAY_48K * ratio, TAPS_48K * ratio);
                assert!(k.kernels.iter().flatten().all(|v| v.is_finite()), "{fs} Hz, {:?} at {}", b.kind, b.freq);
                let w = |f: f64| 2.0 * std::f64::consts::PI * f / fs;
                for f in [50.0, 100.0, 1_000.0, 10_000.0, 18_000.0] {
                    let fir: C = k.kernels[0].iter().enumerate().map(|(n, &h)| C::from_polar(h, -w(f) * n as f64)).sum();
                    let e = fir * design_band(&b, fs).response(w(f)) * C::from_polar(1.0, w(f) * k.delay as f64) / analog_response(&b, f);
                    assert!((20.0 * e.norm().log10()).abs() < 0.05, "{fs} Hz, {:?} at {}: {f} Hz off", b.kind, b.freq);
                }
            }
        }
    }
}
