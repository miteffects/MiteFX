//! Oversampling by 2, 4 or 8: cascaded polyphase half-band FIRs, linear phase, so the round trip is
//! a pure delay of a whole number of samples at the base rate (the plugin reports it, and the dry
//! path stays aligned for mix and bypass). Shared by every plugin that needs it (BLUEPRINT §4).
//!
//! Each 2× stage is a Kaiser-windowed half-band: everything below 0.9 × the base Nyquist passes
//! (flat within 0.001 dB), and what would fold back into that band is at least 100 dB down. The
//! first stage carries the steep transition; the second (2× → 4×) and third (4× → 8×) only have
//! to clear the stage before's images, so they are short.
//!
//! A half-band's taps are zero at every even offset from the centre but the centre itself, so
//! each polyphase branch is either a pure delay or a symmetric FIR over the odd offsets: `K` tap
//! pairs per two high-rate samples. Allocation-free after construction.

/// The largest factor (the engines size their buffers for it).
pub const MAX_FACTOR: usize = 8;
/// Tap pairs of each stage.
const PAIRS: [usize; 3] = [35, 7, 5];
/// Kaiser β of each stage (about 105 dB of stopband).
const BETA: [f64; 3] = [10.5, 10.5, 10.5];

/// The modified Bessel function I₀ (for the Kaiser window).
fn bessel_i0(x: f64) -> f64 {
    let (mut sum, mut term, mut k) = (1.0, 1.0, 1.0);
    while term > 1e-12 * sum {
        term *= (x / (2.0 * k)) * (x / (2.0 * k));
        sum += term;
        k += 1.0;
    }
    sum
}

/// The side taps `a_j` (j = 0…pairs−1, at odd offsets ±(2j + 1) from the centre) of a half-band
/// with `pairs` tap pairs: a windowed `0.5 · sinc(n / 2)`, its centre tap 0.5.
fn half_band(pairs: usize, beta: f64) -> Vec<f32> {
    let half = (2 * pairs - 1) as f64;
    (0..pairs)
        .map(|j| {
            let n = (2 * j + 1) as f64;
            let sinc = (std::f64::consts::FRAC_PI_2 * n).sin() / (std::f64::consts::FRAC_PI_2 * n);
            let w = bessel_i0(beta * (1.0 - (n / (half + 1.0)).powi(2)).max(0.0).sqrt()) / bessel_i0(beta);
            (0.5 * sinc * w) as f32
        })
        .collect()
}

/// A delay line read as contiguous slices (stored twice), newest at `pos`.
#[derive(Clone)]
struct History {
    data: Vec<f32>,
    len: usize,
    pos: usize,
}

impl History {
    fn new(len: usize) -> Self {
        Self { data: vec![0.0; 2 * len], len, pos: 0 }
    }

    #[inline]
    fn push(&mut self, x: f32) {
        self.pos = if self.pos == 0 { self.len - 1 } else { self.pos - 1 };
        self.data[self.pos] = x;
        self.data[self.pos + self.len] = x;
    }

    /// The last `len` samples, newest first.
    #[inline]
    fn recent(&self) -> &[f32] {
        &self.data[self.pos..self.pos + self.len]
    }

    fn clear(&mut self) {
        self.data.fill(0.0);
        self.pos = 0;
    }

    fn copy_from(&mut self, other: &Self) {
        self.data.copy_from_slice(&other.data);
        self.pos = other.pos;
    }
}

/// Lanes of the dot product (the window and the taps are padded to a multiple of it).
const LANES: usize = 8;

/// The symmetric branch of a half-band, `Σ a_j (x[K−1−j] + x[K+j])` over a newest-first window
/// `x`, as a plain dot product with the mirrored taps `full` (`a` reversed, then `a`, then zeros
/// to a multiple of [`LANES`]): eight running sums the compiler keeps in vector registers.
#[inline]
fn symmetric(full: &[f32], x: &[f32]) -> f32 {
    let mut acc = [0.0f32; LANES];
    for (c, v) in full.chunks_exact(LANES).zip(x.chunks_exact(LANES)) {
        for l in 0..LANES {
            acc[l] += c[l] * v[l];
        }
    }
    let [a, b, c, d, e, f, g, h] = acc;
    ((a + e) + (b + f)) + ((c + g) + (d + h))
}

/// `Σ a_j (x[K−1−j] + x[K+j])` folded, one tap at a time: quicker than [`symmetric`] for the
/// short half-bands of the later stages (padding them to the lanes would add work).
#[inline]
fn folded(a: &[f32], x: &[f32]) -> f32 {
    let k = a.len();
    let mut acc = 0.0f32;
    for (j, &c) in a.iter().enumerate() {
        acc += c * (x[k - 1 - j] + x[k + j]);
    }
    acc
}

/// From this many pairs a stage runs [`symmetric`], below it [`folded`].
const VECTOR_PAIRS: usize = 16;

/// `a` mirrored for [`symmetric`]: `a[K−1] … a[0] a[0] … a[K−1]`, zero-padded.
fn mirrored(a: &[f32]) -> Vec<f32> {
    let mut full: Vec<f32> = a.iter().rev().chain(a.iter()).copied().collect();
    full.resize(full.len().div_ceil(LANES) * LANES, 0.0);
    full
}

/// One 2× stage for one channel: an interpolator and a decimator sharing the taps.
#[derive(Clone)]
struct Stage {
    /// The half-band's taps for one side (`K` of them), and mirrored for [`symmetric`] (long
    /// half-bands only).
    taps: Vec<f32>,
    full: Option<Vec<f32>>,
    /// Interpolator input (base rate of this stage), and the decimator's even and odd inputs
    /// (the high rate's samples split by phase).
    up: History,
    even: History,
    odd: History,
}

impl Stage {
    fn new(pairs: usize, beta: f64) -> Self {
        let taps = half_band(pairs, beta);
        let full = (pairs >= VECTOR_PAIRS).then(|| mirrored(&taps));
        let len = full.as_ref().map_or(2 * pairs, Vec::len);
        Self { taps, full, up: History::new(len), even: History::new(len), odd: History::new(len) }
    }

    #[inline]
    fn branch(&self, x: &[f32]) -> f32 {
        match &self.full {
            Some(full) => symmetric(full, x),
            None => folded(&self.taps, x),
        }
    }

    fn pairs(&self) -> usize {
        self.taps.len()
    }

    /// One input sample in, two out (gain 2 on the interpolation, so the level holds).
    #[inline]
    fn interpolate(&mut self, x: f32) -> [f32; 2] {
        self.up.push(x);
        let h = self.up.recent();
        let k = self.pairs();
        [2.0 * self.branch(h), h[k - 1]]
    }

    /// Two input samples in, one out.
    #[inline]
    fn decimate(&mut self, pair: [f32; 2]) -> f32 {
        self.even.push(pair[0]);
        self.odd.push(pair[1]);
        // y[i] = ½·odd[i−K] + Σ a_j (even[i−K+1+j] + even[i−K−j]). Newest first, even[i−K+1+j]
        // sits at index K−1−j and even[i−K−j] at K+j: the interpolator's symmetric window.
        let k = self.pairs();
        0.5 * self.odd.recent()[k] + self.branch(self.even.recent())
    }

    fn clear(&mut self) {
        self.up.clear();
        self.even.clear();
        self.odd.clear();
    }

    fn copy_from(&mut self, other: &Self) {
        self.up.copy_from(&other.up);
        self.even.copy_from(&other.even);
        self.odd.copy_from(&other.odd);
    }
}

/// Up to two channels oversampled by `factor` (1, 2, 4 or 8), round trip a pure delay of
/// [`Oversampler::latency`] base-rate samples.
pub struct Oversampler {
    factor: usize,
    /// Per channel, the 2× stage, the 4× one and the 8× one.
    stages: [[Stage; 3]; 2],
    /// Per channel, the half-sample pad that makes the latency whole: one sample at the 2× rate
    /// at 4×, one at the 4× rate at 8×.
    pad: [History; 2],
}

impl Oversampler {
    /// Allocates; `factor` can change later without allocating.
    pub fn new(factor: usize) -> Self {
        let stage = |i: usize| Stage::new(PAIRS[i], BETA[i]);
        let mut o = Self { factor: 1, stages: [[stage(0), stage(1), stage(2)], [stage(0), stage(1), stage(2)]], pad: [History::new(1), History::new(1)] };
        o.set_factor(factor);
        o
    }

    /// 1, 2, 4 or 8 (anything else is taken as the nearest below). Clears the state.
    pub fn set_factor(&mut self, factor: usize) {
        self.factor = match factor {
            0 | 1 => 1,
            2 | 3 => 2,
            4..=7 => 4,
            _ => 8,
        };
        self.reset();
    }

    pub fn factor(&self) -> usize {
        self.factor
    }

    pub fn reset(&mut self) {
        for s in self.stages.iter_mut().flatten() {
            s.clear();
        }
        for p in &mut self.pad {
            p.clear();
        }
    }

    /// Becomes a copy of `other`'s factor and state, without allocating.
    pub fn copy_from(&mut self, other: &Self) {
        self.factor = other.factor;
        for (a, b) in self.stages.iter_mut().flatten().zip(other.stages.iter().flatten()) {
            a.copy_from(b);
        }
        for (a, b) in self.pad.iter_mut().zip(&other.pad) {
            a.copy_from(b);
        }
    }

    /// The round trip's delay, base-rate samples.
    pub fn latency(&self) -> usize {
        Self::latency_for(self.factor)
    }

    /// The round trip's delay at `factor`, base-rate samples: each stage's interpolator and
    /// decimator delay its centre tap, `2K − 1` samples at its high rate each; the second
    /// stage's `2K − 1` at the 2× rate is padded to `2K`; at 8× the second and third stages'
    /// `2K₂ − 1 + (2K₃ − 1)/2` at the 2× rate are padded by one sample at the 4× rate.
    pub fn latency_for(factor: usize) -> usize {
        match factor {
            0 | 1 => 0,
            2 | 3 => 2 * PAIRS[0] - 1,
            4..=7 => 2 * PAIRS[0] - 1 + PAIRS[1],
            _ => 2 * PAIRS[0] - 1 + (4 * PAIRS[1] + 2 * PAIRS[2]) / 4,
        }
    }

    /// `input` (base rate) of channel `ch` into `out` (`factor` × as long).
    pub fn up(&mut self, ch: usize, input: &[f32], out: &mut [f32]) {
        let [first, second, third] = &mut self.stages[ch];
        match self.factor {
            1 => out[..input.len()].copy_from_slice(input),
            2 => {
                for (x, o) in input.iter().zip(out.chunks_exact_mut(2)) {
                    o.copy_from_slice(&first.interpolate(*x));
                }
            }
            4 => {
                for (x, o) in input.iter().zip(out.chunks_exact_mut(4)) {
                    let [a, b] = first.interpolate(*x);
                    o[..2].copy_from_slice(&second.interpolate(a));
                    o[2..].copy_from_slice(&second.interpolate(b));
                }
            }
            _ => {
                for (x, o) in input.iter().zip(out.chunks_exact_mut(8)) {
                    let [a, b] = first.interpolate(*x);
                    let [p, q] = second.interpolate(a);
                    let [r, s] = second.interpolate(b);
                    for (j, v) in [p, q, r, s].into_iter().enumerate() {
                        o[2 * j..2 * j + 2].copy_from_slice(&third.interpolate(v));
                    }
                }
            }
        }
    }

    /// `input` (`factor` × the base rate) of channel `ch` into `out` (base rate).
    pub fn down(&mut self, ch: usize, input: &[f32], out: &mut [f32]) {
        let [first, second, third] = &mut self.stages[ch];
        let pad = &mut self.pad[ch];
        match self.factor {
            1 => {
                let n = out.len().min(input.len());
                out[..n].copy_from_slice(&input[..n]);
            }
            2 => {
                for (o, x) in out.iter_mut().zip(input.chunks_exact(2)) {
                    *o = first.decimate([x[0], x[1]]);
                }
            }
            4 => {
                for (o, x) in out.iter_mut().zip(input.chunks_exact(4)) {
                    let a = second.decimate([x[0], x[1]]);
                    let b = second.decimate([x[2], x[3]]);
                    // One sample of delay at the 2× rate: the newer half of the pair waits.
                    let held = pad.recent()[0];
                    pad.push(b);
                    *o = first.decimate([held, a]);
                }
            }
            _ => {
                for (o, x) in out.iter_mut().zip(input.chunks_exact(8)) {
                    let c = [third.decimate([x[0], x[1]]), third.decimate([x[2], x[3]]), third.decimate([x[4], x[5]]), third.decimate([x[6], x[7]])];
                    // One sample of delay at the 4× rate.
                    let held = pad.recent()[0];
                    pad.push(c[3]);
                    let a = second.decimate([held, c[0]]);
                    let b = second.decimate([c[1], c[2]]);
                    *o = first.decimate([a, b]);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The round trip of a sine at `f` (fraction of the base rate) at `factor`: gain (dB) after
    /// the latency, and the worst sample error against the delayed sine.
    fn round_trip(factor: usize, f: f64) -> (f64, f64) {
        let mut o = Oversampler::new(factor);
        let n = 4096;
        let x: Vec<f32> = (0..n).map(|i| (std::f64::consts::TAU * f * i as f64).sin() as f32 * 0.5).collect();
        let mut hi = vec![0.0; n * factor];
        let mut y = vec![0.0; n];
        o.up(0, &x, &mut hi);
        o.down(0, &hi, &mut y);
        let lat = o.latency();
        let settled = 1024;
        let (mut err, mut px, mut py) = (0.0f64, 0.0f64, 0.0f64);
        for i in settled..n {
            err = err.max(f64::from((y[i] - x[i - lat]).abs()));
            px += f64::from(x[i - lat]).powi(2);
            py += f64::from(y[i]).powi(2);
        }
        (10.0 * (py / px).log10(), err)
    }

    #[test]
    fn the_round_trip_is_a_pure_delay_in_the_passband() {
        for factor in [2, 4, 8] {
            for f in [0.001, 0.05, 0.2, 0.35, 0.405] {
                let (gain, err) = round_trip(factor, f);
                assert!(gain.abs() < 0.001, "{factor}× at {f}: {gain} dB");
                // Aligned to the reported latency: the error is the ripple, not a phase slip.
                assert!(err < 2e-4, "{factor}× at {f}: {err}");
            }
        }
        let (gain, err) = round_trip(1, 0.3);
        assert!(gain.abs() < 1e-9 && err == 0.0);
    }

    #[test]
    fn what_folds_back_is_more_than_100_db_down() {
        // A tone at the high rate above 1.1 × the base Nyquist (as a compressor's sidebands land),
        // decimated: whatever folds into the base band must be gone.
        for factor in [2, 4, 8] {
            let mut o = Oversampler::new(factor);
            let n = 8192;
            let mut worst = f64::MIN;
            // Multiples of the base Nyquist, up to the high rate's Nyquist.
            for f_nyq in [1.12, 1.3, 1.6, 1.95, 2.5, 3.0, 3.6, 3.95, 4.5, 5.5, 6.4, 7.1, 7.9] {
                if f_nyq >= factor as f64 {
                    continue;
                }
                let f = f_nyq * 0.5 / factor as f64;
                o.reset();
                let hi: Vec<f32> = (0..n * factor).map(|i| (std::f64::consts::TAU * f * i as f64).sin() as f32).collect();
                let mut y = vec![0.0; n];
                o.down(0, &hi, &mut y);
                let rms = (y[2048..].iter().map(|v| f64::from(*v).powi(2)).sum::<f64>() / (n - 2048) as f64).sqrt();
                worst = worst.max(20.0 * (rms * std::f64::consts::SQRT_2).log10());
            }
            assert!(worst < -100.0, "{factor}×: aliases at {worst:.1} dB");
        }
    }

    #[test]
    fn images_of_the_interpolation_are_more_than_100_db_down() {
        for factor in [2, 4, 8] {
            let mut o = Oversampler::new(factor);
            let n = 8192;
            let f = 0.3; // of the base rate: 0.6 of the base Nyquist
            let x: Vec<f32> = (0..n).map(|i| (std::f64::consts::TAU * f * i as f64).sin() as f32).collect();
            let mut hi = vec![0.0; n * factor];
            o.up(0, &x, &mut hi);
            // Correlate with the image at (1 − f) of the base rate, and its 4× images.
            let images: Vec<f64> = match factor {
                2 => vec![1.0 - f],
                4 => vec![1.0 - f, 1.0 + f, 2.0 - f],
                _ => vec![1.0 - f, 1.0 + f, 2.0 - f, 2.0 + f, 3.0 - f, 3.0 + f, 4.0 - f],
            };
            for img in images {
                let w = std::f64::consts::TAU * img / factor as f64;
                // A Blackman-Harris window, so the tone itself does not leak into the image's bin.
                let tail = &hi[4096..];
                let m = tail.len() as f64;
                let win = |i: usize| {
                    let x = std::f64::consts::TAU * i as f64 / m;
                    0.35875 - 0.48829 * x.cos() + 0.14128 * (2.0 * x).cos() - 0.01168 * (3.0 * x).cos()
                };
                let (re, im) = tail.iter().enumerate().fold((0.0, 0.0), |(re, im), (i, &v)| (re + win(i) * f64::from(v) * (w * i as f64).cos(), im + win(i) * f64::from(v) * (w * i as f64).sin()));
                let level = 20.0 * (2.0 * (re * re + im * im).sqrt() / (0.35875 * m)).log10();
                assert!(level < -100.0, "{factor}×: image at {img} of the base rate is {level:.1} dB");
            }
        }
    }

    #[test]
    fn changing_the_factor_clears_and_keeps_the_latency_whole() {
        assert_eq!(Oversampler::latency_for(1), 0);
        assert_eq!(Oversampler::latency_for(2), 69);
        assert_eq!(Oversampler::latency_for(4), 76);
        assert_eq!(Oversampler::latency_for(8), 78);
        let mut o = Oversampler::new(2);
        o.set_factor(4);
        assert_eq!((o.factor(), o.latency()), (4, 76));
        o.set_factor(8);
        assert_eq!((o.factor(), o.latency()), (8, 78));
    }
}
