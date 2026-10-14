//! FFT spectrum analysis for displays. Runs on the UI thread; allocates only in `new`.

use std::sync::Arc;

use realfft::num_complex::Complex;
use realfft::{RealFftPlanner, RealToComplex};

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
        for ((x, s), w) in self.input.iter_mut().zip(samples).zip(&self.window) {
            *x = s * w;
        }
        // The buffers are sized by the planner, so this cannot fail.
        let _ = self.fft.process_with_scratch(&mut self.input, &mut self.spectrum, &mut self.scratch);
        for (o, c) in out_db.iter_mut().zip(&self.spectrum) {
            let mag = c.norm() * self.scale;
            *o = 20.0 * mag.max(1e-9).log10();
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
