//! Zero Latency correction: what to add after the matched IIR bands so their magnitude follows
//! the analog prototypes up to Nyquist, without adding latency. Runs off the audio thread
//! (allocates).
//!
//! One matched biquad per band cannot follow the analog curve everywhere near Nyquist (it matches
//! three frequencies exactly and parts ways between them: by a dB or more for bells and shelves
//! in the top octave). The residual analog / digital, in magnitude only, becomes a
//! **minimum-phase** FIR (real cepstrum, folded): causal, front-loaded, no delay. It corrects the
//! magnitude, and the phase follows from it: a minimum-phase response's phase is fixed by its
//! magnitude at every frequency. The analog prototype's phase depends on its curve above Nyquist
//! too, which a digital filter does not have, so with the analog magnitude up to Nyquist the
//! phase would be some 20° off near 20 kHz. Above 20 kHz the magnitude is free (inaudible), so the
//! correction shapes it there (a few raised-cosine bumps, fitted by least squares, reweighted
//! towards the smallest worst error) to pull the phase below 20 kHz towards the analog phase,
//! as far as the kernel can still hold the magnitude. Matching the phase exactly is Natural
//! Phase's job.
//!
//! Bands on the left, right or both channels are corrected per channel; mid and side bands in the
//! mid/side domain, after them (the two corrections are exact for each group and, together,
//! for every chain that does not mix mid/side bands with one-sided ones).

use realfft::RealFftPlanner;
use realfft::num_complex::Complex;

use crate::eq::{
    BandSettings, Placement, analog_response, analog_response_db, design as design_band,
};

/// The residual is never corrected by more than this, dB (the gain range; a resonant shelf near
/// Nyquist can be that far off). Stopbands are handled separately below.
const MAX_CORRECTION_DB: f64 = 30.0;
/// The kernel lengths tried, shortest first, at 48 kHz (scaled with the sample rate): the
/// correction uses the shortest that brings the response within [`TARGET_DB`] of the analog
/// one where it is audible (within 24 dB of unity), or the longest.
pub const TAPS_48K: [usize; 4] = [64, 128, 256, 512];
const TARGET_DB: f64 = 0.05;
const AUDIBLE_DB: f64 = -24.0;
/// Where the analog response falls from −40 to −60 dB (a cut's stopband), the correction eases
/// back to none.
const STOPBAND_FULL_DB: f64 = -40.0;
const STOPBAND_NONE_DB: f64 = -60.0;
/// A correction whose residual stays inside this everywhere is not worth running, dB.
const NEGLIGIBLE_DB: f64 = 0.005;
/// Above this the magnitude is free, and shaped to bring the phase below it closer to analog.
const FREE_HZ: f64 = 20_000.0;
/// Raised-cosine bumps the free band is shaped with.
const FREE_BUMPS: usize = 8;
/// The free band never moves further than this from the analog curve, dB.
const FREE_LIMIT_DB: f64 = 40.0;
/// Ridge strengths tried for the free-band fit, boldest first: the first whose kernel still
/// meets [`TARGET_DB`] is used (or none).
const FREE_RIDGE: [f64; 4] = [1e-3, 1e-2, 1e-1, 1.0];
/// The fit reads every this many bins (the phase is smooth on the design grid).
const FIT_STRIDE: usize = 8;
/// A fit this close to the analog magnitude counts as exact, dB.
const EXACT_DB: f64 = 0.01;
/// The shortest fit within this of the best phase error is used, radians (2.5°).
const PHASE_SLACK: f64 = 0.043_633_2;
/// Zeros at Nyquist tried (each half a sample of delay below the free band).
const NYQUIST_ZEROS: usize = 3;
/// Reweighting passes towards the smallest worst phase error (Lawson's algorithm).
const LAWSON_PASSES: usize = 4;
/// The phase is fitted where the curve is within this of its peak, dB.
const PHASE_RANGE_DB: f64 = 30.0;

/// Minimum-phase spectra from log magnitudes on the `n/2 + 1` bins of an `n`-point FFT (the
/// FFTs planned once).
struct Cepstrum {
    n: usize,
    forward: std::sync::Arc<dyn realfft::RealToComplex<f64>>,
    inverse: std::sync::Arc<dyn realfft::ComplexToReal<f64>>,
}

impl Cepstrum {
    fn new(n: usize) -> Self {
        let mut planner = RealFftPlanner::<f64>::new();
        Self {
            n,
            forward: planner.plan_fft_forward(n),
            inverse: planner.plan_fft_inverse(n),
        }
    }

    /// The log of the minimum-phase spectrum with magnitude `exp(log_mag)`: `log_mag + j·phase`.
    fn log_spectrum(&self, log_mag: &[f64]) -> Vec<Complex<f64>> {
        let n = self.n;
        // Real cepstrum of the (real, even) log magnitude.
        let mut spectrum: Vec<Complex<f64>> =
            log_mag.iter().map(|&v| Complex::new(v, 0.0)).collect();
        let mut cepstrum = vec![0.0; n];
        self.inverse
            .process(&mut spectrum, &mut cepstrum)
            .expect("sizes match");
        let scale = 1.0 / n as f64;
        // Fold onto positive quefrencies: the minimum-phase spectrum's log.
        for (i, c) in cepstrum.iter_mut().enumerate() {
            *c *= scale
                * match i {
                    0 => 1.0,
                    i if i < n / 2 => 2.0,
                    i if i == n / 2 => 1.0,
                    _ => 0.0,
                };
        }
        let mut log_min = vec![Complex::new(0.0, 0.0); n / 2 + 1];
        self.forward
            .process(&mut cepstrum, &mut log_min)
            .expect("sizes match");
        log_min
    }

    /// The phase (radians, per bin) of the minimum-phase spectrum with magnitude `exp(log_mag)`.
    fn phase(&self, log_mag: &[f64]) -> Vec<f64> {
        self.log_spectrum(log_mag).iter().map(|z| z.im).collect()
    }

    /// The minimum-phase impulse response with magnitude `exp(log_mag)`, all `n` samples.
    fn impulse(&self, log_mag: &[f64]) -> Vec<f64> {
        let n = self.n;
        let mut min: Vec<Complex<f64>> =
            self.log_spectrum(log_mag).iter().map(|z| z.exp()).collect();
        // The first and last bins of a real signal's spectrum are real.
        min[0].im = 0.0;
        min[n / 2].im = 0.0;
        let mut h = vec![0.0; n];
        self.inverse.process(&mut min, &mut h).expect("sizes match");
        let scale = 1.0 / n as f64;
        h.iter_mut().for_each(|v| *v *= scale);
        h
    }
}

/// The first `taps` of impulse response `h`, the last quarter tapered so the truncation does not
/// ring.
fn truncate(h: &[f64], taps: usize) -> Vec<f64> {
    let taper = taps / 4;
    (0..taps.min(h.len()))
        .map(|i| {
            let from_end = taps - i;
            let w = if from_end <= taper {
                0.5 - 0.5 * (std::f64::consts::PI * from_end as f64 / taper as f64).cos()
            } else {
                1.0
            };
            h[i] * w
        })
        .collect()
}

/// The minimum-phase FIR (`taps` long) whose magnitude is `exp(log_mag)`, `log_mag` holding the
/// natural log of the magnitude at the `n/2 + 1` bins of an `n`-point FFT. Its tail is tapered
/// over the last quarter so the truncation does not ring.
pub fn min_phase_fir(log_mag: &[f64], taps: usize) -> Vec<f64> {
    truncate(
        &Cepstrum::new(2 * (log_mag.len() - 1)).impulse(log_mag),
        taps,
    )
}

/// The correction kernels: per channel (left, right; mono uses the left), and, when mid or side
/// bands are present, per mid/side after them.
#[derive(Debug, Clone, PartialEq)]
pub struct Correction {
    pub lr: [Vec<f32>; 2],
    pub ms: Option<[Vec<f32>; 2]>,
    /// The taps past the first `block`, per stream (left, right, mid, side), cut into
    /// `block`-long partitions and transformed for [`CorrectionFilter`]'s FFT stage (with the
    /// inverse FFT's scale folded in). Empty where a kernel fits in the block.
    tails: [Vec<Complex<f32>>; 4],
    block: usize,
}

/// `h` convolved with `((1 + z⁻¹) / 2)^zeros`.
fn with_nyquist_zeros(h: &[f64], zeros: usize) -> Vec<f64> {
    let mut out = h.to_vec();
    for _ in 0..zeros {
        out.push(0.0);
        for i in (1..out.len()).rev() {
            out[i] = 0.5 * (out[i] + out[i - 1]);
        }
        out[0] *= 0.5;
    }
    out
}

/// Solves `a x = b` for a small symmetric positive-definite `a` (`n × n`, row-major; Cholesky).
fn solve_spd(mut a: Vec<f64>, mut b: Vec<f64>) -> Vec<f64> {
    let n = b.len();
    for j in 0..n {
        let d = (a[j * n + j] - (0..j).map(|k| a[j * n + k] * a[j * n + k]).sum::<f64>())
            .max(1e-300)
            .sqrt();
        a[j * n + j] = d;
        for i in j + 1..n {
            a[i * n + j] =
                (a[i * n + j] - (0..j).map(|k| a[i * n + k] * a[j * n + k]).sum::<f64>()) / d;
        }
    }
    for i in 0..n {
        b[i] = (b[i] - (0..i).map(|k| a[i * n + k] * b[k]).sum::<f64>()) / a[i * n + i];
    }
    for i in (0..n).rev() {
        b[i] = (b[i] - (i + 1..n).map(|k| a[k * n + i] * b[k]).sum::<f64>()) / a[i * n + i];
    }
    b
}

/// The free-band fit: bumps above [`FREE_HZ`] (`basis`) and the phase each adds below it
/// (`phases`), against the phase still missing (`missing`, where `weight` is non-zero).
struct FreeBand {
    basis: Vec<Vec<f64>>,
    phases: Vec<Vec<f64>>,
}

impl FreeBand {
    /// `None` when the sample rate leaves no free band.
    fn new(cepstrum: &Cepstrum, sample_rate: f64) -> Option<Self> {
        let bins = cepstrum.n / 2 + 1;
        let nyquist = sample_rate / 2.0;
        if nyquist <= FREE_HZ {
            return None;
        }
        let fft = cepstrum.n;
        let width = (nyquist - FREE_HZ) / FREE_BUMPS as f64;
        // Bumps j = 1…n centred `width` apart from the free band's edge, each spanning a width
        // either side: zero, and flat, at the edge, so the audible band is left alone.
        let basis: Vec<Vec<f64>> = (1..=FREE_BUMPS)
            .map(|j| {
                let centre = FREE_HZ + j as f64 * width;
                (0..bins)
                    .map(|k| {
                        let f = k as f64 * sample_rate / fft as f64;
                        let u = (f - centre) / width;
                        if f > FREE_HZ && u.abs() < 1.0 {
                            0.5 + 0.5 * (std::f64::consts::PI * u).cos()
                        } else {
                            0.0
                        }
                    })
                    .collect()
            })
            .collect();
        let phases = basis.iter().map(|b| cepstrum.phase(b)).collect();
        Some(Self { basis, phases })
    }

    /// The bumps' weights that best supply `missing` (radians) where `weight` > 0, at `ridge`.
    fn fit(&self, missing: &[f64], weight: &[f64], ridge: f64) -> Vec<f64> {
        let n = self.basis.len();
        let mut lawson = weight.to_vec();
        let mut coef = vec![0.0; n];
        for _ in 0..LAWSON_PASSES {
            let total = lawson.iter().step_by(FIT_STRIDE).sum::<f64>().max(1e-300);
            let mut a = vec![0.0; n * n];
            let mut b = vec![0.0; n];
            for (k, &l) in lawson
                .iter()
                .enumerate()
                .step_by(FIT_STRIDE)
                .filter(|(_, l)| **l > 0.0)
            {
                let w = l / total;
                for i in 0..n {
                    b[i] += w * self.phases[i][k] * missing[k];
                    for j in 0..n {
                        a[i * n + j] += w * self.phases[i][k] * self.phases[j][k];
                    }
                }
            }
            for i in 0..n {
                a[i * n + i] += ridge;
            }
            coef = solve_spd(a, b);
            for (k, l) in lawson
                .iter_mut()
                .enumerate()
                .step_by(FIT_STRIDE)
                .filter(|(_, l)| **l > 0.0)
            {
                let e = missing[k] - (0..n).map(|i| coef[i] * self.phases[i][k]).sum::<f64>();
                *l *= e.abs().max(1e-9);
            }
        }
        coef
    }
}

/// The correction for `bands` at `sample_rate`: minimum-phase FIRs of one of `lengths` (the
/// shortest that meets the target, per group), designed on an `fft`-point grid. `mono`: every
/// band processes the one channel. `None` when the bands already match their prototypes (no
/// bands, or all far from Nyquist) and nothing needs to run.
pub fn zero_latency_correction(
    bands: &[BandSettings],
    sample_rate: f64,
    lengths: &[usize],
    fft: usize,
    mono: bool,
) -> Option<Correction> {
    let bins = fft / 2 + 1;
    let active: Vec<_> = bands
        .iter()
        .filter(|s| s.enabled)
        .map(|s| (s, design_band(s, sample_rate)))
        .collect();
    if active.is_empty() {
        return None;
    }
    // Residual in dB per group (left, right, mid, side), and the group's analog response.
    let mut residual = vec![[0.0f64; 4]; bins];
    let mut analog_total = vec![[0.0f64; 4]; bins];
    // The phase the group's analog response has over its digital one, as a unit phasor.
    let mut missing_phase = vec![[Complex::new(1.0, 0.0); 4]; bins];
    let ln_db = std::f64::consts::LN_10 / 20.0;
    for (k, ((r, total), missing)) in residual
        .iter_mut()
        .zip(analog_total.iter_mut())
        .zip(missing_phase.iter_mut())
        .enumerate()
    {
        // Bin 0 is DC, where analog and digital agree exactly; evaluate just above it.
        let f = (k as f64 * sample_rate / fft as f64).max(1.0);
        let w = 2.0 * std::f64::consts::PI * f / sample_rate;
        for (s, sections) in &active {
            let analog = analog_response_db(s, f);
            let digital = sections.response_db(w.min(std::f64::consts::PI));
            let ease = ((analog - STOPBAND_NONE_DB) / (STOPBAND_FULL_DB - STOPBAND_NONE_DB))
                .clamp(0.0, 1.0);
            let d = if analog.is_finite() && digital.is_finite() {
                (analog - digital) * ease
            } else {
                0.0
            };
            let groups: &[usize] = match (mono, s.placement) {
                (true, _) | (_, Placement::Stereo) => &[0, 1],
                (_, Placement::Left) => &[0],
                (_, Placement::Right) => &[1],
                (_, Placement::Mid) => &[2],
                (_, Placement::Side) => &[3],
            };
            let ratio = analog_response(s, f) / sections.response(w.min(std::f64::consts::PI));
            let turn = if ratio.norm() > 0.0 && ratio.norm().is_finite() {
                ratio / ratio.norm()
            } else {
                Complex::new(1.0, 0.0)
            };
            for &g in groups {
                missing[g] *= turn;
                r[g] += d;
                total[g] += if analog.is_finite() { analog } else { -300.0 };
            }
        }
    }
    let mut planner = RealFftPlanner::<f64>::new();
    let forward = planner.plan_fft_forward(fft);
    // A kernel's worst error against the residual where the group is audible (and up to the free
    // band), dB.
    let error = |h: &[f64], g: usize| {
        let mut padded = vec![0.0; fft];
        padded[..h.len()].copy_from_slice(h);
        let mut spectrum = vec![Complex::new(0.0, 0.0); bins];
        forward
            .process(&mut padded, &mut spectrum)
            .expect("sizes match");
        spectrum
            .iter()
            .zip(&residual)
            .zip(&analog_total)
            .enumerate()
            .filter(|(k, (_, total))| {
                total[g] > AUDIBLE_DB && *k as f64 * sample_rate / fft as f64 <= FREE_HZ
            })
            .map(|(_, ((z, r), _))| {
                (20.0 * z.norm().max(1e-12).log10()
                    - r[g].clamp(-MAX_CORRECTION_DB, MAX_CORRECTION_DB))
                .abs()
            })
            .fold(0.0, f64::max)
    };
    let cepstrum = Cepstrum::new(fft);
    let free = FreeBand::new(&cepstrum, sample_rate);
    // The shortest of `lengths` that meets the target for `log_mag`, if any.
    let fit = |log_mag: &[f64], g: usize| {
        let h = cepstrum.impulse(log_mag);
        lengths
            .iter()
            .map(|&taps| truncate(&h, taps))
            .find(|k| error(k, g) <= TARGET_DB)
    };
    // A kernel's worst phase error against what the group misses, where `weight` > 0, radians.
    let phase_error = |h: &[f64], g: usize, weight: &[f64]| {
        let mut padded = vec![0.0; fft];
        padded[..h.len()].copy_from_slice(h);
        let mut spectrum = vec![Complex::new(0.0, 0.0); bins];
        forward
            .process(&mut padded, &mut spectrum)
            .expect("sizes match");
        spectrum
            .iter()
            .zip(weight)
            .zip(&missing_phase)
            .filter(|((_, w), _)| **w > 0.0)
            .map(|((z, _), m)| (m[g] / z).arg().abs())
            .fold(0.0, f64::max)
    };
    let kernel = |g: usize| -> Option<Vec<f32>> {
        let worst = residual.iter().map(|r| r[g].abs()).fold(0.0, f64::max);
        (worst > NEGLIGIBLE_DB).then(|| {
            let log_mag: Vec<f64> = residual
                .iter()
                .map(|r| r[g].clamp(-MAX_CORRECTION_DB, MAX_CORRECTION_DB) * ln_db)
                .collect();
            let shaped = free.as_ref().and_then(|free| {
                let top = analog_total.iter().map(|t| t[g]).fold(f64::MIN, f64::max);
                // Where the phase counts: below the free band, where the curve is within range of
                // its peak, on a log-frequency footing (as the curve is drawn and heard).
                let weight: Vec<f64> = (0..bins)
                    .map(|k| {
                        let f = k as f64 * sample_rate / fft as f64;
                        if (20.0..=FREE_HZ).contains(&f)
                            && analog_total[k][g] > top - PHASE_RANGE_DB
                        {
                            1.0 / f
                        } else {
                            0.0
                        }
                    })
                    .collect();
                let limit = FREE_LIMIT_DB * ln_db;
                // Every fit that holds the magnitude, with its worst phase error.
                let mut held: Vec<(f64, Vec<f64>)> = Vec::new();
                // The plain magnitude fit is a candidate too (usually the shortest).
                if let Some(h) = fit(&log_mag, g) {
                    let score = phase_error(&h, g, &weight);
                    // Already within the slack and exact: no shaping could be preferred to it.
                    if score <= PHASE_SLACK && error(&h, g) <= EXACT_DB {
                        return Some(h);
                    }
                    held.push((score, h));
                }
                let mut best = f64::INFINITY;
                for zeros in 0..=NYQUIST_ZEROS {
                    let mut count_best = f64::INFINITY;
                    // `zeros` zeros at Nyquist, ((1 + z⁻¹)/2)^zeros: half a sample of delay each,
                    // their droop made up below the free band (and held above it).
                    let droop = |k: usize| {
                        let f = (k as f64 * sample_rate / fft as f64).min(FREE_HZ);
                        zeros as f64 * (std::f64::consts::PI * f / sample_rate).cos().ln()
                    };
                    let base_mag: Vec<f64> = log_mag
                        .iter()
                        .enumerate()
                        .map(|(k, &v)| v - droop(k))
                        .collect();
                    let base = cepstrum.phase(&base_mag);
                    // The phase still missing, unwrapped along frequency.
                    let mut missing = vec![0.0; bins];
                    let mut previous = 0.0f64;
                    for (k, m) in missing.iter_mut().enumerate() {
                        let w = std::f64::consts::PI * k as f64 / (bins - 1) as f64;
                        let mut v = missing_phase[k][g].arg() - base[k] + zeros as f64 * w / 2.0;
                        v -= std::f64::consts::TAU
                            * ((v - previous) / std::f64::consts::TAU).round();
                        *m = v;
                        previous = v;
                    }
                    for &ridge in &FREE_RIDGE {
                        let coef = free.fit(&missing, &weight, ridge);
                        let shaped: Vec<f64> = base_mag
                            .iter()
                            .enumerate()
                            .map(|(k, &v)| {
                                v + (0..coef.len())
                                    .map(|i| coef[i] * free.basis[i][k])
                                    .sum::<f64>()
                                    .clamp(-limit, limit)
                            })
                            .collect();
                        let impulse = cepstrum.impulse(&shaped);
                        let found = lengths
                            .iter()
                            .map(|&taps| {
                                with_nyquist_zeros(&truncate(&impulse, taps - zeros), zeros)
                            })
                            .find(|h| error(h, g) <= TARGET_DB);
                        if let Some(h) = found {
                            let score = phase_error(&h, g, &weight);
                            count_best = count_best.min(score);
                            let shortest = h.len() <= lengths[0];
                            held.push((score, h));
                            // Weaker fits can only be as short: no need to try them.
                            if shortest {
                                break;
                            }
                        }
                    }
                    // One more zero no longer helps: neither will more.
                    if count_best >= best {
                        break;
                    }
                    best = count_best;
                }
                // The shortest kernel within a degree of the best phase: longer ones cost more
                // to run for a difference nobody hears.
                let best = held
                    .iter()
                    .map(|(score, _)| *score)
                    .fold(f64::INFINITY, f64::min);
                // Of those, the shortest that is also exact in magnitude, if one is.
                let near: Vec<(f64, Vec<f64>)> = held
                    .into_iter()
                    .filter(|(score, _)| *score <= best + PHASE_SLACK)
                    .collect();
                let exact = near
                    .iter()
                    .filter(|(_, h)| error(h, g) <= EXACT_DB)
                    .min_by_key(|(_, h)| h.len());
                exact
                    .or_else(|| near.iter().min_by_key(|(_, h)| h.len()))
                    .map(|(_, h)| h.clone())
            });
            let best = shaped.or_else(|| fit(&log_mag, g)).unwrap_or_else(|| {
                truncate(&cepstrum.impulse(&log_mag), lengths[lengths.len() - 1])
            });
            best.into_iter().map(|v| v as f32).collect()
        })
    };
    let identity = || vec![1.0f32];
    // Left and right often see the same bands: design once.
    let same_lr =
        residual.iter().all(|r| r[0] == r[1]) && missing_phase.iter().all(|m| m[0] == m[1]);
    let l = kernel(0);
    let r = if same_lr { l.clone() } else { kernel(1) };
    let (m, sd) = (kernel(2), kernel(3));
    if l.is_none() && r.is_none() && m.is_none() && sd.is_none() {
        return None;
    }
    let ms = (m.is_some() || sd.is_some())
        .then(|| [m.unwrap_or_else(identity), sd.unwrap_or_else(identity)]);
    Some(Correction::new(
        [l.unwrap_or_else(identity), r.unwrap_or_else(identity)],
        ms,
    ))
}

/// Kernel lengths the audio-side filter is sized for at `rate_ratio` × 48 kHz.
pub fn max_taps(rate_ratio: usize) -> usize {
    TAPS_48K[TAPS_48K.len() - 1] * rate_ratio.max(1)
}

impl Correction {
    /// Kernels per channel and, optionally, per mid/side after them; not yet partitioned.
    pub fn new(lr: [Vec<f32>; 2], ms: Option<[Vec<f32>; 2]>) -> Self {
        Self {
            lr,
            ms,
            tails: Default::default(),
            block: 0,
        }
    }

    /// Passes the signal unchanged (a single unit tap everywhere, no mid/side stage).
    pub fn identity() -> Self {
        Self::new([vec![1.0], vec![1.0]], None)
    }

    fn is_identity(&self) -> bool {
        self.ms.is_none() && self.lr.iter().all(|k| k.len() == 1 && k[0] == 1.0)
    }

    fn kernels(&self) -> [Option<&[f32]>; 4] {
        let ms = self.ms.as_ref();
        [
            Some(&self.lr[0][..]),
            Some(&self.lr[1][..]),
            ms.map(|m| &m[0][..]),
            ms.map(|m| &m[1][..]),
        ]
    }

    /// Prepares the kernels for a [`CorrectionFilter`] running `block`-sample partitions: the
    /// taps past the first block, transformed. Allocates (call it where the kernel is designed).
    pub fn partitioned(mut self, block: usize) -> Self {
        let block = block.max(1);
        let mut planner = RealFftPlanner::<f32>::new();
        let fft = planner.plan_fft_forward(2 * block);
        let mut frame = fft.make_input_vec();
        let mut spectrum = fft.make_output_vec();
        let scale = 1.0 / (2 * block) as f32;
        let tails: [Vec<Complex<f32>>; 4] = std::array::from_fn(|stream| {
            let Some(k) = self.kernels()[stream] else {
                return Vec::new();
            };
            if k.len() <= block {
                return Vec::new();
            }
            let tail = &k[block..];
            let mut out = Vec::with_capacity(tail.len().div_ceil(block) * (block + 1));
            for part in tail.chunks(block) {
                frame.fill(0.0);
                frame[..part.len()].copy_from_slice(part);
                // Buffers are sized by the planner: this cannot fail.
                let _ = fft.process(&mut frame, &mut spectrum);
                out.extend(spectrum.iter().map(|z| z * scale));
            }
            out
        });
        self.tails = tails;
        self.block = block;
        self
    }

    fn partitions(&self, stream: usize) -> usize {
        if self.block == 0 {
            0
        } else {
            self.tails[stream].len() / (self.block + 1)
        }
    }

    /// Whether every kernel fits a filter running `block`-sample partitions with room for
    /// `max_partitions`.
    fn fits(&self, block: usize, max_partitions: usize) -> bool {
        let long = self.kernels().iter().flatten().any(|k| k.len() > block);
        (!long || self.block == block) && (0..4).all(|s| self.partitions(s) <= max_partitions)
    }
}

/// `h · x` over the kernel, in eight lanes so it vectorises.
#[inline]
fn dot(h: &[f32], x: &[f32]) -> f32 {
    let n = h.len().min(x.len());
    let (h, x) = (&h[..n], &x[..n]);
    let mut acc = [0.0f32; 8];
    let chunks = n / 8;
    for (hc, xc) in h.chunks_exact(8).zip(x.chunks_exact(8)) {
        for i in 0..8 {
            acc[i] += hc[i] * xc[i];
        }
    }
    let mut sum = acc.iter().sum::<f32>();
    for i in chunks * 8..n {
        sum += h[i] * x[i];
    }
    sum
}

/// The audio side of the correction: minimum-phase FIRs with no latency, per channel and then
/// per mid/side. Each kernel's first `block` taps run in direct form; the rest run as a uniformly
/// partitioned FFT convolution of `block`-sample partitions, whose one block of latency is
/// exactly where those taps start, so the sum is the whole kernel with no delay. A new kernel
/// crossfades in over `fade` samples (both run in full meanwhile). Kernels arrive and leave as
/// boxes so the audio thread never allocates or frees. Allocation-free after construction.
pub struct CorrectionFilter {
    kernel: Option<Box<Correction>>,
    previous: Option<Box<Correction>>,
    fade_len: usize,
    faded: usize,
    block: usize,
    max_partitions: usize,
    /// Input history per stream (left/right, then mid/side), newest first, stored twice so any
    /// block-long window is one contiguous slice: the direct-form part.
    history: [Vec<f32>; 4],
    pos: usize,
    /// The FFT stage: per stream, the last two input blocks (the overlap-save frame), how far
    /// into the newer one the input is, and the delay line of past frames' spectra.
    frames: [Vec<f32>; 4],
    fill: usize,
    spectra: [Vec<Complex<f32>>; 4],
    newest: usize,
    /// Whether `spectra` is kept up to date (only while some kernel has a tail; otherwise the
    /// transforms are skipped and `past`, the last blocks as they were, rebuilds it on demand).
    live: bool,
    past: [Vec<f32>; 4],
    past_pos: usize,
    /// Whether the mid/side streams are being fed.
    ms_fed: bool,
    /// Per kernel (current, previous) and stream: the tail's output for the block under way.
    tail_out: [[Vec<f32>; 4]; 2],
    fft: std::sync::Arc<dyn realfft::RealToComplex<f32>>,
    ifft: std::sync::Arc<dyn realfft::ComplexToReal<f32>>,
    frame: Vec<f32>,
    acc: Vec<Complex<f32>>,
    scratch: Vec<Complex<f32>>,
}

impl CorrectionFilter {
    /// Sized for kernels up to `max_taps`, running `block`-sample partitions past the first
    /// block, crossfading over `fade` samples. Allocates.
    pub fn new(max_taps: usize, block: usize, fade: usize) -> Self {
        let block = block.max(1);
        let max_partitions = max_taps.saturating_sub(block).div_ceil(block).max(1);
        let mut planner = RealFftPlanner::<f32>::new();
        let fft = planner.plan_fft_forward(2 * block);
        let ifft = planner.plan_fft_inverse(2 * block);
        let scratch_len = fft.get_scratch_len().max(ifft.get_scratch_len());
        Self {
            kernel: None,
            previous: None,
            fade_len: fade.max(1),
            faded: 0,
            block,
            max_partitions,
            history: std::array::from_fn(|_| vec![0.0; 2 * block]),
            pos: 0,
            frames: std::array::from_fn(|_| vec![0.0; 2 * block]),
            fill: 0,
            spectra: std::array::from_fn(|_| {
                vec![Complex::new(0.0, 0.0); max_partitions * (block + 1)]
            }),
            newest: 0,
            live: false,
            past: std::array::from_fn(|_| vec![0.0; (max_partitions + 1) * block]),
            past_pos: 0,
            ms_fed: false,
            tail_out: std::array::from_fn(|_| std::array::from_fn(|_| vec![0.0; block])),
            frame: vec![0.0; 2 * block],
            acc: vec![Complex::new(0.0, 0.0); block + 1],
            scratch: vec![Complex::new(0.0, 0.0); scratch_len],
            fft,
            ifft,
        }
    }

    pub fn reset(&mut self) {
        for v in self
            .history
            .iter_mut()
            .chain(self.frames.iter_mut())
            .chain(self.past.iter_mut())
            .chain(self.tail_out.iter_mut().flatten())
        {
            v.fill(0.0);
        }
        for s in &mut self.spectra {
            s.fill(Complex::new(0.0, 0.0));
        }
        self.pos = 0;
        self.fill = 0;
        self.newest = 0;
        self.past_pos = 0;
    }

    /// Whether a crossfade is under way (a new kernel cannot be taken until it ends).
    pub fn is_fading(&self) -> bool {
        self.previous.is_some()
    }

    /// Takes `kernel` (crossfading from the current one). Hands back the kernel it no longer
    /// needs (the one faded out), or `kernel` itself while a crossfade is still running or when it
    /// does not fit this filter (longer than it was sized for, or partitioned for another block).
    pub fn offer(&mut self, kernel: Box<Correction>) -> Option<Box<Correction>> {
        if self.previous.is_some() || !kernel.fits(self.block, self.max_partitions) {
            return Some(kernel);
        }
        self.previous = self.kernel.replace(kernel);
        self.faded = 0;
        if self.previous.is_none() {
            // From nothing: fade in from the plain signal.
            self.previous = Some(Box::new(Correction::identity()));
        }
        // The old kernel's tail output moves to the previous slot; the new one's, for the block
        // under way, comes from the input already in the delay line.
        let [current, previous] = &mut self.tail_out;
        std::mem::swap(current, previous);
        if self.needs_tails() && !self.live {
            self.rebuild();
        }
        self.run_tails(0);
        None
    }

    /// The kernel that finished fading out, for the caller to give back.
    pub fn retired(&mut self) -> Option<Box<Correction>> {
        if self.previous.is_some() && self.faded >= self.fade_len {
            return self.previous.take();
        }
        None
    }

    /// Drops the current kernel at once (a mode switch, under the engine's dip) and hands it back.
    pub fn clear(&mut self) -> [Option<Box<Correction>>; 2] {
        self.faded = 0;
        [self.kernel.take(), self.previous.take()]
    }

    fn push(&mut self, stream: usize, x: f32) {
        let (p, b) = (self.pos, self.block);
        self.history[stream][p] = x;
        self.history[stream][p + b] = x;
        self.frames[stream][b + self.fill] = x;
    }

    /// Stream `stream` of kernel `k` (slot `slot`: current or previous), this sample.
    #[inline]
    fn stream_out(&self, k: &Correction, slot: usize, stream: usize, taps: &[f32]) -> f32 {
        let head = &taps[..taps.len().min(self.block)];
        let tail = if k.tails[stream].is_empty() {
            0.0
        } else {
            self.tail_out[slot][stream][self.fill]
        };
        dot(head, &self.history[stream][self.pos..]) + tail
    }

    /// One kernel's output for the current sample (history already holds it).
    fn run(&self, k: &Correction, slot: usize, channels: usize, out: &mut [f32; 2]) {
        for (c, o) in out.iter_mut().enumerate().take(channels) {
            *o = self.stream_out(k, slot, c, &k.lr[c]);
        }
        if let (Some(ms), true) = (&k.ms, channels == 2) {
            let (m, s) = (
                self.stream_out(k, slot, 2, &ms[0]),
                self.stream_out(k, slot, 3, &ms[1]),
            );
            *out = [m + s, m - s];
        }
    }

    /// Whether a kernel in play has taps past the first block.
    fn needs_tails(&self) -> bool {
        [self.kernel.as_deref(), self.previous.as_deref()]
            .into_iter()
            .flatten()
            .any(|k| (0..4).any(|s| k.partitions(s) > 0))
    }

    /// A block of input is complete: keep it, transform it into the delay line (while a kernel
    /// has a tail) and work out every kernel's tail output for the next block.
    fn end_block(&mut self, streams: usize) {
        let b = self.block;
        let ring = self.max_partitions + 1;
        self.newest = (self.newest + 1) % self.max_partitions;
        self.past_pos = (self.past_pos + 1) % ring;
        for s in 0..streams {
            let at = self.past_pos * b;
            self.past[s][at..at + b].copy_from_slice(&self.frames[s][b..]);
        }
        if self.needs_tails() {
            if self.live {
                for s in 0..streams {
                    self.frame.copy_from_slice(&self.frames[s]);
                    let at = self.newest * (b + 1);
                    // Buffers are sized by the planner: this cannot fail.
                    let _ = self.fft.process_with_scratch(
                        &mut self.frame,
                        &mut self.spectra[s][at..at + b + 1],
                        &mut self.scratch,
                    );
                }
            } else {
                self.rebuild();
            }
        } else {
            self.live = false;
        }
        for s in 0..streams {
            self.frames[s].copy_within(b.., 0);
        }
        self.fill = 0;
        self.run_tails(0);
        self.run_tails(1);
    }

    /// Transforms the kept blocks into the delay line, every lag at once (a kernel with a tail
    /// arrived while none was running).
    fn rebuild(&mut self) {
        let b = self.block;
        let ring = self.max_partitions + 1;
        for p in 0..self.max_partitions {
            let slot = (self.newest + self.max_partitions - p) % self.max_partitions;
            let (block, before) = (
                (self.past_pos + ring - p) % ring,
                (self.past_pos + ring - p - 1) % ring,
            );
            for s in 0..4 {
                self.frame[..b].copy_from_slice(&self.past[s][before * b..(before + 1) * b]);
                self.frame[b..].copy_from_slice(&self.past[s][block * b..(block + 1) * b]);
                let at = slot * (b + 1);
                let _ = self.fft.process_with_scratch(
                    &mut self.frame,
                    &mut self.spectra[s][at..at + b + 1],
                    &mut self.scratch,
                );
            }
        }
        self.live = true;
    }

    /// The tail output of the kernel in `slot` for the block under way.
    fn run_tails(&mut self, slot: usize) {
        let k = if slot == 0 {
            self.kernel.as_deref()
        } else {
            self.previous.as_deref()
        };
        let Some(k) = k else { return };
        let b = self.block;
        for s in 0..4 {
            let parts = k.partitions(s);
            if parts == 0 {
                continue;
            }
            self.acc.fill(Complex::new(0.0, 0.0));
            for p in 0..parts {
                let lag = (self.newest + self.max_partitions - p) % self.max_partitions;
                let x = &self.spectra[s][lag * (b + 1)..(lag + 1) * (b + 1)];
                let h = &k.tails[s][p * (b + 1)..(p + 1) * (b + 1)];
                for ((a, x), h) in self.acc.iter_mut().zip(x).zip(h) {
                    *a += x * h;
                }
            }
            self.acc[0].im = 0.0;
            self.acc[b].im = 0.0;
            let _ =
                self.ifft
                    .process_with_scratch(&mut self.acc, &mut self.frame, &mut self.scratch);
            self.tail_out[slot][s].copy_from_slice(&self.frame[b..]);
        }
    }

    /// Processes one or two channels in place (sample by sample across the channels: the
    /// kernels read every channel's history at each sample).
    #[allow(clippy::needless_range_loop)]
    pub fn process(&mut self, channels: &mut [&mut [f32]]) {
        if self.kernel.is_none() {
            return;
        }
        let n = channels.len().min(2);
        let frames = channels.first().map_or(0, |c| c.len());
        // The mid/side streams are fed (from the current kernel's per-channel output) while a
        // kernel in play has a mid/side stage; one that brings it in starts them from silence,
        // under its crossfade.
        let ms_in_play = n == 2
            && [self.kernel.as_deref(), self.previous.as_deref()]
                .into_iter()
                .flatten()
                .any(|k| k.ms.is_some());
        if ms_in_play && !self.ms_fed {
            for s in 2..4 {
                self.history[s].fill(0.0);
                self.frames[s].fill(0.0);
                self.past[s].fill(0.0);
                self.spectra[s].fill(Complex::new(0.0, 0.0));
            }
        }
        self.ms_fed = ms_in_play;
        let streams = if ms_in_play { 4 } else { n };
        // Nothing to correct: the input passes, the history still follows it (a block at a time).
        let plain =
            self.previous.is_none() && self.kernel.as_deref().is_some_and(Correction::is_identity);
        if plain {
            let b = self.block;
            let mut start = 0;
            while start < frames {
                let take = (b - self.fill).min(frames - start);
                let pos = self.pos;
                for c in 0..streams {
                    // Mid and side from the (unchanged) left and right.
                    let x = |i: usize| match c {
                        0 | 1 => channels[c][i],
                        2 => 0.5 * (channels[0][i] + channels[1][i]),
                        _ => 0.5 * (channels[0][i] - channels[1][i]),
                    };
                    let (history, frame) = (&mut self.history[c], &mut self.frames[c]);
                    let mut p = pos;
                    for i in start..start + take {
                        let v = x(i);
                        p = if p == 0 { b - 1 } else { p - 1 };
                        history[p] = v;
                        history[p + b] = v;
                        frame[b + self.fill + (i - start)] = v;
                    }
                }
                self.pos = (pos + b * take - take) % b;
                self.fill += take;
                start += take;
                if self.fill == b {
                    self.end_block(streams);
                }
            }
            return;
        }
        for i in 0..frames {
            self.pos = if self.pos == 0 {
                self.block - 1
            } else {
                self.pos - 1
            };
            for c in 0..n {
                let x = channels[c][i];
                self.push(c, x);
            }
            // The current kernel's per-channel stage (the mid/side stage hears it).
            let mut lr = [0.0f32; 2];
            if let Some(kernel) = self.kernel.as_deref() {
                for (c, o) in lr.iter_mut().enumerate().take(n) {
                    *o = self.stream_out(kernel, 0, c, &kernel.lr[c]);
                }
            }
            if ms_in_play {
                self.push(2, 0.5 * (lr[0] + lr[1]));
                self.push(3, 0.5 * (lr[0] - lr[1]));
            }
            let mut y = lr;
            let ms = self.kernel.as_ref().is_some_and(|k| k.ms.is_some());
            if ms || self.previous.is_some() {
                if let Some(kernel) = self.kernel.as_deref() {
                    self.run(kernel, 0, n, &mut y);
                }
            }
            if let Some(old) = self.previous.as_deref() {
                let mut z = [0.0f32; 2];
                self.run(old, 1, n, &mut z);
                let t = (self.faded as f32 / self.fade_len as f32).min(1.0);
                for c in 0..n {
                    y[c] = z[c] + t * (y[c] - z[c]);
                }
                self.faded += 1;
            }
            for c in 0..n {
                channels[c][i] = y[c];
            }
            self.fill += 1;
            if self.fill == self.block {
                self.end_block(streams);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::eq::BandType;

    fn magnitude_db(h: &[f64], w: f64) -> f64 {
        let z: Complex<f64> = h
            .iter()
            .enumerate()
            .map(|(n, &v)| Complex::from_polar(v, -w * n as f64))
            .sum();
        20.0 * z.norm().log10()
    }

    #[test]
    fn the_fir_has_the_magnitude_asked_for_and_no_delay() {
        // A gentle bump near the top: +3 dB around 0.8π.
        let fft = 4096;
        let target = |w: f64| 3.0 * (-((w - 0.8 * std::f64::consts::PI) / 0.15).powi(2)).exp();
        let ln_db = std::f64::consts::LN_10 / 20.0;
        let log_mag: Vec<f64> = (0..=fft / 2)
            .map(|k| target(std::f64::consts::PI * k as f64 / (fft / 2) as f64) * ln_db)
            .collect();
        let h = min_phase_fir(&log_mag, 128);
        for i in 1..100 {
            let w = std::f64::consts::PI * i as f64 / 100.0;
            assert!(
                (magnitude_db(&h, w) - target(w)).abs() < 0.02,
                "at {w:.2}: {} vs {}",
                magnitude_db(&h, w),
                target(w)
            );
        }
        // Minimum phase: the energy is at the front (no pre-delay).
        assert!(h[0].abs() > 0.8, "{}", h[0]);
        let energy: f64 = h.iter().map(|v| v * v).sum();
        let front: f64 = h[..8].iter().map(|v| v * v).sum();
        assert!(front / energy > 0.99, "{}", front / energy);
    }

    #[test]
    fn the_filter_runs_a_kernel_with_no_delay_and_crossfades_to_the_next() {
        let mut f = CorrectionFilter::new(64, 16, 32);
        // A kernel that halves the left and passes the right.
        let half = Correction::new([vec![0.5], vec![1.0]], None);
        assert!(f.offer(Box::new(half)).is_none());
        let (mut l, mut r) = (vec![1.0f32; 64], vec![1.0f32; 64]);
        f.process(&mut [&mut l, &mut r]);
        // From the plain signal into the kernel over the fade, then exactly the kernel.
        assert_eq!(l[0], 1.0);
        assert!((l[63] - 0.5).abs() < 1e-6 && r.iter().all(|&v| (v - 1.0).abs() < 1e-6));
        assert!(l.windows(2).all(|w| w[1] <= w[0] + 1e-6), "fades smoothly");
        assert!(f.retired().is_some_and(|k| k.is_identity()));
        // A one-sample delay kernel shows the history works: the output lags by one.
        let delay = Correction::new([vec![0.0, 1.0], vec![0.0, 1.0]], None);
        assert!(f.offer(Box::new(delay)).is_none());
        let mut warm = (vec![0.0f32; 64], vec![0.0f32; 64]);
        f.process(&mut [&mut warm.0, &mut warm.1]);
        let _ = f.retired();
        let ramp: Vec<f32> = (0..16).map(|i| i as f32).collect();
        let (mut a, mut b) = (ramp.clone(), ramp.clone());
        f.process(&mut [&mut a, &mut b]);
        assert_eq!(&a[1..], &ramp[..15]);
    }

    #[test]
    fn no_bands_need_no_correction() {
        assert!(
            zero_latency_correction(
                &[BandSettings::default(); 4],
                48_000.0,
                &TAPS_48K,
                4096,
                false
            )
            .is_none()
        );
        // A low, narrow bell already matches: nothing to run.
        let low = BandSettings {
            enabled: true,
            kind: BandType::Bell,
            freq: 200.0,
            gain_db: 6.0,
            q: 4.0,
            ..Default::default()
        };
        assert!(zero_latency_correction(&[low], 48_000.0, &TAPS_48K, 4096, false).is_none());
    }

    /// Direct convolution of `x` with `h` (the reference for the partitioned filter).
    fn convolve(x: &[f32], h: &[f32]) -> Vec<f32> {
        (0..x.len())
            .map(|n| {
                h.iter()
                    .enumerate()
                    .filter(|(k, _)| *k <= n)
                    .map(|(k, &v)| f64::from(v) * f64::from(x[n - k]))
                    .sum::<f64>() as f32
            })
            .collect()
    }

    #[test]
    fn long_kernels_run_without_delay_at_any_block_size_and_crossfade_exactly() {
        let mut seed = 7u32;
        let mut rnd = || {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (seed >> 8) as f32 / (1u32 << 24) as f32 - 0.5
        };
        // Decaying random kernels longer than several partitions, and a mid/side stage.
        let mut kernel = |len: usize| -> Vec<f32> {
            (0..len)
                .map(|i| rnd() * (-(i as f32) / 60.0).exp())
                .collect()
        };
        let (a_l, a_r, a_m, a_s) = (kernel(300), kernel(170), kernel(250), kernel(40));
        let (b_l, b_r) = (kernel(500), kernel(90));
        let n = 3000;
        let input: [Vec<f32>; 2] = std::array::from_fn(|_| (0..n).map(|_| rnd()).collect());
        // Reference: A (per channel, then mid/side) everywhere; B swapped in at `swap` with a
        // linear crossfade over `fade` samples.
        let (swap, fade) = (1100, 200);
        let lr_a = [convolve(&input[0], &a_l), convolve(&input[1], &a_r)];
        let mid: Vec<f32> = (0..n).map(|i| 0.5 * (lr_a[0][i] + lr_a[1][i])).collect();
        let side: Vec<f32> = (0..n).map(|i| 0.5 * (lr_a[0][i] - lr_a[1][i])).collect();
        let (m, sd) = (convolve(&mid, &a_m), convolve(&side, &a_s));
        let out_a = [
            (0..n).map(|i| m[i] + sd[i]).collect::<Vec<_>>(),
            (0..n).map(|i| m[i] - sd[i]).collect(),
        ];
        let out_b = [convolve(&input[0], &b_l), convolve(&input[1], &b_r)];
        // With a mid/side stage, A is checked up to the swap (while fading, the outgoing
        // kernel's mid/side stage hears the incoming kernel's per-channel output); without one,
        // the crossfade too.
        for (host_block, with_ms) in [(1, true), (7, true), (64, false), (333, false), (1, false)] {
            let out_a = if with_ms { out_a.clone() } else { lr_a.clone() };
            let mut f = CorrectionFilter::new(512, 64, fade);
            let a = Correction::new(
                [a_l.clone(), a_r.clone()],
                with_ms.then(|| [a_m.clone(), a_s.clone()]),
            )
            .partitioned(64);
            assert!(f.offer(Box::new(a)).is_none());
            // Settle A in fully before the reference starts comparing (it fades in from plain).
            let _ = f.retired();
            let (mut l, mut r) = (input[0].clone(), input[1].clone());
            let mut start = 0;
            let mut offered = false;
            while start < n {
                if !offered && start >= swap {
                    let _ = f.retired();
                    let b = Correction::new([b_l.clone(), b_r.clone()], None).partitioned(64);
                    assert!(
                        f.offer(Box::new(b)).is_none(),
                        "takes B once A has faded in"
                    );
                    offered = true;
                }
                let end =
                    (start + host_block)
                        .min(n)
                        .min(if offered { n } else { swap.max(start + 1) });
                f.process(&mut [&mut l[start..end], &mut r[start..end]]);
                start = end;
            }
            let at = |i: usize| {
                if i < swap {
                    0.0
                } else {
                    ((i - swap) as f32 / fade as f32).min(1.0)
                }
            };
            for i in fade..if with_ms { swap } else { n } {
                for (c, got) in [&l, &r].into_iter().enumerate() {
                    let t = at(i);
                    let want = out_a[c][i] + t * (out_b[c][i] - out_a[c][i]);
                    assert!(
                        (got[i] - want).abs() < 2e-4,
                        "block {host_block}, mid/side {with_ms}, channel {c}, sample {i}: {} vs {want}",
                        got[i]
                    );
                }
            }
        }
    }
}
