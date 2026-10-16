//! FFT spectrum analysis: the display's analyzer, and the spectral compressor's detector, which
//! reads the spectrum exactly as the display shows it ([`LogSpectrum`]). Allocates only in `new`.

use std::sync::Arc;

use realfft::num_complex::Complex;
use realfft::{RealFftPlanner, RealToComplex};

use crate::eq::{FREQ_MAX, FREQ_MIN};

/// The analyzer's FFT size: the design's "8192 · High" resolution.
pub const ANALYZER_SIZE: usize = 8192;
/// The log-spaced points the spectrum is read at, `FREQ_MIN`…`FREQ_MAX` (one every 4 design px
/// of the EQ's plot).
pub const SPECTRUM_POINTS: usize = 301;
/// Smoothing across frequency: a raised-cosine window this many octaves wide, so bins fade in
/// and out of a point instead of stepping.
pub const SPECTRUM_SMOOTH_OCT: f64 = 1.0 / 6.0;

/// The frequency of spectrum point `i`, Hz.
pub fn spectrum_point_freq(i: usize) -> f64 {
    FREQ_MIN * (FREQ_MAX / FREQ_MIN).powf(i as f64 / (SPECTRUM_POINTS - 1) as f64)
}

/// The spacing of the spectrum points, octaves.
pub fn spectrum_point_oct() -> f64 {
    (FREQ_MAX / FREQ_MIN).log2() / (SPECTRUM_POINTS - 1) as f64
}

/// The spectrum's tilt at `f`: 4.5 dB/oct around 1 kHz, so pink noise reads flat.
pub fn tilt_db(f: f64) -> f64 {
    4.5 * (f / 1000.0).log2()
}

/// Hann-windowed magnitude spectrum in dBFS, calibrated so a full-scale sine reads 0 dB.
pub struct Analyzer {
    fft: Arc<dyn RealToComplex<f32>>,
    window: Vec<f32>,
    input: Vec<f32>,
    spectrum: Vec<Complex<f32>>,
    scratch: Vec<Complex<f32>>,
    /// 2 / Σ window: undoes the window's coherent gain and the one-sided spectrum split.
    scale: f32,
}

impl Analyzer {
    /// `size` must be a power of two (e.g. 8192, the design's "High" resolution).
    pub fn new(size: usize) -> Self {
        assert!(size.is_power_of_two(), "FFT size must be a power of two");
        let fft = RealFftPlanner::<f32>::new().plan_fft_forward(size);
        let window: Vec<f32> = (0..size)
            .map(|n| 0.5 - 0.5 * (2.0 * std::f32::consts::PI * n as f32 / size as f32).cos())
            .collect();
        let scale = 2.0 / window.iter().sum::<f32>();
        let spectrum = fft.make_output_vec();
        let scratch = fft.make_scratch_vec();
        Self { fft, window, input: vec![0.0; size], spectrum, scratch, scale }
    }

    pub fn size(&self) -> usize {
        self.window.len()
    }

    /// Number of output bins: `size / 2 + 1`.
    pub fn bins(&self) -> usize {
        self.spectrum.len()
    }

    /// Analyses the newest `size()` samples into `out_db` (`bins()` long, bin k at k·fs/size).
    pub fn analyze(&mut self, samples: &[f32], out_db: &mut [f32]) {
        self.analyze_power(samples, out_db);
        for o in out_db.iter_mut() {
            *o = 10.0 * o.max(1e-18).log10();
        }
    }

    /// Like [`Self::analyze`] but returns linear power (1.0 = full-scale sine), which is what
    /// displays average over; it skips a log per bin.
    pub fn analyze_power(&mut self, samples: &[f32], out_power: &mut [f32]) {
        for ((x, s), w) in self.input.iter_mut().zip(samples).zip(&self.window) {
            *x = s * w;
        }
        // The buffers are sized by the planner, so this cannot fail.
        let _ = self.fft.process_with_scratch(&mut self.input, &mut self.spectrum, &mut self.scratch);
        let scale_sq = self.scale * self.scale;
        for (o, c) in out_power.iter_mut().zip(&self.spectrum) {
            *o = c.norm_sqr() * scale_sq;
        }
    }
}

/// How one point reads the FFT: a weighted sum of the bins in `weights[start..end]`, then its
/// tilt gain. `cubic` points (where bins are sparser than the window) sum the bins' levels in
/// dB with Catmull-Rom weights, so the curve passes smoothly through the bins; the others
/// average power.
struct Point {
    start: usize,
    end: usize,
    tilt: f32,
    cubic: bool,
}

/// Power spectra (from an [`Analyzer`]) read at the [`SPECTRUM_POINTS`]: smoothed across
/// frequency, tilted and rounded, in dB. One map per FFT size and sample rate; `read` is
/// allocation-free.
pub struct LogSpectrum {
    points: Vec<Point>,
    weights: Vec<(usize, f32)>,
    sample_rate: f64,
}

impl LogSpectrum {
    /// For `size`-point FFTs at `sample_rate`.
    pub fn new(size: usize, sample_rate: f64) -> Self {
        let bins = size / 2 + 1;
        let last = bins - 1;
        let size = size as f64;
        let half = 0.5 * SPECTRUM_SMOOTH_OCT;
        let mut weights = Vec::new();
        let points = (0..SPECTRUM_POINTS)
            .map(|i| {
                let f = spectrum_point_freq(i);
                let tilt = 10f64.powf(tilt_db(f) / 10.0) as f32;
                let start = weights.len();
                let (lo, hi) = (f * 2f64.powf(-half) * size / sample_rate, f * 2f64.powf(half) * size / sample_rate);
                let (a, b) = ((lo.ceil() as usize).max(1), (hi.floor() as usize).min(last));
                if b >= a + 2 {
                    let mut sum = 0.0;
                    for k in a..=b {
                        let d = (k as f64 * sample_rate / size / f).log2() / half;
                        let w = 0.5 * (1.0 + (std::f64::consts::PI * d).cos());
                        weights.push((k, w as f32));
                        sum += w;
                    }
                    for w in &mut weights[start..] {
                        w.1 /= sum as f32;
                    }
                    Point { start, end: weights.len(), tilt, cubic: false }
                } else {
                    // Fewer bins than the window needs (low frequencies): a cubic through the
                    // four bins around the point. Linear interpolation in power bows outwards in
                    // dB and meets the next segment in a point at every bin.
                    let k = (f * size / sample_rate).clamp(1.0, last as f64 - 2.0);
                    let k0 = k.floor() as usize;
                    let t = (k - k0 as f64) as f32;
                    let (t2, t3) = (t * t, t * t * t);
                    weights.push((k0 - 1, 0.5 * (-t3 + 2.0 * t2 - t)));
                    weights.push((k0, 0.5 * (3.0 * t3 - 5.0 * t2 + 2.0)));
                    weights.push((k0 + 1, 0.5 * (-3.0 * t3 + 4.0 * t2 + t)));
                    weights.push((k0 + 2, 0.5 * (t3 - t2)));
                    Point { start, end: weights.len(), tilt, cubic: true }
                }
            })
            .collect();
        Self { points, weights, sample_rate }
    }

    pub fn sample_rate(&self) -> f64 {
        self.sample_rate
    }

    /// Reads `power` (an [`Analyzer::analyze_power`] spectrum) into `out_db`
    /// ([`SPECTRUM_POINTS`] long): smoothed, tilted, then rounded by a light [1 2 1] pass in dB,
    /// twice (about 0.04 octave, far narrower than the smoothing), so dips stay round.
    pub fn read(&self, power: &[f32], out_db: &mut [f32]) {
        let db = |p: f32| 10.0 * p.max(1e-15).log10();
        for (t, m) in out_db.iter_mut().zip(&self.points) {
            let bins = &self.weights[m.start..m.end];
            *t = if m.cubic {
                bins.iter().map(|&(k, w)| db(power[k]) * w).sum::<f32>() + db(m.tilt)
            } else {
                db(bins.iter().map(|&(k, w)| power[k] * w).sum::<f32>() * m.tilt)
            };
        }
        for _ in 0..2 {
            let mut prev = out_db[0];
            for i in 1..out_db.len() - 1 {
                let here = out_db[i];
                out_db[i] = 0.25 * prev + 0.5 * here + 0.25 * out_db[i + 1];
                prev = here;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_scale_sine_reads_zero_dbfs() {
        let n = 8192;
        let fs = 48_000.0_f32;
        // Exactly on bin 256 so there is no scalloping.
        let freq = 256.0 * fs / n as f32;
        let samples: Vec<f32> = (0..n).map(|i| (2.0 * std::f32::consts::PI * freq * i as f32 / fs).sin()).collect();
        let mut a = Analyzer::new(n);
        let mut out = vec![0.0; a.bins()];
        a.analyze(&samples, &mut out);
        assert!(out[256].abs() < 0.05, "{}", out[256]);
        assert!(out[1000] < -100.0, "leakage {}", out[1000]);
    }
}
