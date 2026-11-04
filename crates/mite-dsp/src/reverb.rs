//! Mite Verb's algorithm (docs/design/verb-brief.md §1): pre-delay, early reflections off the
//! pre-delay line, input diffusion (Schroeder allpasses), a modulated 16-line feedback delay
//! network under a Hadamard matrix with per-line absorption (decay time per frequency), then a
//! low and a high cut, width and ducking on the reverb. This produces the reverb only; the plugin
//! mixes it with the dry signal.
//!
//! Everything is sized in [`Reverb::new`]; `process` and `reset` never allocate. Settings glide
//! (20 ms; Size over 80 ms) and the absorption is designed every [`CONTROL`] samples counted
//! across blocks, so the output is the same for every block size.

use crate::filter::Coeffs;
use crate::smooth::LinearSmoother;

pub const LINES: usize = 16;
pub const MAX_PREDELAY_MS: f32 = 500.0;
/// Samples per control step (designs, glides, the ducking's target).
pub const CONTROL: usize = 32;
/// The lines' lengths at the largest size, ms: spread over 1 : 2.7, none a simple ratio of another.
const BASE_MS: [f32; LINES] = [61.3, 67.9, 73.1, 79.7, 86.3, 91.9, 98.7, 104.3, 111.1, 118.9, 125.3, 131.9, 139.7, 147.1, 153.7, 163.3];
/// The smallest size's scale on [`BASE_MS`] (4…11 ms lines: a small room).
const MIN_SCALE: f32 = 0.07;
/// Modulation depth at MOD 100 %, ms (less in small rooms: at most a tenth of the line).
const MOD_MS: f32 = 1.2;
/// The input diffusers' lengths per side, ms (not scaled with Size: they never glide).
const DIFF_MS: [[f32; 4]; 2] = [[1.31, 2.07, 3.71, 5.29], [1.49, 2.33, 3.29, 4.87]];
/// The largest diffusion coefficient (DIFFUSION 100 %).
const DIFF_MAX: f32 = 0.75;
/// Early reflections at the largest size, ms, per side.
const ER_MS: [[f32; 10]; 2] = [[7.1, 11.3, 17.9, 23.3, 31.7, 41.9, 53.1, 61.7, 71.3, 83.9], [8.3, 13.7, 19.1, 27.1, 33.7, 44.9, 49.3, 66.1, 76.9, 89.3]];
/// The reflections' scale at the smallest size (they shrink less than the tank).
const ER_MIN_SCALE: f32 = 0.2;
const ER_GAIN: [f32; 10] = [0.62, -0.52, 0.46, -0.40, 0.34, -0.28, 0.24, -0.19, 0.15, -0.11];
/// The tank's input and output gains (a 2 s decay at Size 50 % comes out about as loud as it went in).
const IN_GAIN: f32 = 0.35;
const OUT_GAIN: f32 = 0.32;
const GLIDE_MS: f32 = 20.0;
const SIZE_GLIDE_MS: f32 = 200.0;
/// A pre-delay change crossfades between two read heads over this long (a glide would be a
/// pitch sweep: 500 ms in 20 ms is 25 times the pitch).
const PREDELAY_FADE_MS: f32 = 30.0;
const FREEZE_MS: f32 = 50.0;
const DUCK_ATTACK_MS: f32 = 2.0;

/// Signs: the left input feeds the even lines, the right the odd ones, each with these signs; the
/// outputs are two orthogonal patterns over all lines (rows of a 16-point Hadamard matrix).
const IN_SIGN: [f32; LINES] = [1.0, 1.0, -1.0, 1.0, 1.0, -1.0, -1.0, -1.0, 1.0, -1.0, 1.0, 1.0, -1.0, 1.0, -1.0, -1.0];
const OUT_L: [f32; LINES] = hadamard_row(5);
const OUT_R: [f32; LINES] = hadamard_row(10);

const fn hadamard_row(r: usize) -> [f32; LINES] {
    let mut row = [0.0; LINES];
    let mut i = 0;
    while i < LINES {
        // Sylvester's construction: H[r][i] = (−1)^popcount(r & i).
        row[i] = if (r & i).count_ones() % 2 == 0 { 1.0 } else { -1.0 };
        i += 1;
    }
    row
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ReverbSettings {
    pub predelay_ms: f32,
    /// 0…1.
    pub size: f32,
    /// T60 at mid frequencies, s.
    pub decay_s: f32,
    /// T60 at DC against `decay_s`.
    pub lo_mult: f32,
    pub lo_freq: f32,
    /// T60 at Nyquist against `decay_s`.
    pub hi_mult: f32,
    pub hi_freq: f32,
    /// 0…1.
    pub diffusion: f32,
    /// Early reflections' level, 0…1.
    pub early: f32,
    /// 0…1.
    pub mod_depth: f32,
    pub mod_rate: f32,
    /// 0…1.5 (1: as the tank makes it).
    pub width: f32,
    pub lo_cut: f32,
    pub hi_cut: f32,
    pub freeze: bool,
    /// Ducking depth, dB (≥ 0; 0: off).
    pub duck_db: f32,
    pub duck_thresh_db: f32,
    pub duck_release_ms: f32,
}

impl Default for ReverbSettings {
    fn default() -> Self {
        Self {
            predelay_ms: 10.0,
            size: 0.5,
            decay_s: 2.0,
            lo_mult: 1.2,
            lo_freq: 250.0,
            hi_mult: 0.5,
            hi_freq: 5000.0,
            diffusion: 0.7,
            early: 0.5,
            mod_depth: 0.3,
            mod_rate: 0.6,
            width: 1.0,
            lo_cut: 20.0,
            hi_cut: 20_000.0,
            freeze: false,
            duck_db: 0.0,
            duck_thresh_db: -30.0,
            duck_release_ms: 300.0,
        }
    }
}

/// Size's scale on the lines' lengths.
pub fn tank_scale(size: f32) -> f32 {
    MIN_SCALE * (1.0 / MIN_SCALE).powf(size.clamp(0.0, 1.0))
}

fn er_scale(size: f32) -> f32 {
    ER_MIN_SCALE * (1.0 / ER_MIN_SCALE).powf(size.clamp(0.0, 1.0))
}

/// Line `i`'s length at `size`, samples.
pub fn line_len(i: usize, size: f32, fs: f32) -> f32 {
    BASE_MS[i] * tank_scale(size) * 0.001 * fs
}

/// A second-order shelf (RBJ, slope 1: monotone, no overshoot), its transition centred (in dB)
/// on `fc`: `gain` at DC (`low`) or at Nyquist (high), 1 at the other end.
pub fn shelf(low: bool, gain: f64, fc: f64, fs: f64) -> Coeffs {
    if (gain - 1.0).abs() < 1e-12 {
        return Coeffs::IDENTITY;
    }
    let a = gain.sqrt();
    let w = std::f64::consts::TAU * (fc / fs).clamp(1e-5, 0.49);
    let (sin, cos) = w.sin_cos();
    let k = 2.0 * a.sqrt() * sin * 0.5 * std::f64::consts::SQRT_2;
    let (ap, am) = (a + 1.0, a - 1.0);
    let (b0, b1, b2, a0, a1, a2) = if low {
        (a * (ap - am * cos + k), 2.0 * a * (am - ap * cos), a * (ap - am * cos - k), ap + am * cos + k, -2.0 * (am + ap * cos), ap + am * cos - k)
    } else {
        (a * (ap + am * cos + k), -2.0 * a * (am + ap * cos), a * (ap + am * cos - k), ap - am * cos + k, 2.0 * (am - ap * cos), ap - am * cos - k)
    };
    Coeffs { b0: b0 / a0, b1: b1 / a0, b2: b2 / a0, a1: a1 / a0, a2: a2 / a0 }
}

/// One line's absorption: a gain and two shelves.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Absorption {
    pub gain: f64,
    pub lo: Coeffs,
    pub hi: Coeffs,
}

impl Absorption {
    /// The loop's gain through this absorption at `w`.
    pub fn magnitude(&self, w: f64) -> f64 {
        self.gain * (self.lo.magnitude_sq(w) * self.hi.magnitude_sq(w)).sqrt()
    }
}

/// The grid the loop gain is checked on: DC, 32 log-spaced points, Nyquist.
fn check_grid(i: usize) -> f64 {
    match i {
        0 => 0.0,
        33 => std::f64::consts::PI,
        _ => std::f64::consts::PI * 10f64.powf(-3.5 + 3.5 * (i - 1) as f64 / 31.0),
    }
}

/// The absorption for a line of `len` samples: a pass loses −60 dB × len / (T60(f)·fs), with
/// T60 the decay at mid, × `lo_mult` at DC and × `hi_mult` at Nyquist. `freeze` (0…1) fades every
/// loss to nothing. The loop gain is kept under 1 at every frequency (checked on a grid).
pub fn design_absorption(len: f64, s: &ReverbSettings, fs: f64, freeze: f64) -> Absorption {
    let keep = (1.0 - freeze).clamp(0.0, 1.0);
    let t = f64::from(s.decay_s.max(0.01));
    // At most 100 dB a pass: a decay shorter than a pass round the loop is gone after one anyway.
    let per_pass = |t60: f64| (-60.0 * len / (fs * t60.max(0.01))).max(-100.0) * keep;
    let (mid, lo, hi) = (per_pass(t), per_pass(t * f64::from(s.lo_mult)), per_pass(t * f64::from(s.hi_mult)));
    let db = |d: f64| 10f64.powf(d / 20.0);
    let mut a = Absorption { gain: db(mid), lo: shelf(true, db(lo - mid), f64::from(s.lo_freq), fs), hi: shelf(false, db(hi - mid), f64::from(s.hi_freq), fs) };
    if keep > 0.0 {
        // The grid's highest point, then refined between its neighbours (ternary search).
        let best = (0..34).max_by(|&i, &j| a.magnitude(check_grid(i)).total_cmp(&a.magnitude(check_grid(j)))).unwrap_or(0);
        let (mut lo, mut hi) = (check_grid(best.saturating_sub(1)), check_grid((best + 1).min(33)));
        for _ in 0..40 {
            let (m1, m2) = (lo + (hi - lo) / 3.0, hi - (hi - lo) / 3.0);
            if a.magnitude(m1) < a.magnitude(m2) {
                lo = m1;
            } else {
                hi = m2;
            }
        }
        let peak = a.magnitude(check_grid(best)).max(a.magnitude(0.5 * (lo + hi)));
        let limit = 1.0 - 1e-6;
        if peak > limit {
            a.gain *= limit / peak;
        }
    }
    a
}

/// The decay time (T60, s) the lines give at `freq`: the mean over the lines of what their
/// absorption, as designed, loses per second there. What the editor draws.
pub fn decay_at(s: &ReverbSettings, freq: f64, fs: f64) -> f64 {
    let w = std::f64::consts::TAU * (freq / fs).clamp(0.0, 0.5);
    let mut sum = 0.0;
    for i in 0..LINES {
        let len = f64::from(line_len(i, s.size, fs as f32));
        let a = design_absorption(len, s, fs, 0.0);
        let per_pass_db = 20.0 * a.magnitude(w).log10();
        sum += if per_pass_db < -1e-9 { -60.0 * len / fs / per_pass_db } else { 1e6 };
    }
    sum / LINES as f64
}

/// [`decay_at`] at many frequencies at once (each line designed once): `out[k]` for `freqs[k]`.
pub fn decay_curve(s: &ReverbSettings, freqs: &[f64], fs: f64, out: &mut [f64]) {
    out.iter_mut().for_each(|v| *v = 0.0);
    for i in 0..LINES {
        let len = f64::from(line_len(i, s.size, fs as f32));
        let a = design_absorption(len, s, fs, 0.0);
        for (o, &f) in out.iter_mut().zip(freqs) {
            let w = std::f64::consts::TAU * (f / fs).clamp(0.0, 0.5);
            let per_pass_db = 20.0 * a.magnitude(w).log10();
            *o += if per_pass_db < -1e-9 { -60.0 * len / fs / per_pass_db } else { 1e6 };
        }
    }
    out.iter_mut().for_each(|v| *v /= LINES as f64);
}

/// A delay line with a power-of-two buffer.
#[derive(Debug, Clone)]
struct Delay {
    buf: Vec<f32>,
    mask: usize,
    pos: usize,
}

impl Delay {
    fn new(max_delay: usize) -> Self {
        let len = (max_delay + 4).next_power_of_two();
        Self { buf: vec![0.0; len], mask: len - 1, pos: 0 }
    }

    fn reset(&mut self) {
        self.buf.iter_mut().for_each(|x| *x = 0.0);
        self.pos = 0;
    }

    #[inline]
    fn write(&mut self, x: f32) {
        self.buf[self.pos] = x;
        self.pos = (self.pos + 1) & self.mask;
    }

    /// The sample written `d` samples ago (0: the last one).
    #[inline]
    fn tap(&self, d: usize) -> f32 {
        self.buf[(self.pos.wrapping_sub(1 + d)) & self.mask]
    }

    /// The signal `d` (≥ 1) samples ago, cubic Lagrange through the four samples around it (the
    /// fraction always between the middle two, where it is passive).
    #[inline]
    fn read(&self, d: f32) -> f32 {
        let d = d.max(1.0);
        let k = d as usize;
        let f = d - k as f32;
        let (x0, x1, x2, x3) = (self.tap(k - 1), self.tap(k), self.tap(k + 1), self.tap(k + 2));
        // Delay relative to x0: D = 1 + f.
        let dd = 1.0 + f;
        let (dm1, dm2, dm3) = (dd - 1.0, dd - 2.0, dd - 3.0);
        let h0 = -dm1 * dm2 * dm3 / 6.0;
        let h1 = dd * dm2 * dm3 * 0.5;
        let h2 = -dd * dm1 * dm3 * 0.5;
        let h3 = dd * dm1 * dm2 / 6.0;
        x0 * h0 + x1 * h1 + x2 * h2 + x3 * h3
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
struct Allpass {
    line: Delay,
    len: usize,
}

impl Allpass {
    fn new(len: usize) -> Self {
        Self { line: Delay::new(len), len: len.max(1) }
    }

    #[inline]
    fn process(&mut self, x: f32, g: f32) -> f32 {
        let delayed = self.line.tap(self.len - 1);
        let w = x + g * delayed;
        self.line.write(w);
        delayed - g * w
    }
}

/// A second-order section, transposed direct form II.
#[derive(Debug, Clone, Copy, Default)]
struct Biquad {
    b: [f32; 3],
    a: [f32; 2],
    s: [f32; 2],
}

impl Biquad {
    /// RBJ Butterworth high-pass (`high`) or low-pass at `fc`.
    fn set(&mut self, high: bool, fc: f32, fs: f32) {
        let w = std::f32::consts::TAU * (fc / fs).clamp(1e-5, 0.49);
        let (sin, cos) = w.sin_cos();
        let alpha = sin * std::f32::consts::FRAC_1_SQRT_2;
        let a0 = 1.0 + alpha;
        let (b0, b1) = if high { ((1.0 + cos) * 0.5, -(1.0 + cos)) } else { ((1.0 - cos) * 0.5, 1.0 - cos) };
        self.b = [b0 / a0, b1 / a0, b0 / a0];
        self.a = [-2.0 * cos / a0, (1.0 - alpha) / a0];
    }

    #[inline]
    fn process(&mut self, x: f32) -> f32 {
        let y = self.b[0] * x + self.s[0];
        self.s[0] = self.b[1] * x - self.a[0] * y + self.s[1];
        self.s[1] = self.b[2] * x - self.a[1] * y;
        y
    }

    fn reset(&mut self) {
        self.s = [0.0; 2];
    }
}

/// A line's absorption in the audio path: two biquads (f32, transposed direct form II) and a gain.
#[derive(Debug, Clone, Copy, Default)]
struct LineFilter {
    gain: f32,
    c: [[f32; 5]; 2],
    z: [[f32; 2]; 2],
}

impl LineFilter {
    fn set(&mut self, a: &Absorption) {
        self.gain = a.gain as f32;
        for (c, k) in self.c.iter_mut().zip([&a.lo, &a.hi]) {
            *c = [k.b0 as f32, k.b1 as f32, k.b2 as f32, k.a1 as f32, k.a2 as f32];
        }
    }

    #[inline]
    fn process(&mut self, mut x: f32) -> f32 {
        for (c, z) in self.c.iter().zip(self.z.iter_mut()) {
            let y = c[0] * x + z[0];
            z[0] = c[1] * x - c[3] * y + z[1];
            z[1] = c[2] * x - c[4] * y;
            x = y;
        }
        x * self.gain
    }

    fn reset(&mut self) {
        self.z = [[0.0; 2]; 2];
    }
}

/// In-place 16-point fast Walsh–Hadamard transform, scaled to be orthonormal.
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
    for x in v.iter_mut() {
        *x *= 0.25;
    }
}

/// The settings that glide, in one array.
const SMOOTHED: usize = 16;

fn pack(s: &ReverbSettings) -> [f32; SMOOTHED] {
    [s.predelay_ms, s.size, s.decay_s, s.lo_mult, s.lo_freq, s.hi_mult, s.hi_freq, s.diffusion, s.early, s.mod_depth, s.mod_rate, s.width, s.lo_cut, s.hi_cut, s.duck_thresh_db, s.duck_db]
}

fn unpack(v: &[f32; SMOOTHED], s: &ReverbSettings) -> ReverbSettings {
    ReverbSettings {
        predelay_ms: v[0],
        size: v[1],
        decay_s: v[2],
        lo_mult: v[3],
        lo_freq: v[4],
        hi_mult: v[5],
        hi_freq: v[6],
        diffusion: v[7],
        early: v[8],
        mod_depth: v[9],
        mod_rate: v[10],
        width: v[11],
        lo_cut: v[12],
        hi_cut: v[13],
        duck_thresh_db: v[14],
        duck_db: v[15],
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
        Self { duck_db: 0.0, freeze: 0.0 }
    }
}

pub struct Reverb {
    fs: f32,
    pre: [Delay; 2],
    diff: [[Allpass; 4]; 2],
    lines: [Delay; LINES],
    reads: [AllpassRead; LINES],
    filters: [LineFilter; LINES],
    cuts: [[Biquad; 2]; 2],
    smooth: [LinearSmoother; SMOOTHED],
    freeze: LinearSmoother,
    /// The pre-delay's two read heads (samples) and the crossfade between them (0: all on the
    /// first; `None`: not fading).
    pd: [f32; 2],
    pd_fade: Option<f32>,
    pd_step: f32,
    /// The current control step's start and end: the reflections' scale, each line's length
    /// (samples), the ducking gain.
    er: [f32; 2],
    len: [[f32; LINES]; 2],
    duck: [f32; 2],
    /// Modulation oscillators (cos, sin) and their rotation per sample.
    osc: [[f32; 2]; LINES],
    rot: [[f32; 2]; LINES],
    depth: f32,
    diff_g: f32,
    early: f32,
    width: f32,
    env: f32,
    duck_g: f32,
    duck_k: f32,
    phase: usize,
    designed: Option<(ReverbSettings, f32)>,
}

impl Reverb {
    /// Sizes every buffer for `fs`. Allocates.
    pub fn new(fs: f32, s: &ReverbSettings) -> Self {
        let ms = |m: f32| (m * 0.001 * fs).ceil() as usize;
        let max_er = ER_MS.iter().flatten().fold(0.0f32, |a, &b| a.max(b));
        let mut r = Self {
            fs,
            pre: std::array::from_fn(|_| Delay::new(ms(MAX_PREDELAY_MS + max_er) + 8)),
            diff: std::array::from_fn(|c| std::array::from_fn(|k| Allpass::new(ms(DIFF_MS[c][k])))),
            lines: std::array::from_fn(|i| Delay::new(ms(BASE_MS[i] + MOD_MS) + 8)),
            reads: [AllpassRead::default(); LINES],
            filters: [LineFilter::default(); LINES],
            cuts: [[Biquad::default(); 2]; 2],
            smooth: std::array::from_fn(|_| LinearSmoother::new(0.0)),
            freeze: LinearSmoother::new(0.0),
            pd: [0.0; 2],
            pd_fade: None,
            pd_step: 1.0 / (PREDELAY_FADE_MS * 0.001 * fs),
            er: [0.0; 2],
            len: [[0.0; LINES]; 2],
            duck: [1.0; 2],
            osc: [[1.0, 0.0]; LINES],
            rot: [[1.0, 0.0]; LINES],
            depth: 0.0,
            diff_g: 0.0,
            early: 0.0,
            width: 1.0,
            env: 0.0,
            duck_g: 1.0,
            duck_k: 1.0 - (-1.0 / (0.003 * fs)).exp(),
            phase: 0,
            designed: None,
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
        for a in self.diff.iter_mut().flatten() {
            a.line.reset();
        }
        for f in &mut self.filters {
            f.reset();
        }
        self.reads = [AllpassRead::default(); LINES];
        for b in self.cuts.iter_mut().flatten() {
            b.reset();
        }
        for (sm, v) in self.smooth.iter_mut().zip(pack(s)) {
            sm.reset(v);
        }
        self.freeze.reset(if s.freeze { 1.0 } else { 0.0 });
        for (i, o) in self.osc.iter_mut().enumerate() {
            let p = std::f32::consts::TAU * i as f32 / LINES as f32 * 0.618_034 * 7.0;
            *o = [p.cos(), p.sin()];
        }
        self.env = 0.0;
        self.phase = 0;
        self.duck_g = 1.0;
        self.designed = None;
        self.control(s, true);
        self.control(s, true);
    }

    /// Moves to the next control step: glides the settings, designs what moved, and sets each
    /// glide's start and end.
    #[allow(clippy::needless_range_loop)] // the lines' parallel arrays, by line
    fn control(&mut self, s: &ReverbSettings, jump: bool) {
        for (sm, v) in self.smooth.iter_mut().zip(pack(s)) {
            sm.set_target(v);
            if jump {
                sm.reset(v);
            } else {
                sm.next_value();
            }
        }
        let v: [f32; SMOOTHED] = std::array::from_fn(|k| self.smooth[k].current());
        let g = unpack(&v, s);
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
        self.er[1] = er_scale(g.size) * 0.001 * fs;
        let scale = tank_scale(g.size);
        for i in 0..LINES {
            self.len[0][i] = self.len[1][i];
            self.len[1][i] = BASE_MS[i] * scale * 0.001 * fs;
        }
        if jump {
            self.er[0] = self.er[1];
            self.len[0] = self.len[1];
        }
        if self.designed != Some((g, freeze)) {
            for i in 0..LINES {
                self.filters[i].set(&design_absorption(f64::from(self.len[1][i]), &g, f64::from(fs), f64::from(freeze)));
            }
            let nyq = 0.45 * fs;
            for c in 0..2 {
                self.cuts[c][0].set(true, g.lo_cut.min(nyq), fs);
                self.cuts[c][1].set(false, g.hi_cut.min(nyq), fs);
            }
            // Modulation: rates spread ±30 % around the setting, depth at most a tenth of the line.
            for (i, r) in self.rot.iter_mut().enumerate() {
                let rate = g.mod_rate * (0.7 + 0.6 * ((i * 7) % LINES) as f32 / (LINES - 1) as f32);
                let w = std::f32::consts::TAU * rate / fs;
                *r = [w.cos(), w.sin()];
            }
            self.designed = Some((g, freeze));
        }
        self.depth = g.mod_depth * (MOD_MS * 0.001 * fs).min(0.1 * self.len[1][0]);
        self.diff_g = g.diffusion.clamp(0.0, 1.0) * DIFF_MAX;
        self.early = g.early;
        self.width = g.width;
        // Ducking: the input's envelope against the threshold, 1 dB per dB, at most the depth.
        self.duck[0] = self.duck[1];
        let over = crate::gain_to_db(self.env.max(1e-9)) - g.duck_thresh_db;
        self.duck[1] = crate::db_to_gain(-over.clamp(0.0, g.duck_db.max(0.0)));
        if jump {
            self.duck[0] = self.duck[1];
        }
        // Keep the oscillators on the unit circle.
        for o in &mut self.osc {
            let k = 1.5 - 0.5 * (o[0] * o[0] + o[1] * o[1]);
            o[0] *= k;
            o[1] *= k;
        }
    }

    /// Writes the reverb of `input` (one or two channels) into `wet` (as many). Never allocates.
    #[allow(clippy::needless_range_loop)] // the lines' parallel arrays, by line
    pub fn process(&mut self, input: [&[f32]; 2], wet: [&mut [f32]; 2], stereo: bool, s: &ReverbSettings) -> ReverbTelemetry {
        let [wl, wr] = wet;
        let n = input[0].len().min(wl.len()).min(if stereo { input[1].len().min(wr.len()) } else { usize::MAX });
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
            // Early reflections and the diffused input to the tank, per side.
            let mut early = [0.0f32; 2];
            let mut into = [0.0f32; 2];
            for c in 0..2 {
                if c == 1 && !stereo {
                    early[1] = early[0];
                    into[1] = into[0];
                    break;
                }
                let line = &self.pre[c];
                let mut x = 0.0;
                for h in 0..heads {
                    let w = if heads == 1 { 1.0 } else if h == 0 { 1.0 - fade } else { fade };
                    let pd = self.pd[h];
                    if self.early > 0.0 {
                        let mut e = 0.0;
                        for (k, &g) in ER_GAIN.iter().enumerate() {
                            e += g * line.read(pd + ER_MS[c][k] * er);
                        }
                        early[c] += w * e * self.early * feed;
                    }
                    x += w * line.read(pd);
                }
                x *= feed;
                for a in &mut self.diff[c] {
                    x = a.process(x, self.diff_g);
                }
                into[c] = x * IN_GAIN;
            }
            // The tank.
            let mut v = [0.0f32; LINES];
            for i in 0..LINES {
                let o = self.osc[i];
                let r = self.rot[i];
                self.osc[i] = [o[0] * r[0] - o[1] * r[1], o[1] * r[0] + o[0] * r[1]];
                let len = self.len[0][i] + (self.len[1][i] - self.len[0][i]) * t;
                v[i] = self.reads[i].read(&self.lines[i], len - 1.0 + self.depth * o[1]);
            }
            let (mut ol, mut or) = (0.0f32, 0.0f32);
            for i in 0..LINES {
                ol += OUT_L[i] * v[i];
                or += OUT_R[i] * v[i];
            }
            let mut fb = v;
            hadamard(&mut fb);
            for i in 0..LINES {
                let y = self.filters[i].process(fb[i]);
                let x = IN_SIGN[i] * into[i & 1];
                self.lines[i].write(y + x);
            }
            let mut l = ol * OUT_GAIN + early[0];
            let mut r = or * OUT_GAIN + early[1];
            l = self.cuts[0][0].process(l);
            l = self.cuts[0][1].process(l);
            if stereo {
                r = self.cuts[1][0].process(r);
                r = self.cuts[1][1].process(r);
                let m = 0.5 * (l + r);
                let side = 0.5 * (l - r) * self.width;
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
        tel.freeze = self.freeze.current();
        tel
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
        let dot: f32 = OUT_L.iter().zip(&OUT_R).map(|(a, b)| a * b).sum();
        assert_eq!(dot, 0.0);
    }

    #[test]
    fn the_shelf_reaches_its_gains_at_dc_and_nyquist() {
        let f = shelf(true, 0.5, 1000.0, 48000.0);
        let m = |w: f64| f.magnitude_sq(w).sqrt();
        assert!((m(0.0) - 0.5).abs() < 1e-12);
        assert!((m(std::f64::consts::PI) - 1.0).abs() < 1e-9);
        let h = shelf(false, 0.5, 1000.0, 48000.0);
        assert!((h.magnitude_sq(0.0).sqrt() - 1.0).abs() < 1e-12);
        assert!((h.magnitude_sq(std::f64::consts::PI).sqrt() - 0.5).abs() < 1e-9);
        // Half the dB at the transition frequency.
        let mid = 20.0 * m(std::f64::consts::TAU * 1000.0 / 48000.0).log10();
        assert!((mid - 0.5 * 20.0 * 0.5f64.log10()).abs() < 0.05, "{mid}");
    }

    #[test]
    fn lagrange_read_is_exact_on_whole_delays_and_passive() {
        let mut d = Delay::new(64);
        for i in 0..40 {
            d.write(i as f32);
        }
        assert_eq!(d.read(5.0), 34.0);
        // Linear ramps are interpolated exactly.
        assert!((d.read(5.25) - 33.75).abs() < 1e-4);
    }

    #[test]
    fn decay_at_matches_the_settings_at_the_ends() {
        let s = ReverbSettings { decay_s: 3.0, lo_mult: 2.0, hi_mult: 0.4, ..ReverbSettings::default() };
        let fs = 48000.0;
        assert!((decay_at(&s, 0.0, fs) - 6.0).abs() < 0.01);
        assert!((decay_at(&s, 24000.0, fs) - 1.2).abs() < 0.01);
        let mid = decay_at(&s, 1500.0, fs);
        assert!((mid - 3.0).abs() < 0.3, "{mid}");
    }
}
