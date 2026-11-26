//! Mite Comp's engine: the compressor (`mite_dsp::dynamics`) and its output stage (mix, delta and
//! bypass against the latency-aligned dry signal, output gain and balance), plus the history
//! telemetry: one record per 5 ms slice, whatever the host's block size.

use mite_core::HistoryTap;
use mite_dsp::dynamics::{CompSettings, CompTelemetry, Compressor, MAX_LOOKAHEAD_MS, STAGES};
use mite_dsp::output::{balance, DryDelay};
use mite_dsp::oversample::{Oversampler, MAX_FACTOR};
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
    /// The RMS and loudness detectors' highest levels, dBFS (the peak's is `DETECTOR`).
    pub const DET_RMS: usize = 14;
    pub const DET_LOUD: usize = 15;
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
    /// Oversampling: the audio's and the key's resamplers, and their high-rate buffers.
    os: Oversampler,
    os_key: Oversampler,
    hi: [Vec<f32>; 2],
    hi_key: [Vec<f32>; 2],
    mix: LinearSmoother,
    delta: LinearSmoother,
    bypass: LinearSmoother,
    gain: LinearSmoother,
    pan: LinearSmoother,
    /// The latency switch: fades out, applies the new lookahead or oversampling, fades back in.
    dip: LinearSmoother,
    lookahead_ms: f32,
    oversampling: usize,
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
            os: Oversampler::new(1),
            os_key: Oversampler::new(1),
            hi: [Vec::new(), Vec::new()],
            hi_key: [Vec::new(), Vec::new()],
            mix: LinearSmoother::new(1.0),
            delta: LinearSmoother::new(0.0),
            bypass: LinearSmoother::new(0.0),
            gain: LinearSmoother::new(1.0),
            pan: LinearSmoother::new(0.0),
            dip: LinearSmoother::new_s_curve(1.0),
            lookahead_ms: s.lookahead_ms,
            oversampling: s.oversampling,
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
        let max_latency = (MAX_LOOKAHEAD_MS * 0.001 * sample_rate).ceil() as usize + 1 + Oversampler::latency_for(MAX_FACTOR);
        self.dry = DryDelay::new(max_latency, CHUNK);
        self.scratch = [vec![0.0; CHUNK], vec![0.0; CHUNK]];
        self.hi = [vec![0.0; CHUNK * MAX_FACTOR], vec![0.0; CHUNK * MAX_FACTOR]];
        self.hi_key = [vec![0.0; CHUNK * MAX_FACTOR], vec![0.0; CHUNK * MAX_FACTOR]];
        self.slice_len = ((HISTORY_SLICE_MS * 0.001 * sample_rate).round() as usize).max(1);
        for sm in [&mut self.delta, &mut self.bypass, &mut self.dip] {
            sm.set_ramp(sample_rate, SWITCH_MS);
        }
        for sm in [&mut self.mix, &mut self.gain, &mut self.pan] {
            sm.set_ramp(sample_rate, GLIDE_MS);
        }
        self.lookahead_ms = s.lookahead_ms;
        self.oversampling = s.oversampling;
        self.comp.prepare(sample_rate, s);
        self.reset(s, out);
    }

    /// Clears the state and snaps every glide to `s` and `out`.
    pub fn reset(&mut self, s: &CompSettings, out: &Output) {
        self.lookahead_ms = s.lookahead_ms;
        self.oversampling = s.oversampling;
        self.apply_oversampling(s);
        self.dry.reset();
        self.dry.set_delay(self.latency());
        self.mix.reset(out.mix);
        self.delta.reset(if out.delta { 1.0 } else { 0.0 });
        self.bypass.reset(if out.bypass { 1.0 } else { 0.0 });
        self.gain.reset(db_to_gain(out.gain_db));
        self.pan.reset(out.pan);
        self.dip.reset(1.0);
        self.slice_pos = 0;
        self.acc = empty_record();
    }

    /// The latency in force, samples (it follows a lookahead or oversampling change once the
    /// dip has faded out): the lookahead plus the oversampler's round trip.
    pub fn latency(&self) -> usize {
        self.comp.latency() / self.comp.multiple() + self.os.latency()
    }

    /// `s` with the lookahead and oversampling in force (a change waits for the dip).
    fn active(&self, s: &CompSettings) -> CompSettings {
        CompSettings { lookahead_ms: self.lookahead_ms, oversampling: self.oversampling, ..*s }
    }

    /// Runs the compressor and the resamplers at the oversampling in force, from rest.
    fn apply_oversampling(&mut self, s: &CompSettings) {
        let active = self.active(s);
        self.os.set_factor(self.oversampling);
        self.os_key.set_factor(self.oversampling);
        self.comp.retune(self.os.factor(), &active);
    }

    /// Processes one host block in place and pushes the history records it completes.
    #[allow(clippy::needless_range_loop)] // channels and scratch buffers indexed together
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
            // A lookahead that keeps the latency (it is constant while lookahead is on) is taken
            // at once: only the detector's key moves, gliding. Switching it on or off, or the
            // oversampling, moves the latency: fade out, switch once silent, fade back in.
            let rate = self.sample_rate;
            let latency_of = |ms: f32| CompSettings { lookahead_ms: ms, ..CompSettings::default() }.latency(rate);
            if s.lookahead_ms != self.lookahead_ms && s.oversampling == self.oversampling && latency_of(s.lookahead_ms) == latency_of(self.lookahead_ms) {
                self.lookahead_ms = s.lookahead_ms;
            }
            if s.lookahead_ms != self.lookahead_ms || s.oversampling != self.oversampling {
                self.dip.set_target(0.0);
                if self.dip.current() == 0.0 {
                    self.lookahead_ms = s.lookahead_ms;
                    if s.oversampling != self.oversampling {
                        self.oversampling = s.oversampling;
                        self.apply_oversampling(s);
                    }
                    self.dry.set_delay(self.latency());
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
            let factor = self.os.factor();
            let telemetry = if factor == 1 {
                let [a, b] = &mut self.scratch;
                let mut parts: [&mut [f32]; 2] = [&mut a[..n], &mut b[..n]];
                self.comp.process(&mut parts[..n_ch], key_chunk.as_ref().map(|k| &k[..]), &active)
            } else {
                // Up, compress at the high rate, down.
                let m = n * factor;
                for c in 0..n_ch {
                    self.os.up(c, &self.scratch[c][..n], &mut self.hi[c][..m]);
                }
                if let Some(k) = key_chunk {
                    for (c, ch) in k.iter().enumerate() {
                        self.os_key.up(c, ch, &mut self.hi_key[c][..m]);
                    }
                }
                let telemetry = {
                    let [a, b] = &mut self.hi;
                    let mut parts: [&mut [f32]; 2] = [&mut a[..m], &mut b[..m]];
                    let [ka, kb] = &self.hi_key;
                    let key_hi: [&[f32]; 2] = [&ka[..m], &kb[..m]];
                    self.comp.process(&mut parts[..n_ch], key_chunk.is_some().then_some(&key_hi[..]), &active)
                };
                for c in 0..n_ch {
                    self.os.down(c, &self.hi[c][..m], &mut self.scratch[c][..n]);
                }
                telemetry
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
        a[record::STAGE..record::STAGE + STAGES].copy_from_slice(&t.stage_db);
        a[record::DETECTOR] = a[record::DETECTOR].max(t.detector_db);
        a[record::DET_RMS] = a[record::DET_RMS].max(t.detector_levels_db[1]);
        a[record::DET_LOUD] = a[record::DET_LOUD].max(t.detector_levels_db[2]);
        a[record::OFFSET] = t.smart_offset_db;
    }
}

fn empty_record() -> [f32; record::WIDTH] {
    let mut r = [0.0; record::WIDTH];
    for i in [record::IN, record::IN + 1, record::OUT, record::OUT + 1, record::DETECTOR, record::DET_RMS, record::DET_LOUD] {
        r[i] = MINUS_INFINITY_DB;
    }
    r
}

#[cfg(test)]
mod tests {
    use super::*;
    use mite_dsp::dynamics::{Detector, Quadrant, Stage};
    use mite_dsp::oversample::Oversampler;

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
        // Switched on: the longest lookahead (20 ms), whatever it is set to.
        assert_eq!(e.latency(), 960);
        // Turned while on: the latency stays, taken at once (no dip).
        s.lookahead_ms = 12.0;
        let mut y = input.clone();
        let tap = HistoryTap::new(64);
        e.process(&mut [&mut y[..480]], None, &s, &Output::default(), &tap);
        assert_eq!(e.latency(), 960);
        assert!(y[..480].iter().any(|v| v.abs() > 1e-4), "no dip");
    }

    /// A sum of tones well inside the band (oversampling passes it as a pure delay).
    fn tones(n: usize, fs: f32) -> Vec<f32> {
        (0..n).map(|i| [110.0f32, 1_234.0, 7_000.0, 15_500.0].iter().map(|f| (std::f32::consts::TAU * f * i as f32 / fs).sin() * 0.2).sum()).collect()
    }

    #[test]
    fn oversampling_with_nothing_to_do_is_a_pure_delay_of_its_latency() {
        for (choice, factor) in [(2, 2), (4, 4)] {
            let s = CompSettings { oversampling: choice, ..CompSettings::default() };
            let mut e = CompEngine::new(FS, &s);
            assert_eq!(e.latency(), Oversampler::latency_for(factor));
            let input = tones(9600, FS);
            let y = run(&mut e, &input, &s, &Output::default(), 333);
            let lat = e.latency();
            let worst = (2000..input.len()).map(|i| (y[i] - input[i - lat]).abs()).fold(0.0f32, f32::max);
            assert!(worst < 2e-4, "{factor}×: {worst}");
        }
    }

    /// The level (dB re the 5 kHz fundamental) of what folds back from above Nyquist when a fast
    /// compressor rides a 5 kHz tone at 44.1 kHz: its gain moves at the tone's harmonics, and the
    /// products above Nyquist land at |5 kHz · k − 44.1 kHz · m|.
    fn aliasing(factor: usize) -> f64 {
        let fs = 44_100.0f32;
        let mut s = CompSettings { oversampling: factor, ..CompSettings::default() };
        s.stages[0] = Stage { on: true, threshold_db: -30.0, ratio: 4.0, knee_db: 0.0, attack_ms: 0.05, release_ms: 10.0, auto_release: false, detector: Detector::Peak, ..Stage::default() };
        let mut e = CompEngine::new(fs, &s);
        let n = 44_100;
        let input: Vec<f32> = (0..n).map(|i| ((std::f64::consts::TAU * 5_000.0 * i as f64 / f64::from(fs)).sin() * 0.5) as f32).collect();
        let tap = HistoryTap::new(1024);
        let mut y = input.clone();
        for chunk in y.chunks_mut(512) {
            e.process(&mut [chunk], None, &s, &Output::default(), &tap);
        }
        if let Ok(dir) = std::env::var("MITE_ALIAS_DUMP") {
            let bytes: Vec<u8> = y.iter().flat_map(|v| v.to_le_bytes()).collect();
            std::fs::write(format!("{dir}/alias{factor}.f32"), bytes).expect("dump");
        }
        let tail = &y[n / 2..];
        let m = tail.len() as f64;
        let level = |f: f64| {
            let w = std::f64::consts::TAU * f / f64::from(fs);
            let (re, im) = tail.iter().enumerate().fold((0.0, 0.0), |(re, im), (i, &v)| {
                let t = std::f64::consts::TAU * i as f64 / m;
                let win = 0.35875 - 0.48829 * t.cos() + 0.14128 * (2.0 * t).cos() - 0.01168 * (3.0 * t).cos();
                (re + win * f64::from(v) * (w * i as f64).cos(), im + win * f64::from(v) * (w * i as f64).sin())
            });
            re * re + im * im
        };
        let fundamental = level(5_000.0);
        let folded: f64 = (5..=16)
            .map(|k| {
                let f = 5_000.0 * k as f64 % 44_100.0;
                if f > 22_050.0 { 44_100.0 - f } else { f }
            })
            .filter(|f| *f > 100.0 && (f / 5_000.0 - (f / 5_000.0).round()).abs() > 0.01)
            .map(level)
            .sum();
        10.0 * (folded / fundamental).log10()
    }

    #[test]
    fn oversampling_folds_back_less() {
        let (off, x2, x4) = (aliasing(1), aliasing(2), aliasing(4));
        assert!(x2 < off - 6.0, "2×: {x2:.1} dB against {off:.1} dB");
        assert!(x4 < x2 - 3.0, "4×: {x4:.1} dB against 2×'s {x2:.1} dB");
    }

    #[test]
    fn an_oversampling_change_moves_the_latency_after_a_dip_and_does_not_click() {
        let mut s = compressing();
        let mut e = CompEngine::new(FS, &s);
        let input = tones(9600, FS);
        run(&mut e, &input, &s, &Output::default(), 480);
        assert_eq!(e.latency(), 0);
        s.oversampling = 4;
        let y = run(&mut e, &input, &s, &Output::default(), 480);
        assert_eq!(e.latency(), Oversampler::latency_for(4));
        // The dip: the output never jumps more than the tones themselves move per sample.
        let step = y.windows(2).map(|w| (w[1] - w[0]).abs()).fold(0.0f32, f32::max);
        let natural = input.windows(2).map(|w| (w[1] - w[0]).abs()).fold(0.0f32, f32::max);
        assert!(step <= natural * 1.05, "{step} against {natural}");
    }
}
