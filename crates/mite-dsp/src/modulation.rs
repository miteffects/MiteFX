//! Modulation effects behind Mite Mod (docs/design/mod-brief.md): chorus, ensemble, flanger
//! (through-zero too), phaser (2…24 stages) and vibrato, on one LFO.
//!
//! Ideas from Nova's NovaChorus, NovaFlanger and NovaPhaser; the code is new. What is new here:
//!
//! - **Clean readings.** Every swept reading uses the doubler's 32-tap windowed sinc
//!   (`doubler::Sinc`: a sliding read adds no roughness, the junk 64 dB under the treble), and
//!   cubic Lagrange only where the sinc cannot reach (a flanger's feedback loop shorter than 15
//!   samples), the two crossfaded over 4 samples so the sweep never steps.
//! - **Smooth shapes.** Every LFO shape is continuous (a ramp falls back along a half cosine, the
//!   square is a soft one, Random glides from value to value along a smoothstep): a delay swept
//!   by a step would click.
//! - **Pure functions for the editor.** The LFO ([`shape_at`]), each mode's reading from it
//!   ([`chorus_delay_ms`], [`flanger_tau_ms`], [`phaser_fc`], [`vibrato_amp_ms`]) and the
//!   flanger's and phaser's responses ([`flanger_response`], [`phaser_response`]) are what the
//!   engine plays and what the editor draws.
//! - **Click-free switching.** Each mode (and the flanger's through-zero, which changes the
//!   latency) is a program with its own state; a switch crossfades the old program's output into
//!   the new one's over 30 ms (both run meanwhile). Every continuous setting glides.
//!
//! Latency: none, except the through-zero flanger (the dry is delayed so the wet can pass it:
//! [`TZ_MAX_MS`] plus the sinc's reach) and vibrato (it plays around [`VIB_CENTER_MS`], so the
//! voice stays in time). Nothing here allocates outside [`Mod::new`].

use crate::doubler::Sinc;
use crate::reverb::Delay;

/// Chorus voices at most.
pub const MAX_VOICES: usize = 4;
/// The phaser's stage counts (a choice; the index is saved state).
pub const STAGE_COUNTS: [usize; 7] = [2, 4, 6, 8, 12, 16, 24];
pub const MAX_STAGES: usize = 24;
/// The flanger's MANUAL range, ms (also the through-zero reach either side).
pub const MIN_MANUAL_MS: f32 = 0.1;
pub const TZ_MAX_MS: f32 = 5.0;
/// Chorus and ensemble base delay range, ms.
pub const MIN_DELAY_MS: f32 = 1.0;
pub const MAX_DELAY_MS: f32 = 40.0;
/// Vibrato plays around this delay, ms (its latency).
pub const VIB_CENTER_MS: f32 = 5.0;
/// Vibrato's depth at 100 %, cents (at the sine's steepest).
pub const VIB_MAX_CENTS: f32 = 100.0;
/// The phaser sweeps ±this many octaves around CENTRE at 100 %.
pub const PHASER_OCTAVES: f32 = 3.0;
/// The flanger sweeps ±this many octaves of delay around MANUAL at 100 % (not through zero).
pub const FLANGER_OCTAVES: f32 = 2.0;
/// The chorus's sweep at 100 %: half the voice's delay, at most this, ms.
pub const CHORUS_MAX_SWEEP_MS: f32 = 6.0;
/// Ensemble: its fast shimmer runs this many times the rate, its slow and fast sweeps at 100 %.
pub const ENSEMBLE_FAST: f32 = 9.5;
pub const ENSEMBLE_SLOW_MS: f32 = 1.6;
pub const ENSEMBLE_FAST_MS: f32 = 0.35;
/// Each chorus voice's place in the LFO cycle and its delay against DELAY.
pub const VOICE_PHASE: [f64; MAX_VOICES] = [0.0, 0.5, 0.25, 0.75];
pub const VOICE_DELAY: [f32; MAX_VOICES] = [1.0, 1.23, 0.81, 1.41];

/// The sinc reaches this many samples either side of a reading.
const SINC_REACH: f64 = 15.0;
/// Under this, Lagrange alone (between, a crossfade).
const LAGRANGE_BELOW: f64 = 11.0;
/// The longest any line is read, ms (chorus: 40 ms × 1.41 + its sweep).
const LINE_MS: f32 = 64.0;
/// Glides: settings, the delays (slower: a fast change of delay is a pitch blip), switching.
const GLIDE_MS: f32 = 20.0;
const DELAY_GLIDE_MS: f32 = 120.0;
const SWITCH_MS: f32 = 30.0;
const SHAPE_MS: f32 = 50.0;
/// The tone's corners glide over this (log frequency).
const TONE_GLIDE_MS: f32 = 80.0;

/// What Mite Mod is. The index is saved state (append only).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    #[default]
    Chorus,
    Ensemble,
    Flanger,
    Phaser,
    Vibrato,
}

impl Mode {
    pub const NAMES: [&'static str; 5] = ["Chorus", "Ensemble", "Flanger", "Phaser", "Vibrato"];
    pub const ALL: [Mode; 5] = [Mode::Chorus, Mode::Ensemble, Mode::Flanger, Mode::Phaser, Mode::Vibrato];

    pub fn from_index(i: usize) -> Self {
        Self::ALL.get(i).copied().unwrap_or_default()
    }

    pub fn index(self) -> usize {
        self as usize
    }
}

/// The LFO's shape. The index is saved state (append only).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Shape {
    #[default]
    Sine,
    Triangle,
    RampUp,
    RampDown,
    Square,
    Random,
}

impl Shape {
    pub const NAMES: [&'static str; 6] = ["Sine", "Triangle", "Ramp Up", "Ramp Down", "Square", "Random"];
    pub const ALL: [Shape; 6] = [Shape::Sine, Shape::Triangle, Shape::RampUp, Shape::RampDown, Shape::Square, Shape::Random];

    pub fn from_index(i: usize) -> Self {
        Self::ALL.get(i).copied().unwrap_or_default()
    }

    pub fn index(self) -> usize {
        self as usize
    }
}

/// Tempo divisions for a synced LFO: a name and one cycle's length in quarter notes. The index
/// is saved state (append only).
pub const DIVISIONS: [(&str, f64); 15] = [
    ("4 bars", 16.0),
    ("2 bars", 8.0),
    ("1 bar", 4.0),
    ("1/2", 2.0),
    ("1/2.", 3.0),
    ("1/2T", 4.0 / 3.0),
    ("1/4", 1.0),
    ("1/4.", 1.5),
    ("1/4T", 2.0 / 3.0),
    ("1/8", 0.5),
    ("1/8.", 0.75),
    ("1/8T", 1.0 / 3.0),
    ("1/16", 0.25),
    ("1/16T", 1.0 / 6.0),
    ("1/32", 0.125),
];

/// The host's transport, as far as the LFO cares.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Transport {
    pub tempo: Option<f64>,
    /// The song position at the block's start, quarter notes (while playing).
    pub beats: Option<f64>,
    pub playing: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ModSettings {
    pub mode: Mode,
    pub rate_hz: f32,
    pub sync: bool,
    pub division: usize,
    pub shape: Shape,
    /// 0…1.
    pub depth: f32,
    /// The right side's LFO this far ahead of the left, 0…1 of half a cycle (1: opposite).
    pub spread: f32,
    /// −0.95…0.95 (flanger and phaser).
    pub feedback: f32,
    /// 0…1 (vibrato is all wet).
    pub mix: f32,
    /// The wet's stereo width, 0…2.
    pub width: f32,
    pub low_cut_hz: f32,
    pub high_cut_hz: f32,
    /// Chorus and ensemble.
    pub delay_ms: f32,
    pub voices: usize,
    /// Flanger.
    pub manual_ms: f32,
    pub thru_zero: bool,
    /// Phaser.
    pub centre_hz: f32,
    pub stages: usize,
}

impl Default for ModSettings {
    fn default() -> Self {
        Self {
            mode: Mode::Chorus,
            rate_hz: 0.6,
            sync: false,
            division: 6,
            shape: Shape::Sine,
            depth: 0.5,
            spread: 1.0,
            feedback: 0.0,
            mix: 0.5,
            width: 1.0,
            low_cut_hz: 20.0,
            high_cut_hz: 20_000.0,
            delay_ms: 12.0,
            voices: 2,
            manual_ms: 1.5,
            thru_zero: false,
            centre_hz: 800.0,
            stages: 4,
        }
    }
}

/// The LFO's rate, Hz (synced to `tempo` when asked and known).
pub fn rate_hz(s: &ModSettings, tempo: Option<f64>) -> f64 {
    match (s.sync, tempo) {
        (true, Some(bpm)) if bpm > 0.0 => bpm / 60.0 / DIVISIONS[s.division.min(DIVISIONS.len() - 1)].1,
        _ => f64::from(s.rate_hz.clamp(0.01, 40.0)),
    }
}

/// A random value per cycle for Random, −1…1, from the cycle's number.
fn cycle_value(k: i64, seed: u32) -> f32 {
    let mut h = (k as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ u64::from(seed).wrapping_mul(0xC2B2_AE3D_27D4_EB4F);
    h ^= h >> 29;
    h = h.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    h ^= h >> 32;
    (h >> 40) as f32 / (1u64 << 24) as f32 * 2.0 - 1.0
}

/// The LFO at total phase `p` (cycles; the fraction is the place in the cycle, the whole part
/// picks Random's values), −1…1. Every shape is continuous.
pub fn shape_at(shape: Shape, p: f64) -> f32 {
    let f = p.rem_euclid(1.0) as f32;
    let tau = std::f32::consts::TAU;
    let ramp = |f: f32| if f < 0.9 { -1.0 + 2.0 * f / 0.9 } else { (std::f32::consts::PI * (f - 0.9) / 0.1).cos() };
    match shape {
        Shape::Sine => (tau * f).sin(),
        Shape::Triangle => {
            if f < 0.25 {
                4.0 * f
            } else if f < 0.75 {
                2.0 - 4.0 * f
            } else {
                4.0 * f - 4.0
            }
        }
        Shape::RampUp => ramp(f),
        Shape::RampDown => -ramp(f),
        Shape::Square => (4.0 * (tau * f).sin()).tanh() / 4f32.tanh(),
        Shape::Random => {
            // A random height each cycle, on alternate sides of the middle (so it never stalls:
            // two near values in a row would hold still for a whole cycle).
            let k = p.floor() as i64;
            let at = |k: i64| (0.25 + 0.75 * cycle_value(k, 0x5EED).abs()) * if k.rem_euclid(2) == 0 { 1.0 } else { -1.0 };
            let (a, b) = (at(k), at(k + 1));
            let t = f * f * (3.0 - 2.0 * f);
            a + (b - a) * t
        }
    }
}

/// A chorus voice's delay, ms, for an LFO value (−1…1).
pub fn chorus_delay_ms(s: &ModSettings, voice: usize, lfo: f32) -> f32 {
    let base = s.delay_ms.clamp(MIN_DELAY_MS, MAX_DELAY_MS) * VOICE_DELAY[voice.min(MAX_VOICES - 1)];
    base + chorus_sweep_ms(s, base) * lfo
}

/// How far a chorus voice at `base` ms sweeps either way, ms.
pub fn chorus_sweep_ms(s: &ModSettings, base: f32) -> f32 {
    s.depth.clamp(0.0, 1.0) * (0.5 * base).min(CHORUS_MAX_SWEEP_MS)
}

/// An ensemble voice's delay, ms, from the slow LFO's value and the fast shimmer's.
pub fn ensemble_delay_ms(s: &ModSettings, slow: f32, fast: f32) -> f32 {
    let d = s.depth.clamp(0.0, 1.0);
    s.delay_ms.clamp(MIN_DELAY_MS, MAX_DELAY_MS).max(2.5) + d * (ENSEMBLE_SLOW_MS * slow + ENSEMBLE_FAST_MS * fast)
}

/// The flanger's delay between the wet and the dry, ms (negative: the wet ahead, through zero).
pub fn flanger_tau_ms(s: &ModSettings, lfo: f32) -> f32 {
    let d = s.depth.clamp(0.0, 1.0);
    if s.thru_zero {
        let m = s.manual_ms.clamp(MIN_MANUAL_MS, TZ_MAX_MS);
        m * (1.0 - d * (lfo + 1.0))
    } else {
        s.manual_ms.clamp(MIN_MANUAL_MS, TZ_MAX_MS) * 2f32.powf(FLANGER_OCTAVES * d * lfo)
    }
}

/// The phaser's corner frequency, Hz, for an LFO value.
pub fn phaser_fc(s: &ModSettings, lfo: f32) -> f32 {
    s.centre_hz.clamp(50.0, 8000.0) * 2f32.powf(PHASER_OCTAVES * s.depth.clamp(0.0, 1.0) * lfo)
}

/// Vibrato's sweep either way, ms, for its depth at `rate` Hz (the sine's steepest makes the
/// depth's cents), at most what the centre leaves.
pub fn vibrato_amp_ms(s: &ModSettings, rate: f64) -> f32 {
    let cents = f64::from(s.depth.clamp(0.0, 1.0) * VIB_MAX_CENTS);
    let a = (2f64.powf(cents / 1200.0) - 1.0) / (std::f64::consts::TAU * rate.max(0.01));
    ((a * 1000.0) as f32).min(VIB_CENTER_MS - 0.5)
}

/// The cents a delay sweeping at `slope` (seconds of delay per second) plays at.
pub fn cents_of_slope(slope: f64) -> f32 {
    (1200.0 * (1.0 - slope).max(1e-6).log2()) as f32
}

/// Samples the through-zero flanger delays the dry at `fs`.
pub fn tz_latency(fs: f32) -> usize {
    SINC_REACH as usize + 1 + (TZ_MAX_MS * 0.001 * fs).ceil() as usize
}

/// Samples vibrato delays the voice at `fs`.
pub fn vibrato_latency(fs: f32) -> usize {
    (VIB_CENTER_MS * 0.001 * fs).round() as usize
}

/// The latency of `s`, samples.
pub fn latency_for(s: &ModSettings, fs: f32) -> usize {
    match s.mode {
        Mode::Flanger if s.thru_zero => tz_latency(fs),
        Mode::Vibrato => vibrato_latency(fs),
        _ => 0,
    }
}

// ---- the tone filters (shared by the engine and the responses) --------------------------------

/// A second-order Butterworth section's coefficients (`a0` = 1).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Biquad {
    pub b: [f64; 3],
    pub a: [f64; 2],
}

impl Biquad {
    pub fn butter(fc: f64, fs: f64, high: bool) -> Self {
        let w = std::f64::consts::TAU * fc.clamp(5.0, 0.49 * fs) / fs;
        let (sin, cos) = w.sin_cos();
        let alpha = sin / std::f64::consts::SQRT_2;
        let a0 = 1.0 + alpha;
        let (b0, b1) = if high { ((1.0 + cos) / 2.0, -(1.0 + cos)) } else { ((1.0 - cos) / 2.0, 1.0 - cos) };
        Self { b: [b0 / a0, b1 / a0, b0 / a0], a: [-2.0 * cos / a0, (1.0 - alpha) / a0] }
    }

    /// The response at `f` Hz.
    pub fn response(&self, f: f64, fs: f64) -> (f64, f64) {
        let w = std::f64::consts::TAU * f / fs;
        let z1 = (w.cos(), -w.sin());
        let z2 = ((2.0 * w).cos(), -(2.0 * w).sin());
        let num = (self.b[0] + self.b[1] * z1.0 + self.b[2] * z2.0, self.b[1] * z1.1 + self.b[2] * z2.1);
        let den = (1.0 + self.a[0] * z1.0 + self.a[1] * z2.0, self.a[0] * z1.1 + self.a[1] * z2.1);
        cdiv(num, den)
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct BiquadState {
    z: [f64; 2],
}

impl BiquadState {
    #[inline]
    fn run(&mut self, c: &Biquad, x: f64) -> f64 {
        let y = c.b[0] * x + self.z[0];
        self.z[0] = c.b[1] * x - c.a[0] * y + self.z[1];
        self.z[1] = c.b[2] * x - c.a[1] * y;
        y
    }
}

fn cmul(a: (f64, f64), b: (f64, f64)) -> (f64, f64) {
    (a.0 * b.0 - a.1 * b.1, a.0 * b.1 + a.1 * b.0)
}

fn cdiv(a: (f64, f64), b: (f64, f64)) -> (f64, f64) {
    let d = (b.0 * b.0 + b.1 * b.1).max(1e-300);
    ((a.0 * b.0 + a.1 * b.1) / d, (a.1 * b.0 - a.0 * b.1) / d)
}

fn cexp(phase: f64) -> (f64, f64) {
    (phase.cos(), phase.sin())
}

/// The wet's tone filters (low cut, high cut), designed once for a curve's many frequencies.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Tone(pub Biquad, pub Biquad);

impl Tone {
    pub fn of(s: &ModSettings, fs: f64) -> Self {
        Self(Biquad::butter(f64::from(s.low_cut_hz), fs, true), Biquad::butter(f64::from(s.high_cut_hz), fs, false))
    }

    fn response(&self, f: f64, fs: f64) -> (f64, f64) {
        cmul(self.0.response(f, fs), self.1.response(f, fs))
    }
}

/// The flanger's response (dry and wet mixed, mono), dB, at `f` Hz with the wet `tau_ms` from
/// the dry.
pub fn flanger_response(s: &ModSettings, tau_ms: f32, f: f64, fs: f64) -> f64 {
    flanger_response_with(s, &Tone::of(s, fs), tau_ms, f, fs)
}

/// [`flanger_response`] with the tone designed already.
pub fn flanger_response_with(s: &ModSettings, tone: &Tone, tau_ms: f32, f: f64, fs: f64) -> f64 {
    let w = std::f64::consts::TAU * f;
    let m = f64::from(s.mix.clamp(0.0, 1.0));
    let fb = f64::from(s.feedback.clamp(-0.95, 0.95));
    // The dry's delay and the feedback loop's, s.
    let lat = if s.thru_zero { tz_latency(fs as f32) as f64 / fs } else { 0.0 };
    let tau = f64::from(tau_ms) * 0.001;
    let lp = if s.thru_zero { lat + tau } else { tau.max(2.0 / fs) };
    let dry = cexp(-w * lat);
    let e = cexp(-w * lp);
    let wet = cdiv(e, (1.0 - fb * e.0, -fb * e.1));
    let wet = cmul(wet, tone.response(f, fs));
    let y = ((1.0 - m) * dry.0 + m * wet.0, (1.0 - m) * dry.1 + m * wet.1);
    10.0 * (y.0 * y.0 + y.1 * y.1).max(1e-12).log10()
}

/// A first-order allpass's coefficient for a corner at `fc`.
#[inline]
pub fn allpass_coef(fc: f64, fs: f64) -> f64 {
    let t = (std::f64::consts::PI * fc.clamp(10.0, 0.45 * fs) / fs).tan();
    (t - 1.0) / (t + 1.0)
}

/// The phaser's response (dry and wet mixed, mono), dB, at `f` Hz with its corners at `fc`.
pub fn phaser_response(s: &ModSettings, fc: f32, f: f64, fs: f64) -> f64 {
    phaser_response_with(s, &Tone::of(s, fs), fc, f, fs)
}

/// [`phaser_response`] with the tone designed already.
pub fn phaser_response_with(s: &ModSettings, tone: &Tone, fc: f32, f: f64, fs: f64) -> f64 {
    let w = std::f64::consts::TAU * f / fs;
    let a = allpass_coef(f64::from(fc), fs);
    let z1 = cexp(-w);
    // One stage: (a + z⁻¹) / (1 + a z⁻¹); all of them, then the loop through one sample.
    let one = cdiv((a + z1.0, z1.1), (1.0 + a * z1.0, a * z1.1));
    // one^n by squaring (24 stages: 6 products, not 24).
    let (mut chain, mut base, mut n) = ((1.0, 0.0), one, s.stages.clamp(1, MAX_STAGES));
    while n > 0 {
        if n & 1 == 1 {
            chain = cmul(chain, base);
        }
        base = cmul(base, base);
        n >>= 1;
    }
    let fb = f64::from(s.feedback.clamp(-0.95, 0.95));
    let loop_ = cmul(chain, z1);
    let wet = cdiv(chain, (1.0 - fb * loop_.0, -fb * loop_.1));
    let wet = cmul(wet, tone.response(f, fs));
    let m = f64::from(s.mix.clamp(0.0, 1.0));
    let y = ((1.0 - m) + m * wet.0, m * wet.1);
    10.0 * (y.0 * y.0 + y.1 * y.1).max(1e-12).log10()
}

// ---- the engine ---------------------------------------------------------------------------------

/// A value gliding to its target, one pole per sample.
#[derive(Debug, Clone, Copy, Default)]
struct Glide {
    v: f32,
}

impl Glide {
    #[inline]
    fn next(&mut self, target: f32, k: f32) -> f32 {
        self.v += (target - self.v) * k;
        self.v
    }
}

const PROGRAMS: usize = 6;

/// The programs: each mode, and the flanger twice (through zero has its own latency and state).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Program {
    Chorus,
    Ensemble,
    Flanger,
    FlangerTz,
    Phaser,
    Vibrato,
}

impl Program {
    fn of(s: &ModSettings) -> Self {
        match s.mode {
            Mode::Chorus => Self::Chorus,
            Mode::Ensemble => Self::Ensemble,
            Mode::Flanger if s.thru_zero => Self::FlangerTz,
            Mode::Flanger => Self::Flanger,
            Mode::Phaser => Self::Phaser,
            Mode::Vibrato => Self::Vibrato,
        }
    }
}

/// One block's readings for the editor (at its end).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct ModTelemetry {
    /// The LFO's total phase (cycles) and rate (Hz).
    pub phase: f64,
    pub rate: f64,
    /// The mode's readings, left then right: chorus, each voice's delay (ms, 4 + 4); ensemble,
    /// each voice's (3 + 3); flanger, the delay between wet and dry (ms); phaser, the corner (Hz);
    /// vibrato, the pitch (cents).
    pub values: [f32; 8],
}

pub struct Mod {
    fs: f32,
    sinc: Sinc,
    /// The input, both sides (chorus, ensemble, vibrato and the through-zero dry read it).
    clean: [Delay; 2],
    /// Each flanger's line (the input and its feedback), plain and through zero.
    flange: [[Delay; 2]; 2],
    flange_wet: [[f32; 2]; 2],
    /// The phaser's allpasses (x₁, y₁ per stage) and its feedback, per side.
    ap: [[[f64; 2]; MAX_STAGES]; 2],
    ap_out: [f64; 2],
    /// The wet's tone, per program and side: low cut, high cut (each program its own, as two
    /// play at once while switching).
    tone: [[[BiquadState; 2]; 2]; PROGRAMS],
    tone_c: [Biquad; 2],
    /// The corners the tone's coefficients were worked out for (again every sample they glide).
    tone_for: (f32, f32),
    phase: f64,
    fast_phase: f64,
    /// Vibrato's last delays (for its pitch), per side.
    vib_last: [f64; 2],
    cur: Program,
    prev: Program,
    fade: f32,
    shape_from: Shape,
    shape_to: Shape,
    shape_fade: f32,
    // Glides.
    depth: Glide,
    spread: Glide,
    feedback: Glide,
    mix: Glide,
    width: Glide,
    delay: Glide,
    manual: Glide,
    centre: Glide,
    vib_amp: Glide,
    low_cut: Glide,
    high_cut: Glide,
    voice_gain: [Glide; MAX_VOICES],
    stage_from: usize,
    stage_to: usize,
    stage_fade: f32,
    k_glide: f32,
    k_delay: f32,
    k_tone: f32,
    switch_step: f32,
    shape_step: f32,
    values: [f32; 8],
}

impl Mod {
    /// Allocates the lines and the sinc table.
    pub fn new(fs: f32, s: &ModSettings) -> Self {
        let line = (LINE_MS * 0.001 * fs) as usize + 64;
        let tz = tz_latency(fs) + (TZ_MAX_MS * 0.001 * fs).ceil() as usize + 64;
        let mk = |n: usize| [Delay::new(n), Delay::new(n)];
        let mut m = Self {
            fs,
            sinc: Sinc::new(),
            clean: mk(line.max(tz)),
            flange: [mk((4.0 * TZ_MAX_MS * 0.001 * fs) as usize + 64), mk(tz)],
            flange_wet: [[0.0; 2]; 2],
            ap: [[[0.0; 2]; MAX_STAGES]; 2],
            ap_out: [0.0; 2],
            tone: [[[BiquadState::default(); 2]; 2]; PROGRAMS],
            tone_c: [Biquad::butter(20.0, f64::from(fs), true), Biquad::butter(20_000.0, f64::from(fs), false)],
            tone_for: (0.0, 0.0),
            phase: 0.0,
            fast_phase: 0.0,
            vib_last: [0.0; 2],
            cur: Program::of(s),
            prev: Program::of(s),
            fade: 1.0,
            shape_from: s.shape,
            shape_to: s.shape,
            shape_fade: 1.0,
            depth: Glide::default(),
            spread: Glide::default(),
            feedback: Glide::default(),
            mix: Glide::default(),
            width: Glide::default(),
            delay: Glide::default(),
            manual: Glide::default(),
            centre: Glide::default(),
            vib_amp: Glide::default(),
            low_cut: Glide::default(),
            high_cut: Glide::default(),
            voice_gain: [Glide::default(); MAX_VOICES],
            stage_from: s.stages,
            stage_to: s.stages,
            stage_fade: 1.0,
            k_glide: 1.0 - (-1.0 / (GLIDE_MS * 0.001 * fs)).exp(),
            k_delay: 1.0 - (-1.0 / (DELAY_GLIDE_MS * 0.001 * fs)).exp(),
            k_tone: 1.0 - (-1.0 / (TONE_GLIDE_MS * 0.001 * fs)).exp(),
            switch_step: 1.0 / (SWITCH_MS * 0.001 * fs),
            shape_step: 1.0 / (SHAPE_MS * 0.001 * fs),
            values: [0.0; 8],
        };
        m.reset(s);
        m
    }

    pub fn sample_rate(&self) -> f32 {
        self.fs
    }

    /// The latency now, samples (the program playing).
    pub fn latency(&self) -> usize {
        match self.cur {
            Program::FlangerTz => tz_latency(self.fs),
            Program::Vibrato => vibrato_latency(self.fs),
            _ => 0,
        }
    }

    pub fn reset(&mut self, s: &ModSettings) {
        for d in self.clean.iter_mut().chain(self.flange.iter_mut().flatten()) {
            d.reset();
        }
        self.flange_wet = [[0.0; 2]; 2];
        self.ap = [[[0.0; 2]; MAX_STAGES]; 2];
        self.ap_out = [0.0; 2];
        self.tone = [[[BiquadState::default(); 2]; 2]; PROGRAMS];
        self.phase = 0.0;
        self.fast_phase = 0.0;
        self.cur = Program::of(s);
        self.prev = self.cur;
        self.fade = 1.0;
        (self.shape_from, self.shape_to, self.shape_fade) = (s.shape, s.shape, 1.0);
        self.depth.v = s.depth;
        self.spread.v = s.spread;
        self.feedback.v = s.feedback;
        self.mix.v = s.mix;
        self.width.v = s.width;
        self.delay.v = s.delay_ms;
        self.manual.v = s.manual_ms;
        self.centre.v = s.centre_hz.max(1.0).log2();
        self.vib_amp.v = vibrato_amp_ms(s, rate_hz(s, None));
        self.low_cut.v = s.low_cut_hz.max(1.0).log2();
        self.high_cut.v = s.high_cut_hz.max(1.0).log2();
        for (v, g) in self.voice_gain.iter_mut().enumerate() {
            g.v = if v < s.voices { 1.0 } else { 0.0 };
        }
        (self.stage_from, self.stage_to, self.stage_fade) = (s.stages, s.stages, 1.0);
        self.vib_last = [f64::from(VIB_CENTER_MS) * 0.001 * f64::from(self.fs); 2];
        self.values = [0.0; 8];
        self.design_tone();
    }

    fn design_tone(&mut self) {
        let fs = f64::from(self.fs);
        self.tone_c = [Biquad::butter(f64::from(self.low_cut.v.exp2()), fs, true), Biquad::butter(f64::from(self.high_cut.v.exp2()), fs, false)];
        self.tone_for = (self.low_cut.v, self.high_cut.v);
    }

    /// The LFO at `p` (the shape crossfading while it changes).
    #[inline]
    fn lfo(&self, p: f64) -> f32 {
        let to = shape_at(self.shape_to, p);
        if self.shape_fade >= 1.0 {
            to
        } else {
            let from = shape_at(self.shape_from, p);
            from + (to - from) * self.shape_fade
        }
    }

    /// A reading `d` samples back on `line`: the sinc where it reaches, Lagrange under it.
    #[inline]
    fn read(sinc: &Sinc, line: &Delay, d: f64) -> f32 {
        if d >= SINC_REACH {
            sinc.read(line, d)
        } else if d <= LAGRANGE_BELOW {
            line.read(d.max(1.0) as f32)
        } else {
            let t = ((d - LAGRANGE_BELOW) / (SINC_REACH - LAGRANGE_BELOW)) as f32;
            let a = line.read(d as f32);
            let b = sinc.read(line, SINC_REACH.max(d));
            a + (b - a) * t
        }
    }

    /// Processes `input` (one side, or two) into `out` (two sides). `stereo_in` false: the left
    /// holds a mono input. Never allocates.
    pub fn process(&mut self, input: [&[f32]; 2], out: [&mut [f32]; 2], stereo_in: bool, s: &ModSettings, transport: &Transport) -> ModTelemetry {
        let [ol, or] = out;
        let len = input[0].len().min(ol.len()).min(or.len()).min(if stereo_in { input[1].len() } else { usize::MAX });
        let fs = self.fs;
        let fsd = f64::from(fs);
        let rate = rate_hz(s, transport.tempo);
        let inc = rate / fsd;
        // A synced LFO locks to the song position, nudged over the block (a relocation is a
        // glide of the phase, never a jump).
        let mut nudge = 0.0;
        if s.sync && transport.playing {
            if let (Some(beats), Some(_)) = (transport.beats, transport.tempo) {
                let want = beats / DIVISIONS[s.division.min(DIVISIONS.len() - 1)].1;
                let err = (want - self.phase + 0.5).rem_euclid(1.0) - 0.5;
                nudge = 0.1 * err / len.max(1) as f64;
            }
        }
        let want = Program::of(s);
        if want != self.cur && self.fade >= 1.0 {
            self.prev = self.cur;
            self.cur = want;
            self.fade = 0.0;
            self.start(want);
        }
        if s.shape != self.shape_to && self.shape_fade >= 1.0 {
            self.shape_from = self.shape_to;
            self.shape_to = s.shape;
            self.shape_fade = 0.0;
        }
        let stages = s.stages.clamp(1, MAX_STAGES);
        if stages != self.stage_to && self.stage_fade >= 1.0 {
            self.stage_from = self.stage_to;
            self.stage_to = stages;
            self.stage_fade = 0.0;
        }
        let vib_target = vibrato_amp_ms(s, rate);
        let (k, kd) = (self.k_glide, self.k_delay);
        let ms = 0.001 * fsd;
        for i in 0..len {
            let xl = input[0][i];
            let xr = if stereo_in { input[1][i] } else { xl };
            self.clean[0].write(xl);
            self.clean[1].write(xr);
            let depth = self.depth.next(s.depth, k);
            let spread = self.spread.next(s.spread, k);
            let feedback = self.feedback.next(s.feedback.clamp(-0.95, 0.95), k);
            let mix = self.mix.next(s.mix.clamp(0.0, 1.0), k);
            let width = self.width.next(s.width.clamp(0.0, 2.0), k);
            let delay = self.delay.next(s.delay_ms.clamp(MIN_DELAY_MS, MAX_DELAY_MS), kd);
            let manual = self.manual.next(s.manual_ms.clamp(MIN_MANUAL_MS, TZ_MAX_MS), kd);
            let centre = self.centre.next(s.centre_hz.clamp(50.0, 8000.0).log2(), kd).exp2();
            let vib_amp = self.vib_amp.next(vib_target, kd);
            self.low_cut.next(s.low_cut_hz.clamp(10.0, 20_000.0).log2(), self.k_tone);
            self.high_cut.next(s.high_cut_hz.clamp(200.0, 22_000.0).log2(), self.k_tone);
            if (self.low_cut.v, self.high_cut.v) != self.tone_for {
                self.design_tone();
            }
            for (v, g) in self.voice_gain.iter_mut().enumerate() {
                g.next(if v < s.voices.clamp(1, MAX_VOICES) { 1.0 } else { 0.0 }, k);
            }
            // The settings as they play this sample (glided), for the readings' functions.
            let now = ModSettings { depth, spread, feedback, mix, width, delay_ms: delay, manual_ms: manual, centre_hz: centre, ..*s };
            let offsets = [0.0, 0.5 * f64::from(spread)];
            let lfo = [self.lfo(self.phase), self.lfo(self.phase + offsets[1])];
            let mut y = self.play(self.cur, &now, lfo, offsets, vib_amp, ms, [xl, xr]);
            if self.fade < 1.0 {
                let old = self.play(self.prev, &now, lfo, offsets, vib_amp, ms, [xl, xr]);
                let t = self.fade;
                y = [old[0] + (y[0] - old[0]) * t, old[1] + (y[1] - old[1]) * t];
                self.fade = (self.fade + self.switch_step).min(1.0);
            }
            ol[i] = y[0];
            or[i] = y[1];
            self.phase += inc + nudge;
            if self.phase >= 1_048_576.0 {
                self.phase -= 1_048_576.0;
            }
            self.fast_phase = (self.fast_phase + inc * f64::from(ENSEMBLE_FAST)).fract();
            if self.shape_fade < 1.0 {
                self.shape_fade = (self.shape_fade + self.shape_step).min(1.0);
            }
            if self.stage_fade < 1.0 {
                self.stage_fade = (self.stage_fade + self.switch_step).min(1.0);
            }
        }
        ModTelemetry { phase: self.phase, rate, values: self.values }
    }

    /// A program comes in: from rest.
    fn start(&mut self, p: Program) {
        match p {
            Program::Flanger | Program::FlangerTz => {
                let k = usize::from(p == Program::FlangerTz);
                for d in &mut self.flange[k] {
                    d.reset();
                }
                self.flange_wet[k] = [0.0; 2];
            }
            Program::Phaser => {
                self.ap = [[[0.0; 2]; MAX_STAGES]; 2];
                self.ap_out = [0.0; 2];
            }
            _ => {}
        }
        self.tone[p as usize] = [[BiquadState::default(); 2]; 2];
    }

    /// The wet through the tone, then its width.
    #[inline]
    fn finish(&mut self, p: Program, wet: [f32; 2], width: f32) -> [f32; 2] {
        let mut w = [0.0f32; 2];
        for c in 0..2 {
            let [hp, lp] = &mut self.tone[p as usize][c];
            w[c] = lp.run(&self.tone_c[1], hp.run(&self.tone_c[0], f64::from(wet[c]))) as f32;
        }
        let mid = 0.5 * (w[0] + w[1]);
        let side = 0.5 * (w[0] - w[1]) * width;
        [mid + side, mid - side]
    }

    /// One sample of program `p`: the output, both sides.
    #[allow(clippy::too_many_arguments)]
    #[inline]
    fn play(&mut self, p: Program, s: &ModSettings, lfo: [f32; 2], offsets: [f64; 2], vib_amp: f32, ms: f64, x: [f32; 2]) -> [f32; 2] {
        let mix = s.mix;
        match p {
            Program::Chorus => {
                let mut wet = [0.0f32; 2];
                let mut power = 0.0f32;
                for (v, phase) in VOICE_PHASE.iter().enumerate() {
                    let g = self.voice_gain[v].v;
                    if g < 1e-4 {
                        continue;
                    }
                    power += g * g;
                    for c in 0..2 {
                        let value = self.lfo(self.phase + offsets[c] + phase);
                        let d = chorus_delay_ms(s, v, value);
                        wet[c] += g * Self::read(&self.sinc, &self.clean[c], f64::from(d) * ms);
                        self.values[v + 4 * c] = d;
                    }
                }
                let norm = 1.0 / power.max(1e-6).sqrt();
                let w = self.finish(p, [wet[0] * norm, wet[1] * norm], s.width);
                [x[0] * (1.0 - mix) + w[0] * mix, x[1] * (1.0 - mix) + w[1] * mix]
            }
            Program::Ensemble => {
                let mut wet = [0.0f32; 2];
                for v in 0..3 {
                    let third = v as f64 / 3.0;
                    for c in 0..2 {
                        let slow = self.lfo(self.phase + offsets[c] + third);
                        let fast = (std::f64::consts::TAU * (self.fast_phase + offsets[c] + third)).sin() as f32;
                        let d = ensemble_delay_ms(s, slow, fast);
                        wet[c] += Self::read(&self.sinc, &self.clean[c], f64::from(d) * ms);
                        self.values[v + 3 * c] = d;
                    }
                }
                let n = 1.0 / 3f32.sqrt();
                let w = self.finish(p, [wet[0] * n, wet[1] * n], s.width);
                [x[0] * (1.0 - mix) + w[0] * mix, x[1] * (1.0 - mix) + w[1] * mix]
            }
            Program::Flanger | Program::FlangerTz => {
                let tz = p == Program::FlangerTz;
                let k = usize::from(tz);
                let lat = if tz { tz_latency(self.fs) } else { 0 };
                let mut wet = [0.0f32; 2];
                let mut dry = x;
                for c in 0..2 {
                    let tau = flanger_tau_ms(&ModSettings { thru_zero: tz, ..*s }, lfo[c]);
                    self.values[c] = tau;
                    // Its delay from the input, samples (the loop's), at least 2.
                    let d = (lat as f64 + f64::from(tau) * ms).max(2.0);
                    // Read before this sample is written: d samples ago is d − 1 back.
                    let w = Self::read(&self.sinc, &self.flange[k][c], d - 1.0);
                    self.flange[k][c].write(x[c] + s.feedback * w);
                    wet[c] = w;
                    if tz {
                        dry[c] = self.clean[c].tap(lat);
                    }
                }
                self.flange_wet[k] = wet;
                let w = self.finish(p, wet, s.width);
                [dry[0] * (1.0 - mix) + w[0] * mix, dry[1] * (1.0 - mix) + w[1] * mix]
            }
            Program::Phaser => {
                let fsd = f64::from(self.fs);
                let mut wet = [0.0f32; 2];
                let (from, to, t) = (self.stage_from, self.stage_to, f64::from(self.stage_fade));
                let n = from.max(to);
                for c in 0..2 {
                    let fc = phaser_fc(s, lfo[c]);
                    self.values[c] = fc;
                    let a = allpass_coef(f64::from(fc), fsd);
                    let mut v = f64::from(x[c]) + f64::from(s.feedback) * self.ap_out[c];
                    let (mut at_from, mut at_to) = (v, v);
                    for (st, state) in self.ap[c].iter_mut().enumerate().take(n) {
                        let y = a * v + state[0] - a * state[1];
                        state[0] = v;
                        state[1] = y;
                        v = y;
                        if st + 1 == from {
                            at_from = v;
                        }
                        if st + 1 == to {
                            at_to = v;
                        }
                    }
                    let y = at_from + (at_to - at_from) * t;
                    self.ap_out[c] = y;
                    wet[c] = y as f32;
                }
                let w = self.finish(p, wet, s.width);
                [x[0] * (1.0 - mix) + w[0] * mix, x[1] * (1.0 - mix) + w[1] * mix]
            }
            Program::Vibrato => {
                let centre = f64::from(VIB_CENTER_MS) * ms;
                let mut wet = [0.0f32; 2];
                for c in 0..2 {
                    let d = centre + f64::from(vib_amp * lfo[c]) * ms;
                    wet[c] = Self::read(&self.sinc, &self.clean[c], d);
                    // The pitch: how fast the reading's delay changes.
                    self.values[c] = cents_of_slope(d - self.vib_last[c]);
                    self.vib_last[c] = d;
                }
                // All wet: the vibrato is the effect (MIX does not apply).
                self.finish(p, wet, s.width)
            }
        }
    }
}
