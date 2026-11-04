//! Mite DeEss's engine: the de-esser (`mite_dsp::deess`, which also does listen, bypass and the
//! lookahead) and the output gain, plus the telemetry: one record per 5 ms slice, whatever the
//! host's block size, and the input and output for the spectrum.

use mite_core::{HistoryTap, SpectrumTap};
use mite_dsp::deess::{DeEssSettings, DeEsser};
use mite_dsp::{db_to_gain, gain_to_db, LinearSmoother, MINUS_INFINITY_DB};

/// Fields of a history record (`DeEssShared::history`).
pub mod record {
    /// Peak in and out, L and R, dBFS.
    pub const IN: usize = 0;
    pub const OUT: usize = 2;
    /// The trigger band's and the whole input's RMS (2 ms), dBFS, the loudest in the slice.
    pub const BAND: usize = 4;
    pub const WHOLE: usize = 5;
    /// The deepest reduction, dB (≥ 0).
    pub const GR: usize = 6;
    pub const WIDTH: usize = 7;
}

pub const HISTORY_SLICE_MS: f32 = 5.0;
pub const HISTORY_RECORDS: usize = 1600;
const CHUNK: usize = 256;

pub struct DeEssEngine {
    deesser: DeEsser,
    gain: LinearSmoother,
    slice_len: usize,
    slice_pos: usize,
    acc: [f32; record::WIDTH],
    copy: [Vec<f32>; 2],
}

impl DeEssEngine {
    pub fn new(sample_rate: f32, s: &DeEssSettings, gain_db: f32) -> Self {
        let mut e = Self { deesser: DeEsser::new(sample_rate, s), gain: LinearSmoother::new(1.0), slice_len: 1, slice_pos: 0, acc: empty_record(), copy: [vec![0.0; CHUNK], vec![0.0; CHUNK]] };
        e.prepare(sample_rate, s, gain_db);
        e
    }

    /// Sizes everything for `sample_rate`. Allocates.
    pub fn prepare(&mut self, sample_rate: f32, s: &DeEssSettings, gain_db: f32) {
        if self.deesser.sample_rate() != sample_rate {
            self.deesser = DeEsser::new(sample_rate, s);
        }
        self.slice_len = ((HISTORY_SLICE_MS * 0.001 * sample_rate).round() as usize).max(1);
        self.gain.set_ramp(sample_rate, 20.0);
        self.reset(s, gain_db);
    }

    pub fn reset(&mut self, s: &DeEssSettings, gain_db: f32) {
        self.deesser.reset(s);
        self.gain.reset(db_to_gain(gain_db));
        self.slice_pos = 0;
        self.acc = empty_record();
    }

    /// The latency now, samples (the lookahead).
    pub fn latency(&self) -> usize {
        self.deesser.latency()
    }

    pub fn process(&mut self, channels: &mut [&mut [f32]], s: &DeEssSettings, gain_db: f32, history: &HistoryTap<{ record::WIDTH }>, spectra: Option<[&SpectrumTap; 2]>) {
        let n_ch = channels.len().min(2);
        if n_ch == 0 {
            return;
        }
        let len = channels.iter().map(|c| c.len()).min().unwrap_or(0);
        self.gain.set_target(db_to_gain(gain_db));
        let mut start = 0;
        while start < len {
            let n = (len - start).min(CHUNK).min(self.slice_len - self.slice_pos);
            let end = start + n;
            let mut peaks = [0.0f32; 4];
            for c in 0..n_ch {
                self.copy[c][..n].copy_from_slice(&channels[c][start..end]);
                peaks[c] = channels[c][start..end].iter().fold(0.0f32, |m, v| m.max(v.abs()));
            }
            if let Some([pre, _]) = spectra {
                let (l, r) = (&self.copy[0][..n], &self.copy[n_ch - 1][..n]);
                pre.push(l.iter().zip(r).map(|(a, b)| 0.5 * (a + b)));
            }
            let tel = if n_ch == 2 {
                let (a, b) = channels.split_at_mut(1);
                self.deesser.process(&mut [&mut a[0][start..end], &mut b[0][start..end]], s)
            } else {
                self.deesser.process(&mut [&mut channels[0][start..end]], s)
            };
            for i in start..end {
                let g = self.gain.next_value();
                for (c, ch) in channels.iter_mut().enumerate().take(n_ch) {
                    ch[i] *= g;
                    peaks[2 + c] = peaks[2 + c].max(ch[i].abs());
                }
            }
            if let Some([_, post]) = spectra {
                let (l, r) = (&channels[0][start..end], &channels[n_ch - 1][start..end]);
                post.push(l.iter().zip(r).map(|(a, b)| 0.5 * (a + b)));
            }
            if n_ch == 1 {
                peaks[1] = peaks[0];
                peaks[3] = peaks[2];
            }
            let a = &mut self.acc;
            for (k, p) in peaks.iter().enumerate() {
                a[k] = a[k].max(gain_to_db(p.max(1e-9)));
            }
            a[record::BAND] = a[record::BAND].max(tel.band_db);
            a[record::WHOLE] = a[record::WHOLE].max(tel.whole_db);
            a[record::GR] = a[record::GR].max(tel.gr_db);
            self.slice_pos += n;
            if self.slice_pos >= self.slice_len {
                history.push(&self.acc);
                self.acc = empty_record();
                self.slice_pos = 0;
            }
            start = end;
        }
    }
}

fn empty_record() -> [f32; record::WIDTH] {
    let mut r = [MINUS_INFINITY_DB; record::WIDTH];
    r[record::GR] = 0.0;
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bypass_is_the_input_at_the_latency_and_mono_runs() {
        let x: Vec<f32> = (0..4800).map(|i| (i as f32 * 0.05).sin() * 0.5).collect();
        let history = HistoryTap::new(64);
        let s = DeEssSettings { bypass: true, ..DeEssSettings::default() };
        let mut e = DeEssEngine::new(48_000.0, &s, 0.0);
        let (mut l, mut r) = (x.clone(), x.clone());
        for (a, b) in l.chunks_mut(333).zip(r.chunks_mut(333)) {
            e.process(&mut [a, b], &s, 0.0, &history, None);
        }
        let look = e.latency();
        assert_eq!(look, 144);
        assert_eq!(&l[look..], &x[..x.len() - look]);
        let mut m = x.clone();
        e.process(&mut [&mut m], &s, 0.0, &history, None);
        assert!(m.iter().all(|v| v.is_finite()));
    }
}
