//! Mite Verb's engine: the reverb (`mite_dsp::reverb`) and its output stage (mix against the dry
//! input, output gain, bypass), plus the history telemetry: one record per 5 ms slice, whatever
//! the host's block size. No latency: mix 0 % and bypass give the input bit for bit.

use mite_core::{HistoryTap, SpectrumTap};
use mite_dsp::reverb::{Reverb, ReverbSettings};
use mite_dsp::{db_to_gain, gain_to_db, LinearSmoother, MINUS_INFINITY_DB};

/// Fields of a history record (`VerbShared::history`).
pub mod record {
    /// Peak in, out and of the reverb alone, L and R, dBFS.
    pub const IN: usize = 0;
    pub const OUT: usize = 2;
    pub const WET: usize = 4;
    /// The deepest ducking, dB (≤ 0).
    pub const DUCK: usize = 6;
    /// How frozen the tank is, 0…1.
    pub const FREEZE: usize = 7;
    pub const WIDTH: usize = 8;
}

pub const HISTORY_SLICE_MS: f32 = 5.0;
pub const HISTORY_RECORDS: usize = 6000;
const CHUNK: usize = 512;
const SWITCH_MS: f32 = 10.0;
const GLIDE_MS: f32 = 20.0;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Output {
    /// 0…1.
    pub mix: f32,
    pub gain_db: f32,
    pub bypass: bool,
}

impl Default for Output {
    fn default() -> Self {
        Self { mix: 0.3, gain_db: 0.0, bypass: false }
    }
}

pub struct VerbEngine {
    reverb: Reverb,
    wet: [Vec<f32>; 2],
    mix: LinearSmoother,
    gain: LinearSmoother,
    bypass: LinearSmoother,
    slice_len: usize,
    slice_pos: usize,
    acc: [f32; record::WIDTH],
}

impl VerbEngine {
    pub fn new(sample_rate: f32, s: &ReverbSettings, out: &Output) -> Self {
        let mut e = Self {
            reverb: Reverb::new(sample_rate, s),
            wet: [vec![0.0; CHUNK], vec![0.0; CHUNK]],
            mix: LinearSmoother::new(out.mix),
            gain: LinearSmoother::new(1.0),
            bypass: LinearSmoother::new(0.0),
            slice_len: 1,
            slice_pos: 0,
            acc: empty_record(),
        };
        e.prepare(sample_rate, s, out);
        e
    }

    /// Sizes everything for `sample_rate`. Allocates.
    pub fn prepare(&mut self, sample_rate: f32, s: &ReverbSettings, out: &Output) {
        if self.reverb.sample_rate() != sample_rate {
            self.reverb = Reverb::new(sample_rate, s);
        }
        self.slice_len = ((HISTORY_SLICE_MS * 0.001 * sample_rate).round() as usize).max(1);
        self.bypass.set_ramp(sample_rate, SWITCH_MS);
        for sm in [&mut self.mix, &mut self.gain] {
            sm.set_ramp(sample_rate, GLIDE_MS);
        }
        self.reset(s, out);
    }

    pub fn reset(&mut self, s: &ReverbSettings, out: &Output) {
        self.reverb.reset(s);
        self.mix.reset(out.mix);
        self.gain.reset(db_to_gain(out.gain_db));
        self.bypass.reset(if out.bypass { 1.0 } else { 0.0 });
        self.slice_pos = 0;
        self.acc = empty_record();
    }

    /// The latency, samples: none.
    pub fn latency(&self) -> usize {
        0
    }

    /// `spectra` takes the input and the reverb alone (mono), for the editor's plot.
    pub fn process(&mut self, channels: &mut [&mut [f32]], s: &ReverbSettings, out: &Output, history: &HistoryTap<{ record::WIDTH }>, spectra: Option<[&SpectrumTap; 2]>) {
        let n_ch = channels.len().min(2);
        if n_ch == 0 {
            return;
        }
        let stereo = n_ch == 2;
        let len = channels.iter().map(|c| c.len()).min().unwrap_or(0);
        self.mix.set_target(out.mix);
        self.bypass.set_target(if out.bypass { 1.0 } else { 0.0 });
        self.gain.set_target(db_to_gain(out.gain_db));
        let mut start = 0;
        while start < len {
            let n = (len - start).min(CHUNK).min(self.slice_len - self.slice_pos);
            let end = start + n;
            let tel = {
                let [wl, wr] = &mut self.wet;
                let input: [&[f32]; 2] = [&channels[0][start..end], &channels[n_ch - 1][start..end]];
                self.reverb.process(input, [&mut wl[..n], &mut wr[..n]], stereo, s)
            };
            if !stereo {
                let (l, r) = self.wet.split_at_mut(1);
                r[0][..n].copy_from_slice(&l[0][..n]);
            }
            if let Some([pre, wet]) = spectra {
                pre.push(channels[0][start..end].iter().zip(&channels[n_ch - 1][start..end]).map(|(a, b)| 0.5 * (a + b)));
                wet.push(self.wet[0][..n].iter().zip(&self.wet[1][..n]).map(|(a, b)| 0.5 * (a + b)));
            }
            let mut peaks = [0.0f32; 6];
            for i in 0..n {
                let (mix, bypass, gain) = (self.mix.next_value(), self.bypass.next_value(), self.gain.next_value());
                for (c, ch) in channels.iter_mut().enumerate().take(n_ch) {
                    let dry = ch[start + i];
                    let wet = self.wet[c][i];
                    // Each blend as a·(1 − t) + b·t, so its ends are exact (mix 0 and bypass null).
                    let y = (dry * (1.0 - mix) + wet * mix) * gain;
                    let y = y * (1.0 - bypass) + dry * bypass;
                    ch[start + i] = y;
                    peaks[c] = peaks[c].max(dry.abs());
                    peaks[2 + c] = peaks[2 + c].max(y.abs());
                    peaks[4 + c] = peaks[4 + c].max(wet.abs());
                }
            }
            if !stereo {
                for k in [0, 2, 4] {
                    peaks[k + 1] = peaks[k];
                }
            }
            let a = &mut self.acc;
            for (k, p) in peaks.iter().enumerate() {
                a[k] = a[k].max(gain_to_db(p.max(1e-9)));
            }
            a[record::DUCK] = a[record::DUCK].min(tel.duck_db);
            a[record::FREEZE] = a[record::FREEZE].max(tel.freeze);
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
    for v in r.iter_mut().take(6) {
        *v = MINUS_INFINITY_DB;
    }
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(n: usize, db: f32) -> Vec<f32> {
        let a = db_to_gain(db);
        (0..n).map(|i| a * (i as f32 * 0.0731).sin()).collect()
    }

    fn run(e: &mut VerbEngine, s: &ReverbSettings, out: &Output, l: &mut [f32], r: &mut [f32], block: usize) {
        let history = HistoryTap::new(64);
        for (a, b) in l.chunks_mut(block).zip(r.chunks_mut(block)) {
            e.process(&mut [a, b], s, out, &history, None);
        }
    }

    #[test]
    fn mix_zero_and_bypass_null_to_the_input() {
        let x = tone(9600, -12.0);
        let s = ReverbSettings::default();
        for out in [Output { mix: 0.0, ..Output::default() }, Output { bypass: true, ..Output::default() }] {
            let mut e = VerbEngine::new(48000.0, &s, &out);
            let (mut l, mut r) = (x.clone(), x.clone());
            run(&mut e, &s, &out, &mut l, &mut r, 333);
            assert_eq!(e.latency(), 0);
            assert_eq!(l, x);
            assert_eq!(r, x);
        }
    }

    #[test]
    fn full_mix_is_the_reverb_alone() {
        let mut x = vec![0.0f32; 4800];
        x[0] = 1.0;
        let s = ReverbSettings { predelay_ms: 20.0, early: 0.0, ..ReverbSettings::default() };
        let out = Output { mix: 1.0, ..Output::default() };
        let mut e = VerbEngine::new(48000.0, &s, &out);
        let (mut l, mut r) = (x.clone(), x.clone());
        run(&mut e, &s, &out, &mut l, &mut r, 256);
        // Nothing before the pre-delay: the dry impulse is gone.
        assert!(l[..900].iter().all(|v| v.abs() < 1e-9), "{}", l[0]);
        assert!(l.iter().any(|v| v.abs() > 1e-4));
    }

    #[test]
    fn mono_runs_one_channel() {
        let x = tone(4800, -12.0);
        let s = ReverbSettings::default();
        let out = Output::default();
        let mut e = VerbEngine::new(48000.0, &s, &out);
        let history = HistoryTap::new(64);
        let mut l = x.clone();
        e.process(&mut [&mut l], &s, &out, &history, None);
        assert!(l.iter().all(|v| v.is_finite()));
        assert_ne!(l, x);
    }
}
