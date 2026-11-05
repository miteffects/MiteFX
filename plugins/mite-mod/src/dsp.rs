//! Mite Mod's engine: the modulation (`mite_dsp::modulation`) and the output stage (gain,
//! bypass: the input at the latency, a mono input on both sides), plus the telemetry: one record
//! per 5 ms slice, whatever the host's block size.

use mite_core::HistoryTap;
use mite_dsp::modulation::{Mod, ModSettings, Transport};
use mite_dsp::{db_to_gain, gain_to_db, LinearSmoother, MINUS_INFINITY_DB};

/// Fields of a history record (`ModShared::history`).
pub mod record {
    /// Peak in (mono), out L and R, dBFS.
    pub const IN: usize = 0;
    pub const OUT: usize = 1;
    /// The LFO's phase at the slice's end: its whole cycles (mod 2²⁰) and the place in the cycle.
    pub const CYCLE: usize = 3;
    pub const PHASE: usize = 4;
    /// The LFO's rate, Hz.
    pub const RATE: usize = 5;
    /// The mode's readings (`ModTelemetry::values`).
    pub const VALUES: usize = 6;
    pub const WIDTH: usize = 14;
}

pub const HISTORY_SLICE_MS: f32 = 5.0;
pub const HISTORY_RECORDS: usize = 1600;
const SWITCH_MS: f32 = 10.0;
const GLIDE_MS: f32 = 20.0;
const CHUNK: usize = 512;

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Output {
    pub gain_db: f32,
    pub bypass: bool,
}

pub struct ModEngine {
    fx: Mod,
    wet: [Vec<f32>; 2],
    /// The input, for bypass at the latency.
    dry: [Vec<f32>; 2],
    dry_pos: usize,
    gain: LinearSmoother,
    bypass: LinearSmoother,
    slice_len: usize,
    slice_pos: usize,
    acc: [f32; record::WIDTH],
}

impl ModEngine {
    pub fn new(sample_rate: f32, s: &ModSettings, out: &Output) -> Self {
        let mut e = Self {
            fx: Mod::new(sample_rate, s),
            wet: [vec![0.0; CHUNK], vec![0.0; CHUNK]],
            dry: [Vec::new(), Vec::new()],
            dry_pos: 0,
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
    pub fn prepare(&mut self, sample_rate: f32, s: &ModSettings, out: &Output) {
        if self.fx.sample_rate() != sample_rate {
            self.fx = Mod::new(sample_rate, s);
        }
        let longest = mite_dsp::modulation::tz_latency(sample_rate).max(mite_dsp::modulation::vibrato_latency(sample_rate)) + 1;
        self.dry = [vec![0.0; longest], vec![0.0; longest]];
        self.slice_len = ((HISTORY_SLICE_MS * 0.001 * sample_rate).round() as usize).max(1);
        self.bypass.set_ramp(sample_rate, SWITCH_MS);
        self.gain.set_ramp(sample_rate, GLIDE_MS);
        self.reset(s, out);
    }

    pub fn reset(&mut self, s: &ModSettings, out: &Output) {
        self.fx.reset(s);
        for d in &mut self.dry {
            d.iter_mut().for_each(|v| *v = 0.0);
        }
        self.dry_pos = 0;
        self.gain.reset(db_to_gain(out.gain_db));
        self.bypass.reset(if out.bypass { 1.0 } else { 0.0 });
        self.slice_pos = 0;
        self.acc = empty_record();
    }

    /// The latency now, samples.
    pub fn latency(&self) -> usize {
        self.fx.latency()
    }

    /// `channels`: the output's two channels, holding the input (one channel when the input is
    /// mono: `stereo_in` false, the left holds it).
    pub fn process(&mut self, channels: &mut [&mut [f32]], stereo_in: bool, s: &ModSettings, out: &Output, transport: &Transport, history: &HistoryTap<{ record::WIDTH }>) {
        if channels.len() < 2 {
            return;
        }
        let len = channels[0].len().min(channels[1].len());
        self.bypass.set_target(if out.bypass { 1.0 } else { 0.0 });
        self.gain.set_target(db_to_gain(out.gain_db));
        let dry_len = self.dry[0].len();
        let mut start = 0;
        // The transport at each chunk: the song position moves with the samples.
        let beats_per_sample = transport.tempo.map_or(0.0, |t| t / 60.0 / f64::from(self.fx.sample_rate()));
        while start < len {
            let n = (len - start).min(CHUNK).min(self.slice_len - self.slice_pos);
            let end = start + n;
            let t = Transport { beats: transport.beats.map(|b| b + start as f64 * beats_per_sample), ..*transport };
            let tel = {
                let [wl, wr] = &mut self.wet;
                let (a, b) = channels.split_at_mut(1);
                let input: [&[f32]; 2] = [&a[0][start..end], &b[0][start..end]];
                self.fx.process(input, [&mut wl[..n], &mut wr[..n]], stereo_in, s, &t)
            };
            let lat = self.fx.latency().min(dry_len - 1);
            let mut peaks = [0.0f32; 3];
            for i in 0..n {
                let (bypass, gain) = (self.bypass.next_value(), self.gain.next_value());
                let xl = channels[0][start + i];
                let xr = if stereo_in { channels[1][start + i] } else { xl };
                self.dry[0][self.dry_pos] = xl;
                self.dry[1][self.dry_pos] = xr;
                let read = (self.dry_pos + dry_len - lat) % dry_len;
                let (dl, dr) = (self.dry[0][read], self.dry[1][read]);
                self.dry_pos = (self.dry_pos + 1) % dry_len;
                let (yl, yr) = (self.wet[0][i] * gain, self.wet[1][i] * gain);
                // Each blend as a·(1 − t) + b·t, so bypass gives the input exactly.
                let (ol, or) = (yl * (1.0 - bypass) + dl * bypass, yr * (1.0 - bypass) + dr * bypass);
                channels[0][start + i] = ol;
                channels[1][start + i] = or;
                peaks[0] = peaks[0].max((0.5 * (xl + xr)).abs());
                peaks[1] = peaks[1].max(ol.abs());
                peaks[2] = peaks[2].max(or.abs());
            }
            let a = &mut self.acc;
            for (k, p) in peaks.iter().enumerate() {
                a[k] = a[k].max(gain_to_db(p.max(1e-9)));
            }
            a[record::CYCLE] = (tel.phase.floor() % 1_048_576.0) as f32;
            a[record::PHASE] = tel.phase.fract() as f32;
            a[record::RATE] = tel.rate as f32;
            a[record::VALUES..record::VALUES + 8].copy_from_slice(&tel.values);
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
    for k in [record::IN, record::OUT, record::OUT + 1] {
        r[k] = MINUS_INFINITY_DB;
    }
    r
}

#[cfg(test)]
mod tests {
    use super::*;
    use mite_dsp::modulation::Mode;

    #[test]
    fn bypass_is_the_input_at_the_latency_and_a_mono_input_fills_both_sides() {
        let x: Vec<f32> = (0..4800).map(|i| (i as f32 * 0.05).sin() * 0.5).collect();
        let history = HistoryTap::new(64);
        for s in [ModSettings::default(), ModSettings { mode: Mode::Flanger, thru_zero: true, ..ModSettings::default() }, ModSettings { mode: Mode::Vibrato, ..ModSettings::default() }] {
            let out = Output { bypass: true, ..Output::default() };
            let mut e = ModEngine::new(48_000.0, &s, &out);
            let lat = e.latency();
            let (mut l, mut r) = (x.clone(), vec![0.0f32; x.len()]);
            for (a, b) in l.chunks_mut(333).zip(r.chunks_mut(333)) {
                e.process(&mut [a, b], false, &s, &out, &Transport::default(), &history);
            }
            assert_eq!(&l[lat..], &x[..x.len() - lat], "{:?}", s.mode);
            assert_eq!(&r[lat..], &x[..x.len() - lat]);
        }
    }
}
