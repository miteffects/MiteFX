//! Mite Verb's algorithm (docs/design/verb-brief.md §1): a room.
//!
//! Pre-delay; an early field of 24 reflections a side (denser as they come later, as a room's
//! image sources), blurred by short allpasses and sent partly into the tail; input diffusion; a
//! 16-line feedback delay network under a Hadamard matrix in which every line carries two
//! allpass diffusers (each pass round the loop is smeared, so the tail turns dense fast and does
//! not repeat), modulated by smoothed random motion, with per-line absorption: decay time per
//! frequency from the decay EQ's bands (each a bell, a shelf or a tilt, none by default). Then
//! width, ducking and the tone EQ (Mite EQ's bands, every shape) on the reverb. This makes the reverb only; the plugin mixes it with the dry signal.
//!
//! Everything is sized in [`Reverb::new`]; `process` and `reset` never allocate. Settings glide
//! (20 ms; Size over 200 ms) and the absorption is designed every [`CONTROL`] samples counted
//! across blocks, so the output is the same for every block size.

use crate::eq::{BandProcessor, BandSettings, BandType};
use crate::filter::Coeffs;
use crate::shimmer::{
    Allpass as PAllpass, Line as PLine, PlateSettings, ShimmerPlate, DATTORRO_FS, DECAY_DIFF as PDECAY_DIFF, EXCURSION, IN_DIFF,
    TANK as PTANK, TAPS as PTAPS,
};
use crate::smooth::LinearSmoother;

pub const LINES: usize = 32;
pub const MAX_PREDELAY_MS: f32 = 500.0;
/// The decay EQ's bands.
pub const DECAY_BANDS: usize = 8;
/// The tone EQ's bands (on the reverb's output).
pub const TONE_BANDS: usize = 8;
/// The tone EQ's glide, ms.
const TONE_SMOOTH_MS: f32 = 20.0;
pub const ER_TAPS: usize = 24;
pub const MAX_EARLY_MS: f32 = 200.0;
/// Samples per control step (designs, glides, the ducking's target).
pub const CONTROL: usize = 32;
/// The lines' main delays at the largest size span this, ms (spread geometrically, jittered: see
/// [`Topology`]).
const MAIN_MS: (f32, f32) = (47.0, 123.0);
/// The smallest size's scale on the main delays (6…16 ms: a small room; the allpasses add 4–6 ms).
const MIN_SCALE: f32 = 0.13;
/// Each line's two in-loop allpasses span these, ms. Not scaled with Size (they never glide).
const LOOP_AP_MS: [(f32, f32); 2] = [(1.1, 2.3), (0.5, 0.9)];
/// Modulation depth at MOD 100 %, ms (at most a fifth of the line's main delay).
const MOD_MS: f32 = 1.6;
/// The input diffusers' lengths per side, ms.
const DIFF_MS: [[f32; 4]; 2] = [[1.31, 2.07, 3.71, 5.29], [1.49, 2.33, 3.29, 4.87]];
/// The largest input diffusion coefficient (DIFFUSION 100 %).
const DIFF_MAX: f32 = 0.75;
/// The early field's blurring allpasses per side, ms.
const ER_AP_MS: [[f32; 2]; 2] = [[0.53, 0.83], [0.61, 0.97]];
/// The tank's input and output gains, and the early field's.
/// (Scaled by √(16/N): the same level whatever the line count.)
const IN_GAIN: f32 = 0.35;
const OUT_GAIN: f32 = 1.95;
const GLIDE_MS: f32 = 20.0;
const SIZE_GLIDE_MS: f32 = 200.0;
/// A pre-delay change crossfades between two read heads over this long (a glide would be a
/// pitch sweep: 500 ms in 20 ms is 25 times the pitch).
const PREDELAY_FADE_MS: f32 = 30.0;
const FREEZE_MS: f32 = 50.0;
const DUCK_ATTACK_MS: f32 = 2.0;
/// The shimmer plate's level at SHIMMER 100 % (it fades in as the square root of the dial).
const SHIM_LEVEL: f32 = 1.0;
/// The shimmer plate's gain per half pass at SHIMMER 0 and 100 % (along the square root).
const SHIM_DECAY: (f32, f32) = (0.7, 0.95);
/// The shimmer's range dials' ends, Hz.
pub const SHIM_RANGE_HZ: (f32, f32) = (20.0, 20_000.0);
/// The interval glides with this time constant, ms.
const SHIM_GLIDE_MS: f32 = 15.0;
/// A type change fades the old tank out and the new one in, ms each way.
const KIND_FADE_MS: f32 = 25.0;
/// The plate's and the spring's output gains (level-matched to the room, measured).
const PLATE_OUT: f32 = 1.33;
const SPRING_OUT: f32 = 1.58;

/// The network's fixed random draws (deterministic): each line's main delay at the largest size
/// and its allpasses (ms), the input's signs (the left input feeds the even lines, the right the
/// odd ones), the outputs, and the mixing matrix's signs and permutation.
///
/// The outputs: each side reads every line once, inside its main delay (at `out_at` of it), at
/// uneven gains, the two vectors orthogonal. Reading the line ends would tie the output to the
/// matrix: a pass later each line end holds a row of it applied to them, an echo at each loop
/// length; read inside, a recurrence lands on N² lags (each pair of lines' offsets) instead of N.
/// Two reads of one line would echo at their gap.
///
/// The mixing matrix is P·H·D: signs, the Hadamard transform, then a permutation. Orthogonal
/// (lossless) and dense like H, but not its own inverse (H·H = I: a signal would come back to its
/// line after two passes).
#[derive(Debug, Clone, Copy)]
pub struct Topology {
    pub main_ms: [f32; LINES],
    pub ap_ms: [[f32; 2]; LINES],
    pub in_sign: [f32; LINES],
    pub out: [[f32; LINES]; 2],
    pub out_at: [[f32; LINES]; 2],
    pub mix_sign: [f32; LINES],
    pub mix_perm: [usize; LINES],
}

impl Topology {
    pub fn new() -> Self {
        let mut rng = Rng(0x6A09_E667);
        let mut t = Self {
            main_ms: [0.0; LINES],
            ap_ms: [[0.0; 2]; LINES],
            in_sign: [0.0; LINES],
            out: [[0.0; LINES]; 2],
            out_at: [[0.0; LINES]; 2],
            mix_sign: [0.0; LINES],
            mix_perm: [0; LINES],
        };
        let ratio = MAIN_MS.1 / MAIN_MS.0;
        for i in 0..LINES {
            let p = (i as f32 + 0.5 + 0.6 * (rng.next() - 0.5)) / LINES as f32;
            t.main_ms[i] = MAIN_MS.0 * ratio.powf(p);
            for (k, &(lo, hi)) in LOOP_AP_MS.iter().enumerate() {
                t.ap_ms[i][k] = lo + (hi - lo) * rng.next();
            }
            let sign = |r: &mut Rng| if r.next() < 0.5 { -1.0 } else { 1.0 };
            t.in_sign[i] = sign(&mut rng);
            t.mix_sign[i] = sign(&mut rng);
            for c in 0..2 {
                t.out[c][i] = sign(&mut rng) * (0.5 + 0.5 * rng.next());
                t.out_at[c][i] = 0.3 + 0.45 * rng.next();
            }
            t.mix_perm[i] = i;
        }
        // Fisher–Yates.
        for i in (1..LINES).rev() {
            let j = ((rng.next() * (i + 1) as f32) as usize).min(i);
            t.mix_perm.swap(i, j);
        }
        // Gram–Schmidt: right orthogonal to left, both of unit length.
        let dot =
            |a: &[f32; LINES], b: &[f32; LINES]| a.iter().zip(b).map(|(x, y)| x * y).sum::<f32>();
        let norm = |a: &mut [f32; LINES]| {
            let n = dot(a, a).sqrt();
            a.iter_mut().for_each(|x| *x /= n);
        };
        norm(&mut t.out[0]);
        let k = dot(&t.out[1], &t.out[0]);
        let left = t.out[0];
        t.out[1]
            .iter_mut()
            .zip(&left)
            .for_each(|(r, l)| *r -= k * l);
        norm(&mut t.out[1]);
        t
    }

    fn main_len_scaled(&self, i: usize, size: f32, fs: f32, scale: f32) -> f32 {
        self.main_ms[i] * tank_scale(size) * scale * 0.001 * fs
    }

    fn ap_len(&self, i: usize, k: usize, fs: f32) -> usize {
        ((self.ap_ms[i][k] * 0.001 * fs).round() as usize).max(2)
    }

    /// Line `i`'s whole loop at `size`, samples: its main delay and its allpasses' (whose mean
    /// group delay is their length).
    pub fn line_len(&self, i: usize, size: f32, fs: f32) -> f32 {
        self.line_len_scaled(i, size, fs, 1.0)
    }

    /// [`Self::line_len`] with the main delay scaled by `scale` (a type's voicing).
    pub fn line_len_scaled(&self, i: usize, size: f32, fs: f32, scale: f32) -> f32 {
        self.main_len_scaled(i, size, fs, scale) + (self.ap_len(i, 0, fs) + self.ap_len(i, 1, fs)) as f32
    }
}

impl Default for Topology {
    fn default() -> Self {
        Self::new()
    }
}

/// A decay band's shape, in its frozen parameter order (append only).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DecayShape {
    /// Around `freq` the decay is `mult` × what it would be without the band.
    #[default]
    Bell,
    /// Below `freq` (towards DC) the decay is `mult` ×.
    LowShelf,
    /// Above `freq` (towards Nyquist) the decay is `mult` ×.
    HighShelf,
    /// A first-order tilt pivoting on `freq`: half the band's rate change at the top, the
    /// opposite half at DC (`mult` > 1: the highs ring longer and the lows shorter).
    Tilt,
}

impl DecayShape {
    pub const ALL: [Self; 4] = [Self::Bell, Self::LowShelf, Self::HighShelf, Self::Tilt];
    pub const NAMES: [&'static str; 4] = ["Bell", "Low Shelf", "High Shelf", "Tilt"];

    pub fn from_index(i: usize) -> Self {
        Self::ALL.get(i).copied().unwrap_or_default()
    }

    pub fn index(self) -> usize {
        Self::ALL.iter().position(|&s| s == self).unwrap_or(0)
    }

    /// The EQ band type that makes it (`mite_dsp::eq`, one section).
    pub fn band_type(self) -> BandType {
        match self {
            Self::Bell => BandType::Bell,
            Self::LowShelf => BandType::LowShelf,
            Self::HighShelf => BandType::HighShelf,
            Self::Tilt => BandType::TiltShelf,
        }
    }
}

/// One band of the decay EQ: its shape multiplies the decay around (bell), below or above
/// (shelves) `freq` by `mult`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DecayBand {
    pub on: bool,
    pub shape: DecayShape,
    pub freq: f32,
    pub mult: f32,
    pub q: f32,
}

impl Default for DecayBand {
    fn default() -> Self {
        Self {
            on: false,
            shape: DecayShape::Bell,
            freq: 1000.0,
            mult: 1.0,
            q: 1.0,
        }
    }
}

impl DecayBand {
    pub fn bell(freq: f32, mult: f32, q: f32) -> Self {
        Self {
            on: true,
            shape: DecayShape::Bell,
            freq,
            mult,
            q,
        }
    }

    pub fn low_shelf(freq: f32, mult: f32) -> Self {
        Self {
            on: true,
            shape: DecayShape::LowShelf,
            freq,
            mult,
            q: std::f32::consts::FRAC_1_SQRT_2,
        }
    }

    pub fn high_shelf(freq: f32, mult: f32) -> Self {
        Self {
            on: true,
            shape: DecayShape::HighShelf,
            freq,
            mult,
            q: std::f32::consts::FRAC_1_SQRT_2,
        }
    }
}

/// The reverb's type: three voicings of the 32-line network, a plate and a spring.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ReverbKind {
    #[default]
    Room,
    Hall,
    Plate,
    Chamber,
    Spring,
}

impl ReverbKind {
    pub const NAMES: [&'static str; 5] = ["Room", "Hall", "Plate", "Chamber", "Spring"];

    pub fn from_index(i: usize) -> Self {
        [Self::Room, Self::Hall, Self::Plate, Self::Chamber, Self::Spring][i.min(4)]
    }

    pub fn index(self) -> usize {
        self as usize
    }

    /// Whether it runs on the 32-line network.
    pub fn network(self) -> bool {
        matches!(self, Self::Room | Self::Hall | Self::Chamber)
    }

    fn voicing(self) -> Voicing {
        match self {
            // A hall: lines nearly twice as long (a slower, smoother build), more drift, a wider
            // early field.
            Self::Hall => Voicing { scale: 1.9, modulation: 1.6, early: 1.8, loop_g: 0.05, diffusion: 1.0, gain: 1.29 },
            // A chamber: short lines, dense at once, a tight early field, little drift.
            Self::Chamber => Voicing { scale: 0.7, modulation: 0.6, early: 0.6, loop_g: 0.1, diffusion: 1.1, gain: 0.86 },
            _ => Voicing { scale: 1.0, modulation: 1.0, early: 1.0, loop_g: 0.0, diffusion: 1.0, gain: 1.0 },
        }
    }
}

/// How a network type differs from the room: its lines' length, modulation and early field
/// (factors), its in-loop allpasses' coefficient (added) and its input diffusion (factor).
#[derive(Debug, Clone, Copy)]
struct Voicing {
    scale: f32,
    modulation: f32,
    early: f32,
    loop_g: f32,
    diffusion: f32,
    /// Output gain, level-matched to the room (measured with `examples/verb_types.rs`).
    gain: f32,
}

/// The longest voicing's scale on the lines and its modulation (the buffers are sized for them).
const MAX_VOICE_SCALE: f32 = 1.9;
const MAX_VOICE_MOD: f32 = 1.6;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ReverbSettings {
    pub kind: ReverbKind,
    pub predelay_ms: f32,
    /// 0…1.
    pub size: f32,
    /// T60 where no decay band acts, s.
    pub decay_s: f32,
    pub bands: [DecayBand; DECAY_BANDS],
    /// The tone EQ on the reverb (Mite EQ's bands; placement and dynamics are not used).
    pub eq: [BandSettings; TONE_BANDS],
    /// 0…1.
    pub diffusion: f32,
    /// Early reflections' level, 0…1.
    pub early: f32,
    /// The early field's span, ms.
    pub early_size_ms: f32,
    /// How much of the early field feeds the tail, 0…1.
    pub early_send: f32,
    /// 0…1.
    pub mod_depth: f32,
    pub mod_rate: f32,
    /// 0…1.5 (1: as the tank makes it).
    pub width: f32,
    pub freeze: bool,
    /// Ducking depth, dB (≥ 0; 0: off).
    pub duck_db: f32,
    pub duck_thresh_db: f32,
    pub duck_release_ms: f32,
    /// Shimmer (`crate::shimmer`), 0…1 (0: off): a Dattorro plate beside the room whose tank
    /// shifts its tail by `shimmer_pitch` semitones every pass; how long it sustains and how
    /// loud it is. `shimmer_low_hz`/`shimmer_high_hz`: the tank's damping.
    pub shimmer: f32,
    pub shimmer_pitch: f32,
    pub shimmer_low_hz: f32,
    pub shimmer_high_hz: f32,
}

impl Default for ReverbSettings {
    fn default() -> Self {
        Self {
            kind: ReverbKind::Room,
            predelay_ms: 10.0,
            size: 0.5,
            decay_s: 2.0,
            bands: [DecayBand::default(); DECAY_BANDS],
            eq: [BandSettings::default(); TONE_BANDS],
            diffusion: 0.7,
            early: 0.5,
            early_size_ms: 35.0,
            early_send: 0.0,
            mod_depth: 0.3,
            mod_rate: 0.6,
            width: 1.0,
            freeze: false,
            duck_db: 0.0,
            duck_thresh_db: -30.0,
            duck_release_ms: 300.0,
            shimmer: 0.0,
            shimmer_pitch: 12.0,
            shimmer_low_hz: 150.0,
            shimmer_high_hz: 6500.0,
        }
    }
}

/// Size's scale on the lines' main delays.
pub fn tank_scale(size: f32) -> f32 {
    MIN_SCALE * (1.0 / MIN_SCALE).powf(size.clamp(0.0, 1.0))
}

/// Line `i`'s whole loop at `size`, samples (see [`Topology::line_len`]).
pub fn line_len(i: usize, size: f32, fs: f32) -> f32 {
    Topology::new().line_len(i, size, fs)
}

impl ReverbSettings {
    /// With a low shelf (decay × `lo.1` below `lo.0`) in band 1 and a high shelf in band 2: a
    /// room's usual absorption, for tests and presets.
    pub fn with_shelves(mut self, lo: (f32, f32), hi: (f32, f32)) -> Self {
        self.bands[0] = DecayBand::low_shelf(lo.0, lo.1);
        self.bands[1] = DecayBand::high_shelf(hi.0, hi.1);
        self
    }
}

/// A decay band's section for a loss of `db` (dB a pass) against the rest of the loop.
fn decay_section(b: &DecayBand, db: f64, fs: f64) -> Coeffs {
    let settings = BandSettings {
        enabled: true,
        kind: b.shape.band_type(),
        freq: f64::from(b.freq),
        gain_db: db,
        q: f64::from(b.q),
        ..BandSettings::default()
    };
    crate::eq::design(&settings, fs)
        .as_slice()
        .first()
        .copied()
        .unwrap_or(Coeffs::IDENTITY)
}

/// One line's absorption: a gain and a section per decay band.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Absorption {
    pub gain: f64,
    pub sections: [Coeffs; DECAY_BANDS],
}

impl Absorption {
    /// The loop's gain through this absorption at `w`.
    pub fn magnitude(&self, w: f64) -> f64 {
        self.gain
            * self
                .sections
                .iter()
                .map(|c| c.magnitude_sq(w))
                .product::<f64>()
                .sqrt()
    }
}

/// The grid the loop gain is checked on: DC, 64 log-spaced points, Nyquist, then each section's
/// own frequency (a sharp bell's peak can sit between grid points).
const GRID: usize = 66;
fn check_points(s: &ReverbSettings, fs: f64) -> [f64; GRID + DECAY_BANDS] {
    let mut p = [0.0; GRID + DECAY_BANDS];
    for (i, v) in p.iter_mut().enumerate().take(GRID) {
        *v = match i {
            0 => 0.0,
            i if i == GRID - 1 => std::f64::consts::PI,
            _ => std::f64::consts::PI * 10f64.powf(-3.5 + 3.5 * (i - 1) as f64 / (GRID - 3) as f64),
        };
    }
    let w = |f: f32| std::f64::consts::TAU * (f64::from(f) / fs).clamp(0.0, 0.5);
    for (k, b) in s.bands.iter().enumerate() {
        p[GRID + k] = w(b.freq);
    }
    p.sort_by(f64::total_cmp);
    p
}

/// The absorption for a line of `len` samples: a pass loses −60 dB × len / (T60(f)·fs), with
/// T60 the decay; each band multiplies the decay where it acts by its `mult` (rates add: a
/// band's dB a pass is what turns the decay there from T to T × mult). `freeze` (0…1) fades
/// every loss to nothing. The loop gain is kept under 1 at every frequency.
pub fn design_absorption(len: f64, s: &ReverbSettings, fs: f64, freeze: f64) -> Absorption {
    let keep = (1.0 - freeze).clamp(0.0, 1.0);
    let t = f64::from(s.decay_s.max(0.01));
    // At most 100 dB a pass: a decay shorter than a pass round the loop is gone after one anyway.
    let per_pass = |t60: f64| (-60.0 * len / (fs * t60.max(0.01))).max(-100.0) * keep;
    let mid = per_pass(t);
    let db = |d: f64| 10f64.powf(d / 20.0);
    let mut sections = [Coeffs::IDENTITY; DECAY_BANDS];
    for (k, b) in s.bands.iter().enumerate() {
        if b.on && (b.mult - 1.0).abs() > 1e-6 {
            let d = per_pass(t * f64::from(b.mult)) - mid;
            // A tilt's gain is the swing from DC to the top: the band's rate change at the top.
            let d = if b.shape == DecayShape::Tilt {
                2.0 * d
            } else {
                d
            };
            sections[k] = decay_section(b, d, fs);
        }
    }
    let mut a = Absorption {
        gain: db(mid),
        sections,
    };
    if keep > 0.0 {
        // The highest checked point, then refined between its neighbours (ternary search).
        let pts = check_points(s, fs);
        let best = (0..pts.len())
            .max_by(|&i, &j| a.magnitude(pts[i]).total_cmp(&a.magnitude(pts[j])))
            .unwrap_or(0);
        let (mut lo, mut hi) = (
            pts[best.saturating_sub(1)],
            pts[(best + 1).min(pts.len() - 1)],
        );
        for _ in 0..40 {
            let (m1, m2) = (lo + (hi - lo) / 3.0, hi - (hi - lo) / 3.0);
            if a.magnitude(m1) < a.magnitude(m2) {
                lo = m1;
            } else {
                hi = m2;
            }
        }
        let peak = a.magnitude(pts[best]).max(a.magnitude(0.5 * (lo + hi)));
        // A margin for a broad maximum the search lands just beside (bands overlapping).
        let limit = 1.0 - 1e-5;
        if peak > limit {
            a.gain *= limit / peak;
        }
    }
    a
}

/// The decay time (T60, s) the lines give at `freq`: the mean over the lines of what their
/// absorption, as designed, loses per second there. What the editor draws.
pub fn decay_at(s: &ReverbSettings, freq: f64, fs: f64) -> f64 {
    let mut out = [0.0];
    decay_curve(s, &[freq], fs, &mut out);
    out[0]
}

/// [`decay_at`] at many frequencies at once (each line designed once): `out[k]` for `freqs[k]`.
pub fn decay_curve(s: &ReverbSettings, freqs: &[f64], fs: f64, out: &mut [f64]) {
    out.iter_mut().for_each(|v| *v = 0.0);
    let topo = Topology::new();
    let scale = s.kind.voicing().scale;
    for i in 0..LINES {
        let len = f64::from(topo.line_len_scaled(i, s.size, fs as f32, scale));
        let a = design_absorption(len, s, fs, 0.0);
        for (o, &f) in out.iter_mut().zip(freqs) {
            let w = std::f64::consts::TAU * (f / fs).clamp(0.0, 0.5);
            let per_pass_db = 20.0 * a.magnitude(w).log10();
            *o += if per_pass_db < -1e-9 {
                -60.0 * len / fs / per_pass_db
            } else {
                1e6
            };
        }
    }
    out.iter_mut().for_each(|v| *v /= LINES as f64);
}

/// The decay EQ's T60 curve, cheaply: one line of the lines' mean loop length, designed once
/// (with the same loop-gain check). Every line's band sections scale with its length (a band's
/// dB a pass is proportional to it), so each line gives nearly the same T60(f) and the mean line
/// stands for them all: within a few % of [`decay_curve`] (tested), at a fraction of the cost.
/// What the editor draws and drags against, every frame.
#[derive(Debug, Clone, Copy)]
pub struct DecayModel {
    len: f64,
    fs: f64,
    a: Absorption,
}

impl DecayModel {
    pub fn new(s: &ReverbSettings, fs: f64) -> Self {
        let topo = Topology::new();
        let scale = s.kind.voicing().scale;
        let len = (0..LINES)
            .map(|i| f64::from(topo.line_len_scaled(i, s.size, fs as f32, scale)))
            .sum::<f64>()
            / LINES as f64;
        Self {
            len,
            fs,
            a: design_absorption(len, s, fs, 0.0),
        }
    }

    /// T60 at `freq`, s.
    pub fn at(&self, freq: f64) -> f64 {
        let w = std::f64::consts::TAU * (freq / self.fs).clamp(0.0, 0.5);
        let per_pass_db = 20.0 * self.a.magnitude(w).log10();
        if per_pass_db < -1e-9 {
            -60.0 * self.len / self.fs / per_pass_db
        } else {
            1e6
        }
    }
}

/// A delay line with a power-of-two buffer.
#[derive(Debug, Clone)]
pub(crate) struct Delay {
    buf: Vec<f32>,
    mask: usize,
    pos: usize,
}

impl Delay {
    pub(crate) fn new(max_delay: usize) -> Self {
        let len = (max_delay + 4).next_power_of_two();
        Self {
            buf: vec![0.0; len],
            mask: len - 1,
            pos: 0,
        }
    }

    pub(crate) fn reset(&mut self) {
        self.buf.iter_mut().for_each(|x| *x = 0.0);
        self.pos = 0;
    }

    #[inline]
    pub(crate) fn write(&mut self, x: f32) {
        self.buf[self.pos] = x;
        self.pos = (self.pos + 1) & self.mask;
    }

    /// The sample written `d` samples ago (0: the last one).
    #[inline]
    pub(crate) fn tap(&self, d: usize) -> f32 {
        self.buf[(self.pos.wrapping_sub(1 + d)) & self.mask]
    }

    /// The signal `d` (≥ 1) samples ago, cubic Lagrange through the four samples around it (the
    /// fraction always between the middle two, where it is passive).
    #[inline]
    pub(crate) fn read(&self, d: f32) -> f32 {
        let d = d.max(1.0);
        let k = d as usize;
        let f = d - k as f32;
        let (x0, x1, x2, x3) = (
            self.tap(k - 1),
            self.tap(k),
            self.tap(k + 1),
            self.tap(k + 2),
        );
        // Delay relative to x0: D = 1 + f.
        let dd = 1.0 + f;
        let (dm1, dm2, dm3) = (dd - 1.0, dd - 2.0, dd - 3.0);
        let h0 = -dm1 * dm2 * dm3 / 6.0;
        let h1 = dd * dm2 * dm3 * 0.5;
        let h2 = -dd * dm1 * dm3 * 0.5;
        let h3 = dd * dm1 * dm2 / 6.0;
        x0 * h0 + x1 * h1 + x2 * h2 + x3 * h3
    }

    /// The signal `d` (≥ 0) samples ago, linearly interpolated (outside any loop: the early taps).
    #[inline]
    pub(crate) fn read_linear(&self, d: f32) -> f32 {
        let d = d.max(0.0);
        let k = d as usize;
        let f = d - k as f32;
        let (a, b) = (self.tap(k), self.tap(k + 1));
        a + (b - a) * f
    }
}

/// A modulated line's read: first-order allpass interpolation (lossless at every frequency, so
/// the loop's decay is the absorption's alone; cubic interpolation would shorten the highs).
#[derive(Debug, Clone, Copy, Default)]
struct AllpassRead {
    y: f32,
}

impl AllpassRead {
    /// The signal `d` (≥ 2) samples ago on `line`: a whole part from it, the rest (0.5…1.5) by
    /// an allpass `(a + z⁻¹) / (1 + a z⁻¹)`, `a = (1 − D) / (1 + D)`.
    #[inline]
    fn read(&mut self, line: &Delay, d: f32) -> f32 {
        let d = d.max(1.5);
        let n = (d - 0.5) as usize;
        let frac = d - n as f32;
        let a = (1.0 - frac) / (1.0 + frac);
        let (x0, x1) = (line.tap(n - 1), line.tap(n));
        let y = a * x0 + x1 - a * self.y;
        self.y = y;
        y
    }
}

/// A Schroeder allpass of a fixed length.
#[derive(Debug, Clone)]
pub(crate) struct Allpass {
    pub(crate) line: Delay,
    len: usize,
}

impl Allpass {
    pub(crate) fn new(len: usize) -> Self {
        Self {
            line: Delay::new(len),
            len: len.max(1),
        }
    }

    #[inline]
    pub(crate) fn process(&mut self, x: f32, g: f32) -> f32 {
        let delayed = self.line.tap(self.len - 1);
        let w = x + g * delayed;
        self.line.write(w);
        delayed - g * w
    }
}

/// A line's absorption in the audio path: the sections that do something (f32, transposed
/// direct form II) and a gain. A new design is reached along a straight line over one control
/// step, every sample (a step every 32 samples is heard when the decay's tone moves fast). A
/// section that becomes identity stops running and forgets its state once it has glided there.
#[derive(Debug, Clone, Copy, Default)]
struct LineFilter {
    gain: f32,
    c: [[f32; 5]; DECAY_BANDS],
    /// Per sample towards the target, while `left` > 0.
    d_gain: f32,
    dc: [[f32; 5]; DECAY_BANDS],
    left: u32,
    on: [bool; DECAY_BANDS],
    off_after: [bool; DECAY_BANDS],
    z: [[f32; 2]; DECAY_BANDS],
}

impl LineFilter {
    /// `glide`: reach `a` over [`CONTROL`] samples (else at once).
    fn set(&mut self, a: &Absorption, glide: bool) {
        let n = CONTROL as f32;
        let gain = a.gain as f32;
        self.d_gain = if glide { (gain - self.gain) / n } else { 0.0 };
        if !glide {
            self.gain = gain;
        }
        for (k, s) in a.sections.iter().enumerate() {
            let target = [
                s.b0 as f32,
                s.b1 as f32,
                s.b2 as f32,
                s.a1 as f32,
                s.a2 as f32,
            ];
            let identity = *s == Coeffs::IDENTITY;
            if !self.on[k] && identity {
                continue;
            }
            if !self.on[k] {
                // Coming on: it starts from identity.
                self.c[k] = [1.0, 0.0, 0.0, 0.0, 0.0];
                self.z[k] = [0.0; 2];
                self.on[k] = true;
            }
            if glide {
                for (d, (t, c)) in self.dc[k].iter_mut().zip(target.iter().zip(&self.c[k])) {
                    *d = (t - c) / n;
                }
            } else {
                self.c[k] = target;
                self.dc[k] = [0.0; 5];
            }
            self.off_after[k] = identity;
            if identity && !glide {
                self.on[k] = false;
                self.z[k] = [0.0; 2];
            }
        }
        self.left = if glide { CONTROL as u32 } else { 0 };
    }

    #[inline]
    fn process(&mut self, mut x: f32) -> f32 {
        if self.left > 0 {
            self.left -= 1;
            self.gain += self.d_gain;
            for k in 0..DECAY_BANDS {
                if self.on[k] {
                    for j in 0..5 {
                        self.c[k][j] += self.dc[k][j];
                    }
                }
            }
            if self.left == 0 {
                for k in 0..DECAY_BANDS {
                    if self.on[k] && self.off_after[k] {
                        self.on[k] = false;
                        self.z[k] = [0.0; 2];
                    }
                }
            }
        }
        for k in 0..DECAY_BANDS {
            if self.on[k] {
                let (c, z) = (&self.c[k], &mut self.z[k]);
                let y = c[0] * x + z[0];
                z[0] = c[1] * x - c[3] * y + z[1];
                z[1] = c[2] * x - c[4] * y;
                x = y;
            }
        }
        x * self.gain
    }

    fn reset(&mut self) {
        self.z = [[0.0; 2]; DECAY_BANDS];
        self.on = [false; DECAY_BANDS];
        self.left = 0;
    }
}

/// In-place fast Walsh–Hadamard transform over the lines, scaled to be orthonormal.
#[inline]
fn hadamard(v: &mut [f32; LINES]) {
    let mut h = 1;
    while h < LINES {
        let mut i = 0;
        while i < LINES {
            for j in i..i + h {
                let (a, b) = (v[j], v[j + h]);
                v[j] = a + b;
                v[j + h] = a - b;
            }
            i += h * 2;
        }
        h *= 2;
    }
    let k = 1.0 / (LINES as f32).sqrt();
    for x in v.iter_mut() {
        *x *= k;
    }
}

/// A small deterministic random source (xorshift32).
#[derive(Debug, Clone, Copy)]
pub(crate) struct Rng(pub(crate) u32);

impl Rng {
    #[inline]
    pub(crate) fn next(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 17;
        self.0 ^= self.0 << 5;
        self.0 as f32 / u32::MAX as f32
    }
}

/// Smoothed random motion: from one random value to the next along a smoothstep, each move
/// lasting 0.6…1.4 of the period. Never a corner, never periodic.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Wander {
    rng: Rng,
    from: f32,
    to: f32,
    phase: f32,
    pub(crate) step: f32,
    jitter: f32,
}

impl Wander {
    pub(crate) fn new(seed: u32) -> Self {
        let mut rng = Rng(seed.max(1));
        let from = rng.next() * 2.0 - 1.0;
        let to = rng.next() * 2.0 - 1.0;
        let jitter = 1.0 / (0.6 + 0.8 * rng.next());
        Self {
            rng,
            from,
            to,
            phase: rng.next(),
            step: 0.0,
            jitter,
        }
    }

    #[inline]
    pub(crate) fn next(&mut self) -> f32 {
        self.phase += self.step * self.jitter;
        if self.phase >= 1.0 {
            self.phase -= 1.0;
            self.from = self.to;
            self.to = self.rng.next() * 2.0 - 1.0;
            self.jitter = 1.0 / (0.6 + 0.8 * self.rng.next());
        }
        let p = self.phase;
        self.from + (self.to - self.from) * p * p * (3.0 - 2.0 * p)
    }
}

/// The early field's taps, per side: time (0…1 of the span), gain, the side it reads.
#[derive(Debug, Clone, Copy)]
struct EarlyTaps {
    at: [[f32; ER_TAPS]; 2],
    gain: [[f32; ER_TAPS]; 2],
    from: [[usize; ER_TAPS]; 2],
}

impl EarlyTaps {
    /// As a room's image sources: their count grows with the square of the time (so the times
    /// follow the cube root of an even spread), each a little jittered, falling off with
    /// distance, random in sign; a third read the other side. Each side's gains sum to unit power.
    fn new() -> Self {
        let mut rng = Rng(0x2545_F491);
        let mut t = Self {
            at: [[0.0; ER_TAPS]; 2],
            gain: [[0.0; ER_TAPS]; 2],
            from: [[0; ER_TAPS]; 2],
        };
        for c in 0..2 {
            let mut power = 0.0;
            for k in 0..ER_TAPS {
                let p = (k as f32 + 0.5 + 0.7 * (rng.next() - 0.5)) / ER_TAPS as f32;
                let at = p.cbrt();
                let sign = if rng.next() < 0.5 { -1.0 } else { 1.0 };
                let g = sign * (0.6 + 0.4 * rng.next()) / (0.25 + at);
                t.at[c][k] = at;
                t.gain[c][k] = g;
                t.from[c][k] = if rng.next() < 0.33 { 1 - c } else { c };
                power += g * g;
            }
            let norm = 1.0 / power.sqrt();
            t.gain[c].iter_mut().for_each(|g| *g *= norm);
        }
        t
    }
}

/// The settings that glide, in one array: the scalars, then each band's freq, mult (1 while off
/// or while its shape is changing, so switching glides) and Q.
const SCALARS: usize = 12;
const SMOOTHED: usize = SCALARS + 3 * DECAY_BANDS;

/// `shapes`: the shape each band is running (a band whose shape is to change glides to nothing
/// first).
fn pack(s: &ReverbSettings, shapes: &[DecayShape; DECAY_BANDS]) -> [f32; SMOOTHED] {
    let mut v = [0.0; SMOOTHED];
    v[..SCALARS].copy_from_slice(&[
        s.predelay_ms,
        s.size,
        s.decay_s,
        s.diffusion,
        s.early,
        s.mod_depth,
        s.mod_rate,
        s.width,
        s.duck_thresh_db,
        s.duck_db,
        s.early_size_ms,
        s.early_send,
    ]);
    for (k, b) in s.bands.iter().enumerate() {
        v[SCALARS + 3 * k] = b.freq;
        v[SCALARS + 3 * k + 1] = if b.on && b.shape == shapes[k] {
            b.mult
        } else {
            1.0
        };
        v[SCALARS + 3 * k + 2] = b.q;
    }
    v
}

fn unpack(
    v: &[f32; SMOOTHED],
    s: &ReverbSettings,
    shapes: &[DecayShape; DECAY_BANDS],
) -> ReverbSettings {
    let mut bands = s.bands;
    for (k, b) in bands.iter_mut().enumerate() {
        b.freq = v[SCALARS + 3 * k];
        b.mult = v[SCALARS + 3 * k + 1];
        b.q = v[SCALARS + 3 * k + 2];
        b.shape = shapes[k];
        // Still sounding while it glides back to nothing.
        b.on = (b.mult - 1.0).abs() > 1e-6;
    }
    ReverbSettings {
        predelay_ms: v[0],
        size: v[1],
        decay_s: v[2],
        diffusion: v[3],
        early: v[4],
        mod_depth: v[5],
        mod_rate: v[6],
        width: v[7],
        duck_thresh_db: v[8],
        duck_db: v[9],
        early_size_ms: v[10],
        early_send: v[11],
        bands,
        // Read from the settings themselves (they never redesign the lines).
        shimmer: 0.0,
        shimmer_pitch: 0.0,
        shimmer_low_hz: 0.0,
        shimmer_high_hz: 0.0,
        ..*s
    }
}

/// What the last block did, for the editor.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ReverbTelemetry {
    /// The deepest ducking, dB (≤ 0).
    pub duck_db: f32,
    /// How frozen the tank is, 0…1.
    pub freeze: f32,
}

impl Default for ReverbTelemetry {
    fn default() -> Self {
        Self {
            duck_db: 0.0,
            freeze: 0.0,
        }
    }
}

pub struct Reverb {
    fs: f32,
    pre: [Delay; 2],
    diff: [[Allpass; 4]; 2],
    early_ap: [[Allpass; 2]; 2],
    taps: EarlyTaps,
    topo: Topology,
    lines: [Delay; LINES],
    reads: [AllpassRead; LINES],
    loop_ap: [[Allpass; 2]; LINES],
    filters: [LineFilter; LINES],
    /// The shape each decay band is running.
    shapes: [DecayShape; DECAY_BANDS],
    tone: [BandProcessor; TONE_BANDS],
    smooth: [LinearSmoother; SMOOTHED],
    freeze: LinearSmoother,
    /// The pre-delay's two read heads (samples) and the crossfade between them (0: all on the
    /// first; `None`: not fading).
    pd: [f32; 2],
    pd_fade: Option<f32>,
    pd_step: f32,
    /// The current control step's start and end: the early field's span (samples), each line's
    /// main delay (samples), the ducking gain.
    er: [f32; 2],
    len: [[f32; LINES]; 2],
    duck: [f32; 2],
    wander: [Wander; LINES],
    /// Gains that glide per sample within a control step: start and end, as `er`.
    depth: [f32; 2],
    diff_g: [f32; 2],
    loop_g: [f32; 2],
    early: [f32; 2],
    send: [f32; 2],
    width: [f32; 2],
    env: f32,
    duck_g: f32,
    duck_k: f32,
    phase: usize,
    designed: Option<(ReverbSettings, f32, ReverbKind)>,
    /// Shimmer: the plate, whether it is running, its settings, its level (as `er`), and the
    /// interval's glide coefficient.
    plate: ShimmerPlate,
    plate_on: bool,
    /// The type running, the gain fading it out and in on a change (its step per sample), and
    /// the plate and spring tanks.
    kind: ReverbKind,
    kind_g: f32,
    kind_step: f32,
    plate_tank: PlateTank,
    spring: SpringTank,
    plate_s: PlateSettings,
    shim: [f32; 2],
    shim_glide: f32,
}

impl Reverb {
    /// Sizes every buffer for `fs`. Allocates.
    pub fn new(fs: f32, s: &ReverbSettings) -> Self {
        let ms = |m: f32| (m * 0.001 * fs).ceil() as usize;
        let topo = Topology::new();
        let mut r = Self {
            fs,
            pre: std::array::from_fn(|_| Delay::new(ms(MAX_PREDELAY_MS + MAX_EARLY_MS) + 8)),
            diff: std::array::from_fn(|c| std::array::from_fn(|k| Allpass::new(ms(DIFF_MS[c][k])))),
            early_ap: std::array::from_fn(|c| {
                std::array::from_fn(|k| Allpass::new(ms(ER_AP_MS[c][k])))
            }),
            taps: EarlyTaps::new(),
            topo,
            lines: std::array::from_fn(|i| Delay::new(ms(topo.main_ms[i] * MAX_VOICE_SCALE + MOD_MS * MAX_VOICE_MOD) + 8)),
            reads: [AllpassRead::default(); LINES],
            loop_ap: std::array::from_fn(|i| {
                std::array::from_fn(|k| Allpass::new(topo.ap_len(i, k, fs)))
            }),
            filters: [LineFilter::default(); LINES],
            shapes: [DecayShape::Bell; DECAY_BANDS],
            tone: std::array::from_fn(|_| BandProcessor::new(f64::from(fs), TONE_SMOOTH_MS)),
            smooth: std::array::from_fn(|_| LinearSmoother::new(0.0)),
            freeze: LinearSmoother::new(0.0),
            pd: [0.0; 2],
            pd_fade: None,
            pd_step: 1.0 / (PREDELAY_FADE_MS * 0.001 * fs),
            er: [0.0; 2],
            len: [[0.0; LINES]; 2],
            duck: [1.0; 2],
            wander: std::array::from_fn(|i| Wander::new(0x9E37_79B9u32.wrapping_mul(i as u32 + 1))),
            depth: [0.0; 2],
            diff_g: [0.0; 2],
            loop_g: [0.0; 2],
            early: [0.0; 2],
            send: [0.0; 2],
            width: [1.0; 2],
            env: 0.0,
            duck_g: 1.0,
            duck_k: 1.0 - (-1.0 / (0.003 * fs)).exp(),
            phase: 0,
            designed: None,
            plate: ShimmerPlate::new(fs),
            plate_on: false,
            kind: s.kind,
            kind_g: 1.0,
            kind_step: 1.0 / (KIND_FADE_MS * 0.001 * fs),
            plate_tank: PlateTank::new(fs),
            spring: SpringTank::new(fs),
            plate_s: PlateSettings { decay: 0.5, ratio: 2.0, low_hz: 150.0, high_hz: 6500.0 },
            shim: [0.0; 2],
            shim_glide: 1.0 - (-(CONTROL as f32) / (SHIM_GLIDE_MS * 0.001 * fs)).exp(),
        };
        let control_rate = fs / CONTROL as f32;
        for (k, sm) in r.smooth.iter_mut().enumerate() {
            sm.set_ramp(control_rate, if k == 1 { SIZE_GLIDE_MS } else { GLIDE_MS });
        }
        r.freeze.set_ramp(fs, FREEZE_MS);
        r.reset(s);
        r
    }

    pub fn sample_rate(&self) -> f32 {
        self.fs
    }

    /// Silences every line and jumps to `s`.
    pub fn reset(&mut self, s: &ReverbSettings) {
        for d in self.pre.iter_mut().chain(self.lines.iter_mut()) {
            d.reset();
        }
        for a in self
            .diff
            .iter_mut()
            .flatten()
            .chain(self.early_ap.iter_mut().flatten())
            .chain(self.loop_ap.iter_mut().flatten())
        {
            a.line.reset();
        }
        for f in &mut self.filters {
            f.reset();
        }
        self.reads = [AllpassRead::default(); LINES];
        for (p, b) in self.tone.iter_mut().zip(&s.eq) {
            p.reset(b);
        }
        self.shapes = std::array::from_fn(|k| s.bands[k].shape);
        for (sm, v) in self.smooth.iter_mut().zip(pack(s, &self.shapes)) {
            sm.reset(v);
        }
        self.freeze.reset(if s.freeze { 1.0 } else { 0.0 });
        self.wander =
            std::array::from_fn(|i| Wander::new(0x9E37_79B9u32.wrapping_mul(i as u32 + 1)));
        self.env = 0.0;
        self.phase = 0;
        self.duck_g = 1.0;
        self.designed = None;
        self.plate.reset();
        self.plate_on = false;
        self.kind = s.kind;
        self.kind_g = 1.0;
        self.reset_tanks();
        self.control(s, true);
        self.control(s, true);
    }

    /// Silences every tank (the network's lines, the plate, the spring), as a type change does
    /// while faded out.
    fn reset_tanks(&mut self) {
        for d in &mut self.lines {
            d.reset();
        }
        for a in self.loop_ap.iter_mut().flatten() {
            a.line.reset();
        }
        for f in &mut self.filters {
            f.reset();
        }
        self.reads = [AllpassRead::default(); LINES];
        self.plate_tank.reset();
        self.spring.reset();
    }

    /// Moves to the next control step: glides the settings, designs what moved, and sets each
    /// glide's start and end.
    #[allow(clippy::needless_range_loop)] // the lines' parallel arrays, by line
    fn control(&mut self, s: &ReverbSettings, jump: bool) {
        if jump {
            self.shapes = std::array::from_fn(|k| s.bands[k].shape);
        }
        for (sm, v) in self.smooth.iter_mut().zip(pack(s, &self.shapes)) {
            sm.set_target(v);
            if jump {
                sm.reset(v);
            } else {
                sm.next_value();
            }
        }
        // A band changing shape has glided to nothing: it takes its new shape and glides back.
        for k in 0..DECAY_BANDS {
            let mult = &self.smooth[SCALARS + 3 * k + 1];
            if self.shapes[k] != s.bands[k].shape && (mult.current() - 1.0).abs() < 1e-6 {
                self.shapes[k] = s.bands[k].shape;
            }
        }
        let v: [f32; SMOOTHED] = std::array::from_fn(|k| self.smooth[k].current());
        let g = unpack(&v, s, &self.shapes);
        let fs = self.fs;
        let freeze = self.freeze.current();
        let pd = (s.predelay_ms.clamp(0.0, MAX_PREDELAY_MS) * 0.001 * fs).max(1.0);
        if jump {
            self.pd = [pd; 2];
            self.pd_fade = None;
        } else if self.pd_fade.is_none() && (pd - self.pd[0]).abs() > 0.25 {
            self.pd[1] = pd;
            self.pd_fade = Some(0.0);
        }
        self.er[0] = self.er[1];
        let v = self.kind.voicing();
        self.er[1] = (g.early_size_ms * v.early).clamp(1.0, MAX_EARLY_MS) * 0.001 * fs;
        for i in 0..LINES {
            self.len[0][i] = self.len[1][i];
            self.len[1][i] = self.topo.main_len_scaled(i, g.size, fs, v.scale);
        }
        if jump {
            self.er[0] = self.er[1];
            self.len[0] = self.len[1];
        }
        if self.designed != Some((g, freeze, self.kind)) {
            let design = |len: f32| design_absorption(f64::from(len), &g, f64::from(fs), f64::from(freeze));
            match self.kind {
                ReverbKind::Plate => {
                    self.plate_tank.size(g.size);
                    for h in 0..2 {
                        let a = design(self.plate_tank.half_len(h));
                        self.plate_tank.filt[h].set(&a, !jump);
                    }
                }
                ReverbKind::Spring => {
                    self.spring.size(g.size);
                    for c in 0..2 {
                        let a = design(self.spring.loop_len(c));
                        self.spring.filt[c].set(&a, !jump);
                    }
                }
                _ => {
                    for i in 0..LINES {
                        self.filters[i].set(&design(self.topo.line_len_scaled(i, g.size, fs, v.scale)), !jump);
                    }
                }
            }
            let step = g.mod_rate.max(0.01) / fs;
            for w in &mut self.wander {
                w.step = step;
            }
            self.designed = Some((g, freeze, self.kind));
        }
        // Shimmer: the plate's sustain and level from the dial, the interval gliding.
        let amount = s.shimmer.clamp(0.0, 1.0);
        let to = (s.shimmer_pitch.clamp(-24.0, 24.0) / 12.0).exp2();
        let ps = &mut self.plate_s;
        ps.ratio = if jump { to } else { ps.ratio + (to - ps.ratio) * self.shim_glide };
        ps.decay = SHIM_DECAY.0 + (SHIM_DECAY.1 - SHIM_DECAY.0) * amount.sqrt();
        ps.low_hz = s.shimmer_low_hz.clamp(SHIM_RANGE_HZ.0, SHIM_RANGE_HZ.1);
        ps.high_hz = s.shimmer_high_hz.clamp(SHIM_RANGE_HZ.0, SHIM_RANGE_HZ.1);
        let d = g.diffusion.clamp(0.0, 1.0);
        let next = [
            g.mod_depth * v.modulation * (MOD_MS * 0.001 * fs).min(0.2 * self.len[1][0]),
            (d * DIFF_MAX * v.diffusion).min(0.8),
            (0.3 + 0.35 * d + v.loop_g).min(0.75),
            g.early,
            g.early_send.clamp(0.0, 1.0),
            g.width,
            SHIM_LEVEL * amount.sqrt(),
        ];
        for (v, n) in [
            &mut self.depth,
            &mut self.diff_g,
            &mut self.loop_g,
            &mut self.early,
            &mut self.send,
            &mut self.width,
            &mut self.shim,
        ]
        .into_iter()
        .zip(next)
        {
            v[0] = if jump { n } else { v[1] };
            v[1] = n;
        }
        // Ducking: the input's envelope against the threshold, 1 dB per dB, at most the depth.
        self.duck[0] = self.duck[1];
        let over = crate::gain_to_db(self.env.max(1e-9)) - g.duck_thresh_db;
        self.duck[1] = crate::db_to_gain(-over.clamp(0.0, g.duck_db.max(0.0)));
        if jump {
            self.duck[0] = self.duck[1];
        }
    }

    /// One sample of the 32-line network: each line's main delay (modulated), then its two
    /// allpasses; the outputs read inside the lines; the mix; the absorption; the input.
    #[allow(clippy::needless_range_loop)] // the lines' parallel arrays, by line
    fn network(&mut self, into: &[f32; 2], t: f32, depth: f32, loop_g: f32) -> (f32, f32) {
        let mut v = [0.0f32; LINES];
        let (mut ol, mut or) = (0.0f32, 0.0f32);
        for i in 0..LINES {
            let len = self.len[0][i] + (self.len[1][i] - self.len[0][i]) * t;
            let m = self.wander[i].next();
            let d = self.reads[i].read(&self.lines[i], len - 1.0 + depth * m);
            let a = self.loop_ap[i][0].process(d, loop_g);
            v[i] = self.loop_ap[i][1].process(a, -loop_g);
            ol += self.topo.out[0][i] * self.lines[i].read_linear(len * self.topo.out_at[0][i]);
            or += self.topo.out[1][i] * self.lines[i].read_linear(len * self.topo.out_at[1][i]);
        }
        let mut h = [0.0f32; LINES];
        for i in 0..LINES {
            h[i] = v[i] * self.topo.mix_sign[i];
        }
        hadamard(&mut h);
        for i in 0..LINES {
            let y = self.filters[i].process(h[self.topo.mix_perm[i]]);
            let x = self.topo.in_sign[i] * into[i & 1];
            self.lines[i].write(y + x);
        }
        (ol, or)
    }

    /// Writes the reverb of `input` (one or two channels) into `wet` (as many). Never allocates.
    #[allow(clippy::needless_range_loop)] // the lines' parallel arrays, by line
    pub fn process(
        &mut self,
        input: [&[f32]; 2],
        wet: [&mut [f32]; 2],
        stereo: bool,
        s: &ReverbSettings,
    ) -> ReverbTelemetry {
        let [wl, wr] = wet;
        let n = input[0].len().min(wl.len()).min(if stereo {
            input[1].len().min(wr.len())
        } else {
            usize::MAX
        });
        self.freeze.set_target(if s.freeze { 1.0 } else { 0.0 });
        let fs = self.fs;
        let att = 1.0 - (-1.0 / (DUCK_ATTACK_MS * 0.001 * fs)).exp();
        let rel = 1.0 - (-1.0 / (s.duck_release_ms.max(1.0) * 0.001 * fs)).exp();
        let mut tel = ReverbTelemetry::default();
        for j in 0..n {
            if self.phase == CONTROL {
                self.phase = 0;
                self.control(s, false);
            }
            let t = self.phase as f32 / CONTROL as f32;
            self.phase += 1;
            let lerp = |a: [f32; 2]| a[0] + (a[1] - a[0]) * t;
            let xl = input[0][j];
            let xr = if stereo { input[1][j] } else { xl };
            let level = xl.abs().max(xr.abs());
            self.env += (level - self.env) * if level > self.env { att } else { rel };
            let feed = 1.0 - self.freeze.next_value();
            self.pre[0].write(xl);
            self.pre[1].write(xr);
            // The pre-delay's heads: one, or two crossfading (raised cosine).
            let (heads, fade) = match self.pd_fade {
                Some(f) => {
                    let next = f + self.pd_step;
                    if next >= 1.0 {
                        self.pd[0] = self.pd[1];
                        self.pd_fade = None;
                    } else {
                        self.pd_fade = Some(next);
                    }
                    (2, 0.5 - 0.5 * (std::f32::consts::PI * f).cos())
                }
                None => (1, 0.0),
            };
            let er = lerp(self.er);
            let (depth, diff_g, loop_g, early_lv, send, width) = (
                lerp(self.depth),
                lerp(self.diff_g),
                lerp(self.loop_g),
                lerp(self.early),
                lerp(self.send),
                lerp(self.width),
            );
            // The early field and the diffused input to the tank, per side.
            let mut early = [0.0f32; 2];
            let mut into = [0.0f32; 2];
            let mut plate_in = 0.0f32;
            let mut into_raw = [0.0f32; 2];
            let sides = if stereo { 2 } else { 1 };
            let needs_early =
                self.early[0].max(self.early[1]) > 0.0 || self.send[0].max(self.send[1]) > 0.0;
            for c in 0..sides {
                let mut x = 0.0;
                let mut e = 0.0;
                for h in 0..heads {
                    let w = if heads == 1 {
                        1.0
                    } else if h == 0 {
                        1.0 - fade
                    } else {
                        fade
                    };
                    let pd = self.pd[h];
                    if needs_early {
                        let mut acc = 0.0;
                        for k in 0..ER_TAPS {
                            let src = if stereo { self.taps.from[c][k] } else { 0 };
                            acc += self.taps.gain[c][k]
                                * self.pre[src].read_linear(pd + self.taps.at[c][k] * er);
                        }
                        e += w * acc;
                    }
                    x += w * self.pre[c].read(pd);
                }
                if needs_early {
                    let g = 0.25 + 0.4 * (diff_g / DIFF_MAX);
                    for a in &mut self.early_ap[c] {
                        e = a.process(e, g);
                    }
                }
                x *= feed;
                e *= feed;
                plate_in += x;
                into_raw[c] = x;
                for a in &mut self.diff[c] {
                    x = a.process(x, diff_g);
                }
                early[c] = e * early_lv;
                into[c] =
                    (x * (1.0 - 0.5 * send) + e * send) * IN_GAIN * (16.0 / LINES as f32).sqrt();
            }
            if !stereo {
                early[1] = early[0];
                into[1] = into[0];
                into_raw[1] = into_raw[0];
            }
            // A type change: the tank fades out, the new one starts silent and fades in.
            if self.kind != s.kind {
                self.kind_g -= self.kind_step;
                if self.kind_g <= 0.0 {
                    self.kind_g = 0.0;
                    self.kind = s.kind;
                    self.reset_tanks();
                    self.designed = None;
                }
            } else if self.kind_g < 1.0 {
                self.kind_g = (self.kind_g + self.kind_step).min(1.0);
            }
            let (ol, or) = match self.kind {
                ReverbKind::Plate => {
                    // MODULATION's default (30 %) is Dattorro's own excursion: his tank relies on it
                    // to smear the echo of its modulated allpasses (at a third of it the tail
                    // repeated at the 908-sample one, periodicity 0.16).
                    let m = depth / (MOD_MS * 0.001 * fs).max(1e-3) / 0.3;
                    // Fed the input before the room's diffusers (through them as well, the tail turned
                    // dense in 35 ms instead of 340 but peaky, 0.33 % against 0.044 %).
                    let p = self.plate_tank.process(plate_in / sides as f32, diff_g / DIFF_MAX, m, s.mod_rate, fs);
                    (p[0] * PLATE_OUT / OUT_GAIN, p[1] * PLATE_OUT / OUT_GAIN)
                }
                ReverbKind::Spring => {
                    let p = self.spring.process(into_raw);
                    (p[0] * SPRING_OUT / OUT_GAIN, p[1] * SPRING_OUT / OUT_GAIN)
                }
                _ => {
                    let (a, b) = self.network(&into, t, depth, loop_g);
                    let g = self.kind.voicing().gain;
                    (a * g, b * g)
                }
            };
            let (ol, or) = (ol * self.kind_g, or * self.kind_g);
            let mut l = ol * OUT_GAIN + early[0];
            let mut r = or * OUT_GAIN + early[1];
            // Shimmer: the plate beside the room, fed what the room is fed (before its diffusers).
            if self.shim[0] > 0.0 || self.shim[1] > 0.0 {
                if !self.plate_on {
                    self.plate.reset();
                    self.plate_on = true;
                }
                let level = lerp(self.shim);
                let p = self.plate.process(plate_in / sides as f32, &self.plate_s);
                l += p[0] * level;
                r += p[1] * level;
            } else {
                self.plate_on = false;
            }
            if stereo {
                let m = 0.5 * (l + r);
                let side = 0.5 * (l - r) * width;
                l = m + side;
                r = m - side;
            }
            // The ducking gain through a 3 ms one-pole, so its ramps have no corners.
            self.duck_g += (lerp(self.duck) - self.duck_g) * self.duck_k;
            let duck = self.duck_g;
            tel.duck_db = tel.duck_db.min(crate::gain_to_db(duck));
            wl[j] = l * duck;
            if stereo {
                wr[j] = r * duck;
            }
        }
        // The tone EQ: the same on both sides, so it commutes with the width.
        for (p, b) in self.tone.iter_mut().zip(&s.eq) {
            p.set_target(&BandSettings {
                placement: crate::eq::Placement::Stereo,
                dynamics: crate::eq::Dynamics::default(),
                ..*b
            });
            if stereo {
                p.process(&mut [&mut wl[..n], &mut wr[..n]]);
            } else {
                p.process(&mut [&mut wl[..n]]);
            }
        }
        tel.freeze = self.freeze.current();
        tel
    }
}

// ---- Plate and spring ----------------------------------------------------------------------------

/// The plate's size: Dattorro's delays times this at SIZE 0 and 100 %.
const PLATE_SCALE: (f32, f32) = (0.5, 1.5);
/// The plate's first decay diffusion: Dattorro's 0.70 rang at its modulated allpasses' length
/// (periodicity 0.09 with his modulation); 0.55 rang more (its first echo is stronger), 0.85
/// built slower; at 0.80, 0.064 (room: 0.028).
const PLATE_DD1: f32 = 0.80;
/// The plate's modulation at the most (a factor on Dattorro's excursion).
const PLATE_MOD_MAX: f32 = 2.0;

/// Jon Dattorro's plate (`crate::shimmer` has his constants), its damping and decay replaced by
/// the decay EQ's absorption designed for each half of the tank, so every type obeys the same
/// decay curve.
struct PlateTank {
    base: f32,
    k: f32,
    diff: [PAllpass; 4],
    ap1: [PAllpass; 2],
    d1: [PLine; 2],
    ap2: [PAllpass; 2],
    d2: [PLine; 2],
    filt: [LineFilter; 2],
    lfo: f32,
    cross: [f32; 2],
}

impl PlateTank {
    fn new(fs: f32) -> Self {
        let base = fs / DATTORRO_FS;
        let km = base * PLATE_SCALE.1;
        let exc = EXCURSION * km * PLATE_MOD_MAX;
        let mut t = Self {
            base,
            k: base,
            diff: IN_DIFF.map(|(len, _)| PAllpass::new(len * km, 0.0)),
            ap1: [PAllpass::new(PTANK[0][0] * km, exc), PAllpass::new(PTANK[1][0] * km, exc)],
            d1: [PLine::new((PTANK[0][1] * km) as usize + 2), PLine::new((PTANK[1][1] * km) as usize + 2)],
            ap2: [PAllpass::new(PTANK[0][2] * km, exc), PAllpass::new(PTANK[1][2] * km, exc)],
            d2: [PLine::new((PTANK[0][3] * km) as usize + 2), PLine::new((PTANK[1][3] * km) as usize + 2)],
            filt: [LineFilter::default(); 2],
            lfo: 0.0,
            cross: [0.0; 2],
        };
        t.size(0.5);
        t
    }

    fn reset(&mut self) {
        for a in self.diff.iter_mut().chain(self.ap1.iter_mut()).chain(self.ap2.iter_mut()) {
            a.line.reset();
        }
        for l in self.d1.iter_mut().chain(self.d2.iter_mut()) {
            l.reset();
        }
        for f in &mut self.filt {
            f.reset();
        }
        self.lfo = 0.0;
        self.cross = [0.0; 2];
    }

    /// Scales the delays for SIZE (0…1).
    fn size(&mut self, size: f32) {
        self.k = self.base * (PLATE_SCALE.0 + (PLATE_SCALE.1 - PLATE_SCALE.0) * size.clamp(0.0, 1.0));
        let k = self.k;
        for (a, &(len, _)) in self.diff.iter_mut().zip(&IN_DIFF) {
            a.len = len * k;
        }
        for ((a1, a2), lens) in self.ap1.iter_mut().zip(self.ap2.iter_mut()).zip(&PTANK) {
            a1.len = lens[0] * k;
            a2.len = lens[2] * k;
        }
    }

    /// A half of the tank, samples (what its absorption is designed for).
    fn half_len(&self, h: usize) -> f32 {
        PTANK[h].iter().sum::<f32>() * self.k
    }

    /// One (mono) sample in; left and right out. `diffusion` 0…1, `modulation` 0…~2.
    fn process(&mut self, x: f32, diffusion: f32, modulation: f32, rate: f32, fs: f32) -> [f32; 2] {
        let k = self.k;
        let gd = 0.5 + 0.5 * diffusion.clamp(0.0, 1.0);
        let mut u = x;
        for (a, &(_, g)) in self.diff.iter_mut().zip(&IN_DIFF) {
            u = a.process(u, g * gd, 0.0);
        }
        self.lfo = (self.lfo + rate.max(0.01) / fs).fract();
        let lfo = (std::f32::consts::TAU * self.lfo).sin();
        let exc = EXCURSION * k * modulation.clamp(0.0, PLATE_MOD_MAX);
        let mut out = [0.0f32; 2];
        for h in 0..2 {
            let v = u + self.cross[1 - h];
            let v = self.ap1[h].process(v, -PLATE_DD1, exc * if h == 0 { lfo } else { -lfo });
            self.d1[h].write(v);
            let t = self.filt[h].process(self.d1[h].at((PTANK[h][1] * k) as usize));
            let lfo2 = (std::f32::consts::TAU * (self.lfo + 0.25 + 0.37 * h as f32)).sin();
            let t = self.ap2[h].process(t, PDECAY_DIFF.1, 0.7 * exc * lfo2);
            self.d2[h].write(t);
            out[h] = self.d2[h].at((PTANK[h][3] * k) as usize);
        }
        self.cross = out;
        let node = |n: usize, d: f32| -> f32 {
            let d = (d * k) as usize;
            match n {
                0 => self.d1[0].at(d),
                1 => self.ap2[0].line.at(d),
                2 => self.d2[0].at(d),
                3 => self.d1[1].at(d),
                4 => self.ap2[1].line.at(d),
                _ => self.d2[1].at(d),
            }
        };
        let mut y = [0.0f32; 2];
        for (side, taps) in PTAPS.iter().enumerate() {
            y[side] = 0.6 * taps.iter().map(|&(n, d, sign)| sign * node(n, d)).sum::<f32>();
        }
        y
    }
}

/// The spring's dispersion: this many stretched allpasses, their coefficient (negative: the
/// highs arrive first and the lows after, the falling "pew" of a spring), and the frequency their
/// stretch is set by (the chirps live under it; a lowpass at it keeps what is over it out).
const SPRING_STAGES: usize = 40;
const SPRING_A: f32 = -0.62;
const SPRING_FC: f32 = 4400.0;
/// The two springs' delays (left, right), ms at SIZE 50 %, and the factor at SIZE 0 and 100 %.
const SPRING_MS: [f32; 2] = [38.0, 44.3];
const SPRING_SCALE: (f32, f32) = (0.6, 1.6);

/// A spring tank (after Välimäki, Parker & Abel, "Parametric Spring Reverberation Effect",
/// JAES 2010, simplified): per side, a loop of a cascade of stretched allpasses
/// (a + z⁻ᴷ)/(1 + a·z⁻ᴷ), a delay, a lowpass and the decay EQ's absorption.
struct SpringTank {
    fs: f32,
    k: usize,
    /// Per side: each stage's last K inputs and outputs (stage-major), the shared position.
    xs: [Vec<f32>; 2],
    ys: [Vec<f32>; 2],
    pos: usize,
    delay: [PLine; 2],
    len: [f32; 2],
    lp: [f32; 5],
    lp_z: [[f32; 2]; 2],
    dc: [f32; 2],
    filt: [LineFilter; 2],
    fb: [f32; 2],
}

impl SpringTank {
    fn new(fs: f32) -> Self {
        let k = ((fs / (2.0 * SPRING_FC)).round() as usize).max(1);
        let max = SPRING_MS.map(|ms| (ms * SPRING_SCALE.1 * 0.001 * fs) as usize + 4);
        // Butterworth lowpass at the chirps' top.
        let w = std::f32::consts::TAU * SPRING_FC / fs;
        let (sn, cs) = w.sin_cos();
        let alpha = sn / (2.0 * std::f32::consts::FRAC_1_SQRT_2);
        let a0 = 1.0 + alpha;
        let b = (1.0 - cs) / 2.0 / a0;
        let mut t = Self {
            fs,
            k,
            xs: [vec![0.0; SPRING_STAGES * k], vec![0.0; SPRING_STAGES * k]],
            ys: [vec![0.0; SPRING_STAGES * k], vec![0.0; SPRING_STAGES * k]],
            pos: 0,
            delay: [PLine::new(max[0]), PLine::new(max[1])],
            len: [0.0; 2],
            lp: [b, 2.0 * b, b, -2.0 * cs / a0, (1.0 - alpha) / a0],
            lp_z: [[0.0; 2]; 2],
            dc: [0.0; 2],
            filt: [LineFilter::default(); 2],
            fb: [0.0; 2],
        };
        t.size(0.5);
        t
    }

    fn reset(&mut self) {
        for v in self.xs.iter_mut().chain(self.ys.iter_mut()) {
            v.fill(0.0);
        }
        self.pos = 0;
        for d in &mut self.delay {
            d.reset();
        }
        self.lp_z = [[0.0; 2]; 2];
        self.dc = [0.0; 2];
        for f in &mut self.filt {
            f.reset();
        }
        self.fb = [0.0; 2];
    }

    fn size(&mut self, size: f32) {
        let f = SPRING_SCALE.0 + (SPRING_SCALE.1 - SPRING_SCALE.0) * size.clamp(0.0, 1.0);
        self.len = SPRING_MS.map(|ms| ms * f * 0.001 * self.fs);
    }

    /// A side's loop, samples: its delay and the cascade's group delay at 1.5 kHz
    /// (K(1 − a²)/(1 + 2a·cos Kω + a²) a stage). Measured: at 500 Hz the T60 came out 15 % short,
    /// with no cascade delay at all 10 % long.
    fn loop_len(&self, c: usize) -> f32 {
        let (a, k) = (SPRING_A, self.k as f32);
        let kw = k * std::f32::consts::TAU * 1500.0 / self.fs;
        let stage = k * (1.0 - a * a) / (1.0 + 2.0 * a * kw.cos() + a * a);
        self.len[c] + SPRING_STAGES as f32 * stage
    }

    fn process(&mut self, x: [f32; 2]) -> [f32; 2] {
        let (k, pos) = (self.k, self.pos);
        let [b0, b1, b2, a1, a2] = self.lp;
        let dc_k = 1.0 - (-std::f32::consts::TAU * 40.0 / self.fs).exp();
        let mut out = [0.0f32; 2];
        for c in 0..2 {
            let mut v = x[c] + self.fb[c];
            let (xs, ys) = (&mut self.xs[c], &mut self.ys[c]);
            for st in 0..SPRING_STAGES {
                let at = st * k + pos;
                let y = SPRING_A * v + xs[at] - SPRING_A * ys[at];
                xs[at] = v;
                ys[at] = y;
                v = y;
            }
            self.delay[c].write(v);
            let mut y = self.delay[c].at(self.len[c] as usize);
            let z = &mut self.lp_z[c];
            let o = b0 * y + z[0];
            z[0] = b1 * y - a1 * o + z[1];
            z[1] = b2 * y - a2 * o;
            y = o;
            self.dc[c] += (y - self.dc[c]) * dc_k;
            y -= self.dc[c];
            y = self.filt[c].process(y);
            self.fb[c] = y;
            out[c] = y;
        }
        self.pos = (pos + 1) % k;
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hadamard_is_orthonormal() {
        for k in 0..LINES {
            let mut v = [0.0f32; LINES];
            v[k] = 1.0;
            hadamard(&mut v);
            let e: f32 = v.iter().map(|x| x * x).sum();
            assert!((e - 1.0).abs() < 1e-6);
        }
        let topo = Topology::new();
        let vs = topo.out;
        for (a, x) in vs.iter().enumerate() {
            for (b, y) in vs.iter().enumerate() {
                let dot: f32 = x.iter().zip(y).map(|(p, q)| p * q).sum();
                assert!(
                    (dot - if a == b { 1.0 } else { 0.0 }).abs() < 0.01,
                    "{a}·{b} = {dot}"
                );
            }
        }
        let mut seen = [false; LINES];
        assert!(topo.main_ms.windows(2).all(|w| w[1] > w[0]));
        topo.mix_perm.iter().for_each(|&p| seen[p] = true);
        assert!(seen.iter().all(|&s| s));
    }

    #[test]
    fn lagrange_read_is_exact_on_whole_delays() {
        let mut d = Delay::new(64);
        for i in 0..40 {
            d.write(i as f32);
        }
        assert_eq!(d.read(5.0), 34.0);
        // Linear ramps are interpolated exactly.
        assert!((d.read(5.25) - 33.75).abs() < 1e-4);
        assert!((d.read_linear(5.25) - 33.75).abs() < 1e-4);
    }

    #[test]
    fn decay_at_matches_the_settings_at_the_ends_and_at_a_bell() {
        let fs = 48000.0;
        let s = ReverbSettings {
            decay_s: 3.0,
            ..ReverbSettings::default()
        }
        .with_shelves((250.0, 2.0), (5000.0, 0.4));
        assert!((decay_at(&s, 0.0, fs) - 6.0).abs() < 0.01);
        assert!((decay_at(&s, 24000.0, fs) - 1.2).abs() < 0.01);
        let mid = decay_at(&s, 1500.0, fs);
        assert!((mid - 3.0).abs() < 0.3, "{mid}");
        // A bell at 1.5 kHz halving the decay there.
        let mut b = s;
        b.bands[2] = DecayBand::bell(1500.0, 0.5, 1.0);
        let at = decay_at(&b, 1500.0, fs);
        assert!(
            (at / (mid * 0.5) - 1.0).abs() < 0.02,
            "{at} against {}",
            mid * 0.5
        );
    }

    #[test]
    fn the_decay_model_follows_the_lines_mean() {
        let fs = 48000.0;
        let mut s = ReverbSettings {
            decay_s: 2.5,
            ..ReverbSettings::default()
        }
        .with_shelves((200.0, 1.8), (4000.0, 0.3));
        s.bands[2] = DecayBand::bell(800.0, 0.4, 3.0);
        s.bands[3] = DecayBand {
            shape: DecayShape::Tilt,
            ..DecayBand::bell(2000.0, 1.5, 1.0)
        };
        for size in [0.0, 0.5, 1.0] {
            s.size = size;
            let m = DecayModel::new(&s, fs);
            for f in [30.0, 200.0, 800.0, 2000.0, 6000.0, 18000.0] {
                let (exact, model) = (decay_at(&s, f, fs), m.at(f));
                assert!(
                    (model / exact - 1.0).abs() < 0.03,
                    "size {size}, {f} Hz: {model} against {exact}"
                );
            }
        }
    }

    #[test]
    fn wander_is_smooth_and_bounded() {
        let mut w = Wander::new(7);
        w.step = 1.0 / 4800.0;
        let mut last = w.next();
        for _ in 0..200_000 {
            let v = w.next();
            assert!((-1.0..=1.0).contains(&v));
            assert!((v - last).abs() < 0.002);
            last = v;
        }
    }
}

#[cfg(test)]
mod shimmer_tests {
    use super::*;

    const FS: f32 = 48_000.0;

    /// The left wet output of `x` (mono in, stereo out) at `s`.
    fn render(s: &ReverbSettings, x: &[f32]) -> Vec<f32> {
        let mut r = Reverb::new(FS, s);
        let (mut l, mut rr) = (vec![0.0; x.len()], vec![0.0; x.len()]);
        for (i, ch) in x.chunks(256).enumerate() {
            let (a, b) = (i * 256, i * 256 + ch.len());
            r.process([ch, ch], [&mut l[a..b], &mut rr[a..b]], true, s);
        }
        l
    }

    fn tone(f: f32, len: usize, on: usize) -> Vec<f32> {
        (0..len).map(|i| if i < on { 0.3 * (std::f32::consts::TAU * f * i as f32 / FS).sin() } else { 0.0 }).collect()
    }

    fn level(v: &[f32], f: f32) -> f32 {
        let (mut c, mut s) = (0.0f64, 0.0f64);
        for (i, &x) in v.iter().enumerate() {
            let w = std::f64::consts::TAU * f64::from(f) * i as f64 / f64::from(FS);
            c += f64::from(x) * w.cos();
            s += f64::from(x) * w.sin();
        }
        (2.0 * (c * c + s * s).sqrt() / v.len() as f64) as f32
    }

    fn peak(v: &[f32]) -> f32 {
        v.iter().fold(0.0, |m, x| m.max(x.abs()))
    }

    #[test]
    fn shimmer_puts_the_interval_in_the_tail() {
        let x = tone(440.0, 4 * 48_000, 24_000);
        for (pitch, f) in [(12.0, 880.0), (7.0, 659.26), (-12.0, 220.0)] {
            let dry = render(&ReverbSettings { decay_s: 6.0, ..ReverbSettings::default() }, &x);
            let shim = render(&ReverbSettings { decay_s: 6.0, shimmer: 0.6, shimmer_pitch: pitch, ..ReverbSettings::default() }, &x);
            let tail = 48_000..96_000;
            let gain = 20.0 * (level(&shim[tail.clone()], f) / level(&dry[tail], f)).log10();
            assert!(gain > 15.0, "{pitch}: {gain:.1} dB");
        }
    }

    #[test]
    fn shimmer_never_runs_away_even_frozen_on_a_held_tone() {
        for pitch in [0.0, 0.3, 7.0, 12.0, -12.0, 24.0] {
            let x = tone(220.0, 20 * 48_000, 20 * 48_000);
            let s = ReverbSettings { decay_s: 30.0, shimmer: 1.0, shimmer_pitch: pitch, ..ReverbSettings::default() };
            let y = render(&s, &x);
            let early = peak(&y[48_000..3 * 48_000]);
            let late = peak(&y[17 * 48_000..]);
            assert!(late < 4.0 * early && late < 4.0, "{pitch}: {early} → {late}");
            // Frozen with the tone gone.
            let mut r = Reverb::new(FS, &s);
            let (mut l, mut rr) = (vec![0.0; 256], vec![0.0; 256]);
            let mut held = 0.0f32;
            for b in 0..(16 * 48_000 / 256) {
                let frozen = b >= 2 * 48_000 / 256;
                let s = ReverbSettings { freeze: frozen, ..s };
                let input = if frozen { vec![0.0; 256] } else { tone(220.0, 256, 256) };
                r.process([&input, &input], [&mut l, &mut rr], true, &s);
                if b == 3 * 48_000 / 256 {
                    held = peak(&l);
                }
                if b > 14 * 48_000 / 256 {
                    assert!(peak(&l) < 4.0 * held.max(1e-3), "{pitch} frozen: {held} → {}", peak(&l));
                }
            }
        }
    }

    #[test]
    fn no_shimmer_is_the_reverb_as_it_was() {
        let x = tone(330.0, 48_000, 4800);
        let a = render(&ReverbSettings::default(), &x);
        let b = render(&ReverbSettings { shimmer: 0.0, shimmer_pitch: -7.0, ..ReverbSettings::default() }, &x);
        assert_eq!(a, b);
    }
}

#[cfg(test)]
mod types_tests {
    use super::*;

    const FS: f32 = 48_000.0;

    fn ir(s: &ReverbSettings, seconds: f32) -> Vec<f32> {
        let n = (FS * seconds) as usize;
        let mut r = Reverb::new(FS, s);
        let mut x = vec![0.0f32; n];
        x[0] = 1.0;
        let (mut l, mut rr) = (vec![0.0f32; n], vec![0.0f32; n]);
        for ((a, b), c) in l.chunks_mut(512).zip(rr.chunks_mut(512)).zip(x.chunks(512)) {
            r.process([c, c], [a, b], true, s);
        }
        l
    }

    /// T60 from the energy decay (Schroeder), its −5…−35 dB slope.
    fn t60(v: &[f32]) -> f32 {
        let mut e: Vec<f64> = v.iter().map(|x| f64::from(*x).powi(2)).collect();
        for i in (0..e.len() - 1).rev() {
            e[i] += e[i + 1];
        }
        let at = |lvl: f64| e.iter().position(|&d| 10.0 * (d / e[0]).max(1e-30).log10() < lvl).unwrap_or(e.len()) as f32 / FS;
        2.0 * (at(-35.0) - at(-5.0))
    }

    #[test]
    fn every_type_rings_for_the_decay_at_the_rooms_level() {
        let room: f64 = ir(&ReverbSettings { early: 0.0, ..ReverbSettings::default() }, 5.0).iter().map(|x| f64::from(*x).powi(2)).sum();
        for k in 0..ReverbKind::NAMES.len() {
            let kind = ReverbKind::from_index(k);
            let s = ReverbSettings { kind, early: 0.0, ..ReverbSettings::default() };
            let y = ir(&s, 5.0);
            let t = t60(&y);
            assert!((t - 2.0).abs() < 0.15, "{kind:?}: T60 {t:.2} s");
            let e: f64 = y.iter().map(|x| f64::from(*x).powi(2)).sum();
            let db = 10.0 * (e / room).log10();
            assert!(db.abs() < 1.5, "{kind:?}: {db:+.1} dB against the room");
        }
    }

    #[test]
    fn a_spring_chirps_highs_first() {
        let y = ir(&ReverbSettings { kind: ReverbKind::Spring, early: 0.0, ..ReverbSettings::default() }, 0.2);
        // When the first pass's energy arrives in a band (its centroid in time over 0…80 ms, the
        // second pass coming at about 85).
        let arrival = |f: f64| {
            let (mut num, mut den) = (0.0, 0.0);
            let n = (0.08 * FS) as usize;
            let w = std::f64::consts::TAU * f / f64::from(FS);
            // A narrow band: the signal heterodyned to DC and smoothed over 2 ms.
            let (mut c, mut s) = (0.0f64, 0.0f64);
            let k = 1.0 / (0.002 * f64::from(FS));
            for (i, &x) in y.iter().take(n).enumerate() {
                c += (f64::from(x) * (w * i as f64).cos() - c) * k;
                s += (f64::from(x) * (w * i as f64).sin() - s) * k;
                let e = c * c + s * s;
                num += e * i as f64;
                den += e;
            }
            num / den / f64::from(FS) * 1000.0
        };
        let (hi, lo) = (arrival(3500.0), arrival(400.0));
        assert!(hi + 3.0 < lo, "3.5 kHz at {hi:.1} ms, 400 Hz at {lo:.1} ms");
    }

    #[test]
    fn switching_type_fades_without_a_jump() {
        let s0 = ReverbSettings::default();
        let mut r = Reverb::new(FS, &s0);
        let mut prev = 0.0f32;
        let mut worst = 0.0f32;
        let mut seed = 1u32;
        for b in 0..400 {
            let kind = ReverbKind::from_index((b / 80) % 5);
            let s = ReverbSettings { kind, ..s0 };
            let x: Vec<f32> = (0..128).map(|_| { seed ^= seed << 13; seed ^= seed >> 17; seed ^= seed << 5; (seed as f32 / u32::MAX as f32 - 0.5) * 0.2 }).collect();
            let (mut l, mut rr) = (vec![0.0; 128], vec![0.0; 128]);
            r.process([&x, &x], [&mut l, &mut rr], true, &s);
            for &v in &l {
                worst = worst.max((v - prev).abs());
                prev = v;
            }
        }
        // A noise-fed tail moves sample to sample by well under this; a hard switch would jump.
        assert!(worst < 0.5, "largest step {worst}");
    }
}
