//! The resonance engine: per-bin gain inside bands' ranges, on an [`Stft`]'s spectra. Shared by
//! the EQ's spectral bands and, later, Mite Soothe and Mite DeEss (BLUEPRINT §6).
//!
//! Two kinds of band use it:
//! - **Spectral compressor** (a dynamic band in compressor mode with spectral on): every bin in
//!   the band's range has its own level envelope and runs the band's compressor curve, so only
//!   the bins that get loud move.
//! - **Resonance band**: bins that stick out of a smoothed version of the spectrum (the spectral
//!   envelope) are cut back towards it, soothe-style. Sharpness sets the envelope's width (only
//!   resonances narrower than it stick out), selectivity how far one must stick out, depth the
//!   largest cut, speed the attack and release.
//!
//! Each band's gains are weighted by its region (the shape of its range) and combine per bin as
//! 2×2 matrices in the L/R basis, so stereo placements work as everywhere else. Allocation-free
//! after [`SpectralBands::new`].

use realfft::num_complex::Complex;

use crate::eq::{BandSettings, BandType, Dynamics, Placement, FREQ_MAX, FREQ_MIN};
use crate::filter::Analog;

/// Lowest level considered, dBFS (silence would otherwise be −∞).
const FLOOR_DB: f64 = -140.0;
/// Soft knee of the resonance cut, dB.
const RESONANCE_KNEE_DB: f64 = 3.0;

/// Per band: the per-bin level envelope (spectral compressor) and the per-bin gain now applied.
struct BandState {
    level: Vec<f64>,
    gain: Vec<f64>,
}

/// Per-bin spectral processing for up to `bands` bands, for an STFT of `size` at `sample_rate`.
pub struct SpectralBands {
    sample_rate: f64,
    /// Seconds between frames, and the magnitude of a full-scale sine (for dBFS levels).
    hop_s: f64,
    full_scale: f64,
    freqs: Vec<f64>,
    states: Vec<BandState>,
    /// Per bin: detected levels (dBFS) for each placement, scratch arrays, and the combined 2×2
    /// gain matrix (`[ll, lr, rl, rr]`).
    levels: [Vec<f64>; 5],
    prefix: Vec<f64>,
    target: Vec<f64>,
    matrix: Vec<[f64; 4]>,
    /// For displays: each band's strongest gain this frame (dB, signed), and the total gain per
    /// bin (dB, the mean of the two channels).
    band_peak: Vec<f64>,
    total_db: Vec<f32>,
}

impl SpectralBands {
    /// Allocates for `bands` bands, `size`-sample frames every `size / 4` samples.
    pub fn new(bands: usize, size: usize, sample_rate: f64, full_scale: f64) -> Self {
        let bins = size / 2 + 1;
        let zeros = || vec![0.0; bins];
        Self {
            sample_rate,
            hop_s: (size / 4) as f64 / sample_rate,
            full_scale,
            freqs: (0..bins).map(|k| k as f64 * sample_rate / size as f64).collect(),
            states: (0..bands).map(|_| BandState { level: vec![FLOOR_DB; bins], gain: zeros() }).collect(),
            levels: [zeros(), zeros(), zeros(), zeros(), zeros()],
            prefix: vec![0.0; bins + 1],
            target: zeros(),
            matrix: vec![[1.0, 0.0, 0.0, 1.0]; bins],
            band_peak: vec![0.0; bands],
            total_db: vec![0.0; bins],
        }
    }

    /// Clears envelopes and gains. Allocation-free.
    pub fn reset(&mut self) {
        for s in &mut self.states {
            s.level.fill(FLOOR_DB);
            s.gain.fill(0.0);
        }
        self.band_peak.fill(0.0);
        self.total_db.fill(0.0);
    }

    /// Each band's strongest gain in the last frame, dB (signed; 0 for bands not using it).
    pub fn band_peaks(&self) -> &[f64] {
        &self.band_peak
    }

    /// The total gain per bin in the last frame, dB, and each bin's frequency.
    pub fn total_db(&self) -> (&[f32], &[f64]) {
        (&self.total_db, &self.freqs)
    }

    /// Processes one frame's spectra in place for `bands` (the settings, indexed like the state).
    /// Allocation-free.
    pub fn process_frame(&mut self, left: &mut [Complex<f64>], right: &mut [Complex<f64>], bands: &[BandSettings]) {
        let to_db = |m: f64| if m > 0.0 { (20.0 * m.log10()).max(FLOOR_DB) } else { FLOOR_DB };
        let fs = self.full_scale;
        for (k, (l, r)) in left.iter().zip(right.iter()).enumerate() {
            let (ml, mr) = (l.norm() / fs, r.norm() / fs);
            self.levels[0][k] = to_db(ml.max(mr));
            self.levels[1][k] = to_db(ml);
            self.levels[2][k] = to_db(mr);
            self.levels[3][k] = to_db(((l + r) * 0.5).norm() / fs);
            self.levels[4][k] = to_db(((l - r) * 0.5).norm() / fs);
        }
        for m in &mut self.matrix {
            *m = [1.0, 0.0, 0.0, 1.0];
        }
        for (b, s) in bands.iter().enumerate().take(self.states.len()) {
            if !s.uses_spectrum() {
                self.band_peak[b] = 0.0;
                if self.states[b].gain.iter().any(|g| *g != 0.0) {
                    self.states[b].gain.fill(0.0);
                    self.states[b].level.fill(FLOOR_DB);
                }
                continue;
            }
            let levels = match s.placement {
                Placement::Stereo => 0,
                Placement::Left => 1,
                Placement::Right => 2,
                Placement::Mid => 3,
                Placement::Side => 4,
            };
            if s.kind == BandType::Resonance {
                self.resonance_targets(levels, s);
            } else {
                self.compressor_targets(b, levels, &s.dynamics);
            }
            // Weight by the band's region, then follow the targets with the band's timing.
            let (attack, release) = self.timing(s);
            let mut peak = 0.0f64;
            let state = &mut self.states[b];
            for (k, (g, &t)) in state.gain.iter_mut().zip(&self.target).enumerate() {
                let t = t * region(s, self.freqs[k]);
                let coeff = if t.abs() > g.abs() { attack } else { release };
                *g += coeff * (t - *g);
                if g.abs() > peak.abs() {
                    peak = *g;
                }
                let m = band_matrix(s.placement, 10f64.powf(*g / 20.0));
                let acc = self.matrix[k];
                self.matrix[k] = [
                    m[0] * acc[0] + m[1] * acc[2],
                    m[0] * acc[1] + m[1] * acc[3],
                    m[2] * acc[0] + m[3] * acc[2],
                    m[2] * acc[1] + m[3] * acc[3],
                ];
            }
            self.band_peak[b] = peak;
        }
        for (k, (l, r)) in left.iter_mut().zip(right.iter_mut()).enumerate() {
            let m = self.matrix[k];
            let (a, c) = (*l, *r);
            *l = a * m[0] + c * m[1];
            *r = a * m[2] + c * m[3];
            self.total_db[k] = (20.0 * (0.5 * (m[0] + m[3])).abs().max(1e-6).log10()) as f32;
        }
    }

    /// Spectral compressor: each bin's level envelope through the band's compressor curve.
    fn compressor_targets(&mut self, b: usize, levels: usize, d: &Dynamics) {
        let coeff = |ms: f64| 1.0 - (-self.hop_s * 1000.0 / ms.max(0.1)).exp();
        let (attack, release) = (coeff(d.attack_ms), coeff(d.release_ms));
        let state = &mut self.states[b];
        for ((env, &lvl), t) in state.level.iter_mut().zip(&self.levels[levels]).zip(self.target.iter_mut()) {
            let k = if lvl > *env { attack } else { release };
            *env += k * (lvl - *env);
            *t = d.gain_db(*env);
        }
    }

    /// Resonance band: how far each bin sticks out of the spectral envelope, cut back towards it.
    fn resonance_targets(&mut self, levels: usize, s: &BandSettings) {
        let r = &s.resonance;
        let level = &self.levels[levels];
        // Envelope: the level averaged in dB over a window of `width` octaves around each bin.
        let width = 2f64.powf(lerp(1.5f64.log2(), 0.25f64.log2(), r.sharpness.clamp(0.0, 1.0)));
        let threshold = lerp(2.0, 10.0, r.selectivity.clamp(0.0, 1.0));
        let depth = r.depth_db.max(0.0);
        self.prefix[0] = 0.0;
        for (k, &l) in level.iter().enumerate() {
            self.prefix[k + 1] = self.prefix[k] + l;
        }
        let bins = level.len();
        let half = 2f64.powf(0.5 * width);
        for (k, t) in self.target.iter_mut().enumerate() {
            let lo = ((k as f64 / half).floor() as usize).min(k);
            let hi = ((k as f64 * half).ceil() as usize).clamp(k, bins - 1);
            let envelope = (self.prefix[hi + 1] - self.prefix[lo]) / (hi + 1 - lo) as f64;
            let over = level[k] - envelope - threshold;
            let half_knee = 0.5 * RESONANCE_KNEE_DB;
            let cut = if over <= -half_knee {
                0.0
            } else if over < half_knee {
                (over + half_knee).powi(2) / (2.0 * RESONANCE_KNEE_DB)
            } else {
                over
            };
            *t = -cut.min(depth);
        }
        // A light blur across neighbouring bins keeps the cuts from fluttering bin to bin.
        let mut prev = self.target[0];
        for k in 1..bins - 1 {
            let here = self.target[k];
            self.target[k] = 0.25 * prev + 0.5 * here + 0.25 * self.target[k + 1];
            prev = here;
        }
    }

    /// Per-frame attack and release coefficients for a band.
    fn timing(&self, s: &BandSettings) -> (f64, f64) {
        let coeff = |ms: f64| 1.0 - (-self.hop_s * 1000.0 / ms.max(0.1)).exp();
        if s.kind == BandType::Resonance {
            let speed = s.resonance.speed.clamp(0.0, 1.0);
            let ms = |slow: f64, fast: f64| 10f64.powf(lerp(slow.log10(), fast.log10(), speed));
            (coeff(ms(40.0, 2.0)), coeff(ms(400.0, 30.0)))
        } else {
            // The compressor's envelope already has the band's timing.
            (1.0, 1.0)
        }
    }

    pub fn sample_rate(&self) -> f64 {
        self.sample_rate
    }
}

fn lerp(a: f64, b: f64, t: f64) -> f64 {
    a + (b - a) * t
}

/// One band's real gain matrix for a linear gain `g`.
fn band_matrix(placement: Placement, g: f64) -> [f64; 4] {
    let (sum, diff) = (0.5 * (g + 1.0), 0.5 * (g - 1.0));
    match placement {
        Placement::Stereo => [g, 0.0, 0.0, g],
        Placement::Left => [g, 0.0, 0.0, 1.0],
        Placement::Right => [1.0, 0.0, 0.0, g],
        Placement::Mid => [sum, diff, diff, sum],
        Placement::Side => [sum, -diff, -diff, sum],
    }
}

/// How much of a band's per-bin gain applies at `f` (0…1): the shape of its range. Bells,
/// notches, band passes and resonance bands use a band-pass around their frequency at their Q;
/// shelves a low- or high-pass at their frequency; a tilt the whole spectrum.
pub fn region(s: &BandSettings, f: f64) -> f64 {
    let x = f / s.freq.clamp(FREQ_MIN, FREQ_MAX);
    let q = s.q.clamp(0.025, 40.0);
    let proto = match s.kind {
        BandType::Bell | BandType::Notch | BandType::BandPass | BandType::Resonance => Analog::second(0.0, 1.0 / q, 0.0, 1.0, 1.0 / q, 1.0),
        BandType::LowShelf | BandType::LowCut => Analog::second(0.0, 0.0, 1.0, 1.0, std::f64::consts::SQRT_2, 1.0),
        BandType::HighShelf | BandType::HighCut => Analog::second(1.0, 0.0, 0.0, 1.0, std::f64::consts::SQRT_2, 1.0),
        BandType::TiltShelf => return 1.0,
    };
    proto.magnitude_sq(x).sqrt().min(1.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::eq::DynamicsMode;
    use crate::stft::Stft;

    const FS: f64 = 48_000.0;
    const SIZE: usize = 1024;

    fn noise(n: usize, seed: u32, amp: f64) -> Vec<f64> {
        let mut s = seed;
        (0..n)
            .map(|_| {
                s ^= s << 13;
                s ^= s >> 17;
                s ^= s << 5;
                (s as f64 / u32::MAX as f64 - 0.5) * 2.0 * amp
            })
            .collect()
    }

    fn tone(n: usize, freq: f64, amp: f64) -> Vec<f64> {
        (0..n).map(|i| amp * (2.0 * std::f64::consts::PI * freq * i as f64 / FS).sin()).collect()
    }

    fn mix(parts: &[&[f64]]) -> Vec<f32> {
        (0..parts[0].len()).map(|i| parts.iter().map(|p| p[i]).sum::<f64>() as f32).collect()
    }

    /// Runs stereo input through an STFT with `bands`.
    fn run(l: &[f32], r: &[f32], bands: &[BandSettings]) -> (Vec<f32>, Vec<f32>) {
        let mut stft = Stft::new(SIZE);
        let mut engine = SpectralBands::new(bands.len(), SIZE, FS, stft.full_scale());
        let (mut l, mut r) = (l.to_vec(), r.to_vec());
        for (a, b) in l.chunks_mut(256).zip(r.chunks_mut(256)) {
            stft.process(a, b, &mut |x, y| engine.process_frame(x, y, bands));
        }
        (l, r)
    }

    /// Amplitude of the `freq` component in `x` (the second half, after things settle).
    fn amplitude(x: &[f32], freq: f64) -> f64 {
        let x = &x[x.len() / 2..];
        let (mut re, mut im) = (0.0, 0.0);
        for (i, &v) in x.iter().enumerate() {
            let p = 2.0 * std::f64::consts::PI * freq * i as f64 / FS;
            re += f64::from(v) * p.cos();
            im += f64::from(v) * p.sin();
        }
        2.0 * (re * re + im * im).sqrt() / x.len() as f64
    }

    fn rms(x: &[f32]) -> f64 {
        let x = &x[x.len() / 2..];
        (x.iter().map(|v| f64::from(*v).powi(2)).sum::<f64>() / x.len() as f64).sqrt()
    }

    fn resonance_band(freq: f64, q: f64) -> BandSettings {
        BandSettings { enabled: true, kind: BandType::Resonance, freq, q, ..Default::default() }
    }

    #[test]
    fn a_resonance_band_cuts_a_tone_that_sticks_out_and_leaves_the_noise() {
        let n = 96_000;
        let (hiss, ring) = (noise(n, 1, 0.05), tone(n, 2_000.0, 0.3));
        let input = mix(&[&hiss, &ring]);
        let mut band = resonance_band(2_000.0, 0.3);
        band.resonance.depth_db = 12.0;
        let (out, _) = run(&input, &input, &[band]);
        let cut = 20.0 * (amplitude(&out, 2_000.0) / amplitude(&input, 2_000.0)).log10();
        assert!(cut < -6.0, "the ringing tone only moved {cut:.2} dB");
        // The noise around it barely changes: measure without the tone.
        let (quiet, _) = run(&mix(&[&hiss]), &mix(&[&hiss]), &[band]);
        let noise_change = 20.0 * (rms(&quiet) / rms(&mix(&[&hiss]))).log10();
        assert!(noise_change.abs() < 1.0, "plain noise changed by {noise_change:.2} dB");
    }

    #[test]
    fn the_spectral_compressor_moves_only_the_loud_bins() {
        let n = 96_000;
        let (loud, soft) = (tone(n, 1_500.0, 0.5), tone(n, 3_000.0, 0.003));
        let input = mix(&[&loud, &soft]);
        let mut band = BandSettings { enabled: true, kind: BandType::Bell, freq: 2_000.0, q: 0.3, ..Default::default() };
        band.dynamics = Dynamics { enabled: true, mode: DynamicsMode::Compressor, spectral: true, threshold_db: -30.0, ratio: 4.0, range_db: -12.0, ..Dynamics::default() };
        let (out, _) = run(&input, &input, &[band]);
        let loud_cut = 20.0 * (amplitude(&out, 1_500.0) / amplitude(&input, 1_500.0)).log10();
        let soft_cut = 20.0 * (amplitude(&out, 3_000.0) / amplitude(&input, 3_000.0)).log10();
        assert!(loud_cut < -6.0, "loud tone moved {loud_cut:.2} dB");
        assert!(soft_cut.abs() < 0.5, "quiet tone moved {soft_cut:.2} dB");
    }

    #[test]
    fn a_mid_resonance_band_leaves_the_side_alone() {
        let n = 96_000;
        let (hiss, ring) = (noise(n, 2, 0.05), tone(n, 2_000.0, 0.3));
        // The tone is in the mid; an opposite-polarity tone at another frequency is the side.
        let side = tone(n, 5_000.0, 0.3);
        let l = mix(&[&hiss, &ring, &side]);
        let r: Vec<f32> = (0..n).map(|i| (hiss[i] + ring[i] - side[i]) as f32).collect();
        let mut band = resonance_band(3_000.0, 0.2);
        band.placement = Placement::Mid;
        band.resonance.depth_db = 12.0;
        let (ol, or) = run(&l, &r, &[band]);
        let s_in: Vec<f32> = l.iter().zip(&r).map(|(a, b)| 0.5 * (a - b)).collect();
        let s_out: Vec<f32> = ol.iter().zip(&or).map(|(a, b)| 0.5 * (a - b)).collect();
        let side_change = 20.0 * (amplitude(&s_out, 5_000.0) / amplitude(&s_in, 5_000.0)).log10();
        assert!(side_change.abs() < 0.1, "side moved {side_change:.3} dB");
        let m_out: Vec<f32> = ol.iter().zip(&or).map(|(a, b)| 0.5 * (a + b)).collect();
        let m_in: Vec<f32> = l.iter().zip(&r).map(|(a, b)| 0.5 * (a + b)).collect();
        assert!(20.0 * (amplitude(&m_out, 2_000.0) / amplitude(&m_in, 2_000.0)).log10() < -3.0);
    }

    #[test]
    fn bands_that_do_not_use_the_spectrum_change_nothing() {
        let n = 20_000;
        let input = mix(&[&noise(n, 3, 0.2)]);
        let plain = BandSettings { enabled: true, kind: BandType::Bell, freq: 1_000.0, gain_db: 6.0, ..Default::default() };
        let (out, _) = run(&input, &input, &[plain]);
        let worst = (2 * SIZE..n).map(|i| (out[i] - input[i - SIZE]).abs()).fold(0.0f32, f32::max);
        assert!(worst < 1e-6, "{worst}");
    }
}
