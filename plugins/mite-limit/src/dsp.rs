//! Mite Limit's engine (docs/design/limit-brief.md §1). In order: the DC filter, input gain, the
//! transient protection (three bands, at the base rate), up to the engine rate, the clipper,
//! the limiter (`mite_dsp::limit`), down again and the guard; then unity-gain audition, Delta and
//! bypass against the latency-aligned dry signal, the platform preview and dither. Around it the
//! history telemetry (one record per 5 ms slice, whatever the host's block size) and the
//! loudness of the output (`mite_dsp::loudness`).
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
use mite_dsp::dither::{Dither, Shaping, BITS};
use mite_dsp::filter::{Coeffs, Tdf2};
use mite_dsp::limit::{self, ClipCurve, Limiter, Protect};
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
    /// What the clipper took, dB (≤ 0; the deepest in the slice, either channel).
    pub const CLIP: usize = 10;
    pub const WIDTH: usize = 11;
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
/// The platform preview's glide, ms (the integrated loudness it follows moves slowly).
const PREVIEW_MS: f32 = 300.0;
/// Unity-gain audition's glide, ms, and how far it may turn the output, dB.
const UNITY_MS: f32 = 2000.0;
const UNITY_RANGE_DB: (f32, f32) = (-40.0, 12.0);
/// Under this (LUFS) a side counts as silent and the unity trim holds.
const UNITY_FLOOR_LUFS: f32 = -70.0;
/// The DC filter: second-order high-pass, Hz.
pub const DC_HZ: f64 = 5.0;
/// The oversampling factors, in the parameter's order.
pub const FACTORS: [usize; 5] = [1, 2, 4, 8, 16];

/// A loudness target: a streaming platform's normalisation or a broadcast delivery spec
/// (brief §3.3; values as published in 2026, reviewed yearly).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Platform {
    pub name: &'static str,
    /// The integrated loudness it plays at, LUFS.
    pub lufs: f32,
    /// The highest true peak it accepts, dBTP.
    pub true_peak: Option<f32>,
    /// Whether it turns a quieter master up (only as far as its true-peak limit allows).
    pub turns_up: bool,
    /// How far off its target counts as on it, LU.
    pub tolerance: f32,
}

/// The targets, in `params::TARGETS`' frozen order (the last, Custom, takes the custom dial).
pub const PLATFORMS: [Platform; 9] = [
    Platform { name: "Spotify", lufs: -14.0, true_peak: Some(-1.0), turns_up: true, tolerance: 1.0 },
    Platform { name: "Apple Music", lufs: -16.0, true_peak: Some(-1.0), turns_up: true, tolerance: 1.0 },
    Platform { name: "YouTube", lufs: -14.0, true_peak: None, turns_up: false, tolerance: 1.0 },
    Platform { name: "Tidal", lufs: -14.0, true_peak: None, turns_up: false, tolerance: 1.0 },
    Platform { name: "Amazon Music", lufs: -14.0, true_peak: Some(-2.0), turns_up: false, tolerance: 1.0 },
    Platform { name: "Deezer", lufs: -15.0, true_peak: None, turns_up: false, tolerance: 1.0 },
    Platform { name: "EBU R128", lufs: -23.0, true_peak: Some(-1.0), turns_up: true, tolerance: 0.5 },
    Platform { name: "ATSC A/85", lufs: -24.0, true_peak: Some(-2.0), turns_up: true, tolerance: 2.0 },
    Platform { name: "Custom", lufs: -14.0, true_peak: Some(-1.0), turns_up: false, tolerance: 1.0 },
];

/// The target `index` with the custom dial's level for Custom.
pub fn platform(index: usize, custom_lufs: f32) -> Platform {
    let p = PLATFORMS[index.min(PLATFORMS.len() - 1)];
    if index >= PLATFORMS.len() - 1 { Platform { lufs: custom_lufs, ..p } } else { p }
}

/// The gain the platform would play a master at, dB: down to its target when louder; up to it
/// when quieter only if it turns masters up, and then no further than its true-peak limit
/// allows. 0 while nothing has been measured.
pub fn platform_gain_db(p: &Platform, integrated: f32, true_peak: f32) -> f32 {
    if integrated <= SILENCE_LUFS + 1.0 {
        return 0.0;
    }
    let want = p.lufs - integrated;
    if want <= 0.0 {
        return want;
    }
    if !p.turns_up {
        return 0.0;
    }
    let room = p.true_peak.map_or(want, |limit| (limit - true_peak).max(0.0));
    want.min(room)
}

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
    /// The clipper (§1.4): on, its level over the ceiling (dB) and curve.
    pub clip_on: bool,
    pub clip_db: f32,
    pub clip_curve: usize,
    /// Transient protection (§1.5), 0…1.
    pub protect: f32,
    /// Unity-gain audition (monitoring, §1.7).
    pub unity: bool,
    /// Dither: `BITS` index (0: off) and the noise shaping; the DC filter.
    pub dither: usize,
    pub shaping: usize,
    pub dc_filter: bool,
    /// The loudness target (`PLATFORMS`), Custom's level, and the platform preview (monitoring).
    pub target: usize,
    pub target_custom: f32,
    pub preview: bool,
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
            clip_on: false,
            clip_db: 3.0,
            clip_curve: 1,
            protect: 0.0,
            unity: false,
            dither: 0,
            shaping: 0,
            dc_filter: false,
            target: 0,
            target_custom: -14.0,
            preview: false,
        }
    }
}

impl LimitSettings {
    fn core(&self) -> limit::Settings {
        limit::Settings { attack_ms: self.attack_ms, release_ms: self.release_ms, style: self.style, slow: self.slow, link_transients: self.link_transients, link_release: self.link_release }
    }

    /// What needs the limiter reconfigured (behind a dip): the lookahead, the detector, the
    /// oversampling, and whether the protection runs (it adds its lookahead to the latency).
    fn structure(&self) -> (f32, bool, usize, bool) {
        (self.lookahead_ms, self.true_peak, self.oversampling, self.protect > 0.0)
    }
}

/// The true-peak interpolation at each factor: to at least 8× the base rate below 4× (at 4× the
/// base rate, a tone near fs/4 can fall between the points and pass the ceiling by up to 0.17 dB
/// on a 32× reference; measured 0.151 dB at 2× with protection, 2026-11-09, 0.058 dB at 8×, for
/// 0.7 % of a core more; `examples/truepeak`), 4× and over at 8× the base rate.
fn phases(factor: usize) -> usize {
    if factor <= 2 { 4 } else { 2 }
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

/// The protection stage's lookahead, base-rate samples.
fn protect_samples(ms: f32, sample_rate: f32) -> usize {
    lookahead_samples(ms, sample_rate, 1, false)
}

/// The latency (base-rate samples) of `s` at `sample_rate`.
pub fn latency_for(s: &LimitSettings, sample_rate: f32) -> usize {
    let f = s.oversampling;
    let l = lookahead_samples(s.lookahead_ms, sample_rate, f, s.true_peak);
    let guard = if f > 1 { Limiter::latency_for(guard_samples(sample_rate), s.true_peak) } else { 0 };
    let protect = if s.protect > 0.0 { Protect::latency_for(protect_samples(s.lookahead_ms, sample_rate)) } else { 0 };
    protect + Limiter::latency_for(l, s.true_peak) / f + Oversampler::latency_for(f) + guard
}

/// The DC filter's sections (Butterworth, as the crossovers' high-pass).
fn dc_coeffs(sample_rate: f32) -> Coeffs {
    mite_dsp::crossover::Cross::new(DC_HZ, f64::from(sample_rate)).hp
}

/// Loudness and peaks of the output, for the meters (audio → UI), and the reset (UI → audio).
pub struct LoudnessTap {
    pub momentary: AtomicF32,
    pub short_term: AtomicF32,
    pub integrated: AtomicF32,
    pub range: AtomicF32,
    /// The highest true peak out since the last reset, dBTP.
    pub true_peak_max: AtomicF32,
    /// The gain the chosen platform would play this master at, dB (what the preview plays).
    pub platform_gain: AtomicF32,
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
            platform_gain: AtomicF32::new(0.0),
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
    /// The platform preview's gain (1 while off).
    preview: LinearSmoother,
    /// The protection stage (base rate) and whether it runs (the structure in force), its amount.
    protect: Protect,
    protecting: bool,
    protect_amount: LinearSmoother,
    /// The clipper: in (0…1, a fade), its level over the ceiling (linear).
    clip_mix: LinearSmoother,
    clip_level: LinearSmoother,
    /// What the clipper took at each base-rate sample (1 − its gain: 0 = nothing; a delay line
    /// starts at 0), and its delay to the output.
    clip_now: Vec<f32>,
    clip_delay: DryDelay,
    /// Unity-gain audition: its trim, and the loudness of the dry input and of the wet output.
    unity: LinearSmoother,
    unity_in: Loudness,
    unity_out: Loudness,
    unity_db: f32,
    /// The DC filter: its fade and per-channel state.
    dc_mix: LinearSmoother,
    dc: [Tdf2; 2],
    dc_coeffs: Coeffs,
    dither: Dither,
    /// The structure switch: fades out, reconfigures, fades back in.
    dip: LinearSmoother,
    /// Samples left before a dip may fade back in (the new delay lines filling).
    refill: usize,
    active: LimitSettings,
    meter: [TruePeak; 2],
    /// The input's true peak (after the gain), for the history.
    in_meter: [TruePeak; 2],
    loudness: Loudness,
    tp_max: f32,
    /// The master's true peak over the slice so far (linear).
    slice_tp: f32,
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
            preview: LinearSmoother::new(1.0),
            protect: Protect::new(sample_rate, 2),
            protecting: false,
            protect_amount: LinearSmoother::new(0.0),
            clip_mix: LinearSmoother::new(0.0),
            clip_level: LinearSmoother::new(1.0),
            clip_now: Vec::new(),
            clip_delay: DryDelay::new(1, 1),
            unity: LinearSmoother::new(1.0),
            unity_in: Loudness::new(f64::from(sample_rate)),
            unity_out: Loudness::new(f64::from(sample_rate)),
            unity_db: 0.0,
            dc_mix: LinearSmoother::new(0.0),
            dc: [Tdf2::default(); 2],
            dc_coeffs: dc_coeffs(sample_rate),
            dither: Dither::new(),
            dip: LinearSmoother::new_s_curve(1.0),
            refill: 0,
            active: *s,
            meter: [TruePeak::new(4), TruePeak::new(4)],
            in_meter: [TruePeak::new(4), TruePeak::new(4)],
            loudness: Loudness::new(f64::from(sample_rate)),
            tp_max: MINUS_INFINITY_DB,
            slice_tp: 0.0,
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
        let max_latency = 2 * base + 2 * (TruePeak::DELAY + 2) + guard_samples(sample_rate) + Oversampler::latency_for(MAX_FACTOR);
        self.dry = DryDelay::new(max_latency, CHUNK);
        self.scale = DryDelay::new(max_latency, CHUNK);
        self.clip_delay = DryDelay::new(max_latency, CHUNK);
        self.clip_now = vec![0.0; CHUNK];
        self.protect = Protect::new(sample_rate, base + 1);
        self.unity_in = Loudness::new(f64::from(sample_rate));
        self.unity_out = Loudness::new(f64::from(sample_rate));
        self.dc_coeffs = dc_coeffs(sample_rate);
        self.scratch = [vec![0.0; CHUNK], vec![0.0; CHUNK]];
        self.scale_now = [vec![0.0; CHUNK], vec![0.0; CHUNK]];
        self.reduction = [vec![0.0; CHUNK], vec![0.0; CHUNK]];
        self.hi = [vec![0.0; CHUNK * MAX_FACTOR], vec![0.0; CHUNK * MAX_FACTOR]];
        self.loudness = Loudness::new(f64::from(sample_rate));
        self.slice_len = ((HISTORY_SLICE_MS * 0.001 * sample_rate).round() as usize).max(1);
        for sm in [&mut self.delta, &mut self.bypass, &mut self.dip, &mut self.clip_mix, &mut self.dc_mix] {
            sm.set_ramp(sample_rate, SWITCH_MS);
        }
        for sm in [&mut self.gain, &mut self.ceiling, &mut self.clip_level, &mut self.protect_amount] {
            sm.set_ramp(sample_rate, GLIDE_MS);
        }
        self.preview.set_ramp(sample_rate, PREVIEW_MS);
        self.unity.set_ramp(sample_rate, UNITY_MS);
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
        self.preview.reset(1.0);
        self.dip.reset(1.0);
        self.refill = 0;
        self.protect_amount.reset(s.protect);
        self.clip_mix.reset(if s.clip_on { 1.0 } else { 0.0 });
        self.clip_level.reset(db_to_gain(s.clip_db));
        self.clip_delay.reset();
        self.unity.reset(1.0);
        self.unity_in.reset();
        self.unity_out.reset();
        self.unity_db = 0.0;
        self.dc_mix.reset(if s.dc_filter { 1.0 } else { 0.0 });
        self.dc = [Tdf2::default(); 2];
        self.dither = Dither::new();
        for m in self.meter.iter_mut().chain(&mut self.in_meter) {
            m.reset();
        }
        self.loudness.reset();
        self.tp_max = MINUS_INFINITY_DB;
        self.slice_tp = 0.0;
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
        self.protecting = s.protect > 0.0;
        self.protect.configure(protect_samples(s.lookahead_ms, self.sample_rate), s.core());
        let latency = self.latency();
        self.dry.set_delay(latency);
        self.scale.set_delay(latency);
        // The clipper acts after the protection: its record waits for the rest.
        let protect = if self.protecting { self.protect.latency() } else { 0 };
        self.clip_delay.set_delay(latency - protect);
    }

    /// The latency in force, base-rate samples (a structure change takes effect behind the dip).
    pub fn latency(&self) -> usize {
        let f = self.os.factor();
        let guard = if f > 1 { self.guard.latency() } else { 0 };
        let protect = if self.protecting { self.protect.latency() } else { 0 };
        protect + self.limiters[Self::index(f)].latency() / f + self.os.latency() + guard
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
        self.clip_mix.set_target(if s.clip_on { 1.0 } else { 0.0 });
        self.clip_level.set_target(db_to_gain(s.clip_db));
        self.dc_mix.set_target(if s.dc_filter { 1.0 } else { 0.0 });
        self.dither.set(BITS[s.dither.min(BITS.len() - 1)], Shaping::from_index(s.shaping));
        let curve = ClipCurve::from_index(s.clip_curve);
        // Unity: the output turned to the dry input's loudness (short-term; momentary for the
        // first 3 s), held while either side is silent. Its meters run only while it is on (from
        // the start each time: the first trim lands 400 ms after it is switched on).
        if !s.unity && self.active.unity {
            self.unity_in.reset();
            self.unity_out.reset();
        }
        let loud = |l: &Loudness| if l.short_term() > SILENCE_LUFS + 1.0 { l.short_term() } else { l.momentary() };
        let (li, lo) = (loud(&self.unity_in), loud(&self.unity_out));
        if li > UNITY_FLOOR_LUFS && lo > UNITY_FLOOR_LUFS {
            self.unity_db = (li - lo).clamp(UNITY_RANGE_DB.0, UNITY_RANGE_DB.1);
        }
        self.unity.set_target(if s.unity { db_to_gain(self.unity_db) } else { 1.0 });
        if tap.reset.swap(false, Ordering::Relaxed) {
            self.loudness.reset_integration();
            self.tp_max = MINUS_INFINITY_DB;
            self.slice_tp = 0.0;
        }
        // What the platform does, from the master as measured so far (before the preview).
        let platform_db = platform_gain_db(&platform(s.target, s.target_custom), self.loudness.integrated(), self.tp_max);
        self.preview.set_target(if s.preview { db_to_gain(platform_db) } else { 1.0 });

        let mut start = 0;
        while start < len {
            // A lookahead, detector or oversampling change: fade out, switch once silent, back in.
            if s.structure() != (self.active.lookahead_ms, self.active.true_peak, self.active.oversampling, self.protecting) {
                self.dip.set_target(0.0);
                if self.dip.current() == 0.0 {
                    self.active = *s;
                    self.configure();
                    // The limiter starts with empty delay lines: stay silent until they hold a
                    // latency of real audio, then fade in (fading in at once, the sound jumped
                    // back in a latency later, half way up the fade: the §10 ramp test).
                    self.refill = self.latency();
                }
            }
            if self.refill == 0 && self.dip.target() == 0.0 && s.structure() == (self.active.lookahead_ms, self.active.true_peak, self.active.oversampling, self.protecting) {
                self.dip.set_target(1.0);
            }
            self.active = LimitSettings { lookahead_ms: self.active.lookahead_ms, true_peak: self.active.true_peak, oversampling: self.active.oversampling, ..*s };
            let f = self.os.factor();
            let idx = Self::index(f);
            self.limiters[idx].set(s.core());
            self.protect.set(s.core());
            self.protect_amount.set_target(if self.protecting { s.protect } else { 0.0 });

            let n = (len - start).min(CHUNK).min(self.slice_len - self.slice_pos);
            let end = start + n;
            // The DC filter, then gain in, ceiling out: the limiter works at a ceiling of 1.
            let dc_wanted = s.dc_filter;
            if !dc_wanted && self.dc_mix.current() == 0.0 {
                self.dc = [Tdf2::default(); 2];
            }
            for i in 0..n {
                let (g, c, dc) = (self.gain.next_value(), self.ceiling.next_value(), self.dc_mix.next_value());
                self.scale_now[0][i] = c;
                self.scale_now[1][i] = g;
                for ch in 0..n_ch {
                    let x = channels[ch][start + i];
                    // The filter runs only while it is on or fading (it starts from rest).
                    let x = if dc > 0.0 || dc_wanted {
                        let hp = self.dc[ch].process(&self.dc_coeffs, f64::from(x)) as f32;
                        x + (hp - x) * dc
                    } else {
                        x
                    };
                    self.scratch[ch][i] = x * g / c;
                }
            }
            self.dry.write(&channels[0][start..end], stereo.then(|| &channels[1][start..end]));
            self.scale.write(&self.scale_now[0][..n], Some(&self.scale_now[1][..n]));
            // Transient protection, at the base rate.
            if self.protecting {
                for i in 0..n {
                    let amount = self.protect_amount.next_value();
                    let y = self.protect.process([self.scratch[0][i], if stereo { self.scratch[1][i] } else { 0.0 }], stereo, amount);
                    for ch in 0..n_ch {
                        self.scratch[ch][i] = y[ch];
                    }
                }
            }

            let m = n * f;
            for ch in 0..n_ch {
                if f == 1 {
                    self.hi[ch][..n].copy_from_slice(&self.scratch[ch][..n]);
                } else {
                    self.os.up(ch, &self.scratch[ch][..n], &mut self.hi[ch][..m]);
                }
            }
            // The clipper, at the engine rate.
            for i in 0..n {
                let (mix, level) = (self.clip_mix.next_value(), self.clip_level.next_value());
                let mut took = 1.0f32;
                if mix > 0.0 {
                    for ch in 0..n_ch {
                        for v in &mut self.hi[ch][i * f..(i + 1) * f] {
                            let y = limit::clip(*v, level, curve);
                            let y = *v + (y - *v) * mix;
                            if v.abs() > 1e-9 {
                                took = took.min(y.abs() / v.abs());
                            }
                            *v = y;
                        }
                    }
                }
                self.clip_now[i] = 1.0 - took;
            }
            self.clip_delay.write(&self.clip_now[..n], None);
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

            let dither = self.dither.is_on() && !s.bypass;
            for i in 0..n {
                let (delta, bypass, dip, preview, unity) = (self.delta.next_value(), self.bypass.next_value(), self.dip.next_value(), self.preview.next_value(), self.unity.next_value());
                let (c, g) = (self.scale.read(0, i), self.scale.read(1, i));
                let mut out = [0.0f32; 2];
                let mut wet_pair = [0.0f32; 2];
                let mut dry_pair = [0.0f32; 2];
                for ch in 0..n_ch {
                    // The ceiling the sample was judged against; the clamp only removes rounding.
                    let wet = (self.scratch[ch][i] * c).clamp(-c, c) * dip;
                    let dry = self.dry.read(ch, i);
                    wet_pair[ch] = wet;
                    dry_pair[ch] = dry;
                    // The input as the limiter judged it, at the output's time (so a peak and the
                    // gain it got land in the same record).
                    let x = dry * g;
                    let peak = if self.active.true_peak { self.in_meter[ch].push(x).max(x.abs()) } else { x.abs() };
                    self.acc[record::IN + ch] = self.acc[record::IN + ch].max(gain_to_db(peak));
                    // The master, as rendered: what the column measures.
                    let master = wet * (1.0 - delta) + (x * dip - wet) * delta;
                    out[ch] = master * (1.0 - bypass) + dry * bypass;
                    // What is heard: unity turns only the limited sound (so bypass compares at
                    // equal loudness), then the preview. The meters and history show this.
                    let y = wet * unity * (1.0 - delta) + (x * dip - wet) * delta;
                    let y = (y * (1.0 - bypass) + dry * bypass) * preview;
                    let y = if dither { self.dither.process(ch, y) } else { y };
                    channels[ch][start + i] = y;
                    let a = &mut self.acc;
                    a[record::OUT + ch] = a[record::OUT + ch].max(gain_to_db(y.abs()));
                    a[record::GR + ch] = a[record::GR + ch].min(gain_to_db(self.reduction[ch][i]));
                    // The master's true peak (for the column); the record shows it as heard.
                    let tp = self.meter[ch].push(out[ch]);
                    self.slice_tp = self.slice_tp.max(tp);
                    a[record::TP + ch] = a[record::TP + ch].max(gain_to_db(tp * preview * (unity + (1.0 - unity) * bypass)));
                }
                if !stereo {
                    out[1] = out[0];
                }
                self.loudness.push(out[0], stereo.then_some(out[1]));
                if s.unity {
                    self.unity_in.push(dry_pair[0], stereo.then_some(dry_pair[1]));
                    self.unity_out.push(wet_pair[0], stereo.then_some(wet_pair[1]));
                }
                self.acc[record::CEILING] = gain_to_db(c);
                self.acc[record::CLIP] = self.acc[record::CLIP].min(gain_to_db(1.0 - self.clip_delay.read(0, i)));
            }
            if !stereo {
                for k in [record::IN, record::OUT, record::GR, record::TP] {
                    self.acc[k + 1] = self.acc[k];
                }
            }
            self.dry.advance(n);
            self.scale.advance(n);
            self.clip_delay.advance(n);
            self.slice_pos += n;
            if self.slice_pos >= self.slice_len {
                self.tp_max = self.tp_max.max(gain_to_db(self.slice_tp));
                self.slice_tp = 0.0;
                history.push(&self.acc);
                self.acc = empty_record();
                self.slice_pos = 0;
            }
            self.refill = self.refill.saturating_sub(n);
            start = end;
        }
        tap.momentary.store(self.loudness.momentary());
        tap.short_term.store(self.loudness.short_term());
        tap.integrated.store(self.loudness.integrated());
        tap.range.store(self.loudness.range());
        tap.true_peak_max.store(self.tp_max);
        tap.platform_gain.store(platform_db);
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
    fn what_each_platform_does() {
        let spotify = platform(0, -14.0);
        // Louder: down to the target, either way.
        assert!((platform_gain_db(&spotify, -8.0, -1.0) + 6.0).abs() < 1e-6);
        assert!((platform_gain_db(&PLATFORMS[2], -8.0, -1.0) + 6.0).abs() < 1e-6);
        // Quieter: YouTube leaves it; Spotify turns it up, but only to its −1 dBTP.
        assert_eq!(platform_gain_db(&PLATFORMS[2], -20.0, -6.0), 0.0);
        assert!((platform_gain_db(&spotify, -20.0, -4.0) - 3.0).abs() < 1e-6);
        assert!((platform_gain_db(&spotify, -20.0, -10.0) - 6.0).abs() < 1e-6);
        // Custom takes the dial; nothing measured yet, nothing done.
        assert!((platform_gain_db(&platform(8, -9.0), -8.0, -1.0) + 1.0).abs() < 1e-6);
        assert_eq!(platform_gain_db(&spotify, SILENCE_LUFS, MINUS_INFINITY_DB), 0.0);
        assert_eq!(PLATFORMS.len(), crate::params::TARGETS.len());
        for (p, name) in PLATFORMS.iter().zip(crate::params::TARGETS) {
            assert_eq!(p.name, name);
        }
    }

    #[test]
    fn the_preview_plays_the_master_as_the_platform_will() {
        // A loud master (pushed 12 dB) previewed as Spotify comes out at about −14 LUFS, while
        // the column still reads the master's own loudness.
        let s = LimitSettings { gain_db: 12.0, oversampling: 1, preview: true, ..LimitSettings::default() };
        let mut e = LimitEngine::new(48000.0, &s);
        let history: HistoryTap<{ record::WIDTH }> = HistoryTap::new(8192);
        let tap = LoudnessTap::new();
        let x = material(48000 * 12, -6.0);
        let (mut l, mut r) = (x.clone(), x.clone());
        for (a, b) in l.chunks_mut(512).zip(r.chunks_mut(512)) {
            e.process(&mut [a, b], &s, &history, &tap);
        }
        let master = tap.integrated.load();
        assert!(master > -12.0, "the master is loud: {master}");
        assert!((tap.platform_gain.load() - (-14.0 - master)).abs() < 0.01);
        let mut heard = Loudness::new(48000.0);
        let tail = 48000 * 8;
        heard.process(&l[tail..], Some(&r[tail..]));
        assert!((heard.integrated() + 14.0).abs() < 0.5, "heard at {}", heard.integrated());
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

    #[test]
    fn protection_adds_its_lookahead_to_the_latency_and_the_ceiling_holds() {
        let fs = 48000.0;
        for f in [1, 2, 8] {
            let s = LimitSettings { oversampling: f, protect: 0.7, ceiling_db: 0.0, ..LimitSettings::default() };
            let mut e = LimitEngine::new(fs, &s);
            assert_eq!(e.latency(), latency_for(&s, fs));
            assert_eq!(e.latency(), latency_for(&LimitSettings { protect: 0.0, ..s }, fs) + protect_samples(s.lookahead_ms, fs) - 1);
            // An impulse comes out at the latency, through the bands' allpass (which has no one
            // delay: it lags the lows; measured 2026-11-09, its peak lands a sample late).
            let mut l = vec![0.0f32; 4096];
            l[100] = 0.5;
            let mut r = l.clone();
            run(&mut e, &s, &mut l, &mut r, 256);
            let peak = l.iter().enumerate().fold((0, 0.0f32), |m, (i, v)| if v.abs() > m.1 { (i, v.abs()) } else { m });
            assert!((100 + e.latency()..=102 + e.latency()).contains(&peak.0), "{f}×: {} for {}", peak.0, 100 + e.latency());
            // Pushed hard: still under the ceiling, in sample and true peak.
            let s = LimitSettings { gain_db: 15.0, ceiling_db: -1.0, ..s };
            let mut e = LimitEngine::new(fs, &s);
            let x = material(48000, -6.0);
            let (mut l, mut r) = (x.clone(), x.iter().map(|v| -v * 0.7).collect::<Vec<_>>());
            run(&mut e, &s, &mut l, &mut r, 480);
            let c = db_to_gain(-1.0);
            assert!(l.iter().chain(&r).all(|v| v.abs() <= c), "{f}×: over the ceiling");
            let tp = gain_to_db(mite_dsp::loudness::true_peak_of(&l, 4));
            assert!(tp < -1.0 + 0.02, "{f}×: {tp} dBTP");
        }
    }

    #[test]
    fn turning_protection_on_dips_and_reports_the_longer_latency() {
        let fs = 48000.0;
        let s = LimitSettings::default();
        let mut e = LimitEngine::new(fs, &s);
        let on = LimitSettings { protect: 0.5, ..s };
        let x = material(9600, -12.0);
        let (mut l, mut r) = (x.clone(), x.clone());
        run(&mut e, &on, &mut l, &mut r, 128);
        assert_eq!(e.latency(), latency_for(&on, fs));
        assert!(e.latency() > latency_for(&s, fs));
    }

    #[test]
    fn the_clipper_takes_the_sharpest_peaks_so_the_limiter_does_less() {
        let fs = 48000.0;
        let history = |s: &LimitSettings| {
            let mut e = LimitEngine::new(fs, s);
            let h: HistoryTap<{ record::WIDTH }> = HistoryTap::new(4096);
            let tap = LoudnessTap::new();
            let x = material(48000, -6.0);
            let (mut l, mut r) = (x.clone(), x.clone());
            for (a, b) in l.chunks_mut(256).zip(r.chunks_mut(256)) {
                e.process(&mut [a, b], s, &h, &tap);
            }
            let (mut seen, mut gr, mut clip) = (0, 0.0f32, 0.0f32);
            h.read_since(&mut seen, |rec| {
                gr = gr.min(rec[record::GR]);
                clip = clip.min(rec[record::CLIP]);
            });
            let c = db_to_gain(s.ceiling_db);
            assert!(l.iter().all(|v| v.abs() <= c), "{s:?}: over the ceiling");
            (gr, clip)
        };
        let base = LimitSettings { gain_db: 12.0, oversampling: 2, ..LimitSettings::default() };
        let (gr0, clip0) = history(&base);
        assert_eq!(clip0, 0.0, "off: nothing clipped");
        for curve in 0..3 {
            let (gr, clip) = history(&LimitSettings { clip_on: true, clip_db: 2.0, clip_curve: curve, ..base });
            assert!(clip < -1.0, "curve {curve}: the clipper took {clip} dB");
            assert!(gr > gr0 + 1.0, "curve {curve}: the limiter took {gr} dB, alone {gr0}");
        }
        // At 0 dB over the ceiling (Hard, 1×, sample peaks) the clipper does it all. Above 1× the
        // guard still acts: a hard clip at the high rate rings through the down filter (1.7 dB
        // of guard on this material at 2×, measured 2026-11-09).
        let (gr, _) = history(&LimitSettings { clip_on: true, clip_db: 0.0, clip_curve: 0, true_peak: false, oversampling: 1, ..base });
        assert!(gr > -0.01, "{gr}");
    }

    #[test]
    fn unity_plays_the_output_at_the_inputs_loudness() {
        let fs = 48000.0;
        let s = LimitSettings { gain_db: 12.0, oversampling: 1, unity: true, ..LimitSettings::default() };
        let mut e = LimitEngine::new(fs, &s);
        let x = material(48000 * 10, -12.0);
        let (mut l, mut r) = (x.clone(), x.clone());
        run(&mut e, &s, &mut l, &mut r, 512);
        let lufs = |v: &[f32]| {
            let mut m = Loudness::new(48000.0);
            m.process(v, Some(v));
            m.integrated()
        };
        let tail = 48000 * 6;
        let (heard, input) = (lufs(&l[tail..]), lufs(&x[tail..]));
        assert!((heard - input).abs() < 0.5, "heard {heard}, input {input}");
        // The loudness column still reads the master, not the trimmed monitoring.
        let s_off = LimitSettings { unity: false, ..s };
        let master = |s: &LimitSettings| {
            let mut e = LimitEngine::new(fs, s);
            let h: HistoryTap<{ record::WIDTH }> = HistoryTap::new(64);
            let tap = LoudnessTap::new();
            let (mut l, mut r) = (x.clone(), x.clone());
            for (a, b) in l.chunks_mut(512).zip(r.chunks_mut(512)) {
                e.process(&mut [a, b], s, &h, &tap);
            }
            tap.integrated.load()
        };
        assert!((master(&s) - master(&s_off)).abs() < 0.01, "{} vs {}", master(&s), master(&s_off));
        // Without it, 9 dB or so louder.
        let s = LimitSettings { unity: false, ..s };
        let mut e = LimitEngine::new(fs, &s);
        let (mut l, mut r) = (x.clone(), x.clone());
        run(&mut e, &s, &mut l, &mut r, 512);
        assert!(lufs(&l[tail..]) > input + 6.0);
    }

    #[test]
    fn dither_lands_on_the_word_length_and_bypass_stays_the_input() {
        let fs = 48000.0;
        for (dither, bits) in [(1, 24), (2, 20), (3, 16)] {
            for shaping in 0..3 {
                let s = LimitSettings { gain_db: 6.0, oversampling: 1, dither, shaping, ..LimitSettings::default() };
                let mut e = LimitEngine::new(fs, &s);
                let x = material(9600, -12.0);
                let (mut l, mut r) = (x.clone(), x.clone());
                run(&mut e, &s, &mut l, &mut r, 300);
                let q = mite_dsp::dither::Dither::lsb(bits);
                assert!(l.iter().chain(&r).all(|v| (v / q).round() * q == *v), "{bits} bit, shaping {shaping}");
            }
        }
        let s = LimitSettings { oversampling: 1, dither: 3, shaping: 2, bypass: true, ..LimitSettings::default() };
        let mut e = LimitEngine::new(fs, &s);
        let x = material(9600, -12.0);
        let (mut l, mut r) = (x.clone(), x.clone());
        run(&mut e, &s, &mut l, &mut r, 300);
        let d = e.latency();
        for i in d..x.len() {
            assert_eq!(l[i], x[i - d]);
        }
    }

    #[test]
    fn the_dc_filter_takes_out_an_offset() {
        let fs = 48000.0;
        let s = LimitSettings { oversampling: 1, ceiling_db: 0.0, dc_filter: true, ..LimitSettings::default() };
        let mut e = LimitEngine::new(fs, &s);
        let x: Vec<f32> = (0..48000 * 3).map(|i| 0.2 + 0.3 * (i as f32 * 0.05).sin()).collect();
        let (mut l, mut r) = (x.clone(), x.clone());
        run(&mut e, &s, &mut l, &mut r, 512);
        let tail = &l[48000 * 2..];
        let mean = tail.iter().sum::<f32>() / tail.len() as f32;
        assert!(mean.abs() < 1e-3, "{mean}");
        // Off: bit for bit (the neutral test covers it), and on it leaves a 1 kHz tone alone.
        let amp = tail.iter().fold(0.0f32, |m, v| m.max(v.abs()));
        assert!((amp - 0.3).abs() < 0.01, "{amp}");
    }
}
