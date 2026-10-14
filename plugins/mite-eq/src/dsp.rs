//! The Mite EQ engine: 24 bands in series, output gain, meters and analyzer taps. Free of the
//! plugin framework so it can be tested directly.

use mite_core::SpectrumTap;
use mite_dsp::eq::{BandProcessor, BandSettings};
use mite_dsp::{db_to_gain, LinearSmoother, PeakMeter};

use crate::params::{BANDS, FREQ, OUT_GAIN};

pub struct EqEngine {
    bands: Vec<BandProcessor>,
    out_gain: LinearSmoother,
    meters: [PeakMeter; 2],
}

impl EqEngine {
    /// Allocates; call off the audio thread.
    pub fn new(sample_rate: f32, settings: &[BandSettings; BANDS], out_db: f32) -> Self {
        let mut engine = Self {
            bands: (0..BANDS).map(|_| BandProcessor::new(f64::from(sample_rate), FREQ.smoothing_ms)).collect(),
            out_gain: LinearSmoother::new(1.0),
            meters: [PeakMeter::new(sample_rate), PeakMeter::new(sample_rate)],
        };
        engine.prepare(sample_rate, settings, out_db);
        engine
    }

    /// Off the audio thread: sets the sample rate and jumps to the given settings.
    pub fn prepare(&mut self, sample_rate: f32, settings: &[BandSettings; BANDS], out_db: f32) {
        for (band, s) in self.bands.iter_mut().zip(settings) {
            band.prepare(f64::from(sample_rate), FREQ.smoothing_ms, s);
        }
        self.out_gain.set_ramp(sample_rate, OUT_GAIN.smoothing_ms);
        self.meters = [PeakMeter::new(sample_rate), PeakMeter::new(sample_rate)];
        self.reset(settings, out_db);
    }

    /// Clears all state. Allocation-free.
    pub fn reset(&mut self, settings: &[BandSettings; BANDS], out_db: f32) {
        for (band, s) in self.bands.iter_mut().zip(settings) {
            band.reset(s);
        }
        self.out_gain.reset(db_to_gain(out_db));
        for m in &mut self.meters {
            m.reset();
        }
    }

    /// Processes in place and returns output peak levels (left, right) in dBFS. `pre`/`post`
    /// receive the mono sum before and after the EQ for the analyzer.
    pub fn process(
        &mut self,
        channels: &mut [&mut [f32]],
        settings: &[BandSettings; BANDS],
        out_db: f32,
        pre: Option<&SpectrumTap>,
        post: Option<&SpectrumTap>,
    ) -> (f32, f32) {
        if let Some(tap) = pre {
            push_mono(tap, channels);
        }
        for (band, s) in self.bands.iter_mut().zip(settings) {
            band.set_target(s);
            band.process(channels);
        }

        self.out_gain.set_target(db_to_gain(out_db));
        let frames = channels.first().map_or(0, |c| c.len());
        if self.out_gain.is_smoothing() {
            for i in 0..frames {
                let g = self.out_gain.next_value();
                for ch in channels.iter_mut() {
                    ch[i] *= g;
                }
            }
        } else if self.out_gain.current() != 1.0 {
            let g = self.out_gain.current();
            for ch in channels.iter_mut() {
                for s in ch.iter_mut() {
                    *s *= g;
                }
            }
        }

        if let Some(tap) = post {
            push_mono(tap, channels);
        }
        let left = channels.first().map_or(mite_dsp::MINUS_INFINITY_DB, |c| self.meters[0].process(c));
        let right = channels.get(1).map_or(left, |c| self.meters[1].process(c));
        (left, right)
    }

    /// What each band is producing right now (smoothed values), for the display.
    pub fn band_state(&self, band: usize) -> BandSettings {
        self.bands[band].current()
    }
}

fn push_mono(tap: &SpectrumTap, channels: &[&mut [f32]]) {
    match channels {
        [l, r, ..] => tap.push(l.iter().zip(r.iter()).map(|(a, b)| 0.5 * (a + b))),
        [m] => tap.push(m.iter().copied()),
        [] => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mite_dsp::eq::BandType;
    use std::f64::consts::PI;

    const SR: f32 = 48_000.0;

    fn sine(freq: f64, len: usize) -> Vec<f32> {
        (0..len).map(|n| (2.0 * PI * freq * n as f64 / f64::from(SR)).sin() as f32 * 0.5).collect()
    }

    fn flat() -> [BandSettings; BANDS] {
        [BandSettings::default(); BANDS]
    }

    #[test]
    fn empty_eq_passes_audio_bit_exactly() {
        // The design's empty state promises "passing audio unchanged".
        let mut eq = EqEngine::new(SR, &flat(), 0.0);
        let input = sine(997.0, 4096);
        let (mut l, mut r) = (input.clone(), input.clone());
        eq.process(&mut [&mut l, &mut r], &flat(), 0.0, None, None);
        assert_eq!(l, input);
        assert_eq!(r, input);
    }

    #[test]
    fn bands_stack_in_series() {
        let mut settings = flat();
        for (i, s) in settings.iter_mut().take(2).enumerate() {
            *s = BandSettings { enabled: true, kind: BandType::Bell, freq: 1000.0, gain_db: 3.0, q: 1.0, slope: 3 };
            let _ = i;
        }
        let mut eq = EqEngine::new(SR, &settings, 0.0);
        let input = sine(1000.0, 48_000);
        let mut buf = input.clone();
        eq.process(&mut [&mut buf], &settings, 0.0, None, None);
        let rms = |x: &[f32]| (x.iter().map(|v| f64::from(*v).powi(2)).sum::<f64>() / x.len() as f64).sqrt();
        let gain = 20.0 * (rms(&buf[24_000..]) / rms(&input[24_000..])).log10();
        assert!((gain - 6.0).abs() < 0.05, "{gain}");
    }

    #[test]
    fn taps_receive_mono_sum() {
        let tap = SpectrumTap::new(16);
        let mut eq = EqEngine::new(SR, &flat(), 0.0);
        let (mut l, mut r) = (vec![1.0_f32; 4], vec![0.0_f32; 4]);
        eq.process(&mut [&mut l, &mut r], &flat(), 0.0, Some(&tap), None);
        let mut out = [0.0; 4];
        tap.read_latest(&mut out);
        assert_eq!(out, [0.5; 4]);
    }
}
