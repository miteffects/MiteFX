//! Mite Sat's engine (BLUEPRINT §6.7, `docs/design/sat-brief.md`): saturation on one band or
//! split into up to six by the shared LR4 crossover (`crate::crossover`), each band with its own
//! model, drive, bias, tone, mix and output, oversampled 1/2/4/8/16× (4× by default).
//!
//! A band's path: `u = g·x + b`, `F(x) = f(u) − f(b)` at the high rate, back down, through the
//! tone tilt, then `y = x_d + a·HP₅(wet − x_d)` with `x_d` the band delayed by the oversampler's
//! round trip and `a = smoothstep(drive / 6 dB) · band mix · engaged`. So drive 0, band bypass
//! and band mix 0 are the band bit for bit; the 5 Hz high-pass on the difference takes out the
//! DC a bias makes without touching the band's lows. A band at `a = 0` skips the oversampler.
//!
//! Tape and Transformer run a Jiles–Atherton hysteresis core (`crate::hysteresis`) between an
//! emphasis filter and its exact inverse: Tape lifts the highs into the core (they saturate
//! first, as on a tape machine), Transformer the lows (the core's flux is the integral of the
//! voltage). Quiet signals come out as they went in.
//!
//! After the model, each band has its own dynamics (`dynamics_gain_db`): its detector hears the
//! band going in (before drive), so the threshold stays where it is as drive moves; below 0 it
//! compresses, above 0 it lifts what passes the threshold (an upward expander, so hits squashed by
//! the curve come back). Amount 0 and band bypass are the band bit for bit.
//!
//! Every curve is smooth (C¹): the oversampler does the anti-aliasing. First-order ADAA was
//! dropped (sat-brief §1.2): on the nonlinear part alone it leaks `g·Δu/2` at high drive; on the
//! whole curve its half-sample delay combs against the dry path.

use std::sync::OnceLock;

use crate::crossover::{Cross, Split};
use crate::dynamics::{soft_above, soft_cap};
use crate::hysteresis::{self, Ja, JaParams, anhysteretic};
use crate::oversample::{Filter, Oversampler};

pub use crate::crossover::{FREQ_MAX, FREQ_MIN, MIN_GAP_OCT};

pub const BANDS: usize = 6;
pub const CROSSOVERS: usize = BANDS - 1;
pub const DRIVE_MAX_DB: f32 = 60.0;
/// The saturation fades in over the first this many dB of drive.
pub const ENGAGE_DB: f32 = 6.0;
/// Bias ±1 (±100 %) moves the operating point this far, plus [`BIAS_TRACK`] of the signal's peak.
pub const BIAS_RANGE: f32 = 0.5;
/// Bias ±1 also moves the operating point this share of the peak going into the curve (its
/// envelope), so the asymmetry holds however hard the band is driven: a fixed offset is lost under
/// a signal 60 times its size, and a curve clipped on both sides at equal times makes no evens.
pub const BIAS_TRACK: f32 = 0.5;
/// Tube: the operating point drifts down by this share of the peak going in (grid-current
/// blocking), with no ceiling, so it stays asymmetric at any drive.
pub const TUBE_DRIFT: f32 = 0.2;
/// Where the smooth `|u|` the point grows with ([`operating_point`]) turns from `u²` to `|u|`.
const POINT_KNEE: f32 = 1.0;
pub const TONE_MAX_DB: f32 = 12.0;
/// Feedback (as Saturn 2's): the band's saturated signal back into its own curve through a
/// resonant band-pass, so it rings at the feedback frequency. It joins after the drive, in the
/// curve's units, and 100 % is this loop gain at the resonance: under 1 even where a curve is
/// steepest (Tube 1.05), so the loop rings but never runs away on its own, at any drive.
pub const FEEDBACK_LOOP: f32 = 0.9;
/// The feedback band-pass's Q (its own ring before the loop adds to it).
pub const FEEDBACK_Q: f32 = 2.0;
pub const FEEDBACK_HZ: (f32, f32) = (20.0, 10_000.0);
/// The dynamics' knee (dB of input, centred on the threshold) and the most it lifts, dB.
pub const DYN_KNEE_DB: f32 = 6.0;
pub const DYN_LIFT_MAX_DB: f32 = 12.0;
/// The dynamics detector: instant attack, this release (so a waveform's zero crossings are not
/// silence), ms.
const DYN_PEAK_MS: f64 = 10.0;
/// The dynamics' threshold is the band's own level, the detector's reading in dB averaged over
/// this long (ms); below [`DYN_FLOOR_DB`] (silence, a tail's last breath) it holds.
const DYN_REF_MS: f64 = 1000.0;
pub const DYN_FLOOR_DB: f32 = -70.0;
/// Where each crossover slot sits when it is first made, Hz.
pub const DEFAULT_CROSSOVERS: [f32; CROSSOVERS] = [120.0, 500.0, 2000.0, 6000.0, 12_000.0];
/// The tone tilt's pivot with one band, Hz (with several, each band's geometric centre).
pub const TONE_PIVOT: f32 = 800.0;
/// Oversampling factors offered (the `oversampling` parameter's choices).
pub const FACTORS: [usize; 5] = [1, 2, 4, 8, 16];

/// Samples between control updates (values ramp linearly in between).
const CONTROL: usize = 16;
/// Drive, bias, tone, mix and output glide with this time constant, ms.
const GLIDE_MS: f64 = 20.0;
/// Band bypass, mute and solo fade, ms.
const SWITCH_MS: f64 = 10.0;
/// A model change crossfades between the two curves over this long, ms.
const MODEL_MS: f64 = 20.0;
/// A band made or removed crossfades between the old and new split over this long, ms.
const STRUCTURE_MS: f64 = 10.0;
/// The difference's high-pass, Hz.
const HP_HZ: f64 = 5.0;
/// The band level auto gain reads (mean square), ms. Slow, so the makeup holds through a song's
/// phrases instead of riding them (300 ms moved it ±0.5 dB between loud and quiet stretches,
/// 2026-11-24); it starts as a plain average of what it has heard, so it settles within the
/// first second all the same.
const LEVEL_MS: f64 = 2000.0;
/// Auto gain's measured trim: mean squares over this long (s), corrected this far (dB).
const TRIM_SECONDS: f64 = 1.5;
const TRIM_LIMIT_DB: f64 = 12.0;
/// Tape and Transformer: their emphasis makes the prediction coarser (it depends on the band's
/// spectrum), so the trim reaches further.
const TRIM_LIMIT_MAGNET_DB: f64 = 12.0;
/// Auto gain never moves a band by more than this, dB.
const AUTO_LIMIT_DB: f64 = 24.0;
/// Below this mean square (−80 dBFS) the trim is not measured.
const TRIM_GATE: f64 = 1e-8;
/// Auto gain matches the music, not the gaps in it: its meters (the prediction's level and the
/// trim's) only run while the band's short-term level (mean square over this long, ms) is within
/// [`HIT_GATE_DB`] of its recent loudest, which falls back with this time constant (ms). On sparse
/// material (a kick, speech) the tails and bleed between hits are left out: saturation lifts them
/// by the whole drive, and matching them pulled the hits down by as much. 5 ms: at 20 ms the
/// gate closed late after each loud stretch and let its quiet start into the meters.
const SHORT_MS: f64 = 5.0;
const TOP_RELEASE_MS: f64 = 1000.0;
const HIT_GATE_DB: f64 = 10.0;
/// The envelope the operating point follows (the peak going into the curve): attack and release, ms.
pub const ENV_ATTACK_MS: f64 = 10.0;
pub const ENV_RELEASE_MS: f64 = 150.0;
/// Tape and Transformer: the core's rest magnetisation is worked out again once the operating
/// point has moved this far (times `1 + |point|`), and interpolated along its slope in between.
const REST_STEP: f32 = 0.01;
/// Longest stretch processed at once: one control step, so every stretch starts on one and the
/// meters a step reads are those of the whole stretch before (the output is the same for every
/// host block size).
const CHUNK: usize = CONTROL;
/// A band waking up runs this many of its past samples through the oversampler first (more than
/// the longest half-band's history).
const PREROLL: usize = 80;
/// Each channel's stretch at the high rate in the scratch: a chunk at 8×.
/// A channel's room at the high rate: a chunk at the largest factor (16×, added 2026-11-24).
const HI_CH: usize = CHUNK * 16;

/// The saturation models, in their frozen parameter order (append only).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Model {
    Tape,
    Tube,
    Transformer,
    Diode,
    Soft,
    Hard,
    Fold,
    Rectify,
    /// Drawn harmonics (2026-11-24): a Chebyshev mix with the 2nd…9th harmonics the band's
    /// recipe asks for at full scale ([`recipe`]); appended, so saved indices keep their models.
    Harmonic,
}

impl Model {
    pub const ALL: [Model; 9] = [
        Model::Tape,
        Model::Tube,
        Model::Transformer,
        Model::Diode,
        Model::Soft,
        Model::Hard,
        Model::Fold,
        Model::Rectify,
        Model::Harmonic,
    ];
    pub const NAMES: [&'static str; 9] = [
        "Tape",
        "Tube",
        "Transformer",
        "Diode",
        "Soft",
        "Hard",
        "Fold",
        "Rectify",
        "Harmonic",
    ];
    pub const DEFAULT: Model = Model::Tube;

    pub fn from_index(i: usize) -> Self {
        Self::ALL.get(i).copied().unwrap_or(Self::DEFAULT)
    }

    pub fn index(self) -> usize {
        self as usize
    }

    pub fn name(self) -> &'static str {
        Self::NAMES[self.index()]
    }

    /// Which hysteresis core a model runs (Tape 0, Transformer 1), if any.
    pub fn magnet(self) -> Option<usize> {
        match self {
            Model::Tape => Some(0),
            Model::Transformer => Some(1),
            _ => None,
        }
    }

    /// Whether the operating point follows the signal's envelope ([`operating_point`]): every
    /// model that saturates. Fold's and Rectify's bias is a place on a curve that never flattens,
    /// so it stays put (a moving one would sweep through the folds).
    pub fn tracks(self) -> bool {
        !matches!(self, Model::Fold | Model::Rectify)
    }
}

/// The hysteresis cores, by [`Model::magnet`].
const MAGNETS: [JaParams; 2] = [hysteresis::TAPE, hysteresis::TRANSFORMER];
/// Tape's emphasis: up to +18 dB above about 2 kHz into the core; Transformer's: up to +18 dB
/// below about 120 Hz (tilt and pivot, as [`Tilt::new`] takes them, then half the tilt as gain,
/// so the other end stays at 0 dB).
const EMPHASIS: [(f32, f32); 2] = [(18.0, 2000.0), (-18.0, 120.0)];

/// The model's static curve `f(u)`: `f(0) = 0`, smooth, slope 1 at 0 (Rectify: 0, it trades the
/// fundamental for the octave). Tape and Transformer: their core's anhysteretic curve (what the
/// loop closes onto); they play their loop ([`Magnet`]).
#[inline]
pub fn curve(model: Model, u: f32) -> f32 {
    if let Some(w) = model.magnet() {
        let p = &MAGNETS[w];
        return anhysteretic(p, p.h_in * u) * (3.0 - p.coupling) / p.h_in;
    }
    match model {
        // One tanh, off centre: analytic, so its harmonics fall away fast at any level. The
        // two-piece curve it replaced (tanh above zero, an exponential below) jumped in
        // curvature at zero: like a slight rectifier, it kept the 5th harmonic and up at −30 dB
        // against the 2nd–4th even at a whisper (Saturn 2's Warm Tube: −68 dB; 2026-11-24).
        // Same ceilings (+0.8, −1.25) and slope 1 at zero.
        Model::Tube => ((TUBE_A * (u + TUBE_C)).tanh() - TUBE_T) * TUBE_S,
        // An unmatched pair: one diode conducts the top, two in series the bottom, so the halves
        // differ in shape at any level (even harmonics without bias).
        Model::Diode => {
            let k = if u > 0.0 {
                DIODE_KNEES.0
            } else {
                DIODE_KNEES.1
            };
            k * (u / k).asinh()
        }
        // A pure cubic up to its clip: only the 3rd harmonic until the swing reaches ±1.5
        // (measured purer than Saturn 2's Gentle Saturation up to 10 % THD+N; a tanh-smoothed
        // limit added a 5th-order term everywhere and was 60 dB harsher at moderate levels).
        Model::Tape | Model::Transformer | Model::Soft => {
            if u.abs() < 1.5 {
                u - 4.0 / 27.0 * u * u * u
            } else {
                u.signum()
            }
        }
        Model::Hard => {
            let a = u.abs();
            let y = if a <= 0.95 {
                a
            } else if a < 1.05 {
                a - (a - 0.95) * (a - 0.95) / 0.2
            } else {
                1.0
            };
            y.copysign(u)
        }
        Model::Fold => u.sin(),
        Model::Rectify => ((u * u + 0.0025).sqrt() - 0.05).tanh(),
        // Without a recipe: the clamp alone (what the gain tables measure; the band runs
        // [`curve_with`]).
        Model::Harmonic => harmonic(u, &[0.0; HARMONICS]),
    }
}

/// Where drive puts the curves' knees: 0 dB drive takes a signal in at −12 dB, so a −12 dBFS
/// track starts about 24 dB under them, gently (Tube under 1 % THD+N), and reaches them near
/// 24 dB (2026-11-24, state 5). It goes in before the curve and comes off after it, so quiet
/// signals keep slope `g` and only the knees move. It used to be +12 dB: a −12 dBFS track met
/// the knees at 0 dB and the first 6 dB faded a hard-driven sound in as a mix, so there was no
/// gentle range (docs/compare/saturn-2.md).
pub const CALIBRATION: f32 = 0.251_188_64;

/// Drive below this (dB) moves only the knee, not the level: the curve's output comes off by the
/// drive it took in, so quiet signals pass at unity and drive is how close to saturation. From
/// here up each dB of drive is a dB of gain as well, as every setting was under the old law (old
/// 0 dB is new 24), so a migrated session keeps its level with auto gain off too.
pub const LEVEL_DB: f32 = 24.0;

/// What the curve's output (after `/ CALIBRATION`) is scaled by at `drive_db`: the drive under
/// [`LEVEL_DB`] taken back off.
#[inline]
pub fn out_scale(drive_db: f32) -> f32 {
    crate::db_to_gain(-drive_db.clamp(0.0, LEVEL_DB))
}

/// The share of the saturation heard at `drive_db` (0 at 0 dB, 1 from [`ENGAGE_DB`], S-shaped).
#[inline]
pub fn engagement(drive_db: f32) -> f32 {
    let t = (drive_db / ENGAGE_DB).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// The harmonics a drawn recipe sets: the 2nd to the 9th.
pub const HARMONICS: usize = 8;
/// A recipe's levels run from here (off) to 0 dB against the fundamental.
pub const HARMONIC_FLOOR_DB: f32 = -72.0;
/// A drawn recipe as the curve uses it: the Chebyshev weight of each harmonic, 2nd to 9th.
pub type Recipe = [f32; HARMONICS];
/// Harmonic's input clamp: the identity to ±1 (so the recipe is exact for a sine reaching it),
/// then onto ±(1 + this) with value, slope and curvature matched, so the curve is bounded past it.
const HARMONIC_OVER: f32 = 0.05;

/// The weights for target levels `db` (2nd…9th, dB against the fundamental at full scale):
/// `P(x) = x + Σ aₖ·(Tₖ(x) − Tₖ(0) − Tₖ'(0)·x)` gives a full-scale sine harmonic k at `aₖ` and a
/// fundamental `F = 1 + Σ cₖ·aₖ` (`cₖ = −Tₖ'(0)`: 3, −5, 7, −9 for the odd ones), so `aₖ = rₖ·F`
/// with `F = 1/(1 − Σ cₖ·rₖ)` (scaled back where the odd ones would cancel the fundamental).
pub fn recipe(db: &[f32; HARMONICS]) -> Recipe {
    let mut r: [f64; HARMONICS] = std::array::from_fn(|i| {
        if db[i] <= HARMONIC_FLOOR_DB + 0.5 {
            0.0
        } else {
            10f64.powf(f64::from(db[i].min(0.0)) / 20.0)
        }
    });
    let c = |i: usize| {
        let k = i + 2;
        if k % 2 == 0 { 0.0 } else { -(k as f64) * ((k as f64) * std::f64::consts::FRAC_PI_2).sin() }
    };
    let mut sum: f64 = (0..HARMONICS).map(|i| c(i) * r[i]).sum();
    if sum > 0.75 {
        for v in &mut r {
            *v *= 0.75 / sum;
        }
        sum = 0.75;
    }
    let f = 1.0 / (1.0 - sum);
    std::array::from_fn(|i| (r[i] * f) as f32)
}

/// Harmonic's curve for recipe `a`: the clamp, then the Chebyshev mix. Slope 1 and 0 at 0.
#[inline]
pub fn harmonic(u: f32, a: &Recipe) -> f32 {
    let m = u.abs();
    let x = if m <= 1.0 {
        u
    } else {
        let h = HARMONIC_OVER;
        let al = 1.0 / h;
        let d = m - 1.0;
        (1.0 + h - h * (-al * d - 0.5 * al * al * d * d).exp()).copysign(u)
    };
    // Tₖ by the recurrence; Tₖ(0) = cos(kπ/2), Tₖ'(0) = k·sin(kπ/2).
    let (mut t0, mut t1) = (1.0f32, x);
    let mut y = x;
    for (i, &w) in a.iter().enumerate() {
        let t2 = 2.0 * x * t1 - t0;
        (t0, t1) = (t1, t2);
        if w != 0.0 {
            let k = i + 2;
            let (at0, slope) = match k % 4 {
                0 => (1.0, 0.0),
                1 => (0.0, k as f32),
                2 => (-1.0, 0.0),
                _ => (0.0, -(k as f32)),
            };
            y += w * (t2 - at0 - slope * x);
        }
    }
    y
}

/// The steepest Harmonic's curve gets for recipe `a` (sampled over ±1; past it the clamp only
/// flattens it): feedback scales its loop by it, so a drawn curve cannot run away either.
pub fn harmonic_max_slope(a: &Recipe) -> f32 {
    let n = 128;
    let h = 1.0 / n as f32;
    (0..=2 * n)
        .map(|i| {
            let x = -1.0 + i as f32 * h;
            ((harmonic((x + 0.5 * h).min(1.0), a) - harmonic((x - 0.5 * h).max(-1.0), a)) / h).abs()
        })
        .fold(1.0f32, f32::max)
}

/// [`curve`], with Harmonic's recipe.
#[inline]
pub fn curve_with(model: Model, u: f32, a: &Recipe) -> f32 {
    if model == Model::Harmonic {
        harmonic(u, a)
    } else {
        curve(model, u)
    }
}

/// Tube's curve `(tanh(A·(u + C)) − T)·S`: `T = tanh(A·C)` puts it through 0, `S = 1/(A·(1 − T²))`
/// gives slope 1 there, and `T = 0.5625/2.5625`, `A = 1/(0.8·(1 + T))` set the ceilings to +0.8
/// and −1.25 (the two-piece curve's).
const TUBE_T: f32 = 0.219_512_2;
const TUBE_A: f32 = 1.025;
const TUBE_C: f32 = 0.217_701_03;
const TUBE_S: f32 = 1.025;

/// Diode: the knees of its top and bottom halves (one diode against two).
const DIODE_KNEES: (f32, f32) = (0.3, 0.6);

/// The operating point a bias setting (−1…1) gives in silence.
#[inline]
pub fn bias_point(bias: f32) -> f32 {
    bias.clamp(-1.0, 1.0) * BIAS_RANGE
}

/// The operating point for the driven sample `u` at bias `bias` (−1…1): [`bias_point`], then for
/// a model that [`tracks`](Model::tracks) a share of the sample's own size (a smooth `|u|`), less
/// Tube's drift. It grows with the signal, so the asymmetry holds at any drive, and it has no
/// memory: the curve is static. It followed the envelope (150 ms release) until 2026-11-24, and
/// after a loud passage held the point out where the quiet audio after it fell on the curve's flat
/// part: muted by up to 25 dB for 150 ms, a thump with it (measured, state 5 kept the change).
#[inline]
pub fn operating_point(model: Model, bias: f32, u: f32) -> f32 {
    let bias = bias.clamp(-1.0, 1.0);
    let mut k = 0.0;
    if model.tracks() {
        k += bias * BIAS_TRACK;
    }
    if model == Model::Tube {
        k -= TUBE_DRIFT;
    }
    bias * BIAS_RANGE + k * ((u * u + POINT_KNEE * POINT_KNEE).sqrt() - POINT_KNEE)
}

/// One step of the envelope follower: `env` toward `|x|` with coefficients (attack, release).
#[inline]
pub fn follow(env: &mut f32, x: f32, k: (f32, f32)) {
    let a = x.abs();
    *env += (a - *env) * if a > *env { k.0 } else { k.1 };
}

/// Where the follower starts a cycle of the periodic signal `x` once settled: the
/// start one cycle maps back onto itself, by bisection (the map is monotone, and from rest the
/// envelope only climbs, from the peak only falls). The release would take hundreds of cycles
/// to get there by running.
pub fn settled_env(x: &[f32], k: (f32, f32)) -> f32 {
    let peak = x.iter().fold(0.0f32, |m, v| m.max(v.abs()));
    let (mut lo, mut hi) = (0.0f32, peak);
    // 2⁻²⁰ of the peak: far finer than the point needs.
    for _ in 0..20 {
        let mid = 0.5 * (lo + hi);
        let mut env = mid;
        for &v in x {
            follow(&mut env, v, k);
        }
        if env > mid {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    0.5 * (lo + hi)
}

/// The follower's (attack, release) coefficients at `rate`.
pub fn env_coefficients(rate: f64) -> (f32, f32) {
    let per = |ms: f64| (1.0 - (-1000.0 / (ms * rate)).exp()) as f32;
    (per(ENV_ATTACK_MS), per(ENV_RELEASE_MS))
}

/// A static model's `F` for the driven sample `u0` at bias `bias` (Harmonic with recipe `a`): 0
/// at 0.
#[inline]
fn static_lane(model: Model, u0: f32, bias: f32, a: &Recipe) -> f32 {
    (curve_with(model, u0 + operating_point(model, bias, u0), a) - curve_with(model, bias_point(bias), a)) / CALIBRATION
}

/// The band's static transfer at input `x`: `x + a·(F(x) − x)` with `a` = [`engagement`] (before
/// tone, mix and gains), at bias `bias`.
#[inline]
pub fn transfer(model: Model, x: f32, drive_db: f32, bias: f32) -> f32 {
    transfer_with(model, x, drive_db, bias, &[0.0; HARMONICS])
}

/// [`transfer`] with Harmonic's recipe.
#[inline]
pub fn transfer_with(model: Model, x: f32, drive_db: f32, bias: f32, recipe: &Recipe) -> f32 {
    let g = crate::db_to_gain(drive_db);
    let a = engagement(drive_db);
    x + a * (static_lane(model, CALIBRATION * g * x, bias, recipe) * out_scale(drive_db) - x)
}

// ---- Auto gain ----------------------------------------------------------------------------

/// Sine amplitudes into the curve the gain tables cover, dB (1 dB steps).
const TABLE_U_DB: (f64, f64) = (-60.0, 48.0);
const TABLE_U: usize = 109;
/// Bias settings the tables cover (−1…1, 0.2 steps).
const TABLE_B: usize = 11;
/// The envelope in a table's measurement: the peak at once, held.
const HELD_PEAK: (f32, f32) = (1.0, 0.0);

/// For one model, a sine of amplitude `U` into the curve at bias `b` (the operating point it
/// gives with the envelope at the sine's peak):
/// `p = ⟨F²⟩ / (U²/2)` and `c = ⟨sin·F⟩ / (U/2)` (both `f'(b)²` and `f'(b)` while it is linear).
struct GainTable {
    p: [f32; TABLE_U * TABLE_B],
    c: [f32; TABLE_U * TABLE_B],
}

fn gain_tables() -> &'static [GainTable; 9] {
    static TABLES: OnceLock<Box<[GainTable; 9]>> = OnceLock::new();
    TABLES.get_or_init(|| {
        Box::new(Model::ALL.map(|m| {
            let mut t = GainTable {
                p: [0.0; TABLE_U * TABLE_B],
                c: [0.0; TABLE_U * TABLE_B],
            };
            let flat = (Tilt::IDENTITY, Tilt::IDENTITY);
            for j in 0..TABLE_B {
                let b = -1.0 + j as f32 * (2.0 / (TABLE_B - 1) as f32);
                for i in 0..TABLE_U {
                    let u = 10f64.powf((TABLE_U_DB.0 + i as f64) / 20.0);
                    let (mut p, mut c, mut mean) = (0.0f64, 0.0f64, 0.0f64);
                    const N: usize = 128;
                    // A hysteresis core is run through a settling cycle first, then measured
                    // on its loop (without the emphasis: the trim measures the rest).
                    let mut magnet = Magnet::default();
                    let settle = if m.magnet().is_some() { N } else { 0 };
                    for k in 0..settle + N {
                        let s = (std::f64::consts::TAU * (k as f64 + 0.5) / N as f64).sin();
                        let f = match m.magnet() {
                            Some(w) => {
                                f64::from(magnet.process(w, &flat, (u * s) as f32, b, HELD_PEAK))
                            }
                            None => {
                                let us = (u * s) as f32;
                                f64::from(curve(m, us + operating_point(m, b, us)))
                                    - f64::from(curve(m, bias_point(b)))
                            }
                        };
                        if k < settle {
                            continue;
                        }
                        p += f * f;
                        c += s * f;
                        mean += f;
                    }
                    // What is heard: the band's high-pass takes the DC an asymmetric curve makes,
                    // so the prediction leaves it out too.
                    let mean = mean / N as f64;
                    p -= N as f64 * mean * mean;
                    t.p[j * TABLE_U + i] = (p / N as f64 / (u * u / 2.0)) as f32;
                    t.c[j * TABLE_U + i] = (c / N as f64 / (u / 2.0)) as f32;
                }
            }
            t
        }))
    })
}

/// The band path's gain (linear, RMS) for a sine of amplitude `amplitude` at `drive_db` and bias
/// `bias` (−1…1), with the saturation at share `a`: the dry part, the curve and the cross term
/// between them.
pub fn predicted_gain(model: Model, amplitude: f32, drive_db: f32, bias: f32, a: f32) -> f32 {
    let g = f64::from(crate::db_to_gain(drive_db));
    let u_db = 20.0 * (f64::from(CALIBRATION) * g * f64::from(amplitude).max(1e-9)).log10();
    let x =
        ((u_db - TABLE_U_DB.0).clamp(0.0, TABLE_U_DB.1 - TABLE_U_DB.0)).min((TABLE_U - 1) as f64);
    let y = ((f64::from(bias) + 1.0) / 2.0 * (TABLE_B - 1) as f64).clamp(0.0, (TABLE_B - 1) as f64);
    let (i, j) = ((x as usize).min(TABLE_U - 2), (y as usize).min(TABLE_B - 2));
    let (fx, fy) = (x - i as f64, y - j as f64);
    let t = &gain_tables()[model.index()];
    let at = |v: &[f32]| {
        let q = |jj: usize, ii: usize| f64::from(v[jj * TABLE_U + ii]);
        (q(j, i) * (1.0 - fx) + q(j, i + 1) * fx) * (1.0 - fy)
            + (q(j + 1, i) * (1.0 - fx) + q(j + 1, i + 1) * fx) * fy
    };
    let (p, c) = (at(&t.p), at(&t.c));
    let a = f64::from(a);
    // The curve's quiet slope: the drive, less what [`out_scale`] takes back.
    let g = g * f64::from(out_scale(drive_db));
    let g2 = (1.0 - a) * (1.0 - a) + 2.0 * a * (1.0 - a) * g * c + a * a * g * g * p;
    g2.max(0.0).sqrt() as f32
}

// ---- Settings ------------------------------------------------------------------------------

/// The band output's L/R and M/S balance glide, s.
const BALANCE_GLIDE_S: f64 = 0.02;

/// What part of the stereo image a band saturates (frozen order, as the `stereo` parameter).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StereoMode {
    #[default]
    Stereo,
    /// The mid goes through the curve, the side stays clean.
    Mid,
    /// The side goes through the curve, the mid stays clean.
    Side,
    /// Mid and side each through the curve of their own.
    MidSide,
    /// The left goes through the curve, the right stays clean.
    Left,
    /// The right goes through the curve, the left stays clean.
    Right,
}

impl StereoMode {
    pub const ALL: [StereoMode; 6] = [
        StereoMode::Stereo,
        StereoMode::Mid,
        StereoMode::Side,
        StereoMode::MidSide,
        StereoMode::Left,
        StereoMode::Right,
    ];

    pub fn from_index(i: usize) -> Self {
        Self::ALL[i.min(Self::ALL.len() - 1)]
    }

    /// Whether the curves see mid and side (else left and right).
    pub fn mid_side(self) -> bool {
        matches!(self, StereoMode::Mid | StereoMode::Side | StereoMode::MidSide)
    }

    /// Which of the two signals (L/R, or mid/side) go through the curve.
    fn driven(self) -> [bool; 2] {
        match self {
            StereoMode::Stereo | StereoMode::MidSide => [true, true],
            StereoMode::Mid | StereoMode::Left => [true, false],
            StereoMode::Side | StereoMode::Right => [false, true],
        }
    }
}

/// One band's settings.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BandSat {
    pub model: Model,
    pub drive_db: f32,
    /// −1…1.
    pub bias: f32,
    pub stereo: StereoMode,
    /// The tilt after the model, dB at the top (minus as much at the bottom).
    pub tone_db: f32,
    /// The band's own dry/wet, 0…1.
    pub mix: f32,
    /// The band's fader in the sum, dB.
    pub out_db: f32,
    /// The dynamics after the model, −1…1: below 0 compresses (−0.5 is 2:1, −1 ∞:1), above 0
    /// lifts what passes the threshold (1 is 1:2, at most [`DYN_LIFT_MAX_DB`]).
    pub dyn_amount: f32,
    /// Saved but not used: the threshold is automatic, the band's own average level (the
    /// parameter was released with the first dynamics, 2026-11-03, so it stays).
    pub dyn_threshold_db: f32,
    pub dyn_attack_ms: f32,
    pub dyn_release_ms: f32,
    /// Clean but heard.
    pub bypass: bool,
    pub mute: bool,
    pub solo: bool,
    /// The band's output balance, −1…1 (0: as it is), as Saturn 2's band output: left (−) or
    /// right (+), then mid (−, at −1 the side is gone: mono) or side (+, at +1 the mid is gone).
    pub lr: f32,
    pub ms: f32,
    /// Feedback, 0…1, and the frequency it rings at, Hz.
    pub feedback: f32,
    pub feedback_hz: f32,
    /// Harmonic's recipe: the 2nd…9th harmonics' levels at full scale, dB against the fundamental
    /// ([`HARMONIC_FLOOR_DB`] is off).
    pub harmonics_db: [f32; HARMONICS],
    /// Extra drive on the hits and on what sustains, dB (the transient detector's weights): how
    /// close to the knee each gets, not how loud it is.
    pub punch_db: f32,
    pub body_db: f32,
}

impl Default for BandSat {
    fn default() -> Self {
        Self {
            model: Model::DEFAULT,
            drive_db: 0.0,
            bias: 0.0,
            lr: 0.0,
            ms: 0.0,
            stereo: StereoMode::Stereo,
            tone_db: 0.0,
            mix: 1.0,
            out_db: 0.0,
            dyn_amount: 0.0,
            dyn_threshold_db: -24.0,
            dyn_attack_ms: 10.0,
            dyn_release_ms: 150.0,
            bypass: false,
            mute: false,
            solo: false,
            feedback: 0.0,
            feedback_hz: 250.0,
            harmonics_db: DEFAULT_HARMONICS_DB,
            punch_db: 0.0,
            body_db: 0.0,
        }
    }
}

/// Harmonic's recipe until one is drawn: a warm one, the even ones first and falling away.
pub const DEFAULT_HARMONICS_DB: [f32; HARMONICS] = [-18.0, -26.0, -32.0, -40.0, -72.0, -72.0, -72.0, -72.0];
/// Punch and Body: their range, dB, and the detector's transient time, ms.
pub const PUSH_MAX_DB: f32 = 24.0;
const PUSH_TRANSIENT_MS: f64 = 30.0;

/// The gain (dB) a band's dynamics settle at for an input peak of `level_db` against `threshold_db`
/// (the engine's gain computer; there the threshold is the band's own average level).
pub fn dynamics_gain_db(amount: f32, threshold_db: f32, level_db: f32) -> f32 {
    let a = amount.clamp(-1.0, 1.0);
    let over = soft_above(level_db - threshold_db, DYN_KNEE_DB);
    if a < 0.0 {
        a * over
    } else {
        soft_cap(a * over, DYN_LIFT_MAX_DB)
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SatSettings {
    /// Each crossover slot's frequency, Hz, or `None` while it is off.
    pub crossovers: [Option<f32>; CROSSOVERS],
    pub bands: [BandSat; BANDS],
    /// Per-band loudness matching.
    pub auto_gain: bool,
}

impl Default for SatSettings {
    fn default() -> Self {
        Self {
            crossovers: [None; CROSSOVERS],
            bands: [BandSat::default(); BANDS],
            auto_gain: true,
        }
    }
}

/// The split the crossovers make.
pub type Layout = crate::crossover::Layout<BANDS>;

/// The tone tilt's pivot for band slot `band` in `layout`, Hz: the geometric centre of its region
/// (open ends at 20 Hz and 20 kHz), or [`TONE_PIVOT`] with one band.
pub fn tone_pivot(layout: &Layout, crossovers: &[Option<f32>; CROSSOVERS], band: usize) -> f32 {
    if layout.n == 0 {
        return TONE_PIVOT;
    }
    layout
        .region(band, crossovers)
        .map_or(TONE_PIVOT, |(lo, hi)| (lo * hi).sqrt())
}

/// A first-order tilt, `tone_db / 2` above the pivot and as much below, unity at it (bilinear,
/// warped at the pivot). Exactly the identity at 0 dB.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Tilt {
    b0: f32,
    b1: f32,
    a1: f32,
}

impl Tilt {
    pub const IDENTITY: Tilt = Tilt {
        b0: 1.0,
        b1: 0.0,
        a1: 0.0,
    };

    pub fn new(tone_db: f32, pivot: f32, fs: f32) -> Self {
        if tone_db == 0.0 {
            return Self::IDENTITY;
        }
        let (f0, fs) = (f64::from(pivot.min(0.45 * fs)), f64::from(fs));
        let hi = 10f64.powf(f64::from(tone_db) / 40.0);
        let lo = 1.0 / hi;
        let w0 = std::f64::consts::TAU * f0;
        let wp = w0 * (hi / lo).sqrt();
        let k = w0 / (std::f64::consts::PI * f0 / fs).tan();
        let d = k + wp;
        Self {
            b0: ((hi * k + lo * wp) / d) as f32,
            b1: ((lo * wp - hi * k) / d) as f32,
            a1: ((wp - k) / d) as f32,
        }
    }

    /// The same section times `gain`.
    pub fn scaled(self, gain: f32) -> Self {
        Self {
            b0: self.b0 * gain,
            b1: self.b1 * gain,
            ..self
        }
    }

    /// The exact inverse (the section is minimum phase: its zero is inside the unit circle).
    pub fn inverse(self) -> Self {
        Self {
            b0: 1.0 / self.b0,
            b1: self.a1 / self.b0,
            a1: self.b1 / self.b0,
        }
    }

    /// The magnitude at `f` Hz (for the editor and tests).
    pub fn magnitude(&self, f: f64, fs: f64) -> f64 {
        let w = std::f64::consts::TAU * f / fs;
        let (c, s) = (w.cos(), w.sin());
        let num = (
            f64::from(self.b0) + f64::from(self.b1) * c,
            -f64::from(self.b1) * s,
        );
        let den = (1.0 + f64::from(self.a1) * c, -f64::from(self.a1) * s);
        (num.0.hypot(num.1)) / den.0.hypot(den.1)
    }

    /// `z` (re, im) through the section at `f` Hz: gain and phase.
    fn apply(&self, f: f64, fs: f64, z: (f64, f64)) -> (f64, f64) {
        let w = std::f64::consts::TAU * f / fs;
        let (c, s) = (w.cos(), w.sin());
        let (nr, ni) = (
            f64::from(self.b0) + f64::from(self.b1) * c,
            -f64::from(self.b1) * s,
        );
        let (dr, di) = (1.0 + f64::from(self.a1) * c, -f64::from(self.a1) * s);
        let d = dr * dr + di * di;
        let (hr, hi) = ((nr * dr + ni * di) / d, (ni * dr - nr * di) / d);
        (z.0 * hr - z.1 * hi, z.0 * hi + z.1 * hr)
    }

    #[inline]
    fn process(&self, state: &mut f32, x: f32) -> f32 {
        let y = self.b0 * x + *state;
        *state = self.b1 * x - self.a1 * y;
        y
    }
}

// ---- Hysteresis lanes -----------------------------------------------------------------------

/// The emphasis around hysteresis core `w` at rate `fs`: into the core, and its exact inverse out.
pub fn emphasis(w: usize, fs: f32) -> (Tilt, Tilt) {
    let (tilt, pivot) = EMPHASIS[w];
    let pre = Tilt::new(tilt, pivot, fs).scaled(crate::db_to_gain(tilt.abs() / 2.0));
    (pre, pre.inverse())
}

/// One channel of a Tape or Transformer lane: the core and its emphasis filters' states. A
/// fresh one starts at rest at the operating point, so silence stays silence.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Magnet {
    core: Ja,
    pre: f32,
    post: f32,
    /// The core's rest magnetisation at the operating point it was worked out for, and its slope
    /// there (per unit of operating point).
    rest: f32,
    rest_at: f32,
    rest_slope: f32,
    /// The envelope of what goes into the core (after the emphasis), for the operating point.
    env: f32,
    /// The core's output scale ([`hysteresis::unity`]).
    unity: f32,
    fresh: bool,
}

impl Default for Magnet {
    fn default() -> Self {
        Self {
            core: Ja::default(),
            pre: 0.0,
            post: 0.0,
            rest: 0.0,
            rest_at: 0.0,
            rest_slope: 0.0,
            env: 0.0,
            unity: 1.0,
            fresh: true,
        }
    }
}

impl Magnet {
    /// A fresh core whose envelope starts at `env` and emphasis filter at `pre` (where a steady
    /// tone holds them).
    pub fn seeded(env: f32, pre: f32) -> Self {
        Self {
            env,
            pre,
            ..Self::default()
        }
    }

    /// Seeded for a sine `amp·sin(2π·i/n)` from `i = 0` (the drive in it) through emphasis `em` at
    /// rate `fs`: the emphasis in its steady state, and the envelope where it settles.
    fn steady(em: &(Tilt, Tilt), amp: f32, n: usize, fs: f32, k: (f32, f32)) -> Self {
        let freq = f64::from(fs) / n as f64;
        let (hr, hi) = em.0.apply(freq, f64::from(fs), (1.0, 0.0));
        let v: Vec<f32> = (0..n)
            .map(|i| {
                let th = std::f64::consts::TAU * i as f64 / n as f64;
                (f64::from(amp) * (hr * th.sin() + hi * th.cos())) as f32
            })
            .collect();
        // At i = 0 the input is 0, so the filter's state is its output.
        Self::seeded(settled_env(&v, k), v[0])
    }

    /// The model core `w` plays.
    fn model(w: usize) -> Model {
        if w == 0 {
            Model::Tape
        } else {
            Model::Transformer
        }
    }

    /// The operating point in silence (what the rest magnetisation is taken at).
    pub fn point(&self, _w: usize, bias: f32) -> f32 {
        bias_point(bias)
    }

    fn set_rest(&mut self, p: &JaParams, point: f32) {
        let d = 1e-3;
        self.rest = anhysteretic(p, p.h_in * point);
        self.rest_slope = (anhysteretic(p, p.h_in * (point + d))
            - anhysteretic(p, p.h_in * (point - d)))
            / (2.0 * d);
        self.rest_at = point;
    }

    /// `F` for the driven sample `u0` (`g·x`) at bias `bias` (−1…1), through core `w` with its
    /// emphasis `em`: slope 1 for quiet signals, 0 at rest. `_k` (the envelope's coefficients) is
    /// no longer used: the operating point follows the sample, not an envelope.
    #[inline]
    pub fn process(
        &mut self,
        w: usize,
        em: &(Tilt, Tilt),
        u0: f32,
        bias: f32,
        _k: (f32, f32),
    ) -> f32 {
        let p = &MAGNETS[w];
        if self.fresh {
            let point = self.point(w, bias);
            self.core = Ja::rest(p, p.h_in * point);
            (self.post, self.fresh) = (0.0, false);
            self.unity = hysteresis::unity(p);
            self.set_rest(p, point);
        }
        let v = em.0.process(&mut self.pre, u0);
        let point = self.point(w, bias);
        if (point - self.rest_at).abs() > REST_STEP * (1.0 + point.abs()) {
            self.set_rest(p, point);
        }
        let rest = self.rest + self.rest_slope * (point - self.rest_at);
        let m = self.core.step(p, p.h_in * (v + operating_point(Self::model(w), bias, v)));
        em.1.process(&mut self.post, self.unity * (m - rest))
    }
}

/// What a Tape or Transformer band makes of `x` at rate `fs` (the rate the curve runs at),
/// from rest: `F` through the emphasis and the core, the code the engine runs per lane (without
/// the oversampler, the tone and the dry blend). For the editor and tests; allocates.
pub fn magnet_response(model: Model, x: &[f32], drive_db: f32, bias: f32, fs: f32) -> Vec<f32> {
    respond(model, x, drive_db, bias, fs, None)
}

/// [`magnet_response`] from `magnet`'s state (`None`: from rest).
fn respond(
    model: Model,
    x: &[f32],
    drive_db: f32,
    bias: f32,
    fs: f32,
    steady: Option<(f32, usize)>,
) -> Vec<f32> {
    let Some(w) = model.magnet() else {
        return x.iter().map(|&v| curve(model, v)).collect();
    };
    let em = emphasis(w, fs);
    let g = CALIBRATION * crate::db_to_gain(drive_db);
    let k = env_coefficients(f64::from(fs));
    let mut magnet = steady.map_or_else(Magnet::default, |(amp, n)| {
        Magnet::steady(&em, g * amp, n, fs, k)
    });
    x.iter()
        .map(|&v| magnet.process(w, &em, g * v, bias, k) / CALIBRATION * out_scale(drive_db))
        .collect()
}

/// The loop a Tape or Transformer band traces for a sine of amplitude `amp` at `freq` Hz once
/// settled (after one cycle from rest): `(x, F(x))` over the next, `n` points from phase 0 (the
/// editor draws it).
pub fn magnet_loop(
    model: Model,
    amp: f32,
    drive_db: f32,
    bias: f32,
    freq: f32,
    n: usize,
) -> Vec<(f32, f32)> {
    // The core is rate independent and the emphasis only sees `freq / fs`: any rate with `n`
    // samples a cycle gives the same loop. The emphasis and envelope start where the tone holds them.
    let fs = freq * n as f32;
    let x: Vec<f32> = (0..2 * n)
        .map(|i| amp * (std::f64::consts::TAU * i as f64 / n as f64).sin() as f32)
        .collect();
    let y = respond(model, &x, drive_db, bias, fs, Some((amp, n)));
    x[n..].iter().copied().zip(y[n..].iter().copied()).collect()
}

// ---- Harmonics probe ------------------------------------------------------------------------

/// The partials the probe reads: the fundamental and H2…H9.
pub const PARTIALS: usize = 9;
/// Samples a cycle through a static curve (cheap, and fine enough for a fold at full drive).
const PROBE_STATIC: usize = 1024;
/// Through a hysteresis core each sample is a solve, and the core is rate independent: enough
/// samples to resolve H9, and for the emphasis to see its corner (at least this rate).
const PROBE_MAGNET: (usize, usize) = (64, 256);
const PROBE_MAGNET_FS: f32 = 16_000.0;
/// A biased core's loop takes up to about 32 cycles to find its centre, an unbiased one 2: it runs
/// until a cycle repeats the last within this share of the band's swing, or this many cycles.
const PROBE_SETTLED: f32 = 2e-4;
const PROBE_CYCLES: usize = 48;

/// The band a probe tone goes through.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Probe {
    pub model: Model,
    /// The tone's amplitude going in, linear.
    pub amp: f32,
    pub drive_db: f32,
    /// The band's bias, −1…1 (the operating point follows from it and the tone's envelope).
    pub bias: f32,
    /// The tone, Hz (the band's centre).
    pub freq: f32,
    pub tone_db: f32,
    /// The tilt's pivot, Hz ([`tone_pivot`]).
    pub pivot: f32,
    /// The band's mix, 0…1 (0 when it is bypassed).
    pub mix: f32,
    /// The base sample rate.
    pub fs: f32,
    /// Harmonic's recipe ([`recipe`]).
    pub recipe: Recipe,
}

/// What a probe measured: each partial's amplitude coming out of the band, before its gains.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Harmonics {
    /// H1…H9, linear.
    pub amp: [f32; PARTIALS],
    /// Above the base Nyquist: the curve makes it at the high rate, the way down removes it.
    pub hollow: [bool; PARTIALS],
}

impl Harmonics {
    /// Partial `i` (0 = H1) in dB against the strongest (Rectify trades H1 for H2, so the
    /// fundamental cannot be the reference). Silence reads −∞.
    pub fn db(&self, i: usize) -> f32 {
        let top = self.amp.iter().copied().fold(0.0, f32::max);
        if top <= 0.0 || self.amp[i] <= 0.0 {
            return f32::NEG_INFINITY;
        }
        20.0 * (self.amp[i] / top).log10()
    }
}

/// Sends a sine through the band's own code (the curve or core and emphasis, then the tone and
/// the dry blend, as sat-brief §1.1) and reads H1…H9, phase included. It runs at a rate of its
/// own, well above every partial, so it measures what the curve makes rather than the aliases
/// of one oversampling factor. The envelope the operating point follows runs as in the engine, at
/// the probe's rate, from the tone's peak until it repeats. For the editor; allocates.
pub fn probe_harmonics(p: &Probe) -> Harmonics {
    let share = f64::from(engagement(p.drive_db)) * f64::from(p.mix);
    let mut out = Harmonics {
        amp: [0.0; PARTIALS],
        hollow: std::array::from_fn(|i| (i + 1) as f32 * p.freq >= p.fs / 2.0),
    };
    let n = match p.model.magnet() {
        Some(_) => ((PROBE_MAGNET_FS / p.freq.max(1.0)).ceil() as usize)
            .clamp(PROBE_MAGNET.0, PROBE_MAGNET.1),
        None => PROBE_STATIC,
    };
    let sine = |i: usize| p.amp * (std::f64::consts::TAU * i as f64 / n as f64).sin() as f32;
    let g = CALIBRATION * crate::db_to_gain(p.drive_db);
    let rate = p.freq * n as f32;
    let k = env_coefficients(f64::from(rate));
    let wet: Vec<f32> = if let Some(w) = p.model.magnet() {
        let em = emphasis(w, rate);
        let mut magnet = Magnet::steady(&em, g * p.amp, n, rate, k);
        let mut last = vec![0.0f32; n];
        let mut cycle = vec![0.0f32; n];
        for c in 0..PROBE_CYCLES {
            for (i, y) in cycle.iter_mut().enumerate() {
                *y = magnet.process(w, &em, g * sine(i), p.bias, k) / CALIBRATION * out_scale(p.drive_db);
            }
            let swing = cycle.iter().fold(0.0f32, |m, v| m.max(v.abs()));
            let moved = cycle
                .iter()
                .zip(&last)
                .fold(0.0f32, |m, (a, b)| m.max((a - b).abs()));
            std::mem::swap(&mut cycle, &mut last);
            if c > 0 && moved <= PROBE_SETTLED * swing {
                break;
            }
        }
        last
    } else {
        (0..n)
            .map(|i| static_lane(p.model, g * sine(i), p.bias, &p.recipe) * out_scale(p.drive_db))
            .collect()
    };
    let tilt = Tilt::new(p.tone_db, p.pivot, p.fs);
    let (fs, freq) = (f64::from(p.fs), f64::from(p.freq));
    for (i, a) in out.amp.iter_mut().enumerate() {
        let h = i + 1;
        let w = std::f64::consts::TAU * h as f64 / n as f64;
        let (mut re, mut im) =
            wet.iter()
                .enumerate()
                .fold((0.0f64, 0.0f64), |(re, im), (k, &v)| {
                    let th = w * k as f64;
                    (re + f64::from(v) * th.cos(), im - f64::from(v) * th.sin())
                });
        re *= 2.0 / n as f64;
        im *= 2.0 / n as f64;
        // The tone tilts what is left after the way down; what the way down removes never meets it.
        if !out.hollow[i] {
            (re, im) = tilt.apply(h as f64 * freq, fs, (re, im));
        }
        // y = x + a·HP₅(wet − x), the dry band being the sine itself (−j·A in this transform).
        let dry = if h == 1 { f64::from(p.amp) } else { 0.0 };
        im += dry;
        let f = h as f64 * freq;
        let d = f * f + HP_HZ * HP_HZ;
        let (hr, hi) = (f * f / d, f * HP_HZ / d);
        (re, im) = (
            (re * hr - im * hi) * share,
            (re * hi + im * hr) * share - dry,
        );
        *a = re.hypot(im) as f32;
    }
    out
}

// ---- Measurement -----------------------------------------------------------------------------

/// What one band does to a steady sine, measured on the engine's output.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Distortion {
    /// The harmonics below Nyquist against the fundamental, dB (THD; Rectify's fundamental is
    /// small, so it reads high).
    pub thd_db: f64,
    /// Everything that is neither a harmonic nor DC (aliases: harmonics above Nyquist folded
    /// back, and any other leakage) against the strongest harmonic, dB.
    pub alias_db: f64,
}

/// Window of [`distortion`]: the tone is whole in it, so every partial sits on a bin.
pub const MEASURE_N: usize = 16_384;

/// Runs a sine of `cycles` periods per [`MEASURE_N`] samples at `amp` through one band of
/// `model` at `factor`, `drive_db` and `bias` (auto gain off) at `fs` for a second, and reads
/// the last window's spectrum. For reports and tests; allocates.
pub fn distortion(
    model: Model,
    factor: usize,
    fs: f32,
    cycles: usize,
    amp: f32,
    drive_db: f32,
    bias: f32,
) -> Distortion {
    use realfft::RealFftPlanner;
    let mut s = SatSettings {
        auto_gain: false,
        ..SatSettings::default()
    };
    s.bands[0] = BandSat {
        model,
        drive_db,
        bias,
        ..BandSat::default()
    };
    let n = MEASURE_N;
    let total = fs as usize + n;
    let w = std::f64::consts::TAU * cycles as f64 / n as f64;
    let mut x: Vec<f32> = (0..total)
        .map(|i| (f64::from(amp) * (w * i as f64).sin()) as f32)
        .collect();
    let mut sat = Saturator::new(fs, factor, &s);
    for block in x.chunks_mut(512) {
        sat.process(&mut [block], &mut [], &s);
    }
    let mut tail: Vec<f64> = x[total - n..].iter().map(|&v| f64::from(v)).collect();
    let fft = RealFftPlanner::<f64>::new().plan_fft_forward(n);
    let mut spec = fft.make_output_vec();
    fft.process(&mut tail, &mut spec).expect("sizes match");
    let power: Vec<f64> = spec.iter().map(|c| c.norm_sqr()).collect();
    let half = n / 2;
    let harmonic = |bin: usize| bin % cycles == 0;
    let fundamental = power[cycles];
    let (mut harmonics, mut strongest, mut rest) = (0.0f64, fundamental, 0.0f64);
    for (bin, &p) in power.iter().enumerate().take(half).skip(1) {
        if harmonic(bin) {
            if bin != cycles {
                harmonics += p;
            }
            strongest = strongest.max(p);
        } else {
            rest += p;
        }
    }
    let db = |r: f64| 10.0 * r.max(1e-30).log10();
    Distortion {
        thd_db: db(harmonics / fundamental.max(1e-30)),
        alias_db: db(rest / strongest.max(1e-30)),
    }
}

// ---- Engine --------------------------------------------------------------------------------

/// One band slot's state in one core, per channel.
struct BandCore {
    os: Oversampler,
    /// Minimum phase: the band's dry through the same round trip with no curve in it, so every
    /// blend of dry and wet lines up (a plain delay against the minimum-phase wet combed −4…−15
    /// dB at 15–18 kHz in a 50 % blend).
    dry_os: Oversampler,
    /// The static models' envelope (for the operating point) per lane and channel.
    env: [[f32; 2]; 2],
    /// Tape's or Transformer's core per lane and channel.
    magnet: [[Magnet; 2]; 2],
    tone: [f32; 2],
    /// The difference high-pass: last input and output per channel.
    hp: [(f32, f32); 2],
    /// The feedback band-pass's state per channel, and the curve's last output (its units).
    fb: [[f32; 2]; 2],
    fb_last: [f32; 2],
    /// Punch and Body's transient detector (Mite Shape's), on the band going in.
    hit: crate::eq::ShapeDetector,
    /// The band's level (mean square, both channels), for auto gain.
    level: f64,
    /// The same through Tape's and Transformer's emphasis (what their cores are driven with), and
    /// those filters' states per channel.
    level_emph: [f64; 2],
    emph_state: [[f32; 2]; 2],
    /// Auto gain's trim meters: mean squares in and out (after the prediction).
    trim_in: f64,
    trim_out: f64,
    /// The gate auto gain's meters run behind: the short-term mean square going in, and its
    /// recent loudest.
    short: f64,
    top: f64,
    /// How many samples the meters have taken: until it passes their time constant they average
    /// all of them (a slow meter otherwise starts far off and drifts in for seconds).
    heard: f64,
    /// The dynamics: the detector's peak (linear) and the gain now, dB. They run whether or not
    /// the curve does, so `clear` leaves them.
    dyn_peak: f32,
    dyn_db: f32,
    /// The automatic threshold, dB, once the band has been heard.
    dyn_ref: Option<f32>,
    /// Nothing runs: the oversampler and filters are clear.
    idle: bool,
    /// The L/R and M/S balance as it glides (`BandSat::lr`, `ms`).
    balance: [f32; 2],
}

impl BandCore {
    fn new(factor: usize) -> Self {
        Self {
            os: Oversampler::new(factor),
            dry_os: Oversampler::new(factor),
            env: [[0.0; 2]; 2],
            magnet: [[Magnet::default(); 2]; 2],
            tone: [0.0; 2],
            hp: [(0.0, 0.0); 2],
            fb: [[0.0; 2]; 2],
            fb_last: [0.0; 2],
            hit: crate::eq::ShapeDetector::new(48_000.0, PUSH_TRANSIENT_MS, 1.0),
            level: 0.0,
            level_emph: [0.0; 2],
            emph_state: [[0.0; 2]; 2],
            trim_in: 0.0,
            trim_out: 0.0,
            short: 0.0,
            top: 0.0,
            heard: 0.0,
            dyn_peak: 0.0,
            dyn_db: 0.0,
            dyn_ref: None,
            idle: true,
            balance: [0.0; 2],
        }
    }

    fn clear(&mut self) {
        self.os.reset();
        self.dry_os.reset();
        self.env = [[0.0; 2]; 2];
        self.magnet = [[Magnet::default(); 2]; 2];
        self.tone = [0.0; 2];
        self.hp = [(0.0, 0.0); 2];
        self.fb = [[0.0; 2]; 2];
        self.fb_last = [0.0; 2];
        self.hit.reset(0.0);
        self.idle = true;
    }

    fn copy_from(&mut self, other: &BandCore) {
        self.os.copy_from(&other.os);
        self.dry_os.copy_from(&other.dry_os);
        self.env = other.env;
        self.magnet = other.magnet;
        self.tone = other.tone;
        self.hp = other.hp;
        self.fb = other.fb;
        self.fb_last = other.fb_last;
        self.hit = other.hit;
        self.level = other.level;
        self.level_emph = other.level_emph;
        self.emph_state = other.emph_state;
        self.trim_in = other.trim_in;
        self.trim_out = other.trim_out;
        self.short = other.short;
        self.top = other.top;
        self.heard = other.heard;
        self.dyn_peak = other.dyn_peak;
        self.dyn_db = other.dyn_db;
        self.dyn_ref = other.dyn_ref;
        self.idle = other.idle;
    }
}

/// Everything one split runs. Two of them crossfade when the split changes. Allocation-free once
/// sized.
struct Core {
    layout: Layout,
    /// Each band's stereo mode (Stereo for all on mono).
    modes: [StereoMode; BANDS],
    split: Split<BANDS>,
    bands: [BandCore; BANDS],
    /// The band delays (the oversampler's round trip): band slot × channel rings.
    rings: Vec<f32>,
    ring_len: usize,
}

impl Core {
    fn new(factor: usize) -> Self {
        let ring_len = Oversampler::latency_for(8) + CHUNK + PREROLL + 1;
        Self {
            layout: Layout::new(&[None; CROSSOVERS]),
            modes: [StereoMode::Stereo; BANDS],
            split: Split::default(),
            bands: std::array::from_fn(|_| BandCore::new(factor)),
            rings: vec![0.0; BANDS * 2 * ring_len],
            ring_len,
        }
    }

    fn reset(&mut self) {
        self.split.reset();
        for b in &mut self.bands {
            b.clear();
            b.level = 0.0;
            b.level_emph = [0.0; 2];
            b.emph_state = [[0.0; 2]; 2];
            b.trim_in = 0.0;
            b.trim_out = 0.0;
            b.short = 0.0;
            b.top = 0.0;
            b.heard = 0.0;
            b.dyn_peak = 0.0;
            b.dyn_db = 0.0;
            b.dyn_ref = None;
        }
        self.rings.fill(0.0);
    }

    fn copy_from(&mut self, other: &Core) {
        self.layout = other.layout;
        self.modes = other.modes;
        self.split = other.split;
        for (a, b) in self.bands.iter_mut().zip(&other.bands) {
            a.copy_from(b);
        }
        self.rings.copy_from_slice(&other.rings);
    }

    fn ring(&self, slot: usize, c: usize) -> std::ops::Range<usize> {
        let start = (slot * 2 + c) * self.ring_len;
        start..start + self.ring_len
    }

    /// Moves to `layout`: a new crossover starts from rest, and so does its band (it has no past:
    /// its delay and oversampler start silent, while the band below keeps playing the part it
    /// held); the band's meters start from the band it split.
    fn restructure(&mut self, layout: Layout, crossovers: &[Option<f32>; CROSSOVERS]) {
        let parents = self.split.restructure(&self.layout, &layout, crossovers);
        for (slot, parent) in parents.iter().enumerate() {
            let Some(parent) = *parent else { continue };
            let p = &self.bands[parent];
            let meters = (p.level, p.level_emph, p.trim_in, p.trim_out, p.short, p.top);
            let dynamics = (p.dyn_peak, p.dyn_db, p.dyn_ref);
            let band = &mut self.bands[slot];
            band.clear();
            (
                band.level,
                band.level_emph,
                band.trim_in,
                band.trim_out,
                band.short,
                band.top,
            ) = meters;
            (band.dyn_peak, band.dyn_db, band.dyn_ref) = dynamics;
            for c in 0..2 {
                let r = self.ring(slot, c);
                self.rings[r].fill(0.0);
            }
        }
        self.layout = layout;
    }

    /// Moves to `modes`: a band that changes starts its curves afresh (their state belongs to the
    /// signals they had), so the crossfade only begins once it has run one round trip. Its delay
    /// holds left and right either way.
    fn remode(&mut self, modes: [StereoMode; BANDS]) {
        for (b, band) in self.bands.iter_mut().enumerate() {
            if modes[b] != self.modes[b] {
                band.clear();
            }
        }
        self.modes = modes;
    }
}

/// A band's settings as they glide, and its model lanes.
#[derive(Debug, Clone, Copy)]
struct Eased {
    drive_db: f64,
    bias: f64,
    tone_db: f64,
    mix: f64,
    out: f64,
    engage: f64,
    audible: f64,
    comp: f64,
    trim: f64,
    /// The dynamics amount as it glides, and the gain's attack and release coefficients per
    /// sample.
    dyn_amount: f64,
    dyn_k: (f32, f32),
    /// Feedback, 0…1, and its frequency as it glides (ln Hz).
    feedback: f64,
    feedback_ln: f64,
    /// Harmonic's recipe weights as they glide, and Punch and Body, dB.
    recipe: [f64; HARMONICS],
    punch: f64,
    body: f64,
    /// The two curve lanes' models, the one heard, and the crossfade into the other (0…1).
    lanes: [Model; 2],
    lane: usize,
    lane_fade: Option<f64>,
}

impl Eased {
    fn at(b: &BandSat, audible: bool) -> Self {
        Self {
            drive_db: f64::from(b.drive_db),
            bias: f64::from(b.bias),
            tone_db: f64::from(b.tone_db),
            mix: f64::from(b.mix),
            out: f64::from(crate::db_to_gain(b.out_db)),
            engage: if b.bypass { 0.0 } else { 1.0 },
            audible: if audible { 1.0 } else { 0.0 },
            comp: 1.0,
            trim: 1.0,
            dyn_amount: f64::from(b.dyn_amount),
            feedback: f64::from(b.feedback.clamp(0.0, 1.0)),
            feedback_ln: f64::from(b.feedback_hz.clamp(FEEDBACK_HZ.0, FEEDBACK_HZ.1)).ln(),
            recipe: recipe(&b.harmonics_db).map(f64::from),
            punch: f64::from(b.punch_db.clamp(-PUSH_MAX_DB, PUSH_MAX_DB)),
            body: f64::from(b.body_db.clamp(-PUSH_MAX_DB, PUSH_MAX_DB)),
            dyn_k: (0.0, 0.0),
            lanes: [b.model; 2],
            lane: 0,
            lane_fade: None,
        }
    }

    /// The saturation's share now.
    fn share(&self) -> f64 {
        f64::from(engagement(self.drive_db as f32)) * self.mix * smooth(self.engage)
    }
}

/// Per chunk, by band slot: what the editor shows.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SatTelemetry {
    /// The band's peak going in and coming out (after its fader), linear.
    pub in_peak: [f32; BANDS],
    pub out_peak: [f32; BANDS],
    /// The operating point the heard lane ends the chunk at (it follows the envelope), on the
    /// first signal it drives.
    pub point: [f32; BANDS],
    /// Auto gain's correction, linear.
    pub auto: [f32; BANDS],
    /// The dynamics' gain at the chunk's end, linear, and their automatic threshold (the band's
    /// average level, linear; 0 before it has been heard).
    pub dynamics: [f32; BANDS],
    pub threshold: [f32; BANDS],
}

impl Default for SatTelemetry {
    fn default() -> Self {
        Self {
            in_peak: [0.0; BANDS],
            out_peak: [0.0; BANDS],
            point: [0.0; BANDS],
            auto: [1.0; BANDS],
            dynamics: [1.0; BANDS],
            threshold: [0.0; BANDS],
        }
    }
}

/// An S curve over 0…1 (the linear switch ramps are heard through it).
fn smooth(t: f64) -> f64 {
    t * t * (3.0 - 2.0 * t)
}

/// Which bands are heard: muted ones are not; with any soloed, only those.
fn audible(s: &SatSettings, layout: &Layout) -> [bool; BANDS] {
    let any_solo = layout
        .bands()
        .iter()
        .any(|&b| s.bands[b].solo && !s.bands[b].mute);
    std::array::from_fn(|b| !s.bands[b].mute && (!any_solo || s.bands[b].solo))
}

/// Per-sample values of one chunk, by band slot.
struct Lanes {
    gain: [[f32; CHUNK]; BANDS],
    /// The bias, −1…1.
    bias: [[f32; CHUNK]; BANDS],
    share: [[f32; CHUNK]; BANDS],
    post: [[f32; CHUNK]; BANDS],
    /// The band's fader alone, without auto gain (what a part left clean gets).
    fader: [[f32; CHUNK]; BANDS],
    /// What the curve's output is scaled by ([`out_scale`]).
    level: [[f32; CHUNK]; BANDS],
    /// The feedback's loop gain ([`FEEDBACK_LOOP`] times the amount).
    feedback: [[f32; CHUNK]; BANDS],
    /// Punch and Body's push into the curve (1: none).
    push: [[f32; CHUNK]; BANDS],
    /// The model crossfade's position (0: the heard lane only).
    blend: [[f32; CHUNK]; BANDS],
    tilt: [[Tilt; CHUNK]; BANDS],
}

pub struct Saturator {
    fs: f64,
    factor: usize,
    /// One channel: no mid or side to speak of.
    mono: bool,
    cores: [Core; 2],
    active: usize,
    fade: Option<f64>,
    eased: [Eased; BANDS],
    log_freq: [f64; CROSSOVERS],
    cross: [Cross; BANDS],
    countdown: usize,
    write: usize,
    /// Per-control-step and per-sample coefficients.
    glide: f64,
    switch_step: f64,
    model_step: f64,
    fade_step: f64,
    level_k: f64,
    /// Each band's feedback band-pass at the high rate: the state-variable gains and its damping.
    fb_tune: [[f32; 4]; BANDS],
    /// Each band's Harmonic recipe as it glides, and Punch and Body's push now.
    recipe_now: [Recipe; BANDS],
    push: [f32; BANDS],
    /// What the feedback's loop gain is divided by: 1, or for Harmonic its steepest slope with
    /// the bias's widening (at least 1).
    fb_scale: [f64; BANDS],
    /// The recipe Harmonic's steepest slope was last measured for, and the slope.
    slope_of: [(Recipe, f32); BANDS],
    trim_k: f64,
    short_k: f64,
    top_fall: f64,
    /// The dynamics detector's fall per sample, and its reference's step per sample.
    dyn_fall: f32,
    dyn_ref_k: f32,
    env_k: (f32, f32),
    /// Tape's and Transformer's emphasis at the high rate, and (pre only) at the base rate for
    /// the levels auto gain reads.
    emph: [(Tilt, Tilt); 2],
    emph_base: [Tilt; 2],
    hp_k: f32,
    /// The values each control step ramps from and to, by band slot: gain, bias, share, post, fader.
    from: [[f32; 8]; BANDS],
    to: [[f32; 8]; BANDS],
    tilt_now: [Tilt; BANDS],
    /// The tone and pivot each band's tilt was designed for.
    tilt_key: [(f32, f32); BANDS],
    /// The band slots either playing core uses (only they are worked out).
    live: [bool; BANDS],
    lanes: Box<Lanes>,
    /// Scratch: the split per core, position and channel; the high rate; a band's wet.
    split: Vec<f32>,
    hi: Vec<f32>,
    /// One channel's dry at the high rate on its way through `dry_os`.
    dry_hi: Vec<f32>,
    /// Linear or minimum phase, for every band's oversamplers.
    filter: Filter,
    wet: Vec<f32>,
}

impl Saturator {
    /// Allocates.
    pub fn new(sample_rate: f32, factor: usize, s: &SatSettings) -> Self {
        gain_tables();
        let mut sat = Self {
            fs: f64::from(sample_rate),
            factor: 1,
            mono: false,
            cores: [Core::new(factor), Core::new(factor)],
            active: 0,
            fade: None,
            eased: [Eased::at(&BandSat::default(), true); BANDS],
            log_freq: DEFAULT_CROSSOVERS.map(|f| f64::from(f).ln()),
            cross: [Cross::new(1000.0, f64::from(sample_rate)); BANDS],
            countdown: 0,
            write: 0,
            glide: 0.0,
            switch_step: 0.0,
            model_step: 0.0,
            fade_step: 0.0,
            level_k: 0.0,
            fb_tune: [[0.0; 4]; BANDS],
            recipe_now: [[0.0; HARMONICS]; BANDS],
            push: [1.0; BANDS],
            fb_scale: [1.0; BANDS],
            slope_of: [([f32::NAN; HARMONICS], 1.0); BANDS],
            trim_k: 0.0,
            short_k: 0.0,
            top_fall: 1.0,
            dyn_fall: 0.0,
            dyn_ref_k: 0.0,
            env_k: (0.0, 0.0),
            emph: [(Tilt::IDENTITY, Tilt::IDENTITY); 2],
            emph_base: [Tilt::IDENTITY; 2],
            hp_k: 0.0,
            from: [[0.0; 8]; BANDS],
            to: [[0.0; 8]; BANDS],
            tilt_now: [Tilt::IDENTITY; BANDS],
            tilt_key: [(0.0, 0.0); BANDS],
            live: [false; BANDS],
            lanes: Box::new(Lanes {
                gain: [[0.0; CHUNK]; BANDS],
                bias: [[0.0; CHUNK]; BANDS],
                share: [[0.0; CHUNK]; BANDS],
                post: [[0.0; CHUNK]; BANDS],
                fader: [[0.0; CHUNK]; BANDS],
                level: [[0.0; CHUNK]; BANDS],
                feedback: [[0.0; CHUNK]; BANDS],
                push: [[1.0; CHUNK]; BANDS],
                blend: [[0.0; CHUNK]; BANDS],
                tilt: [[Tilt::IDENTITY; CHUNK]; BANDS],
            }),
            split: vec![0.0; 2 * BANDS * 2 * CHUNK],
            hi: vec![0.0; (PREROLL * 16).max(2 * HI_CH)],
            dry_hi: vec![0.0; HI_CH],
            filter: Filter::LinearPhase,
            wet: vec![0.0; CHUNK],
        };
        sat.prepare(sample_rate, factor, s);
        sat
    }

    /// Sizes everything for `sample_rate`. Allocates.
    pub fn prepare(&mut self, sample_rate: f32, factor: usize, s: &SatSettings) {
        self.fs = f64::from(sample_rate);
        self.cores = [Core::new(factor), Core::new(factor)];
        let per = |ms: f64| 1.0 - (-1000.0 / (ms * self.fs)).exp();
        self.glide = 1.0 - (-(CONTROL as f64) * 1000.0 / (GLIDE_MS * self.fs)).exp();
        self.switch_step = CONTROL as f64 * 1000.0 / (SWITCH_MS * self.fs);
        self.model_step = 1000.0 / (MODEL_MS * self.fs);
        self.fade_step = 1000.0 / (STRUCTURE_MS * self.fs);
        self.level_k = per(LEVEL_MS);
        for core in &mut self.cores {
            for b in &mut core.bands {
                b.hit.set_times(self.fs, PUSH_TRANSIENT_MS, 1.0);
            }
        }
        self.trim_k = per(TRIM_SECONDS * 1000.0);
        self.short_k = per(SHORT_MS);
        self.top_fall = 1.0 - per(TOP_RELEASE_MS);
        self.dyn_fall = (1.0 - per(DYN_PEAK_MS)) as f32;
        self.dyn_ref_k = per(DYN_REF_MS) as f32;
        self.hp_k = (-std::f64::consts::TAU * HP_HZ / self.fs).exp() as f32;
        self.set_oversampling(factor, s);
    }

    /// Changes the oversampling (1, 2, 4 or 8) and clears the state: call it in silence (the
    /// engine's dip), as the latency changes with it.
    pub fn set_oversampling(&mut self, factor: usize, s: &SatSettings) {
        for core in &mut self.cores {
            for b in &mut core.bands {
                b.os.set_factor(factor);
                b.dry_os.set_factor(factor);
            }
        }
        self.factor = self.cores[0].bands[0].os.factor();
        let hi_rate = self.fs * self.factor as f64;
        self.env_k = env_coefficients(hi_rate);
        self.emph = [emphasis(0, hi_rate as f32), emphasis(1, hi_rate as f32)];
        self.emph_base = [emphasis(0, self.fs as f32).0, emphasis(1, self.fs as f32).0];
        self.reset(s);
    }

    pub fn oversampling(&self) -> usize {
        self.factor
    }

    /// The oversamplers' filters: linear phase (the round trip a pure delay; the band's dry a
    /// delayed copy, so neutral settings null) or minimum phase (a few samples of latency; the dry
    /// goes the same way, so it is the input's level but not its phase above ~10 kHz).
    pub fn set_filter(&mut self, filter: Filter, s: &SatSettings) {
        for core in &mut self.cores {
            for b in &mut core.bands {
                b.os.set_filter(filter);
                b.dry_os.set_filter(filter);
            }
        }
        self.filter = filter;
        self.reset(s);
    }

    pub fn filter(&self) -> Filter {
        self.filter
    }

    /// The round trip of the oversampling in force, samples (the latency).
    pub fn latency(&self) -> usize {
        Oversampler::latency_of(self.factor, self.filter)
    }

    /// The split playing now (it follows a change once its crossfade is done).
    pub fn layout(&self) -> Layout {
        self.cores[self.active].layout
    }

    /// Clears the state and snaps every glide to `s`.
    pub fn reset(&mut self, s: &SatSettings) {
        let layout = Layout::new(&s.crossovers);
        let heard = audible(s, &layout);
        let modes = self.modes_for(s);
        for (b, e) in self.eased.iter_mut().enumerate() {
            *e = Eased::at(&s.bands[b], heard[b]);
            e.dyn_k = Self::dyn_coefficients(&s.bands[b], self.fs);
        }
        for (k, (lf, x)) in self.log_freq.iter_mut().zip(&mut self.cross).enumerate() {
            let f = s.crossovers[k].unwrap_or(DEFAULT_CROSSOVERS[k]);
            *lf = f64::from(f).ln();
            *x = Cross::new(f64::from(f), self.fs);
        }
        for core in &mut self.cores {
            core.reset();
            core.layout = layout;
            core.modes = modes;
        }
        self.live = std::array::from_fn(|b| layout.position(b).is_some());
        for b in 0..BANDS {
            let key = (s.bands[b].tone_db, tone_pivot(&layout, &s.crossovers, b));
            self.tilt_now[b] = Tilt::new(key.0, key.1, self.fs as f32);
            self.tilt_key[b] = key;
            let v = self.targets(b, s.auto_gain);
            self.from[b] = v;
            self.to[b] = v;
        }
        self.active = 0;
        self.fade = None;
        self.countdown = 0;
        self.write = 0;
    }

    /// A band's dynamics attack and release, per sample.
    fn dyn_coefficients(b: &BandSat, fs: f64) -> (f32, f32) {
        let per =
            |ms: f32| (1.0 - (-1000.0 / (f64::from(ms.clamp(0.05, 5000.0)) * fs)).exp()) as f32;
        (per(b.dyn_attack_ms), per(b.dyn_release_ms))
    }

    fn modes_for(&self, s: &SatSettings) -> [StereoMode; BANDS] {
        std::array::from_fn(|b| {
            if self.mono {
                StereoMode::Stereo
            } else {
                s.bands[b].stereo
            }
        })
    }

    /// Band slot `b`'s gain into the curve, bias, share, post gain and fader for its eased settings.
    fn targets(&self, b: usize, auto_gain: bool) -> [f32; 8] {
        let e = &self.eased[b];
        let share = e.share();
        let auto = if auto_gain {
            e.comp * e.trim.powf(share)
        } else {
            1.0
        };
        let fader = e.out * smooth(e.audible);
        [
            CALIBRATION * crate::db_to_gain(e.drive_db as f32),
            e.bias as f32,
            share as f32,
            (auto * fader) as f32,
            fader as f32,
            out_scale(e.drive_db as f32),
            (f64::from(FEEDBACK_LOOP) * e.feedback * smooth(e.engage) / self.fb_scale[b]) as f32,
            self.push[b],
        ]
    }

    /// Once per control step: glides the settings, follows a split change, retunes auto gain.
    #[allow(clippy::needless_range_loop)] // band slots index several arrays
    fn control(&mut self, s: &SatSettings) {
        let layout = Layout::new(&s.crossovers);
        let modes = self.modes_for(s);
        if self.fade.is_none()
            && (layout != self.cores[self.active].layout || modes != self.cores[self.active].modes)
        {
            let next = 1 - self.active;
            let (a, b) = self.cores.split_at_mut(1);
            let (from, to) = if next == 1 {
                (&a[0], &mut b[0])
            } else {
                (&b[0], &mut a[0])
            };
            to.copy_from(from);
            for p in 0..layout.n {
                let k = layout.crossovers[p];
                if !from.layout.crossovers[..from.layout.n].contains(&k) {
                    let f = s.crossovers[k].unwrap_or(DEFAULT_CROSSOVERS[k]);
                    self.log_freq[k] = f64::from(f).ln();
                    self.cross[k] = Cross::new(f64::from(f), self.fs);
                }
            }
            to.restructure(layout, &s.crossovers);
            to.remode(modes);
            // The new split's band signal reaches the band delay's far end one round trip later:
            // only then is there something to fade into.
            self.fade = Some(-(self.latency() as f64) * self.fade_step);
        }
        for k in 0..CROSSOVERS {
            if let Some(f) = s.crossovers[k] {
                let target = f64::from(f.clamp(FREQ_MIN, FREQ_MAX)).ln();
                let d = target - self.log_freq[k];
                if d.abs() > 1e-6 {
                    self.log_freq[k] = if d.abs() < 1e-4 {
                        target
                    } else {
                        self.log_freq[k] + self.glide * d
                    };
                    self.cross[k] = Cross::new(self.log_freq[k].exp(), self.fs);
                }
            }
        }
        let heard = audible(s, &layout);
        // Only the bands a playing core uses are worked out; one coming into use starts at its
        // settings (it had no sound to glide from).
        let was_live = self.live;
        let other = self.fade.map(|_| self.cores[1 - self.active].layout);
        self.live = std::array::from_fn(|b| {
            self.cores[self.active].layout.position(b).is_some()
                || other.is_some_and(|l| l.position(b).is_some())
        });
        for b in 0..BANDS {
            if self.live[b] && !was_live[b] {
                self.eased[b] = Eased::at(&s.bands[b], heard[b]);
            }
            self.eased[b].dyn_k = Self::dyn_coefficients(&s.bands[b], self.fs);
        }
        let ease = |v: &mut f64, target: f64, k: f64| {
            let d = target - *v;
            *v = if d.abs() < 1e-6 { target } else { *v + k * d };
        };
        let toward = |v: &mut f64, target: f64, step: f64| {
            *v = if target > *v {
                (*v + step).min(target)
            } else {
                (*v - step).max(target)
            };
        };
        let level = self.cores[self.active]
            .bands
            .each_ref()
            .map(|b| (b.level, b.level_emph));
        let trim_ms = self.cores[self.active]
            .bands
            .each_ref()
            .map(|b| (b.trim_in, b.trim_out));
        for b in 0..BANDS {
            if !self.live[b] {
                continue;
            }
            let t = &s.bands[b];
            let e = &mut self.eased[b];
            ease(
                &mut e.drive_db,
                f64::from(t.drive_db.clamp(0.0, DRIVE_MAX_DB)),
                self.glide,
            );
            ease(&mut e.bias, f64::from(t.bias.clamp(-1.0, 1.0)), self.glide);
            ease(
                &mut e.tone_db,
                f64::from(t.tone_db.clamp(-TONE_MAX_DB, TONE_MAX_DB)),
                self.glide,
            );
            ease(&mut e.mix, f64::from(t.mix.clamp(0.0, 1.0)), self.glide);
            ease(
                &mut e.out,
                f64::from(crate::db_to_gain(t.out_db)),
                self.glide,
            );
            ease(
                &mut e.dyn_amount,
                f64::from(t.dyn_amount.clamp(-1.0, 1.0)),
                self.glide,
            );
            ease(&mut e.feedback, f64::from(t.feedback.clamp(0.0, 1.0)), self.glide);
            ease(
                &mut e.feedback_ln,
                f64::from(t.feedback_hz.clamp(FEEDBACK_HZ.0, FEEDBACK_HZ.1)).ln(),
                self.glide,
            );
            // The band-pass at the high rate the curves run at.
            let hi = self.fs * self.factor as f64;
            let g = (std::f64::consts::PI * e.feedback_ln.exp().min(0.45 * hi) / hi).tan();
            let k = 1.0 / f64::from(FEEDBACK_Q);
            let a1 = 1.0 / (1.0 + g * (g + k));
            self.fb_tune[b] = [a1 as f32, (g * a1) as f32, (g * g * a1) as f32, k as f32];
            let target = recipe(&t.harmonics_db);
            for (w, &to) in e.recipe.iter_mut().zip(&target) {
                ease(w, f64::from(to), self.glide);
            }
            self.recipe_now[b] = e.recipe.map(|w| w as f32);
            self.fb_scale[b] = if e.feedback > 0.0 && e.lanes.contains(&Model::Harmonic) {
                let widen = 1.0 + (e.bias.abs() * f64::from(BIAS_TRACK));
                // Measured again only when the recipe has moved.
                if self.slope_of[b].0 != self.recipe_now[b] {
                    self.slope_of[b] = (self.recipe_now[b], harmonic_max_slope(&self.recipe_now[b]));
                }
                (f64::from(self.slope_of[b].1) * widen).max(1.0)
            } else {
                1.0
            };
            ease(&mut e.punch, f64::from(t.punch_db.clamp(-PUSH_MAX_DB, PUSH_MAX_DB)), self.glide);
            ease(&mut e.body, f64::from(t.body_db.clamp(-PUSH_MAX_DB, PUSH_MAX_DB)), self.glide);
            // Punch and Body: the detector's weights now, as a push into the curve (taken back
            // off after it, so it moves the knee and not the level).
            self.push[b] = if e.punch == 0.0 && e.body == 0.0 {
                1.0
            } else {
                let hit = &mut self.cores[self.active].bands[b].hit;
                crate::db_to_gain(hit.gain_db(e.punch, e.body) as f32)
            };
            toward(
                &mut e.engage,
                if t.bypass { 0.0 } else { 1.0 },
                self.switch_step,
            );
            toward(
                &mut e.audible,
                if heard[b] { 1.0 } else { 0.0 },
                self.switch_step,
            );
            // A new model: crossfade into it on the other lane (one change at a time).
            if e.lane_fade.is_none() && t.model != e.lanes[e.lane] {
                let other = 1 - e.lane;
                e.lanes[other] = t.model;
                e.lane_fade = Some(0.0);
                for core in &mut self.cores {
                    core.bands[b].env[other] = [0.0; 2];
                    core.bands[b].magnet[other] = [Magnet::default(); 2];
                }
            }
            // Auto gain: the curve's predicted gain for a sine at the band's level, then a trim
            // measured on the music.
            let share = e.share();
            let model = e.lanes[e.lane];
            // Tape and Transformer: the level their emphasis drives the core with (the inverse
            // after it takes back about as much as it lifted).
            let ms = model.magnet().map_or(level[b].0, |w| level[b].1[w]);
            let amplitude = (2.0 * ms).sqrt() as f32;
            let predicted = f64::from(predicted_gain(
                model,
                amplitude,
                e.drive_db as f32,
                e.bias as f32,
                share as f32,
            ))
            .max(1e-6);
            let limit = 10f64.powf(AUTO_LIMIT_DB / 20.0);
            // Its inputs (level, eased settings) are smooth already: no lag of its own, so the
            // level holds while drive or bypass move.
            e.comp = (1.0 / predicted).clamp(1.0 / limit, limit);
            let (tin, tout) = trim_ms[b];
            if s.auto_gain && tin > TRIM_GATE && tout > TRIM_GATE {
                let lim = 10f64.powf(
                    if model.magnet().is_some() {
                        TRIM_LIMIT_MAGNET_DB
                    } else {
                        TRIM_LIMIT_DB
                    } / 20.0,
                );
                ease(
                    &mut e.trim,
                    (tin / tout).sqrt().clamp(1.0 / lim, lim),
                    self.trim_k * CONTROL as f64,
                );
            }
            if !s.auto_gain {
                e.trim = 1.0;
            }
        }
        let fs = self.fs as f32;
        for b in 0..BANDS {
            if !self.live[b] {
                continue;
            }
            self.from[b] = if was_live[b] {
                self.to[b]
            } else {
                self.targets(b, s.auto_gain)
            };
            self.to[b] = self.targets(b, s.auto_gain);
            let key = (
                self.eased[b].tone_db as f32,
                tone_pivot(&layout, &s.crossovers, b),
            );
            if key != self.tilt_key[b] {
                self.tilt_now[b] = Tilt::new(key.0, key.1, fs);
                self.tilt_key[b] = key;
            }
        }
    }

    /// Saturates one stretch in place (`channels`: one or two) and writes the dry band sum (the
    /// allpass the split makes, delayed like the result) into `dry`, for mix and delta.
    pub fn process(
        &mut self,
        channels: &mut [&mut [f32]],
        dry: &mut [&mut [f32]],
        s: &SatSettings,
    ) -> SatTelemetry {
        self.process_split(channels, dry, s, None)
    }

    /// [`Self::process`] with the bands split outside (Linear Phase): `bands[slot][channel]`, as
    /// long as `channels`, replaces the minimum-phase split (whose state then stands still). The
    /// result and `dry` follow the bands' timing: their split's delay adds to [`Self::latency`].
    pub fn process_bands(
        &mut self,
        channels: &mut [&mut [f32]],
        dry: &mut [&mut [f32]],
        s: &SatSettings,
        bands: &[[&[f32]; 2]; BANDS],
    ) -> SatTelemetry {
        self.process_split(channels, dry, s, Some(bands))
    }

    fn process_split(
        &mut self,
        channels: &mut [&mut [f32]],
        dry: &mut [&mut [f32]],
        s: &SatSettings,
        bands: Option<&[[&[f32]; 2]; BANDS]>,
    ) -> SatTelemetry {
        let mut telemetry = SatTelemetry::default();
        let n_ch = channels.len().min(2);
        if n_ch == 0 {
            return telemetry;
        }
        if self.mono != (n_ch == 1) {
            // Decided by the host's layout, before any sound: nothing to crossfade.
            self.mono = n_ch == 1;
            let modes = self.modes_for(s);
            for core in &mut self.cores {
                core.modes = modes;
            }
        }
        let len = channels.iter().map(|c| c.len()).min().unwrap_or(0);
        let mut start = 0;
        while start < len {
            if self.countdown == 0 {
                self.control(s);
                self.countdown = CONTROL;
            }
            let n = (len - start).min(self.countdown);
            let mut ch: [&mut [f32]; 2] = [&mut [], &mut []];
            let mut dr: [&mut [f32]; 2] = [&mut [], &mut []];
            let (first, rest) = channels.split_at_mut(1);
            ch[0] = &mut first[0][start..start + n];
            if n_ch == 2 {
                ch[1] = &mut rest[0][start..start + n];
            }
            let (dfirst, drest) = dry.split_at_mut(dry.len().min(1));
            if let Some(d) = dfirst.first_mut() {
                dr[0] = &mut d[start..start + n];
            }
            if let Some(d) = drest.first_mut() {
                dr[1] = &mut d[start..start + n];
            }
            self.chunk(&mut ch[..n_ch], &mut dr, n, s, &mut telemetry, bands.map(|b| (b, start)));
            start += n;
        }
        telemetry
    }

    #[allow(clippy::needless_range_loop)] // bands, channels and samples indexed across arrays
    fn chunk(
        &mut self,
        channels: &mut [&mut [f32]],
        dry: &mut [&mut [f32]; 2],
        n: usize,
        s: &SatSettings,
        telemetry: &mut SatTelemetry,
        bands: Option<(&[[&[f32]; 2]; BANDS], usize)>,
    ) {
        let n_ch = channels.len();
        let fading = self.fade.is_some();
        let latency = self.latency();
        // 1. Control and the per-sample values, and the split of each core into its bands.
        let stride = 2 * CHUNK;
        let at = |core: usize, p: usize, c: usize| (core * BANDS + p) * stride + c * CHUNK;
        let mut fade_at = [0.0f64; CHUNK];
        for i in 0..n {
            let u = 1.0 - self.countdown as f32 / CONTROL as f32;
            self.countdown -= 1;
            for b in 0..BANDS {
                if !self.live[b] {
                    continue;
                }
                let (f, t) = (self.from[b], self.to[b]);
                let r = |k: usize| f[k] + (t[k] - f[k]) * u;
                self.lanes.gain[b][i] = r(0);
                self.lanes.bias[b][i] = r(1);
                self.lanes.share[b][i] = r(2);
                self.lanes.post[b][i] = r(3);
                self.lanes.fader[b][i] = r(4);
                self.lanes.level[b][i] = r(5);
                self.lanes.feedback[b][i] = r(6);
                self.lanes.push[b][i] = r(7);
                self.lanes.tilt[b][i] = self.tilt_now[b];
                let e = &mut self.eased[b];
                self.lanes.blend[b][i] = match e.lane_fade {
                    Some(t) => {
                        let t = (t + self.model_step).min(1.0);
                        e.lane_fade = Some(t);
                        (t * t * (3.0 - 2.0 * t)) as f32
                    }
                    None => 0.0,
                };
            }
            fade_at[i] = self.fade.unwrap_or(0.0);
            let x = [channels[0][i], channels[n_ch - 1][i]];
            for ci in 0..2 {
                if ci != self.active && !fading {
                    continue;
                }
                let core = &mut self.cores[ci];
                let layout = core.layout;
                if let Some((lp, offset)) = bands {
                    for c in 0..n_ch {
                        for p in 0..=layout.n {
                            self.split[at(ci, p, c) + i] = lp[layout.owners[p]][c][offset + i];
                        }
                    }
                    continue;
                }
                let mut sig = [0.0f64; BANDS];
                for c in 0..n_ch {
                    core.split
                        .process(c, f64::from(x[c]), &layout, &self.cross, &mut sig);
                    for p in 0..=layout.n {
                        self.split[at(ci, p, c) + i] = sig[p] as f32;
                    }
                }
            }
            if let Some(t) = self.fade {
                self.fade = Some(t + self.fade_step);
            }
        }
        // 2. Each core's bands: delay, saturate, sum.
        let mut out = [[[0.0f32; CHUNK]; 2]; 2];
        let mut dry_out = [[[0.0f32; CHUNK]; 2]; 2];
        let shown = if fading { 1 - self.active } else { self.active };
        for ci in 0..2 {
            if ci != self.active && !fading {
                continue;
            }
            let layout = self.cores[ci].layout;
            for p in 0..=layout.n {
                let b = layout.owners[p];
                let e = self.eased[b];
                let lane_fading = e.lane_fade.is_some();
                let engaged = lane_fading || self.lanes.share[b][..n].iter().any(|&a| a != 0.0);
                let core = &mut self.cores[ci];
                let band = &mut core.bands[b];
                if !engaged && !band.idle {
                    band.clear();
                }
                let mut peak_in = 0.0f32;
                let mut peak_out = 0.0f32;
                // The two signals the curves see: left and right, or mid and side.
                let mode = core.modes[b];
                let ms = mode.mid_side();
                let driven = mode.driven();
                let driven = [driven[0], driven[1] && n_ch == 2];
                // Mean square across what goes through the curves, going in and coming out (before
                // the gains). Mid and side add up (M² + S² is the mean of L² and R²).
                let mut power = [0.0f64; CHUNK];
                let mut power_emph = [[0.0f64; CHUNK]; 2];
                let mut power_out = [0.0f64; CHUNK];
                let per_ch = if ms { 1.0 } else { 1.0 / n_ch as f64 };
                // A: each channel's delayed band, the signals going in and their delayed selves
                // (x_d), the wake-up pre-roll, and the stretch up to the high rate (signal c at
                // `hi[c·HI_CH..]`).
                let mut xds = [[0.0f32; CHUNK]; 2];
                let mut input = [[0.0f32; CHUNK]; 2];
                let f = self.factor;
                let m = n * f;
                for c in 0..n_ch {
                    let src = at(ci, p, c);
                    let r = (b * 2 + c) * core.ring_len;
                    let xd = &mut xds[c];
                    let mut w = self.write;
                    for i in 0..n {
                        let v = self.split[src + i];
                        input[c][i] = v;
                        if latency == 0 {
                            xd[i] = v;
                        } else {
                            core.rings[r + w] = v;
                            xd[i] = core.rings[r + (w + core.ring_len - latency) % core.ring_len];
                        }
                        w = (w + 1) % core.ring_len;
                    }
                    if self.filter == Filter::MinimumPhase && f > 1 {
                        // The dry the minimum-phase wet lines up with: the same round trip.
                        band.dry_os.up(c, &input[c][..n], &mut self.dry_hi[..m]);
                        band.dry_os.down(c, &self.dry_hi[..m], &mut xd[..n]);
                    }
                }
                let mut xde = xds;
                if ms {
                    for i in 0..n {
                        let (l, r) = (input[0][i], input[1][i]);
                        (input[0][i], input[1][i]) = (0.5 * (l + r), 0.5 * (l - r));
                        let (l, r) = (xds[0][i], xds[1][i]);
                        (xde[0][i], xde[1][i]) = (0.5 * (l + r), 0.5 * (l - r));
                    }
                }
                for c in (0..n_ch).filter(|&c| driven[c]) {
                    for i in 0..n {
                        let v = input[c][i];
                        power[i] += f64::from(v) * f64::from(v) * per_ch;
                        for (w, pe) in power_emph.iter_mut().enumerate() {
                            let ve =
                                f64::from(self.emph_base[w].process(&mut band.emph_state[w][c], v));
                            pe[i] += ve * ve * per_ch;
                        }
                        peak_in = peak_in.max(v.abs());
                    }
                    if engaged && band.idle && latency > 0 {
                        // Waking up: run the band's recent past (its ring) through the
                        // oversampler first, so its filters hold the signal, not silence.
                        let past = PREROLL;
                        let mut tmp = [0.0f32; PREROLL];
                        for (j, v) in tmp[..past].iter_mut().enumerate() {
                            let back = (self.write + core.ring_len - past + j) % core.ring_len;
                            let ring = |ch: usize| core.rings[(b * 2 + ch) * core.ring_len + back];
                            *v = if ms {
                                0.5 * (ring(0) + if c == 0 { ring(1) } else { -ring(1) })
                            } else {
                                ring(c)
                            };
                        }
                        band.os.up(c, &tmp[..past], &mut self.hi[..past * f]);
                        let (g, bias, model, level) = (
                            self.lanes.gain[b][0],
                            self.lanes.bias[b][0],
                            e.lanes[e.lane],
                            self.lanes.level[b][0],
                        );
                        let recipe_now = self.recipe_now[b];
                        // The core and the envelope warm up on the past too.
                        for v in &mut self.hi[..past * f] {
                            *v = match model.magnet() {
                                Some(w) => {
                                    band.magnet[e.lane][c].process(
                                        w,
                                        &self.emph[w],
                                        g * *v,
                                        bias,
                                        self.env_k,
                                    ) / CALIBRATION
                                }
                                None => static_lane(model, g * *v, bias, &recipe_now),
                            } * level;
                        }
                        let mut sink = [0.0f32; PREROLL];
                        band.os.down(c, &self.hi[..past * f], &mut sink[..past]);
                    }
                }
                if engaged {
                    for c in (0..n_ch).filter(|&c| driven[c]) {
                        band.os
                            .up(c, &input[c][..n], &mut self.hi[c * HI_CH..c * HI_CH + m]);
                    }
                    // B: the curves, both signals in the same pass (their state chains are
                    // independent, so the CPU overlaps them: a hysteresis core is latency bound).
                    let models = e.lanes;
                    let heard = e.lane;
                    let env_k = self.env_k;
                    let emph = &self.emph;
                    // Feedback's state, out of `band` while the lanes hold it.
                    let (mut fb, mut fb_last) = (band.fb, band.fb_last);
                    let [a1, a2, a3, damp] = self.fb_tune[b];
                    let recipe_now = self.recipe_now[b];
                    let mut lane = |l: usize, c: usize, u0: f32, bias: f32| {
                        let model = models[l];
                        match model.magnet() {
                            Some(w) => {
                                band.magnet[l][c].process(w, &emph[w], u0, bias, env_k)
                                    / CALIBRATION
                            }
                            None => static_lane(model, u0, bias, &recipe_now),
                        }
                    };
                    for k in 0..m {
                        let i = k / f;
                        let (g, bias, level) = (self.lanes.gain[b][i], self.lanes.bias[b][i], self.lanes.level[b][i]);
                        // Punch and Body: further into the curve, and back off after it.
                        let push = self.lanes.push[b][i];
                        for c in 0..n_ch {
                            if !driven[c] {
                                continue;
                            }
                            let at = c * HI_CH + k;
                            let mut u0 = g * push * self.hi[at];
                            let loop_gain = self.lanes.feedback[b][i];
                            if loop_gain > 0.0 {
                                // The curve's last output through the band-pass (state-variable,
                                // unity at its peak), back into the curve.
                                let st = &mut fb[c];
                                let v3 = fb_last[c] - st[1];
                                let v1 = a1 * st[0] + a2 * v3;
                                let v2 = st[1] + a2 * st[0] + a3 * v3;
                                st[0] = 2.0 * v1 - st[0];
                                st[1] = 2.0 * v2 - st[1];
                                u0 += loop_gain * damp * v1;
                            } else {
                                fb[c] = [0.0; 2];
                            }
                            let y = if lane_fading {
                                let (a, z) =
                                    (lane(heard, c, u0, bias), lane(1 - heard, c, u0, bias));
                                a + (z - a) * self.lanes.blend[b][i]
                            } else {
                                lane(heard, c, u0, bias)
                            };
                            fb_last[c] = if loop_gain > 0.0 { y * CALIBRATION } else { 0.0 };
                            self.hi[at] = if push == 1.0 { level * y } else { level * y / push };
                        }
                    }
                    band.fb = fb;
                    band.fb_last = fb_last;
                    band.idle = false;
                }
                // C: back down, the tone and the difference each signal now carries.
                let mut diff = [[0.0f32; CHUNK]; 2];
                for c in (0..n_ch).filter(|&c| driven[c]) {
                    if engaged {
                        band.os
                            .down(c, &self.hi[c * HI_CH..c * HI_CH + m], &mut self.wet[..n]);
                        let (mut x1, mut y1) = band.hp[c];
                        let mut tone = band.tone[c];
                        for i in 0..n {
                            let wet = self.lanes.tilt[b][i].process(&mut tone, self.wet[i]);
                            let d = wet - xde[c][i];
                            let y = self.hp_k * (y1 + d - x1);
                            x1 = d;
                            y1 = y;
                            diff[c][i] = self.lanes.share[b][i] * y;
                        }
                        band.hp[c] = (x1, y1);
                        band.tone[c] = tone;
                    }
                    for i in 0..n {
                        let pre = xde[c][i] + diff[c][i];
                        power_out[i] += f64::from(pre) * f64::from(pre) * per_ch;
                    }
                }
                // The dynamics: a peak detector on what goes into the curves (the band before
                // drive), against the band's own average level (the threshold, in dB over a
                // second, held in silence), its gain eased by the band's attack and release, on
                // what they drive. Bypass eases the amount away; at 0 the gain is exactly 1.
                let amount = (e.dyn_amount * smooth(e.engage)) as f32;
                let threshold = band.dyn_ref.unwrap_or(0.0);
                let (att, rel) = e.dyn_k;
                let mut dyn_gain = [1.0f32; CHUNK];
                let mut loudest = 0.0f32;
                for i in 0..n {
                    let mut x = 0.0f32;
                    for c in (0..n_ch).filter(|&c| driven[c]) {
                        x = x.max(xde[c][i].abs());
                    }
                    band.hit.process(f64::from(x));
                    band.dyn_peak = x.max(band.dyn_peak * self.dyn_fall);
                    loudest = loudest.max(band.dyn_peak);
                    if amount == 0.0 && band.dyn_db == 0.0 {
                        continue;
                    }
                    let level = 20.0 * band.dyn_peak.max(1e-6).log10();
                    let target = dynamics_gain_db(amount, threshold, level);
                    // Attack while the gain moves away from 0 dB, release while it comes back,
                    // blended over 2 dB: a switch from one to the other put a corner in a moving
                    // gain (an amount stepped from lift to cut glides through 0, where release
                    // took over, then attack: +10 dB in the §10 step test, 2026-11-24).
                    let w = ((target.abs() - band.dyn_db.abs()) * 0.5 + 0.5).clamp(0.0, 1.0);
                    let k = rel + (att - rel) * w * w * (3.0 - 2.0 * w);
                    band.dyn_db += k * (target - band.dyn_db);
                    if amount == 0.0 && band.dyn_db.abs() < 1e-4 {
                        band.dyn_db = 0.0;
                    }
                    if band.dyn_db != 0.0 {
                        dyn_gain[i] = crate::db_to_gain(band.dyn_db);
                    }
                }
                // The reference moves once a stretch, on the stretch's loudest reading.
                let level = 20.0 * loudest.max(1e-6).log10();
                if level > DYN_FLOOR_DB {
                    band.dyn_ref = Some(match band.dyn_ref {
                        None => level,
                        Some(r) => r + (1.0 - (1.0 - self.dyn_ref_k).powi(n as i32)) * (level - r),
                    });
                }
                // The sum. Left and right start from what they were and only take on the
                // differences, so a neutral band stays the input bit for bit in every mode.
                let bal_to = [s.bands[b].lr.clamp(-1.0, 1.0), s.bands[b].ms.clamp(-1.0, 1.0)];
                let bal_k = (1.0 - (-1.0 / (BALANCE_GLIDE_S * self.fs)).exp()) as f32;
                for i in 0..n {
                    let post = self.lanes.post[b][i] * dyn_gain[i];
                    let fader = self.lanes.fader[b][i];
                    let mut ys = [0.0f32; 2];
                    if ms {
                        let part = |c: usize| {
                            if driven[c] {
                                xde[c][i] * (post - fader) + post * diff[c][i]
                            } else {
                                0.0
                            }
                        };
                        let (pm, ps) = (part(0), part(1));
                        ys = [xds[0][i] * fader + (pm + ps), xds[1][i] * fader + (pm - ps)];
                    } else {
                        for c in 0..n_ch {
                            // A channel left out (Left, Right) passes as the band's dry part
                            // would, untouched by the curve's gains.
                            ys[c] = if driven[c] { (xds[c][i] + diff[c][i]) * post } else { xds[c][i] * fader };
                        }
                    }
                    // The output balance: the band's output toward the left or the right (L/R), then
                    // toward the mid or the side (M/S). Centred, it is left alone (bit for bit).
                    let bal = &mut band.balance;
                    for (v, &to) in bal.iter_mut().zip(&bal_to) {
                        *v += (to - *v) * bal_k;
                        // Landed: exactly there (centred is then bit for bit, a far end clean).
                        if (to - *v).abs() < 1e-4 {
                            *v = to;
                        }
                    }
                    if n_ch == 2 && (bal[0] != 0.0 || bal[1] != 0.0) {
                        let (wl, wr) = ((1.0 - bal[0]).min(1.0), (1.0 + bal[0]).min(1.0));
                        let (wm, ws) = ((1.0 - bal[1]).min(1.0), (1.0 + bal[1]).min(1.0));
                        let (l, r) = (ys[0] * wl, ys[1] * wr);
                        let (m, sd) = (0.5 * (l + r) * wm, 0.5 * (l - r) * ws);
                        ys = [m + sd, m - sd];
                    }
                    for c in 0..n_ch {
                        out[ci][c][i] += ys[c];
                        peak_out = peak_out.max(ys[c].abs());
                    }
                    for c in 0..n_ch {
                        dry_out[ci][c][i] += xds[c][i];
                    }
                }
                let comp2 = e.comp * e.comp;
                let gate = 10f64.powf(-HIT_GATE_DB / 10.0);
                for i in 0..n {
                    band.short += self.short_k * (power[i] - band.short);
                    band.top = band.short.max(band.top * self.top_fall);
                    if band.short < band.top * gate {
                        continue;
                    }
                    band.heard += 1.0;
                    let first = 1.0 / band.heard;
                    let level_k = self.level_k.max(first);
                    band.level += level_k * (power[i] - band.level);
                    for w in 0..2 {
                        band.level_emph[w] +=
                            level_k * (power_emph[w][i] - band.level_emph[w]);
                    }
                    let trim_k = self.trim_k.max(first);
                    band.trim_in += trim_k * (power[i] - band.trim_in);
                    band.trim_out += trim_k * (power_out[i] * comp2 - band.trim_out);
                }
                if ci == shown {
                    telemetry.in_peak[b] = telemetry.in_peak[b].max(peak_in);
                    telemetry.out_peak[b] = telemetry.out_peak[b].max(peak_out);
                    telemetry.point[b] = bias_point(e.bias as f32);
                    telemetry.dynamics[b] = crate::db_to_gain(band.dyn_db);
                    telemetry.threshold[b] = band.dyn_ref.map_or(0.0, crate::db_to_gain);
                    telemetry.auto[b] = if s.auto_gain {
                        (e.comp * e.trim.powf(e.share())) as f32
                    } else {
                        1.0
                    };
                }
            }
        }
        self.write = (self.write + n) % self.cores[0].ring_len;
        // 3. The result, crossfading between the splits while one changes.
        for i in 0..n {
            for c in 0..n_ch {
                let (y, d) = if fading {
                    let t = fade_at[i].clamp(0.0, 1.0);
                    let u = (t * t * (3.0 - 2.0 * t)) as f32;
                    let (a, z) = (self.active, 1 - self.active);
                    (
                        out[a][c][i] * (1.0 - u) + out[z][c][i] * u,
                        dry_out[a][c][i] * (1.0 - u) + dry_out[z][c][i] * u,
                    )
                } else {
                    (out[self.active][c][i], dry_out[self.active][c][i])
                };
                channels[c][i] = y;
                if !dry[c].is_empty() {
                    dry[c][i] = d;
                }
            }
        }
        if let Some(t) = self.fade {
            if t >= 1.0 {
                self.active = 1 - self.active;
                self.fade = None;
            }
        }
        for e in &mut self.eased {
            if e.lane_fade.is_some_and(|t| t >= 1.0) {
                e.lane = 1 - e.lane;
                e.lane_fade = None;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FS: f32 = 48_000.0;
    /// A tone that is whole in `N` samples and whose aliases miss its harmonics' bins.
    const N: usize = 4096;
    const K: usize = 89;

    fn tone(n: usize, amp: f32, k: usize) -> Vec<f32> {
        (0..n)
            .map(|i| amp * (std::f64::consts::TAU * (k * i) as f64 / N as f64).sin() as f32)
            .collect()
    }

    fn noise(n: usize, seed: u32, amp: f32) -> Vec<f32> {
        let mut x = seed;
        (0..n)
            .map(|_| {
                x = x.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                ((x >> 8) as f32 / (1u32 << 24) as f32 - 0.5) * 2.0 * amp
            })
            .collect()
    }

    fn run(
        sat: &mut Saturator,
        input: &[f32],
        s: &SatSettings,
        block: usize,
    ) -> (Vec<f32>, Vec<f32>) {
        let mut y = input.to_vec();
        let mut d = vec![0.0; input.len()];
        for (a, b) in y.chunks_mut(block).zip(d.chunks_mut(block)) {
            sat.process(&mut [a], &mut [b], s);
        }
        (y, d)
    }

    fn band(model: Model, drive_db: f32) -> SatSettings {
        let mut s = SatSettings {
            auto_gain: false,
            ..SatSettings::default()
        };
        s.bands[0].model = model;
        s.bands[0].drive_db = drive_db;
        s
    }

    /// The magnitude of harmonic `h` (bin `h·K`) over the last `N` samples, linear amplitude.
    fn harmonic(y: &[f32], h: usize) -> f64 {
        let tail = &y[y.len() - N..];
        let w = std::f64::consts::TAU * (h * K) as f64 / N as f64;
        let (re, im) = tail
            .iter()
            .enumerate()
            .fold((0.0, 0.0), |(re, im), (i, &v)| {
                (
                    re + f64::from(v) * (w * i as f64).cos(),
                    im + f64::from(v) * (w * i as f64).sin(),
                )
            });
        2.0 * re.hypot(im) / N as f64
    }

    /// The same harmonic of the static curve itself (`F(A·sin)`), worked out on a fine grid.
    fn curve_harmonic(model: Model, amp: f32, drive_db: f32, bias: f32, h: usize) -> f64 {
        let m = 8192;
        let (re, im) = (0..m).fold((0.0, 0.0), |(re, im), i| {
            let th = std::f64::consts::TAU * i as f64 / m as f64;
            let v = f64::from(transfer(model, amp * th.sin() as f32, drive_db, bias));
            (
                re + v * (h as f64 * th).cos(),
                im + v * (h as f64 * th).sin(),
            )
        });
        2.0 * re.hypot(im) / m as f64
    }

    #[test]
    fn every_curve_is_zero_at_zero_smooth_and_has_slope_one() {
        for m in Model::ALL {
            // Zero to rounding (−156 dB in an unoptimised build, where Tube's offset sums round
            // differently): no DC from silence.
            assert!(curve(m, 0.0).abs() < 1e-6, "{m:?}: {}", curve(m, 0.0));
            let slope = (curve(m, 1e-3) - curve(m, -1e-3)) / 2e-3;
            let want = if m == Model::Rectify { 0.0 } else { 1.0 };
            assert!((slope - want).abs() < 2e-3, "{m:?}: slope {slope}");
            // No corner anywhere: the slope never jumps between neighbouring points.
            let h = 1e-3f32;
            let mut last: Option<f32> = None;
            let mut u = -6.0f32;
            while u < 6.0 {
                let d = (curve(m, u + h) - curve(m, u - h)) / (2.0 * h);
                if let Some(prev) = last {
                    assert!(
                        (d - prev).abs() < 0.02,
                        "{m:?}: slope jumps at {u}: {prev} → {d}"
                    );
                }
                last = Some(d);
                u += h;
            }
        }
    }

    #[test]
    fn the_probe_reads_what_the_engine_plays() {
        // Every model through the engine at 8×, tone and mix included: the probe's partials are
        // the output's, within 0.25 dB down to −60 dB under the strongest, the operating point
        // following the envelope in both.
        let freq = K as f32 / N as f32 * FS;
        for m in Model::ALL {
            for (amp, drive, bias, tilt, mix) in [
                (0.25f32, 36.0f32, 0.0f32, 0.0f32, 1.0f32),
                (0.1, 48.0, 0.4, 6.0, 1.0),
                (0.3, 33.0, -0.3, -8.0, 0.6),
                (0.2, 27.0, 0.0, 0.0, 1.0),
            ] {
                let mut s = band(m, drive);
                s.bands[0].bias = bias;
                s.bands[0].tone_db = tilt;
                s.bands[0].mix = mix;
                let mut sat = Saturator::new(FS, 8, &s);
                let (y, _) = run(&mut sat, &tone(32 * N, amp, K), &s, 333);
                let got = probe_harmonics(&Probe {
                    model: m,
                    amp,
                    drive_db: drive,
                    bias,
                    freq,
                    tone_db: tilt,
                    pivot: TONE_PIVOT,
                    mix,
                    fs: FS,
                    recipe: recipe(&s.bands[0].harmonics_db),
                });
                let want: [f64; PARTIALS] = std::array::from_fn(|i| harmonic(&y, i + 1));
                let top = want.iter().copied().fold(0.0, f64::max);
                for (i, &w) in want.iter().enumerate() {
                    if w < top * 1e-3 {
                        continue;
                    }
                    let db = 20.0 * (f64::from(got.amp[i]) / w).log10();
                    assert!(
                        db.abs() < 0.25,
                        "{m:?} amp {amp} drive {drive} bias {bias} tone {tilt} mix {mix}: H{} {db:.3} dB off",
                        i + 1
                    );
                }
            }
        }
    }

    #[test]
    fn the_probe_follows_tape_and_transformer_across_the_band() {
        // Their emphasis makes them depend on the frequency: a low and a high tone through the
        // engine, partials against the probe's (those the oversampler's filter passes flat).
        for m in [Model::Tape, Model::Transformer] {
            for k in [6usize, 40, 377, 1009] {
                let freq = k as f32 / N as f32 * FS;
                for (amp, drive, bias) in [(0.2f32, 12.0f32, 0.0f32), (0.1, 24.0, 0.4)] {
                    let mut s = band(m, drive);
                    s.bands[0].bias = bias;
                    let mut sat = Saturator::new(FS, 8, &s);
                    let (y, _) = run(&mut sat, &tone(32 * N, amp, k), &s, 333);
                    let got = probe_harmonics(&Probe {
                        model: m,
                        amp,
                        drive_db: drive,
                        bias,
                        freq,
                        tone_db: 0.0,
                        pivot: TONE_PIVOT,
                        mix: 1.0,
                        fs: FS,
                        recipe: [0.0; HARMONICS],
                    });
                    let tail = &y[y.len() - N..];
                    let read = |h: usize| {
                        let w = std::f64::consts::TAU * (h * k) as f64 / N as f64;
                        let (re, im) =
                            tail.iter()
                                .enumerate()
                                .fold((0.0, 0.0), |(re, im), (i, &v)| {
                                    (
                                        re + f64::from(v) * (w * i as f64).cos(),
                                        im + f64::from(v) * (w * i as f64).sin(),
                                    )
                                });
                        2.0 * f64::hypot(re, im) / N as f64
                    };
                    let top = (1..=PARTIALS)
                        .filter(|&h| !got.hollow[h - 1])
                        .map(read)
                        .fold(0.0, f64::max);
                    for h in 1..=PARTIALS {
                        // Above 0.9 × Nyquist the way down is rolling off (the probe reads the curve).
                        if h as f32 * freq > 0.9 * FS / 2.0 || read(h) < top * 1e-2 {
                            continue;
                        }
                        let db = 20.0 * (f64::from(got.amp[h - 1]) / read(h)).log10();
                        assert!(
                            db.abs() < 1.0,
                            "{m:?} bin {k} amp {amp} drive {drive} bias {bias}: H{h} {db:.3} dB off"
                        );
                    }
                }
            }
        }
    }

    #[test]
    #[ignore = "prints what a probe costs"]
    fn probe_cost() {
        for m in Model::ALL {
            for (freq, bias) in [
                (49.0f32, 0.0f32),
                (800.0, 0.0),
                (1043.0, 0.8),
                (9000.0, 0.8),
            ] {
                let p = Probe {
                    model: m,
                    amp: 0.3,
                    drive_db: 24.0,
                    bias,
                    freq,
                    tone_db: 0.0,
                    pivot: TONE_PIVOT,
                    mix: 1.0,
                    fs: FS,
                    recipe: [0.0; HARMONICS],
                };
                let t = std::time::Instant::now();
                for _ in 0..20 {
                    std::hint::black_box(probe_harmonics(&p));
                }
                println!(
                    "{m:?} {freq} Hz bias {bias}: {:.0} µs",
                    t.elapsed().as_secs_f64() * 1e6 / 20.0
                );
            }
        }
    }

    /// Mid and side over the last `N` samples of a stereo run through `s`, and the delayed input's.
    fn run_stereo(
        s: &SatSettings,
        l: &[f32],
        r: &[f32],
    ) -> (Vec<f32>, Vec<f32>, Vec<f32>, Vec<f32>) {
        let mut sat = Saturator::new(FS, 2, s);
        let (mut a, mut b) = (l.to_vec(), r.to_vec());
        let (mut da, mut db) = (vec![0.0; l.len()], vec![0.0; l.len()]);
        for ((x, y), (u, v)) in a
            .chunks_mut(333)
            .zip(b.chunks_mut(333))
            .zip(da.chunks_mut(333).zip(db.chunks_mut(333)))
        {
            sat.process(&mut [x, y], &mut [u, v], s);
        }
        (a, b, da, db)
    }

    fn stereo_input() -> (Vec<f32>, Vec<f32>) {
        (noise(4 * N, 7, 0.3), noise(4 * N, 91, 0.3))
    }

    #[test]
    fn a_neutral_band_nulls_in_every_stereo_mode() {
        // Drive 0, or the band bypassed: left and right exactly, at any mode.
        let (l, r) = stereo_input();
        for mode in StereoMode::ALL {
            for bypass in [false, true] {
                let mut s = band(Model::Tube, if bypass { 18.0 } else { 0.0 });
                s.bands[0].stereo = mode;
                s.bands[0].bypass = bypass;
                let (a, b, da, db) = run_stereo(&s, &l, &r);
                assert_eq!(a, da, "{mode:?}");
                assert_eq!(b, db, "{mode:?}");
            }
        }
    }

    #[test]
    fn the_output_balance_pans_and_narrows_the_band_and_centred_changes_nothing() {
        let (l, r) = stereo_input();
        let s = band(Model::Tube, 18.0);
        let (a0, b0, _, _) = run_stereo(&s, &l, &r);
        let peak = |x: &[f32]| x.iter().fold(0.0f32, |m, v| m.max(v.abs()));
        let mid = |x: &[f32], y: &[f32]| -> Vec<f32> { x.iter().zip(y).map(|(p, q)| 0.5 * (p + q)).collect() };
        let side = |x: &[f32], y: &[f32]| -> Vec<f32> { x.iter().zip(y).map(|(p, q)| 0.5 * (p - q)).collect() };
        // Past the glide (a 20 ms time constant lands within 1e-4 after ~9 of them).
        let k = 12_000;
        for (lr, ms) in [(-1.0f32, 0.0f32), (1.0, 0.0), (0.0, -1.0), (0.0, 1.0), (0.0, 0.0)] {
            let mut s = band(Model::Tube, 18.0);
            s.bands[0].lr = lr;
            s.bands[0].ms = ms;
            let (a, b, _, _) = run_stereo(&s, &l, &r);
            match (lr as i32, ms as i32) {
                (-1, 0) => assert!(peak(&b[k..]) < 1e-5 && (peak(&a[k..]) - peak(&a0[k..])).abs() < 1e-5, "left only"),
                (1, 0) => assert!(peak(&a[k..]) < 1e-5 && (peak(&b[k..]) - peak(&b0[k..])).abs() < 1e-5, "right only"),
                (0, -1) => assert!(peak(&side(&a, &b)[k..]) < 1e-5 && peak(&mid(&a, &b)[k..]) > 0.05, "mid only (mono)"),
                (0, 1) => assert!(peak(&mid(&a, &b)[k..]) < 1e-5 && peak(&side(&a, &b)[k..]) > 0.05, "side only"),
                _ => assert_eq!((a, b), (a0.clone(), b0.clone()), "centred: as without them, bit for bit"),
            }
        }
    }

    #[test]
    fn left_leaves_the_right_alone_and_right_the_left() {
        for auto_gain in [false, true] {
            let (l, r) = stereo_input();
            for (mode, clean_left) in [(StereoMode::Left, false), (StereoMode::Right, true)] {
                let mut s = band(Model::Tube, 18.0);
                s.auto_gain = auto_gain;
                s.bands[0].stereo = mode;
                let (a, b, da, db) = run_stereo(&s, &l, &r);
                let (clean, clean_dry, driven, driven_dry) = if clean_left { (&a, &da, &b, &db) } else { (&b, &db, &a, &da) };
                let worst = clean.iter().zip(clean_dry.iter()).fold(0.0f32, |m, (x, y)| m.max((x - y).abs()));
                assert!(worst < 2e-6, "{mode:?}: the clean side moved by {worst}");
                let moved = driven.iter().zip(driven_dry.iter()).fold(0.0f32, |m, (x, y)| m.max((x - y).abs()));
                assert!(moved > 0.01, "{mode:?}: the driven side did not change: {moved}");
            }
        }
    }

    #[test]
    fn mid_leaves_the_side_alone_and_side_the_mid() {
        for auto_gain in [false, true] {
            let (l, r) = stereo_input();
            let part = |x: &[f32], y: &[f32], side: bool| -> Vec<f32> {
                x.iter()
                    .zip(y)
                    .map(|(a, b)| if side { 0.5 * (a - b) } else { 0.5 * (a + b) })
                    .collect()
            };
            for (mode, clean_side) in [(StereoMode::Mid, true), (StereoMode::Side, false)] {
                let mut s = band(Model::Tube, 18.0);
                s.auto_gain = auto_gain;
                s.bands[0].stereo = mode;
                let (a, b, da, db) = run_stereo(&s, &l, &r);
                let (got, want) = (part(&a, &b, clean_side), part(&da, &db, clean_side));
                let worst = got
                    .iter()
                    .zip(&want)
                    .fold(0.0f32, |m, (x, y)| m.max((x - y).abs()));
                assert!(worst < 2e-6, "{mode:?}: the clean part moved by {worst}");
                let (got, want) = (part(&a, &b, !clean_side), part(&da, &db, !clean_side));
                let moved = got
                    .iter()
                    .zip(&want)
                    .fold(0.0f32, |m, (x, y)| m.max((x - y).abs()));
                assert!(
                    moved > 0.01,
                    "{mode:?}: the driven part did not change: {moved}"
                );
            }
        }
    }

    #[test]
    fn mid_on_a_centred_signal_is_stereo_and_side_is_silent_on_it() {
        let x = tone(4 * N, 0.2, K);
        let mut s = band(Model::Tube, 12.0);
        s.auto_gain = false;
        let (stereo, _, _, _) = run_stereo(&s, &x, &x);
        s.bands[0].stereo = StereoMode::Mid;
        let (mid, _, _, _) = run_stereo(&s, &x, &x);
        s.bands[0].stereo = StereoMode::MidSide;
        let (both, _, _, _) = run_stereo(&s, &x, &x);
        for (name, y) in [("Mid", &mid), ("M/S", &both)] {
            let worst = y
                .iter()
                .zip(&stereo)
                .fold(0.0f32, |m, (a, b)| m.max((a - b).abs()));
            assert!(worst < 1e-5, "{name}: {worst}");
        }
        s.bands[0].stereo = StereoMode::Side;
        let (a, _, da, _) = run_stereo(&s, &x, &x);
        assert!(
            a.iter().zip(&da).all(|(p, q)| (p - q).abs() < 1e-6),
            "no side, nothing to saturate"
        );
    }

    #[test]
    fn stereo_mode_auto_gain_holds_the_level() {
        // Mid and side at different levels: the part driven comes out as loud as it went in.
        let (m, sd) = (noise(72 * N, 3, 0.25), noise(72 * N, 5, 0.1));
        let l: Vec<f32> = m.iter().zip(&sd).map(|(a, b)| a + b).collect();
        let r: Vec<f32> = m.iter().zip(&sd).map(|(a, b)| a - b).collect();
        let rms = |x: &[f32]| {
            (x.iter().map(|v| f64::from(*v).powi(2)).sum::<f64>() / x.len() as f64).sqrt()
        };
        for (mode, side) in [(StereoMode::Mid, false), (StereoMode::Side, true)] {
            let mut s = band(Model::Tube, 18.0);
            s.auto_gain = true;
            s.bands[0].stereo = mode;
            let (a, b, da, db) = run_stereo(&s, &l, &r);
            let t = a.len() - 12 * N;
            let part = |x: &[f32], y: &[f32]| -> Vec<f32> {
                x[t..]
                    .iter()
                    .zip(&y[t..])
                    .map(|(p, q)| if side { 0.5 * (p - q) } else { 0.5 * (p + q) })
                    .collect()
            };
            let db_off = 20.0 * (rms(&part(&a, &b)) / rms(&part(&da, &db))).log10();
            assert!(db_off.abs() < 0.5, "{mode:?}: {db_off:.2} dB");
        }
    }

    #[test]
    fn a_stereo_mode_change_never_steps() {
        // A change every so often: no sample steps further than any mode's steady wave does.
        let (l, r) = (tone(12 * N, 0.05, 40), tone(12 * N, 0.04, 61));
        let mut s = band(Model::Tube, 12.0);
        let step = |x: &[f32]| {
            x[8192..]
                .windows(2)
                .fold(0.0f32, |m, w| m.max((w[1] - w[0]).abs()))
        };
        let mut steady = 0.0f32;
        for mode in StereoMode::ALL {
            s.bands[0].stereo = mode;
            let (a, b, _, _) = run_stereo(&s, &l, &r);
            steady = steady.max(step(&a)).max(step(&b));
        }
        s.bands[0].stereo = StereoMode::Stereo;
        let mut sat = Saturator::new(FS, 4, &s);
        let (mut a, mut b) = (l, r);
        let (mut da, mut db) = (vec![0.0; a.len()], vec![0.0; a.len()]);
        for (count, ((x, y), (u, v))) in a
            .chunks_mut(64)
            .zip(b.chunks_mut(64))
            .zip(da.chunks_mut(64).zip(db.chunks_mut(64)))
            .enumerate()
        {
            s.bands[0].stereo = StereoMode::ALL[(count / 40) % 4];
            sat.process(&mut [x, y], &mut [u, v], &s);
        }
        let worst = step(&a).max(step(&b));
        assert!(
            worst < 1.3 * steady,
            "largest step {worst}, steady {steady}"
        );
    }

    #[test]
    fn mono_ignores_the_stereo_mode() {
        let x = tone(2 * N, 0.2, K);
        let mut s = band(Model::Soft, 12.0);
        let mut sat = Saturator::new(FS, 2, &s);
        let (want, _) = run(&mut sat, &x, &s, 256);
        s.bands[0].stereo = StereoMode::Side;
        let mut sat = Saturator::new(FS, 2, &s);
        let (got, _) = run(&mut sat, &x, &s, 256);
        assert_eq!(want, got);
    }

    #[test]
    fn a_track_at_minus_18_is_gentle_low_and_reaches_the_knee_by_36() {
        // The calibration (state 5): a −18 dBFS sine at 36 dB of drive has audible harmonics on
        // the curves a kick would be heard through; at 6 dB it is gently coloured, every harmonic
        // under −40 dB (the old law had no such range: 3 dB already gave 6 % THD+N).
        let amp = 10f32.powf(-18.0 / 20.0);
        for m in [Model::Tube, Model::Soft, Model::Hard, Model::Diode, Model::Tape, Model::Transformer] {
            let at = |drive_db: f32| probe_harmonics(&Probe {
                model: m,
                amp,
                drive_db,
                bias: 0.0,
                freq: 1000.0,
                tone_db: 0.0,
                pivot: TONE_PIVOT,
                mix: 1.0,
                fs: FS,
                recipe: [0.0; HARMONICS],
            });
            let gentle = at(6.0);
            for k in 1..6 {
                // Tube leads with its second harmonic (its asymmetry): a little more there.
                let bound = if m == Model::Tube && k == 1 { -38.0 } else { -40.0 };
                assert!(gentle.db(k) < bound, "{m:?} at 6 dB: H{} {:.1} dB", k + 1, gentle.db(k));
            }
            if matches!(m, Model::Tape | Model::Transformer) {
                continue;
            }
            let h = probe_harmonics(&Probe {
                model: m,
                amp,
                drive_db: 36.0,
                bias: 0.0,
                freq: 1000.0,
                tone_db: 0.0,
                pivot: TONE_PIVOT,
                mix: 1.0,
                fs: FS,
                recipe: [0.0; HARMONICS],
            });
            assert!(h.db(1).max(h.db(2)) > -30.0, "{m:?}: H2 {:.1}, H3 {:.1} dB", h.db(1), h.db(2));
        }
    }

    #[test]
    fn the_probe_marks_what_the_way_down_removes() {
        // A 9 kHz tone at 48 kHz: H2 (18 kHz) is heard, H3 (27 kHz) is not, though the curve makes it.
        let p = Probe {
            model: Model::Soft,
            amp: 0.5,
            drive_db: 36.0,
            bias: 0.0,
            freq: 9000.0,
            tone_db: 0.0,
            pivot: TONE_PIVOT,
            mix: 1.0,
            fs: FS,
            recipe: [0.0; HARMONICS],
        };
        let h = probe_harmonics(&p);
        assert_eq!(&h.hollow[..3], &[false, false, true]);
        assert!(h.amp[2] > 0.01, "the curve makes H3: {}", h.amp[2]);
    }

    #[test]
    fn a_clean_band_probes_clean() {
        // Drive 0 or mix 0 is the band itself: the tone and nothing else, at its own level.
        for (drive, mix) in [(0.0f32, 1.0f32), (18.0, 0.0)] {
            let h = probe_harmonics(&Probe {
                model: Model::Tube,
                amp: 0.5,
                drive_db: drive,
                bias: 0.0,
                freq: 1000.0,
                tone_db: 0.0,
                pivot: TONE_PIVOT,
                mix,
                fs: FS,
                recipe: [0.0; HARMONICS],
            });
            assert!((h.amp[0] - 0.5).abs() < 1e-6, "H1 {}", h.amp[0]);
            assert!(h.amp[1..].iter().all(|&a| a == 0.0));
        }
        // Rectify trades the fundamental for the octave: H2 is the reference.
        let h = probe_harmonics(&Probe {
            model: Model::Rectify,
            amp: 0.5,
            drive_db: 14.0,
            bias: 0.0,
            freq: 1000.0,
            tone_db: 0.0,
            pivot: TONE_PIVOT,
            mix: 1.0,
            fs: FS,
            recipe: [0.0; HARMONICS],
        });
        assert_eq!(h.db(1), 0.0);
    }

    #[test]
    fn soft_matches_its_closed_form_harmonics() {
        // f(u) = u − 4u³/27: a sine of amplitude U gives H1 = U − U³/9 and H3 = U³/27, and the
        // calibration and the drive under `LEVEL_DB` come off afterwards.
        let (amp, drive) = (0.1f32, 32.0f32);
        let k = f64::from(CALIBRATION / out_scale(drive));
        let u = f64::from(amp * crate::db_to_gain(drive) * CALIBRATION);
        let s = band(Model::Soft, drive);
        let mut sat = Saturator::new(FS, 8, &s);
        let (y, _) = run(&mut sat, &tone(4 * N, amp, K), &s, 512);
        let (h1, h3) = (harmonic(&y, 1), harmonic(&y, 3));
        assert!(
            (h1 * k / (u - u.powi(3) / 9.0) - 1.0).abs() < 1e-3,
            "H1 {h1}"
        );
        assert!((h3 * k / (u.powi(3) / 27.0) - 1.0).abs() < 2e-3, "H3 {h3}");
        assert!(harmonic(&y, 2) < 1e-6 && harmonic(&y, 5) < 1e-6);
    }

    /// Band power (mean square) of `y` around `f` Hz (± a sixth of an octave), by DFT bins.
    fn band_power(y: &[f32], f: f64) -> f64 {
        let n = 16_384.min(y.len());
        let tail = &y[y.len() - n..];
        let bin = f64::from(FS) / n as f64;
        let (lo, hi) = ((f * 0.89 / bin) as usize, (f * 1.12 / bin) as usize + 1);
        (lo..hi)
            .map(|k| {
                let w = std::f64::consts::TAU * k as f64 / n as f64;
                let (re, im) = tail.iter().enumerate().fold((0.0, 0.0), |(re, im), (i, &v)| {
                    (re + f64::from(v) * (w * i as f64).cos(), im + f64::from(v) * (w * i as f64).sin())
                });
                re * re + im * im
            })
            .sum()
    }

    #[test]
    fn a_drawn_recipe_is_what_a_full_scale_sine_makes() {
        // The curve alone, a sine reaching ±1: each harmonic within 0.5 dB of its target against
        // the fundamental, the ones left off under −90 dB.
        for db in [DEFAULT_HARMONICS_DB, [-6.0, -12.0, -72.0, -20.0, -72.0, -30.0, -72.0, -40.0], [-72.0, -10.0, -72.0, -16.0, -72.0, -22.0, -72.0, -28.0]] {
            let a = recipe(&db);
            let n = 4096;
            let y: Vec<f64> = (0..n).map(|i| f64::from(super::harmonic((std::f64::consts::TAU * i as f64 / n as f64).sin() as f32, &a))).collect();
            let amp = |k: usize| {
                let w = std::f64::consts::TAU * k as f64 / n as f64;
                let (re, im) = y.iter().enumerate().fold((0.0, 0.0), |(re, im), (i, v)| (re + v * (w * i as f64).cos(), im + v * (w * i as f64).sin()));
                2.0 * re.hypot(im) / n as f64
            };
            let f = amp(1);
            for (i, &want) in db.iter().enumerate() {
                let got = 20.0 * (amp(i + 2) / f).log10();
                if want <= HARMONIC_FLOOR_DB {
                    assert!(got < -90.0, "{db:?}: H{} {got:.1} dB, should be off", i + 2);
                } else {
                    assert!((got - f64::from(want)).abs() < 0.5, "{db:?}: H{} {got:.1} dB, wanted {want}", i + 2);
                }
            }
            // Slope 1 at 0: quiet audio passes as it is.
            let slope = (super::harmonic(1e-4, &a) - super::harmonic(-1e-4, &a)) / 2e-4;
            assert!((slope - 1.0).abs() < 1e-3, "slope {slope}");
        }
    }

    #[test]
    fn punch_drives_the_hits_and_body_the_tails() {
        // 60 Hz hits decaying over 80 ms through Soft: Punch adds its distortion in the first 20 ms
        // of each hit, Body after it; both at 0 is the band bit for bit.
        let fs = FS as usize;
        let n = 2 * fs;
        let x: Vec<f32> = (0..n).map(|i| {
            let t = (i % (fs / 2)) as f64 / f64::from(FS);
            (0.4 * (-t / 0.08).exp() * (std::f64::consts::TAU * 60.0 * t).sin()) as f32
        }).collect();
        let play = |punch: f32, body: f32| {
            let mut s = band(Model::Soft, 30.0);
            s.bands[0].punch_db = punch;
            s.bands[0].body_db = body;
            let mut sat = Saturator::new(FS, 4, &s);
            run(&mut sat, &x, &s, 256).0
        };
        let plain = play(0.0, 0.0);
        let (head, tail) = (|i: usize| i % (fs / 2) < fs / 50, |i: usize| { let k = i % (fs / 2); k > fs / 25 && k < fs / 5 });
        let moved = |y: &[f32], part: &dyn Fn(usize) -> bool| {
            let (mut d, mut e) = (0.0f64, 0.0f64);
            for i in fs..n {
                if part(i) { d += f64::from(y[i] - plain[i]).powi(2); e += f64::from(plain[i]).powi(2); }
            }
            10.0 * (d / e).log10()
        };
        let punch = play(12.0, 0.0);
        let body = play(0.0, 12.0);
        let (ph, pt, bh, bt) = (moved(&punch, &head), moved(&punch, &tail), moved(&body, &head), moved(&body, &tail));
        assert!(ph > pt + 6.0, "punch: hits {ph:.1} dB, tails {pt:.1} dB");
        assert!(bt > bh + 6.0, "body: tails {bt:.1} dB, hits {bh:.1} dB");
        assert_eq!(play(0.0, 0.0), plain);
    }

    #[test]
    fn feedback_rings_at_its_frequency() {
        // Noise through a driven Soft band: feedback at 1 kHz lifts the band round 1 kHz, and
        // leaves 4 kHz about where it was.
        let x = noise(FS as usize, 3, 0.1);
        let out = |fb: f32| {
            let mut s = band(Model::Soft, 30.0);
            s.bands[0].feedback = fb;
            s.bands[0].feedback_hz = 1000.0;
            let mut sat = Saturator::new(FS, 4, &s);
            run(&mut sat, &x, &s, 256).0
        };
        let (dry, wet) = (out(0.0), out(1.0));
        let lift = |f: f64| 10.0 * (band_power(&wet, f) / band_power(&dry, f)).log10();
        let (at, away) = (lift(1000.0), lift(4000.0));
        assert!(at > 6.0, "1 kHz lifted {at:.1} dB");
        assert!(away.abs() < 3.0 && at > away + 6.0, "4 kHz {away:.1} dB against 1 kHz {at:.1} dB");
    }

    #[test]
    fn feedback_never_runs_away() {
        // Full feedback, every model, bias at both ends and none, driven gently to hard: a burst,
        // then silence, dies away (the loop gain stays under 1 where the curves are steepest).
        let n = 2 * FS as usize;
        let mut x = noise(n, 9, 0.5);
        for v in &mut x[FS as usize / 10..] {
            *v = 0.0;
        }
        for m in Model::ALL {
            for bias in [-1.0f32, 0.0, 1.0] {
                for drive in [24.0f32, 42.0, 60.0] {
                    for hz in [60.0f32, 1000.0, 8000.0] {
                        let mut s = band(m, drive);
                        s.bands[0].bias = bias;
                        s.bands[0].feedback = 1.0;
                        s.bands[0].feedback_hz = hz;
                        let mut sat = Saturator::new(FS, 2, &s);
                        let (y, _) = run(&mut sat, &x, &s, 512);
                        let tail = y[n - FS as usize / 4..].iter().fold(0.0f32, |p, v| p.max(v.abs()));
                        assert!(tail.is_finite() && tail < 1e-3, "{m:?} bias {bias} drive {drive} at {hz} Hz: {tail} after 1.65 s of silence");
                    }
                }
            }
        }
    }

    #[test]
    fn feedback_off_is_the_band_exactly() {
        let x = noise(FS as usize / 2, 4, 0.3);
        let play = |fb: Option<f32>| {
            let mut s = band(Model::Tube, 36.0);
            if let Some(fb) = fb {
                s.bands[0].feedback = fb;
                s.bands[0].feedback_hz = 3000.0;
            }
            let mut sat = Saturator::new(FS, 4, &s);
            run(&mut sat, &x, &s, 256).0
        };
        assert_eq!(play(None), play(Some(0.0)));
    }

    #[test]
    fn the_engine_plays_the_drawn_curve() {
        // Every static model, through the oversampled engine: its harmonics are those of the curve
        // the editor draws, within 0.05 dB down to −80 dB under the fundamental, biased too (the
        // curve is static: the point follows the sample, not an envelope).
        for m in [
            Model::Diode,
            Model::Soft,
            Model::Hard,
            Model::Fold,
            Model::Rectify,
        ] {
            for (amp, drive, bias) in [
                (0.0625f32, 12.0f32, 0.0f32),
                (0.025, 24.0, 0.4),
                (0.125, 3.0, -0.6),
            ] {
                let mut s = band(m, drive);
                s.bands[0].bias = bias;
                let mut sat = Saturator::new(FS, 8, &s);
                let (y, _) = run(&mut sat, &tone(6 * N, amp, K), &s, 333);
                let reference = (1..=5)
                    .map(|h| curve_harmonic(m, amp, drive, bias, h))
                    .fold(0.0, f64::max);
                for h in 1..=5 {
                    let want = curve_harmonic(m, amp, drive, bias, h);
                    if want < reference * 1e-4 {
                        continue;
                    }
                    let got = harmonic(&y, h);
                    let db = 20.0 * (got / want).log10();
                    // Or within −60 dB of the input: in the fade-in a partial can be the dry and the
                    // wet nearly cancelling (Rectify's fundamental at 3 dB: −28 dB of the input),
                    // where the difference's 5 Hz high-pass shows.
                    assert!(
                        db.abs() < 0.05 || (got - want).abs() < 1e-3 * f64::from(amp),
                        "{m:?} amp {amp} drive {drive} bias {bias}: H{h} {db:.3} dB off"
                    );
                }
            }
        }
    }

    /// Harmonic `h` of a tone at bin `K` of `N`, over the last `over·N` samples of a signal at
    /// `over` times the base rate.
    fn harmonic_at(y: &[f32], h: usize, over: usize) -> f64 {
        let n = N * over;
        let tail = &y[y.len() - n..];
        let w = std::f64::consts::TAU * (h * K) as f64 / n as f64;
        let (re, im) = tail
            .iter()
            .enumerate()
            .fold((0.0, 0.0), |(re, im), (i, &v)| {
                (
                    re + f64::from(v) * (w * i as f64).cos(),
                    im + f64::from(v) * (w * i as f64).sin(),
                )
            });
        2.0 * re.hypot(im) / n as f64
    }

    #[test]
    fn tape_and_transformer_play_the_drawn_loop() {
        // The engine at 8× against `magnet_response` (what the editor's loop is drawn from) on
        // the same tone at the high rate: the same harmonics within 0.1 dB down to −60 dB.
        for m in [Model::Tape, Model::Transformer] {
            for (amp, drive, bias) in [(0.25f32, 12.0f32, 0.0f32), (0.1, 24.0, 0.4)] {
                let mut s = band(m, drive);
                s.bands[0].bias = bias;
                let mut sat = Saturator::new(FS, 8, &s);
                let (y, _) = run(&mut sat, &tone(6 * N, amp, K), &s, 333);
                let x_hi: Vec<f32> = (0..6 * N * 8)
                    .map(|i| {
                        amp * (std::f64::consts::TAU * (K * i) as f64 / (8 * N) as f64).sin() as f32
                    })
                    .collect();
                let want_y = magnet_response(m, &x_hi, drive, bias, 8.0 * FS);
                let reference = (1..=5)
                    .map(|h| harmonic_at(&want_y, h, 8))
                    .fold(0.0, f64::max);
                for h in 1..=5 {
                    let want = harmonic_at(&want_y, h, 8);
                    if want < reference * 1e-3 {
                        continue;
                    }
                    let db = 20.0 * (harmonic(&y, h) / want).log10();
                    assert!(
                        db.abs() < 0.1,
                        "{m:?} amp {amp} drive {drive} bias {bias}: H{h} {db:.3} dB off"
                    );
                }
            }
        }
    }

    #[test]
    fn tape_saturates_the_highs_first_and_transformer_the_lows() {
        // The third harmonic's share for a low (≈ 70 Hz) and a high (≈ 4.4 kHz) tone at the same
        // level: Tape distorts the high one more, Transformer the low one.
        let h3 = |m: Model, k: usize| {
            let s = band(m, 12.0);
            let mut sat = Saturator::new(FS, 4, &s);
            let x: Vec<f32> = (0..8 * N)
                .map(|i| 0.05 * (std::f64::consts::TAU * (k * i) as f64 / N as f64).sin() as f32)
                .collect();
            let (y, _) = run(&mut sat, &x, &s, 256);
            let at = |h: usize| {
                let tail = &y[y.len() - N..];
                let w = std::f64::consts::TAU * (h * k) as f64 / N as f64;
                let (re, im) = tail
                    .iter()
                    .enumerate()
                    .fold((0.0, 0.0), |(re, im), (i, &v)| {
                        (
                            re + f64::from(v) * (w * i as f64).cos(),
                            im + f64::from(v) * (w * i as f64).sin(),
                        )
                    });
                re.hypot(im)
            };
            20.0 * (at(3) / at(1)).log10()
        };
        let (tape_lo, tape_hi) = (h3(Model::Tape, 6), h3(Model::Tape, 377));
        let (iron_lo, iron_hi) = (h3(Model::Transformer, 6), h3(Model::Transformer, 377));
        assert!(
            tape_hi > tape_lo + 6.0,
            "tape: low {tape_lo:.1} dB, high {tape_hi:.1} dB"
        );
        assert!(
            iron_lo > iron_hi + 6.0,
            "transformer: low {iron_lo:.1} dB, high {iron_hi:.1} dB"
        );
    }

    #[test]
    fn tape_and_transformer_pass_quiet_signals_at_unity() {
        for m in [Model::Tape, Model::Transformer] {
            // Auto gain off. Under `LEVEL_DB` drive moves only the knee: unity. Above it drive is
            // gain as well: 36 dB is 12 dB louder, as the static curves.
            for (drive, gain) in [(12.0f32, 0.0f64), (36.0, 12.0)] {
                let s = band(m, drive);
                let mut sat = Saturator::new(FS, 2, &s);
                let (y, _) = run(&mut sat, &tone(4 * N, 0.002, K), &s, 256);
                let db = 20.0 * (harmonic(&y, 1) / 0.002).log10() - gain;
                assert!(db.abs() < 1.0, "{m:?} at {drive} dB: {db:.2} dB");
                assert!(harmonic(&y, 3) < harmonic(&y, 1) * 0.01, "{m:?}: H3");
            }
        }
    }

    #[test]
    fn symmetric_models_make_no_even_harmonics_and_tube_and_diode_do() {
        for m in [Model::Soft, Model::Hard, Model::Fold] {
            let s = band(m, 18.0);
            let mut sat = Saturator::new(FS, 4, &s);
            let (y, _) = run(&mut sat, &tone(4 * N, 0.3, K), &s, 256);
            let h1 = harmonic(&y, 1).max(harmonic(&y, 3));
            for h in [2, 4, 6] {
                assert!(harmonic(&y, h) < h1 * 1e-6, "{m:?}: H{h}");
            }
        }
        // A loop is odd-symmetric too, once the core has left its virgin curve (to f32 rounding).
        for m in [Model::Tape, Model::Transformer] {
            let s = band(m, 18.0);
            let mut sat = Saturator::new(FS, 4, &s);
            let (y, _) = run(&mut sat, &tone(6 * N, 0.3, K), &s, 256);
            let h1 = harmonic(&y, 1);
            for h in [2, 4, 6] {
                assert!(
                    harmonic(&y, h) < h1 * 1e-4,
                    "{m:?}: H{h} {}",
                    harmonic(&y, h) / h1
                );
            }
        }
        // Tube: even harmonics, more as it is pushed.
        let h2_share = |amp: f32| {
            let s = band(Model::Tube, 12.0);
            let mut sat = Saturator::new(FS, 4, &s);
            let (y, _) = run(&mut sat, &tone(8 * N, amp, K), &s, 256);
            harmonic(&y, 2) / harmonic(&y, 1)
        };
        let (quiet, loud) = (h2_share(0.0125), h2_share(0.1));
        assert!(quiet > 1e-3 && loud > 2.0 * quiet, "{quiet} {loud}");
    }

    /// H2 against H1 through the engine, dB, for a −18 dBFS tone.
    fn h2_db(m: Model, drive: f32, bias: f32) -> f64 {
        let mut s = band(m, drive);
        s.bands[0].bias = bias;
        let mut sat = Saturator::new(FS, 4, &s);
        let (y, _) = run(&mut sat, &tone(8 * N, 0.125, K), &s, 256);
        20.0 * (harmonic(&y, 2) / harmonic(&y, 1)).log10()
    }

    #[test]
    fn even_harmonics_hold_through_the_normal_range() {
        // The operating point grows with the signal (a smooth |u|), so a bias, or Tube by itself,
        // makes even harmonics through 36 dB of drive (H2 over −38 dB), not only gently. Past that
        // the waveform nears a square and they fade: keeping them needs the point to remember the
        // amplitude, which muted quiet audio after loud passages (user chose static, 2026-11-24).
        // Diode's halves differ in shape, so it keeps them at any drive.
        let mut cases: Vec<(Model, f32)> = vec![(Model::Tube, 0.0), (Model::Diode, 0.0)];
        cases.extend([Model::Tape, Model::Transformer, Model::Diode, Model::Soft, Model::Hard].map(|m| (m, 0.3)));
        cases.push((Model::Tube, -0.3));
        for (m, bias) in cases {
            // At 36 dB (well driven; at 24 a gentle Soft makes too little of anything to judge).
            let h2 = h2_db(m, 36.0, bias);
            assert!(h2 > -38.0, "{m:?} bias {bias}: H2 {h2:.1} dB at 36");
        }
        for bias in [0.0, 0.3] {
            let h2 = h2_db(Model::Diode, 60.0, bias);
            assert!(h2 > -26.0, "Diode bias {bias}: H2 {h2:.1} dB at 60");
        }
    }

    #[test]
    fn neutral_settings_null_bit_for_bit() {
        let input = noise(20_000, 3, 0.7);
        for factor in FACTORS {
            let mut cases = vec![band(Model::Hard, 0.0)];
            let mut bypassed = band(Model::Fold, 24.0);
            bypassed.bands[0].bypass = true;
            cases.push(bypassed);
            let mut dry = band(Model::Diode, 30.0);
            dry.bands[0].mix = 0.0;
            cases.push(dry);
            for s in cases {
                let mut sat = Saturator::new(FS, factor, &s);
                let lat = sat.latency();
                let (y, d) = run(&mut sat, &input, &s, 257);
                for i in lat..input.len() {
                    assert_eq!(y[i], input[i - lat], "{factor}× {:?} at {i}", s.bands[0]);
                    assert_eq!(d[i], input[i - lat]);
                }
            }
        }
    }

    #[test]
    fn latency_is_where_the_impulse_lands() {
        for factor in FACTORS {
            // Driven, but in the curve's linear stretch: a scaled, delayed impulse.
            let s = band(Model::Soft, 12.0);
            let mut sat = Saturator::new(FS, factor, &s);
            let mut x = vec![0.0f32; 4000];
            x[1000] = 1e-3;
            let (y, _) = run(&mut sat, &x, &s, 128);
            let peak = (0..y.len())
                .max_by(|&a, &b| y[a].abs().total_cmp(&y[b].abs()))
                .unwrap();
            assert_eq!(peak, 1000 + sat.latency(), "{factor}×");
            assert_eq!(sat.latency(), Oversampler::latency_for(factor));
        }
    }

    #[test]
    fn block_size_does_not_change_the_output() {
        let mut s = band(Model::Tube, 18.0);
        s.bands[0].tone_db = 4.0;
        s.bands[0].bias = 0.3;
        s.auto_gain = true;
        s.crossovers[1] = Some(700.0);
        s.bands[2].model = Model::Fold;
        s.bands[2].drive_db = 9.0;
        let input = noise(20_000, 4, 0.5);
        let mut reference = None;
        for block in [1, 7, 64, 128, 4096] {
            let mut sat = Saturator::new(FS, 2, &s);
            let (y, _) = run(&mut sat, &input, &s, block);
            match &reference {
                None => reference = Some(y),
                Some(r) => assert_eq!(&y, r, "block {block}"),
            }
        }
    }

    /// Renders `x` switching from `before` to `after` at `at`, and both held: no step between
    /// neighbouring samples of the switch may exceed what either held setting makes there by more
    /// than 0.01 (a crossfade's own movement), so the change itself never clicks.
    fn assert_change_is_clean(
        x: &[f32],
        before: &SatSettings,
        after: &SatSettings,
        at: usize,
        factor: usize,
    ) {
        let render = |switch: Option<usize>, first: &SatSettings| {
            let mut sat = Saturator::new(FS, factor, first);
            let mut y = x.to_vec();
            let mut pos = 0;
            for chunk in y.chunks_mut(480) {
                let s = if switch.is_some_and(|k| pos >= k) {
                    after
                } else {
                    first
                };
                sat.process(&mut [chunk], &mut [], s);
                pos += 480;
            }
            y
        };
        let (y, held_a, held_b) = (
            render(Some(at), before),
            render(None, before),
            render(None, after),
        );
        let step = |v: &[f32], i: usize| (v[i + 1] - v[i]).abs();
        // The held settings' steepest step within half a period of the tone either side (edges
        // move in time as a setting glides).
        let near = |v: &[f32], i: usize| {
            (i.saturating_sub(512)..(i + 512).min(x.len() - 1))
                .map(|j| step(v, j))
                .fold(0.0, f32::max)
        };
        for i in at..(at + 9600).min(x.len() - 1) {
            let allowed = near(&held_a, i).max(near(&held_b, i)) + 0.01;
            assert!(
                step(&y, i) <= allowed,
                "step {} at {} (held: {} / {}) {:?} → {:?}",
                step(&y, i),
                i - at,
                step(&held_a, i),
                step(&held_b, i),
                before.bands[0],
                after.bands[0]
            );
        }
    }

    #[test]
    fn moving_drive_bias_tone_and_switching_model_never_clicks() {
        let x = tone(48_000, 0.4, 4);
        let base = SatSettings {
            auto_gain: true,
            ..band(Model::Hard, 30.0)
        };
        let with = |f: &dyn Fn(&mut BandSat)| {
            let mut s = base;
            f(&mut s.bands[0]);
            s
        };
        let cases = [
            (with(&|b| b.drive_db = 0.0), base),
            (base, with(&|b| b.bias = 1.0)),
            (base, with(&|b| b.tone_db = -12.0)),
            (with(&|b| b.model = Model::Fold), base),
            (
                with(&|b| b.model = Model::Tube),
                with(&|b| b.model = Model::Rectify),
            ),
            // Auto gain off: a trim measured on Tape settles onto Transformer over seconds.
            (
                SatSettings {
                    auto_gain: false,
                    ..with(&|b| b.model = Model::Tape)
                },
                SatSettings {
                    auto_gain: false,
                    ..with(&|b| b.model = Model::Transformer)
                },
            ),
            (base, with(&|b| b.model = Model::Tape)),
            (base, with(&|b| b.bypass = true)),
            (with(&|b| b.bypass = true), base),
            (base, with(&|b| b.mix = 0.0)),
        ];
        for (before, after) in &cases {
            assert_change_is_clean(&x, before, after, 14_400, 2);
        }
    }

    #[test]
    fn bands_at_rest_sum_flat_and_a_band_made_does_not_click() {
        let mut s = band(Model::Soft, 0.0);
        s.crossovers = [
            Some(120.0),
            Some(500.0),
            Some(2000.0),
            Some(6000.0),
            Some(12_000.0),
        ];
        let mut sat = Saturator::new(FS, 4, &s);
        let mut impulse = vec![0.0f32; 16_384];
        impulse[0] = 1.0;
        let (y, d) = run(&mut sat, &impulse, &s, 512);
        assert_eq!(y, d);
        for f in [40.0, 120.0, 900.0, 6000.0, 15_000.0] {
            let w = std::f64::consts::TAU * f / f64::from(FS);
            let (re, im) = y.iter().enumerate().fold((0.0, 0.0), |(re, im), (i, &v)| {
                (
                    re + f64::from(v) * (w * i as f64).cos(),
                    im - f64::from(v) * (w * i as f64).sin(),
                )
            });
            let db = 10.0 * (re * re + im * im).log10();
            assert!(db.abs() < 0.01, "{f} Hz: {db}");
        }
        // A band split off right on a driven tone, and merged back.
        let x = tone(48_000, 0.3, 9);
        let one = band(Model::Tube, 12.0);
        let mut two = one;
        two.crossovers[2] = Some(105.0);
        assert_change_is_clean(&x, &one, &two, 19_200, 2);
        assert_change_is_clean(&x, &two, &one, 19_200, 2);
    }

    #[test]
    fn auto_gain_keeps_the_level() {
        let rms = |v: &[f32]| {
            (v.iter().map(|x| f64::from(*x).powi(2)).sum::<f64>() / v.len() as f64).sqrt()
        };
        for m in [
            Model::Tape,
            Model::Tube,
            Model::Transformer,
            Model::Diode,
            Model::Soft,
            Model::Hard,
            Model::Fold,
        ] {
            for drive in [6.0f32, 18.0, 36.0] {
                // The trim settles over seconds (1.5 s time constant): judge the last two.
                for input in [tone(288_000, 0.25, K), noise(288_000, 9, 0.3)] {
                    let mut s = band(m, drive);
                    s.auto_gain = true;
                    let mut sat = Saturator::new(FS, 2, &s);
                    let (y, _) = run(&mut sat, &input, &s, 512);
                    let tail = 192_000..;
                    let db = 20.0 * (rms(&y[tail.clone()]) / rms(&input[tail])).log10();
                    assert!(db.abs() < 1.0, "{m:?} at {drive} dB: {db:.2} dB");
                }
            }
        }
    }

    #[test]
    fn auto_gain_matches_the_hits_not_the_gaps() {
        // Sparse material: a 60 Hz hit a second decaying over 60 ms, over noise 40 dB down (a kick
        // and its bleed). Saturation lifts the gaps by the whole drive; auto gain must match the
        // hits, not the average they make with the gaps (that pulled the hits down by as much).
        let n = 6 * FS as usize;
        let period = FS as usize;
        // The hit: until it has fallen 10 dB (69 ms of a 60 ms decay), what auto gain's gate
        // hears as the hit. Saturation flattens a kick (with auto gain off its first 10 ms come out
        // 6 dB under its tail), so a matched hit's attack is a little under the input and its tail
        // over: a window of the attack alone read −2.7 dB with the energy matched.
        let burst = |i: usize| i % period < period * 69 / 1000;
        let bed = noise(n, 5, 0.01);
        let x: Vec<f32> = (0..n)
            .map(|i| {
                let t = (i % period) as f64 / f64::from(FS);
                bed[i] + (0.5 * (-t / 0.06).exp() * (std::f64::consts::TAU * 60.0 * t).sin()) as f32
            })
            .collect();
        for m in [Model::Tube, Model::Hard, Model::Tape] {
            let mut s = band(m, 60.0);
            s.auto_gain = true;
            let mut sat = Saturator::new(FS, 2, &s);
            let lat = sat.latency();
            let (y, _) = run(&mut sat, &x, &s, 512);
            let (mut pin, mut pout, mut k) = (0.0f64, 0.0f64, 0usize);
            for i in 3 * FS as usize..n - lat {
                if burst(i) {
                    pin += f64::from(x[i]).powi(2);
                    pout += f64::from(y[i + lat]).powi(2);
                    k += 1;
                }
            }
            let db = 10.0 * (pout / pin).log10();
            assert!(
                k > 0 && db.abs() < 2.0,
                "{m:?}: the hits come out {db:.2} dB off"
            );
        }
    }

    #[test]
    fn the_dynamics_gain_is_a_ratio_above_the_threshold() {
        let g = dynamics_gain_db;
        // Below the knee: nothing, whatever the amount.
        for a in [-1.0, -0.3, 0.4, 1.0] {
            assert_eq!(g(a, -24.0, -40.0), 0.0);
        }
        assert_eq!(g(0.0, -24.0, 0.0), 0.0);
        // Above it: −50 % is 2:1, −100 % ∞:1, +100 % 1:2.
        assert!((g(-0.5, -24.0, -12.0) + 6.0).abs() < 1e-4);
        assert!((g(-1.0, -24.0, -12.0) + 12.0).abs() < 1e-4);
        assert!((g(-0.75, -30.0, -10.0) + 15.0).abs() < 1e-4);
        assert!((g(1.0, -24.0, -21.0) - 3.0).abs() < 1e-4);
        // The lift is capped, softly, and it never turns back.
        let mut last = 0.0f32;
        for l in (-40..=0).map(|d| d as f32) {
            let v = g(1.0, -40.0, l);
            assert!(v >= last && v <= DYN_LIFT_MAX_DB, "{l}: {v}");
            last = v;
        }
        assert!(last > DYN_LIFT_MAX_DB - 0.1);
        // Smooth through the knee: no step between neighbouring levels.
        for k in 0..1200 {
            let l = -30.0 + k as f32 * 0.01;
            assert!((g(-1.0, -24.0, l + 0.01) - g(-1.0, -24.0, l)).abs() < 0.0101);
        }
    }

    /// The level of the last `N` samples against the input's, dB.
    fn settled_db(y: &[f32], x: &[f32]) -> f64 {
        let rms = |v: &[f32]| {
            (v.iter().map(|a| f64::from(*a).powi(2)).sum::<f64>() / v.len() as f64).sqrt()
        };
        20.0 * (rms(&y[y.len() - N..]) / rms(&x[x.len() - N..])).log10()
    }

    /// Hits every half second (a 60 Hz thump decaying over 80 ms, peak `amp`) over noise 40 dB
    /// under them.
    fn hits(n: usize, amp: f32) -> Vec<f32> {
        let bed = noise(n, 11, amp * 0.01);
        (0..n)
            .map(|i| {
                let t = (i % (FS as usize / 2)) as f64 / f64::from(FS);
                bed[i]
                    + (f64::from(amp)
                        * (-t / 0.08).exp()
                        * (std::f64::consts::TAU * 60.0 * t).sin()) as f32
            })
            .collect()
    }

    /// The loudest sample in each hit and in each gap of the last second (hits at the start of each
    /// half second), dB, averaged.
    fn hit_and_gap_db(y: &[f32], lat: usize) -> (f64, f64) {
        let half = FS as usize / 2;
        let peak = |r: std::ops::Range<usize>| {
            20.0 * f64::from(y[r].iter().fold(0.0f32, |m, v| m.max(v.abs()))).log10()
        };
        let starts = [y.len() - 2 * half, y.len() - half];
        let hit = starts
            .iter()
            .map(|&s| peak(s + lat..s + lat + 2400))
            .sum::<f64>()
            / 2.0;
        let gap = starts
            .iter()
            .map(|&s| peak(s + lat + 17_000..s + lat + 23_000))
            .sum::<f64>()
            / 2.0;
        (hit, gap)
    }

    #[test]
    fn the_dynamics_leave_steady_material_alone_and_move_the_hits_at_any_level() {
        let run_with = |x: &[f32], amount: f32| {
            let mut s = band(Model::Soft, 0.0);
            s.bands[0].dyn_amount = amount;
            // A quick attack, so a hit's first peak is caught, and a short release, so the gaps measured
            // are past the hits' recovery.
            s.bands[0].dyn_release_ms = 50.0;
            s.bands[0].dyn_attack_ms = 1.0;
            let mut sat = Saturator::new(FS, 2, &s);
            let lat = sat.latency();
            (run(&mut sat, x, &s, 512).0, lat)
        };
        // A steady tone sits at its own average: next to nothing happens.
        let x = tone(96_000, 0.5, K);
        let (y0, _) = run_with(&x, 0.0);
        for amount in [-1.0, 1.0] {
            let (y, _) = run_with(&x, amount);
            let db = settled_db(&y, &x) - settled_db(&y0, &x);
            assert!(db.abs() < 1.5, "steady tone at {amount}: {db:.2} dB");
        }
        // Hits over a quiet bed: compressing pulls them down, lifting pushes them up, the bed stays;
        // the same 20 dB lower (the threshold is the band's own level, not a fixed one).
        for amp in [0.5f32, 0.05] {
            let x = hits(4 * FS as usize, amp);
            let (y0, lat) = run_with(&x, 0.0);
            let (hit0, gap0) = hit_and_gap_db(&y0, lat);
            let (yc, _) = run_with(&x, -1.0);
            let (ye, _) = run_with(&x, 1.0);
            let (hc, gc) = hit_and_gap_db(&yc, lat);
            let (he, ge) = hit_and_gap_db(&ye, lat);
            assert!(
                hc < hit0 - 6.0,
                "{amp}: compressed hits {hc:.1} against {hit0:.1}"
            );
            assert!(
                he > hit0 + 4.0,
                "{amp}: lifted hits {he:.1} against {hit0:.1}"
            );
            assert!(
                (gc - gap0).abs() < 1.0 && (ge - gap0).abs() < 1.0,
                "{amp}: the bed moved ({gc:.1}, {ge:.1} against {gap0:.1})"
            );
            if amp < 0.5 {
                continue;
            }
            // Record the loud case's moves to compare with the quiet one.
            let quiet = hits(4 * FS as usize, 0.05);
            let (q0, _) = run_with(&quiet, 0.0);
            let (qc, _) = run_with(&quiet, -1.0);
            let moved = |a: &[f32], b: &[f32]| hit_and_gap_db(b, lat).0 - hit_and_gap_db(a, lat).0;
            assert!(
                (moved(&y0, &yc) - moved(&q0, &qc)).abs() < 1.0,
                "level dependence: {:.2} vs {:.2}",
                moved(&y0, &yc),
                moved(&q0, &qc)
            );
        }
    }

    #[test]
    fn dynamics_follow_their_attack_and_release() {
        // A step from −40 to −6 dBFS and back: the gain covers about 63 % of its way within the
        // attack, and lets go to about 37 % within the release.
        let n = 48_000;
        let x: Vec<f32> = (0..n).map(|i| if (12_000..30_000).contains(&i) { 0.5 } else { 0.01 } * (std::f64::consts::TAU * 1000.0 * i as f64 / f64::from(FS)).sin() as f32).collect();
        let mut s = band(Model::Soft, 0.0);
        s.bands[0].dyn_amount = -1.0;
        s.bands[0].dyn_attack_ms = 20.0;
        s.bands[0].dyn_release_ms = 200.0;
        let mut sat = Saturator::new(FS, 1, &s);
        let mut gains = Vec::new();
        let mut y = x.clone();
        for chunk in y.chunks_mut(CHUNK) {
            let t = sat.process(&mut [chunk], &mut [], &s);
            gains.push((20.0 * t.dynamics[0].log10(), 20.0 * t.threshold[0].log10()));
        }
        let at = |ms: f64| gains[(ms * f64::from(FS) / 1000.0) as usize / CHUNK];
        let (g, thr) = at(250.0 + 20.0);
        let a = g / dynamics_gain_db(-1.0, thr, -6.02);
        assert!((0.5..0.75).contains(&a), "attack: {a}");
        let before = at(624.0).0;
        let r = at(625.0 + 200.0).0 / before;
        assert!(
            before < -10.0 && (0.25..0.45).contains(&r),
            "release: {r} from {before}"
        );
    }

    #[test]
    fn dynamics_null_when_bypassed_and_never_click() {
        let input = noise(20_000, 7, 0.7);
        let mut s = band(Model::Fold, 24.0);
        s.bands[0].dyn_amount = -1.0;
        s.bands[0].bypass = true;
        let mut sat = Saturator::new(FS, 2, &s);
        let lat = sat.latency();
        let (y, _) = run(&mut sat, &input, &s, 257);
        for i in lat..input.len() {
            assert_eq!(y[i], input[i - lat], "at {i}");
        }
        let x = tone(48_000, 0.4, 4);
        let base = band(Model::Hard, 12.0);
        let with = |f: &dyn Fn(&mut BandSat)| {
            let mut s = base;
            f(&mut s.bands[0]);
            s
        };
        let cases = [
            (base, with(&|b| b.dyn_amount = -1.0)),
            (base, with(&|b| b.dyn_amount = 1.0)),
            (
                with(&|b| b.dyn_amount = 1.0),
                with(&|b| b.dyn_amount = -1.0),
            ),
            (
                with(&|b| b.dyn_amount = -1.0),
                with(&|b| {
                    b.dyn_amount = -1.0;
                    b.dyn_release_ms = 10.0
                }),
            ),
            (
                with(&|b| {
                    b.dyn_amount = -1.0;
                    b.dyn_attack_ms = 0.1
                }),
                with(&|b| {
                    b.dyn_amount = -1.0;
                    b.dyn_attack_ms = 0.1;
                    b.bypass = true
                }),
            ),
        ];
        for (before, after) in &cases {
            assert_change_is_clean(&x, before, after, 14_400, 2);
        }
    }

    #[test]
    fn aliasing_stays_where_the_report_has_it() {
        // docs/dsp/sat-aliasing.md: a 5 kHz tone at −6 dBFS, drive 18 dB. A clean pass is the
        // oversampler alone (its filters reject more than 100 dB).
        let at = |m: Model, f: usize| distortion(m, f, FS, 1709, 0.5, 18.0, 0.0).alias_db;
        assert!(distortion(Model::Soft, 8, FS, 1709, 0.01, 0.0, 0.0).alias_db < -100.0);
        for (m, worst_8x) in [
            (Model::Tube, -38.0),
            (Model::Hard, -34.0),
            (Model::Diode, -40.0),
        ] {
            let (one, eight) = (at(m, 1), at(m, 8));
            assert!(eight < worst_8x, "{m:?} at 8×: {eight:.1} dB");
            assert!(
                eight < one - 20.0,
                "{m:?}: 8× {eight:.1} against 1× {one:.1}"
            );
        }
    }

    #[test]
    fn the_tilt_pivots_on_its_frequency() {
        let t = Tilt::new(12.0, 800.0, FS);
        let db = |f: f64| 20.0 * t.magnitude(f, f64::from(FS)).log10();
        assert!(db(800.0).abs() < 0.01, "{}", db(800.0));
        assert!(
            (db(20.0) + 6.0).abs() < 0.3 && (db(20_000.0) - 6.0).abs() < 0.6,
            "{} {}",
            db(20.0),
            db(20_000.0)
        );
        assert_eq!(Tilt::new(0.0, 800.0, FS), Tilt::IDENTITY);
    }

    #[test]
    fn stereo_and_nan_input_recover() {
        let s = band(Model::Hard, 20.0);
        let mut sat = Saturator::new(FS, 4, &s);
        let mut l = noise(4096, 5, 0.5);
        let mut r = noise(4096, 6, 0.5);
        l[100] = f32::NAN;
        sat.process(&mut [&mut l, &mut r], &mut [], &s);
        sat.reset(&s);
        let mut l = noise(4096, 7, 0.5);
        let mut r = noise(4096, 8, 0.5);
        sat.process(&mut [&mut l, &mut r], &mut [], &s);
        assert!(l.iter().chain(&r).all(|v| v.is_finite()));
    }
}
