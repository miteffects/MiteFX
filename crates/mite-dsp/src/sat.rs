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
/// Bias ±1 (±100 %) moves the operating point this far.
pub const BIAS_RANGE: f32 = 0.5;
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
const TRIM_LIMIT_DB: f64 = 6.0;
/// Tape and Transformer: their emphasis makes the prediction coarser (it depends on the band's
/// spectrum), so the trim reaches further.
const TRIM_LIMIT_MAGNET_DB: f64 = 12.0;
/// Auto gain never moves a band by more than this, dB.
const AUTO_LIMIT_DB: f64 = 24.0;
/// Below this mean square (−80 dBFS) the trim is not measured.
const TRIM_GATE: f64 = 1e-8;
/// Tube: the bias drifts down by this much per unit of rectified drive, following it with these
/// times (grid-current blocking).
const DRIFT: f32 = 0.25;
const DRIFT_ATTACK_MS: f64 = 10.0;
const DRIFT_RELEASE_MS: f64 = 150.0;
const DRIFT_MAX: f32 = 2.0;
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

    /// Whether the curve itself has state (drift or hysteresis), so the drawn curve moves.
    pub fn is_stateful(self) -> bool {
        matches!(self, Model::Tape | Model::Tube | Model::Transformer)
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
        Model::Diode => 0.3 * (u / 0.3).asinh(),
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

/// The share of the saturation heard at `drive_db` (0 at 0 dB, 1 from [`ENGAGE_DB`], S-shaped).
#[inline]
pub fn engagement(drive_db: f32) -> f32 {
    let t = (drive_db / ENGAGE_DB).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// The operating point a bias setting (−1…1) gives.
#[inline]
pub fn bias_point(bias: f32) -> f32 {
    bias.clamp(-1.0, 1.0) * BIAS_RANGE
}

/// The band's static transfer at input `x`: `x + a·(F(x) − x)` with `a` = [`engagement`] (before
/// tone, mix and gains). `point` is the operating point ([`bias_point`], or Tube's drifted one).
#[inline]
pub fn transfer(model: Model, x: f32, drive_db: f32, point: f32) -> f32 {
    let g = crate::db_to_gain(drive_db);
    let a = engagement(drive_db);
    x + a * (curve(model, g * x + point) - curve(model, point) - x)
}

// ---- Auto gain ----------------------------------------------------------------------------

/// Sine amplitudes into the curve the gain tables cover, dB (1 dB steps).
const TABLE_U_DB: (f64, f64) = (-60.0, 48.0);
const TABLE_U: usize = 109;
/// Operating points the tables cover (the bias range, 0.1 steps).
const TABLE_B: usize = 11;

/// For one model, a sine of amplitude `U` into the curve at operating point `b`:
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
                let b = -BIAS_RANGE + j as f32 * (2.0 * BIAS_RANGE / (TABLE_B - 1) as f32);
                for i in 0..TABLE_U {
                    let u = 10f64.powf((TABLE_U_DB.0 + i as f64) / 20.0);
                    let (mut p, mut c) = (0.0f64, 0.0f64);
                    const N: usize = 128;
                    // A hysteresis core is run through a settling cycle first, then measured
                    // on its loop (without the emphasis: the trim measures the rest).
                    let mut magnet = Magnet::default();
                    let settle = if m.magnet().is_some() { N } else { 0 };
                    for k in 0..settle + N {
                        let s = (std::f64::consts::TAU * (k as f64 + 0.5) / N as f64).sin();
                        let f = match m.magnet() {
                            Some(w) => f64::from(magnet.process(w, &flat, (u * s) as f32, b)),
                            None => f64::from(curve(m, (u * s) as f32 + b)) - f64::from(curve(m, b)),
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

/// The band path's gain (linear, RMS) for a sine of amplitude `amplitude` at `drive_db`,
/// operating point `point`, with the saturation at share `a`: the dry part, the curve and the
/// cross term between them.
pub fn predicted_gain(model: Model, amplitude: f32, drive_db: f32, point: f32, a: f32) -> f32 {
    let g = f64::from(crate::db_to_gain(drive_db));
    let u_db = 20.0 * (g * f64::from(amplitude).max(1e-9)).log10();
    let x = ((u_db - TABLE_U_DB.0).clamp(0.0, TABLE_U_DB.1 - TABLE_U_DB.0)).min((TABLE_U - 1) as f64);
    let y = ((f64::from(point) + f64::from(BIAS_RANGE)) / (2.0 * f64::from(BIAS_RANGE)) * (TABLE_B - 1) as f64).clamp(0.0, (TABLE_B - 1) as f64);
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

/// One band's settings.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BandSat {
    pub model: Model,
    pub drive_db: f32,
    /// −1…1.
    pub bias: f32,
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
        Self { model: Model::DEFAULT, drive_db: 0.0, bias: 0.0, tone_db: 0.0, mix: 1.0, out_db: 0.0, bypass: false, mute: false, solo: false }
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
    /// The core's rest magnetisation at the operating point it was worked out for.
    rest: f32,
    rest_at: f32,
    /// The core's output scale ([`hysteresis::unity`]).
    unity: f32,
    fresh: bool,
}

impl Default for Magnet {
    fn default() -> Self {
        Self { core: Ja::default(), pre: 0.0, post: 0.0, rest: 0.0, rest_at: 0.0, unity: 1.0, fresh: true }
    }
}

impl Magnet {
    /// `F` for the driven sample `u0` (`g·x`) at operating point `point`, through core `w` with
    /// its emphasis `em`: slope 1 for quiet signals, 0 at rest.
    #[inline]
    pub fn process(&mut self, w: usize, em: &(Tilt, Tilt), u0: f32, point: f32) -> f32 {
        let p = &MAGNETS[w];
        let at = p.h_in * point;
        if self.fresh {
            self.core = Ja::rest(p, at);
            (self.pre, self.post, self.rest, self.rest_at, self.fresh) = (0.0, 0.0, self.core.m, point, false);
            self.unity = hysteresis::unity(p);
        } else if point != self.rest_at {
            self.rest = anhysteretic(p, at);
            self.rest_at = point;
        }
        let v = em.0.process(&mut self.pre, u0);
        let m = self.core.step(p, p.h_in * v + at);
        em.1.process(&mut self.post, self.unity * (m - self.rest))
    }
}

/// What a Tape or Transformer band makes of `x` at rate `fs` (the rate the curve runs at),
/// from rest: `F` through the emphasis and the core, the code the engine runs per lane (without
/// the oversampler, the tone and the dry blend). For the editor and tests; allocates.
pub fn magnet_response(model: Model, x: &[f32], drive_db: f32, point: f32, fs: f32) -> Vec<f32> {
    let Some(w) = model.magnet() else { return x.iter().map(|&v| curve(model, v)).collect() };
    let em = emphasis(w, fs);
    let g = crate::db_to_gain(drive_db);
    let mut magnet = Magnet::default();
    x.iter().map(|&v| magnet.process(w, &em, g * v, point)).collect()
}

/// The loop a Tape or Transformer band traces for a sine of amplitude `amp` at `freq` Hz once
/// settled (after one cycle from rest): `(x, F(x))` over the next, `n` points from phase 0 (the
/// editor draws it).
pub fn magnet_loop(model: Model, amp: f32, drive_db: f32, point: f32, freq: f32, n: usize) -> Vec<(f32, f32)> {
    // The core is rate independent and the emphasis only sees `freq / fs`: any rate with `n`
    // samples a cycle gives the same loop.
    let fs = freq * n as f32;
    let x: Vec<f32> = (0..2 * n).map(|i| amp * (std::f64::consts::TAU * i as f64 / n as f64).sin() as f32).collect();
    let y = magnet_response(model, &x, drive_db, point, fs);
    x[n..].iter().copied().zip(y[n..].iter().copied()).collect()
}

// ---- Engine --------------------------------------------------------------------------------

/// One band slot's state in one core, per channel.
struct BandCore {
    os: Oversampler,
    /// Tube's drift follower per lane and channel.
    drift: [[f32; 2]; 2],
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
        Self { os: Oversampler::new(factor), drift: [[0.0; 2]; 2], magnet: [[Magnet::default(); 2]; 2], tone: [0.0; 2], hp: [(0.0, 0.0); 2], level: 0.0, level_emph: [0.0; 2], emph_state: [[0.0; 2]; 2], trim_in: 0.0, trim_out: 0.0, idle: true }
    }

    fn clear(&mut self) {
        self.os.reset();
        self.drift = [[0.0; 2]; 2];
        self.magnet = [[Magnet::default(); 2]; 2];
        self.tone = [0.0; 2];
        self.hp = [(0.0, 0.0); 2];
        self.idle = true;
    }

    fn copy_from(&mut self, other: &BandCore) {
        self.os.copy_from(&other.os);
        self.drift = other.drift;
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
    /// The operating point the heard lane ends the chunk at (Tube drifts), channel 1.
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
    point: [[f32; CHUNK]; BANDS],
    share: [[f32; CHUNK]; BANDS],
    post: [[f32; CHUNK]; BANDS],
    /// The model crossfade's position (0: the heard lane only).
    blend: [[f32; CHUNK]; BANDS],
    tilt: [[Tilt; CHUNK]; BANDS],
}

pub struct Saturator {
    fs: f64,
    factor: usize,
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
    drift_k: (f32, f32),
    /// Tape's and Transformer's emphasis at the high rate, and (pre only) at the base rate for
    /// the levels auto gain reads.
    emph: [(Tilt, Tilt); 2],
    emph_base: [Tilt; 2],
    hp_k: f32,
    /// The values each control step ramps from and to, by band slot: gain, point, share, post.
    from: [[f32; 4]; BANDS],
    to: [[f32; 4]; BANDS],
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
            drift_k: (0.0, 0.0),
            emph: [(Tilt::IDENTITY, Tilt::IDENTITY); 2],
            emph_base: [Tilt::IDENTITY; 2],
            hp_k: 0.0,
            from: [[0.0; 4]; BANDS],
            to: [[0.0; 4]; BANDS],
            tilt_now: [Tilt::IDENTITY; BANDS],
            tilt_key: [(0.0, 0.0); BANDS],
            live: [false; BANDS],
            lanes: Box::new(Lanes {
                gain: [[0.0; CHUNK]; BANDS],
                point: [[0.0; CHUNK]; BANDS],
                share: [[0.0; CHUNK]; BANDS],
                post: [[0.0; CHUNK]; BANDS],
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
        let per = |ms: f64| (1.0 - (-1000.0 / (ms * hi_rate)).exp()) as f32;
        self.drift_k = (per(DRIFT_ATTACK_MS), per(DRIFT_RELEASE_MS));
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

    /// Band slot `b`'s gain, operating point, share and post gain for its eased settings.
    fn targets(&self, b: usize, auto_gain: bool) -> [f32; 4] {
        let e = &self.eased[b];
        let share = e.share();
        let auto = if auto_gain { e.comp * e.trim.powf(share) } else { 1.0 };
        [crate::db_to_gain(e.drive_db as f32), bias_point(e.bias as f32), share as f32, (auto * e.out * smooth(e.audible)) as f32]
    }

    /// Once per control step: glides the settings, follows a split change, retunes auto gain.
    #[allow(clippy::needless_range_loop)] // band slots index several arrays
    fn control(&mut self, s: &SatSettings) {
        let layout = Layout::new(&s.crossovers);
        if self.fade.is_none() && layout != self.cores[self.active].layout {
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
                    core.bands[b].drift[other] = [0.0; 2];
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
            let predicted = f64::from(predicted_gain(model, amplitude, e.drive_db as f32, bias_point(e.bias as f32), share as f32)).max(1e-6);
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
                self.lanes.point[b][i] = r(1);
                self.lanes.share[b][i] = r(2);
                self.lanes.post[b][i] = r(3);
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
                // Mean square across the channels, going in and coming out (before the gains).
                let mut power = [0.0f64; CHUNK];
                let mut power_emph = [[0.0f64; CHUNK]; 2];
                let mut power_out = [0.0f64; CHUNK];
                let per_ch = 1.0 / n_ch as f64;
                // A: each channel's delayed band (x_d) and meters, the wake-up pre-roll, and the
                // stretch up to the high rate (channel c at `hi[c·HI_CH..]`).
                let mut xds = [[0.0f32; CHUNK]; 2];
                let f = self.factor;
                let m = n * f;
                for c in 0..n_ch {
                    let src = at(ci, p, c);
                    let r = (b * 2 + c) * core.ring_len;
                    let xd = &mut xds[c];
                    let mut w = self.write;
                    for i in 0..n {
                        let v = self.split[src + i];
                        if latency == 0 {
                            xd[i] = v;
                        } else {
                            core.rings[r + w] = v;
                            xd[i] = core.rings[r + (w + core.ring_len - latency) % core.ring_len];
                        }
                        w = (w + 1) % core.ring_len;
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
                            *v = core.rings[r + (self.write + core.ring_len - past + j) % core.ring_len];
                        }
                        band.os.up(c, &tmp[..past], &mut self.hi[..past * f]);
                        let (g, point, model) = (self.lanes.gain[b][0], self.lanes.point[b][0], e.lanes[e.lane]);
                        for v in &mut self.hi[..past * f] {
                            *v = match model.magnet() {
                                // The core warms up on the past too.
                                Some(w) => band.magnet[e.lane][c].process(w, &self.emph[w], g * *v, point),
                                None => curve(model, g * *v + point) - curve(model, point),
                            };
                        }
                        let mut sink = [0.0f32; PREROLL];
                        band.os.down(c, &self.hi[..past * f], &mut sink[..past]);
                    }
                }
                if engaged {
                    for c in 0..n_ch {
                        let src = at(ci, p, c);
                        band.os.up(c, &self.split[src..src + n], &mut self.hi[c * HI_CH..c * HI_CH + m]);
                    }
                    // B: the curves, both channels in the same pass (their state chains are
                    // independent, so the CPU overlaps them: a hysteresis core is latency bound).
                    let models = e.lanes;
                    let heard = e.lane;
                    let (ka, kr) = self.drift_k;
                    let emph = &self.emph;
                    let mut lane = |l: usize, c: usize, u0: f32, point: f32| {
                        let model = models[l];
                        if let Some(w) = model.magnet() {
                            return band.magnet[l][c].process(w, &emph[w], u0, point);
                        }
                        let pt = if model == Model::Tube {
                            let d = &mut band.drift[l][c];
                            let target = u0.clamp(0.0, DRIFT_MAX);
                            *d += (target - *d) * if target > *d { ka } else { kr };
                            point - DRIFT * *d
                        } else {
                            point
                        };
                        curve(model, u0 + pt) - curve(model, pt)
                    };
                    for k in 0..m {
                        let i = k / f;
                        let (g, point) = (self.lanes.gain[b][i], self.lanes.point[b][i]);
                        for c in 0..n_ch {
                            let at = c * HI_CH + k;
                            let u0 = g * self.hi[at];
                            self.hi[at] = if lane_fading {
                                let (a, z) = (lane(heard, c, u0, point), lane(1 - heard, c, u0, point));
                                a + (z - a) * self.lanes.blend[b][i]
                            } else {
                                lane(heard, c, u0, point)
                            };
                        }
                    }
                    band.idle = false;
                }
                // C: back down, the tone and the dry blend, the sum.
                for c in 0..n_ch {
                    let xd = &xds[c];
                    if engaged {
                        band.os.down(c, &self.hi[c * HI_CH..c * HI_CH + m], &mut self.wet[..n]);
                        let (mut x1, mut y1) = band.hp[c];
                        let mut tone = band.tone[c];
                        for i in 0..n {
                            let wet = self.lanes.tilt[b][i].process(&mut tone, self.wet[i]);
                            let d = wet - xd[i];
                            let y = self.hp_k * (y1 + d - x1);
                            x1 = d;
                            y1 = y;
                            self.wet[i] = xd[i] + self.lanes.share[b][i] * y;
                        }
                        band.hp[c] = (x1, y1);
                        band.tone[c] = tone;
                    } else {
                        self.wet[..n].copy_from_slice(&xd[..n]);
                    }
                    for i in 0..n {
                        let pre = self.wet[i];
                        power_out[i] += f64::from(pre) * f64::from(pre) * per_ch;
                        let y = pre * self.lanes.post[b][i];
                        out[ci][c][i] += y;
                        dry_out[ci][c][i] += xd[i];
                        peak_out = peak_out.max(y.abs());
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
                    let d = if e.lanes[e.lane] == Model::Tube { DRIFT * band.drift[e.lane][0] } else { 0.0 };
                    telemetry.point[b] = bias_point(e.bias as f32) - d;
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
    fn soft_matches_its_closed_form_harmonics() {
        // f(u) = u − 4u³/27: a sine of amplitude U gives H1 = U − U³/9 and H3 = U³/27.
        let (amp, drive) = (0.3f32, 12.0f32);
        let u = f64::from(amp * crate::db_to_gain(drive));
        let s = band(Model::Soft, drive);
        let mut sat = Saturator::new(FS, 8, &s);
        let (y, _) = run(&mut sat, &tone(4 * N, amp, K), &s, 512);
        let (h1, h3) = (harmonic(&y, 1), harmonic(&y, 3));
        assert!((h1 / (u - u.powi(3) / 9.0) - 1.0).abs() < 1e-3, "H1 {h1}");
        assert!((h3 / (u.powi(3) / 27.0) - 1.0).abs() < 2e-3, "H3 {h3}");
        assert!(harmonic(&y, 2) < 1e-6 && harmonic(&y, 5) < 1e-6);
    }

    #[test]
    fn the_engine_plays_the_drawn_curve() {
        // Every static model, through the oversampled engine: its harmonics are those of the curve
        // the editor draws, within 0.05 dB down to −80 dB under the fundamental.
        for m in [Model::Diode, Model::Soft, Model::Hard, Model::Fold, Model::Rectify] {
            for (amp, drive, bias) in [(0.25f32, 12.0f32, 0.0f32), (0.1, 24.0, 0.4), (0.5, 3.0, -0.6)] {
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
                let want_y = magnet_response(m, &x_hi, drive, bias_point(bias), 8.0 * FS);
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
    fn symmetric_models_make_no_even_harmonics_and_tube_does() {
        for m in [Model::Diode, Model::Soft, Model::Hard, Model::Fold] {
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
        let (quiet, loud) = (h2_share(0.05), h2_share(0.4));
        assert!(quiet > 1e-3 && loud > 2.0 * quiet, "{quiet} {loud}");
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
