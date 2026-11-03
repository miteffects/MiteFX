//! Mite Limit's engine (docs/design/limit-brief.md §1): input gain, the limiter
//! (`mite_dsp::limit`) at the oversampled rate, the output stage (Delta and bypass against the
//! latency-aligned dry signal), the history telemetry (one record per 5 ms slice, whatever the
//! host's block size) and the loudness of the output (`mite_dsp::loudness`).
//!
//! **The ceiling is exact while it glides.** The limiter works with its ceiling at 1: the input
//! is scaled by `gain / ceiling` and the output by the ceiling *of the moment the sample was
//! judged* (a delay line of the ceiling, as long as the latency). So the limiter's guarantee is
//! against the ceiling that sample was limited to, and a ceiling change reaches the output with
//! the audio rather than ahead of it. The last clamp is to that same value: on sample peaks it
//! only ever removes rounding.
//!
//! **The guard.** Above 1× the limiter holds the ceiling at the high rate, but the down filter
//! then removes what the limiting put above the base Nyquist, and that lifts the peaks (by up to
//! 0.2 dB on bright music, measured 2026-11-03). A second limiter on the same law, at the base
//! rate with a 0.5 ms lookahead and a fast release, catches it after the down filter; its delay is
//! part of the latency. It only ever acts by fractions of a dB.
//!
//! **Latency is a whole number of base-rate samples.** The lookahead at the engine rate is the
//! base-rate lookahead times the factor, plus a pad so that it and the true-peak detector's delay
//! add up to a multiple of the factor (a pad only lengthens the ramp, which the proof allows).

use std::sync::atomic::{AtomicBool, Ordering};

use mite_core::{AtomicF32, HistoryTap};
use mite_dsp::limit::{self, Limiter};
use mite_dsp::loudness::{Loudness, TruePeak, SILENCE_LUFS};
use mite_dsp::output::DryDelay;
use mite_dsp::oversample::{Oversampler, MAX_FACTOR};
use mite_dsp::{db_to_gain, gain_to_db, LinearSmoother, MINUS_INFINITY_DB};

/// Fields of a history record (`LimitShared::history`).
pub mod record {
    /// Peak in (after the gain), L and R, dBFS: the true peak while True Peak is on, so what is
    /// drawn crossing the ceiling is what the limiter acts on.
    pub const IN: usize = 0;
    /// Peak out, L and R, dBFS (sample peak).
    pub const OUT: usize = 2;
    /// The deepest gain reduction, L and R, dB (≤ 0).
    pub const GR: usize = 4;
    /// True peak out, L and R, dBTP.
    pub const TP: usize = 6;
    /// The ceiling, dB.
    pub const CEILING: usize = 8;
    pub const WIDTH: usize = 10;
}

/// One history record per this much audio.
pub const HISTORY_SLICE_MS: f32 = 5.0;
/// Records kept: 30 s.
pub const HISTORY_RECORDS: usize = 6000;
/// The longest lookahead, ms (`params::LOOKAHEAD`'s top).
pub const MAX_LOOKAHEAD_MS: f32 = 10.0;
/// Longest chunk the engine processes at once.
const CHUNK: usize = 512;
/// Switch fades (delta, bypass, the latency dip) and glides (gain, ceiling), ms.
const SWITCH_MS: f32 = 10.0;
const GLIDE_MS: f32 = 20.0;
/// The guard's lookahead, ms, and release.
const GUARD_MS: f32 = 0.5;
const GUARD_RELEASE_MS: f32 = 20.0;
/// The oversampling factors, in the parameter's order.
pub const FACTORS: [usize; 5] = [1, 2, 4, 8, 16];

/// What the parameters set.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LimitSettings {
    pub gain_db: f32,
    pub ceiling_db: f32,
    pub style: usize,
    pub lookahead_ms: f32,
    pub attack_ms: f32,
    pub release_ms: f32,
    /// Custom's slow share, 0…1.
    pub slow: f32,
    pub link_transients: f32,
    pub link_release: f32,
    pub true_peak: bool,
    /// 1, 2, 4, 8 or 16.
    pub oversampling: usize,
    pub delta: bool,
    pub bypass: bool,
}

impl Default for LimitSettings {
    fn default() -> Self {
        Self {
            gain_db: 0.0,
            ceiling_db: -1.0,
            style: 0,
            lookahead_ms: 2.0,
            attack_ms: 30.0,
            release_ms: 100.0,
            slow: 0.5,
            link_transients: 1.0,
            link_release: 1.0,
            true_peak: true,
            oversampling: 2,
            delta: false,
            bypass: false,
        }
    }
}

impl LimitSettings {
    fn core(&self) -> limit::Settings {
        limit::Settings { attack_ms: self.attack_ms, release_ms: self.release_ms, style: self.style, slow: self.slow, link_transients: self.link_transients, link_release: self.link_release }
    }

    /// What needs the limiter reconfigured (behind a dip): the lookahead, the detector and the
    /// oversampling.
    fn structure(&self) -> (f32, bool, usize) {
        (self.lookahead_ms, self.true_peak, self.oversampling)
    }
}

/// The true-peak interpolation at each factor: to at least 4× the base rate.
fn phases(factor: usize) -> usize {
    if factor == 1 { 4 } else { 2 }
}

/// The lookahead in engine samples (`L`, 1 = none) for `ms` at `factor`, with the pad that
/// makes the engine latency a multiple of the factor.
pub fn lookahead_samples(ms: f32, sample_rate: f32, factor: usize, true_peak: bool) -> usize {
    let base = (ms.max(0.0) * 0.001 * sample_rate).round() as usize;
    let tp = if true_peak { TruePeak::DELAY + 1 } else { 0 };
    let pad = (factor - tp % factor) % factor;
    factor * base + pad + 1
}

/// The guard's lookahead, base-rate samples (no guard at 1×).
fn guard_samples(sample_rate: f32) -> usize {
    lookahead_samples(GUARD_MS, sample_rate, 1, false)
}

fn guard_settings() -> limit::Settings {
    limit::Settings { attack_ms: 1.0, release_ms: GUARD_RELEASE_MS, style: 0, slow: 0.0, link_transients: 1.0, link_release: 1.0 }
}

/// The latency (base-rate samples) of `s` at `sample_rate`.
pub fn latency_for(s: &LimitSettings, sample_rate: f32) -> usize {
    let f = s.oversampling;
    let l = lookahead_samples(s.lookahead_ms, sample_rate, f, s.true_peak);
    let guard = if f > 1 { Limiter::latency_for(guard_samples(sample_rate), s.true_peak) } else { 0 };
    Limiter::latency_for(l, s.true_peak) / f + Oversampler::latency_for(f) + guard
}

/// Loudness and peaks of the output, for the meters (audio → UI), and the reset (UI → audio).
pub struct LoudnessTap {
    pub momentary: AtomicF32,
    pub short_term: AtomicF32,
    pub integrated: AtomicF32,
    pub range: AtomicF32,
    /// The highest true peak out since the last reset, dBTP.
    pub true_peak_max: AtomicF32,
    /// Set by the editor: restart the integrated loudness, the range and the peak.
    pub reset: AtomicBool,
}

impl LoudnessTap {
    pub fn new() -> Self {
        Self {
            momentary: AtomicF32::new(SILENCE_LUFS),
            short_term: AtomicF32::new(SILENCE_LUFS),
            integrated: AtomicF32::new(SILENCE_LUFS),
            range: AtomicF32::new(0.0),
            true_peak_max: AtomicF32::new(MINUS_INFINITY_DB),
            reset: AtomicBool::new(false),
        }
    }
}

impl Default for LoudnessTap {
    fn default() -> Self {
        Self::new()
    }
}

pub struct LimitEngine {
    sample_rate: f32,
    /// One limiter per factor (each sized for its rate), so a factor change never allocates.
    limiters: Vec<Limiter>,
    /// After the down filter, above 1×.
    guard: Limiter,
    os: Oversampler,
    /// The scaled input at the base rate, then the output; and the high-rate buffers.
    scratch: [Vec<f32>; 2],
    hi: [Vec<f32>; 2],
    /// The gain reduction each base-rate sample got (the deepest of its high-rate ones), dB.
    reduction: [Vec<f32>; 2],
    /// The input, and the (ceiling, gain) each sample was judged with, delayed by the latency.
    dry: DryDelay,
    scale: DryDelay,
    scale_now: [Vec<f32>; 2],
    gain: LinearSmoother,
    ceiling: LinearSmoother,
    delta: LinearSmoother,
    bypass: LinearSmoother,
    /// The structure switch: fades out, reconfigures, fades back in.
    dip: LinearSmoother,
    active: LimitSettings,
    meter: [TruePeak; 2],
    /// The input's true peak (after the gain), for the history.
    in_meter: [TruePeak; 2],
    loudness: Loudness,
    tp_max: f32,
    slice_len: usize,
    slice_pos: usize,
    acc: [f32; record::WIDTH],
}

impl LimitEngine {
    pub fn new(sample_rate: f32, s: &LimitSettings) -> Self {
        let mut e = Self {
            sample_rate,
            limiters: Vec::new(),
            guard: Limiter::new(sample_rate, 2, 4),
            os: Oversampler::new(1),
            scratch: [Vec::new(), Vec::new()],
            hi: [Vec::new(), Vec::new()],
            reduction: [Vec::new(), Vec::new()],
            dry: DryDelay::new(1, 1),
            scale: DryDelay::new(1, 1),
            scale_now: [Vec::new(), Vec::new()],
            gain: LinearSmoother::new(1.0),
            ceiling: LinearSmoother::new(1.0),
            delta: LinearSmoother::new(0.0),
            bypass: LinearSmoother::new(0.0),
            dip: LinearSmoother::new(1.0),
            active: *s,
            meter: [TruePeak::new(4), TruePeak::new(4)],
            in_meter: [TruePeak::new(4), TruePeak::new(4)],
            loudness: Loudness::new(f64::from(sample_rate)),
            tp_max: MINUS_INFINITY_DB,
            slice_len: 1,
            slice_pos: 0,
            acc: empty_record(),
        };
        e.prepare(sample_rate, s);
        e
    }

    /// Sizes everything for `sample_rate`. Allocates.
    pub fn prepare(&mut self, sample_rate: f32, s: &LimitSettings) {
        self.sample_rate = sample_rate;
        let base = (MAX_LOOKAHEAD_MS * 0.001 * sample_rate).ceil() as usize + 1;
        self.limiters = FACTORS.iter().map(|&f| Limiter::new(sample_rate * f as f32, f * base + f + 1, phases(f))).collect();
        self.guard = Limiter::new(sample_rate, guard_samples(sample_rate) + 1, 4);
        let max_latency = base + 2 * (TruePeak::DELAY + 2) + guard_samples(sample_rate) + Oversampler::latency_for(MAX_FACTOR);
        self.dry = DryDelay::new(max_latency, CHUNK);
        self.scale = DryDelay::new(max_latency, CHUNK);
        self.scratch = [vec![0.0; CHUNK], vec![0.0; CHUNK]];
        self.scale_now = [vec![0.0; CHUNK], vec![0.0; CHUNK]];
        self.reduction = [vec![0.0; CHUNK], vec![0.0; CHUNK]];
        self.hi = [vec![0.0; CHUNK * MAX_FACTOR], vec![0.0; CHUNK * MAX_FACTOR]];
        self.loudness = Loudness::new(f64::from(sample_rate));
        self.slice_len = ((HISTORY_SLICE_MS * 0.001 * sample_rate).round() as usize).max(1);
        for sm in [&mut self.delta, &mut self.bypass, &mut self.dip] {
            sm.set_ramp(sample_rate, SWITCH_MS);
        }
        for sm in [&mut self.gain, &mut self.ceiling] {
            sm.set_ramp(sample_rate, GLIDE_MS);
        }
        self.reset(s);
    }

    /// Clears the state and snaps every glide to `s`.
    pub fn reset(&mut self, s: &LimitSettings) {
        self.active = *s;
        self.configure();
        self.dry.reset();
        self.scale.reset();
        self.gain.reset(db_to_gain(s.gain_db));
        self.ceiling.reset(db_to_gain(s.ceiling_db));
        self.delta.reset(if s.delta { 1.0 } else { 0.0 });
        self.bypass.reset(if s.bypass { 1.0 } else { 0.0 });
        self.dip.reset(1.0);
        for m in self.meter.iter_mut().chain(&mut self.in_meter) {
            m.reset();
        }
        self.loudness.reset();
        self.tp_max = MINUS_INFINITY_DB;
        self.slice_pos = 0;
        self.acc = empty_record();
    }

    fn index(factor: usize) -> usize {
        FACTORS.iter().position(|&f| f == factor).unwrap_or(1)
    }

    /// Sets the limiter, the oversampler and the delays to the active structure, from rest.
    fn configure(&mut self) {
        let s = self.active;
        let f = FACTORS[Self::index(s.oversampling)];
        self.os.set_factor(f);
        let l = lookahead_samples(s.lookahead_ms, self.sample_rate, f, s.true_peak);
        self.limiters[Self::index(f)].configure(l, s.true_peak, s.core());
        self.guard.configure(guard_samples(self.sample_rate), s.true_peak, guard_settings());
        let latency = self.latency();
        self.dry.set_delay(latency);
        self.scale.set_delay(latency);
    }

    /// The latency in force, base-rate samples (a structure change takes effect behind the dip).
    pub fn latency(&self) -> usize {
        let f = self.os.factor();
        let guard = if f > 1 { self.guard.latency() } else { 0 };
        self.limiters[Self::index(f)].latency() / f + self.os.latency() + guard
    }

    /// Processes one host block in place and pushes the history records it completes.
    #[allow(clippy::needless_range_loop)] // channels and scratch buffers indexed together
    pub fn process(&mut self, channels: &mut [&mut [f32]], s: &LimitSettings, history: &HistoryTap<{ record::WIDTH }>, tap: &LoudnessTap) {
        let n_ch = channels.len().min(2);
        if n_ch == 0 {
            return;
        }
        let stereo = n_ch == 2;
        let len = channels.iter().map(|c| c.len()).min().unwrap_or(0);
        self.gain.set_target(db_to_gain(s.gain_db));
        self.ceiling.set_target(db_to_gain(s.ceiling_db));
        self.delta.set_target(if s.delta { 1.0 } else { 0.0 });
        self.bypass.set_target(if s.bypass { 1.0 } else { 0.0 });
        if tap.reset.swap(false, Ordering::Relaxed) {
            self.loudness.reset_integration();
            self.tp_max = MINUS_INFINITY_DB;
        }

        let mut start = 0;
        while start < len {
            // A lookahead, detector or oversampling change: fade out, switch once silent, back in.
            if s.structure() != self.active.structure() {
                self.dip.set_target(0.0);
                if self.dip.current() == 0.0 {
                    self.active = *s;
                    self.configure();
                    self.dip.set_target(1.0);
                }
            }
            self.active = LimitSettings { lookahead_ms: self.active.lookahead_ms, true_peak: self.active.true_peak, oversampling: self.active.oversampling, ..*s };
            let f = self.os.factor();
            let idx = Self::index(f);
            self.limiters[idx].set(s.core());

            let n = (len - start).min(CHUNK).min(self.slice_len - self.slice_pos);
            let end = start + n;
            // Gain in, ceiling out: the limiter works at a ceiling of 1.
            for i in 0..n {
                let (g, c) = (self.gain.next_value(), self.ceiling.next_value());
                self.scale_now[0][i] = c;
                self.scale_now[1][i] = g;
                for ch in 0..n_ch {
                    self.scratch[ch][i] = channels[ch][start + i] * g / c;
                }
            }
            self.dry.write(&channels[0][start..end], stereo.then(|| &channels[1][start..end]));
            self.scale.write(&self.scale_now[0][..n], Some(&self.scale_now[1][..n]));

            let m = n * f;
            for ch in 0..n_ch {
                if f == 1 {
                    self.hi[ch][..n].copy_from_slice(&self.scratch[ch][..n]);
                } else {
                    self.os.up(ch, &self.scratch[ch][..n], &mut self.hi[ch][..m]);
                }
            }
            let limiter = &mut self.limiters[idx];
            for i in 0..n {
                let mut deepest = [1.0f32; 2];
                for k in i * f..(i + 1) * f {
                    let (y, g) = limiter.process([self.hi[0][k], if stereo { self.hi[1][k] } else { 0.0 }], stereo);
                    self.hi[0][k] = y[0];
                    if stereo {
                        self.hi[1][k] = y[1];
                    }
                    deepest = [deepest[0].min(g[0]), deepest[1].min(g[1])];
                }
                self.reduction[0][i] = deepest[0];
                self.reduction[1][i] = if stereo { deepest[1] } else { deepest[0] };
            }
            for ch in 0..n_ch {
                if f == 1 {
                    self.scratch[ch][..n].copy_from_slice(&self.hi[ch][..n]);
                } else {
                    self.os.down(ch, &self.hi[ch][..m], &mut self.scratch[ch][..n]);
                }
            }
            if f > 1 {
                for i in 0..n {
                    let (y, g) = self.guard.process([self.scratch[0][i], if stereo { self.scratch[1][i] } else { 0.0 }], stereo);
                    for ch in 0..n_ch {
                        self.scratch[ch][i] = y[ch];
                        self.reduction[ch][i] *= g[ch];
                    }
                }
            }

            for i in 0..n {
                let (delta, bypass, dip) = (self.delta.next_value(), self.bypass.next_value(), self.dip.next_value());
                let (c, g) = (self.scale.read(0, i), self.scale.read(1, i));
                let mut out = [0.0f32; 2];
                for ch in 0..n_ch {
                    // The ceiling the sample was judged against; the clamp only removes rounding.
                    let wet = (self.scratch[ch][i] * c).clamp(-c, c) * dip;
                    let dry = self.dry.read(ch, i);
                    // The input as the limiter judged it, at the output's time (so a peak and the
                    // gain it got land in the same record).
                    let x = dry * g;
                    let peak = if self.active.true_peak { self.in_meter[ch].push(x).max(x.abs()) } else { x.abs() };
                    self.acc[record::IN + ch] = self.acc[record::IN + ch].max(gain_to_db(peak));
                    let y = wet * (1.0 - delta) + (x * dip - wet) * delta;
                    let y = y * (1.0 - bypass) + dry * bypass;
                    channels[ch][start + i] = y;
                    out[ch] = y;
                    let a = &mut self.acc;
                    a[record::OUT + ch] = a[record::OUT + ch].max(gain_to_db(y.abs()));
                    a[record::GR + ch] = a[record::GR + ch].min(gain_to_db(self.reduction[ch][i]));
                    let tp = self.meter[ch].push(y);
                    a[record::TP + ch] = a[record::TP + ch].max(gain_to_db(tp));
                }
                if !stereo {
                    out[1] = out[0];
                }
                self.loudness.push(out[0], stereo.then_some(out[1]));
                self.acc[record::CEILING] = gain_to_db(c);
            }
            if !stereo {
                for k in [record::IN, record::OUT, record::GR, record::TP] {
                    self.acc[k + 1] = self.acc[k];
                }
            }
            self.dry.advance(n);
            self.scale.advance(n);
            self.slice_pos += n;
            if self.slice_pos >= self.slice_len {
                self.tp_max = self.tp_max.max(self.acc[record::TP]).max(self.acc[record::TP + 1]);
                history.push(&self.acc);
                self.acc = empty_record();
                self.slice_pos = 0;
            }
            start = end;
        }
        tap.momentary.store(self.loudness.momentary());
        tap.short_term.store(self.loudness.short_term());
        tap.integrated.store(self.loudness.integrated());
        tap.range.store(self.loudness.range());
        tap.true_peak_max.store(self.tp_max);
    }
}

fn empty_record() -> [f32; record::WIDTH] {
    let mut r = [0.0; record::WIDTH];
    for i in [record::IN, record::IN + 1, record::OUT, record::OUT + 1, record::TP, record::TP + 1] {
        r[i] = MINUS_INFINITY_DB;
    }
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(engine: &mut LimitEngine, s: &LimitSettings, l: &mut [f32], r: &mut [f32], block: usize) {
        let history = HistoryTap::new(64);
        let tap = LoudnessTap::new();
        for (a, b) in l.chunks_mut(block).zip(r.chunks_mut(block)) {
            engine.process(&mut [a, b], s, &history, &tap);
        }
    }

    /// Drum-like material: a kick, a hat and a tone, `db` loud.
    fn material(n: usize, db: f32) -> Vec<f32> {
        let a = db_to_gain(db);
        (0..n)
            .map(|i| {
                let t = (i % 24000) as f32 / 48000.0;
                let kick = (t * 55.0 * std::f32::consts::TAU * (1.0 + 2.0 * (-t * 30.0).exp())).sin() * (-t * 8.0).exp();
                let hat = if i % 6000 < 300 { ((i * 7919) % 1000) as f32 / 500.0 - 1.0 } else { 0.0 } * 0.5;
                let tone = 0.3 * (i as f32 * 0.0731).sin();
                // A bright square (odd harmonics to Nyquist): what the down filter trims.
                let square = if (i / 9) % 2 == 0 { 0.25 } else { -0.25 };
                a * (kick + hat + tone + square)
            })
            .collect()
    }

    #[test]
    fn the_reported_latency_is_whole_and_measured() {
        let fs = 48000.0;
        for f in FACTORS {
            for tp in [false, true] {
                for la in [0.0, 0.7, 2.0, 10.0] {
                    let s = LimitSettings { oversampling: f, true_peak: tp, lookahead_ms: la, ceiling_db: 0.0, ..LimitSettings::default() };
                    let mut e = LimitEngine::new(fs, &s);
                    let lat = e.latency();
                    assert_eq!(lat, latency_for(&s, fs));
                    // An impulse well under the ceiling comes out at the latency.
                    let mut l = vec![0.0f32; 4096];
                    l[100] = 0.5;
                    let mut r = l.clone();
                    run(&mut e, &s, &mut l, &mut r, 256);
                    let peak = l.iter().enumerate().fold((0, 0.0f32), |m, (i, v)| if v.abs() > m.1 { (i, v.abs()) } else { m });
                    assert_eq!(peak.0, 100 + lat, "{f}× tp {tp} {la} ms");
                    assert!((peak.1 - 0.5).abs() < 0.02, "{f}×: {}", peak.1);
                }
            }
        }
    }

    #[test]
    fn under_the_ceiling_at_1x_it_is_the_input_delayed_bit_for_bit() {
        let s = LimitSettings { oversampling: 1, ceiling_db: 0.0, ..LimitSettings::default() };
        let mut e = LimitEngine::new(48000.0, &s);
        let x = material(20000, -12.0);
        let (mut l, mut r) = (x.clone(), x.clone());
        run(&mut e, &s, &mut l, &mut r, 333);
        let d = e.latency();
        for i in d..x.len() {
            assert_eq!(l[i], x[i - d], "sample {i}");
        }
    }

    #[test]
    fn the_output_holds_the_ceiling_in_sample_and_true_peak() {
        let fs = 48000.0;
        for f in FACTORS {
            for style in 0..6 {
                let s = LimitSettings { oversampling: f, style, gain_db: 18.0, ceiling_db: -1.0, lookahead_ms: 1.5, release_ms: 60.0, ..LimitSettings::default() };
                let mut e = LimitEngine::new(fs, &s);
                let x = material(48000, -6.0);
                let (mut l, mut r) = (x.clone(), x.iter().map(|v| -v * 0.8).collect::<Vec<_>>());
                run(&mut e, &s, &mut l, &mut r, 480);
                let c = db_to_gain(-1.0);
                assert!(l.iter().chain(&r).all(|v| v.abs() <= c), "{f}× style {style}: sample peak over");
                let tp = gain_to_db(mite_dsp::loudness::true_peak_of(&l, 4).max(mite_dsp::loudness::true_peak_of(&r, 4)));
                // Measured 2026-11-03: worst 0.015 dB over (2×), inside the brief's 0.05 dB.
                assert!(tp < -1.0 + 0.02, "{f}× style {style}: {tp} dBTP");
            }
        }
    }

    #[test]
    fn where_the_drawn_input_crosses_the_ceiling_the_limiter_takes_at_least_the_excess() {
        // The history's honesty (user, 2026-11-03): its input is what the limiter judges.
        for (f, tp) in [(1, true), (2, true), (4, false)] {
            let s = LimitSettings { oversampling: f, true_peak: tp, gain_db: 9.0, ..LimitSettings::default() };
            let mut e = LimitEngine::new(48000.0, &s);
            let history: HistoryTap<{ record::WIDTH }> = HistoryTap::new(4096);
            let tap = LoudnessTap::new();
            let x = material(48000, -6.0);
            let (mut l, mut r) = (x.clone(), x.clone());
            for (a, b) in l.chunks_mut(256).zip(r.chunks_mut(256)) {
                e.process(&mut [a, b], &s, &history, &tap);
            }
            let mut seen = 0;
            let mut over = 0;
            history.read_since(&mut seen, |rec| {
                let excess = rec[record::IN].max(rec[record::IN + 1]) - rec[record::CEILING];
                if excess > 0.0 {
                    over += 1;
                    let gr = rec[record::GR].min(rec[record::GR + 1]);
                    assert!(gr <= -excess + 0.1, "{f}×: {excess:.2} dB over, GR {gr:.2}");
                }
            });
            assert!(over > 20, "{f}×: the material should cross the ceiling");
        }
    }

    #[test]
    fn delta_is_what_was_taken_and_bypass_is_the_dry() {
        let s = LimitSettings { oversampling: 1, gain_db: 12.0, ..LimitSettings::default() };
        let x = material(24000, -6.0);
        let mut e = LimitEngine::new(48000.0, &s);
        let (mut wet, mut wr) = (x.clone(), x.clone());
        run(&mut e, &s, &mut wet, &mut wr, 256);
        let mut e = LimitEngine::new(48000.0, &LimitSettings { delta: true, ..s });
        let (mut dl, mut dr) = (x.clone(), x.clone());
        run(&mut e, &LimitSettings { delta: true, ..s }, &mut dl, &mut dr, 256);
        let d = e.latency();
        let g = db_to_gain(12.0);
        for i in d..x.len() {
            assert!((wet[i] + dl[i] - x[i - d] * g).abs() < 1e-5, "sample {i}");
        }
        let mut e = LimitEngine::new(48000.0, &LimitSettings { bypass: true, ..s });
        let (mut bl, mut br) = (x.clone(), x.clone());
        run(&mut e, &LimitSettings { bypass: true, ..s }, &mut bl, &mut br, 256);
        for i in d..x.len() {
            assert_eq!(bl[i], x[i - d]);
        }
    }

    #[test]
    fn a_structure_change_dips_and_reports_the_new_latency() {
        let fs = 48000.0;
        let s = LimitSettings::default();
        let mut e = LimitEngine::new(fs, &s);
        let s2 = LimitSettings { oversampling: 8, lookahead_ms: 5.0, ..s };
        let x = material(9600, -12.0);
        let (mut l, mut r) = (x.clone(), x.clone());
        run(&mut e, &s2, &mut l, &mut r, 128);
        assert_eq!(e.latency(), latency_for(&s2, fs));
        assert!(l.iter().all(|v| v.is_finite() && v.abs() <= db_to_gain(-1.0)));
    }
}
