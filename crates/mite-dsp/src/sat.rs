//! Mite Sat's engine (BLUEPRINT §6.7, `docs/design/sat-brief.md`): saturation on one band or
//! split into up to six by the shared LR4 crossover (`crate::crossover`), each band with its own
//! model, drive, bias, tone, mix and output, oversampled 1/2/4/8×.
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
//! Every curve is smooth (C¹): the oversampler does the anti-aliasing. First-order ADAA was
//! dropped (sat-brief §1.2): on the nonlinear part alone it leaks `g·Δu/2` at high drive; on the
//! whole curve its half-sample delay combs against the dry path.

use std::sync::OnceLock;

use crate::crossover::{Cross, Split};
use crate::hysteresis::{self, anhysteretic, Ja, JaParams};
use crate::oversample::Oversampler;

pub use crate::crossover::{FREQ_MAX, FREQ_MIN, MIN_GAP_OCT};

pub const BANDS: usize = 6;
pub const CROSSOVERS: usize = BANDS - 1;
pub const DRIVE_MAX_DB: f32 = 36.0;
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
pub const TONE_MAX_DB: f32 = 12.0;
/// Where each crossover slot sits when it is first made, Hz.
pub const DEFAULT_CROSSOVERS: [f32; CROSSOVERS] = [120.0, 500.0, 2000.0, 6000.0, 12_000.0];
/// The tone tilt's pivot with one band, Hz (with several, each band's geometric centre).
pub const TONE_PIVOT: f32 = 800.0;
/// Oversampling factors offered (the `oversampling` parameter's choices).
pub const FACTORS: [usize; 4] = [1, 2, 4, 8];

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
/// The band level auto gain reads (mean square), ms.
const LEVEL_MS: f64 = 300.0;
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
const HI_CH: usize = CHUNK * 8;

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
}

impl Model {
    pub const ALL: [Model; 8] = [Model::Tape, Model::Tube, Model::Transformer, Model::Diode, Model::Soft, Model::Hard, Model::Fold, Model::Rectify];
    pub const NAMES: [&'static str; 8] = ["Tape", "Tube", "Transformer", "Diode", "Soft", "Hard", "Fold", "Rectify"];
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
        Model::Tube => {
            if u > 0.0 {
                0.8 * (u / 0.8).tanh()
            } else {
                -1.25 * (1.0 - (u / 1.25).exp())
            }
        }
        // An unmatched pair: one diode conducts the top, two in series the bottom, so the halves
        // differ in shape at any level (even harmonics without bias).
        Model::Diode => {
            let k = if u > 0.0 { DIODE_KNEES.0 } else { DIODE_KNEES.1 };
            k * (u / k).asinh()
        }
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
    }
}

/// Where drive 0 dB puts the curves' knees: this far (12 dB) under full scale, so a track at a
/// sensible level reaches them within the first few dB of drive rather than past +18 dB. It goes in
/// before the curve and comes off after it, so quiet signals keep slope `g` and only the knees move.
pub const CALIBRATION: f32 = 3.981_071_7;

/// The share of the saturation heard at `drive_db` (0 at 0 dB, 1 from [`ENGAGE_DB`], S-shaped).
#[inline]
pub fn engagement(drive_db: f32) -> f32 {
    let t = (drive_db / ENGAGE_DB).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Diode: the knees of its top and bottom halves (one diode against two).
const DIODE_KNEES: (f32, f32) = (0.3, 0.6);

/// The operating point a bias setting (−1…1) gives in silence.
#[inline]
pub fn bias_point(bias: f32) -> f32 {
    bias.clamp(-1.0, 1.0) * BIAS_RANGE
}

/// The operating point at bias `bias` (−1…1) with `env` the envelope of what goes into the curve:
/// [`bias_point`], then for a model that [`tracks`](Model::tracks) a share of the envelope, and
/// Tube's drift.
#[inline]
pub fn operating_point(model: Model, bias: f32, env: f32) -> f32 {
    let bias = bias.clamp(-1.0, 1.0);
    let mut p = bias * BIAS_RANGE;
    if model.tracks() {
        p += bias * BIAS_TRACK * env;
    }
    if model == Model::Tube {
        p -= TUBE_DRIFT * env;
    }
    p
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

/// A static model's `F` for the driven sample `u0` at bias `bias`, following the envelope `env`.
#[inline]
fn static_lane(model: Model, env: &mut f32, u0: f32, bias: f32, k: (f32, f32)) -> f32 {
    if model.tracks() {
        follow(env, u0, k);
    }
    let pt = operating_point(model, bias, *env);
    (curve(model, u0 + pt) - curve(model, pt)) / CALIBRATION
}

/// The band's static transfer at input `x`: `x + a·(F(x) − x)` with `a` = [`engagement`] (before
/// tone, mix and gains). `point` is the operating point ([`operating_point`]).
#[inline]
pub fn transfer(model: Model, x: f32, drive_db: f32, point: f32) -> f32 {
    let g = crate::db_to_gain(drive_db);
    let a = engagement(drive_db);
    x + a * ((curve(model, CALIBRATION * g * x + point) - curve(model, point)) / CALIBRATION - x)
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

fn gain_tables() -> &'static [GainTable; 8] {
    static TABLES: OnceLock<Box<[GainTable; 8]>> = OnceLock::new();
    TABLES.get_or_init(|| {
        Box::new(Model::ALL.map(|m| {
            let mut t = GainTable { p: [0.0; TABLE_U * TABLE_B], c: [0.0; TABLE_U * TABLE_B] };
            let flat = (Tilt::IDENTITY, Tilt::IDENTITY);
            for j in 0..TABLE_B {
                let b = -1.0 + j as f32 * (2.0 / (TABLE_B - 1) as f32);
                for i in 0..TABLE_U {
                    let u = 10f64.powf((TABLE_U_DB.0 + i as f64) / 20.0);
                    let (mut p, mut c) = (0.0f64, 0.0f64);
                    const N: usize = 128;
                    // A hysteresis core is run through a settling cycle first, then measured
                    // on its loop (without the emphasis: the trim measures the rest).
                    let mut magnet = Magnet::default();
                    let settle = if m.magnet().is_some() { N } else { 0 };
                    let pt = operating_point(m, b, u as f32);
                    for k in 0..settle + N {
                        let s = (std::f64::consts::TAU * (k as f64 + 0.5) / N as f64).sin();
                        let f = match m.magnet() {
                            Some(w) => f64::from(magnet.process(w, &flat, (u * s) as f32, b, HELD_PEAK)),
                            None => f64::from(curve(m, (u * s) as f32 + pt)) - f64::from(curve(m, pt)),
                        };
                        if k < settle {
                            continue;
                        }
                        p += f * f;
                        c += s * f;
                    }
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
    let x = ((u_db - TABLE_U_DB.0).clamp(0.0, TABLE_U_DB.1 - TABLE_U_DB.0)).min((TABLE_U - 1) as f64);
    let y = ((f64::from(bias) + 1.0) / 2.0 * (TABLE_B - 1) as f64).clamp(0.0, (TABLE_B - 1) as f64);
    let (i, j) = ((x as usize).min(TABLE_U - 2), (y as usize).min(TABLE_B - 2));
    let (fx, fy) = (x - i as f64, y - j as f64);
    let t = &gain_tables()[model.index()];
    let at = |v: &[f32]| {
        let q = |jj: usize, ii: usize| f64::from(v[jj * TABLE_U + ii]);
        (q(j, i) * (1.0 - fx) + q(j, i + 1) * fx) * (1.0 - fy) + (q(j + 1, i) * (1.0 - fx) + q(j + 1, i + 1) * fx) * fy
    };
    let (p, c) = (at(&t.p), at(&t.c));
    let a = f64::from(a);
    let g2 = (1.0 - a) * (1.0 - a) + 2.0 * a * (1.0 - a) * g * c + a * a * g * g * p;
    g2.max(0.0).sqrt() as f32
}

// ---- Settings ------------------------------------------------------------------------------

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
}

impl StereoMode {
    pub const ALL: [StereoMode; 4] = [StereoMode::Stereo, StereoMode::Mid, StereoMode::Side, StereoMode::MidSide];

    pub fn from_index(i: usize) -> Self {
        Self::ALL[i.min(3)]
    }

    /// Which of the two signals (L/R, or mid/side) go through the curve.
    fn driven(self) -> [bool; 2] {
        match self {
            StereoMode::Stereo | StereoMode::MidSide => [true, true],
            StereoMode::Mid => [true, false],
            StereoMode::Side => [false, true],
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
    /// Clean but heard.
    pub bypass: bool,
    pub mute: bool,
    pub solo: bool,
}

impl Default for BandSat {
    fn default() -> Self {
        Self { model: Model::DEFAULT, drive_db: 0.0, bias: 0.0, stereo: StereoMode::Stereo, tone_db: 0.0, mix: 1.0, out_db: 0.0, bypass: false, mute: false, solo: false }
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
        Self { crossovers: [None; CROSSOVERS], bands: [BandSat::default(); BANDS], auto_gain: true }
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
    layout.region(band, crossovers).map_or(TONE_PIVOT, |(lo, hi)| (lo * hi).sqrt())
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
    pub const IDENTITY: Tilt = Tilt { b0: 1.0, b1: 0.0, a1: 0.0 };

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
        Self { b0: ((hi * k + lo * wp) / d) as f32, b1: ((lo * wp - hi * k) / d) as f32, a1: ((wp - k) / d) as f32 }
    }

    /// The same section times `gain`.
    pub fn scaled(self, gain: f32) -> Self {
        Self { b0: self.b0 * gain, b1: self.b1 * gain, ..self }
    }

    /// The exact inverse (the section is minimum phase: its zero is inside the unit circle).
    pub fn inverse(self) -> Self {
        Self { b0: 1.0 / self.b0, b1: self.a1 / self.b0, a1: self.b1 / self.b0 }
    }

    /// The magnitude at `f` Hz (for the editor and tests).
    pub fn magnitude(&self, f: f64, fs: f64) -> f64 {
        let w = std::f64::consts::TAU * f / fs;
        let (c, s) = (w.cos(), w.sin());
        let num = (f64::from(self.b0) + f64::from(self.b1) * c, -f64::from(self.b1) * s);
        let den = (1.0 + f64::from(self.a1) * c, -f64::from(self.a1) * s);
        (num.0.hypot(num.1)) / den.0.hypot(den.1)
    }

    /// `z` (re, im) through the section at `f` Hz: gain and phase.
    fn apply(&self, f: f64, fs: f64, z: (f64, f64)) -> (f64, f64) {
        let w = std::f64::consts::TAU * f / fs;
        let (c, s) = (w.cos(), w.sin());
        let (nr, ni) = (f64::from(self.b0) + f64::from(self.b1) * c, -f64::from(self.b1) * s);
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
        Self { core: Ja::default(), pre: 0.0, post: 0.0, rest: 0.0, rest_at: 0.0, rest_slope: 0.0, env: 0.0, unity: 1.0, fresh: true }
    }
}

impl Magnet {
    /// A fresh core whose envelope starts at `env` and emphasis filter at `pre` (where a steady
    /// tone holds them).
    pub fn seeded(env: f32, pre: f32) -> Self {
        Self { env, pre, ..Self::default() }
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
        if w == 0 { Model::Tape } else { Model::Transformer }
    }

    /// The operating point now, from bias `bias` and the envelope.
    pub fn point(&self, w: usize, bias: f32) -> f32 {
        operating_point(Self::model(w), bias, self.env)
    }

    fn set_rest(&mut self, p: &JaParams, point: f32) {
        let d = 1e-3;
        self.rest = anhysteretic(p, p.h_in * point);
        self.rest_slope = (anhysteretic(p, p.h_in * (point + d)) - anhysteretic(p, p.h_in * (point - d))) / (2.0 * d);
        self.rest_at = point;
    }

    /// `F` for the driven sample `u0` (`g·x`) at bias `bias` (−1…1), through core `w` with its
    /// emphasis `em`, the envelope following with coefficients `k`: slope 1 for quiet signals, 0
    /// at rest.
    #[inline]
    pub fn process(&mut self, w: usize, em: &(Tilt, Tilt), u0: f32, bias: f32, k: (f32, f32)) -> f32 {
        let p = &MAGNETS[w];
        if self.fresh {
            let point = self.point(w, bias);
            self.core = Ja::rest(p, p.h_in * point);
            (self.post, self.fresh) = (0.0, false);
            self.unity = hysteresis::unity(p);
            self.set_rest(p, point);
        }
        let v = em.0.process(&mut self.pre, u0);
        follow(&mut self.env, v, k);
        let point = self.point(w, bias);
        if (point - self.rest_at).abs() > REST_STEP * (1.0 + point.abs()) {
            self.set_rest(p, point);
        }
        let rest = self.rest + self.rest_slope * (point - self.rest_at);
        let m = self.core.step(p, p.h_in * (v + point));
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
fn respond(model: Model, x: &[f32], drive_db: f32, bias: f32, fs: f32, steady: Option<(f32, usize)>) -> Vec<f32> {
    let Some(w) = model.magnet() else { return x.iter().map(|&v| curve(model, v)).collect() };
    let em = emphasis(w, fs);
    let g = CALIBRATION * crate::db_to_gain(drive_db);
    let k = env_coefficients(f64::from(fs));
    let mut magnet = steady.map_or_else(Magnet::default, |(amp, n)| Magnet::steady(&em, g * amp, n, fs, k));
    x.iter().map(|&v| magnet.process(w, &em, g * v, bias, k) / CALIBRATION).collect()
}

/// The loop a Tape or Transformer band traces for a sine of amplitude `amp` at `freq` Hz once
/// settled (after one cycle from rest): `(x, F(x))` over the next, `n` points from phase 0 (the
/// editor draws it).
pub fn magnet_loop(model: Model, amp: f32, drive_db: f32, bias: f32, freq: f32, n: usize) -> Vec<(f32, f32)> {
    // The core is rate independent and the emphasis only sees `freq / fs`: any rate with `n`
    // samples a cycle gives the same loop. The emphasis and envelope start where the tone holds them.
    let fs = freq * n as f32;
    let x: Vec<f32> = (0..2 * n).map(|i| amp * (std::f64::consts::TAU * i as f64 / n as f64).sin() as f32).collect();
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
    let mut out = Harmonics { amp: [0.0; PARTIALS], hollow: std::array::from_fn(|i| (i + 1) as f32 * p.freq >= p.fs / 2.0) };
    let n = match p.model.magnet() {
        Some(_) => ((PROBE_MAGNET_FS / p.freq.max(1.0)).ceil() as usize).clamp(PROBE_MAGNET.0, PROBE_MAGNET.1),
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
                *y = magnet.process(w, &em, g * sine(i), p.bias, k) / CALIBRATION;
            }
            let swing = cycle.iter().fold(0.0f32, |m, v| m.max(v.abs()));
            let moved = cycle.iter().zip(&last).fold(0.0f32, |m, (a, b)| m.max((a - b).abs()));
            std::mem::swap(&mut cycle, &mut last);
            if c > 0 && moved <= PROBE_SETTLED * swing {
                break;
            }
        }
        last
    } else {
        let u: Vec<f32> = (0..n).map(|i| g * sine(i)).collect();
        // The envelope only matters where it moves the point.
        let moves = p.model.tracks() && (p.bias != 0.0 || p.model == Model::Tube);
        let mut env = if moves { settled_env(&u, k) } else { 0.0 };
        u.iter().map(|&v| static_lane(p.model, &mut env, v, p.bias, k)).collect()
    };
    let tilt = Tilt::new(p.tone_db, p.pivot, p.fs);
    let (fs, freq) = (f64::from(p.fs), f64::from(p.freq));
    for (i, a) in out.amp.iter_mut().enumerate() {
        let h = i + 1;
        let w = std::f64::consts::TAU * h as f64 / n as f64;
        let (mut re, mut im) = wet.iter().enumerate().fold((0.0f64, 0.0f64), |(re, im), (k, &v)| {
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
        (re, im) = ((re * hr - im * hi) * share, (re * hi + im * hr) * share - dry);
        *a = re.hypot(im) as f32;
    }
    out
}

// ---- Engine --------------------------------------------------------------------------------

/// One band slot's state in one core, per channel.
struct BandCore {
    os: Oversampler,
    /// The static models' envelope (for the operating point) per lane and channel.
    env: [[f32; 2]; 2],
    /// Tape's or Transformer's core per lane and channel.
    magnet: [[Magnet; 2]; 2],
    tone: [f32; 2],
    /// The difference high-pass: last input and output per channel.
    hp: [(f32, f32); 2],
    /// The band's level (mean square, both channels), for auto gain.
    level: f64,
    /// The same through Tape's and Transformer's emphasis (what their cores are driven with), and
    /// those filters' states per channel.
    level_emph: [f64; 2],
    emph_state: [[f32; 2]; 2],
    /// Auto gain's trim meters: mean squares in and out (after the prediction).
    trim_in: f64,
    trim_out: f64,
    /// Nothing runs: the oversampler and filters are clear.
    idle: bool,
}

impl BandCore {
    fn new(factor: usize) -> Self {
        Self { os: Oversampler::new(factor), env: [[0.0; 2]; 2], magnet: [[Magnet::default(); 2]; 2], tone: [0.0; 2], hp: [(0.0, 0.0); 2], level: 0.0, level_emph: [0.0; 2], emph_state: [[0.0; 2]; 2], trim_in: 0.0, trim_out: 0.0, idle: true }
    }

    fn clear(&mut self) {
        self.os.reset();
        self.env = [[0.0; 2]; 2];
        self.magnet = [[Magnet::default(); 2]; 2];
        self.tone = [0.0; 2];
        self.hp = [(0.0, 0.0); 2];
        self.idle = true;
    }

    fn copy_from(&mut self, other: &BandCore) {
        self.os.copy_from(&other.os);
        self.env = other.env;
        self.magnet = other.magnet;
        self.tone = other.tone;
        self.hp = other.hp;
        self.level = other.level;
        self.level_emph = other.level_emph;
        self.emph_state = other.emph_state;
        self.trim_in = other.trim_in;
        self.trim_out = other.trim_out;
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
            let (level, level_emph, trim_in, trim_out) = (self.bands[parent].level, self.bands[parent].level_emph, self.bands[parent].trim_in, self.bands[parent].trim_out);
            let band = &mut self.bands[slot];
            band.clear();
            (band.level, band.level_emph, band.trim_in, band.trim_out) = (level, level_emph, trim_in, trim_out);
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
}

impl Default for SatTelemetry {
    fn default() -> Self {
        Self { in_peak: [0.0; BANDS], out_peak: [0.0; BANDS], point: [0.0; BANDS], auto: [1.0; BANDS] }
    }
}

/// An S curve over 0…1 (the linear switch ramps are heard through it).
fn smooth(t: f64) -> f64 {
    t * t * (3.0 - 2.0 * t)
}

/// Which bands are heard: muted ones are not; with any soloed, only those.
fn audible(s: &SatSettings, layout: &Layout) -> [bool; BANDS] {
    let any_solo = layout.bands().iter().any(|&b| s.bands[b].solo && !s.bands[b].mute);
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
    trim_k: f64,
    env_k: (f32, f32),
    /// Tape's and Transformer's emphasis at the high rate, and (pre only) at the base rate for
    /// the levels auto gain reads.
    emph: [(Tilt, Tilt); 2],
    emph_base: [Tilt; 2],
    hp_k: f32,
    /// The values each control step ramps from and to, by band slot: gain, bias, share, post, fader.
    from: [[f32; 5]; BANDS],
    to: [[f32; 5]; BANDS],
    tilt_now: [Tilt; BANDS],
    /// The tone and pivot each band's tilt was designed for.
    tilt_key: [(f32, f32); BANDS],
    /// The band slots either playing core uses (only they are worked out).
    live: [bool; BANDS],
    lanes: Box<Lanes>,
    /// Scratch: the split per core, position and channel; the high rate; a band's wet.
    split: Vec<f32>,
    hi: Vec<f32>,
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
            trim_k: 0.0,
            env_k: (0.0, 0.0),
            emph: [(Tilt::IDENTITY, Tilt::IDENTITY); 2],
            emph_base: [Tilt::IDENTITY; 2],
            hp_k: 0.0,
            from: [[0.0; 5]; BANDS],
            to: [[0.0; 5]; BANDS],
            tilt_now: [Tilt::IDENTITY; BANDS],
            tilt_key: [(0.0, 0.0); BANDS],
            live: [false; BANDS],
            lanes: Box::new(Lanes {
                gain: [[0.0; CHUNK]; BANDS],
                bias: [[0.0; CHUNK]; BANDS],
                share: [[0.0; CHUNK]; BANDS],
                post: [[0.0; CHUNK]; BANDS],
                fader: [[0.0; CHUNK]; BANDS],
                blend: [[0.0; CHUNK]; BANDS],
                tilt: [[Tilt::IDENTITY; CHUNK]; BANDS],
            }),
            split: vec![0.0; 2 * BANDS * 2 * CHUNK],
            hi: vec![0.0; (PREROLL * 8).max(2 * HI_CH)],
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
        self.trim_k = per(TRIM_SECONDS * 1000.0);
        self.hp_k = (-std::f64::consts::TAU * HP_HZ / self.fs).exp() as f32;
        self.set_oversampling(factor, s);
    }

    /// Changes the oversampling (1, 2, 4 or 8) and clears the state: call it in silence (the
    /// engine's dip), as the latency changes with it.
    pub fn set_oversampling(&mut self, factor: usize, s: &SatSettings) {
        for core in &mut self.cores {
            for b in &mut core.bands {
                b.os.set_factor(factor);
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

    /// The round trip of the oversampling in force, samples (the latency).
    pub fn latency(&self) -> usize {
        Oversampler::latency_for(self.factor)
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

    fn modes_for(&self, s: &SatSettings) -> [StereoMode; BANDS] {
        std::array::from_fn(|b| if self.mono { StereoMode::Stereo } else { s.bands[b].stereo })
    }

    /// Band slot `b`'s gain into the curve, bias, share, post gain and fader for its eased settings.
    fn targets(&self, b: usize, auto_gain: bool) -> [f32; 5] {
        let e = &self.eased[b];
        let share = e.share();
        let auto = if auto_gain { e.comp * e.trim.powf(share) } else { 1.0 };
        let fader = e.out * smooth(e.audible);
        [CALIBRATION * crate::db_to_gain(e.drive_db as f32), e.bias as f32, share as f32, (auto * fader) as f32, fader as f32]
    }

    /// Once per control step: glides the settings, follows a split change, retunes auto gain.
    #[allow(clippy::needless_range_loop)] // band slots index several arrays
    fn control(&mut self, s: &SatSettings) {
        let layout = Layout::new(&s.crossovers);
        let modes = self.modes_for(s);
        if self.fade.is_none() && (layout != self.cores[self.active].layout || modes != self.cores[self.active].modes) {
            let next = 1 - self.active;
            let (a, b) = self.cores.split_at_mut(1);
            let (from, to) = if next == 1 { (&a[0], &mut b[0]) } else { (&b[0], &mut a[0]) };
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
                    self.log_freq[k] = if d.abs() < 1e-4 { target } else { self.log_freq[k] + self.glide * d };
                    self.cross[k] = Cross::new(self.log_freq[k].exp(), self.fs);
                }
            }
        }
        let heard = audible(s, &layout);
        // Only the bands a playing core uses are worked out; one coming into use starts at its
        // settings (it had no sound to glide from).
        let was_live = self.live;
        let other = self.fade.map(|_| self.cores[1 - self.active].layout);
        self.live = std::array::from_fn(|b| self.cores[self.active].layout.position(b).is_some() || other.is_some_and(|l| l.position(b).is_some()));
        for b in 0..BANDS {
            if self.live[b] && !was_live[b] {
                self.eased[b] = Eased::at(&s.bands[b], heard[b]);
            }
        }
        let ease = |v: &mut f64, target: f64, k: f64| {
            let d = target - *v;
            *v = if d.abs() < 1e-6 { target } else { *v + k * d };
        };
        let toward = |v: &mut f64, target: f64, step: f64| {
            *v = if target > *v { (*v + step).min(target) } else { (*v - step).max(target) };
        };
        let level = self.cores[self.active].bands.each_ref().map(|b| (b.level, b.level_emph));
        let trim_ms = self.cores[self.active].bands.each_ref().map(|b| (b.trim_in, b.trim_out));
        for b in 0..BANDS {
            if !self.live[b] {
                continue;
            }
            let t = &s.bands[b];
            let e = &mut self.eased[b];
            ease(&mut e.drive_db, f64::from(t.drive_db.clamp(0.0, DRIVE_MAX_DB)), self.glide);
            ease(&mut e.bias, f64::from(t.bias.clamp(-1.0, 1.0)), self.glide);
            ease(&mut e.tone_db, f64::from(t.tone_db.clamp(-TONE_MAX_DB, TONE_MAX_DB)), self.glide);
            ease(&mut e.mix, f64::from(t.mix.clamp(0.0, 1.0)), self.glide);
            ease(&mut e.out, f64::from(crate::db_to_gain(t.out_db)), self.glide);
            toward(&mut e.engage, if t.bypass { 0.0 } else { 1.0 }, self.switch_step);
            toward(&mut e.audible, if heard[b] { 1.0 } else { 0.0 }, self.switch_step);
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
            let predicted = f64::from(predicted_gain(model, amplitude, e.drive_db as f32, e.bias as f32, share as f32)).max(1e-6);
            let limit = 10f64.powf(AUTO_LIMIT_DB / 20.0);
            // Its inputs (level, eased settings) are smooth already: no lag of its own, so the
            // level holds while drive or bypass move.
            e.comp = (1.0 / predicted).clamp(1.0 / limit, limit);
            let (tin, tout) = trim_ms[b];
            if s.auto_gain && tin > TRIM_GATE && tout > TRIM_GATE {
                let lim = 10f64.powf(if model.magnet().is_some() { TRIM_LIMIT_MAGNET_DB } else { TRIM_LIMIT_DB } / 20.0);
                ease(&mut e.trim, (tin / tout).sqrt().clamp(1.0 / lim, lim), self.trim_k * CONTROL as f64);
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
            self.from[b] = if was_live[b] { self.to[b] } else { self.targets(b, s.auto_gain) };
            self.to[b] = self.targets(b, s.auto_gain);
            let key = (self.eased[b].tone_db as f32, tone_pivot(&layout, &s.crossovers, b));
            if key != self.tilt_key[b] {
                self.tilt_now[b] = Tilt::new(key.0, key.1, fs);
                self.tilt_key[b] = key;
            }
        }
    }

    /// Saturates one stretch in place (`channels`: one or two) and writes the dry band sum (the
    /// allpass the split makes, delayed like the result) into `dry`, for mix and delta.
    pub fn process(&mut self, channels: &mut [&mut [f32]], dry: &mut [&mut [f32]], s: &SatSettings) -> SatTelemetry {
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
            self.chunk(&mut ch[..n_ch], &mut dr, n, s, &mut telemetry);
            start += n;
        }
        telemetry
    }

    #[allow(clippy::needless_range_loop)] // bands, channels and samples indexed across arrays
    fn chunk(&mut self, channels: &mut [&mut [f32]], dry: &mut [&mut [f32]; 2], n: usize, s: &SatSettings, telemetry: &mut SatTelemetry) {
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
                let mut sig = [0.0f64; BANDS];
                for c in 0..n_ch {
                    core.split.process(c, f64::from(x[c]), &layout, &self.cross, &mut sig);
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
                let ms = mode != StereoMode::Stereo;
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
                            let ve = f64::from(self.emph_base[w].process(&mut band.emph_state[w][c], v));
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
                            *v = if ms { 0.5 * (ring(0) + if c == 0 { ring(1) } else { -ring(1) }) } else { ring(c) };
                        }
                        band.os.up(c, &tmp[..past], &mut self.hi[..past * f]);
                        let (g, bias, model) = (self.lanes.gain[b][0], self.lanes.bias[b][0], e.lanes[e.lane]);
                        // The core and the envelope warm up on the past too.
                        for v in &mut self.hi[..past * f] {
                            *v = match model.magnet() {
                                Some(w) => band.magnet[e.lane][c].process(w, &self.emph[w], g * *v, bias, self.env_k) / CALIBRATION,
                                None => static_lane(model, &mut band.env[e.lane][c], g * *v, bias, self.env_k),
                            };
                        }
                        let mut sink = [0.0f32; PREROLL];
                        band.os.down(c, &self.hi[..past * f], &mut sink[..past]);
                    }
                }
                if engaged {
                    for c in (0..n_ch).filter(|&c| driven[c]) {
                        band.os.up(c, &input[c][..n], &mut self.hi[c * HI_CH..c * HI_CH + m]);
                    }
                    // B: the curves, both signals in the same pass (their state chains are
                    // independent, so the CPU overlaps them: a hysteresis core is latency bound).
                    let models = e.lanes;
                    let heard = e.lane;
                    let env_k = self.env_k;
                    let emph = &self.emph;
                    let mut lane = |l: usize, c: usize, u0: f32, bias: f32| {
                        let model = models[l];
                        match model.magnet() {
                            Some(w) => band.magnet[l][c].process(w, &emph[w], u0, bias, env_k) / CALIBRATION,
                            None => static_lane(model, &mut band.env[l][c], u0, bias, env_k),
                        }
                    };
                    for k in 0..m {
                        let i = k / f;
                        let (g, bias) = (self.lanes.gain[b][i], self.lanes.bias[b][i]);
                        for c in 0..n_ch {
                            if !driven[c] {
                                continue;
                            }
                            let at = c * HI_CH + k;
                            let u0 = g * self.hi[at];
                            self.hi[at] = if lane_fading {
                                let (a, z) = (lane(heard, c, u0, bias), lane(1 - heard, c, u0, bias));
                                a + (z - a) * self.lanes.blend[b][i]
                            } else {
                                lane(heard, c, u0, bias)
                            };
                        }
                    }
                    band.idle = false;
                }
                // C: back down, the tone and the difference each signal now carries.
                let mut diff = [[0.0f32; CHUNK]; 2];
                for c in (0..n_ch).filter(|&c| driven[c]) {
                    if engaged {
                        band.os.down(c, &self.hi[c * HI_CH..c * HI_CH + m], &mut self.wet[..n]);
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
                // The sum. Left and right start from what they were and only take on the
                // differences, so a neutral band stays the input bit for bit in every mode.
                for i in 0..n {
                    let post = self.lanes.post[b][i];
                    if ms {
                        let fader = self.lanes.fader[b][i];
                        let part = |c: usize| if driven[c] { xde[c][i] * (post - fader) + post * diff[c][i] } else { 0.0 };
                        let (pm, ps) = (part(0), part(1));
                        let (l, r) = (xds[0][i] * fader + (pm + ps), xds[1][i] * fader + (pm - ps));
                        out[ci][0][i] += l;
                        out[ci][1][i] += r;
                        peak_out = peak_out.max(l.abs()).max(r.abs());
                    } else {
                        for c in 0..n_ch {
                            let y = (xds[c][i] + diff[c][i]) * post;
                            out[ci][c][i] += y;
                            peak_out = peak_out.max(y.abs());
                        }
                    }
                    for c in 0..n_ch {
                        dry_out[ci][c][i] += xds[c][i];
                    }
                }
                let comp2 = e.comp * e.comp;
                for i in 0..n {
                    band.level += self.level_k * (power[i] - band.level);
                    for w in 0..2 {
                        band.level_emph[w] += self.level_k * (power_emph[w][i] - band.level_emph[w]);
                    }
                    band.trim_in += self.trim_k * (power[i] - band.trim_in);
                    band.trim_out += self.trim_k * (power_out[i] * comp2 - band.trim_out);
                }
                if ci == shown {
                    telemetry.in_peak[b] = telemetry.in_peak[b].max(peak_in);
                    telemetry.out_peak[b] = telemetry.out_peak[b].max(peak_out);
                    let shown_ch = usize::from(!driven[0]);
                    let (model, bias) = (e.lanes[e.lane], e.bias as f32);
                    telemetry.point[b] = match model.magnet() {
                        Some(w) => band.magnet[e.lane][shown_ch].point(w, bias),
                        None => operating_point(model, bias, band.env[e.lane][shown_ch]),
                    };
                    telemetry.auto[b] = if s.auto_gain { (e.comp * e.trim.powf(e.share())) as f32 } else { 1.0 };
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
                    (out[a][c][i] * (1.0 - u) + out[z][c][i] * u, dry_out[a][c][i] * (1.0 - u) + dry_out[z][c][i] * u)
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
        (0..n).map(|i| amp * (std::f64::consts::TAU * (k * i) as f64 / N as f64).sin() as f32).collect()
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

    fn run(sat: &mut Saturator, input: &[f32], s: &SatSettings, block: usize) -> (Vec<f32>, Vec<f32>) {
        let mut y = input.to_vec();
        let mut d = vec![0.0; input.len()];
        for (a, b) in y.chunks_mut(block).zip(d.chunks_mut(block)) {
            sat.process(&mut [a], &mut [b], s);
        }
        (y, d)
    }

    fn band(model: Model, drive_db: f32) -> SatSettings {
        let mut s = SatSettings { auto_gain: false, ..SatSettings::default() };
        s.bands[0].model = model;
        s.bands[0].drive_db = drive_db;
        s
    }

    /// The magnitude of harmonic `h` (bin `h·K`) over the last `N` samples, linear amplitude.
    fn harmonic(y: &[f32], h: usize) -> f64 {
        let tail = &y[y.len() - N..];
        let w = std::f64::consts::TAU * (h * K) as f64 / N as f64;
        let (re, im) = tail.iter().enumerate().fold((0.0, 0.0), |(re, im), (i, &v)| (re + f64::from(v) * (w * i as f64).cos(), im + f64::from(v) * (w * i as f64).sin()));
        2.0 * re.hypot(im) / N as f64
    }

    /// The same harmonic of the static curve itself (`F(A·sin)`), worked out on a fine grid.
    fn curve_harmonic(model: Model, amp: f32, drive_db: f32, point: f32, h: usize) -> f64 {
        let m = 8192;
        let (re, im) = (0..m).fold((0.0, 0.0), |(re, im), i| {
            let th = std::f64::consts::TAU * i as f64 / m as f64;
            let v = f64::from(transfer(model, amp * th.sin() as f32, drive_db, point));
            (re + v * (h as f64 * th).cos(), im + v * (h as f64 * th).sin())
        });
        2.0 * re.hypot(im) / m as f64
    }

    #[test]
    fn every_curve_is_zero_at_zero_smooth_and_has_slope_one() {
        for m in Model::ALL {
            assert_eq!(curve(m, 0.0), 0.0, "{m:?}");
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
                    assert!((d - prev).abs() < 0.02, "{m:?}: slope jumps at {u}: {prev} → {d}");
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
            for (amp, drive, bias, tilt, mix) in [(0.25f32, 12.0f32, 0.0f32, 0.0f32, 1.0f32), (0.1, 24.0, 0.4, 6.0, 1.0), (0.3, 9.0, -0.3, -8.0, 0.6), (0.2, 3.0, 0.0, 0.0, 1.0)] {
                let mut s = band(m, drive);
                s.bands[0].bias = bias;
                s.bands[0].tone_db = tilt;
                s.bands[0].mix = mix;
                let mut sat = Saturator::new(FS, 8, &s);
                let (y, _) = run(&mut sat, &tone(32 * N, amp, K), &s, 333);
                let got = probe_harmonics(&Probe { model: m, amp, drive_db: drive, bias, freq, tone_db: tilt, pivot: TONE_PIVOT, mix, fs: FS });
                let want: [f64; PARTIALS] = std::array::from_fn(|i| harmonic(&y, i + 1));
                let top = want.iter().copied().fold(0.0, f64::max);
                for (i, &w) in want.iter().enumerate() {
                    if w < top * 1e-3 {
                        continue;
                    }
                    let db = 20.0 * (f64::from(got.amp[i]) / w).log10();
                    assert!(db.abs() < 0.25, "{m:?} amp {amp} drive {drive} bias {bias} tone {tilt} mix {mix}: H{} {db:.3} dB off", i + 1);
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
                    let got = probe_harmonics(&Probe { model: m, amp, drive_db: drive, bias, freq, tone_db: 0.0, pivot: TONE_PIVOT, mix: 1.0, fs: FS });
                    let tail = &y[y.len() - N..];
                    let read = |h: usize| {
                        let w = std::f64::consts::TAU * (h * k) as f64 / N as f64;
                        let (re, im) = tail.iter().enumerate().fold((0.0, 0.0), |(re, im), (i, &v)| (re + f64::from(v) * (w * i as f64).cos(), im + f64::from(v) * (w * i as f64).sin()));
                        2.0 * f64::hypot(re, im) / N as f64
                    };
                    let top = (1..=PARTIALS).filter(|&h| !got.hollow[h - 1]).map(read).fold(0.0, f64::max);
                    for h in 1..=PARTIALS {
                        // Above 0.9 × Nyquist the way down is rolling off (the probe reads the curve).
                        if h as f32 * freq > 0.9 * FS / 2.0 || read(h) < top * 1e-2 {
                            continue;
                        }
                        let db = 20.0 * (f64::from(got.amp[h - 1]) / read(h)).log10();
                        assert!(db.abs() < 1.0, "{m:?} bin {k} amp {amp} drive {drive} bias {bias}: H{h} {db:.3} dB off");
                    }
                }
            }
        }
    }

    #[test]
    #[ignore = "prints what a probe costs"]
    fn probe_cost() {
        for m in Model::ALL {
            for (freq, bias) in [(49.0f32, 0.0f32), (800.0, 0.0), (1043.0, 0.8), (9000.0, 0.8)] {
                let p = Probe { model: m, amp: 0.3, drive_db: 24.0, bias, freq, tone_db: 0.0, pivot: TONE_PIVOT, mix: 1.0, fs: FS };
                let t = std::time::Instant::now();
                for _ in 0..20 {
                    std::hint::black_box(probe_harmonics(&p));
                }
                println!("{m:?} {freq} Hz bias {bias}: {:.0} µs", t.elapsed().as_secs_f64() * 1e6 / 20.0);
            }
        }
    }

    /// Mid and side over the last `N` samples of a stereo run through `s`, and the delayed input's.
    fn run_stereo(s: &SatSettings, l: &[f32], r: &[f32]) -> (Vec<f32>, Vec<f32>, Vec<f32>, Vec<f32>) {
        let mut sat = Saturator::new(FS, 2, s);
        let (mut a, mut b) = (l.to_vec(), r.to_vec());
        let (mut da, mut db) = (vec![0.0; l.len()], vec![0.0; l.len()]);
        for ((x, y), (u, v)) in a.chunks_mut(333).zip(b.chunks_mut(333)).zip(da.chunks_mut(333).zip(db.chunks_mut(333))) {
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
    fn mid_leaves_the_side_alone_and_side_the_mid() {
        for auto_gain in [false, true] {
            let (l, r) = stereo_input();
            let part = |x: &[f32], y: &[f32], side: bool| -> Vec<f32> { x.iter().zip(y).map(|(a, b)| if side { 0.5 * (a - b) } else { 0.5 * (a + b) }).collect() };
            for (mode, clean_side) in [(StereoMode::Mid, true), (StereoMode::Side, false)] {
                let mut s = band(Model::Tube, 18.0);
                s.auto_gain = auto_gain;
                s.bands[0].stereo = mode;
                let (a, b, da, db) = run_stereo(&s, &l, &r);
                let (got, want) = (part(&a, &b, clean_side), part(&da, &db, clean_side));
                let worst = got.iter().zip(&want).fold(0.0f32, |m, (x, y)| m.max((x - y).abs()));
                assert!(worst < 2e-6, "{mode:?}: the clean part moved by {worst}");
                let (got, want) = (part(&a, &b, !clean_side), part(&da, &db, !clean_side));
                let moved = got.iter().zip(&want).fold(0.0f32, |m, (x, y)| m.max((x - y).abs()));
                assert!(moved > 0.01, "{mode:?}: the driven part did not change: {moved}");
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
            let worst = y.iter().zip(&stereo).fold(0.0f32, |m, (a, b)| m.max((a - b).abs()));
            assert!(worst < 1e-5, "{name}: {worst}");
        }
        s.bands[0].stereo = StereoMode::Side;
        let (a, _, da, _) = run_stereo(&s, &x, &x);
        assert!(a.iter().zip(&da).all(|(p, q)| (p - q).abs() < 1e-6), "no side, nothing to saturate");
    }

    #[test]
    fn stereo_mode_auto_gain_holds_the_level() {
        // Mid and side at different levels: the part driven comes out as loud as it went in.
        let (m, sd) = (noise(72 * N, 3, 0.25), noise(72 * N, 5, 0.1));
        let l: Vec<f32> = m.iter().zip(&sd).map(|(a, b)| a + b).collect();
        let r: Vec<f32> = m.iter().zip(&sd).map(|(a, b)| a - b).collect();
        let rms = |x: &[f32]| (x.iter().map(|v| f64::from(*v).powi(2)).sum::<f64>() / x.len() as f64).sqrt();
        for (mode, side) in [(StereoMode::Mid, false), (StereoMode::Side, true)] {
            let mut s = band(Model::Tube, 18.0);
            s.auto_gain = true;
            s.bands[0].stereo = mode;
            let (a, b, da, db) = run_stereo(&s, &l, &r);
            let t = a.len() - 12 * N;
            let part = |x: &[f32], y: &[f32]| -> Vec<f32> { x[t..].iter().zip(&y[t..]).map(|(p, q)| if side { 0.5 * (p - q) } else { 0.5 * (p + q) }).collect() };
            let db_off = 20.0 * (rms(&part(&a, &b)) / rms(&part(&da, &db))).log10();
            assert!(db_off.abs() < 0.5, "{mode:?}: {db_off:.2} dB");
        }
    }

    #[test]
    fn a_stereo_mode_change_never_steps() {
        // A change every so often: no sample steps further than any mode's steady wave does.
        let (l, r) = (tone(12 * N, 0.05, 40), tone(12 * N, 0.04, 61));
        let mut s = band(Model::Tube, 12.0);
        let step = |x: &[f32]| x[8192..].windows(2).fold(0.0f32, |m, w| m.max((w[1] - w[0]).abs()));
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
        for (count, ((x, y), (u, v))) in a.chunks_mut(64).zip(b.chunks_mut(64)).zip(da.chunks_mut(64).zip(db.chunks_mut(64))).enumerate() {
            s.bands[0].stereo = StereoMode::ALL[(count / 40) % 4];
            sat.process(&mut [x, y], &mut [u, v], &s);
        }
        let worst = step(&a).max(step(&b));
        assert!(worst < 1.3 * steady, "largest step {worst}, steady {steady}");
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
    fn a_track_at_minus_18_reaches_the_knee_within_a_little_drive() {
        // The calibration: a −18 dBFS sine at 12 dB of drive has audible harmonics on the curves
        // a kick would be heard through; at drive 0 nothing has changed.
        let amp = 10f32.powf(-18.0 / 20.0);
        for m in [Model::Tube, Model::Soft, Model::Hard, Model::Diode] {
            let h = probe_harmonics(&Probe { model: m, amp, drive_db: 12.0, bias: 0.0, freq: 1000.0, tone_db: 0.0, pivot: TONE_PIVOT, mix: 1.0, fs: FS });
            assert!(h.db(2) > -30.0, "{m:?}: H3 {:.1} dB", h.db(2));
        }
    }

    #[test]
    fn the_probe_marks_what_the_way_down_removes() {
        // A 9 kHz tone at 48 kHz: H2 (18 kHz) is heard, H3 (27 kHz) is not, though the curve makes it.
        let p = Probe { model: Model::Soft, amp: 0.5, drive_db: 12.0, bias: 0.0, freq: 9000.0, tone_db: 0.0, pivot: TONE_PIVOT, mix: 1.0, fs: FS };
        let h = probe_harmonics(&p);
        assert_eq!(&h.hollow[..3], &[false, false, true]);
        assert!(h.amp[2] > 0.01, "the curve makes H3: {}", h.amp[2]);
    }

    #[test]
    fn a_clean_band_probes_clean() {
        // Drive 0 or mix 0 is the band itself: the tone and nothing else, at its own level.
        for (drive, mix) in [(0.0f32, 1.0f32), (18.0, 0.0)] {
            let h = probe_harmonics(&Probe { model: Model::Tube, amp: 0.5, drive_db: drive, bias: 0.0, freq: 1000.0, tone_db: 0.0, pivot: TONE_PIVOT, mix, fs: FS });
            assert!((h.amp[0] - 0.5).abs() < 1e-6, "H1 {}", h.amp[0]);
            assert!(h.amp[1..].iter().all(|&a| a == 0.0));
        }
        // Rectify trades the fundamental for the octave: H2 is the reference.
        let h = probe_harmonics(&Probe { model: Model::Rectify, amp: 0.5, drive_db: 14.0, bias: 0.0, freq: 1000.0, tone_db: 0.0, pivot: TONE_PIVOT, mix: 1.0, fs: FS });
        assert_eq!(h.db(1), 0.0);
    }

    #[test]
    fn soft_matches_its_closed_form_harmonics() {
        // f(u) = u − 4u³/27: a sine of amplitude U gives H1 = U − U³/9 and H3 = U³/27, and the
        // calibration comes off afterwards.
        let (amp, drive) = (0.1f32, 8.0f32);
        let k = f64::from(CALIBRATION);
        let u = f64::from(amp * crate::db_to_gain(drive)) * k;
        let s = band(Model::Soft, drive);
        let mut sat = Saturator::new(FS, 8, &s);
        let (y, _) = run(&mut sat, &tone(4 * N, amp, K), &s, 512);
        let (h1, h3) = (harmonic(&y, 1), harmonic(&y, 3));
        assert!((h1 * k / (u - u.powi(3) / 9.0) - 1.0).abs() < 1e-3, "H1 {h1}");
        assert!((h3 * k / (u.powi(3) / 27.0) - 1.0).abs() < 2e-3, "H3 {h3}");
        assert!(harmonic(&y, 2) < 1e-6 && harmonic(&y, 5) < 1e-6);
    }

    #[test]
    fn the_engine_plays_the_drawn_curve() {
        // Every static model, through the oversampled engine: its harmonics are those of the curve
        // the editor draws, within 0.05 dB down to −80 dB under the fundamental. With a bias, only
        // where the point stays put (elsewhere it follows the envelope: the probe test covers it).
        for m in [Model::Diode, Model::Soft, Model::Hard, Model::Fold, Model::Rectify] {
            for (amp, drive, bias) in [(0.0625f32, 12.0f32, 0.0f32), (0.025, 24.0, 0.4), (0.125, 3.0, -0.6)] {
                if bias != 0.0 && m.tracks() {
                    continue;
                }
                let mut s = band(m, drive);
                s.bands[0].bias = bias;
                let mut sat = Saturator::new(FS, 8, &s);
                let (y, _) = run(&mut sat, &tone(6 * N, amp, K), &s, 333);
                let point = bias_point(bias);
                let reference = (1..=5).map(|h| curve_harmonic(m, amp, drive, point, h)).fold(0.0, f64::max);
                for h in 1..=5 {
                    let want = curve_harmonic(m, amp, drive, point, h);
                    if want < reference * 1e-4 {
                        continue;
                    }
                    let got = harmonic(&y, h);
                    let db = 20.0 * (got / want).log10();
                    assert!(db.abs() < 0.05, "{m:?} amp {amp} drive {drive} bias {bias}: H{h} {db:.3} dB off");
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
        let (re, im) = tail.iter().enumerate().fold((0.0, 0.0), |(re, im), (i, &v)| (re + f64::from(v) * (w * i as f64).cos(), im + f64::from(v) * (w * i as f64).sin()));
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
                let x_hi: Vec<f32> = (0..6 * N * 8).map(|i| amp * (std::f64::consts::TAU * (K * i) as f64 / (8 * N) as f64).sin() as f32).collect();
                let want_y = magnet_response(m, &x_hi, drive, bias, 8.0 * FS);
                let reference = (1..=5).map(|h| harmonic_at(&want_y, h, 8)).fold(0.0, f64::max);
                for h in 1..=5 {
                    let want = harmonic_at(&want_y, h, 8);
                    if want < reference * 1e-3 {
                        continue;
                    }
                    let db = 20.0 * (harmonic(&y, h) / want).log10();
                    assert!(db.abs() < 0.1, "{m:?} amp {amp} drive {drive} bias {bias}: H{h} {db:.3} dB off");
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
            let x: Vec<f32> = (0..8 * N).map(|i| 0.05 * (std::f64::consts::TAU * (k * i) as f64 / N as f64).sin() as f32).collect();
            let (y, _) = run(&mut sat, &x, &s, 256);
            let at = |h: usize| {
                let tail = &y[y.len() - N..];
                let w = std::f64::consts::TAU * (h * k) as f64 / N as f64;
                let (re, im) = tail.iter().enumerate().fold((0.0, 0.0), |(re, im), (i, &v)| (re + f64::from(v) * (w * i as f64).cos(), im + f64::from(v) * (w * i as f64).sin()));
                re.hypot(im)
            };
            20.0 * (at(3) / at(1)).log10()
        };
        let (tape_lo, tape_hi) = (h3(Model::Tape, 6), h3(Model::Tape, 377));
        let (iron_lo, iron_hi) = (h3(Model::Transformer, 6), h3(Model::Transformer, 377));
        assert!(tape_hi > tape_lo + 6.0, "tape: low {tape_lo:.1} dB, high {tape_hi:.1} dB");
        assert!(iron_lo > iron_hi + 6.0, "transformer: low {iron_lo:.1} dB, high {iron_hi:.1} dB");
    }

    #[test]
    fn tape_and_transformer_pass_quiet_signals_at_unity() {
        for m in [Model::Tape, Model::Transformer] {
            let s = band(m, 12.0);
            let mut sat = Saturator::new(FS, 2, &s);
            let (y, _) = run(&mut sat, &tone(4 * N, 0.002, K), &s, 256);
            // Drive is gain into the curve (auto gain off): 12 dB louder, as the static curves.
            let db = 20.0 * (harmonic(&y, 1) / 0.002).log10() - 12.0;
            assert!(db.abs() < 1.0, "{m:?}: {db:.2} dB");
            assert!(harmonic(&y, 3) < harmonic(&y, 1) * 0.01, "{m:?}: H3");
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
                assert!(harmonic(&y, h) < h1 * 1e-4, "{m:?}: H{h} {}", harmonic(&y, h) / h1);
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
    fn even_harmonics_hold_however_hard_the_band_is_driven() {
        // The operating point follows the envelope: from 12 dB of drive to 36 the asymmetry, and so
        // H2, does not fade (a fixed offset would be lost under the signal, the halves clipping at
        // equal times). Tube and Diode without a bias, every saturating model with one.
        let mut cases: Vec<(Model, f32)> = vec![(Model::Tube, 0.0), (Model::Diode, 0.0)];
        cases.extend([Model::Tape, Model::Transformer, Model::Diode, Model::Soft, Model::Hard].map(|m| (m, 0.3)));
        cases.push((Model::Tube, -0.3));
        for (m, bias) in cases {
            let (mid, top) = (h2_db(m, 12.0, bias), h2_db(m, 36.0, bias));
            assert!(top > -26.0 && top > mid - 6.0, "{m:?} bias {bias}: H2 {mid:.1} dB at 12, {top:.1} dB at 36");
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
            let peak = (0..y.len()).max_by(|&a, &b| y[a].abs().total_cmp(&y[b].abs())).unwrap();
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
    fn assert_change_is_clean(x: &[f32], before: &SatSettings, after: &SatSettings, at: usize, factor: usize) {
        let render = |switch: Option<usize>, first: &SatSettings| {
            let mut sat = Saturator::new(FS, factor, first);
            let mut y = x.to_vec();
            let mut pos = 0;
            for chunk in y.chunks_mut(480) {
                let s = if switch.is_some_and(|k| pos >= k) { after } else { first };
                sat.process(&mut [chunk], &mut [], s);
                pos += 480;
            }
            y
        };
        let (y, held_a, held_b) = (render(Some(at), before), render(None, before), render(None, after));
        let step = |v: &[f32], i: usize| (v[i + 1] - v[i]).abs();
        // The held settings' steepest step within half a period of the tone either side (edges
        // move in time as a setting glides).
        let near = |v: &[f32], i: usize| (i.saturating_sub(512)..(i + 512).min(x.len() - 1)).map(|j| step(v, j)).fold(0.0, f32::max);
        for i in at..(at + 9600).min(x.len() - 1) {
            let allowed = near(&held_a, i).max(near(&held_b, i)) + 0.01;
            assert!(step(&y, i) <= allowed, "step {} at {} (held: {} / {}) {:?} → {:?}", step(&y, i), i - at, step(&held_a, i), step(&held_b, i), before.bands[0], after.bands[0]);
        }
    }

    #[test]
    fn moving_drive_bias_tone_and_switching_model_never_clicks() {
        let x = tone(48_000, 0.4, 4);
        let base = SatSettings { auto_gain: true, ..band(Model::Hard, 30.0) };
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
            (with(&|b| b.model = Model::Tube), with(&|b| b.model = Model::Rectify)),
            // Auto gain off: a trim measured on Tape settles onto Transformer over seconds.
            (SatSettings { auto_gain: false, ..with(&|b| b.model = Model::Tape) }, SatSettings { auto_gain: false, ..with(&|b| b.model = Model::Transformer) }),
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
        s.crossovers = [Some(120.0), Some(500.0), Some(2000.0), Some(6000.0), Some(12_000.0)];
        let mut sat = Saturator::new(FS, 4, &s);
        let mut impulse = vec![0.0f32; 16_384];
        impulse[0] = 1.0;
        let (y, d) = run(&mut sat, &impulse, &s, 512);
        assert_eq!(y, d);
        for f in [40.0, 120.0, 900.0, 6000.0, 15_000.0] {
            let w = std::f64::consts::TAU * f / f64::from(FS);
            let (re, im) = y.iter().enumerate().fold((0.0, 0.0), |(re, im), (i, &v)| (re + f64::from(v) * (w * i as f64).cos(), im - f64::from(v) * (w * i as f64).sin()));
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
        let rms = |v: &[f32]| (v.iter().map(|x| f64::from(*x).powi(2)).sum::<f64>() / v.len() as f64).sqrt();
        for m in [Model::Tape, Model::Tube, Model::Transformer, Model::Diode, Model::Soft, Model::Hard, Model::Fold] {
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
    fn the_tilt_pivots_on_its_frequency() {
        let t = Tilt::new(12.0, 800.0, FS);
        let db = |f: f64| 20.0 * t.magnitude(f, f64::from(FS)).log10();
        assert!(db(800.0).abs() < 0.01, "{}", db(800.0));
        assert!((db(20.0) + 6.0).abs() < 0.3 && (db(20_000.0) - 6.0).abs() < 0.6, "{} {}", db(20.0), db(20_000.0));
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
