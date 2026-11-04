//! Mite Double's engine: the doubler (`mite_dsp::doubler`, the source and its doubles placed in
//! stereo) and the output stage (gain, bypass: the input as it came, a mono input on both
//! sides), plus the telemetry: one record per 5 ms slice, whatever the host's block size. No
//! latency (the source is never delayed).

use mite_core::HistoryTap;
use mite_dsp::doubler::{auto_gain_db, Doubler, DoublerSettings, MAX_VOICES};
use mite_dsp::{db_to_gain, gain_to_db, LinearSmoother, MINUS_INFINITY_DB};

/// Fields of a history record (`DoubleShared::history`).
pub mod record {
    /// Peak in (mono), out L and R, dBFS.
    pub const IN: usize = 0;
    pub const OUT: usize = 1;
    /// Per double: its reading (ms), its detune (cents), its peak (dBFS).
    pub const DELAY: usize = 3;
    pub const CENTS: usize = 6;
    pub const PEAK: usize = 9;
    /// 1 when a note began in the slice.
    pub const ONSET: usize = 12;
    /// The output's left/right correlation over the slice (−1…1; 1 when silent).
    pub const CORR: usize = 13;
    pub const WIDTH: usize = 14;
}

pub const HISTORY_SLICE_MS: f32 = 5.0;
pub const HISTORY_RECORDS: usize = 1600;
const SWITCH_MS: f32 = 10.0;
const GLIDE_MS: f32 = 20.0;

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Output {
    pub gain_db: f32,
    pub bypass: bool,
    /// Keep the output as loud as the input (`mite_dsp::doubler::auto_gain_db`).
    pub auto_gain: bool,
}

pub struct DoubleEngine {
    doubler: Doubler,
    wet: [Vec<f32>; 2],
    gain: LinearSmoother,
    bypass: LinearSmoother,
    slice_len: usize,
    slice_pos: usize,
    acc: [f32; record::WIDTH],
    corr: [f64; 3],
}

const CHUNK: usize = 512;

impl DoubleEngine {
    pub fn new(sample_rate: f32, out: &Output) -> Self {
        let mut e = Self {
            doubler: Doubler::new(sample_rate),
            wet: [vec![0.0; CHUNK], vec![0.0; CHUNK]],
            gain: LinearSmoother::new(1.0),
            bypass: LinearSmoother::new(0.0),
            slice_len: 1,
            slice_pos: 0,
            acc: empty_record(),
            corr: [0.0; 3],
        };
        e.prepare(sample_rate, out);
        e
    }

    /// Sizes everything for `sample_rate`. Allocates.
    pub fn prepare(&mut self, sample_rate: f32, out: &Output) {
        if self.doubler.sample_rate() != sample_rate {
            self.doubler = Doubler::new(sample_rate);
        }
        self.slice_len = ((HISTORY_SLICE_MS * 0.001 * sample_rate).round() as usize).max(1);
        self.bypass.set_ramp(sample_rate, SWITCH_MS);
        self.gain.set_ramp(sample_rate, GLIDE_MS);
        self.reset(out);
    }

    pub fn reset(&mut self, out: &Output) {
        self.doubler.reset();
        self.gain.reset(db_to_gain(out.gain_db));
        self.bypass.reset(if out.bypass { 1.0 } else { 0.0 });
        self.slice_pos = 0;
        self.acc = empty_record();
        self.corr = [0.0; 3];
    }

    pub fn latency(&self) -> usize {
        0
    }

    /// `channels`: the output's two channels, holding the input (one channel when the input is
    /// mono: `stereo_in` false, the left holds it).
    pub fn process(&mut self, channels: &mut [&mut [f32]], stereo_in: bool, s: &DoublerSettings, out: &Output, history: &HistoryTap<{ record::WIDTH }>) {
        if channels.len() < 2 {
            return;
        }
        let len = channels[0].len().min(channels[1].len());
        self.bypass.set_target(if out.bypass { 1.0 } else { 0.0 });
        self.gain.set_target(db_to_gain(out.gain_db + if out.auto_gain { auto_gain_db(s) } else { 0.0 }));
        let mut start = 0;
        while start < len {
            let n = (len - start).min(CHUNK).min(self.slice_len - self.slice_pos);
            let end = start + n;
            let tel = {
                let [wl, wr] = &mut self.wet;
                let (a, b) = channels.split_at_mut(1);
                let input: [&[f32]; 2] = [&a[0][start..end], &b[0][start..end]];
                self.doubler.process(input, [&mut wl[..n], &mut wr[..n]], stereo_in, s)
            };
            let mut peaks = [0.0f32; 3];
            for i in 0..n {
                let (bypass, gain) = (self.bypass.next_value(), self.gain.next_value());
                let xl = channels[0][start + i];
                let xr = if stereo_in { channels[1][start + i] } else { xl };
                let (yl, yr) = (self.wet[0][i] * gain, self.wet[1][i] * gain);
                // Each blend as a·(1 − t) + b·t, so bypass gives the input exactly.
                let (ol, or) = (yl * (1.0 - bypass) + xl * bypass, yr * (1.0 - bypass) + xr * bypass);
                channels[0][start + i] = ol;
                channels[1][start + i] = or;
                peaks[0] = peaks[0].max((0.5 * (xl + xr)).abs());
                peaks[1] = peaks[1].max(ol.abs());
                peaks[2] = peaks[2].max(or.abs());
                self.corr[0] += f64::from(ol) * f64::from(or);
                self.corr[1] += f64::from(ol) * f64::from(ol);
                self.corr[2] += f64::from(or) * f64::from(or);
            }
            let a = &mut self.acc;
            for (k, p) in peaks.iter().enumerate() {
                a[k] = a[k].max(gain_to_db(p.max(1e-9)));
            }
            for v in 0..MAX_VOICES {
                a[record::DELAY + v] = tel.delay_ms[v];
                a[record::CENTS + v] = tel.cents[v];
                a[record::PEAK + v] = a[record::PEAK + v].max(gain_to_db(tel.peak[v].max(1e-9)));
            }
            if tel.onset {
                a[record::ONSET] = 1.0;
            }
            self.slice_pos += n;
            if self.slice_pos >= self.slice_len {
                let [lr, ll, rr] = self.corr;
                a[record::CORR] = if ll * rr > 1e-12 { (lr / (ll * rr).sqrt()) as f32 } else { 1.0 };
                history.push(&self.acc);
                self.acc = empty_record();
                self.corr = [0.0; 3];
                self.slice_pos = 0;
            }
            start = end;
        }
    }
}

fn empty_record() -> [f32; record::WIDTH] {
    let mut r = [0.0; record::WIDTH];
    for k in [record::IN, record::OUT, record::OUT + 1, record::PEAK, record::PEAK + 1, record::PEAK + 2] {
        r[k] = MINUS_INFINITY_DB;
    }
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bypass_is_the_input_and_a_mono_input_fills_both_sides() {
        let x: Vec<f32> = (0..4800).map(|i| (i as f32 * 0.05).sin() * 0.5).collect();
        let history = HistoryTap::new(64);
        let s = DoublerSettings::default();
        let out = Output { bypass: true, ..Output::default() };
        let mut e = DoubleEngine::new(48_000.0, &out);
        let (mut l, mut r) = (x.clone(), vec![0.0f32; x.len()]);
        for (a, b) in l.chunks_mut(333).zip(r.chunks_mut(333)) {
            e.process(&mut [a, b], false, &s, &out, &history);
        }
        assert_eq!(l, x);
        assert_eq!(r, x);
        assert_eq!(e.latency(), 0);
    }

    #[test]
    fn the_source_is_never_late() {
        // Source hard left, doubles off: the left is the input, at once.
        let mut x = vec![0.0f32; 4800];
        x[100] = 1.0;
        let history = HistoryTap::new(64);
        let s = DoublerSettings { voices: 0, ..DoublerSettings::default() };
        let out = Output::default();
        let mut e = DoubleEngine::new(48_000.0, &out);
        let (mut l, mut r) = (x.clone(), vec![0.0f32; x.len()]);
        e.process(&mut [&mut l, &mut r], false, &s, &out, &history);
        assert!((l[100] - 1.0).abs() < 1e-6);
        assert!(r.iter().all(|v| v.abs() < 1e-6));
    }
}
