//! Mite Sat's tape machine (sat-brief §1.4): one deck after the bands are summed, as a mix goes
//! to tape. Record level into the record equaliser, a magnetic core (`crate::hysteresis`) at the
//! high rate, the tape's own hiss, the playback equaliser (the record one's exact inverse), the
//! bias's high-frequency trade, the playback head's gap and spacing loss, its head bump, then the
//! transport's wow and flutter.
//!
//! ```text
//! x → ·g → R(s) → up → core → down → + hiss → R⁻¹(s) → bias shelf → head loss → bump → ·1/g → transport
//! ```
//!
//! * **Record level** `g` (−12…+24 dB): the core's field. Its output comes back by `1/g`, so the
//!   level moves the tape into saturation, not the volume. At 0 dB a −18 dBFS tone is the deck's
//!   operating level and a −8 dBFS one (10 dB over) its maximum output level: 3 % THD at 1 kHz.
//! * **Record equaliser** `R`: the standard's high-frequency pre-emphasis (a shelf from its time
//!   constant τ₁ up to a depth that grows as the tape slows) and, for NAB, a cut under
//!   τ₂ = 3180 µs (50 Hz) that the playback side lifts back. So the highs saturate first, more on
//!   slower tape; playback undoes it exactly, so a quiet signal comes out flat.
//! * **Bias** (−100…+100 %, 0 the calibrated point): under-biased tape keeps more of its loop (a
//!   lower reversible share, a wider loop: grit at every level) and is brighter; over-biased tape
//!   is cleaner and loses its highs (bias erasure).
//! * **Speed** (3.75, 7.5, 15, 30 ips): sets the equaliser's time constants, the emphasis depth,
//!   the head's high-frequency loss (gap `g` and spacing `d` against the recorded wavelength
//!   `λ = v/f`: `|sinc(π·g/λ)|·e^(−2π·d/λ)`, a minimum-phase FIR), and the head bump (a lift
//!   where the head's contact length matches the wavelength, a dip an octave above, the fringing
//!   loss under it).
//! * **Hiss**: the tape's noise, white on the tape, shaped by the playback equaliser and the
//!   head; at 100 % the deck's own level for its speed.
//! * **Wow and flutter**: one transport for both channels (so the stereo image holds), a sinc
//!   reading of a delay line moving around a fixed centre: a slow drift (wow) and a fast
//!   ripple (flutter). Both at 0: an exact delay.
//!
//! A speed or standard change crossfades two heads (each with its own state); the transport
//! after them is shared. The stage's latency is the oversampler's round trip plus the transport's
//! centre, whole samples, constant while it runs.

use std::f64::consts::{PI, TAU};
use std::sync::OnceLock;

use crate::doubler::Sinc;
use crate::filter::{Coeffs, Tdf2};
use crate::hysteresis::{Ja, JaParams};
use crate::min_phase::min_phase_fir;
use crate::oversample::{Filter, Oversampler};
use crate::reverb::{Delay, Rng, Wander};

/// Tape speeds, in their frozen parameter order (append only).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Speed {
    Ips3_75,
    Ips7_5,
    #[default]
    Ips15,
    Ips30,
}

impl Speed {
    pub const ALL: [Self; 4] = [Self::Ips3_75, Self::Ips7_5, Self::Ips15, Self::Ips30];
    pub const NAMES: [&'static str; 4] = ["3.75 ips", "7.5 ips", "15 ips", "30 ips"];

    pub fn from_index(i: usize) -> Self {
        Self::ALL[i.min(Self::ALL.len() - 1)]
    }

    pub fn index(self) -> usize {
        Self::ALL.iter().position(|&s| s == self).unwrap_or(2)
    }

    /// Inches per second.
    pub fn ips(self) -> f64 {
        [3.75, 7.5, 15.0, 30.0][self.index()]
    }

    /// Metres per second.
    pub fn metres(self) -> f64 {
        self.ips() * 0.0254
    }
}

/// Equalisation standards, in their frozen parameter order (append only).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Standard {
    /// NAB (IEC 2): 50 µs (90 at 3.75 ips) with 3180 µs at the bottom.
    #[default]
    Nab,
    /// CCIR (IEC 1): 35 µs at 15 ips, 70 at 7.5, no bass time constant.
    Ccir,
}

impl Standard {
    pub const ALL: [Self; 2] = [Self::Nab, Self::Ccir];
    pub const NAMES: [&'static str; 2] = ["NAB", "CCIR"];

    pub fn from_index(i: usize) -> Self {
        Self::ALL[i.min(1)]
    }

    pub fn index(self) -> usize {
        match self {
            Self::Nab => 0,
            Self::Ccir => 1,
        }
    }
}

/// The equaliser for a speed and standard: τ₁ (s), τ₂ (s, `None`: no bass time constant), and
/// the pre-emphasis's depth (dB).
pub fn equaliser(speed: Speed, standard: Standard) -> (f64, Option<f64>, f64) {
    use Speed::*;
    use Standard::*;
    let us = 1e-6;
    let (t1, t2) = match (speed, standard) {
        // AES at 30 ips: 17.5 µs, no bass constant, whichever the machine's standard.
        (Ips30, _) => (17.5 * us, None),
        (Ips15, Nab) => (50.0 * us, Some(3180.0 * us)),
        (Ips15, Ccir) => (35.0 * us, None),
        (Ips7_5, Nab) => (50.0 * us, Some(3180.0 * us)),
        (Ips7_5, Ccir) => (70.0 * us, None),
        (Ips3_75, _) => (90.0 * us, Some(3180.0 * us)),
    };
    let depth = [18.0, 15.0, 12.0, 9.0][speed.index()];
    (t1, t2, depth)
}

/// The head bump for a speed: its centre (Hz) and lift (dB). It moves with the speed (the
/// wavelength that matches the head's contact length) and is smaller on fast tape.
pub fn head_bump(speed: Speed) -> (f64, f64) {
    [(16.0, 3.0), (30.0, 2.5), (52.0, 2.0), (105.0, 1.5)][speed.index()]
}
/// The bump's own Q, the dip an octave above it (ratio, dB, Q), and the fringing loss under it
/// (a second-order high-pass at the bump over this).
const BUMP_Q: f64 = 1.4;
const DIP: (f64, f64, f64) = (2.2, -1.0, 2.0);
const FRINGE_RATIO: f64 = 2.5;

/// The playback head's gap and the tape's spacing from it, metres.
const GAP: f64 = 2.0e-6;
const SPACING: f64 = 0.5e-6;
/// The head loss's FIR, taps (its impulse is a few microseconds long at any speed).
const LOSS_TAPS: usize = 32;
const LOSS_FFT: usize = 512;

/// The head's high-frequency loss at `f` (Hz) for `speed`, linear.
pub fn head_loss(speed: Speed, f: f64) -> f64 {
    let lambda = speed.metres() / f.max(1e-3);
    let x = PI * GAP / lambda;
    let gap = if x < 1e-9 { 1.0 } else { (x.sin() / x).abs() };
    (gap * (-TAU * SPACING / lambda).exp()).max(1e-4)
}

/// The record-loss compensation's corner at 7.5 ips (Hz; it moves with the speed) and depth (dB).
const COMP_HZ: f64 = 4000.0;
const COMP_DB: f64 = 12.0;

/// The NAB bass cut's floor into the core (dB): the shelf under 50 Hz stops here.
const BASS_CUT_DB: f64 = 10.0;

/// The field one unit of signal makes at record level 0 dB: a −8 dBFS sine (10 dB over the
/// −18 dBFS operating level) reaches 3 % THD at 1 kHz on 15 ips tape (tested).
pub const FIELD: f32 = 4.4;
pub const DRIVE_DB: (f32, f32) = (-12.0, 24.0);

/// The core's constants at bias `b` (−1…1): Tape's loop, kept more (lower reversible share,
/// wider) when under-biased, less when over-biased.
pub fn core_params(bias: f32) -> JaParams {
    let b = bias.clamp(-1.0, 1.0);
    let reversible = if b >= 0.0 { 0.97 + 0.02 * b } else { 0.97 + 0.12 * b };
    JaParams {
        coupling: 0.1,
        pinning: 0.3 * (1.0 - 0.4 * b),
        reversible,
        h_in: 1.0,
    }
}

/// The bias's high-frequency trade (playback side): a first-order shelf at this frequency at
/// 7.5 ips (it moves with the square root of the speed), down this far at +100 % (erasure), up
/// this far at −100 %.
const BIAS_SHELF_HZ: f64 = 3000.0;
const OVER_BIAS_DB: f64 = -8.0;
const UNDER_BIAS_DB: f64 = 2.5;

/// The tape noise after playback, dB under the operating level (−18 dBFS), full band, per speed.
const HISS_DB: [f64; 4] = [-58.0, -63.0, -66.0, -68.0];
const OPERATING_DBFS: f64 = -18.0;

/// Wow at 100 %: the slow drift's reach, ms (about 0.6 % peak speed deviation); flutter at
/// 100 %: the fast ripple's, ms (about 0.25 %). Their rates, Hz.
const WOW_MS: f64 = 2.5;
const FLUTTER_MS: f64 = 0.055;
const WOW_HZ: f64 = 0.55;
const WOW_WANDER_S: f64 = 1.5;
const FLUTTER_HZ: (f64, f64) = (7.3, 12.9);
/// The transport's centre, ms (more than the reach, so the reading never passes the writing).
const CENTRE_MS: f64 = 3.0;

/// A speed or standard change crossfades the heads over this long, ms.
const HEAD_FADE_MS: f64 = 30.0;
/// Record level, bias, wow, flutter and hiss glide with this time constant, ms.
const GLIDE_MS: f64 = 20.0;
/// Control step, samples (the core's constants and the bias shelf are worked out once a step).
const CONTROL: usize = 16;
/// The longest stretch processed at once.
pub const MAX_BLOCK: usize = 512;

/// The machine as its parameters set it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MachineSettings {
    pub on: bool,
    /// Record level, dB.
    pub drive_db: f32,
    /// −1…1.
    pub bias: f32,
    pub speed: Speed,
    pub standard: Standard,
    /// 0…1 each.
    pub wow: f32,
    pub flutter: f32,
    pub hiss: f32,
}

impl Default for MachineSettings {
    fn default() -> Self {
        Self {
            on: false,
            drive_db: 0.0,
            bias: 0.0,
            speed: Speed::Ips15,
            standard: Standard::Nab,
            wow: 0.1,
            flutter: 0.1,
            hiss: 0.0,
        }
    }
}

/// Per block: what the editor shows.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct MachineTelemetry {
    /// The transport's speed now against its nominal, as a fraction (+0.001: 0.1 % fast).
    pub speed_dev: f32,
    /// The peak going into the core (after record level and emphasis) and the most the core
    /// compressed it this block, dB (≤ 0).
    pub field_peak: f32,
    pub squash_db: f32,
}

/// The core's gain for a quiet sine at bias `b` (its output is divided by it, so quiet signals
/// come out at unity), tabulated over the bias.
fn unity(bias: f32) -> f32 {
    static TABLE: OnceLock<[f32; 9]> = OnceLock::new();
    let t = TABLE.get_or_init(|| {
        std::array::from_fn(|i| {
            let p = core_params(-1.0 + 0.25 * i as f32);
            let (n, amp) = (256usize, 0.002f32);
            let mut core = Ja::rest(&p, 0.0);
            let mut c = 0.0f64;
            for k in 0..4 * n {
                let s = (TAU * k as f64 / n as f64).sin();
                let m = core.step(&p, FIELD * amp * s as f32);
                if k >= 3 * n {
                    c += f64::from(m) * s;
                }
            }
            (c / n as f64 / (f64::from(amp) / 2.0)) as f32
        })
    });
    let x = ((bias.clamp(-1.0, 1.0) + 1.0) * 4.0).min(7.999);
    let i = x as usize;
    let f = x - i as f32;
    t[i] + (t[i + 1] - t[i]) * f
}

/// A first-order section `(b0 + b1·z⁻¹)/(1 + a1·z⁻¹)` from `(n1·s + n0)/(d1·s + d0)` by the
/// bilinear transform, prewarped at `f_warp`.
fn bilinear1(n1: f64, n0: f64, d1: f64, d0: f64, f_warp: f64, fs: f64) -> Coeffs {
    let w = TAU * f_warp;
    let k = w / (w / (2.0 * fs)).tan();
    let (b0, b1) = (n1 * k + n0, n0 - n1 * k);
    let (a0, a1) = (d1 * k + d0, d0 - d1 * k);
    Coeffs {
        b0: b0 / a0,
        b1: b1 / a0,
        b2: 0.0,
        a1: a1 / a0,
        a2: 0.0,
    }
}

/// RBJ's peaking section.
fn peaking(f: f64, db: f64, q: f64, fs: f64) -> Coeffs {
    let a = 10f64.powf(db / 40.0);
    let w = TAU * f / fs;
    let alpha = w.sin() / (2.0 * q);
    let cw = w.cos();
    let a0 = 1.0 + alpha / a;
    Coeffs {
        b0: (1.0 + alpha * a) / a0,
        b1: -2.0 * cw / a0,
        b2: (1.0 - alpha * a) / a0,
        a1: -2.0 * cw / a0,
        a2: (1.0 - alpha / a) / a0,
    }
}

/// A second-order Butterworth high-pass.
fn high_pass(f: f64, fs: f64) -> Coeffs {
    let w = TAU * f / fs;
    let alpha = w.sin() / (2.0 * std::f64::consts::FRAC_1_SQRT_2);
    let cw = w.cos();
    let a0 = 1.0 + alpha;
    Coeffs {
        b0: (1.0 + cw) / 2.0 / a0,
        b1: -(1.0 + cw) / a0,
        b2: (1.0 + cw) / 2.0 / a0,
        a1: -2.0 * cw / a0,
        a2: (1.0 - alpha) / a0,
    }
}

/// The record equaliser's three sections for a speed and standard at `fs`: the standard's treble
/// shelf `(1 + s·τ₁)/(1 + s·τ₁/G)`, the NAB bass cut `(s·τ₂ + 1/B)/(s·τ₂ + 1)` (identity for
/// CCIR), and the record-loss compensation: a second treble shelf a deck's record amplifier adds
/// to make up for the record head's losses, from [`COMP_HZ`] (scaled with the speed) up by
/// [`COMP_DB`]. Its share is what makes slow tape lose its highs first: at 7.5 ips a 10 kHz tone
/// saturates about 10 dB under the maximum output level, at 30 ips near it.
pub fn record_eq(speed: Speed, standard: Standard, fs: f64) -> [Coeffs; 3] {
    let (t1, t2, depth) = equaliser(speed, standard);
    let g = 10f64.powf(depth / 20.0);
    let f1 = 1.0 / (TAU * t1);
    // Prewarp where the shelf turns, so its corner lands at the standard's frequency.
    let treble = bilinear1(t1, 1.0, t1 / g, 1.0, f1.min(0.45 * fs), fs);
    let bass = match t2 {
        Some(t2) => {
            let b = 10f64.powf(BASS_CUT_DB / 20.0);
            bilinear1(t2, 1.0 / b, t2, 1.0, 1.0 / (TAU * t2), fs)
        }
        None => Coeffs::IDENTITY,
    };
    let fc = COMP_HZ * speed.ips() / 7.5;
    let tc = 1.0 / (TAU * fc);
    let gc = 10f64.powf(COMP_DB / 20.0);
    let comp = bilinear1(tc, 1.0, tc / gc, 1.0, fc.min(0.45 * fs), fs);
    [treble, bass, comp]
}

/// A first-order section's exact inverse (both of the record equaliser's are minimum phase).
fn inverse1(c: &Coeffs) -> Coeffs {
    Coeffs {
        b0: 1.0 / c.b0,
        b1: c.a1 / c.b0,
        b2: 0.0,
        a1: c.b1 / c.b0,
        a2: 0.0,
    }
}

/// The bias shelf's gain at the top of the band for `bias`, linear.
fn bias_gain(bias: f32) -> f32 {
    let b = f64::from(bias.clamp(-1.0, 1.0));
    let db = if b >= 0.0 { OVER_BIAS_DB * b } else { -UNDER_BIAS_DB * b };
    10f64.powf(db / 20.0) as f32
}

/// The bias shelf's fixed high-pass for `speed` at `fs`: the shelf is `x + (g − 1)·HP(x)`, which
/// is `(1 + s·t·g)/(1 + s·t)`, unity at DC and `g` at the top. Only `g` moves with the bias, so a
/// bias change ramps a gain and never switches a filter's coefficients under the music (that
/// clicked: +23 dB on a jump end to end).
fn bias_high_pass(speed: Speed, fs: f64) -> Coeffs {
    let f = BIAS_SHELF_HZ * (speed.ips() / 7.5).sqrt();
    let t = 1.0 / (TAU * f);
    bilinear1(t, 0.0, t, 1.0, f.min(0.45 * fs), fs)
}

/// The head loss's minimum-phase FIR for `speed` at `fs`.
pub fn loss_kernel(speed: Speed, fs: f64) -> Vec<f32> {
    let bins = LOSS_FFT / 2 + 1;
    let log_mag: Vec<f64> = (0..bins)
        .map(|k| head_loss(speed, fs * k as f64 / LOSS_FFT as f64).ln())
        .collect();
    let h = min_phase_fir(&log_mag, LOSS_TAPS);
    // Unity at DC exactly (the truncation's taper moves it a little).
    let sum: f64 = h.iter().sum();
    h.iter().map(|v| (v / sum) as f32).collect()
}

/// One playback chain's filters (the linear part after the core) for a speed and standard.
#[derive(Debug, Clone, Copy)]
struct Playback {
    de_treble: Coeffs,
    de_bass: Coeffs,
    de_comp: Coeffs,
    bump: Coeffs,
    dip: Coeffs,
    fringe: Coeffs,
    /// The hiss's scale for white noise of unit variance into this chain to come out at the
    /// deck's level.
    hiss: f32,
}

impl Playback {
    fn new(speed: Speed, standard: Standard, fs: f64, kernel: &[f32]) -> Self {
        let [treble, bass, comp] = record_eq(speed, standard, fs);
        let (de_treble, de_bass, de_comp) = (inverse1(&treble), inverse1(&bass), inverse1(&comp));
        let (bf, bdb) = head_bump(speed);
        let bump = peaking(bf, bdb, BUMP_Q, fs);
        let dip = peaking(bf * DIP.0, DIP.1, DIP.2, fs);
        let fringe = high_pass(bf / FRINGE_RATIO, fs);
        // The power white noise has after the chain, by its response on a fine grid.
        let n = 4096;
        let mut power = 0.0;
        for k in 1..n {
            let w = PI * k as f64 / n as f64;
            let z1 = realfft::num_complex::Complex::from_polar(1.0, -w);
            let fir: realfft::num_complex::Complex<f64> = kernel
                .iter()
                .enumerate()
                .map(|(i, &h)| f64::from(h) * z1.powu(i as u32))
                .sum();
            let m = [de_treble, de_bass, de_comp, bump, dip, fringe]
                .iter()
                .map(|c| c.magnitude_sq(w))
                .product::<f64>()
                * fir.norm_sqr();
            power += m;
        }
        power /= n as f64;
        let target = 10f64.powf((OPERATING_DBFS + HISS_DB[speed.index()]) / 20.0);
        Self {
            de_treble,
            de_bass,
            de_comp,
            bump,
            dip,
            fringe,
            hiss: (target / power.sqrt()) as f32,
        }
    }
}

/// One head: a configuration (speed, standard) with all its state.
struct Head {
    speed: Speed,
    standard: Standard,
    os: Oversampler,
    rec: [Coeffs; 3],
    pre: [[Tdf2; 3]; 2],
    play: Playback,
    post: [[Tdf2; 7]; 2],
    bias_hp: Coeffs,
    kernel: [f32; LOSS_TAPS],
    fir: [[f32; LOSS_TAPS]; 2],
    fir_pos: usize,
    core: [Ja; 2],
}

impl Head {
    fn new(factor: usize) -> Self {
        let mut os = Oversampler::new(factor);
        os.set_filter(Filter::MinimumPhase);
        Self {
            speed: Speed::Ips15,
            standard: Standard::Nab,
            os,
            rec: [Coeffs::IDENTITY; 3],
            pre: [[Tdf2::default(); 3]; 2],
            play: Playback {
                de_treble: Coeffs::IDENTITY,
                de_bass: Coeffs::IDENTITY,
                de_comp: Coeffs::IDENTITY,
                bump: Coeffs::IDENTITY,
                dip: Coeffs::IDENTITY,
                fringe: Coeffs::IDENTITY,
                hiss: 0.0,
            },
            post: [[Tdf2::default(); 7]; 2],
            bias_hp: Coeffs::IDENTITY,
            kernel: [0.0; LOSS_TAPS],
            fir: [[0.0; LOSS_TAPS]; 2],
            fir_pos: 0,
            core: [Ja::default(); 2],
        }
    }

    fn clear(&mut self) {
        self.os.reset();
        self.pre = [[Tdf2::default(); 3]; 2];
        self.post = [[Tdf2::default(); 7]; 2];
        self.fir = [[0.0; LOSS_TAPS]; 2];
        self.fir_pos = 0;
        self.core = [Ja::default(); 2];
    }
}

/// The transport: one delay line per channel read at one moving position.
struct Transport {
    line: [Delay; 2],
    sinc: Sinc,
    centre: usize,
    wander: Wander,
    ripple: Wander,
    t: f64,
    dt: f64,
    last: f64,
    exact: bool,
    /// Wow's and flutter's reach, samples, gliding every sample (a reach that stepped once per
    /// control stretch jumped the read position: a click on every move of Wow).
    wow_d: f64,
    flutter_d: f64,
    /// The per-sample glide toward them.
    k: f64,
}

/// The tape machine stage.
pub struct Machine {
    fs: f64,
    factor: usize,
    heads: Box<[Head; 2]>,
    active: usize,
    /// The crossfade into the other head: its position, 0…1.
    fade: Option<f64>,
    fade_step: f64,
    /// Loss kernels per speed at this rate.
    kernels: [[f32; LOSS_TAPS]; 4],
    transport: Transport,
    glide: f64,
    /// Gliding values: record level (linear), bias, wow, flutter, hiss.
    now: [f64; 5],
    /// The hiss's source, one per channel (so the output does not depend on the block size).
    rng: [Rng; 2],
    countdown: usize,
    /// The record level the control step ramps from (the last one's).
    g_last: f32,
    /// The bias the control step ramps from.
    b_last: f32,
    hi: Vec<f32>,
    lo: Vec<f32>,
    heads_out: [[Vec<f32>; 2]; 2],
    telemetry: MachineTelemetry,
}

impl Machine {
    /// Allocates.
    pub fn new(sample_rate: f32, factor: usize, s: &MachineSettings) -> Self {
        let fs = f64::from(sample_rate);
        let mut m = Self {
            fs,
            factor,
            heads: Box::new([Head::new(factor), Head::new(factor)]),
            active: 0,
            fade: None,
            fade_step: 0.0,
            kernels: [[0.0; LOSS_TAPS]; 4],
            transport: Transport {
                line: [Delay::new(1), Delay::new(1)],
                sinc: Sinc::new(),
                centre: 1,
                wander: Wander::new(0x51A7_E5ED),
                ripple: Wander::new(0x0F1E_77E2),
                t: 0.0,
                dt: 0.0,
                last: 0.0,
                exact: true,
                wow_d: 0.0,
                flutter_d: 0.0,
                k: 0.0,
            },
            glide: 0.0,
            now: [1.0, 0.0, 0.0, 0.0, 0.0],
            rng: [Rng(0x7A9E_0001), Rng(0x3C6E_F372)],
            countdown: 0,
            g_last: 1.0,
            b_last: 0.0,
            hi: vec![0.0; CONTROL * 16],
            lo: vec![0.0; CONTROL],
            heads_out: [
                [vec![0.0; MAX_BLOCK], vec![0.0; MAX_BLOCK]],
                [vec![0.0; MAX_BLOCK], vec![0.0; MAX_BLOCK]],
            ],
            telemetry: MachineTelemetry::default(),
        };
        m.prepare(sample_rate, factor, s);
        m
    }

    /// Sizes everything for `sample_rate` and oversampling `factor`. Allocates.
    pub fn prepare(&mut self, sample_rate: f32, factor: usize, s: &MachineSettings) {
        self.fs = f64::from(sample_rate);
        self.factor = factor;
        for (k, speed) in self.kernels.iter_mut().zip(Speed::ALL) {
            let h = loss_kernel(speed, self.fs);
            k.copy_from_slice(&h);
        }
        for h in self.heads.iter_mut() {
            h.os.set_factor(factor);
            h.os.set_filter(Filter::MinimumPhase);
        }
        let reach = (WOW_MS + FLUTTER_MS * 1.35) * 1e-3 * self.fs;
        let centre = ((CENTRE_MS * 1e-3 * self.fs).max(reach) + 20.0).ceil() as usize;
        self.transport.centre = centre;
        self.transport.line = [Delay::new(2 * centre + 64), Delay::new(2 * centre + 64)];
        self.transport.dt = 1.0 / self.fs;
        self.transport.wander.step = (1.0 / (WOW_WANDER_S * self.fs)) as f32;
        self.transport.ripple.step = (1.0 / (0.4 * self.fs)) as f32;
        self.fade_step = 1000.0 / (HEAD_FADE_MS * self.fs);
        self.glide = 1.0 - (-(CONTROL as f64) * 1000.0 / (GLIDE_MS * self.fs)).exp();
        self.transport.k = 1.0 - (-1000.0 / (GLIDE_MS * self.fs)).exp();
        self.reset(s);
    }

    /// The stage's latency, samples: the oversampler's round trip and the transport's centre.
    pub fn latency(&self) -> usize {
        Self::latency_of(self.factor, self.fs)
    }

    /// The latency at `factor` and `fs`, samples.
    pub fn latency_of(factor: usize, fs: f64) -> usize {
        let reach = (WOW_MS + FLUTTER_MS * 1.35) * 1e-3 * fs;
        let centre = ((CENTRE_MS * 1e-3 * fs).max(reach) + 20.0).ceil() as usize;
        Oversampler::latency_of(factor, Filter::MinimumPhase) + centre
    }

    pub fn oversampling(&self) -> usize {
        self.factor
    }

    /// Changes the oversampling (clears the state; the caller dips around it, as the latency moves).
    pub fn set_oversampling(&mut self, factor: usize, s: &MachineSettings) {
        self.factor = factor;
        for h in self.heads.iter_mut() {
            h.os.set_factor(factor);
        }
        self.reset(s);
    }

    pub fn telemetry(&self) -> MachineTelemetry {
        self.telemetry
    }

    /// Clears the state and snaps every glide to `s`.
    pub fn reset(&mut self, s: &MachineSettings) {
        self.active = 0;
        self.fade = None;
        self.configure(0, s.speed, s.standard);
        self.heads[0].clear();
        self.heads[1].clear();
        self.now = Self::targets(s);
        self.g_last = self.now[0] as f32;
        self.b_last = self.now[1] as f32;
        let t = &mut self.transport;
        for l in &mut t.line {
            l.reset();
        }
        t.wander = Wander::new(0x51A7_E5ED);
        t.wander.step = (1.0 / (WOW_WANDER_S * self.fs)) as f32;
        t.ripple = Wander::new(0x0F1E_77E2);
        t.ripple.step = (1.0 / (0.4 * self.fs)) as f32;
        t.t = 0.0;
        t.last = t.centre as f64;
        t.exact = s.wow == 0.0 && s.flutter == 0.0;
        t.wow_d = f64::from(s.wow.clamp(0.0, 1.0)) * WOW_MS * 1e-3 * self.fs;
        t.flutter_d = f64::from(s.flutter.clamp(0.0, 1.0)) * FLUTTER_MS * 1e-3 * self.fs;
        self.rng = [Rng(0x7A9E_0001), Rng(0x3C6E_F372)];
        self.countdown = 0;
        self.telemetry = MachineTelemetry::default();
    }

    fn targets(s: &MachineSettings) -> [f64; 5] {
        let db = s.drive_db.clamp(DRIVE_DB.0, DRIVE_DB.1);
        [
            10f64.powf(f64::from(db) / 20.0),
            f64::from(s.bias.clamp(-1.0, 1.0)),
            f64::from(s.wow.clamp(0.0, 1.0)),
            f64::from(s.flutter.clamp(0.0, 1.0)),
            f64::from(s.hiss.clamp(0.0, 1.0)),
        ]
    }

    fn configure(&mut self, i: usize, speed: Speed, standard: Standard) {
        let fs = self.fs;
        let kernel = self.kernels[speed.index()];
        let h = &mut self.heads[i];
        h.speed = speed;
        h.standard = standard;
        h.rec = record_eq(speed, standard, fs);
        h.bias_hp = bias_high_pass(speed, fs);
        h.play = Playback::new(speed, standard, fs, &kernel);
        h.kernel = kernel;
    }

    /// Processes up to [`MAX_BLOCK`] samples of one or two channels in place.
    #[allow(clippy::needless_range_loop)] // channels, heads' outputs and lines indexed together
    pub fn process(&mut self, channels: &mut [&mut [f32]], s: &MachineSettings) {
        let n_ch = channels.len().min(2);
        if n_ch == 0 {
            return;
        }
        let len = channels.iter().map(|c| c.len()).min().unwrap_or(0).min(MAX_BLOCK);
        // A new speed or standard: into the other head, once the last crossfade is done.
        let want = (s.speed, s.standard);
        let a = &self.heads[self.active];
        if self.fade.is_none() && (a.speed, a.standard) != want {
            let other = 1 - self.active;
            self.configure(other, want.0, want.1);
            self.heads[other].clear();
            self.fade = Some(0.0);
        }
        let target = Self::targets(s);
        let mut field_peak = 0.0f32;
        let mut squash = 0.0f32;

        let mut start = 0;
        while start < len {
            if self.countdown == 0 {
                self.g_last = self.now[0] as f32;
                self.b_last = self.now[1] as f32;
                for (v, t) in self.now.iter_mut().zip(target) {
                    *v += (t - *v) * self.glide;
                }
                self.countdown = CONTROL;
            }
            let n = (len - start).min(self.countdown);
            let end = start + n;
            let g = self.now[0] as f32;
            let g_from = self.g_last;
            // Where this stretch starts in its control step (a host block can end inside one).
            let into = CONTROL - self.countdown;
            // The core's constants and its scale at the step's two ends, ramped between like the
            // level (a bias change stepped them every 16 samples: a buzz under the music).
            let (p0, p1) = (core_params(self.b_last), core_params(self.now[1] as f32));
            let (u0, u1) = (unity(self.b_last), unity(self.now[1] as f32));
            let (sg0, sg1) = (bias_gain(self.b_last), bias_gain(self.now[1] as f32));
            let hiss = self.now[4] as f32;
            let running: &[usize] = if self.fade.is_some() { &[0, 1] } else { &[0] };
            for &which in running {
                let hi_idx = if which == 0 { self.active } else { 1 - self.active };
                let head = &mut self.heads[hi_idx];
                let f = head.os.factor();
                for c in 0..n_ch {
                    // The record equaliser.
                    for (i, v) in channels[c][start..end].iter().enumerate() {
                        let mut x = f64::from(*v);
                        x = head.pre[c][0].process(&head.rec[0], x);
                        x = head.pre[c][1].process(&head.rec[1], x);
                        x = head.pre[c][2].process(&head.rec[2], x);
                        self.lo[i] = x as f32;
                    }
                    head.os.up(c, &self.lo[..n], &mut self.hi[..n * f]);
                    // The record level around the core alone, ramped sample by sample at the high
                    // rate: nothing with memory but the tape sits between it and its undoing, so a
                    // level change never leaves the filters holding audio at another scale.
                    let step = (g - g_from) / (CONTROL * f) as f32;
                    for (j, v) in self.hi[..n * f].iter_mut().enumerate() {
                        let t = (into * f + j + 1) as f32 / (CONTROL * f) as f32;
                        let gj = g_from + step * (into * f + j + 1) as f32;
                        let x = *v * gj;
                        field_peak = field_peak.max(x.abs());
                        let p = JaParams {
                            coupling: p0.coupling,
                            pinning: p0.pinning + (p1.pinning - p0.pinning) * t,
                            reversible: p0.reversible + (p1.reversible - p0.reversible) * t,
                            h_in: 1.0,
                        };
                        let u = u0 + (u1 - u0) * t;
                        let m = head.core[c].step(&p, FIELD * x) / u;
                        if x.abs() > 1e-3 {
                            squash = squash.min((m / x).abs().max(1e-6).log10() * 20.0);
                        }
                        *v = m / gj;
                    }
                    head.os.down(c, &self.hi[..n * f], &mut self.lo[..n]);
                    let out = &mut self.heads_out[which][c];
                    for i in 0..n {
                        // The tape's noise is on the tape: it goes through playback.
                        let noise = if hiss > 0.0 {
                            {
                            let r = &mut self.rng[c];
                            (r.next() + r.next() + r.next() - 1.5) * 2.0
                        }
                        } else {
                            0.0
                        };
                        let mut y = f64::from(self.lo[i] + noise * hiss * head.play.hiss);
                        let pl = &head.play;
                        let st = &mut head.post[c];
                        y = st[0].process(&pl.de_bass, y);
                        y = st[1].process(&pl.de_treble, y);
                        y = st[2].process(&pl.de_comp, y);
                        let shelf = sg0 + (sg1 - sg0) * (into + i + 1) as f32 / CONTROL as f32 - 1.0;
                        y += f64::from(shelf) * st[3].process(&head.bias_hp, y);
                        // The head's loss.
                        let ring = &mut head.fir[c];
                        let pos = (head.fir_pos + i) % LOSS_TAPS;
                        ring[pos] = y as f32;
                        let mut acc = 0.0f32;
                        for (k, h) in head.kernel.iter().enumerate() {
                            acc += h * ring[(pos + LOSS_TAPS - k) % LOSS_TAPS];
                        }
                        y = f64::from(acc);
                        y = st[4].process(&pl.bump, y);
                        y = st[5].process(&pl.dip, y);
                        y = st[6].process(&pl.fringe, y);
                        out[start + i] = y as f32;
                    }
                }
                head.fir_pos = (head.fir_pos + n) % LOSS_TAPS;
            }
            self.countdown -= n;
            start = end;
        }

        // The heads' crossfade, then the transport.
        let t = &mut self.transport;
        let wow_to = target[2] * WOW_MS * 1e-3 * self.fs;
        let flutter_to = target[3] * FLUTTER_MS * 1e-3 * self.fs;
        if wow_to > 0.0 || flutter_to > 0.0 {
            t.exact = false;
        } else if t.wow_d < 1e-6 && t.flutter_d < 1e-6 {
            // Back at the centre to within a millionth of a sample: the plain tap again.
            (t.wow_d, t.flutter_d) = (0.0, 0.0);
            t.exact = true;
        }
        let mut fade = self.fade;
        for i in 0..len {
            let mut d = t.centre as f64;
            if !t.exact {
                let wn = f64::from(t.wander.next());
                let rp = f64::from(t.ripple.next());
                let tt = t.t;
                let w = 0.6 * wn + 0.4 * (TAU * WOW_HZ * tt).sin();
                let fl = (TAU * FLUTTER_HZ.0 * tt).sin() * (0.75 + 0.25 * rp)
                    + 0.35 * (TAU * FLUTTER_HZ.1 * tt).sin();
                t.wow_d += (wow_to - t.wow_d) * t.k;
                t.flutter_d += (flutter_to - t.flutter_d) * t.k;
                d += t.wow_d * w + t.flutter_d * fl;
                t.t += t.dt;
                if t.t > 1e4 {
                    t.t -= 1e4;
                }
            }
            let k = fade.map_or(0.0, |f| f as f32);
            for c in 0..n_ch {
                let a = self.heads_out[0][c][i];
                let b = if fade.is_some() { self.heads_out[1][c][i] } else { 0.0 };
                let x = a * (1.0 - k) + b * k;
                t.line[c].write(x);
                channels[c][i] = if t.exact {
                    t.line[c].tap(t.centre)
                } else {
                    t.sinc.read(&t.line[c], d)
                };
            }
            if let Some(f) = fade.as_mut() {
                *f += self.fade_step;
                if *f >= 1.0 {
                    fade = None;
                    self.active = 1 - self.active;
                    // The new head's output moves to slot 0 from the next sample on.
                    for c in 0..n_ch {
                        let (x, y) = self.heads_out.split_at_mut(1);
                        x[0][c][i + 1..len].copy_from_slice(&y[0][c][i + 1..len]);
                    }
                }
            }
            if i == len - 1 {
                // The block's mean speed against nominal: the reading falling behind is the tape
                // slowing (its position moved `last − d` over the block).
                self.telemetry.speed_dev = ((t.last - d) / len as f64) as f32;
                t.last = d;
            }
        }
        self.fade = fade;
        self.telemetry.field_peak = field_peak;
        self.telemetry.squash_db = squash;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FS: f32 = 48_000.0;

    fn sine(freq: f32, amp: f32, n: usize) -> Vec<f32> {
        (0..n)
            .map(|i| amp * (std::f32::consts::TAU * freq * i as f32 / FS).sin())
            .collect()
    }

    fn run(m: &mut Machine, input: &[f32], s: &MachineSettings) -> Vec<f32> {
        let mut out = input.to_vec();
        for chunk in out.chunks_mut(256) {
            let mut ch: [&mut [f32]; 1] = [chunk];
            m.process(&mut ch, s);
        }
        out
    }

    /// Magnitude of `freq` in `x` (a whole number of cycles is not needed: Hann window).
    fn amplitude(x: &[f32], freq: f64) -> f64 {
        let n = x.len();
        let (mut re, mut im, mut wsum) = (0.0, 0.0, 0.0);
        for (i, &v) in x.iter().enumerate() {
            let w = 0.5 - 0.5 * (TAU * i as f64 / n as f64).cos();
            let ph = TAU * freq * i as f64 / f64::from(FS);
            re += f64::from(v) * w * ph.cos();
            im += f64::from(v) * w * ph.sin();
            wsum += w;
        }
        2.0 * (re * re + im * im).sqrt() / wsum
    }

    fn thd(x: &[f32], f0: f64) -> f64 {
        let h1 = amplitude(x, f0);
        let rest: f64 = (2..=9).map(|k| amplitude(x, f0 * k as f64).powi(2)).sum();
        rest.sqrt() / h1
    }

    fn quiet() -> MachineSettings {
        MachineSettings { on: true, wow: 0.0, flutter: 0.0, ..Default::default() }
    }

    #[test]
    fn maximum_output_level_is_ten_db_over_operating_level() {
        // At record level 0, 15 ips: −8 dBFS at 1 kHz reaches about 3 % THD; −18 dBFS is clean.
        let s = quiet();
        let mut m = Machine::new(FS, 4, &s);
        let mol = run(&mut m, &sine(1000.0, 10f32.powf(-8.0 / 20.0), 48_000), &s);
        let t = thd(&mol[24_000..], 1000.0);
        assert!((0.022..0.04).contains(&t), "THD at MOL {:.2} %", 100.0 * t);
        let mut m = Machine::new(FS, 4, &s);
        let op = run(&mut m, &sine(1000.0, 10f32.powf(-18.0 / 20.0), 48_000), &s);
        let t = thd(&op[24_000..], 1000.0);
        assert!(t < 0.006, "THD at operating level {:.3} %", 100.0 * t);
    }

    #[test]
    fn quiet_signals_come_out_flat_but_for_the_head() {
        // −40 dBFS: the record and playback equalisers cancel; what is left is the head's bump
        // (lows) and loss (highs), as specified.
        let s = quiet();
        for speed in Speed::ALL {
            let s = MachineSettings { speed, ..s };
            for f in [200.0f32, 1000.0, 5000.0] {
                let mut m = Machine::new(FS, 2, &s);
                let y = run(&mut m, &sine(f, 0.01, 48_000), &s);
                let db = 20.0 * (amplitude(&y[24_000..], f64::from(f)) / 0.01).log10();
                let loss = 20.0 * head_loss(speed, f64::from(f)).log10();
                assert!((db - loss).abs() < 0.6, "{speed:?} {f} Hz: {db:.2} dB (head {loss:.2})");
            }
        }
    }

    #[test]
    fn the_head_bump_sits_where_the_speed_puts_it() {
        let s = quiet();
        for speed in Speed::ALL {
            let s = MachineSettings { speed, ..s };
            let (bf, bdb) = head_bump(speed);
            let mut m = Machine::new(FS, 2, &s);
            let y = run(&mut m, &sine(bf as f32, 0.01, 96_000), &s);
            let db = 20.0 * (amplitude(&y[48_000..], bf) / 0.01).log10();
            assert!(db > 0.5 * bdb - 1.0 && db < bdb + 0.5, "{speed:?}: {db:.2} dB at {bf} Hz");
        }
    }

    #[test]
    fn slower_tape_saturates_its_highs_sooner() {
        // A 10 kHz tone at −20 dBFS: compressed more at 7.5 ips than at 30.
        let level = |speed| {
            let s = MachineSettings { speed, ..quiet() };
            let mut m = Machine::new(FS, 4, &s);
            let lo = run(&mut m, &sine(10_000.0, 0.001, 24_000), &s);
            let mut m = Machine::new(FS, 4, &s);
            let hi = run(&mut m, &sine(10_000.0, 0.1, 24_000), &s);
            20.0 * (amplitude(&hi[12_000..], 10_000.0) / 0.1 / (amplitude(&lo[12_000..], 10_000.0) / 0.001)).log10()
        };
        let (slow, fast) = (level(Speed::Ips7_5), level(Speed::Ips30));
        assert!(slow < fast - 1.0, "7.5 ips {slow:.2} dB, 30 ips {fast:.2} dB");
    }

    #[test]
    fn under_bias_is_grittier_over_bias_darker() {
        let s = quiet();
        let run_b = |bias: f32, f: f32, amp: f32| {
            let s = MachineSettings { bias, ..s };
            let mut m = Machine::new(FS, 4, &s);
            run(&mut m, &sine(f, amp, 48_000), &s)
        };
        let t = |b| thd(&run_b(b, 1000.0, 0.05)[24_000..], 1000.0);
        assert!(t(-1.0) > 2.0 * t(0.0), "under {:.3} % vs {:.3} %", 100.0 * t(-1.0), 100.0 * t(0.0));
        assert!(t(1.0) < t(0.0));
        let hf = |b| amplitude(&run_b(b, 12_000.0, 0.005)[24_000..], 12_000.0);
        assert!(20.0 * (hf(1.0) / hf(0.0)).log10() < -3.0);
        assert!(hf(-1.0) > hf(0.0));
    }

    #[test]
    fn hiss_is_the_decks_own_level() {
        for speed in Speed::ALL {
            let s = MachineSettings { speed, hiss: 1.0, ..quiet() };
            let mut m = Machine::new(FS, 2, &s);
            let y = run(&mut m, &vec![0.0; 96_000], &s);
            let rms = (y[48_000..].iter().map(|v| f64::from(*v).powi(2)).sum::<f64>() / 48_000.0).sqrt();
            let db = 20.0 * rms.log10();
            let want = OPERATING_DBFS + HISS_DB[speed.index()];
            assert!((db - want).abs() < 1.0, "{speed:?}: {db:.1} dBFS, want {want:.1}");
        }
        // Hiss 0: silence stays silence.
        let s = quiet();
        let mut m = Machine::new(FS, 2, &s);
        assert!(run(&mut m, &vec![0.0; 4800], &s).iter().all(|v| *v == 0.0));
    }

    #[test]
    fn latency_is_where_the_impulse_lands() {
        let s = MachineSettings { speed: Speed::Ips30, ..quiet() };
        for factor in [1, 2, 4, 8, 16] {
            let mut m = Machine::new(FS, factor, &s);
            let mut x = vec![0.0f32; 4800];
            x[1000] = 0.01;
            let y = run(&mut m, &x, &s);
            let peak = y.iter().enumerate().max_by(|a, b| a.1.abs().total_cmp(&b.1.abs())).unwrap().0;
            let lat = m.latency();
            assert!(peak.abs_diff(1000 + lat) <= 1, "{factor}×: peak at {peak}, latency {lat}");
        }
    }

    #[test]
    fn wow_bends_the_pitch_and_flutter_ripples_it() {
        // A 1 kHz tone through a deck with wow: its phase wanders against the dry tone by the
        // drift's reach; with none it is a steady delay.
        let s = MachineSettings { wow: 1.0, flutter: 0.0, ..quiet() };
        let mut m = Machine::new(FS, 1, &s);
        let x = sine(1000.0, 0.01, 4 * 48_000);
        let y = run(&mut m, &x, &s);
        // Zero crossings' spacing varies.
        let crossings: Vec<f64> = (1..y.len())
            .filter(|&i| y[i - 1] < 0.0 && y[i] >= 0.0)
            .map(|i| i as f64 - f64::from(y[i]) / f64::from(y[i] - y[i - 1]))
            .collect();
        let periods: Vec<f64> = crossings.windows(2).map(|w| w[1] - w[0]).collect();
        let (lo, hi) = periods[100..].iter().fold((f64::MAX, f64::MIN), |(a, b), &p| (a.min(p), b.max(p)));
        let dev = (hi - lo) / 48.0 / 2.0;
        assert!((0.001..0.012).contains(&dev), "wow 100 %: ±{:.3} %", 100.0 * dev);
        assert!(m.telemetry().speed_dev != 0.0, "the transport reports its speed");
    }

    #[test]
    fn a_speed_change_crossfades_without_a_click() {
        let s = quiet();
        let mut m = Machine::new(FS, 2, &s);
        let x = sine(220.0, 0.1, 48_000);
        let mut y = run(&mut m, &x[..24_000], &s);
        let s2 = MachineSettings { speed: Speed::Ips7_5, ..s };
        y.extend(run(&mut m, &x[24_000..], &s2));
        let jump = y.windows(2).skip(20_000).map(|w| (w[1] - w[0]).abs()).fold(0.0f32, f32::max);
        let normal = std::f32::consts::TAU * 220.0 / FS * 0.1 * 1.3;
        assert!(jump < normal * 1.2, "largest step {jump} against {normal}");
    }

    #[test]
    fn non_finite_input_recovers() {
        let s = MachineSettings { hiss: 0.5, ..quiet() };
        let mut m = Machine::new(FS, 4, &s);
        let mut x = sine(1000.0, 0.1, 48_000);
        x[100] = f32::NAN;
        x[101] = f32::INFINITY;
        let _ = run(&mut m, &x, &s);
        m.reset(&s);
        let y = run(&mut m, &sine(1000.0, 0.1, 9600), &s);
        assert!(y.iter().all(|v| v.is_finite()));
    }
}
