//! Mite Comp's engine: the compressor (`mite_dsp::dynamics`) and its output stage (mix, delta and
//! bypass against the latency-aligned dry signal, output gain and balance), plus the history
//! telemetry: one record per 5 ms slice, whatever the host's block size.

use mite_core::HistoryTap;
use mite_dsp::dynamics::{CompSettings, CompTelemetry, Compressor, MAX_LOOKAHEAD_MS, STAGES};
use mite_dsp::output::{balance, DryDelay};
use mite_dsp::{db_to_gain, LinearSmoother, MINUS_INFINITY_DB};

/// Fields of a history record (`CompShared::history`).
pub mod record {
    /// Peak in and out, L and R, dBFS.
    pub const IN: usize = 0;
    pub const OUT: usize = 2;
    /// The deepest cut and the highest lift per processing channel (L/R or M/S), dB.
    pub const CUT: usize = 4;
    pub const LIFT: usize = 6;
    /// Each stage's gain by slot, dB (signed).
    pub const STAGE: usize = 8;
    /// The linked detector level, dBFS.
    pub const DETECTOR: usize = 12;
    /// Smart's slide of the thresholds, dB.
    pub const OFFSET: usize = 13;
    pub const WIDTH: usize = 16;
}

/// One history record per this much audio.
pub const HISTORY_SLICE_MS: f32 = 5.0;
/// Records kept: 30 s (what pausing keeps, per the design).
pub const HISTORY_RECORDS: usize = 6000;
/// Longest chunk the engine processes at once (scratch buffers are this long).
const CHUNK: usize = 512;
/// Switch fades (delta, bypass, the latency dip) and output glides (mix, gain, balance), ms.
const SWITCH_MS: f32 = 10.0;
const GLIDE_MS: f32 = 20.0;

/// The output stage as the parameters set it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Output {
    /// 0…1.
    pub mix: f32,
    pub delta: bool,
    pub bypass: bool,
    pub gain_db: f32,
    /// −1 … 1.
    pub pan: f32,
}

impl Default for Output {
    fn default() -> Self {
        Self { mix: 1.0, delta: false, bypass: false, gain_db: 0.0, pan: 0.0 }
    }
}

pub struct CompEngine {
    sample_rate: f32,
    comp: Compressor,
    dry: DryDelay,
    scratch: [Vec<f32>; 2],
    mix: LinearSmoother,
    delta: LinearSmoother,
    bypass: LinearSmoother,
    gain: LinearSmoother,
    pan: LinearSmoother,
    /// The latency switch: fades out, applies the new lookahead, fades back in.
    dip: LinearSmoother,
    lookahead_ms: f32,
    slice_len: usize,
    slice_pos: usize,
    acc: [f32; record::WIDTH],
}

impl CompEngine {
    pub fn new(sample_rate: f32, s: &CompSettings) -> Self {
        let mut e = Self {
            sample_rate,
            comp: Compressor::new(sample_rate),
            dry: DryDelay::new(1, 1),
            scratch: [Vec::new(), Vec::new()],
            mix: LinearSmoother::new(1.0),
            delta: LinearSmoother::new(0.0),
            bypass: LinearSmoother::new(0.0),
            gain: LinearSmoother::new(1.0),
            pan: LinearSmoother::new(0.0),
            dip: LinearSmoother::new(1.0),
            lookahead_ms: s.lookahead_ms,
            slice_len: 1,
            slice_pos: 0,
            acc: empty_record(),
        };
        e.prepare(sample_rate, s, &Output::default());
        e
    }

    /// Sizes everything for `sample_rate`. Allocates.
    pub fn prepare(&mut self, sample_rate: f32, s: &CompSettings, out: &Output) {
        self.sample_rate = sample_rate;
        let max_latency = (MAX_LOOKAHEAD_MS * 0.001 * sample_rate).ceil() as usize + 1;
        self.dry = DryDelay::new(max_latency, CHUNK);
        self.scratch = [vec![0.0; CHUNK], vec![0.0; CHUNK]];
        self.slice_len = ((HISTORY_SLICE_MS * 0.001 * sample_rate).round() as usize).max(1);
        for sm in [&mut self.delta, &mut self.bypass, &mut self.dip] {
            sm.set_ramp(sample_rate, SWITCH_MS);
        }
        for sm in [&mut self.mix, &mut self.gain, &mut self.pan] {
            sm.set_ramp(sample_rate, GLIDE_MS);
        }
        self.lookahead_ms = s.lookahead_ms;
        self.comp.prepare(sample_rate, s);
        self.reset(s, out);
    }

    /// Clears the state and snaps every glide to `s` and `out`.
    pub fn reset(&mut self, s: &CompSettings, out: &Output) {
        self.lookahead_ms = s.lookahead_ms;
        self.comp.reset(&self.active(s));
        self.dry.reset();
        self.dry.set_delay(self.comp.latency());
        self.mix.reset(out.mix);
        self.delta.reset(if out.delta { 1.0 } else { 0.0 });
        self.bypass.reset(if out.bypass { 1.0 } else { 0.0 });
        self.gain.reset(db_to_gain(out.gain_db));
        self.pan.reset(out.pan);
        self.dip.reset(1.0);
        self.slice_pos = 0;
        self.acc = empty_record();
    }

    /// The latency in force, samples (it follows a lookahead change once the dip has faded out).
    pub fn latency(&self) -> usize {
        self.comp.latency()
    }

    /// `s` with the lookahead in force (a change waits for the dip).
    fn active(&self, s: &CompSettings) -> CompSettings {
        CompSettings { lookahead_ms: self.lookahead_ms, ..*s }
    }

    /// Processes one host block in place and pushes the history records it completes.
    pub fn process(&mut self, channels: &mut [&mut [f32]], key: Option<&[&[f32]]>, s: &CompSettings, out: &Output, history: &HistoryTap<{ record::WIDTH }>) {
        let n_ch = channels.len().min(2);
        if n_ch == 0 {
            return;
        }
        let len = channels.iter().map(|c| c.len()).min().unwrap_or(0);
        // Listen replaces the output with the detector signal: no mix, delta or bypass then.
        let (mix, delta, bypass) = if s.listen { (1.0, false, false) } else { (out.mix, out.delta, out.bypass) };
        self.mix.set_target(mix);
        self.delta.set_target(if delta { 1.0 } else { 0.0 });
        self.bypass.set_target(if bypass { 1.0 } else { 0.0 });
        self.gain.set_target(db_to_gain(out.gain_db));
        self.pan.set_target(if n_ch == 2 { out.pan } else { 0.0 });

        let mut start = 0;
        while start < len {
            // A lookahead change: fade out, switch once silent, fade back in.
            if s.lookahead_ms != self.lookahead_ms {
                self.dip.set_target(0.0);
                if self.dip.current() == 0.0 {
                    self.lookahead_ms = s.lookahead_ms;
                    self.dry.set_delay(s.latency(self.sample_rate));
                    self.dip.set_target(1.0);
                }
            }
            let n = (len - start).min(CHUNK).min(self.slice_len - self.slice_pos);
            let end = start + n;
            for c in 0..n_ch {
                self.scratch[c][..n].copy_from_slice(&channels[c][start..end]);
            }
            self.dry.write(&channels[0][start..end], (n_ch == 2).then(|| &channels[1][start..end]));
            let active = self.active(s);
            let key_chunk: Option<[&[f32]; 2]> = key.filter(|k| !k.is_empty() && k.iter().all(|c| c.len() >= end)).map(|k| [&k[0][start..end], &k[k.len().min(2) - 1][start..end]]);
            let telemetry = {
                let [a, b] = &mut self.scratch;
                let mut parts: [&mut [f32]; 2] = [&mut a[..n], &mut b[..n]];
                self.comp.process(&mut parts[..n_ch], key_chunk.as_ref().map(|k| &k[..]), &active)
            };
            for i in 0..n {
                let (mix, delta, bypass) = (self.mix.next_value(), self.delta.next_value(), self.bypass.next_value());
                let gain = self.gain.next_value() * self.dip.next_value();
                let (pl, pr) = balance(self.pan.next_value());
                for c in 0..n_ch {
                    let wet = self.scratch[c][i];
                    let dry = self.dry.read(c, i);
                    // Each blend as a·(1 − t) + b·t, so its ends are exact (mix 0 and bypass null).
                    let mixed = dry * (1.0 - mix) + wet * mix;
                    let y = mixed * (1.0 - delta) + (wet - dry) * delta;
                    let y = y * (1.0 - bypass) + dry * bypass;
                    let side = if n_ch == 2 { if c == 0 { pl } else { pr } } else { 1.0 };
                    channels[c][start + i] = y * gain * side;
                }
            }
            self.dry.advance(n);
            self.accumulate(&telemetry);
            self.slice_pos += n;
            if self.slice_pos >= self.slice_len {
                history.push(&self.acc);
                self.acc = empty_record();
                self.slice_pos = 0;
            }
            start = end;
        }
    }

    fn accumulate(&mut self, t: &CompTelemetry) {
        let a = &mut self.acc;
        for c in 0..2 {
            a[record::IN + c] = a[record::IN + c].max(t.in_peak_db[c]);
            a[record::OUT + c] = a[record::OUT + c].max(t.out_peak_db[c]);
            a[record::CUT + c] = a[record::CUT + c].min(t.cut_db[c]);
            a[record::LIFT + c] = a[record::LIFT + c].max(t.lift_db[c]);
        }
        for k in 0..STAGES {
            a[record::STAGE + k] = t.stage_db[k];
        }
        a[record::DETECTOR] = a[record::DETECTOR].max(t.detector_db);
        a[record::OFFSET] = t.smart_offset_db;
    }
}

fn empty_record() -> [f32; record::WIDTH] {
    let mut r = [0.0; record::WIDTH];
    for i in [record::IN, record::IN + 1, record::OUT, record::OUT + 1, record::DETECTOR] {
        r[i] = MINUS_INFINITY_DB;
    }
    r
}

#[cfg(test)]
mod tests {
    use super::*;
    use mite_dsp::dynamics::{Detector, Quadrant, Stage};

    const FS: f32 = 48_000.0;

    fn compressing() -> CompSettings {
        let mut s = CompSettings::default();
        s.stages[0] = Stage { on: true, quadrant: Quadrant::Compress, threshold_db: -30.0, ratio: 4.0, detector: Detector::Peak, ..Stage::default() };
        s
    }

    fn noise(n: usize, seed: u32) -> Vec<f32> {
        let mut x = seed;
        (0..n)
            .map(|_| {
                x = x.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                ((x >> 8) as f32 / (1u32 << 24) as f32 - 0.5) * 0.5
            })
            .collect()
    }

    fn run(e: &mut CompEngine, input: &[f32], s: &CompSettings, out: &Output, block: usize) -> Vec<f32> {
        let tap = HistoryTap::new(1024);
        let mut y = input.to_vec();
        for chunk in y.chunks_mut(block) {
            e.process(&mut [chunk], None, s, out, &tap);
        }
        y
    }

    #[test]
    fn bypass_and_mix_zero_null_against_the_delayed_input() {
        let s = CompSettings { lookahead_ms: 3.0, ..compressing() };
        let input = noise(9600, 1);
        let latency = s.latency(FS);
        for out in [Output { bypass: true, ..Output::default() }, Output { mix: 0.0, ..Output::default() }] {
            let mut e = CompEngine::new(FS, &s);
            e.prepare(FS, &s, &out);
            let y = run(&mut e, &input, &s, &out, 256);
            assert_eq!(e.latency(), latency);
            for i in latency..input.len() {
                assert_eq!(y[i], input[i - latency], "{out:?} at {i}");
            }
        }
    }

    #[test]
    fn delta_plus_dry_is_the_wet_signal() {
        let s = compressing();
        let input = noise(9600, 2);
        let mut wet_engine = CompEngine::new(FS, &s);
        let wet = run(&mut wet_engine, &input, &s, &Output::default(), 480);
        let delta_out = Output { delta: true, ..Output::default() };
        let mut delta_engine = CompEngine::new(FS, &s);
        delta_engine.prepare(FS, &s, &delta_out);
        let delta = run(&mut delta_engine, &input, &s, &delta_out, 480);
        for i in 0..input.len() {
            assert!((delta[i] + input[i] - wet[i]).abs() < 1e-6, "at {i}");
        }
    }

    #[test]
    fn block_size_does_not_change_the_output() {
        let s = CompSettings { lookahead_ms: 2.0, ..compressing() };
        let input = noise(12_000, 3);
        let mut reference = None;
        for block in [1, 7, 64, 128, 4096, 8192] {
            let mut e = CompEngine::new(FS, &s);
            let y = run(&mut e, &input, &s, &Output::default(), block);
            match &reference {
                None => reference = Some(y),
                Some(r) => assert_eq!(&y, r, "block {block}"),
            }
        }
    }

    #[test]
    fn history_gets_one_record_per_slice() {
        let s = compressing();
        let mut e = CompEngine::new(FS, &s);
        let tap = HistoryTap::new(1024);
        let mut y = noise(4800, 4);
        for chunk in y.chunks_mut(333) {
            e.process(&mut [chunk], None, &s, &Output::default(), &tap);
        }
        // 100 ms of 5 ms slices.
        assert_eq!(tap.count(), 20);
        let mut seen = 0;
        let mut cut = 0.0f32;
        tap.read_since(&mut seen, |r| cut = cut.min(r[record::CUT]));
        assert!(cut < -1.0, "the history shows the gain reduction: {cut}");
    }

    #[test]
    fn a_lookahead_change_moves_the_latency_after_a_dip() {
        let mut s = compressing();
        let mut e = CompEngine::new(FS, &s);
        let input = noise(4800, 5);
        run(&mut e, &input, &s, &Output::default(), 480);
        assert_eq!(e.latency(), 0);
        s.lookahead_ms = 5.0;
        run(&mut e, &input, &s, &Output::default(), 480);
        assert_eq!(e.latency(), 240);
    }
}
