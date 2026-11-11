//! Mite Tuner's audio path: the input passes untouched (or muted, faded over 10 ms); the mono sum
//! goes to the editor's pitch detector through a tap, and the input's peaks to the meter, one
//! record per 5 ms slice. No latency.

use mite_core::{HistoryTap, SpectrumTap};
use mite_dsp::{gain_to_db, LinearSmoother, MINUS_INFINITY_DB};

/// Fields of a history record: the input's peak, L and R, dBFS.
pub mod record {
    pub const IN: usize = 0;
    pub const WIDTH: usize = 2;
}

pub const HISTORY_SLICE_MS: f32 = 5.0;
pub const HISTORY_RECORDS: usize = 1200;
const MUTE_MS: f32 = 10.0;

pub struct TunerEngine {
    mute: LinearSmoother,
    slice_len: usize,
    slice_pos: usize,
    acc: [f32; record::WIDTH],
}

impl TunerEngine {
    pub fn new(sample_rate: f32, mute: bool) -> Self {
        let mut e = Self { mute: LinearSmoother::new(0.0), slice_len: 1, slice_pos: 0, acc: [MINUS_INFINITY_DB; record::WIDTH] };
        e.prepare(sample_rate, mute);
        e
    }

    pub fn prepare(&mut self, sample_rate: f32, mute: bool) {
        self.slice_len = ((HISTORY_SLICE_MS * 0.001 * sample_rate).round() as usize).max(1);
        self.mute.set_ramp(sample_rate, MUTE_MS);
        self.reset(mute);
    }

    pub fn reset(&mut self, mute: bool) {
        self.mute.reset(if mute { 1.0 } else { 0.0 });
        self.slice_pos = 0;
        self.acc = [MINUS_INFINITY_DB; record::WIDTH];
    }

    pub fn process(&mut self, channels: &mut [&mut [f32]], mute: bool, history: &HistoryTap<{ record::WIDTH }>, tap: Option<&SpectrumTap>) {
        let n_ch = channels.len().min(2);
        if n_ch == 0 {
            return;
        }
        let len = channels.iter().map(|c| c.len()).min().unwrap_or(0);
        if let Some(tap) = tap {
            let (l, r) = (&channels[0][..len], &channels[n_ch - 1][..len]);
            tap.push(l.iter().zip(r).map(|(a, b)| 0.5 * (a + b)));
        }
        self.mute.set_target(if mute { 1.0 } else { 0.0 });
        for i in 0..len {
            for (acc, ch) in self.acc.iter_mut().zip(channels.iter()) {
                *acc = acc.max(gain_to_db(ch[i].abs().max(1e-9)));
            }
            if n_ch == 1 {
                self.acc[1] = self.acc[0];
            }
            let m = self.mute.next_value();
            if m > 0.0 {
                for ch in channels.iter_mut().take(n_ch) {
                    ch[i] *= 1.0 - m;
                }
            }
            self.slice_pos += 1;
            if self.slice_pos >= self.slice_len {
                history.push(&self.acc);
                self.acc = [MINUS_INFINITY_DB; record::WIDTH];
                self.slice_pos = 0;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_input_passes_untouched_and_mute_silences_it() {
        let x: Vec<f32> = (0..4800).map(|i| (i as f32 * 0.05).sin() * 0.5).collect();
        let history = HistoryTap::new(64);
        let mut e = TunerEngine::new(48_000.0, false);
        let (mut l, mut r) = (x.clone(), x.clone());
        for (a, b) in l.chunks_mut(333).zip(r.chunks_mut(333)) {
            e.process(&mut [a, b], false, &history, None);
        }
        assert_eq!(l, x);
        assert_eq!(r, x);
        let (mut l, mut r) = (x.clone(), x.clone());
        e.process(&mut [&mut l, &mut r], true, &history, None);
        // Faded over 10 ms, then silent; no step at the start.
        assert!((l[0] - x[0]).abs() < 1e-3);
        assert!(l[480..].iter().all(|v| *v == 0.0));
        assert!(history.count() > 0);
    }
}
