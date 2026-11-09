//! Mite Analyzer's engine. The audio passes through untouched; on the way it is measured:
//! loudness (BS.1770: momentary, short-term, integrated, range) and true peak (4× oversampled,
//! BS.1770 Annex 2) on the audio thread, so they are exact whether the editor is open or not; a
//! history record every 100 ms; and the raw left and right streams for the editor, which draws
//! the spectrum, the spectrogram and the stereo field from them.

use std::sync::atomic::{AtomicBool, Ordering};

use mite_core::{AtomicF32, HistoryTap, SpectrumTap};
use mite_dsp::loudness::{Loudness, TruePeak, SILENCE_LUFS};
use mite_dsp::{gain_to_db, MINUS_INFINITY_DB};

/// Fields of a history record.
pub mod record {
    /// Momentary and short-term loudness, LUFS.
    pub const MOMENTARY: usize = 0;
    pub const SHORT_TERM: usize = 1;
    /// The slice's true peak, dBTP.
    pub const TRUE_PEAK: usize = 2;
    /// The slice's correlation (−1…1; 0 for silence).
    pub const CORRELATION: usize = 3;
    pub const WIDTH: usize = 4;
}

pub const HISTORY_SLICE_MS: f32 = 100.0;
/// About 13 minutes of history.
pub const HISTORY_RECORDS: usize = 8192;
/// The raw streams the editor reads (the largest FFT and the scope come out of it).
pub const TAP_SAMPLES: usize = 32_768;
/// True peak: 4× (BS.1770-4 Annex 2).
const TP_PHASES: usize = 4;

/// Audio ↔ editor: the streams, the history and the running readings. Lock-free.
pub struct Telemetry {
    pub left: SpectrumTap,
    pub right: SpectrumTap,
    pub history: HistoryTap<{ record::WIDTH }>,
    pub momentary: AtomicF32,
    pub short_term: AtomicF32,
    pub integrated: AtomicF32,
    pub range: AtomicF32,
    /// The highest true peak since the last reset, dBTP.
    pub true_peak: AtomicF32,
    /// The editor asks for the integration (integrated, range, highest peak) to restart.
    pub reset: AtomicBool,
}

impl Telemetry {
    pub fn new() -> Self {
        Self {
            left: SpectrumTap::new(TAP_SAMPLES),
            right: SpectrumTap::new(TAP_SAMPLES),
            history: HistoryTap::new(HISTORY_RECORDS),
            momentary: AtomicF32::new(SILENCE_LUFS),
            short_term: AtomicF32::new(SILENCE_LUFS),
            integrated: AtomicF32::new(SILENCE_LUFS),
            range: AtomicF32::new(0.0),
            true_peak: AtomicF32::new(MINUS_INFINITY_DB),
            reset: AtomicBool::new(false),
        }
    }
}

impl Default for Telemetry {
    fn default() -> Self {
        Self::new()
    }
}

pub struct AnalyzerEngine {
    loudness: Loudness,
    tp: [TruePeak; 2],
    tp_max: f32,
    slice_len: usize,
    slice_pos: usize,
    slice_tp: f32,
    /// The slice's Σ l², Σ r², Σ l·r.
    sums: [f64; 3],
}

impl AnalyzerEngine {
    pub fn new(sample_rate: f32) -> Self {
        let mut e = Self { loudness: Loudness::new(f64::from(sample_rate)), tp: [TruePeak::new(TP_PHASES), TruePeak::new(TP_PHASES)], tp_max: 0.0, slice_len: 1, slice_pos: 0, slice_tp: 0.0, sums: [0.0; 3] };
        e.prepare(sample_rate);
        e
    }

    /// Sizes everything for `sample_rate`. Allocates.
    pub fn prepare(&mut self, sample_rate: f32) {
        self.loudness = Loudness::new(f64::from(sample_rate));
        self.slice_len = ((HISTORY_SLICE_MS * 0.001 * sample_rate).round() as usize).max(1);
        self.reset();
    }

    pub fn reset(&mut self) {
        self.loudness.reset();
        for t in &mut self.tp {
            t.reset();
        }
        self.tp_max = 0.0;
        self.slice_pos = 0;
        self.slice_tp = 0.0;
        self.sums = [0.0; 3];
    }

    /// Measures one block (one or two channels); the audio is left as it is.
    pub fn process(&mut self, channels: &[&mut [f32]], tel: &Telemetry) {
        let n_ch = channels.len().min(2);
        if n_ch == 0 {
            return;
        }
        if tel.reset.swap(false, Ordering::AcqRel) {
            self.loudness.reset_integration();
            self.tp_max = 0.0;
        }
        let left: &[f32] = channels[0];
        let right: &[f32] = channels[n_ch - 1];
        let len = left.len().min(right.len());
        tel.left.push(left[..len].iter().copied());
        tel.right.push(right[..len].iter().copied());
        let mut start = 0;
        while start < len {
            let n = (len - start).min(self.slice_len - self.slice_pos);
            let (l, r) = (&left[start..start + n], &right[start..start + n]);
            self.loudness.process(l, if n_ch == 2 { Some(r) } else { None });
            for (&a, &b) in l.iter().zip(r) {
                let p = self.tp[0].push(a).max(if n_ch == 2 { self.tp[1].push(b) } else { 0.0 });
                self.slice_tp = self.slice_tp.max(p);
                self.sums[0] += f64::from(a) * f64::from(a);
                self.sums[1] += f64::from(b) * f64::from(b);
                self.sums[2] += f64::from(a) * f64::from(b);
            }
            self.slice_pos += n;
            start += n;
            if self.slice_pos >= self.slice_len {
                self.slice_pos = 0;
                self.tp_max = self.tp_max.max(self.slice_tp);
                let [ll, rr, lr] = self.sums;
                let corr = if ll * rr > 1e-20 { (lr / (ll * rr).sqrt()) as f32 } else { 0.0 };
                tel.history.push(&[self.loudness.momentary(), self.loudness.short_term(), gain_to_db(self.slice_tp), corr]);
                self.slice_tp = 0.0;
                self.sums = [0.0; 3];
            }
        }
        tel.momentary.store(self.loudness.momentary());
        tel.short_term.store(self.loudness.short_term());
        tel.integrated.store(self.loudness.integrated());
        tel.range.store(self.loudness.range());
        tel.true_peak.store(gain_to_db(self.tp_max.max(self.slice_tp)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_minus_23_lufs_tone_reads_minus_23_and_its_true_peak() {
        // A 997 Hz sine of peak A on both channels reads 20·log10(A) LUFS (BS.1770's calibration:
        // 0 dBFS on one channel reads −3.01).
        let fs = 48_000.0;
        let tel = Telemetry::new();
        let mut e = AnalyzerEngine::new(fs);
        let amp = 10f32.powf(-23.0 / 20.0);
        for b in 0..(10 * 48_000 / 480) {
            let x: Vec<f32> = (0..480).map(|i| amp * (std::f32::consts::TAU * 997.0 * (b * 480 + i) as f32 / fs).sin()).collect();
            let (mut l, mut r) = (x.clone(), x.clone());
            e.process(&[&mut l, &mut r], &tel);
            assert_eq!(l, x, "the audio passes untouched");
        }
        let i = tel.integrated.load();
        assert!((i + 23.0).abs() < 0.3, "integrated {i}");
        let tp = tel.true_peak.load();
        assert!((tp - gain_to_db(amp)).abs() < 0.1, "true peak {tp} against {}", gain_to_db(amp));
        let mut seen = 0;
        let mut n = 0;
        tel.history.read_since(&mut seen, |r| {
            n += 1;
            assert!((r[record::CORRELATION] - 1.0).abs() < 1e-3 || n < 2);
        });
        assert!(n >= 99, "a record every 100 ms: {n}");
        // A reset restarts the integration.
        tel.reset.store(true, Ordering::Relaxed);
        let (mut l, mut r) = (vec![0.0f32; 480], vec![0.0f32; 480]);
        e.process(&[&mut l, &mut r], &tel);
        assert!(tel.integrated.load() <= SILENCE_LUFS + 1.0);
    }
}
