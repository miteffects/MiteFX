//! Mite Gain DSP, kept free of the plugin framework so it can be tested directly.

use mite_dsp::{db_to_gain, LinearSmoother, PeakMeter, MINUS_INFINITY_DB};

use crate::params::GAIN;

pub struct GainDsp {
    gain: LinearSmoother,
    meters: [PeakMeter; 2],
}

impl GainDsp {
    pub fn new(sample_rate: f32, gain_db: f32) -> Self {
        let mut dsp = Self { gain: LinearSmoother::new(1.0), meters: [PeakMeter::new(sample_rate), PeakMeter::new(sample_rate)] };
        dsp.prepare(sample_rate, gain_db);
        dsp
    }

    /// Off the audio thread: sizes everything for `sample_rate`.
    pub fn prepare(&mut self, sample_rate: f32, gain_db: f32) {
        self.gain.set_ramp(sample_rate, GAIN.smoothing_ms);
        self.meters = [PeakMeter::new(sample_rate), PeakMeter::new(sample_rate)];
        self.reset(gain_db);
    }

    /// Clears state without allocating; safe on the audio thread.
    pub fn reset(&mut self, gain_db: f32) {
        self.gain.reset(db_to_gain(gain_db));
        for m in &mut self.meters {
            m.reset();
        }
    }

    /// Applies the (smoothed) gain in place. Returns the output peak levels (left, right) in dBFS;
    /// mono input reports the same level twice.
    pub fn process(&mut self, channels: &mut [&mut [f32]], gain_db: f32) -> (f32, f32) {
        self.gain.set_target(db_to_gain(gain_db));
        let frames = channels.first().map_or(0, |c| c.len());
        if self.gain.is_smoothing() {
            for i in 0..frames {
                let g = self.gain.next_value();
                for ch in channels.iter_mut() {
                    ch[i] *= g;
                }
            }
        } else {
            let g = self.gain.current();
            // Unity must be bit-exact (null test), so skip the multiply entirely.
            if g != 1.0 {
                for ch in channels.iter_mut() {
                    for s in ch.iter_mut() {
                        *s *= g;
                    }
                }
            }
        }
        let left = channels.first().map_or(MINUS_INFINITY_DB, |c| self.meters[0].process(c));
        let right = match channels.get(1) {
            Some(c) => self.meters[1].process(c),
            None => left,
        };
        (left, right)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f32 = 48_000.0;

    fn sine(len: usize) -> Vec<f32> {
        (0..len).map(|i| (i as f32 * 0.05).sin() * 0.5).collect()
    }

    #[test]
    fn zero_db_nulls_bit_exactly() {
        let mut dsp = GainDsp::new(SR, 0.0);
        let input = sine(1024);
        let mut l = input.clone();
        let mut r = input.clone();
        dsp.process(&mut [&mut l, &mut r], 0.0);
        assert_eq!(l, input);
        assert_eq!(r, input);
    }

    #[test]
    fn steady_gain_is_exact() {
        let mut dsp = GainDsp::new(SR, -6.0);
        let mut buf = vec![1.0_f32; 256];
        dsp.process(&mut [&mut buf], -6.0);
        assert!(buf.iter().all(|&s| s == db_to_gain(-6.0)));
    }

    #[test]
    fn full_range_jump_is_click_free() {
        // Ramp test (BLUEPRINT §10): min → max in one block must not step by more than one
        // smoothing increment per sample.
        let mut dsp = GainDsp::new(SR, GAIN.min);
        let ramp_len = (SR * GAIN.smoothing_ms / 1000.0) as usize;
        let mut buf = vec![1.0_f32; ramp_len + 64];
        dsp.process(&mut [&mut buf], GAIN.max);
        let max_step = (db_to_gain(GAIN.max) - db_to_gain(GAIN.min)) / ramp_len as f32 * 1.0001;
        for w in buf.windows(2) {
            assert!((w[1] - w[0]).abs() <= max_step, "step {}", w[1] - w[0]);
        }
        assert_eq!(*buf.last().unwrap(), db_to_gain(GAIN.max));
    }

    #[test]
    fn mono_reports_one_level_twice() {
        let mut dsp = GainDsp::new(SR, 0.0);
        let mut buf = vec![0.5_f32; 64];
        let (l, r) = dsp.process(&mut [&mut buf], 0.0);
        assert_eq!(l, r);
        assert!((l - (-6.0206)).abs() < 1e-3);
    }

    #[test]
    fn variable_block_sizes_match_one_big_block() {
        let input = sine(4096);
        let mut whole = input.clone();
        GainDsp::new(SR, 0.0).process(&mut [&mut whole], 12.0);

        let mut dsp = GainDsp::new(SR, 0.0);
        let mut split = input.clone();
        let mut start = 0;
        for size in [1, 7, 64, 128, 1000].iter().cycle() {
            if start >= split.len() {
                break;
            }
            let end = (start + size).min(split.len());
            dsp.process(&mut [&mut split[start..end]], 12.0);
            start = end;
        }
        assert_eq!(whole, split);
    }
}
