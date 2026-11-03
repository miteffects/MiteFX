//! Mite Multiband's engine: `mite_dsp::multiband` plus its telemetry: one record per 5 ms slice
//! (levels in and out, each band's input peak and gain change), and the input and output for the
//! spectrum.

use mite_core::{HistoryTap, SpectrumTap};
use mite_dsp::multiband::{MbSettings, Multiband, BANDS};
use mite_dsp::MINUS_INFINITY_DB;

/// Fields of a history record (`MbShared::history`).
pub mod record {
    use mite_dsp::multiband::BANDS;
    /// Peak in and out, L and R, dBFS.
    pub const IN: usize = 0;
    pub const OUT: usize = 2;
    /// Each band slot's input peak (dBFS) and gain change (signed dB).
    pub const BAND_IN: usize = 4;
    pub const BAND_GAIN: usize = BAND_IN + BANDS;
    pub const WIDTH: usize = BAND_GAIN + BANDS;
}

pub const HISTORY_SLICE_MS: f32 = 5.0;
pub const HISTORY_RECORDS: usize = 6000;

pub struct MbEngine {
    mb: Multiband,
    slice_len: usize,
    slice_pos: usize,
    acc: [f32; record::WIDTH],
    mono: Vec<f32>,
}

impl MbEngine {
    pub fn new(sample_rate: f32, s: &MbSettings) -> Self {
        let mut e = Self { mb: Multiband::new(sample_rate), slice_len: 1, slice_pos: 0, acc: empty_record(), mono: vec![0.0; 8192] };
        e.prepare(sample_rate, s);
        e
    }

    /// Allocates.
    pub fn prepare(&mut self, sample_rate: f32, s: &MbSettings) {
        self.mb = Multiband::new(sample_rate);
        self.slice_len = ((HISTORY_SLICE_MS * 0.001 * sample_rate).round() as usize).max(1);
        self.reset(s);
    }

    pub fn reset(&mut self, s: &MbSettings) {
        self.mb.reset(s);
        self.slice_pos = 0;
        self.acc = empty_record();
    }

    pub fn latency(&self) -> usize {
        self.mb.latency()
    }

    /// Processes one host block in place; `spectrum` takes the input and the output (mono).
    pub fn process(&mut self, channels: &mut [&mut [f32]], key: Option<&[&[f32]]>, s: &MbSettings, history: &HistoryTap<{ record::WIDTH }>, spectrum: Option<[&SpectrumTap; 2]>) {
        let n_ch = channels.len().min(2);
        if n_ch == 0 {
            return;
        }
        let len = channels.iter().map(|c| c.len()).min().unwrap_or(0);
        let mut start = 0;
        while start < len {
            let n = (len - start).min(self.slice_len - self.slice_pos).min(self.mono.len());
            let end = start + n;
            if let Some([pre, _]) = spectrum {
                for (i, m) in self.mono[..n].iter_mut().enumerate() {
                    *m = 0.5 * (channels[0][start + i] + channels[n_ch - 1][start + i]);
                }
                pre.push(self.mono[..n].iter().copied());
            }
            let key_chunk: Option<[&[f32]; 2]> = key.filter(|k| !k.is_empty() && k.iter().all(|c| c.len() >= end)).map(|k| [&k[0][start..end], &k[k.len().min(2) - 1][start..end]]);
            let t = {
                let (first, rest) = channels.split_at_mut(1);
                let a = &mut first[0][start..end];
                if n_ch == 2 {
                    let b = &mut rest[0][start..end];
                    self.mb.process(&mut [a, b], key_chunk.as_ref().map(|k| &k[..]), s)
                } else {
                    self.mb.process(&mut [a], key_chunk.as_ref().map(|k| &k[..1]), s)
                }
            };
            if let Some([_, post]) = spectrum {
                post.push((0..n).map(|i| 0.5 * (channels[0][start + i] + channels[n_ch - 1][start + i])));
            }
            let a = &mut self.acc;
            for c in 0..2 {
                a[record::IN + c] = a[record::IN + c].max(t.in_peak_db[c]);
                a[record::OUT + c] = a[record::OUT + c].max(t.out_peak_db[c]);
            }
            for b in 0..BANDS {
                a[record::BAND_IN + b] = a[record::BAND_IN + b].max(t.band_in_db[b]);
                let g = t.band_gain_db[b];
                if g.abs() > a[record::BAND_GAIN + b].abs() {
                    a[record::BAND_GAIN + b] = g;
                }
            }
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
    let mut r = [0.0; record::WIDTH];
    for i in [record::IN, record::IN + 1, record::OUT, record::OUT + 1] {
        r[i] = MINUS_INFINITY_DB;
    }
    for b in 0..BANDS {
        r[record::BAND_IN + b] = MINUS_INFINITY_DB;
    }
    r
}
