//! Dual-channel measurement of what a chain does: a reference (the signal before the chain) and
//! a measurement (after it), both live music or anything else that plays. No test signal.
//!
//! - [`DelayFinder`]: the chain's bulk delay, from the generalised cross-correlation with phase
//!   transform (GCC-PHAT), averaged over time. Signed and fractional, with the polarity and how
//!   sure it is.
//! - [`Transfer`]: the transfer function H = Gxy / Gxx (cross spectrum over the reference's
//!   power), averaged over overlapping frames, with the coherence γ² = |Gxy|² / (Gxx·Gyy) that
//!   says where H can be trusted. Read at log-spaced points with fractional-octave smoothing of
//!   the spectra (before the division, so noise averages out rather than into the result), as
//!   magnitude, phase and group delay; and the impulse response.
//!
//! - **Level dependence** ([`TransferPoint::level_spread`]): H measured twice more, over the
//!   frames louder than the running mean level and over the quieter ones. A filter gives both
//!   the same H whatever plays; a gate, compressor, limiter or saturator does something else to
//!   loud and soft passages, so the two disagree. Coherence alone misses these: on a drum loop a
//!   compressor's output is explained by one filter to γ² 0.99, a filter with a 25 ms group delay
//!   at 50 Hz (the kick's tail came through louder than its body), which the chain does not have
//!   (`examples/tv_probe`, 2026-11-27: every filter reads a spread of 0.000, the gate, compressor
//!   and saturator 0.05–0.4 where they draw a delay that is not there).
//!
//! The editor aligns the reference by the integer part of the delay before feeding [`Transfer`],
//! and removes the fraction when it reads the phase, so the phase shown is what the chain does on
//! top of its latency.

use std::f64::consts::{PI, TAU};
use std::sync::Arc;

use realfft::num_complex::Complex;
use realfft::{ComplexToReal, RealFftPlanner, RealToComplex};

/// The band measured, Hz.
pub const BAND_LO: f64 = 20.0;
pub const BAND_HI: f64 = 20_000.0;
/// A frame whose reference is quieter than this (mean square) is skipped: pauses must not
/// dilute the averages.
pub const SILENT_MS: f64 = 1e-9;

fn hann(n: usize) -> Vec<f64> {
    (0..n).map(|i| 0.5 - 0.5 * (TAU * i as f64 / n as f64).cos()).collect()
}

/// The averaging weight of one update, `every` seconds apart, for a time constant `tau` s.
pub fn average_weight(every: f64, tau: f64) -> f64 {
    1.0 - (-every / tau.max(1e-6)).exp()
}

/// A bulk delay as found: `samples` > 0 when the measurement lags the reference.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DelayEstimate {
    pub samples: f64,
    /// The correlation peak is negative: the chain flips the polarity.
    pub inverted: bool,
    /// The peak's height, 0…1: 1 for a pure delay heard in every bin.
    pub confidence: f64,
}

/// Below this confidence the peak is not trusted (noise reaches about 1/√bins).
pub const MIN_CONFIDENCE: f64 = 0.04;

pub struct DelayFinder {
    n: usize,
    fs: f64,
    fwd: Arc<dyn RealToComplex<f32>>,
    inv: Arc<dyn ComplexToReal<f32>>,
    window: Vec<f32>,
    buf: Vec<f32>,
    spec: [Vec<Complex<f32>>; 2],
    scratch: Vec<Complex<f32>>,
    cross: Vec<Complex<f64>>,
    acc: Vec<Complex<f64>>,
    acc_c: Vec<Complex<f32>>,
    corr: Vec<f32>,
    inv_scratch: Vec<Complex<f32>>,
    bins: (usize, usize),
    fed: bool,
}

impl DelayFinder {
    /// `n` samples a frame (a power of two); delays up to ±n/2 are found. Allocates.
    pub fn new(n: usize, fs: f64) -> Self {
        let mut planner = RealFftPlanner::<f32>::new();
        let fwd = planner.plan_fft_forward(n);
        let inv = planner.plan_fft_inverse(n);
        let hz = fs / n as f64;
        let bins = (((BAND_LO / hz).ceil() as usize).max(1), ((BAND_HI.min(0.45 * fs) / hz).floor() as usize).min(n / 2 - 1));
        Self {
            spec: [fwd.make_output_vec(), fwd.make_output_vec()],
            scratch: fwd.make_scratch_vec(),
            inv_scratch: inv.make_scratch_vec(),
            cross: vec![Complex::new(0.0, 0.0); n / 2 + 1],
            acc: vec![Complex::new(0.0, 0.0); n / 2 + 1],
            acc_c: inv.make_input_vec(),
            corr: inv.make_output_vec(),
            window: hann(n).into_iter().map(|w| w as f32).collect(),
            buf: vec![0.0; n],
            n,
            fs,
            fwd,
            inv,
            bins,
            fed: false,
        }
    }

    pub fn len(&self) -> usize {
        self.n
    }

    pub fn is_empty(&self) -> bool {
        self.n == 0
    }

    pub fn sample_rate(&self) -> f64 {
        self.fs
    }

    pub fn reset(&mut self) {
        self.acc.fill(Complex::new(0.0, 0.0));
        self.fed = false;
    }

    /// One frame of each channel pair (reference, measurement; `n` samples each, ending at the
    /// same moment), folded into the average with weight `a` (0…1). None while the reference is
    /// silent or no peak stands out.
    pub fn update(&mut self, refs: &[&[f32]], meas: &[&[f32]], a: f64) -> Option<DelayEstimate> {
        self.cross.fill(Complex::new(0.0, 0.0));
        let mut energy = 0.0f64;
        for (r, m) in refs.iter().zip(meas) {
            for (s, x) in [r, m].into_iter().enumerate() {
                let x = &x[x.len() - self.n..];
                if s == 0 {
                    energy += x.iter().map(|&v| f64::from(v) * f64::from(v)).sum::<f64>() / self.n as f64;
                }
                for ((b, &v), &w) in self.buf.iter_mut().zip(x).zip(&self.window) {
                    *b = v * w;
                }
                let _ = self.fwd.process_with_scratch(&mut self.buf, &mut self.spec[s], &mut self.scratch);
            }
            for k in self.bins.0..=self.bins.1 {
                let (x, y) = (self.spec[0][k], self.spec[1][k]);
                self.cross[k] += Complex::new(f64::from(x.re), -f64::from(x.im)) * Complex::new(f64::from(y.re), f64::from(y.im));
            }
        }
        if energy / refs.len().max(1) as f64 >= SILENT_MS {
            let a = if self.fed { a } else { 1.0 };
            self.fed = true;
            for k in self.bins.0..=self.bins.1 {
                let c = self.cross[k];
                let m = c.norm();
                let unit = if m > 1e-30 { c / m } else { Complex::new(0.0, 0.0) };
                self.acc[k] = self.acc[k] * (1.0 - a) + unit * a;
            }
        }
        if !self.fed {
            return None;
        }
        self.estimate()
    }

    /// The peak of the averaged correlation.
    pub fn estimate(&mut self) -> Option<DelayEstimate> {
        self.acc_c.fill(Complex::new(0.0, 0.0));
        for k in self.bins.0..=self.bins.1 {
            self.acc_c[k] = Complex::new(self.acc[k].re as f32, self.acc[k].im as f32);
        }
        let _ = self.inv.process_with_scratch(&mut self.acc_c, &mut self.corr, &mut self.inv_scratch);
        let n = self.n;
        let (mut best, mut at) = (0.0f32, 0usize);
        for (i, &v) in self.corr.iter().enumerate() {
            if v.abs() > best {
                best = v.abs();
                at = i;
            }
        }
        let norm = 2.0 * (self.bins.1 + 1 - self.bins.0) as f64;
        let confidence = f64::from(best) / norm;
        if confidence < MIN_CONFIDENCE {
            return None;
        }
        let v = |i: isize| f64::from(self.corr[i.rem_euclid(n as isize) as usize]);
        let (p, c, q) = (v(at as isize - 1), v(at as isize), v(at as isize + 1));
        let den = p - 2.0 * c + q;
        let frac = if den.abs() > 1e-12 { (0.5 * (p - q) / den).clamp(-0.5, 0.5) } else { 0.0 };
        let lag = if at > n / 2 { at as f64 - n as f64 } else { at as f64 };
        Some(DelayEstimate { samples: lag + frac, inverted: c < 0.0, confidence })
    }
}

/// One point of a transfer function as read.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct TransferPoint {
    pub freq: f64,
    /// |H|, dB.
    pub magnitude_db: f64,
    /// arg H, radians, −π…π (after the fractional delay is taken out).
    pub phase: f64,
    /// −dφ/dω, seconds (on top of the delay taken out).
    pub group_delay: f64,
    /// γ², 0…1.
    pub coherence: f64,
    /// |H_loud − H_quiet| / |H|: how far H over the loud frames differs from H over the quiet
    /// ones. 0 for a filter (to the measurement's noise, about 0.04 at γ² 0.996); a dynamics
    /// processor or a saturator reads more, and its phase there is not a filter's.
    pub level_spread: f64,
    /// The spread the measurement's own noise would give, the same scale: each class's random
    /// error of H, |H|·√((1−γ²)/(γ²·n)) with its own coherence and number of averages (Bendat &
    /// Piersol), joined. Only spread well above this says the chain changes with level
    /// (2026-11-27, user: behind ordinary plugins on music the phase page went blank; quiet
    /// passages under a plugin's hiss read γ² far under the loud ones, so their H wanders).
    pub level_noise: f64,
}

pub struct Transfer {
    n: usize,
    fs: f64,
    fwd: Arc<dyn RealToComplex<f64>>,
    inv: Arc<dyn ComplexToReal<f64>>,
    window: Vec<f64>,
    buf: Vec<f64>,
    spec: [Vec<Complex<f64>>; 2],
    scratch: Vec<Complex<f64>>,
    inv_in: Vec<Complex<f64>>,
    inv_scratch: Vec<Complex<f64>>,
    gxy: Vec<Complex<f64>>,
    gxx: Vec<f64>,
    gyy: Vec<f64>,
    fed: bool,
    /// The same spectra over the loud frames [0] and the quiet ones [1] (each averaged over its
    /// own frames), and the running mean level that splits them, dB.
    class_gxy: [Vec<Complex<f64>>; 2],
    class_gxx: [Vec<f64>; 2],
    class_gyy: [Vec<f64>; 2],
    class_fed: [bool; 2],
    /// Frames each class has averaged (up to its average's length), and that length in frames.
    class_frames: [f64; 2],
    frames_full: f64,
    level_db: Option<f64>,
    /// The points' frequencies and, per point, its run of (bin, weight) in `weights`.
    freqs: Vec<f64>,
    runs: Vec<(usize, usize)>,
    weights: Vec<(usize, f64)>,
    /// Each window's centre of mass, Hz: where its smoothed phase really sits (at the low end,
    /// where a window spans a few bins, that is not quite the point's frequency).
    centroids: Vec<f64>,
    /// Each point's group delay is the phase slope over its neighbours this many points away,
    /// a span as wide as its window: a slope over less than the smoothing only adds noise.
    spans: Vec<usize>,
    smoothed: Vec<(Complex<f64>, f64, f64, f64)>,
}

impl Transfer {
    /// `n`-sample frames (a power of two), read at `points` log-spaced frequencies from
    /// [`BAND_LO`] to [`BAND_HI`] smoothed over `smooth_oct` octaves. Allocates.
    pub fn new(n: usize, fs: f64, points: usize, smooth_oct: f64) -> Self {
        let mut planner = RealFftPlanner::<f64>::new();
        let fwd = planner.plan_fft_forward(n);
        let inv = planner.plan_fft_inverse(n);
        let freqs = (0..points).map(|i| BAND_LO * (BAND_HI / BAND_LO).powf(i as f64 / (points - 1).max(1) as f64)).collect();
        let mut t = Self {
            spec: [fwd.make_output_vec(), fwd.make_output_vec()],
            scratch: fwd.make_scratch_vec(),
            inv_in: inv.make_input_vec(),
            inv_scratch: inv.make_scratch_vec(),
            gxy: vec![Complex::new(0.0, 0.0); n / 2 + 1],
            gxx: vec![0.0; n / 2 + 1],
            gyy: vec![0.0; n / 2 + 1],
            class_gxy: [vec![Complex::new(0.0, 0.0); n / 2 + 1], vec![Complex::new(0.0, 0.0); n / 2 + 1]],
            class_gxx: [vec![0.0; n / 2 + 1], vec![0.0; n / 2 + 1]],
            class_gyy: [vec![0.0; n / 2 + 1], vec![0.0; n / 2 + 1]],
            class_fed: [false; 2],
            class_frames: [0.0; 2],
            frames_full: 1.0,
            level_db: None,
            window: hann(n),
            buf: vec![0.0; n],
            fed: false,
            freqs,
            runs: Vec::new(),
            weights: Vec::new(),
            centroids: Vec::new(),
            spans: Vec::new(),
            smoothed: vec![(Complex::new(0.0, 0.0), 0.0, 0.0, 0.0); points],
            n,
            fs,
            fwd,
            inv,
        };
        t.set_smoothing(smooth_oct);
        t
    }

    pub fn len(&self) -> usize {
        self.n
    }

    pub fn is_empty(&self) -> bool {
        self.n == 0
    }

    pub fn sample_rate(&self) -> f64 {
        self.fs
    }

    pub fn points(&self) -> usize {
        self.freqs.len()
    }

    /// Whether anything has been measured since the last reset.
    pub fn has_data(&self) -> bool {
        self.fed
    }

    pub fn reset(&mut self) {
        self.gxy.fill(Complex::new(0.0, 0.0));
        self.gxx.fill(0.0);
        self.gyy.fill(0.0);
        self.fed = false;
        for c in 0..2 {
            self.class_gxy[c].fill(Complex::new(0.0, 0.0));
            self.class_gxx[c].fill(0.0);
            self.class_gyy[c].fill(0.0);
        }
        self.class_fed = [false; 2];
        self.class_frames = [0.0; 2];
        self.level_db = None;
    }

    /// Each point's window: a raised cosine `oct` octaves wide (as Hz either side of the point),
    /// never narrower than 1.5 bins either side, so every point hears at least two bins and the
    /// smoothing is continuous down to the lowest point. Allocates.
    pub fn set_smoothing(&mut self, oct: f64) {
        let hz = self.fs / self.n as f64;
        self.runs.clear();
        self.weights.clear();
        self.centroids.clear();
        self.spans.clear();
        let last = self.n / 2;
        for &f in &self.freqs {
            let half = (f * (2f64.powf(oct / 2.0) - 1.0)).max(1.5 * hz);
            let lo = (((f - half) / hz).ceil().max(1.0)) as usize;
            let hi = (((f + half) / hz).floor() as usize).min(last);
            let start = self.weights.len();
            for k in lo..=hi.max(lo) {
                let d = (k as f64 * hz - f) / half;
                if d.abs() < 1.0 {
                    let c = (0.5 * PI * d).cos();
                    self.weights.push((k, c * c));
                }
            }
            let run = &self.weights[start..];
            let total: f64 = run.iter().map(|w| w.1).sum();
            self.centroids.push(if total > 0.0 { run.iter().map(|&(k, w)| k as f64 * hz * w).sum::<f64>() / total } else { f });
            self.runs.push((start, self.weights.len() - start));
        }
        let m = self.freqs.len();
        let step = if m > 1 { (self.freqs[1] / self.freqs[0]).ln() } else { 1.0 };
        for &f in &self.freqs {
            // Over at least four bins either side: a slope across fewer is mostly noise.
            let half = (f * (2f64.powf(oct / 2.0) - 1.0)).max(4.0 * hz);
            // At least 1/24 octave either side (1/12 across): finer than that a derivative of
            // measured phase shows mostly its noise.
            let k = ((((f + half) / f).ln() / step).round() as usize).max((2f64.ln() / 24.0 / step).ceil() as usize);
            self.spans.push(k.clamp(1, (m / 8).max(1)));
        }
    }

    /// One frame of each channel pair (reference, measurement; `n` samples each, the reference
    /// already aligned by the delay), into the average with weight `a`. False (nothing learned)
    /// while the reference is silent.
    pub fn feed(&mut self, refs: &[&[f32]], meas: &[&[f32]], a: f64) -> bool {
        let mut energy = 0.0;
        for r in refs {
            let r = &r[r.len() - self.n..];
            energy += r.iter().map(|&v| f64::from(v) * f64::from(v)).sum::<f64>() / self.n as f64;
        }
        if energy / (refs.len().max(1) as f64) < SILENT_MS {
            return false;
        }
        let a_all = a;
        let a = if self.fed { a } else { 1.0 };
        self.fed = true;
        let keep = 1.0 - a;
        for g in &mut self.gxy {
            *g *= keep;
        }
        for g in self.gxx.iter_mut().chain(self.gyy.iter_mut()) {
            *g *= keep;
        }
        // Louder or quieter than the running mean (which follows over the same time).
        let level = 10.0 * (energy / refs.len().max(1) as f64).log10();
        let mean = *self.level_db.get_or_insert(level);
        let class = usize::from(level < mean);
        self.level_db = Some(mean + (level - mean) * a_all);
        let ac = if self.class_fed[class] { a_all } else { 1.0 };
        self.class_fed[class] = true;
        // An exponential average of weight a holds (2 − a)/a frames' worth.
        self.frames_full = (2.0 - a_all) / a_all.max(1e-9);
        self.class_frames[class] = (self.class_frames[class] + 1.0).min(self.frames_full);
        let ckeep = 1.0 - ac;
        for g in &mut self.class_gxy[class] {
            *g *= ckeep;
        }
        for g in self.class_gxx[class].iter_mut().chain(self.class_gyy[class].iter_mut()) {
            *g *= ckeep;
        }
        for (r, m) in refs.iter().zip(meas) {
            for (s, x) in [r, m].into_iter().enumerate() {
                let x = &x[x.len() - self.n..];
                for ((b, &v), &w) in self.buf.iter_mut().zip(x).zip(&self.window) {
                    *b = f64::from(v) * w;
                }
                let _ = self.fwd.process_with_scratch(&mut self.buf, &mut self.spec[s], &mut self.scratch);
            }
            for k in 0..self.gxy.len() {
                let (x, y) = (self.spec[0][k], self.spec[1][k]);
                let (xy, xx, yy) = (x.conj() * y, x.norm_sqr(), y.norm_sqr());
                self.gxy[k] += xy * a;
                self.gxx[k] += xx * a;
                self.gyy[k] += yy * a;
                self.class_gxy[class][k] += xy * ac;
                self.class_gxx[class][k] += xx * ac;
                self.class_gyy[class][k] += yy * ac;
            }
        }
        true
    }

    /// The points, with `delay_frac` samples (the delay's fraction the alignment left) taken out
    /// of the phase. `out` must hold [`Transfer::points`].
    #[allow(clippy::needless_range_loop)] // neighbours by index
    pub fn read(&mut self, delay_frac: f64, out: &mut [TransferPoint]) {
        // H at a point is the mean of the bins' own H (Gxy/Gxx), each weighted by its window
        // and its coherence: every bin that hears the reference counts alike. The ratio of the
        // smoothed spectra (what the coherence uses) lets the loudest bin win, so on music H
        // would stick to the notes and step between them at the low end.
        let split = self.class_fed[0] && self.class_fed[1];
        for (i, &(start, len)) in self.runs.iter().enumerate() {
            let run = &self.weights[start..start + len];
            let (h, xx, yy, xy2) = point_h(run, &self.gxy, &self.gxx, &self.gyy);
            // The fraction out: H·e^{+jωτ}.
            let f = self.freqs[i];
            let turn = Complex::from_polar(1.0, TAU * f * delay_frac / self.fs);
            self.smoothed[i] = (h * turn, xx, yy, xy2);
            (out[i].level_spread, out[i].level_noise) = if split && h.norm() > 1e-12 {
                // Independent looks: the window's bins (its weights' effective count), times the
                // class's frames, halved for the frames' 75 % overlap.
                let (sw, sw2) = run.iter().fold((0.0, 0.0), |(a, b), &(_, w)| (a + w, b + w * w));
                let bins = if sw2 > 0.0 { sw * sw / sw2 } else { 1.0 };
                let mut var = 0.0;
                let mut hs = [Complex::new(0.0, 0.0); 2];
                for c in 0..2 {
                    let (hc, xx, yy, xy2) = point_h(run, &self.class_gxy[c], &self.class_gxx[c], &self.class_gyy[c]);
                    let g = if xx > 1e-30 && yy > 1e-30 { (xy2 / (xx * yy)).clamp(1e-6, 1.0) } else { 1e-6 };
                    let n = (bins * 0.5 * self.class_frames[c]).max(1.0);
                    var += hc.norm_sqr() * (1.0 - g) / (g * n);
                    hs[c] = hc;
                }
                ((hs[0] - hs[1]).norm() / h.norm(), var.sqrt() / h.norm())
            } else {
                (0.0, 0.0)
            };
        }
        let m = self.freqs.len();
        for i in 0..m {
            let (h, xx, yy, xy2) = self.smoothed[i];
            let p = &mut out[i];
            p.freq = self.freqs[i];
            if xx <= 1e-30 || yy <= 1e-30 {
                *p = TransferPoint { freq: p.freq, magnitude_db: -120.0, ..Default::default() };
                continue;
            }
            // (level_spread was written in the first pass.)
            p.magnitude_db = 20.0 * h.norm().max(1e-6).log10();
            p.phase = h.arg();
            p.coherence = (xy2 / (xx * yy)).clamp(0.0, 1.0);
            let k = self.spans[i];
            let (a, b) = (i.saturating_sub(k), (i + k).min(m - 1));
            let d = self.smoothed[b].0 * self.smoothed[a].0.conj();
            let df = self.centroids[b] - self.centroids[a];
            p.group_delay = if df > 0.0 && d.norm() > 0.0 { -d.arg() / (TAU * df) } else { 0.0 };
        }
    }

    /// The impulse response (n samples, circular: lag 0 first, negative lags at the end), from
    /// the averaged H, normalised to unit gain at the peak's scale (a pure delay reads 1).
    pub fn impulse(&mut self, out: &mut [f64]) {
        let floor = 1e-7 * self.gxx.iter().copied().fold(0.0, f64::max);
        let hz = self.fs / self.n as f64;
        for (k, c) in self.inv_in.iter_mut().enumerate() {
            let f = k as f64 * hz;
            // A gentle edge outside the band (where the reference has no say).
            let edge = if f < BAND_LO { (f / BAND_LO).powi(2) } else { 1.0 };
            *c = if self.gxx[k] > floor { self.gxy[k] / self.gxx[k] * edge } else { Complex::new(0.0, 0.0) };
        }
        self.inv_in[0].im = 0.0;
        let last = self.inv_in.len() - 1;
        self.inv_in[last].im = 0.0;
        let _ = self.inv.process_with_scratch(&mut self.inv_in, out, &mut self.inv_scratch);
        let k = 1.0 / self.n as f64;
        for v in out.iter_mut() {
            *v *= k;
        }
    }

    pub fn make_impulse_vec(&self) -> Vec<f64> {
        vec![0.0; self.n]
    }
}

/// H at one point from spectra: the mean of the bins' own H (Gxy/Gxx), each weighted by its
/// window and its coherence, with the window's sums (Gxx, Gyy, |Gxy|²) for the coherence.
fn point_h(run: &[(usize, f64)], gxy: &[Complex<f64>], gxx: &[f64], gyy: &[f64]) -> (Complex<f64>, f64, f64, f64) {
    let (mut xy, mut xx, mut yy) = (Complex::new(0.0, 0.0), 0.0, 0.0);
    let (mut h, mut wsum) = (Complex::new(0.0, 0.0), 0.0);
    for &(k, w) in run {
        let (gxy, gxx, gyy) = (gxy[k], gxx[k], gyy[k]);
        xy += gxy * w;
        xx += gxx * w;
        yy += gyy * w;
        if gxx > 1e-30 && gyy > 1e-30 {
            let c = w * gxy.norm_sqr() / (gxx * gyy);
            h += gxy / gxx * c;
            wsum += c;
        }
    }
    let h = if wsum > 1e-12 { h / wsum } else if xx > 1e-30 { xy / xx } else { Complex::new(0.0, 0.0) };
    (h, xx, yy, xy.norm_sqr())
}

/// Two readings at the same points joined: `low` (a long frame: fine bins for the low end)
/// below `from` Hz, `high` (a short frame: quicker, less smeared) above `to`, crossfaded on a
/// smooth step in log frequency between. H blends as a complex number, so the phase turns the
/// short way.
pub fn blend(low: &[TransferPoint], high: &[TransferPoint], from: f64, to: f64, out: &mut [TransferPoint]) {
    for ((o, l), h) in out.iter_mut().zip(low).zip(high) {
        let t = ((h.freq / from).ln() / (to / from).ln()).clamp(0.0, 1.0);
        let u = t * t * (3.0 - 2.0 * t);
        if u <= 0.0 {
            *o = *l;
            continue;
        }
        if u >= 1.0 {
            *o = *h;
            continue;
        }
        let c = |p: &TransferPoint| Complex::from_polar(10f64.powf(p.magnitude_db / 20.0), p.phase);
        let z = c(l) * (1.0 - u) + c(h) * u;
        *o = TransferPoint {
            freq: h.freq,
            magnitude_db: 20.0 * z.norm().max(1e-6).log10(),
            phase: z.arg(),
            group_delay: l.group_delay * (1.0 - u) + h.group_delay * u,
            coherence: l.coherence * (1.0 - u) + h.coherence * u,
            level_spread: l.level_spread * (1.0 - u) + h.level_spread * u,
            level_noise: l.level_noise * (1.0 - u) + h.level_noise * u,
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::filter::Coeffs;

    const FS: f64 = 48_000.0;

    fn noise(n: usize, seed: u32) -> Vec<f32> {
        let mut s = seed;
        (0..n)
            .map(|_| {
                s ^= s << 13;
                s ^= s >> 17;
                s ^= s << 5;
                (s as f32 / u32::MAX as f32) * 2.0 - 1.0
            })
            .map(|v| 0.3 * v)
            .collect()
    }

    fn biquad(c: &Coeffs, x: &[f32]) -> Vec<f32> {
        let (mut s1, mut s2) = (0.0f64, 0.0f64);
        x.iter()
            .map(|&v| {
                let v = f64::from(v);
                let y = c.b0 * v + s1;
                s1 = c.b1 * v - c.a1 * y + s2;
                s2 = c.b2 * v - c.a2 * y;
                y as f32
            })
            .collect()
    }

    /// RBJ second-order all-pass at `f0`, Q.
    fn allpass(f0: f64, q: f64) -> Coeffs {
        let w = TAU * f0 / FS;
        let alpha = w.sin() / (2.0 * q);
        let a0 = 1.0 + alpha;
        Coeffs { b0: (1.0 - alpha) / a0, b1: -2.0 * w.cos() / a0, b2: 1.0, a1: -2.0 * w.cos() / a0, a2: (1.0 - alpha) / a0 }
    }

    fn lowpass(f0: f64, q: f64) -> Coeffs {
        let w = TAU * f0 / FS;
        let alpha = w.sin() / (2.0 * q);
        let a0 = 1.0 + alpha;
        let c = w.cos();
        Coeffs { b0: (1.0 - c) / 2.0 / a0, b1: (1.0 - c) / a0, b2: (1.0 - c) / 2.0 / a0, a1: -2.0 * c / a0, a2: (1.0 - alpha) / a0 }
    }

    fn delayed(x: &[f32], d: usize) -> Vec<f32> {
        (0..x.len()).map(|i| if i >= d { x[i - d] } else { 0.0 }).collect()
    }

    /// Runs the finder and the transfer over `x` → `y` the way the editor does (hops of n/4,
    /// the reference aligned by the integer delay).
    fn measure(x: &[f32], y: &[f32], points: usize, a: f64) -> (Option<DelayEstimate>, Vec<TransferPoint>, Transfer) {
        let mut finder = DelayFinder::new(16_384, FS);
        let mut est = None;
        let mut end = finder.len() + 2048;
        while end <= x.len() {
            est = finder.update(&[&x[end - finder.len()..end]], &[&y[end - finder.len()..end]], 0.3).or(est);
            end += 4096;
        }
        let e = est.expect("a delay");
        let d = e.samples.round() as isize;
        let mut t = Transfer::new(8192, FS, points, 1.0 / 24.0);
        let mut end = 8192 + 4096;
        while end <= x.len() {
            let re = (end as isize - d) as usize;
            t.feed(&[&x[re - 8192..re]], &[&y[end - 8192..end]], a);
            end += 2048;
        }
        let mut out = vec![TransferPoint::default(); points];
        t.read(e.samples - d as f64, &mut out);
        (Some(e), out, t)
    }

    #[test]
    fn a_pure_delay_reads_flat_with_its_latency_and_full_coherence() {
        let x = noise(FS as usize * 3, 7);
        let y = delayed(&x, 1234);
        let (e, pts, mut t) = measure(&x, &y, 121, 0.1);
        let e = e.unwrap();
        assert!((e.samples - 1234.0).abs() < 0.05, "delay {}", e.samples);
        assert!(!e.inverted);
        assert!(e.confidence > 0.5, "confidence {}", e.confidence);
        for p in &pts {
            assert!(p.magnitude_db.abs() < 0.05, "{} Hz: {} dB", p.freq, p.magnitude_db);
            assert!(p.phase.abs().to_degrees() < 1.0, "{} Hz: {}°", p.freq, p.phase.to_degrees());
            assert!(p.coherence > 0.99, "{} Hz: γ² {}", p.freq, p.coherence);
            assert!(p.group_delay.abs() < 2e-5, "{} Hz: {} s", p.freq, p.group_delay);
        }
        let mut ir = t.make_impulse_vec();
        t.impulse(&mut ir);
        assert!((ir[0] - 1.0).abs() < 0.05, "the impulse sits at lag 0: {}", ir[0]);
    }

    #[test]
    fn a_flipped_polarity_reads_180_degrees_and_inverted() {
        let x = noise(FS as usize * 3, 11);
        let y: Vec<f32> = delayed(&x, 300).into_iter().map(|v| -v).collect();
        let (e, pts, _) = measure(&x, &y, 61, 0.1);
        let e = e.unwrap();
        assert!(e.inverted);
        assert!((e.samples - 300.0).abs() < 0.05);
        for p in &pts {
            assert!((p.phase.abs() - PI).abs().to_degrees() < 1.0, "{} Hz: {}°", p.freq, p.phase.to_degrees());
        }
    }

    #[test]
    fn negative_delays_mean_the_reference_lags() {
        let x = noise(FS as usize * 2, 3);
        let y = delayed(&x, 500);
        let mut finder = DelayFinder::new(16_384, FS);
        let e = finder.update(&[&y[40_000..40_000 + 16_384]], &[&x[40_000..40_000 + 16_384]], 1.0).unwrap();
        assert!((e.samples + 500.0).abs() < 0.05, "{}", e.samples);
    }

    #[test]
    fn a_fractional_delay_is_found_and_taken_out_of_the_phase() {
        // Half a sample by a linear-phase windowed sinc (accurate through 20 kHz is not possible
        // at 48 kHz, so check to 16 kHz).
        let x = noise(FS as usize * 3, 5);
        let taps = 255usize;
        let frac = 0.5;
        let h: Vec<f64> = (0..taps)
            .map(|i| {
                let t = i as f64 - (taps / 2) as f64 - frac;
                let s = if t.abs() < 1e-9 { 1.0 } else { (PI * t).sin() / (PI * t) };
                s * (0.5 - 0.5 * (TAU * (i as f64 + 0.5) / taps as f64).cos())
            })
            .collect();
        let y: Vec<f32> = (0..x.len()).map(|n| h.iter().enumerate().filter(|(k, _)| n >= *k).map(|(k, c)| c * f64::from(x[n - k])).sum::<f64>() as f32).collect();
        let (e, pts, _) = measure(&x, &y, 121, 0.1);
        let e = e.unwrap();
        assert!((e.samples - (127.0 + frac)).abs() < 0.1, "delay {}", e.samples);
        for p in pts.iter().filter(|p| p.freq < 16_000.0) {
            assert!(p.phase.abs().to_degrees() < 6.0, "{} Hz: {}°", p.freq, p.phase.to_degrees());
        }
    }

    #[test]
    fn an_allpass_reads_its_own_phase_and_group_delay() {
        // A long average: group delay is a derivative, and noise at the low end (few bins a
        // point) is what it amplifies.
        let c = allpass(1000.0, 0.7);
        let x = noise(FS as usize * 12, 9);
        let y = biquad(&c, &x);
        let (e, pts, _) = measure(&x, &y, 241, 0.01);
        let e = e.unwrap();
        let d = e.samples;
        for p in pts.iter().filter(|p| p.freq > 40.0) {
            let w = TAU * p.freq / FS;
            let want = c.response(w) * Complex::from_polar(1.0, w * d);
            let err = (p.phase - want.arg() + PI).rem_euclid(TAU) - PI;
            assert!(err.abs().to_degrees() < 3.0, "{} Hz: {}° against {}°", p.freq, p.phase.to_degrees(), want.arg().to_degrees());
            assert!(p.magnitude_db.abs() < 0.2, "{} Hz: {} dB", p.freq, p.magnitude_db);
            // Group delay: −dφ/dω of the filter, numerically, against the reading plus the delay
            // (above 100 Hz: under it 8192-point frames are too coarse for a derivative).
            if p.freq < 100.0 {
                continue;
            }
            let dw = 1e-4;
            let gd = -((c.response(w + dw) * c.response(w - dw).conj()).arg()) / (2.0 * dw) / FS;
            let got = p.group_delay + d / FS;
            assert!((got - gd).abs() < 4e-5 + 0.03 * gd.abs(), "{} Hz: group delay {got} against {gd}", p.freq);
        }
    }

    #[test]
    fn a_lowpass_reads_its_magnitude_and_phase_where_coherent() {
        let c = lowpass(2000.0, 0.707);
        let x = noise(FS as usize * 4, 13);
        let y = biquad(&c, &x);
        let (e, pts, _) = measure(&x, &y, 241, 0.1);
        let d = e.unwrap().samples;
        for p in pts.iter().filter(|p| p.freq > 40.0 && p.freq < 8000.0) {
            let w = TAU * p.freq / FS;
            let want = c.response(w) * Complex::from_polar(1.0, w * d);
            let err = (p.phase - want.arg() + PI).rem_euclid(TAU) - PI;
            assert!(err.abs().to_degrees() < 3.0, "{} Hz: {}° against {}°", p.freq, p.phase.to_degrees(), want.arg().to_degrees());
            let db = 20.0 * want.norm().log10();
            assert!((p.magnitude_db - db).abs() < 0.3, "{} Hz: {} dB against {db}", p.freq, p.magnitude_db);
            assert!(p.coherence > 0.95);
        }
    }

    /// RBJ high-pass at `f0`, Q.
    fn highpass(f0: f64, q: f64) -> Coeffs {
        let w = TAU * f0 / FS;
        let alpha = w.sin() / (2.0 * q);
        let a0 = 1.0 + alpha;
        let c = w.cos();
        Coeffs { b0: (1.0 + c) / 2.0 / a0, b1: -(1.0 + c) / a0, b2: (1.0 + c) / 2.0 / a0, a1: -2.0 * c / a0, a2: (1.0 - alpha) / a0 }
    }

    #[test]
    fn a_long_frame_reads_a_highpass_group_delay_down_to_20_hz() {
        let c = highpass(60.0, 0.707);
        let x = noise(FS as usize * 20, 21);
        let y = biquad(&c, &x);
        let n = 65_536;
        let mut t = Transfer::new(n, FS, 241, 1.0 / 24.0);
        let mut end = n;
        while end <= x.len() {
            t.feed(&[&x[end - n..end]], &[&y[end - n..end]], 0.05);
            end += n / 4;
        }
        let mut out = vec![TransferPoint::default(); 241];
        t.read(0.0, &mut out);
        let mut worst = 0.0f64;
        for p in out.iter().filter(|p| p.freq < 400.0) {
            let w = TAU * p.freq / FS;
            let dw = 1e-5;
            let gd = -((c.response(w + dw) * c.response(w - dw).conj()).arg()) / (2.0 * dw) / FS;
            worst = worst.max((p.group_delay - gd).abs() / gd);
            let want = c.response(w);
            let err = (p.phase - want.arg() + PI).rem_euclid(TAU) - PI;
            assert!(err.abs().to_degrees() < 2.0, "{} Hz: {}° against {}°", p.freq, p.phase.to_degrees(), want.arg().to_degrees());
            assert!((p.group_delay - gd).abs() < 0.06 * gd, "{} Hz: group delay {} ms against {} ms", p.freq, p.group_delay * 1e3, gd * 1e3);
        }
        assert!(worst < 0.06, "{worst}");
    }

    #[test]
    fn unrelated_signals_find_no_delay_and_no_coherence() {
        let x = noise(FS as usize * 3, 17);
        let y = noise(FS as usize * 3, 23);
        let mut finder = DelayFinder::new(16_384, FS);
        let mut est = None;
        let mut end = 16_384;
        while end <= x.len() {
            est = finder.update(&[&x[end - 16_384..end]], &[&y[end - 16_384..end]], 0.3);
            end += 4096;
        }
        assert!(est.is_none(), "{est:?}");
        let mut t = Transfer::new(8192, FS, 61, 1.0 / 24.0);
        let mut end = 8192;
        while end <= x.len() {
            t.feed(&[&x[end - 8192..end]], &[&y[end - 8192..end]], 0.1);
            end += 2048;
        }
        let mut out = vec![TransferPoint::default(); 61];
        t.read(0.0, &mut out);
        let mean = out.iter().map(|p| p.coherence).sum::<f64>() / out.len() as f64;
        assert!(mean < 0.3, "mean coherence {mean}");
    }

    /// Noise stepping between 0 and −15 dB every half second, as music moves between loud and
    /// soft passages.
    fn stepped(n: usize, seed: u32) -> Vec<f32> {
        let step = (FS * 0.5) as usize;
        noise(n, seed).into_iter().enumerate().map(|(i, v)| if (i / step) % 2 == 0 { v } else { v * 0.178 }).collect()
    }

    fn spread_of(x: &[f32], y: &[f32]) -> Vec<TransferPoint> {
        let mut t = Transfer::new(8192, FS, 61, 1.0 / 24.0);
        let mut end = 8192;
        while end <= x.len() {
            t.feed(&[&x[end - 8192..end]], &[&y[end - 8192..end]], 0.05);
            end += 2048;
        }
        let mut out = vec![TransferPoint::default(); 61];
        t.read(0.0, &mut out);
        out
    }

    #[test]
    fn a_filter_reads_the_same_loud_and_quiet() {
        let x = stepped(FS as usize * 12, 31);
        let y = biquad(&allpass(300.0, 0.7), &biquad(&lowpass(3000.0, 0.7), &x));
        for p in spread_of(&x, &y).iter().filter(|p| p.freq > 40.0 && p.freq < 8000.0) {
            // Under the editor's 0.03, where it starts to fade the phase.
            assert!(p.level_spread < 0.02, "{} Hz: spread {}", p.freq, p.level_spread);
        }
    }

    #[test]
    fn a_soft_clip_reads_differently_loud_and_quiet() {
        // Coherent enough to pass as a filter (γ² > 0.9), but loud passages are squashed more.
        let x = stepped(FS as usize * 12, 37);
        let y: Vec<f32> = x.iter().map(|v| (4.0 * v).tanh() * 0.25).collect();
        let pts = spread_of(&x, &y);
        let band: Vec<_> = pts.iter().filter(|p| p.freq > 40.0 && p.freq < 8000.0).collect();
        let mean = band.iter().map(|p| p.level_spread).sum::<f64>() / band.len() as f64;
        let coh = band.iter().map(|p| p.coherence).sum::<f64>() / band.len() as f64;
        assert!(coh > 0.9, "coherence {coh}");
        assert!(mean > 0.15, "spread {mean}");
    }

    #[test]
    fn a_silent_reference_learns_nothing() {
        let z = vec![0.0f32; 8192];
        let y = noise(8192, 1);
        let mut t = Transfer::new(8192, FS, 61, 1.0 / 24.0);
        assert!(!t.feed(&[&z], &[&y], 0.5));
        assert!(!t.has_data());
    }
}
