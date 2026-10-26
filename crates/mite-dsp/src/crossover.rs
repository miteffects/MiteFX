//! The band split shared by Mite Shape and Mite Sat (and later Mite Multiband): a cascade of LR4
//! crossovers with allpass compensation, so the bands always sum back to an allpass of the input.
//!
//! Slots never swap: crossover slot k, when on, owns band slot k + 1, the region above it up to
//! the next crossover; band slot 0 is always the lowest region. `B` is the most bands a plugin
//! splits into (`B − 1` crossover slots); arrays are sized `B` and the last crossover entry is
//! unused.

use std::f64::consts::{FRAC_1_SQRT_2, PI};

use crate::filter::{Coeffs, Tdf2};

pub const FREQ_MIN: f32 = 20.0;
pub const FREQ_MAX: f32 = 20_000.0;
/// Neighbouring crossovers stay at least this far apart, octaves (they are pushed, never cross).
pub const MIN_GAP_OCT: f32 = 1.0 / 3.0;

/// One crossover's sections: Butterworth low- and high-pass (each run twice, LR4), and the
/// allpass their sum makes (for the bands below it).
#[derive(Debug, Clone, Copy)]
pub struct Cross {
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
        let lp = Coeffs { b0: k * k * norm, b1: 2.0 * k * k * norm, b2: k * k * norm, a1, a2 };
        let hp = Coeffs { b0: norm, b1: -2.0 * norm, b2: norm, a1, a2 };
        let ap = Coeffs { b0: a2, b1: a1, b2: 1.0, a1, a2 };
        Self { lp, hp, ap }
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
        Self { crossovers: order, n, owners }
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
                let (lo, hi) = self.region(self.owners[q], crossovers).unwrap_or((FREQ_MIN, FREQ_MAX));
                f >= lo && f < hi
            })
            .map_or(0, |q| self.owners[q])
    }
}

/// The filter states of a split, for two channels. Allocation-free.
#[derive(Debug, Clone, Copy)]
pub struct Split<const B: usize> {
    /// Crossover sections by channel and crossover slot: low-pass ×2, high-pass ×2.
    cross: [[[Tdf2; 4]; B]; 2],
    /// Allpass compensation by channel, band slot and the crossover slot above it.
    ap: [[[Tdf2; B]; B]; 2],
}

impl<const B: usize> Default for Split<B> {
    fn default() -> Self {
        Self { cross: [[[Tdf2::default(); 4]; B]; 2], ap: [[[Tdf2::default(); B]; B]; 2] }
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
    pub fn process(&mut self, c: usize, x: f64, layout: &Layout<B>, sections: &[Cross], out: &mut [f64; B]) {
        let mut rest = x;
        for p in 0..layout.n {
            let k = layout.crossovers[p];
            let st = &mut self.cross[c][k];
            let xc = &sections[k];
            let lo = st[0].process(&xc.lp, rest);
            out[p] = st[1].process(&xc.lp, lo);
            let hi = st[2].process(&xc.hp, rest);
            rest = st[3].process(&xc.hp, hi);
        }
        out[layout.n] = rest;
        for p in 0..layout.n {
            let owner = layout.owners[p];
            for q in p + 1..layout.n {
                let k = layout.crossovers[q];
                out[p] = self.ap[c][owner][k].process(&sections[k].ap, out[p]);
            }
        }
    }

    /// Carries every state across an L/R ↔ M/S switch (the filters are linear): to M/S the
    /// channels become (L + R)/2 and (L − R)/2, back they become M + S and M − S.
    pub fn switch_mid_side(&mut self, to_mid_side: bool) {
        let (ka, kb) = if to_mid_side { (0.5, 0.5) } else { (1.0, 1.0) };
        let mix = |a: &mut Tdf2, b: &mut Tdf2| {
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
    pub fn restructure(&mut self, old: &Layout<B>, new: &Layout<B>, crossovers: &[Option<f32>]) -> [Option<usize>; B] {
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
                self.cross[c][k] = [Tdf2::default(); 4];
                for b in 0..B {
                    self.ap[c][b][k] = Tdf2::default();
                }
                self.ap[c][k + 1] = [Tdf2::default(); B];
            }
        }
        parents
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn six_bands_sum_flat() {
        let x = [Some(120.0), Some(500.0), Some(2000.0), Some(6000.0), Some(12_000.0)];
        let layout = Layout::<6>::new(&x);
        assert_eq!(layout.bands(), &[0, 1, 2, 3, 4, 5]);
        let fs = 48_000.0;
        let sections: Vec<Cross> = x.iter().map(|f| Cross::new(f64::from(f.unwrap()), fs)).chain([Cross::new(1000.0, fs)]).collect();
        let mut split = Split::<6>::default();
        let mut y = vec![0.0f64; 16_384];
        let mut out = [0.0; 6];
        for (i, v) in y.iter_mut().enumerate() {
            split.process(0, if i == 0 { 1.0 } else { 0.0 }, &layout, &sections, &mut out);
            *v = out.iter().sum();
        }
        for f in [30.0, 120.0, 700.0, 2000.0, 9000.0, 16_000.0] {
            let w = 2.0 * PI * f / fs;
            let (re, im) = y.iter().enumerate().fold((0.0, 0.0), |(re, im), (i, &v)| (re + v * (w * i as f64).cos(), im - v * (w * i as f64).sin()));
            let db = 10.0 * (re * re + im * im).log10();
            assert!(db.abs() < 0.01, "{f} Hz: {db:.4} dB");
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
}
