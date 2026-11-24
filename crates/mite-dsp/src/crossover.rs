//! The band split shared by Mite Shape and Mite Sat (and later Mite Multiband): a cascade of LR4
//! crossovers with allpass compensation, so the bands always sum back to an allpass of the input.
//!
//! Slots never swap: crossover slot k, when on, owns band slot k + 1, the region above it up to
//! the next crossover; band slot 0 is always the lowest region. `B` is the most bands a plugin
//! splits into (`B − 1` crossover slots); arrays are sized `B` and the last crossover entry is
//! unused.

use std::f64::consts::{FRAC_1_SQRT_2, PI, SQRT_2};

use crate::filter::Coeffs;

pub const FREQ_MIN: f32 = 20.0;
pub const FREQ_MAX: f32 = 20_000.0;
/// Neighbouring crossovers stay at least this far apart, octaves (they are pushed, never cross).
pub const MIN_GAP_OCT: f32 = 1.0 / 3.0;
/// Between the caller's updates a moving crossover glides there sample by sample (one pole, this
/// long, ms): a filter retuned in steps clicks on every step.
const GLIDE_MS: f64 = 0.5;

/// One crossover: a Butterworth pair (LR4: each half run twice) and the allpass their sum makes
/// (for the bands below it), all on state-variable sections (TPT), whose state stays valid while
/// the frequency moves. Direct-form biquads retuned every 16 samples while swept clicked 25–55 dB
/// over the music (2026-11-24); these stay within a few dB. Same responses as the bilinear biquads.
#[derive(Debug, Clone, Copy)]
pub struct Cross {
    /// `tan(π f / fs)`, the sections' tuning.
    pub g: f64,
    /// The per-sample glide toward `g`, and the sections' gains at `g`.
    glide: f64,
    tune: Tune,
    /// The same Butterworth halves and allpass as direct-form coefficients (for a fixed filter
    /// elsewhere, e.g. a DC high-pass).
    pub lp: Coeffs,
    pub hp: Coeffs,
    pub ap: Coeffs,
}

impl Cross {
    pub fn new(f: f64, fs: f64) -> Self {
        let k = (PI * f.min(0.45 * fs) / fs).tan();
        let q = FRAC_1_SQRT_2;
        let norm = 1.0 / (1.0 + k / q + k * k);
        let a1 = 2.0 * (k * k - 1.0) * norm;
        let a2 = (1.0 - k / q + k * k) * norm;
        let lp = Coeffs {
            b0: k * k * norm,
            b1: 2.0 * k * k * norm,
            b2: k * k * norm,
            a1,
            a2,
        };
        let hp = Coeffs {
            b0: norm,
            b1: -2.0 * norm,
            b2: norm,
            a1,
            a2,
        };
        let ap = Coeffs {
            b0: a2,
            b1: a1,
            b2: 1.0,
            a1,
            a2,
        };
        let glide = 1.0 - (-1000.0 / (GLIDE_MS * fs)).exp();
        Self { g: k, glide, tune: Tune::at(k), lp, hp, ap }
    }
}

/// A state-variable section's gains at tuning `g` (Butterworth damping, k = √2).
#[derive(Debug, Clone, Copy, Default)]
struct Tune {
    a1: f64,
    a2: f64,
    a3: f64,
}

impl Tune {
    #[inline]
    fn at(g: f64) -> Self {
        let a1 = 1.0 / (1.0 + g * (g + SQRT_2));
        let a2 = g * a1;
        Self { a1, a2, a3: g * a2 }
    }
}

/// A state-variable (TPT) section's state.
#[derive(Debug, Clone, Copy, Default)]
struct Svf {
    s1: f64,
    s2: f64,
}

impl Svf {
    /// One sample: (low-pass, band-pass, high-pass).
    #[inline]
    fn tick(&mut self, t: &Tune, x: f64) -> (f64, f64, f64) {
        let v3 = x - self.s2;
        let v1 = t.a1 * self.s1 + t.a2 * v3;
        let v2 = self.s2 + t.a2 * self.s1 + t.a3 * v3;
        self.s1 = 2.0 * v1 - self.s1;
        self.s2 = 2.0 * v2 - self.s2;
        (v2, v1, x - SQRT_2 * v1 - v2)
    }

    #[inline]
    fn allpass(&mut self, t: &Tune, x: f64) -> f64 {
        let (_, bp, _) = self.tick(t, x);
        x - 2.0 * SQRT_2 * bp
    }

    fn combine(&self, other: &Self, ka: f64, kb: f64) -> Self {
        Self {
            s1: ka * self.s1 + kb * other.s1,
            s2: ka * self.s2 + kb * other.s2,
        }
    }
}

/// The split the crossovers make: the active crossover slots in frequency order, and the band
/// slot of each region from the bottom.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Layout<const B: usize> {
    /// The active crossover slots from the bottom (the first `n`).
    pub crossovers: [usize; B],
    pub n: usize,
    /// The band slot of each region from the bottom (the first `n + 1`).
    pub owners: [usize; B],
}

impl<const B: usize> Layout<B> {
    /// `crossovers`: each slot's frequency, Hz, or `None` while it is off (at most `B − 1`).
    pub fn new(crossovers: &[Option<f32>]) -> Self {
        let mut order = [0usize; B];
        let mut n = 0;
        for (k, f) in crossovers.iter().enumerate().take(B - 1) {
            if f.is_some() {
                order[n] = k;
                n += 1;
            }
        }
        let freq = |k: usize| crossovers[k].unwrap_or(0.0);
        order[..n].sort_by(|&a, &b| freq(a).total_cmp(&freq(b)).then(a.cmp(&b)));
        let mut owners = [0usize; B];
        for p in 0..n {
            owners[p + 1] = order[p] + 1;
        }
        Self {
            crossovers: order,
            n,
            owners,
        }
    }

    /// The bands in use, from the bottom.
    pub fn bands(&self) -> &[usize] {
        &self.owners[..=self.n]
    }

    /// Where band slot `band` sits from the bottom, if it is in use.
    pub fn position(&self, band: usize) -> Option<usize> {
        self.bands().iter().position(|&b| b == band)
    }

    /// Band slot `band`'s region, Hz, if it is in use.
    pub fn region(&self, band: usize, crossovers: &[Option<f32>]) -> Option<(f32, f32)> {
        let p = self.position(band)?;
        let edge = |q: usize| crossovers[self.crossovers[q]].unwrap_or(FREQ_MIN);
        let lo = if p == 0 { FREQ_MIN } else { edge(p - 1) };
        let hi = if p == self.n { FREQ_MAX } else { edge(p) };
        Some((lo, hi))
    }

    /// Each band slot's share of the signal at `f` Hz (LR4 magnitudes, in phase, so they add to 1).
    pub fn weights(&self, crossovers: &[Option<f32>], f: f64) -> [f64; B] {
        let mut w = [0.0; B];
        let mut rest = 1.0;
        for p in 0..self.n {
            let fc = f64::from(crossovers[self.crossovers[p]].unwrap_or(FREQ_MIN));
            let r4 = (f / fc).powi(4);
            let (lo, hi) = (1.0 / (1.0 + r4), r4 / (1.0 + r4));
            w[self.owners[p]] = rest * lo;
            rest *= hi;
        }
        w[self.owners[self.n]] = rest;
        w
    }

    /// The band slot whose region holds `f` Hz.
    pub fn band_at(&self, crossovers: &[Option<f32>], f: f32) -> usize {
        (0..=self.n)
            .find(|&q| {
                let (lo, hi) = self
                    .region(self.owners[q], crossovers)
                    .unwrap_or((FREQ_MIN, FREQ_MAX));
                f >= lo && f < hi
            })
            .map_or(0, |q| self.owners[q])
    }
}

/// The filter states of a split, for two channels. Allocation-free.
#[derive(Debug, Clone, Copy)]
pub struct Split<const B: usize> {
    /// Crossover sections by channel and crossover slot: the first stage (its low- and high-pass
    /// both), then the second low-pass and the second high-pass.
    cross: [[[Svf; 3]; B]; 2],
    /// Allpass compensation by channel, band slot and the crossover slot above it.
    ap: [[[Svf; B]; B]; 2],
    /// Each crossover's tuning now, by channel and slot, gliding to its section's (0: not yet
    /// heard, it starts there).
    g: [[f64; B]; 2],
}

impl<const B: usize> Default for Split<B> {
    fn default() -> Self {
        Self {
            cross: [[[Svf::default(); 3]; B]; 2],
            ap: [[[Svf::default(); B]; B]; 2],
            g: [[0.0; B]; 2],
        }
    }
}

impl<const B: usize> Split<B> {
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// Splits one sample of channel `c` into `out` by position from the bottom (`layout.n + 1`
    /// bands; with none, `out[0]` is `x`). `sections` by crossover slot.
    #[inline]
    #[allow(clippy::needless_range_loop)] // positions index the layout and `out` alike
    pub fn process(
        &mut self,
        c: usize,
        x: f64,
        layout: &Layout<B>,
        sections: &[Cross],
        out: &mut [f64; B],
    ) {
        let mut tunes = [Tune::default(); B];
        for p in 0..layout.n {
            let k = layout.crossovers[p];
            let x = &sections[k];
            let g = &mut self.g[c][k];
            tunes[k] = if *g == 0.0 || (x.g - *g).abs() <= 1e-9 * x.g {
                *g = x.g;
                x.tune
            } else {
                *g += x.glide * (x.g - *g);
                Tune::at(*g)
            };
        }
        let mut rest = x;
        for p in 0..layout.n {
            let k = layout.crossovers[p];
            let st = &mut self.cross[c][k];
            let t = &tunes[k];
            let (lo, _, hi) = st[0].tick(t, rest);
            out[p] = st[1].tick(t, lo).0;
            rest = st[2].tick(t, hi).2;
        }
        out[layout.n] = rest;
        for p in 0..layout.n {
            let owner = layout.owners[p];
            for q in p + 1..layout.n {
                let k = layout.crossovers[q];
                out[p] = self.ap[c][owner][k].allpass(&tunes[k], out[p]);
            }
        }
    }

    /// Carries every state across an L/R ↔ M/S switch (the filters are linear): to M/S the
    /// channels become (L + R)/2 and (L − R)/2, back they become M + S and M − S.
    pub fn switch_mid_side(&mut self, to_mid_side: bool) {
        let (ka, kb) = if to_mid_side { (0.5, 0.5) } else { (1.0, 1.0) };
        let mix = |a: &mut Svf, b: &mut Svf| {
            let (x, y) = (*a, *b);
            *a = x.combine(&y, ka, kb);
            *b = x.combine(&y, ka, -kb);
        };
        let (l, r) = self.cross.split_at_mut(1);
        for (a, b) in l[0].iter_mut().flatten().zip(r[0].iter_mut().flatten()) {
            mix(a, b);
        }
        let (l, r) = self.ap.split_at_mut(1);
        for (a, b) in l[0].iter_mut().flatten().zip(r[0].iter_mut().flatten()) {
            mix(a, b);
        }
    }

    /// Moves from `old` to `new`: each crossover just switched on starts from rest, and so do the
    /// allpasses of the band it makes. Returns, by band slot, the slot each new band was split
    /// from (in `old`), so the caller can carry that band's own state across.
    pub fn restructure(
        &mut self,
        old: &Layout<B>,
        new: &Layout<B>,
        crossovers: &[Option<f32>],
    ) -> [Option<usize>; B] {
        let mut parents = [None; B];
        let was_on = |k: usize| old.crossovers[..old.n].contains(&k);
        for p in 0..new.n {
            let k = new.crossovers[p];
            if was_on(k) {
                continue;
            }
            let f = crossovers[k].unwrap_or(FREQ_MIN);
            parents[k + 1] = Some(old.band_at(crossovers, f));
            for c in 0..2 {
                self.cross[c][k] = [Svf::default(); 3];
                self.g[c][k] = 0.0;
                for b in 0..B {
                    self.ap[c][b][k] = Svf::default();
                }
                self.ap[c][k + 1] = [Svf::default(); B];
            }
        }
        parents
    }
}

/// The share of each half of a linear-phase band kernel its window spends tapering (Tukey).
const LP_TAPER: f64 = 0.3;
/// The design grid, as a multiple of the kernel length (less time aliasing of the long low bands).
const LP_GRID: usize = 4;

/// The split as linear-phase FIR kernels (Mite Sat's Linear Phase; built off the audio thread,
/// allocates): by band slot, `len` taps (a power of two) centred on `len / 2`, each band the same
/// LR4 magnitude as the minimum-phase split's ([`Layout::weights`]) with zero phase. They add up
/// to a delay of `len / 2` exactly: the top band is that delay less the others, so the window's
/// smoothing of the lower bands moves into it instead of into the sum. Slots the layout does not
/// use get all-zero kernels.
pub fn linear_phase_kernels<const B: usize>(layout: &Layout<B>, crossovers: &[Option<f32>], fs: f64, len: usize) -> [Vec<f64>; B] {
    assert!(len.is_power_of_two() && len >= 8, "kernel length must be a power of two");
    let mut out: [Vec<f64>; B] = std::array::from_fn(|_| vec![0.0; len]);
    let half = len / 2;
    let fine = LP_GRID * len;
    let ifft = realfft::RealFftPlanner::<f64>::new().plan_fft_inverse(fine);
    let mut spectrum = ifft.make_input_vec();
    let mut h = ifft.make_output_vec();
    let window = |m: usize| {
        // m = 0 … half − 1 from the centre; flat, then a cosine to zero at the ends.
        let t = m as f64 / half as f64;
        let edge = 1.0 - LP_TAPER;
        if t <= edge { 1.0 } else { 0.5 + 0.5 * (PI * (t - edge) / LP_TAPER).cos() }
    };
    let top = layout.owners[layout.n];
    for p in 0..layout.n {
        let slot = layout.owners[p];
        for (k, v) in spectrum.iter_mut().enumerate() {
            let f = k as f64 * fs / fine as f64;
            *v = realfft::num_complex::Complex::new(layout.weights(crossovers, f)[slot] / fine as f64, 0.0);
        }
        if ifft.process(&mut spectrum, &mut h).is_err() {
            continue;
        }
        let kernel = &mut out[slot];
        // Zero phase: h is symmetric about index 0 (and fine − m mirrors m).
        kernel[half] = h[0];
        for m in 1..half {
            let w = window(m);
            kernel[half + m] = h[m] * w;
            kernel[half - m] = h[fine - m] * w;
        }
    }
    let rest: Vec<f64> = (0..len)
        .map(|i| {
            let below: f64 = (0..layout.n).map(|p| out[layout.owners[p]][i]).sum();
            f64::from(u8::from(i == half)) - below
        })
        .collect();
    out[top] = rest;
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn six_bands_sum_flat() {
        let x = [
            Some(120.0),
            Some(500.0),
            Some(2000.0),
            Some(6000.0),
            Some(12_000.0),
        ];
        let layout = Layout::<6>::new(&x);
        assert_eq!(layout.bands(), &[0, 1, 2, 3, 4, 5]);
        let fs = 48_000.0;
        let sections: Vec<Cross> = x
            .iter()
            .map(|f| Cross::new(f64::from(f.unwrap()), fs))
            .chain([Cross::new(1000.0, fs)])
            .collect();
        let mut split = Split::<6>::default();
        let mut y = vec![0.0f64; 16_384];
        let mut out = [0.0; 6];
        for (i, v) in y.iter_mut().enumerate() {
            split.process(
                0,
                if i == 0 { 1.0 } else { 0.0 },
                &layout,
                &sections,
                &mut out,
            );
            *v = out.iter().sum();
        }
        for f in [30.0, 120.0, 700.0, 2000.0, 9000.0, 16_000.0] {
            let w = 2.0 * PI * f / fs;
            let (re, im) = y.iter().enumerate().fold((0.0, 0.0), |(re, im), (i, &v)| {
                (re + v * (w * i as f64).cos(), im - v * (w * i as f64).sin())
            });
            let db = 10.0 * (re * re + im * im).log10();
            assert!(db.abs() < 0.01, "{f} Hz: {db:.4} dB");
        }
    }

    /// A crossover swept 20 kHz → 20 Hz (and back, and across a few octaves) over a 220 Hz tone,
    /// retuned every 16 samples on a 20 ms glide (as the plugins drive it): no band and not the sum
    /// may have a sharper edge (largest second difference) during the sweep than the tone itself
    /// has, by more than 6 dB. The direct-form biquads it replaced reached +25…+55 dB.
    #[test]
    fn a_swept_crossover_does_not_click() {
        let fs = 48_000.0f64;
        let n = (0.9 * fs) as usize;
        let jump = (0.4 * fs) as usize + 37;
        let w = (0.1 * fs) as usize;
        let glide = 1.0 - (-16.0 * 1000.0 / (20.0 * fs)).exp();
        let edge = |y: &[f64], r: std::ops::Range<usize>| r.map(|i| (y[i] - 2.0 * y[i - 1] + y[i - 2]).abs()).fold(0.0, f64::max);
        for (from, to) in [(20_000.0f64, 20.0f64), (20.0, 20_000.0), (2000.0, 60.0), (60.0, 2000.0)] {
            let layout = Layout::<3>::new(&[Some(1000.0), None]);
            let mut split = Split::<3>::default();
            let mut lf = from.ln();
            let mut sections = [Cross::new(from, fs); 3];
            let mut bands = [vec![0.0; n], vec![0.0; n], vec![0.0; n]];
            let mut tone = vec![0.0; n];
            for i in 0..n {
                let x = 0.25 * (2.0 * PI * 220.0 * i as f64 / fs).sin();
                tone[i] = x;
                let target = if i >= jump { to.ln() } else { from.ln() };
                if i % 16 == 0 {
                    lf += glide * (target - lf);
                    sections[0] = Cross::new(lf.exp(), fs);
                }
                let mut out = [0.0; 3];
                split.process(0, x, &layout, &sections, &mut out);
                bands[0][i] = out[0];
                bands[1][i] = out[1];
                bands[2][i] = out[0] + out[1];
            }
            let floor = edge(&tone, jump - w..jump);
            for (name, y) in ["low", "high", "sum"].iter().zip(&bands) {
                let db = 20.0 * (edge(y, jump..jump + w) / floor).log10();
                assert!(db < 6.0, "{from} → {to} Hz, {name}: {db:+.1} dB");
            }
        }
    }

    #[test]
    fn a_new_crossover_names_the_band_it_splits() {
        let before = [Some(200.0), None, None];
        let after = [Some(200.0), None, Some(3000.0)];
        let (old, new) = (Layout::<4>::new(&before), Layout::<4>::new(&after));
        let parents = Split::<4>::default().restructure(&old, &new, &after);
        assert_eq!(parents, [None, None, None, Some(1)]);
    }

    /// A kernel's magnitude at `f` Hz, its delay taken out.
    fn kernel_gain(kernel: &[f64], f: f64, fs: f64) -> f64 {
        let w = 2.0 * PI * f / fs;
        let (re, im) = kernel.iter().enumerate().fold((0.0, 0.0), |(re, im), (i, &v)| (re + v * (w * i as f64).cos(), im - v * (w * i as f64).sin()));
        (re * re + im * im).sqrt()
    }

    #[test]
    fn linear_phase_bands_sum_to_a_pure_delay_and_follow_lr4() {
        let x = [Some(80.0), Some(500.0), None, Some(4000.0), Some(12_000.0)];
        let layout = Layout::<6>::new(&x);
        let fs = 48_000.0;
        let len = 8192;
        let k = linear_phase_kernels(&layout, &x, fs, len);
        // The sum is exactly a delay of len / 2.
        for i in 0..len {
            let sum: f64 = k.iter().map(|v| v[i]).sum();
            let want = if i == len / 2 { 1.0 } else { 0.0 };
            assert!((sum - want).abs() < 1e-12, "sum at {i}: {sum}");
        }
        // The unused slot is silent.
        assert!(k[3].iter().all(|&v| v == 0.0), "crossover slot 2 is off: band slot 3 is unused");
        // Each band is the minimum-phase split's magnitude, within 0.3 dB where it is over −30 dB.
        for f in [30.0, 80.0, 160.0, 500.0, 1500.0, 4000.0, 9000.0, 12_000.0, 18_000.0] {
            let want = layout.weights(&x, f);
            for slot in layout.bands() {
                let got = kernel_gain(&k[*slot], f, fs);
                if want[*slot] > 0.03 {
                    let db = 20.0 * (got / want[*slot]).log10();
                    assert!(db.abs() < 0.3, "slot {slot} at {f} Hz: {db:+.2} dB");
                }
            }
        }
        // Symmetric: linear phase about len / 2.
        for slot in layout.bands() {
            for m in 1..len / 2 {
                assert!((k[*slot][len / 2 + m] - k[*slot][len / 2 - m]).abs() < 1e-12);
            }
        }
    }
}
