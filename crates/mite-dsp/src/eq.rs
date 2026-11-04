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
    /// A steady slope across the whole spectrum, pivoting at the frequency: the gain spread over
    /// half the 10 Hz–30 kHz range (gain / 5.8 dB per octave).
    FlatTilt,
    /// Rotates the phase around the frequency (Q: how quickly) and leaves the level alone.
    AllPass,
}

impl BandType {
    pub const ALL: [Self; 10] = [
        Self::Bell,
        Self::LowShelf,
        Self::HighShelf,
        Self::LowCut,
        Self::HighCut,
        Self::Notch,
        Self::BandPass,
        Self::TiltShelf,
        Self::FlatTilt,
        Self::AllPass,
    ];
    pub const NAMES: [&'static str; 10] =
        ["Bell", "Low Shelf", "High Shelf", "Low Cut", "High Cut", "Notch", "Band Pass", "Tilt Shelf", "Flat Tilt", "All Pass"];

    pub fn from_index(i: usize) -> Self {
        Self::ALL.get(i).copied().unwrap_or_default()
    }

    /// Whether the gain parameter does anything for this type.
    pub fn has_gain(self) -> bool {
        matches!(self, Self::Bell | Self::LowShelf | Self::HighShelf | Self::TiltShelf | Self::FlatTilt)
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
/// The slope choices: the IIR slopes, then Brickwall, a vertical edge that only Linear Phase
/// can make (the IIR modes play their steepest slope, 96 dB/oct, for it).
pub const SLOPE_NAMES: [&str; 10] =
    ["6 dB/oct", "12 dB/oct", "18 dB/oct", "24 dB/oct", "30 dB/oct", "36 dB/oct", "48 dB/oct", "72 dB/oct", "96 dB/oct", "Brickwall"];
/// Index of the Brickwall slope in [`SLOPE_NAMES`].
pub const BRICKWALL: usize = 9;
/// Flat Tilt: how many pole/zero pairs make its slope, and the span they cover, octaves (a little
/// past 10 Hz–30 kHz so the slope holds to the ends).
const FLAT_TILT_PAIRS: usize = 9;
const FLAT_TILT_SPAN: (f64, f64) = (5.0, 60_000.0);
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
    /// Transient mode: how long attack lasts (it has faded to about a twentieth by then), ms.
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
    /// What the detector listens to.
    pub source: DetectSource,
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
            source: DetectSource::Own,
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

/// What a dynamic band listens to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DetectSource {
    /// Its own region of the signal it processes.
    #[default]
    Own,
    /// Its region of the external sidechain.
    Sidechain,
    /// Another band's region (by index) of the signal this band processes.
    Band(usize),
}

impl DetectSource {
    /// The parameter's choices: own, sidechain, then bands 1–24.
    pub fn from_index(i: usize) -> Self {
        match i {
            0 => Self::Own,
            1 => Self::Sidechain,
            n => Self::Band(n - 2),
        }
    }

    pub fn index(self) -> usize {
        match self {
            Self::Own => 0,
            Self::Sidechain => 1,
            Self::Band(n) => n + 2,
        }
    }
}

/// The band's gain plus its dynamic gain never leaves ±this, dB (the gain parameter's range).
pub const GAIN_LIMIT_DB: f64 = 30.0;

/// How long attack lasts by default (it has faded to about a twentieth by then), ms.
pub const DEFAULT_TRANSIENT_MS: f64 = 25.0;
/// How quickly sustain comes in by default (share of the transient time).
pub const DEFAULT_TRANSITION: f64 = 1.0;
/// The fast envelope's attack, ms.
const SHAPE_ATTACK_MS: f64 = 0.5;
/// The fast envelope holds its peak this long before it releases, ms: longer than half a period of
/// 25 Hz, so a held tone's envelope has no ripple and its gain does not distort it.
const SHAPE_HOLD_MS: f64 = 20.0;
/// The slow-rising envelope's time constant, as a share of the transient time. With one half, the
/// lead out of silence has fallen to about 1.3 dB by the transient time.
const SHAPE_RISE: f64 = 0.5;
/// The slow-falling envelope's time constant, as a multiple of the transient time.
const SHAPE_FALL: f64 = 20.0;
/// The lead that brings attack and the fall that brings sustain near their full amounts, dB
/// (the amount saturates along a tanh, so nothing has a corner).
const ATTACK_FULL_DB: f64 = 12.0;
const SUSTAIN_FULL_DB: f64 = 6.0;
/// The soft knee under the leads, dB: what a noisy envelope wobbles by moves nothing.
const SHAPE_KNEE_DB: f64 = 1.0;
/// A hit is reported when the fast envelope leads the slow-rising one by this much (a linear ratio,
/// 3 dB), and it must fall back under this lead (1.5 dB) before another can. Only the history's
/// ticks and the sustain length use hits: the gain follows the envelopes alone.
const ONSET_RATIO: f64 = 1.412_537_5;
const REARM_RATIO: f64 = 1.188_502_2;
/// How far the lead must reach for a hit to count as fully sharp, dB.
const SHARP_FULL_DB: f64 = 12.0;
/// Below this level the transient detector fades out (silence has no shape), dBFS.
const SHAPE_GATE_DB: f64 = -80.0;
/// Under this the envelopes are silence (and never run into denormals).
const SHAPE_SILENT: f64 = 1e-12;

/// The transient shaper's detector, a differential envelope follower with no threshold. Three
/// envelopes of one level:
/// - `fast` catches every peak and holds it through the waveform's ripple;
/// - `rise` follows `fast` up slowly and down at once, so `fast` leads it only during an onset;
/// - `fall` follows `fast` up at once and down slowly, so it exceeds `fast` only during a decay.
///
/// The lead sets the attack weight and the fall the sustain weight, so a steady signal is left
/// alone however long it lasts, a bigger rise gets more attack, and sustain grows as a sound dies
/// away. Both are ratios, so the shape does not depend on level. Gain = attack·w + sustain·v·(1 − w),
/// so it always lies between 0 dB and the two amounts. The band itself and the editor's preview
/// both run it, so the preview shows exactly what the band does.
#[derive(Debug, Clone, Copy)]
pub struct ShapeDetector {
    fast: f64,
    rise: f64,
    fall: f64,
    /// `fall / fast`, rising no quicker than the transition allows.
    decay: f64,
    /// Samples the fast envelope still holds its peak, and how long it holds.
    hold_left: u32,
    hold: u32,
    /// Per-sample coefficients: the fast envelope's attack and release, `rise`'s rise, `fall`'s
    /// fall and `decay`'s rise.
    k_attack: f64,
    k_release: f64,
    k_rise: f64,
    k_fall: f64,
    k_decay: f64,
    /// Samples a hit stays under way, and those since the current one started, while it is.
    hit_len: u32,
    elapsed: Option<u32>,
    onset: bool,
    /// The sharpest the current hit has been (0…1), held while it is under way.
    sharpness: f64,
    /// Samples since the last hit started.
    since: Option<u32>,
    /// The attack weight the last [`Self::gain_db`] worked with.
    weight: f64,
}

impl ShapeDetector {
    /// A detector fed `rate` levels per second, whose attack fades over `transient_ms` and whose
    /// sustain comes in over `transition` (share of the transient time).
    pub fn new(rate: f64, transient_ms: f64, transition: f64) -> Self {
        let mut d = Self {
            fast: 0.0,
            rise: 0.0,
            fall: 0.0,
            decay: 1.0,
            hold_left: 0,
            hold: 0,
            k_attack: 1.0,
            k_release: 1.0,
            k_rise: 1.0,
            k_fall: 1.0,
            k_decay: 1.0,
            hit_len: 0,
            elapsed: None,
            onset: false,
            sharpness: 0.0,
            since: None,
            weight: 0.0,
        };
        d.set_times(rate, transient_ms, transition);
        d
    }

    pub fn set_times(&mut self, rate: f64, transient_ms: f64, transition: f64) {
        let coeff = |ms: f64| if ms > 0.0 { 1.0 - (-1000.0 / (ms * rate)).exp() } else { 1.0 };
        self.k_attack = coeff(SHAPE_ATTACK_MS);
        self.k_release = coeff(transient_ms);
        self.k_rise = coeff(SHAPE_RISE * transient_ms);
        self.k_fall = coeff(SHAPE_FALL * transient_ms);
        self.k_decay = coeff(SHAPE_RISE * transition.clamp(0.0, 2.0) * transient_ms);
        self.hold = (SHAPE_HOLD_MS * rate / 1000.0).round() as u32;
        self.hit_len = (transient_ms * rate / 1000.0).round() as u32;
    }

    /// Sets the envelopes to `level` (linear), as if it had been steady, outside any hit.
    pub fn reset(&mut self, level: f64) {
        self.fast = level;
        self.rise = level;
        self.fall = level;
        self.decay = 1.0;
        self.hold_left = 0;
        self.elapsed = None;
        self.onset = false;
        self.sharpness = 0.0;
        self.since = None;
        self.weight = 0.0;
    }

    /// Feeds one level (linear peak).
    #[inline]
    pub fn process(&mut self, level: f64) {
        if level >= self.fast {
            self.fast += self.k_attack * (level - self.fast);
            self.hold_left = self.hold;
        } else if self.hold_left > 0 {
            self.hold_left -= 1;
        } else {
            self.fast += self.k_release * (level - self.fast);
        }
        if self.fast < SHAPE_SILENT {
            self.fast = 0.0;
        }
        self.rise = if self.fast > self.rise { self.rise + self.k_rise * (self.fast - self.rise) } else { self.fast };
        self.fall = if self.fast > self.fall { self.fast } else { self.fall + self.k_fall * (self.fast - self.fall) };
        if self.fall < SHAPE_SILENT {
            self.fall = 0.0;
        }
        let decay = if self.fast > 0.0 { self.fall / self.fast } else { 1.0 };
        self.decay = if decay > self.decay { self.decay + self.k_decay * (decay - self.decay) } else { decay };

        let ratio = if self.onset { REARM_RATIO } else { ONSET_RATIO };
        let onset = self.fast > self.rise * ratio && self.fast > 10f64.powf(SHAPE_GATE_DB / 20.0);
        if onset && !self.onset {
            self.elapsed = Some(0);
            self.sharpness = 0.0;
            self.since = Some(0);
        } else if let Some(n) = self.since {
            self.since = Some(n.saturating_add(1));
        }
        self.onset = onset;
        if let Some(n) = self.elapsed {
            self.elapsed = (n < self.hit_len).then_some(n + 1);
        }
    }

    /// The fast envelope: the level as the detector hears it now (linear).
    pub fn level(&self) -> f64 {
        self.fast
    }

    /// The attack weight w (0…1) the last [`Self::gain_db`] worked with: how much of the attack
    /// amount applies now.
    pub fn weight(&self) -> f64 {
        self.weight
    }

    /// Samples since the last hit started (`None` before the first).
    pub fn since_hit(&self) -> Option<u32> {
        self.since
    }

    /// Samples since the current hit started, while it is under way.
    pub fn hit_age(&self) -> Option<u32> {
        self.elapsed
    }

    /// The sharpest the current hit has been (0…1, as of the last [`Self::gain_db`]).
    pub fn sharpness(&self) -> f64 {
        self.sharpness
    }

    /// The gain for these amounts right now, dB. Call it at the control rate; it tracks the hit's
    /// peak sharpness.
    pub fn gain_db(&mut self, transient_db: f64, sustain_db: f64) -> f64 {
        let db = |x: f64| 20.0 * x.max(1e-10).log10();
        let lead = db(self.fast) - db(self.rise);
        if self.elapsed.is_some() {
            let u = (lead / SHARP_FULL_DB).clamp(0.0, 1.0);
            self.sharpness = self.sharpness.max(u * u * (3.0 - 2.0 * u));
        }
        // Smooth from 0: under the knee nothing moves, well over it the amount follows the lead.
        let weight = |d: f64, full: f64| (((d * d + SHAPE_KNEE_DB * SHAPE_KNEE_DB).sqrt() - SHAPE_KNEE_DB) / full).tanh();
        let w = weight(lead.max(0.0), ATTACK_FULL_DB);
        let v = weight(db(self.decay).max(0.0), SUSTAIN_FULL_DB);
        self.weight = w;
        let gate = ((db(self.fast) - SHAPE_GATE_DB) / 20.0).clamp(0.0, 1.0);
        gate * (transient_db * w + sustain_db * v * (1.0 - w))
    }
}

/// How quickly a dynamic band's curve (threshold, ratio, range, transient amounts) follows its
/// settings: the time constant, ms. Dragging any of them glides the gain instead of stepping it.
pub const CURVE_SMOOTH_MS: f64 = 20.0;

/// Soft knee width of the dynamic gain computer, dB.
const DYNAMIC_KNEE_DB: f64 = 6.0;

/// How far past the threshold (after the knee and the ratio) a compressor band reaches its whole
/// range, dB. The range is a depth: whatever its size, the band gets there once the level is far
/// enough over the threshold, so pulling the range further always moves the band further.
pub const FULL_RANGE_DB: f64 = 6.0;
/// Width of the soft top where the movement settles into the full range, as a share of it.
const RANGE_KNEE: f64 = 0.25;

impl Dynamics {
    /// Dynamic gain in dB for a detected level in dBFS: from 0 at the threshold (soft knee) to
    /// the whole range, eased into it at the top, so the curve has no corner.
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
        let u = over * (1.0 - 1.0 / self.ratio.max(1.0)) / FULL_RANGE_DB;
        let h = RANGE_KNEE;
        let reach = if u <= 1.0 - h {
            u
        } else if u >= 1.0 + h {
            1.0
        } else {
            u - (u - (1.0 - h)).powi(2) / (4.0 * h)
        };
        reach * self.range_db
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
        BandType::FlatTilt => {
            // A constant dB/oct slope from first-order pole/zero pairs spread evenly in log
            // frequency (a pinking filter): each pair adds 6 dB/oct over the part of its interval
            // between its zero and its pole. The slope is the gain over half the plot's range
            // (so a pivot in the middle reaches ± the gain at its ends); the pivot (the band
            // frequency) stays at unity.
            let octaves = (FREQ_MAX / FREQ_MIN).log2() / 2.0;
            let slope = s.gain_db.clamp(-GAIN_RANGE_DB, GAIN_RANGE_DB) / octaves;
            let (lo, hi) = (FLAT_TILT_SPAN.0.log2(), FLAT_TILT_SPAN.1.log2());
            let spacing = (hi - lo) / FLAT_TILT_PAIRS as f64;
            let width = slope / (20.0 * 2f64.log10()) * spacing;
            let f0 = s.freq.clamp(FREQ_MIN, FREQ_MAX);
            let pair = |k: usize| {
                let centre = 2f64.powf(lo + spacing * (k as f64 + 0.5)) / f0;
                (centre * 2f64.powf(-width / 2.0), centre * 2f64.powf(width / 2.0))
            };
            // Unity at the pivot: the pairs' gain there, divided out of the first one.
            let at_pivot: f64 = (0..FLAT_TILT_PAIRS)
                .map(|k| {
                    let (z, p) = pair(k);
                    ((1.0 + 1.0 / (z * z)) / (1.0 + 1.0 / (p * p))).sqrt()
                })
                .product();
            for k in 0..FLAT_TILT_PAIRS {
                let (z, p) = pair(k);
                let g = if k == 0 { 1.0 / at_pivot } else { 1.0 };
                f(Analog::first(g / z, g, 1.0 / p, 1.0), false);
            }
        }
        BandType::AllPass => f(Analog::second(1.0, -1.0 / q, 1.0, 1.0, 1.0 / q, 1.0), false),
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

/// Whether `s` is a cut at the Brickwall slope.
pub fn is_brickwall(s: &BandSettings) -> bool {
    s.kind.has_slope() && s.slope == BRICKWALL
}

/// How wide a Brickwall's edge is, octaves (from unity to the floor, a raised cosine).
const BRICKWALL_EDGE_OCT: f64 = 1.0 / 24.0;
/// A Brickwall's stopband, dB.
const BRICKWALL_FLOOR_DB: f64 = -150.0;

/// The response Linear Phase builds its kernel from, dB: the analog prototype's, except that a
/// Brickwall cut is a vertical edge at its frequency (only a linear-phase kernel can make one;
/// the IIR modes play 96 dB/oct for it).
pub fn linear_phase_response_db(s: &BandSettings, freq: f64) -> f64 {
    if !s.enabled || !is_brickwall(s) {
        return analog_response_db(s, freq);
    }
    let octaves = (freq.max(1e-3) / s.freq.clamp(FREQ_MIN, FREQ_MAX)).log2();
    // Distance into the stopband, octaves: above the corner for a high cut, below it for a low.
    let into = if s.kind == BandType::HighCut { octaves } else { -octaves };
    let t = (into / BRICKWALL_EDGE_OCT + 0.5).clamp(0.0, 1.0);
    if t <= 0.0 {
        return 0.0;
    }
    if t >= 1.0 {
        return BRICKWALL_FLOOR_DB;
    }
    let gain = 0.5 + 0.5 * (std::f64::consts::PI * t).cos();
    (20.0 * gain.max(1e-12).log10()).max(BRICKWALL_FLOOR_DB)
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

/// The detector's rectified output for one sample, `x(c)` being channel `c`'s input (`stereo`:
/// there are two): the louder side for a stereo band, its own side or mid/side otherwise.
#[inline(always)]
fn detector_level(det: &Coeffs, state: &mut [Tdf2; 2], placement: Placement, stereo: bool, x: impl Fn(usize) -> f64) -> f64 {
    match placement {
        Placement::Stereo if stereo => state[0].process(det, x(0)).abs().max(state[1].process(det, x(1)).abs()),
        Placement::Stereo => state[0].process(det, x(0)).abs(),
        Placement::Left => state[0].process(det, x(0)).abs(),
        Placement::Right => state[0].process(det, x(1)).abs(),
        Placement::Mid | Placement::Side => {
            let (l, r) = (x(0), x(1));
            state[0].process(det, if placement == Placement::Mid { 0.5 * (l + r) } else { 0.5 * (l - r) }).abs()
        }
    }
}

/// Samples between coefficient redesigns while a parameter is moving.
const CONTROL_INTERVAL: u32 = 16;
/// While only the dynamic gain moves: the envelope is already smooth, so 0.7 ms at 48 kHz.
const DYNAMIC_INTERVAL: u32 = 32;
/// Dynamic gain changes smaller than this don't redesign the filter, dB.
const DYNAMIC_STEP_DB: f64 = 0.01;
/// A dynamic gain moving less than this per control update has settled, dB.
const DYNAMIC_SETTLED_DB: f64 = 1e-4;
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
    /// The dynamics curve the gain is computed with: the settings' threshold, ratio, range and
    /// transient amounts, eased (see [`CURVE_SMOOTH_MS`]).
    curve: Dynamics,
    dynamic_db: f64,
    /// Dynamic gain the current sections were designed with, and the detector's design key
    /// (frequency, Q, type): redesigns happen only when these move.
    designed_dynamic_db: f64,
    /// The dynamic gain at the previous control update (to tell when it has settled).
    previous_dynamic_db: f64,
    /// Samples since the previous control update (the interval last scheduled).
    interval: u32,
    detector_key: (f64, f64, BandType),
    /// The region the detector listens through when it is another band's (its settings).
    detect_region: Option<BandSettings>,
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
            curve: Dynamics::default(),
            dynamic_db: 0.0,
            designed_dynamic_db: 0.0,
            previous_dynamic_db: 0.0,
            interval: CONTROL_INTERVAL,
            detector_key: (0.0, 0.0, BandType::Bell),
            detect_region: None,
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

    /// How long a change that can't be smoothed (type, slope, on/off) fades out and in (8 ms
    /// unless set; a band inside a feedback loop wants longer, as every echo hears the fade).
    pub fn set_structure_fade_ms(&mut self, ms: f32) {
        self.mix.set_ramp(self.sample_rate as f32, ms);
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
        self.curve = settings.dynamics;
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
        // The detector listens through this band's region, or another band's.
        let region = self.detect_region.unwrap_or(settings);
        let key = (region.freq, region.q, region.kind);
        if self.detecting() && key != self.detector_key {
            self.detector = detector_design(&region, self.sample_rate);
            self.detector_key = key;
        }
    }

    /// Whether the detector runs: dynamics on for a type with gain.
    /// (Not for spectral dynamics: those work per bin in the spectral stage, not in this filter.)
    fn detecting(&self) -> bool {
        self.target.dynamics.enabled && self.active.kind.has_gain() && self.target.kind == self.active.kind && !self.target.is_spectral()
    }

    /// Makes the detector listen through `region` (another band's settings), or the band's own
    /// region with `None`. The engine sets it each block for bands that detect from another one.
    pub fn set_detect_region(&mut self, region: Option<BandSettings>) {
        let region = region.map(|r| BandSettings { enabled: true, ..r });
        if region != self.detect_region {
            self.detect_region = region;
            // Redesigned at the next control update.
            self.detector_key = (0.0, 0.0, BandType::Bell);
            if self.detecting() {
                self.redesign();
            }
        }
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
            self.ease_curve();
            let d = self.curve;
            let raw = match d.mode {
                DynamicsMode::Compressor => d.gain_db(20.0 * self.envelope.max(1e-10).log10()),
                DynamicsMode::Transient => self.shape.gain_db(d.transient_db, d.sustain_db),
            };
            // Gain plus dynamics stays inside the gain range, as the gain dial shows it.
            let gain = f64::from(self.gain.current());
            self.dynamic_db = (gain + raw).clamp(-GAIN_LIMIT_DB, GAIN_LIMIT_DB) - gain;
        } else {
            self.envelope = 0.0;
            self.curve = self.target.dynamics;
            self.shape.reset(0.0);
            self.dynamic_db *= (1.0 - self.release).powi(self.interval as i32);
            if self.dynamic_db.abs() < 1e-4 {
                self.dynamic_db = 0.0;
            }
        }
    }

    /// One control interval's step of the curve towards the settings.
    fn ease_curve(&mut self) {
        let d = self.target.dynamics;
        let k = 1.0 - (-f64::from(self.interval) * 1000.0 / (CURVE_SMOOTH_MS * self.sample_rate)).exp();
        let c = &mut self.curve;
        for (value, target) in [
            (&mut c.threshold_db, d.threshold_db),
            (&mut c.ratio, d.ratio),
            (&mut c.range_db, d.range_db),
            (&mut c.transient_db, d.transient_db),
            (&mut c.sustain_db, d.sustain_db),
        ] {
            *value += (target - *value) * k;
            if (target - *value).abs() < 1e-6 {
                *value = target;
            }
        }
        c.mode = d.mode;
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
        // A glide that ends between two control updates still lands exactly on its target.
        let settled = smoothing && !(self.log_freq.is_smoothing() || self.gain.is_smoothing() || self.log_q.is_smoothing());

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
            if settled {
                self.redesign();
            }
            if self.countdown == 0 {
                if dynamic {
                    self.update_dynamic_gain();
                }
                // Redesign once the gain has moved a step, and once more when it settles short of
                // a step from the last design, so a steady level lands exactly on its gain.
                let off = (self.dynamic_db - self.designed_dynamic_db).abs();
                let settling = (self.dynamic_db - self.previous_dynamic_db).abs() < DYNAMIC_SETTLED_DB;
                self.previous_dynamic_db = self.dynamic_db;
                if smoothing || off >= DYNAMIC_STEP_DB || (settling && off > DYNAMIC_SETTLED_DB) {
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
        self.process_detecting(channels, None);
    }

    /// [`Self::process`], with the detector listening to `detect` (the sidechain, as long as the
    /// channels; one channel serves both sides) instead of the channels themselves.
    pub fn process_detecting(&mut self, channels: &mut [&mut [f32]], detect: Option<&[&[f32]]>) {
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
                // Peak envelope of the band's region of the input it processes (or the sidechain).
                let level = match detect {
                    Some(d) => {
                        let last = d.len().max(1) - 1;
                        let get = |c: usize| d.get(c.min(last)).map_or(0.0, |ch| f64::from(ch[i]));
                        detector_level(&self.detector, &mut self.detector_state, placement, d.len() >= 2, get)
                    }
                    None => detector_level(&self.detector, &mut self.detector_state, placement, channels.len() >= 2, |c| f64::from(channels[c][i])),
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
    fn the_whole_dynamic_range_is_reachable_and_the_curve_has_no_corner() {
        for range_db in [-3.0, -6.0, -12.0, -24.0, 18.0] {
            let d = Dynamics { enabled: true, threshold_db: -30.0, ratio: 3.0, range_db, ..Default::default() };
            // Well over the threshold the band moves its whole range, however large.
            assert!((d.gain_db(-10.0) - range_db).abs() < 1e-9, "{range_db}: {}", d.gain_db(-10.0));
            // Below the knee nothing moves; in between it grows smoothly and never overshoots.
            assert_eq!(d.gain_db(-40.0), 0.0);
            let mut last = 0.0;
            let mut slope = 0.0;
            for i in 0..=400 {
                let level = -40.0 + 0.1 * f64::from(i);
                let g = d.gain_db(level);
                assert!(g.abs() <= range_db.abs() + 1e-9 && g.abs() >= last - 1e-12, "{range_db} at {level}");
                let next = (d.gain_db(level + 0.1) - g).abs() / 0.1;
                assert!((next - slope).abs() < 0.08 * range_db.abs().max(1.0), "{range_db}: corner at {level}");
                (last, slope) = (g.abs(), next);
            }
        }
    }

    /// The level change of a 1 kHz band (`range` −12 dB) on `main`, its detector listening to
    /// `detect` (or to `main` itself) and, optionally, through `region`.
    fn ducking(main: &[f32], detect: Option<&[f32]>, region: Option<BandSettings>) -> f64 {
        let mut s = band(BandType::Bell, 1000.0, 0.0, 1.0);
        s.dynamics = Dynamics { enabled: true, threshold_db: -20.0, ratio: 10.0, range_db: -12.0, ..Default::default() };
        let mut p = BandProcessor::new(FS, 20.0);
        p.reset(&s);
        p.set_detect_region(region);
        let mut buf = main.to_vec();
        match detect {
            Some(d) => p.process_detecting(&mut [&mut buf], Some(&[d])),
            None => p.process(&mut [&mut buf]),
        }
        20.0 * (rms(&buf[24_000..]) / rms(&main[24_000..])).log10()
    }

    #[test]
    fn the_sidechain_drives_the_band() {
        let quiet: Vec<f32> = sine(1000.0, 48_000).iter().map(|v| v * 0.01).collect();
        let loud = sine(1000.0, 48_000);
        let silence = vec![0.0; 48_000];
        // The main signal is far below the threshold; a loud sidechain still ducks the band.
        assert!(ducking(&quiet, None, None).abs() < 0.1);
        assert!(ducking(&quiet, Some(&loud), None) < -10.0);
        // And a silent sidechain leaves a loud signal alone.
        assert!(ducking(&loud, Some(&silence), None).abs() < 0.1);
    }

    #[test]
    fn a_steady_level_lands_exactly_on_its_gain() {
        // A tone climbing 1 dB every 0.4 s far over the threshold (the comparison's test): at
        // each step the band settles on its gain, not a redesign step short of it.
        let fs = 44_100.0;
        let f = fs / 50.0;
        let mut s = band(BandType::Bell, f, 0.0, 1.0);
        s.dynamics = Dynamics { enabled: true, threshold_db: -30.0, range_db: -12.0, ..Default::default() };
        let mut p = BandProcessor::new(fs, 20.0);
        p.reset(&s);
        let step = (0.4 * fs) as usize;
        for (k, level_db) in (-20..=0).enumerate() {
            let a = 10f64.powf(f64::from(level_db) / 20.0);
            let x: Vec<f32> = (0..step).map(|i| (a * (std::f64::consts::TAU * f * (k * step + i) as f64 / fs).sin()) as f32).collect();
            let mut y = x.clone();
            p.process(&mut [&mut y]);
            let want = s.dynamics.gain_db(f64::from(level_db) - 3.0103);
            let got = 20.0 * (rms(&y[step * 6 / 10..]) / rms(&x[step * 6 / 10..])).log10();
            if level_db >= -10 {
                assert!((got + 12.0).abs() < 0.002, "{level_db} dBFS: {got:.4} dB (curve {want:.4})");
            }
        }
    }

    #[test]
    fn a_band_can_listen_through_another_bands_region() {
        // The band works at 1 kHz but listens at 8 kHz: a loud 1 kHz tone alone (about −30 dB
        // through the 8 kHz region, under the −20 dB threshold) does not move it; with loud 8 kHz
        // content as well, it ducks.
        let region = Some(band(BandType::Bell, 8000.0, 0.0, 2.0));
        let tone = sine(1000.0, 48_000);
        // The 8 kHz part (about −12 dBFS, over the threshold) passes untouched, so it is kept
        // smaller than the tone to leave the duck readable in the overall level.
        let both: Vec<f32> = tone.iter().zip(sine(8000.0, 48_000)).map(|(a, b)| a + 0.5 * b).collect();
        assert!(ducking(&tone, None, region).abs() < 0.3, "{}", ducking(&tone, None, region));
        assert!(ducking(&both, None, region) < -3.0, "{}", ducking(&both, None, region));
        // Without the region the band hears its own 1 kHz and ducks the tone.
        assert!(ducking(&tone, None, None) < -10.0);
    }

    #[test]
    fn changing_the_dynamic_range_glides_the_gain() {
        // A loud tone held well over the threshold: the band sits at its full range. Dragging the
        // range from −6 to −12 dB must move the gain smoothly, not in one jump.
        let mut s = band(BandType::Bell, 1000.0, 0.0, 1.0);
        s.dynamics = Dynamics { enabled: true, threshold_db: -40.0, ratio: 20.0, range_db: -6.0, ..Default::default() };
        let mut p = BandProcessor::new(FS, 20.0);
        p.reset(&s);
        let mut buf = sine(1000.0, 24_000);
        p.process(&mut [&mut buf]);
        let before = p.dynamic_gain_db();
        assert!((before + 6.0).abs() < 0.1, "{before}");
        p.set_target(&BandSettings { dynamics: Dynamics { range_db: -12.0, ..s.dynamics }, ..s });
        let tone = sine(1000.0, 4800);
        let (mut last, mut largest) = (before, 0.0f64);
        for chunk in tone.chunks(16) {
            let mut c = chunk.to_vec();
            p.process(&mut [&mut c]);
            let now = p.dynamic_gain_db();
            largest = largest.max((now - last).abs());
            last = now;
        }
        assert!((last + 12.0).abs() < 0.1, "{last}");
        assert!(largest < 0.6, "largest step {largest} dB");
    }

    #[test]
    fn a_gain_glide_lands_exactly_on_its_target() {
        // 20 ms glides end between two coefficient updates; the last update must still happen.
        for (from, to) in [(6.0, 0.0), (0.0, -6.0), (-3.0, 4.5)] {
            let mut p = BandProcessor::new(FS, 20.0);
            p.reset(&band(BandType::Bell, 1000.0, from, 1.0));
            p.set_target(&band(BandType::Bell, 1000.0, to, 1.0));
            let input = sine(1000.0, 48_000);
            let mut buf = input.clone();
            p.process(&mut [&mut buf]);
            let gain = 20.0 * (rms(&buf[24_000..]) / rms(&input[24_000..])).log10();
            assert!((gain - to).abs() < 0.005, "{from} → {to}: {gain}");
        }
    }

    #[test]
    fn a_flat_tilt_is_one_straight_slope_through_its_pivot() {
        for gain in [-12.0, 6.0, 18.0] {
            let s = band(BandType::FlatTilt, 1000.0, gain, 1.0);
            let slope = gain / ((FREQ_MAX / FREQ_MIN).log2() / 2.0);
            assert!(analog_response_db(&s, 1000.0).abs() < 1e-9);
            let mut f = 20.0;
            while f < 20_000.0 {
                let off = analog_response_db(&s, f) - slope * (f / 1000.0).log2();
                assert!(off.abs() < 0.3, "{gain} dB at {f:.0} Hz: {off:+.2} dB off the line");
                f *= 1.1;
            }
        }
    }

    #[test]
    fn an_all_pass_keeps_the_level_and_turns_the_phase_half_way_at_its_frequency() {
        let s = band(BandType::AllPass, 2000.0, 0.0, 1.0);
        let sections = design(&s, FS);
        let w0 = 2.0 * PI * 2000.0 / FS;
        for i in 1..200 {
            let w = PI * f64::from(i) / 200.0;
            assert!(sections.response_db(w).abs() < 1e-9, "level {} at {w:.2}", sections.response_db(w));
        }
        // Analog: −180° at the centre; the digital one with it, within a few degrees.
        let phase = |h: realfft::num_complex::Complex<f64>| h.arg().to_degrees().abs();
        assert!((phase(analog_response(&s, 2000.0)) - 180.0).abs() < 1e-6);
        assert!((phase(sections.response(w0)) - 180.0).abs() < 3.0, "{}", phase(sections.response(w0)));
    }

    #[test]
    fn a_brickwall_cut_is_a_vertical_edge_for_linear_phase_and_96_db_for_the_rest() {
        let s = BandSettings { slope: BRICKWALL, ..band(BandType::HighCut, 1000.0, 0.0, 0.7) };
        assert!(is_brickwall(&s));
        assert_eq!(linear_phase_response_db(&s, 960.0), 0.0);
        assert!(linear_phase_response_db(&s, 1040.0) <= -149.0);
        // The IIR modes play the steepest slope for it.
        let steep = BandSettings { slope: SLOPES_DB.len() - 1, ..s };
        assert_eq!(analog_response_db(&s, 2000.0), analog_response_db(&steep, 2000.0));
        let low = BandSettings { kind: BandType::LowCut, ..s };
        assert_eq!(linear_phase_response_db(&low, 1040.0), 0.0);
        assert!(linear_phase_response_db(&low, 960.0) <= -149.0);
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
        // The last coefficient step (at the top of the sweep, where it moves furthest in Hz) is
        // the largest, about 0.0103; it is the same 16-sample step as all the others.
        assert!(worst < 0.0125, "{worst}");
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
        // Nothing while the fast envelope holds its peak, then rising with the decay to the amount.
        assert!(s[100..118].iter().all(|g| g.abs() < 0.1), "sustain during the hit: {:?}", &s[100..118]);
        let held = s[200..280].iter().copied().fold(f64::MAX, f64::min);
        assert!(held > 8.5, "sustain sagged to {held} dB in the tail");
        assert!(s[118..180].windows(2).all(|w| w[1] >= w[0] - 1e-3), "not monotonic: {:?}", &s[118..180]);
    }

    #[test]
    fn cutting_both_attack_and_sustain_never_goes_past_either_amount() {
        let t = shape_trace(&transient_bell(-9.0, -6.0));
        let worst = t[102..300].iter().copied().fold(f64::MIN, f64::max);
        assert!(worst <= 0.0, "the band rose to {worst} dB");
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
