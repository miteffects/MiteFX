//! Parametric EQ bands: prototypes, matched design, smoothing and processing.
//!
//! Shared by Mite EQ and, later, every plugin with an EQ section (Comp sidechain EQ, Verb decay
//! EQ, Delay per-tap EQ — BLUEPRINT §6 "shared engines").

use std::f64::consts::{FRAC_1_SQRT_2, PI};

use crate::filter::{matched, matched_symmetric, Analog, Coeffs, Tdf2};
use crate::smooth::LinearSmoother;

/// Band types, in their frozen parameter order (append only; the index is saved state).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BandType {
    #[default]
    Bell,
    LowShelf,
    HighShelf,
    LowCut,
    HighCut,
    Notch,
    BandPass,
    TiltShelf,
}

impl BandType {
    pub const ALL: [Self; 8] = [
        Self::Bell,
        Self::LowShelf,
        Self::HighShelf,
        Self::LowCut,
        Self::HighCut,
        Self::Notch,
        Self::BandPass,
        Self::TiltShelf,
    ];
    pub const NAMES: [&'static str; 8] =
        ["Bell", "Low Shelf", "High Shelf", "Low Cut", "High Cut", "Notch", "Band Pass", "Tilt Shelf"];

    pub fn from_index(i: usize) -> Self {
        Self::ALL.get(i).copied().unwrap_or_default()
    }

    /// Whether the gain parameter does anything for this type.
    pub fn has_gain(self) -> bool {
        matches!(self, Self::Bell | Self::LowShelf | Self::HighShelf | Self::TiltShelf)
    }

    /// Whether the slope parameter does anything for this type.
    pub fn has_slope(self) -> bool {
        matches!(self, Self::LowCut | Self::HighCut)
    }
}

/// Which part of a stereo signal a band processes, in its frozen parameter order (append only).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Placement {
    #[default]
    Stereo,
    Left,
    Right,
    Mid,
    Side,
}

impl Placement {
    pub const ALL: [Self; 5] = [Self::Stereo, Self::Left, Self::Right, Self::Mid, Self::Side];
    pub const NAMES: [&'static str; 5] = ["Stereo", "Left", "Right", "Mid", "Side"];

    pub fn from_index(i: usize) -> Self {
        Self::ALL.get(i).copied().unwrap_or_default()
    }
}

/// Cut slopes in dB/octave, in their frozen parameter order.
pub const SLOPES_DB: [u32; 9] = [6, 12, 18, 24, 30, 36, 48, 72, 96];
pub const SLOPE_NAMES: [&str; 9] =
    ["6 dB/oct", "12 dB/oct", "18 dB/oct", "24 dB/oct", "30 dB/oct", "36 dB/oct", "48 dB/oct", "72 dB/oct", "96 dB/oct"];
/// Index into [`SLOPES_DB`] of the default slope (24 dB/oct).
pub const DEFAULT_SLOPE: usize = 3;

/// Most sections a band can need: 96 dB/oct = order 16 = 8 second-order sections.
pub const MAX_SECTIONS: usize = 9;

pub const FREQ_MIN: f64 = 10.0;
pub const FREQ_MAX: f64 = 30_000.0;
pub const GAIN_RANGE_DB: f64 = 30.0;
pub const Q_MIN: f64 = 0.025;
pub const Q_MAX: f64 = 40.0;

/// A band's dynamic gain (BLUEPRINT §8.1): above `threshold_db` the band's gain moves towards
/// `gain + range_db`, by `(1 − 1/ratio)` dB per dB over the threshold (6 dB soft knee), never
/// further than the range. A negative range compresses the band, a positive one expands it.
/// Detection listens to the band's own region of the input.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Dynamics {
    pub enabled: bool,
    pub mode: DynamicsMode,
    /// Compressor: how far the band moves when loud.
    pub range_db: f64,
    /// Transient mode: how far the band moves on attacks, and in the tails after them.
    pub transient_db: f64,
    pub sustain_db: f64,
    /// Transient mode: how long the attack phase lasts before the sustain phase takes over, ms.
    pub transient_ms: f64,
    /// Transient mode: how wide the crossfade from attack to sustain is, as a share (0…2) of the
    /// transient time, centred on it.
    pub transition: f64,
    /// Compressor mode: work per frequency bin inside the band's range (spectral dynamics)
    /// instead of moving the whole band.
    pub spectral: bool,
    /// Spectral mode: how far each frequency's gain spreads to its neighbours (0…1: each bin on
    /// its own … a half-octave average).
    pub smoothing: f64,
    /// Spectral mode: compare each frequency with its surroundings (the average level around
    /// it) instead of a fixed level; `relative_db` is then the threshold, dB above them.
    pub relative: bool,
    pub relative_db: f64,
    pub threshold_db: f64,
    pub attack_ms: f64,
    pub release_ms: f64,
    pub ratio: f64,
}

impl Default for Dynamics {
    fn default() -> Self {
        Self {
            enabled: false,
            mode: DynamicsMode::Compressor,
            range_db: -6.0,
            transient_db: 0.0,
            sustain_db: 0.0,
            transient_ms: DEFAULT_TRANSIENT_MS,
            transition: DEFAULT_TRANSITION,
            spectral: false,
            smoothing: DEFAULT_SMOOTHING,
            relative: false,
            relative_db: DEFAULT_RELATIVE_DB,
            threshold_db: -30.0,
            attack_ms: 10.0,
            release_ms: 150.0,
            ratio: 3.0,
        }
    }
}

/// How a dynamic band decides when to move.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DynamicsMode {
    /// By level: past the threshold, by the ratio, up to the range.
    #[default]
    Compressor,
    /// By shape, whatever the level: the transient amount on attacks, the sustain in the tails.
    Transient,
}

impl DynamicsMode {
    pub const NAMES: [&'static str; 2] = ["Compressor", "Transient"];

    pub fn from_index(i: usize) -> Self {
        if i == 1 { Self::Transient } else { Self::Compressor }
    }
}

/// The band's gain plus its dynamic gain never leaves ±this, dB (the gain parameter's range).
pub const GAIN_LIMIT_DB: f64 = 30.0;

/// Transient detector: envelope (attack, release) times in ms, for the default transient time.
/// Attacks are where a fast-attack envelope leads a slow-attack one (the fast one also releases
/// sooner, so a decaying hit ends the attack). The slow times scale with the transient time.
const TRANSIENT_FAST: (f64, f64) = (0.5, 20.0);
const TRANSIENT_SLOW: (f64, f64) = (25.0, 60.0);
/// Where attack hands over to sustain by default, ms.
pub const DEFAULT_TRANSIENT_MS: f64 = 25.0;
/// How far the fast envelope must lead the slow one for a hit to count as fully sharp, dB. Softer
/// hits get a share of the attack amount (eased, smoothstep).
const SHAPE_FULL_DB: f64 = 12.0;
/// A hit starts when the fast envelope leads the slow one by this much (as a linear ratio, 3 dB).
const ONSET_RATIO: f64 = 1.412_537_5;
/// Into the attack phase at a hit: a 0.25 ms ramp, quick enough that no sustain leaks into the
/// hit's start (the jump coincides with the hit itself, which masks it).
const PHASE_RISE_MS: f64 = 0.25;
/// The crossfade from attack to sustain: its default width (share of the transient time) and the
/// narrowest it gets (ms), which still never clicks.
pub const DEFAULT_TRANSITION: f64 = 1.0;
/// Steepness of the crossfade's S (a logistic curve, rescaled to run exactly 1 → 0): long flat
/// shoulders and a quicker middle, so it reads as a curve rather than a ramp.
const TRANSITION_CURL: f64 = 8.0;
const MIN_TRANSITION_MS: f64 = 1.5;
/// Below this level the transient detector fades out (silence has no shape), dBFS.
const SHAPE_GATE_DB: f64 = -80.0;

/// The transient shaper's detector. A hit (the fast envelope pulling ahead of the slow one)
/// starts the attack phase, which lasts the transient time; then sustain takes over until the
/// next hit. Through the attack phase the band follows the hit: its attack weight rises with the
/// hit's sharpness and holds the peak, so a sharp hit gets the whole attack amount and a soft one
/// a share. The weight then crossfades into sustain. Gain = attack·w + sustain·(1 − w) with w in
/// 0…1, so it always lies between the two amounts. The band itself and the editor's preview both run it, so
/// the preview shows exactly what the band does.
#[derive(Debug, Clone, Copy)]
pub struct ShapeDetector {
    /// Fast and slow peak envelopes (linear) and their (attack, release) coefficients.
    env: [f64; 2],
    coeffs: [(f64, f64); 2],
    /// The attack phase: samples since the hit started (while one is under way), where the
    /// crossfade to sustain starts and how long it is (samples), whether the fast envelope leads
    /// now, and the switch (1 = attack phase, 0 = sustain) with its rise coefficient.
    elapsed: Option<u32>,
    fade_start: f64,
    fade_len: f64,
    onset: bool,
    phase: f64,
    rise: f64,
    /// The sharpest the current hit has been (0…1), held through its attack phase.
    sharpness: f64,
}

impl ShapeDetector {
    /// A detector fed `rate` levels per second, whose attack phase lasts `transient_ms` and hands
    /// over to sustain over `transition` (share of the transient time).
    pub fn new(rate: f64, transient_ms: f64, transition: f64) -> Self {
        let mut d = Self {
            env: [0.0; 2],
            coeffs: [(1.0, 1.0); 2],
            elapsed: None,
            fade_start: 0.0,
            fade_len: 1.0,
            onset: false,
            phase: 0.0,
            rise: 1.0,
            sharpness: 0.0,
        };
        d.set_times(rate, transient_ms, transition);
        d
    }

    pub fn set_times(&mut self, rate: f64, transient_ms: f64, transition: f64) {
        let coeff = |ms: f64| 1.0 - (-1000.0 / (ms * rate)).exp();
        let scale = transient_ms / DEFAULT_TRANSIENT_MS;
        let scaled = |(a, r): (f64, f64)| (if a > 1.0 { a * scale } else { a }, r * scale);
        for (c, times) in self.coeffs.iter_mut().zip([TRANSIENT_FAST, TRANSIENT_SLOW]) {
            let (a, r) = scaled(times);
            *c = (coeff(a), coeff(r));
        }
        let samples = |ms: f64| ms * rate / 1000.0;
        self.fade_len = samples((transition.clamp(0.0, 2.0) * transient_ms).max(MIN_TRANSITION_MS));
        self.fade_start = (samples(transient_ms) - 0.5 * self.fade_len).max(0.0);
        self.rise = coeff(PHASE_RISE_MS);
    }

    /// Sets both envelopes to `level` (linear), as if it had been steady, outside any hit.
    pub fn reset(&mut self, level: f64) {
        self.env = [level; 2];
        self.elapsed = None;
        self.onset = false;
        self.phase = 0.0;
        self.sharpness = 0.0;
    }

    /// Feeds one level (linear peak).
    #[inline]
    pub fn process(&mut self, level: f64) {
        for (e, &(a, r)) in self.env.iter_mut().zip(&self.coeffs) {
            let k = if level > *e { a } else { r };
            *e += k * (level - *e);
        }
        let onset = self.env[0] > self.env[1] * ONSET_RATIO && self.env[0] > 10f64.powf(SHAPE_GATE_DB / 20.0);
        if onset && !self.onset {
            self.elapsed = Some(0);
            self.sharpness = 0.0;
        }
        self.onset = onset;
        // 1 through the attack phase, then an S-shaped crossfade to 0 centred on the transient
        // time: flat at both ends, so the handover has no corner.
        let target = match self.elapsed {
            Some(n) => {
                let t = f64::from(n) - self.fade_start;
                self.elapsed = Some(n.saturating_add(1));
                if t <= 0.0 {
                    1.0
                } else if t < self.fade_len {
                    let logistic = |u: f64| 1.0 / (1.0 + (-TRANSITION_CURL * (u - 0.5)).exp());
                    let (lo, hi) = (logistic(0.0), logistic(1.0));
                    1.0 - (logistic(t / self.fade_len) - lo) / (hi - lo)
                } else {
                    self.elapsed = None;
                    0.0
                }
            }
            None => 0.0,
        };
        // Rising (a new hit) eases in over 0.2 ms; falling already follows the smooth crossfade.
        self.phase = if target > self.phase { self.phase + self.rise * (target - self.phase) } else { target };
    }

    /// The fast envelope: the level as the detector hears it now (linear).
    pub fn level(&self) -> f64 {
        self.env[0]
    }

    /// The gain for these amounts right now, dB: `transient_db` through the attack phase (scaled
    /// by the hit's sharpness), `sustain_db` after it, crossfaded. Call it at the control rate;
    /// it tracks the hit's peak sharpness.
    pub fn gain_db(&mut self, transient_db: f64, sustain_db: f64) -> f64 {
        let db = |x: f64| 20.0 * x.max(1e-10).log10();
        let [fast, slow] = self.env;
        if self.elapsed.is_some() {
            let u = ((db(fast) - db(slow)) / SHAPE_FULL_DB).clamp(0.0, 1.0);
            self.sharpness = self.sharpness.max(u * u * (3.0 - 2.0 * u));
        }
        let w = self.phase * self.sharpness;
        let gate = ((db(fast) - SHAPE_GATE_DB) / 20.0).clamp(0.0, 1.0);
        gate * (transient_db * w + sustain_db * (1.0 - w))
    }
}

/// Soft knee width of the dynamic gain computer, dB.
const DYNAMIC_KNEE_DB: f64 = 6.0;

impl Dynamics {
    /// Dynamic gain in dB for a detected level in dBFS.
    pub fn gain_db(&self, level_db: f64) -> f64 {
        let over = level_db - self.threshold_db;
        let half = 0.5 * DYNAMIC_KNEE_DB;
        let over = if over <= -half {
            0.0
        } else if over < half {
            (over + half).powi(2) / (2.0 * DYNAMIC_KNEE_DB)
        } else {
            over
        };
        let amount = over * (1.0 - 1.0 / self.ratio.max(1.0));
        amount.min(self.range_db.abs()) * self.range_db.signum()
    }
}

/// Default spectral smoothing (share of the widest, half an octave).
pub const DEFAULT_SMOOTHING: f64 = 0.25;
/// The widest spectral smoothing window, octaves.
pub const SMOOTHING_MAX_OCT: f64 = 0.5;

/// Relative spectral mode: the default threshold above the surroundings, dB, and how wide the
/// surroundings each frequency is compared with are, octaves.
pub const DEFAULT_RELATIVE_DB: f64 = 6.0;
pub const RELATIVE_WIDTH_OCT: f64 = 0.6;

/// One band's settings, as the host parameters describe them.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BandSettings {
    pub enabled: bool,
    pub kind: BandType,
    pub freq: f64,
    pub gain_db: f64,
    pub q: f64,
    /// Index into [`SLOPES_DB`].
    pub slope: usize,
    pub placement: Placement,
    pub dynamics: Dynamics,
}

impl Default for BandSettings {
    fn default() -> Self {
        Self { enabled: false, kind: BandType::Bell, freq: 1000.0, gain_db: 0.0, q: FRAC_1_SQRT_2, slope: DEFAULT_SLOPE, placement: Placement::Stereo, dynamics: Dynamics::default() }
    }
}

impl BandSettings {
    /// Whether the band is running dynamically (only types with a gain can).
    pub fn is_dynamic(&self) -> bool {
        self.dynamics.enabled && self.kind.has_gain()
    }

    /// Whether the band's dynamics work per frequency bin (spectral compressor mode).
    pub fn is_spectral(&self) -> bool {
        self.is_dynamic() && self.dynamics.mode == DynamicsMode::Compressor && self.dynamics.spectral
    }

    /// Whether the band needs the spectral stage (a spectral compressor).
    pub fn uses_spectrum(&self) -> bool {
        self.enabled && self.is_spectral()
    }

    /// Whether `other` needs a different filter structure (a crossfade instead of smoothing).
    fn structure_differs(&self, other: &Self) -> bool {
        self.enabled != other.enabled || self.kind != other.kind || self.slope != other.slope || self.placement != other.placement
    }
}

/// A band's digital sections. Fixed capacity, so designing never allocates.
#[derive(Debug, Clone, Copy)]
pub struct Sections {
    coeffs: [Coeffs; MAX_SECTIONS],
    len: usize,
}

impl Default for Sections {
    fn default() -> Self {
        Self { coeffs: [Coeffs::IDENTITY; MAX_SECTIONS], len: 0 }
    }
}

impl Sections {
    fn push(&mut self, c: Coeffs) {
        if self.len < MAX_SECTIONS {
            self.coeffs[self.len] = c;
            self.len += 1;
        }
    }

    pub fn as_slice(&self) -> &[Coeffs] {
        &self.coeffs[..self.len]
    }

    /// Digital response in dB at `w` radians/sample.
    /// The sections' response at `w` (radians/sample), magnitude and phase.
    pub fn response(&self, w: f64) -> realfft::num_complex::Complex<f64> {
        self.as_slice().iter().fold(realfft::num_complex::Complex::new(1.0, 0.0), |h, c| h * c.response(w))
    }

    pub fn response_db(&self, w: f64) -> f64 {
        self.as_slice().iter().map(|c| 10.0 * c.magnitude_sq(w).log10()).sum()
    }
}

/// Butterworth Q of section `k` (0-based) in an order-`order` cascade.
/// Q of the `k`-th pole pair of an order-`order` Butterworth filter; `k = 0` is the sharpest.
fn butterworth_q(order: usize, k: usize) -> f64 {
    1.0 / (2.0 * ((2 * k + 1) as f64 * PI / (2 * order) as f64).sin())
}

/// Calls `f` with every analog section of the band, plus whether it is a cut (|H| < 1 at the
/// centre) so it can be designed by inversion.
fn for_each_prototype(s: &BandSettings, mut f: impl FnMut(Analog, bool)) {
    let q = s.q.clamp(Q_MIN, Q_MAX);
    let a = 10f64.powf(s.gain_db.clamp(-GAIN_RANGE_DB, GAIN_RANGE_DB) / 40.0);
    let cut = s.gain_db < 0.0;
    match s.kind {
        BandType::Bell => f(Analog::second(1.0, a / q, 1.0, 1.0, 1.0 / (a * q), 1.0), cut),
        BandType::LowShelf => {
            let r = a.sqrt();
            f(Analog::second(a, a * r / q, a * a, a, r / q, 1.0), cut);
        }
        BandType::HighShelf => {
            let r = a.sqrt();
            f(Analog::second(a * a, a * r / q, a, 1.0, r / q, a), cut);
        }
        BandType::TiltShelf => {
            // First-order tilt pivoting on f0: −g/2 at DC, +g/2 at the top, unity at f0.
            f(Analog::first(a, 1.0, 1.0, a), false);
        }
        BandType::Notch => f(Analog::second(1.0, 0.0, 1.0, 1.0, 1.0 / q, 1.0), false),
        BandType::BandPass => f(Analog::second(0.0, 1.0 / q, 0.0, 1.0, 1.0 / q, 1.0), false),
        BandType::LowCut | BandType::HighCut => {
            let high = s.kind == BandType::HighCut;
            let order = (SLOPES_DB[s.slope.min(SLOPES_DB.len() - 1)] / 6) as usize;
            let pairs = order / 2;
            // The user Q sets the resonance of the sharpest section (= Butterworth at Q 0.71).
            let resonance = q / FRAC_1_SQRT_2;
            for k in 0..pairs {
                let mut qk = butterworth_q(order, k);
                if k == 0 {
                    qk *= resonance;
                }
                f(
                    if high {
                        Analog::second(0.0, 0.0, 1.0, 1.0, 1.0 / qk, 1.0)
                    } else {
                        Analog::second(1.0, 0.0, 0.0, 1.0, 1.0 / qk, 1.0)
                    },
                    false,
                );
            }
            if order % 2 == 1 {
                f(if high { Analog::first(0.0, 1.0, 1.0, 1.0) } else { Analog::first(1.0, 0.0, 1.0, 1.0) }, false);
            }
        }
    }
}

/// Designs the band's digital sections at `sample_rate`.
pub fn design(s: &BandSettings, sample_rate: f64) -> Sections {
    let mut out = Sections::default();
    if !s.enabled {
        return out;
    }
    let w0 = 2.0 * PI * s.freq.clamp(FREQ_MIN, FREQ_MAX) / sample_rate;
    for_each_prototype(s, |proto, cut| {
        out.push(if proto.is_first_order() { matched(&proto, w0) } else { matched_symmetric(&proto, w0, cut) });
    });
    out
}

/// The analog target response at `freq`, magnitude and phase (1 for a disabled band).
pub fn analog_response(s: &BandSettings, freq: f64) -> realfft::num_complex::Complex<f64> {
    let mut h = realfft::num_complex::Complex::new(1.0, 0.0);
    if s.enabled {
        let x = freq / s.freq.clamp(FREQ_MIN, FREQ_MAX);
        for_each_prototype(s, |proto, _| h *= proto.response(x));
    }
    h
}

/// The analog target response in dB at `freq`, what the band is designed to match.
pub fn analog_response_db(s: &BandSettings, freq: f64) -> f64 {
    if !s.enabled {
        return 0.0;
    }
    let x = freq / s.freq.clamp(FREQ_MIN, FREQ_MAX);
    let mut db = 0.0;
    for_each_prototype(s, |proto, _| db += 10.0 * proto.magnitude_sq(x).log10());
    db
}

/// The filter dynamic detection listens through: the band's region of the spectrum.
fn detector_design(s: &BandSettings, sample_rate: f64) -> Coeffs {
    let w0 = 2.0 * PI * s.freq.clamp(FREQ_MIN, FREQ_MAX) / sample_rate;
    let q = s.q.clamp(Q_MIN, Q_MAX);
    let proto = match s.kind {
        BandType::Bell => Analog::second(0.0, 1.0 / q, 0.0, 1.0, 1.0 / q, 1.0),
        BandType::LowShelf => Analog::second(0.0, 0.0, 1.0, 1.0, 1.0 / FRAC_1_SQRT_2, 1.0),
        BandType::HighShelf => Analog::second(1.0, 0.0, 0.0, 1.0, 1.0 / FRAC_1_SQRT_2, 1.0),
        // Tilt (and anything else): the whole spectrum.
        _ => return Coeffs::IDENTITY,
    };
    matched(&proto, w0)
}

/// Samples between coefficient redesigns while a parameter is moving.
const CONTROL_INTERVAL: u32 = 16;
/// While only the dynamic gain moves: the envelope is already smooth, so 0.7 ms at 48 kHz.
const DYNAMIC_INTERVAL: u32 = 32;
/// Dynamic gain changes smaller than this don't redesign the filter, dB.
const DYNAMIC_STEP_DB: f64 = 0.01;
/// Crossfade time for changes that can't be smoothed (type, slope, on/off).
const STRUCTURE_FADE_MS: f32 = 8.0;

/// One band, stereo, with smoothing. Allocation-free after construction.
pub struct BandProcessor {
    sample_rate: f64,
    /// Type/slope/enabled currently running.
    active: BandSettings,
    target: BandSettings,
    log_freq: LinearSmoother,
    gain: LinearSmoother,
    log_q: LinearSmoother,
    /// 0 = band bypassed, 1 = band fully in.
    mix: LinearSmoother,
    sections: Sections,
    state: [[Tdf2; MAX_SECTIONS]; 2],
    countdown: u32,
    /// Dynamics: detector filter and its state per channel, the detected peak envelope (linear),
    /// its attack/release coefficients, and the dynamic gain now applied (dB).
    detector: Coeffs,
    detector_state: [Tdf2; 2],
    envelope: f64,
    attack: f64,
    release: f64,
    /// Transient mode: the shape detector.
    shape: ShapeDetector,
    dynamic_db: f64,
    /// Dynamic gain the current sections were designed with, and the detector's design key
    /// (frequency, Q, type): redesigns happen only when these move.
    designed_dynamic_db: f64,
    /// Samples since the previous control update (the interval last scheduled).
    interval: u32,
    detector_key: (f64, f64, BandType),
}

impl BandProcessor {
    pub fn new(sample_rate: f64, smoothing_ms: f32) -> Self {
        let mut p = Self {
            sample_rate,
            active: BandSettings::default(),
            target: BandSettings::default(),
            log_freq: LinearSmoother::new(0.0),
            gain: LinearSmoother::new(0.0),
            log_q: LinearSmoother::new(0.0),
            mix: LinearSmoother::new(0.0),
            sections: Sections::default(),
            state: [[Tdf2::default(); MAX_SECTIONS]; 2],
            countdown: 0,
            detector: Coeffs::IDENTITY,
            detector_state: [Tdf2::default(); 2],
            envelope: 0.0,
            attack: 1.0,
            release: 1.0,
            shape: ShapeDetector::new(sample_rate, DEFAULT_TRANSIENT_MS, DEFAULT_TRANSITION),
            dynamic_db: 0.0,
            designed_dynamic_db: 0.0,
            interval: CONTROL_INTERVAL,
            detector_key: (0.0, 0.0, BandType::Bell),
        };
        p.prepare(sample_rate, smoothing_ms, &BandSettings::default());
        p
    }

    /// Off the audio thread: sets rates and jumps to `settings` without smoothing.
    pub fn prepare(&mut self, sample_rate: f64, smoothing_ms: f32, settings: &BandSettings) {
        self.sample_rate = sample_rate;
        let sr = sample_rate as f32;
        for s in [&mut self.log_freq, &mut self.gain, &mut self.log_q] {
            s.set_ramp(sr, smoothing_ms);
        }
        self.mix.set_ramp(sr, STRUCTURE_FADE_MS);
        self.reset(settings);
    }

    /// Jumps to `settings` and clears filter state. Allocation-free.
    pub fn reset(&mut self, settings: &BandSettings) {
        self.active = *settings;
        self.target = *settings;
        self.log_freq.reset(settings.freq.ln() as f32);
        self.gain.reset(settings.gain_db as f32);
        self.log_q.reset(settings.q.ln() as f32);
        self.mix.reset(if settings.enabled { 1.0 } else { 0.0 });
        self.envelope = 0.0;
        self.shape.reset(0.0);
        self.dynamic_db = 0.0;
        self.detector_state = [Tdf2::default(); 2];
        self.detector_key = (0.0, 0.0, BandType::Bell);
        self.update_ballistics();
        self.redesign();
        self.state = [[Tdf2::default(); MAX_SECTIONS]; 2];
    }

    /// One-pole coefficients for the envelope's attack and release times.
    fn update_ballistics(&mut self) {
        let coeff = |ms: f64| 1.0 - (-1000.0 / (ms.max(0.01) * self.sample_rate)).exp();
        self.attack = coeff(self.target.dynamics.attack_ms);
        self.release = coeff(self.target.dynamics.release_ms);
        let d = &self.target.dynamics;
        self.shape.set_times(self.sample_rate, d.transient_ms, d.transition);
    }

    pub fn set_target(&mut self, s: &BandSettings) {
        let ballistics_changed = s.dynamics.attack_ms != self.target.dynamics.attack_ms
            || s.dynamics.release_ms != self.target.dynamics.release_ms
            || s.dynamics.transient_ms != self.target.dynamics.transient_ms
            || s.dynamics.transition != self.target.dynamics.transition;
        if s.dynamics.mode != self.target.dynamics.mode {
            // Start the other detector from the level heard so far, so switching isn't read as a hit.
            self.shape.reset(self.envelope);
        }
        self.target = *s;
        if ballistics_changed {
            self.update_ballistics();
        }
        self.log_freq.set_target(s.freq.ln() as f32);
        self.gain.set_target(s.gain_db as f32);
        self.log_q.set_target(s.q.ln() as f32);
        if s.structure_differs(&self.active) {
            // Fade the band out; the swap happens once it is silent (see `advance`).
            self.mix.set_target(0.0);
        }
    }

    fn redesign(&mut self) {
        let settings = BandSettings {
            freq: f64::from(self.log_freq.current()).exp(),
            gain_db: f64::from(self.gain.current()) + self.dynamic_db,
            q: f64::from(self.log_q.current()).exp(),
            ..self.active
        };
        self.sections = design(&settings, self.sample_rate);
        self.designed_dynamic_db = self.dynamic_db;
        let key = (settings.freq, settings.q, settings.kind);
        if self.detecting() && key != self.detector_key {
            self.detector = detector_design(&settings, self.sample_rate);
            self.detector_key = key;
        }
    }

    /// Whether the detector runs: dynamics on for a type with gain.
    /// (Not for spectral dynamics: those work per bin in the spectral stage, not in this filter.)
    fn detecting(&self) -> bool {
        self.target.dynamics.enabled && self.active.kind.has_gain() && self.target.kind == self.active.kind && !self.target.is_spectral()
    }

    /// The dynamic gain the band applies right now, dB (0 when not dynamic).
    pub fn dynamic_gain_db(&self) -> f64 {
        self.dynamic_db
    }

    /// The detector's level right now, dBFS (floored at −120; the floor when not dynamic).
    pub fn detector_level_db(&self) -> f64 {
        20.0 * self.envelope.max(1e-6).log10()
    }

    /// Once per control interval: the dynamic gain from the envelope. With dynamics off it
    /// releases to 0 at the release rate instead of jumping (a jump would click).
    fn update_dynamic_gain(&mut self) {
        if self.detecting() {
            let d = &self.target.dynamics;
            let raw = match d.mode {
                DynamicsMode::Compressor => d.gain_db(20.0 * self.envelope.max(1e-10).log10()),
                DynamicsMode::Transient => self.shape.gain_db(d.transient_db, d.sustain_db),
            };
            // Gain plus dynamics stays inside the gain range, as the gain dial shows it.
            let gain = f64::from(self.gain.current());
            self.dynamic_db = (gain + raw).clamp(-GAIN_LIMIT_DB, GAIN_LIMIT_DB) - gain;
        } else {
            self.envelope = 0.0;
            self.shape.reset(0.0);
            self.dynamic_db *= (1.0 - self.release).powi(self.interval as i32);
            if self.dynamic_db.abs() < 1e-4 {
                self.dynamic_db = 0.0;
            }
        }
    }

    /// The settings the band is producing right now (smoothed values, active structure).
    pub fn current(&self) -> BandSettings {
        BandSettings {
            freq: f64::from(self.log_freq.current()).exp(),
            gain_db: f64::from(self.gain.current()),
            q: f64::from(self.log_q.current()).exp(),
            ..self.active
        }
    }

    /// True when the band is fully in and has no sections and no dynamics: it leaves the audio as
    /// it is.
    fn passes_through(&self) -> bool {
        self.sections.as_slice().is_empty()
            && !self.target.structure_differs(&self.active)
            && !self.mix.is_smoothing()
            && !self.detecting()
            && self.dynamic_db == 0.0
    }

    /// True when the band contributes nothing and needs no work: disabled and fully faded out.
    pub fn is_idle(&self) -> bool {
        !self.active.enabled && !self.target.enabled && !self.mix.is_smoothing() && self.mix.current() == 0.0
    }

    #[inline]
    fn advance(&mut self) -> f64 {
        let dynamic = self.detecting() || self.dynamic_db != 0.0;
        let smoothing = self.log_freq.is_smoothing() || self.gain.is_smoothing() || self.log_q.is_smoothing();
        let moving = smoothing || dynamic;
        self.log_freq.next_value();
        self.gain.next_value();
        self.log_q.next_value();
        let mix = self.mix.next_value();

        if mix == 0.0 && self.target.structure_differs(&self.active) {
            self.active.enabled = self.target.enabled;
            self.active.kind = self.target.kind;
            self.active.slope = self.target.slope;
            self.active.placement = self.target.placement;
            self.state = [[Tdf2::default(); MAX_SECTIONS]; 2];
            // The structure changed (often the type): the old dynamic gain no longer applies.
            self.dynamic_db = 0.0;
            self.envelope = 0.0;
            self.detector_state = [Tdf2::default(); 2];
            self.redesign();
            self.countdown = CONTROL_INTERVAL;
            self.mix.set_target(if self.active.enabled { 1.0 } else { 0.0 });
        } else if moving {
            if self.countdown == 0 {
                if dynamic {
                    self.update_dynamic_gain();
                }
                if smoothing || (self.dynamic_db - self.designed_dynamic_db).abs() >= DYNAMIC_STEP_DB {
                    self.redesign();
                }
                self.interval = if smoothing { CONTROL_INTERVAL } else { DYNAMIC_INTERVAL };
                self.countdown = self.interval;
            }
            self.countdown -= 1;
        }
        f64::from(mix)
    }

    /// Processes up to two channels in place. With one channel every placement processes it.
    pub fn process(&mut self, channels: &mut [&mut [f32]]) {
        if self.is_idle() {
            return;
        }
        if self.passes_through() {
            // No filter to run (no sections, e.g. a flat band): just keep the
            // smoothed values on their targets for the display.
            self.log_freq.reset(self.target.freq.ln() as f32);
            self.gain.reset(self.target.gain_db as f32);
            self.log_q.reset(self.target.q.ln() as f32);
            return;
        }
        let frames = channels.first().map_or(0, |c| c.len());
        let placement = if channels.len() >= 2 { self.active.placement } else { Placement::Stereo };
        for i in 0..frames {
            if self.detecting() {
                // Peak envelope of the band's region of the input it processes.
                let det = &self.detector;
                let level = match placement {
                    Placement::Stereo => channels
                        .iter()
                        .zip(self.detector_state.iter_mut())
                        .map(|(ch, st)| st.process(det, f64::from(ch[i])).abs())
                        .fold(0.0, f64::max),
                    Placement::Left => self.detector_state[0].process(det, f64::from(channels[0][i])).abs(),
                    Placement::Right => self.detector_state[0].process(det, f64::from(channels[1][i])).abs(),
                    Placement::Mid | Placement::Side => {
                        let (l, r) = (f64::from(channels[0][i]), f64::from(channels[1][i]));
                        let x = if placement == Placement::Mid { 0.5 * (l + r) } else { 0.5 * (l - r) };
                        self.detector_state[0].process(det, x).abs()
                    }
                };
                if self.target.dynamics.mode == DynamicsMode::Compressor {
                    let k = if level > self.envelope { self.attack } else { self.release };
                    self.envelope += k * (level - self.envelope);
                } else {
                    self.shape.process(level);
                    self.envelope = self.shape.level();
                }
            }
            let mix = self.advance();
            if mix == 0.0 {
                continue;
            }
            let sections = self.sections.as_slice();
            let filter = |state: &mut [Tdf2; MAX_SECTIONS], x: f64| {
                let mut y = x;
                for (section, st) in sections.iter().zip(state.iter_mut()) {
                    y = st.process(section, y);
                }
                if mix == 1.0 { y } else { x + mix * (y - x) }
            };
            match placement {
                Placement::Stereo => {
                    for (ch, state) in channels.iter_mut().zip(self.state.iter_mut()) {
                        ch[i] = filter(state, f64::from(ch[i])) as f32;
                    }
                }
                Placement::Left | Placement::Right => {
                    let c = usize::from(placement == Placement::Right);
                    channels[c][i] = filter(&mut self.state[0], f64::from(channels[c][i])) as f32;
                }
                Placement::Mid | Placement::Side => {
                    let (l, r) = (f64::from(channels[0][i]), f64::from(channels[1][i]));
                    let (mut m, mut s) = (0.5 * (l + r), 0.5 * (l - r));
                    if placement == Placement::Mid {
                        m = filter(&mut self.state[0], m);
                    } else {
                        s = filter(&mut self.state[0], s);
                    }
                    channels[0][i] = (m + s) as f32;
                    channels[1][i] = (m - s) as f32;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FS: f64 = 48_000.0;

    fn sine(freq: f64, len: usize) -> Vec<f32> {
        (0..len).map(|n| (2.0 * PI * freq * n as f64 / FS).sin() as f32 * 0.5).collect()
    }

    fn rms(x: &[f32]) -> f64 {
        (x.iter().map(|&v| f64::from(v).powi(2)).sum::<f64>() / x.len() as f64).sqrt()
    }

    fn band(kind: BandType, freq: f64, gain_db: f64, q: f64) -> BandSettings {
        BandSettings { enabled: true, kind, freq, gain_db, q, slope: DEFAULT_SLOPE, placement: Placement::Stereo, dynamics: Dynamics::default() }
    }

    #[test]
    fn disabled_band_nulls_bit_exactly() {
        let mut p = BandProcessor::new(FS, 20.0);
        let input = sine(440.0, 2048);
        let mut buf = input.clone();
        p.process(&mut [&mut buf]);
        assert_eq!(buf, input);
    }

    #[test]
    fn bell_gain_is_measured_in_the_signal() {
        let s = band(BandType::Bell, 1000.0, 6.0, 1.0);
        let mut p = BandProcessor::new(FS, 20.0);
        p.reset(&s);
        let input = sine(1000.0, 48_000);
        let mut buf = input.clone();
        p.process(&mut [&mut buf]);
        let gain = 20.0 * (rms(&buf[24_000..]) / rms(&input[24_000..])).log10();
        assert!((gain - 6.0).abs() < 0.05, "{gain}");
    }

    #[test]
    fn digital_follows_analog_for_every_type() {
        for kind in BandType::ALL {
            for freq in [80.0, 1000.0, 6000.0] {
                let s = band(kind, freq, -7.5, 1.3);
                let sections = design(&s, FS);
                let mut f = 20.0;
                while f < 16_000.0 {
                    let analog = analog_response_db(&s, f);
                    // Cuts are compared where they are within 12 dB of the passband: further down
                    // the steep skirt a dB of difference is inaudible.
                    let floor = if kind.has_slope() || kind == BandType::BandPass { -12.0 } else { -24.0 };
                    if analog > floor {
                        let digital = sections.response_db(2.0 * PI * f / FS);
                        // High bands carry the second-order Nyquist limit (see filter.rs tests).
                        let limit = if freq > 4000.0 { 1.0 } else { 0.35 };
                        assert!((digital - analog).abs() < limit, "{kind:?} {freq} Hz at {f:.0} Hz: {digital:.2} vs {analog:.2}");
                    }
                    f *= 1.05;
                }
            }
        }
    }

    #[test]
    fn cut_slopes_have_their_steepness() {
        for (slope, &db_per_oct) in SLOPES_DB.iter().enumerate() {
            let s = BandSettings { slope, ..band(BandType::LowCut, 1000.0, 0.0, FRAC_1_SQRT_2) };
            let sections = design(&s, FS);
            // Three and five octaves below the corner the asymptote dominates.
            let at = |f: f64| sections.response_db(2.0 * PI * f / FS);
            let measured = at(125.0) - at(31.25);
            let expected = 2.0 * f64::from(db_per_oct);
            let tolerance = (0.005 * expected).max(0.5);
            assert!((measured - expected).abs() < tolerance, "{db_per_oct} dB/oct: {measured:.2}");
        }
    }

    #[test]
    fn moving_frequency_is_click_free() {
        let mut p = BandProcessor::new(FS, 20.0);
        p.reset(&band(BandType::Bell, 100.0, 12.0, 2.0));
        let mut buf = vec![0.25_f32; 4800];
        p.process(&mut [&mut buf]);
        p.set_target(&band(BandType::Bell, 10_000.0, 12.0, 2.0));
        let mut buf = vec![0.25_f32; 4800];
        p.process(&mut [&mut buf]);
        // DC input: any discontinuity shows up as a jump between neighbouring samples.
        let worst = buf.windows(2).map(|w| (w[1] - w[0]).abs()).fold(0.0, f32::max);
        assert!(worst < 0.01, "{worst}");
    }

    #[test]
    fn type_change_crossfades_instead_of_clicking() {
        let mut p = BandProcessor::new(FS, 20.0);
        p.reset(&band(BandType::Bell, 1000.0, 12.0, 1.0));
        let input = sine(1000.0, 9600);
        let mut warm = input.clone();
        p.process(&mut [&mut warm]);
        p.set_target(&band(BandType::HighCut, 200.0, 0.0, FRAC_1_SQRT_2));
        let mut buf = input.clone();
        p.process(&mut [&mut buf]);
        let step = buf.windows(2).map(|w| (w[1] - w[0]).abs()).fold(0.0, f32::max);
        // A 1 kHz sine at +12 dB moves at most ~0.26 per sample; a hard swap would jump further.
        assert!(step < 0.3, "{step}");
        // After the fade the high cut at 200 Hz has removed most of the 1 kHz tone.
        assert!(rms(&buf[4800..]) < 0.05 * rms(&input[4800..]));
    }

    #[test]
    fn enabling_and_disabling_ends_bit_exact() {
        let mut p = BandProcessor::new(FS, 20.0);
        p.set_target(&band(BandType::Bell, 1000.0, 6.0, 1.0));
        let mut buf = sine(1000.0, 4800);
        p.process(&mut [&mut buf]);
        p.set_target(&BandSettings { enabled: false, ..band(BandType::Bell, 1000.0, 6.0, 1.0) });
        let mut fade = sine(1000.0, 4800);
        p.process(&mut [&mut fade]);
        assert!(p.is_idle());
        let input = sine(1000.0, 1024);
        let mut after = input.clone();
        p.process(&mut [&mut after]);
        assert_eq!(after, input);
    }

    fn gain_db_at(p: &mut BandProcessor, freq: f64, channel: usize, other: f32) -> (f64, Vec<f32>) {
        let x = sine(freq, 48_000);
        let mut l = x.clone();
        let mut r: Vec<f32> = x.iter().map(|v| v * other).collect();
        p.process(&mut [&mut l, &mut r]);
        let out = if channel == 0 { &l } else { &r };
        let reference: Vec<f32> = if channel == 0 { x.clone() } else { x.iter().map(|v| v * other).collect() };
        (20.0 * (rms(&out[24_000..]) / rms(&reference[24_000..])).log10(), if channel == 0 { r } else { l })
    }

    #[test]
    fn left_and_right_placement_touch_only_their_channel() {
        for (placement, c) in [(Placement::Left, 0), (Placement::Right, 1)] {
            let s = BandSettings { placement, ..band(BandType::Bell, 1000.0, 6.0, 1.0) };
            let mut p = BandProcessor::new(FS, 20.0);
            p.reset(&s);
            let (gain, other) = gain_db_at(&mut p, 1000.0, c, 1.0);
            assert!((gain - 6.0).abs() < 0.05, "{placement:?} {gain}");
            assert_eq!(other, sine(1000.0, 48_000), "{placement:?} touched the other channel");
        }
    }

    #[test]
    fn mid_and_side_placement_process_only_their_component() {
        // Identical channels are pure mid: a side band leaves them bit-exact, a mid band boosts.
        let mut p = BandProcessor::new(FS, 20.0);
        p.reset(&BandSettings { placement: Placement::Side, ..band(BandType::Bell, 1000.0, 6.0, 1.0) });
        let (gain, _) = gain_db_at(&mut p, 1000.0, 0, 1.0);
        assert!(gain.abs() < 1e-6, "side band changed mid signal: {gain}");
        let mut p = BandProcessor::new(FS, 20.0);
        p.reset(&BandSettings { placement: Placement::Mid, ..band(BandType::Bell, 1000.0, 6.0, 1.0) });
        let (gain, _) = gain_db_at(&mut p, 1000.0, 0, 1.0);
        assert!((gain - 6.0).abs() < 0.05, "{gain}");
        // Opposite channels are pure side.
        let mut p = BandProcessor::new(FS, 20.0);
        p.reset(&BandSettings { placement: Placement::Side, ..band(BandType::Bell, 1000.0, 6.0, 1.0) });
        let (gain, _) = gain_db_at(&mut p, 1000.0, 0, -1.0);
        assert!((gain - 6.0).abs() < 0.05, "{gain}");
    }

    #[test]
    fn placement_change_crossfades() {
        let s = band(BandType::Bell, 1000.0, 12.0, 1.0);
        let mut p = BandProcessor::new(FS, 20.0);
        p.reset(&s);
        let x = sine(1000.0, 4800);
        let (mut l, mut r) = (x.clone(), x.clone());
        p.process(&mut [&mut l, &mut r]);
        p.set_target(&BandSettings { placement: Placement::Left, ..s });
        let (mut l, mut r) = (x.clone(), x.clone());
        p.process(&mut [&mut l, &mut r]);
        let jump = r.windows(2).map(|w| (w[1] - w[0]).abs()).fold(0.0_f32, f32::max);
        // A 1 kHz sine at 12 dB (amplitude 2) moves at most ~0.27 per sample.
        assert!(jump < 0.3, "right channel jumped {jump}");
        assert_eq!(p.current().placement, Placement::Left);
    }

    #[test]
    fn every_cut_slope_is_butterworth_at_default_q() {
        // −3 dB at the cutoff and no overshoot anywhere, for every slope: changing the slope must
        // never add a bump.
        for kind in [BandType::LowCut, BandType::HighCut] {
            for (slope, db_oct) in SLOPES_DB.iter().enumerate() {
                let s = BandSettings { slope, ..band(kind, 1000.0, 0.0, FRAC_1_SQRT_2) };
                let at_cutoff = analog_response_db(&s, 1000.0);
                assert!((at_cutoff + 3.0103).abs() < 0.01, "{kind:?} {db_oct} dB/oct: {at_cutoff}");
                // Every corner, up to near Nyquist: corner-matched high-pass sections used to lift
                // the top end by up to 0.5 dB for corners above ~5 kHz.
                for f0 in [30.0, 1000.0, 5000.0, 12_000.0, 18_000.0] {
                    let sec = design(&BandSettings { freq: f0, ..s }, FS);
                    let peak = (1..4000).map(|i| sec.response_db(PI * f64::from(i) / 4000.0)).fold(f64::MIN, f64::max);
                    let limit = if kind == BandType::HighCut && db_oct % 12 != 0 { 0.25 } else { 0.01 };
                    assert!(peak < limit, "{kind:?} {db_oct} dB/oct at {f0} Hz overshoots {peak}");
                }
            }
        }
    }

    fn dynamic_bell(range_db: f64, threshold_db: f64) -> BandSettings {
        BandSettings {
            dynamics: Dynamics { enabled: true, range_db, threshold_db, attack_ms: 1.0, release_ms: 50.0, ratio: 20.0, ..Dynamics::default() },
            ..band(BandType::Bell, 2000.0, 0.0, 2.0)
        }
    }

    /// Gain at `freq` in dB (second half, after the dynamics settle) for a sine of `amplitude`.
    fn settled_gain_db(p: &mut BandProcessor, freq: f64, amplitude: f32) -> f64 {
        let x: Vec<f32> = sine(freq, 48_000).iter().map(|v| v * 2.0 * amplitude).collect();
        let mut y = x.clone();
        p.process(&mut [&mut y]);
        20.0 * (rms(&y[36_000..]) / rms(&x[36_000..])).log10()
    }

    #[test]
    fn gain_computer_follows_ratio_knee_and_range() {
        let d = Dynamics { enabled: true, range_db: -6.0, threshold_db: -30.0, ratio: 3.0, ..Dynamics::default() };
        assert_eq!(d.gain_db(-40.0), 0.0);
        // 6 dB over at 3:1 → 4 dB of reduction.
        assert!((d.gain_db(-24.0) + 4.0).abs() < 1e-9);
        // Capped at the range.
        assert_eq!(d.gain_db(0.0), -6.0);
        // Inside the knee: continuous, between 0 and the straight line.
        let k = d.gain_db(-30.0);
        assert!(k < 0.0 && k > -1.0, "{k}");
        let up = Dynamics { range_db: 4.0, ..d };
        assert_eq!(up.gain_db(0.0), 4.0);
    }

    #[test]
    fn loud_band_content_moves_the_gain_by_the_range() {
        let mut p = BandProcessor::new(FS, 20.0);
        p.reset(&dynamic_bell(-6.0, -30.0));
        let gain = settled_gain_db(&mut p, 2000.0, 0.5);
        assert!((gain + 6.0).abs() < 0.2, "{gain}");
        assert!((p.dynamic_gain_db() + 6.0).abs() < 1e-6, "{}", p.dynamic_gain_db());
    }

    #[test]
    fn quiet_band_content_stays_static() {
        let mut p = BandProcessor::new(FS, 20.0);
        p.reset(&dynamic_bell(-6.0, -30.0));
        // About −46 dBFS peak, well under the threshold and knee.
        let gain = settled_gain_db(&mut p, 2000.0, 0.005);
        assert!(gain.abs() < 0.01, "{gain}");
    }

    #[test]
    fn detection_ignores_content_outside_the_band() {
        let mut p = BandProcessor::new(FS, 20.0);
        p.reset(&dynamic_bell(-6.0, -30.0));
        // A loud 100 Hz tone, more than four octaves under a Q 2 band at 2 kHz.
        settled_gain_db(&mut p, 100.0, 0.5);
        assert!(p.dynamic_gain_db().abs() < 0.5, "{}", p.dynamic_gain_db());
    }

    /// Dynamic gain over time for a 2 kHz tone that starts abruptly at `start` and then decays by
    /// 30 dB per 100 ms, sampled every 1 ms.
    fn shape_trace(s: &BandSettings) -> Vec<f64> {
        let mut p = BandProcessor::new(FS, 20.0);
        p.reset(s);
        let start = 4800;
        let mut trace = Vec::new();
        for block in 0..400 {
            let mut x: Vec<f32> = (0..48)
                .map(|j| {
                    let i = block * 48 + j;
                    if i < start {
                        return 0.0;
                    }
                    let t = (i - start) as f64 / f64::from(FS as f32);
                    (0.5 * 10f64.powf(-15.0 * t) * (2.0 * std::f64::consts::PI * 2000.0 * t).sin()) as f32
                })
                .collect();
            p.process(&mut [&mut x]);
            trace.push(p.dynamic_gain_db());
        }
        trace
    }

    fn transient_bell(attack: f64, sustain: f64) -> BandSettings {
        BandSettings {
            dynamics: Dynamics { enabled: true, mode: DynamicsMode::Transient, transient_db: attack, sustain_db: sustain, ..Dynamics::default() },
            ..band(BandType::Bell, 2000.0, 0.0, 2.0)
        }
    }

    #[test]
    fn transient_mode_moves_on_attacks_and_in_tails() {
        // Attack: the band jumps up as the hit starts and settles back within ~100 ms.
        let t = shape_trace(&transient_bell(9.0, 0.0));
        let onset = t[100..120].iter().copied().fold(f64::MIN, f64::max);
        assert!(onset > 5.0, "onset {onset}");
        assert!(t[300].abs() < 1.0, "late {}", t[300]);
        assert!(t[..100].iter().all(|g| g.abs() < 1e-6), "moved before the hit");
        // Sustain: the band rises in the tail, not at the onset.
        let s = shape_trace(&transient_bell(0.0, 9.0));
        let early = s[100..103].iter().copied().fold(f64::MIN, f64::max);
        let tail = s[180..260].iter().copied().fold(f64::MIN, f64::max);
        assert!(early < 1.0 && tail > 4.0, "early {early}, tail {tail}");
        // A shelf from the time mark (25 ms after the hit), crossfading over 25 ms around it:
        // flat before the crossfade, full after it, rising steadily in between.
        // (The first millisecond is the 0.25 ms switch into the attack phase.)
        assert!(s[101..112].iter().all(|g| g.abs() < 0.1), "sustain before the crossfade: {:?}", &s[100..112]);
        let held = s[140..280].iter().copied().fold(f64::MAX, f64::min);
        assert!(held > 8.5, "sustain sagged to {held} dB in the tail");
        assert!(s[112..140].windows(2).all(|w| w[1] >= w[0] - 1e-9), "crossfade not monotonic: {:?}", &s[112..140]);
    }

    #[test]
    fn cutting_both_attack_and_sustain_never_gets_louder_in_between() {
        // The gain must stay between the two amounts through the handover: no bump towards 0 dB.
        let t = shape_trace(&transient_bell(-9.0, -6.0));
        let worst = t[102..300].iter().copied().fold(f64::MIN, f64::max);
        assert!(worst < -5.9, "the band came up to {worst} dB between attack and sustain");
        let lowest = t[102..300].iter().copied().fold(f64::MAX, f64::min);
        assert!(lowest > -9.1, "{lowest}");
    }

    #[test]
    fn softer_hits_get_less_attack() {
        // A hit out of silence is fully sharp; a 6 dB step on a steady tone is a soft one.
        let peak = |x: &[f32]| {
            let mut p = BandProcessor::new(FS, 20.0);
            p.reset(&transient_bell(9.0, 0.0));
            // Only the moment of the hit counts (the soft signal's own start is a sharp hit too).
            let mut most = f64::MIN;
            for (k, chunk) in x.chunks(48).enumerate() {
                let mut c = chunk.to_vec();
                p.process(&mut [&mut c]);
                if k >= 100 {
                    most = most.max(p.dynamic_gain_db());
                }
            }
            most
        };
        let tone = |i: usize, a: f64| (a * (2.0 * std::f64::consts::PI * 2000.0 * i as f64 / 48_000.0).sin()) as f32;
        let sharp: Vec<f32> = (0..9600).map(|i| if i < 4800 { 0.0 } else { tone(i, 0.2) }).collect();
        let soft: Vec<f32> = (0..9600).map(|i| tone(i, if i < 4800 { 0.1 } else { 0.2 })).collect();
        let (sharp, soft) = (peak(&sharp), peak(&soft));
        assert!(sharp > 8.5, "sharp hit {sharp}");
        assert!(soft < 0.75 * sharp && soft > 0.5, "soft hit {soft} vs sharp {sharp}");
    }

    #[test]
    fn switching_into_the_attack_phase_does_not_click() {
        // A second hit lands on the first one's tail with attack and sustain 24 dB apart.
        let mut p = BandProcessor::new(FS, 20.0);
        p.reset(&transient_bell(12.0, -12.0));
        let hit = |t: f64| if t < 0.0 { 0.0 } else { 10f64.powf(-3.0 * t) };
        let x: Vec<f32> = (0..19_200)
            .map(|i| {
                let t = i as f64 / 48_000.0;
                (0.25 * (hit(t) + hit(t - 0.2)).min(1.0) * (2.0 * std::f64::consts::PI * 2000.0 * t).sin()) as f32
            })
            .collect();
        let mut y = x.clone();
        p.process(&mut [&mut y]);
        // The loudest the band can make it: +12 dB on the strongest moment of the input.
        let tone = 4.0 * 0.5 * 2.0 * std::f64::consts::PI * 2000.0 / 48_000.0;
        let jump = y.windows(2).map(|w| f64::from((w[1] - w[0]).abs())).fold(0.0, f64::max);
        assert!(jump < 1.15 * tone, "step {jump} vs tone slope {tone}");
    }

    #[test]
    fn the_handover_to_sustain_does_not_click() {
        // +12 dB of sustain switches in 25 ms into a 2 kHz hit. A click would show as a
        // sample-to-sample step beyond what the louder tone itself moves.
        let mut p = BandProcessor::new(FS, 20.0);
        p.reset(&transient_bell(0.0, 12.0));
        let x: Vec<f32> = (0..9600)
            .map(|i| {
                let t = i as f64 / 48_000.0;
                (0.25 * 10f64.powf(-3.0 * t) * (2.0 * std::f64::consts::PI * 2000.0 * t).sin()) as f32
            })
            .collect();
        let mut y = x.clone();
        p.process(&mut [&mut y]);
        let tone = 4.0 * 0.25 * 2.0 * std::f64::consts::PI * 2000.0 / 48_000.0;
        let jump = y.windows(2).map(|w| f64::from((w[1] - w[0]).abs())).fold(0.0, f64::max);
        assert!(jump < 1.15 * tone, "step {jump} vs tone slope {tone}");
    }

    #[test]
    fn a_longer_transient_time_holds_the_attack_longer() {
        let at = |ms: f64| {
            let s = BandSettings { dynamics: Dynamics { transient_ms: ms, ..transient_bell(9.0, 0.0).dynamics }, ..transient_bell(9.0, 0.0) };
            // Gain 40 ms after the hit starts (trace index 100 is the onset, 1 ms per index).
            shape_trace(&s)[140]
        };
        let (short, long) = (at(10.0), at(100.0));
        assert!(long > short + 2.0, "10 ms: {short}, 100 ms: {long}");
    }

    #[test]
    fn gain_and_dynamics_stay_inside_the_gain_range() {
        let s = BandSettings { gain_db: 25.0, ..dynamic_bell(12.0, -60.0) };
        let mut p = BandProcessor::new(FS, 20.0);
        p.reset(&s);
        let mut x: Vec<f32> = sine(2000.0, 24_000);
        p.process(&mut [&mut x]);
        assert!((p.dynamic_gain_db() - 5.0).abs() < 1e-6, "{}", p.dynamic_gain_db());
    }

    #[test]
    fn turning_dynamics_off_does_not_click() {
        let s = dynamic_bell(-12.0, -40.0);
        let mut p = BandProcessor::new(FS, 20.0);
        p.reset(&s);
        let x: Vec<f32> = sine(2000.0, 24_000);
        let mut y = x.clone();
        p.process(&mut [&mut y]);
        assert!(p.dynamic_gain_db() < -11.0);
        p.set_target(&BandSettings { dynamics: Dynamics { enabled: false, ..s.dynamics }, ..s });
        let mut y = x.clone();
        p.process(&mut [&mut y]);
        // A 2 kHz sine at 0.5 moves at most ~0.13 per sample; a gain jump would exceed it.
        let jump = y.windows(2).map(|w| (w[1] - w[0]).abs()).fold(0.0_f32, f32::max);
        assert!(jump < 0.14, "{jump}");
        assert!(p.dynamic_gain_db().abs() < 0.01, "released to {}", p.dynamic_gain_db());
    }
}
