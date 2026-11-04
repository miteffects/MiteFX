//! Mite Delay's engine (docs/design/delay-brief.md §1): up to four taps, each its own stereo echo
//! line with its own feedback. Inside every loop: the loop EQ (`mite_dsp::eq` bands, none by
//! default), the machine (Digital, Tape or BBD: its saturation, its losses, how a time change
//! sounds), drive and diffusion. Then the taps' sum gets its width and ducking. This makes the
//! echoes only; the plugin mixes them with the dry signal.
//!
//! An echo comes back exactly at its tap's time: the loop reads its line that much earlier than
//! the diffusion's own delay, so echo `k` of an impulse lands at `k × time` (tested). The loop is
//! run in steps of at most [`CONTROL`] samples (a line is never read closer than that to where it
//! is written), so the EQ runs on blocks; the result is the same for any host block size.

use crate::eq::{analog_response_db, BandProcessor, BandSettings, Dynamics, Placement};
use crate::reverb::{Allpass, Delay, Wander};

pub const TAPS: usize = 4;
pub const LOOP_BANDS: usize = 6;
pub const MIN_TIME_MS: f32 = 10.0;
pub const MAX_TIME_MS: f32 = 5000.0;
/// Samples per control step (targets, glides, the ducking's target); the loop's block.
pub const CONTROL: usize = 32;
const EQ_SMOOTH_MS: f32 = 20.0;
/// A loop band's on/off or type change fades this long (every echo hears it).
const EQ_FADE_MS: f32 = 30.0;
/// Gains, feedback, pan, the machine and drive glide to a new value with this time constant.
const GLIDE_MS: f32 = 20.0;
/// Digital: a new time is reached by crossfading to a second read head.
const XFADE_MS: f32 = 40.0;
/// Tape and BBD: a new time is glided to (the pitch bends) like a motor with inertia: critically
/// damped (this time constant), its speed and its acceleration limited, so the pitch never
/// jumps: at most `MAX_SLEW` samples per sample (a fifth over or an octave under), reached over
/// `ACCEL_MS` at the least.
const TIME_GLIDE_MS: f64 = 50.0;
const MAX_SLEW: f64 = 0.5;
const ACCEL_MS: f64 = 30.0;
/// The modulation's depth glides slower: a fast change of depth is a pitch blip.
const DEPTH_GLIDE_MS: f32 = 300.0;
/// The diffusion's allpasses per side (each side's total 6 ms, taken off the line's read).
const DIFF_MS: [[f32; 4]; 2] = [[0.71, 1.13, 1.69, 2.47], [0.79, 1.07, 1.83, 2.31]];
const DIFF_MAX: f32 = 0.65;
/// A line is never read closer than this to its write position (the loop's block, and margin).
const MIN_READ: f32 = (CONTROL + 8) as f32;
const DUCK_ATTACK_MS: f32 = 2.0;
/// The BBD's virtual stage count: its clock, and so its bandwidth, falls as its time grows.
const BBD_STAGES: f32 = 8192.0;
/// Each side's loop is held under this (soft): no setting can blow up.
const SAFE: f32 = 1.0;

/// The machine inside the loop.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    /// Clean: no saturation, no losses; a time change crossfades.
    #[default]
    Digital,
    /// Saturates softly, loses the highest highs (9 kHz) and the deep lows (35 Hz) each pass,
    /// wows and flutters; a time change glides (the pitch bends).
    Tape,
    /// A bucket brigade: darker the longer its time (its clock falls), saturates harder, a
    /// triangle chorus; a time change glides.
    Bbd,
}

impl Mode {
    pub const ALL: [Self; 3] = [Self::Digital, Self::Tape, Self::Bbd];
    pub const NAMES: [&'static str; 3] = ["Digital", "Tape", "BBD"];

    pub fn from_index(i: usize) -> Self {
        Self::ALL[i.min(Self::ALL.len() - 1)]
    }

    pub fn index(self) -> usize {
        self as usize
    }

    /// The saturation's hardness at no drive (0: none).
    fn hardness(self) -> f32 {
        [0.0, 1.0, 1.6][self.index()]
    }

    fn hp_hz(self) -> f32 {
        [0.0, 35.0, 80.0][self.index()]
    }

    /// The two lowpass poles' frequencies (`None`: no pole) for a line `time_s` long.
    fn lp_hz(self, time_s: f32) -> [Option<f32>; 2] {
        match self {
            Mode::Digital => [None, None],
            Mode::Tape => [Some(9000.0), None],
            Mode::Bbd => {
                let fc = 0.2 * BBD_STAGES / time_s.max(1e-3);
                [Some(fc), Some(fc)]
            }
        }
    }

    /// The modulation's depth at 100 %, ms.
    fn mod_ms(self) -> f32 {
        [3.0, 2.5, 4.0][self.index()]
    }
}

/// One tap: an echo line with its own feedback.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TapSettings {
    pub on: bool,
    /// MIN_TIME_MS…MAX_TIME_MS (a synced note is resolved to ms by the plugin).
    pub time_ms: f32,
    /// The first echo's level, dB.
    pub level_db: f32,
    /// 0…1: each echo against the one before (at a frequency where the loop is flat).
    pub feedback: f32,
    /// −1…1: where the tap's echoes sit.
    pub pan: f32,
    /// The echoes alternate sides (the input, summed to mono, enters the left line).
    pub pingpong: bool,
}

impl Default for TapSettings {
    fn default() -> Self {
        Self { on: false, time_ms: 500.0, level_db: 0.0, feedback: 0.4, pan: 0.0, pingpong: false }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DelaySettings {
    pub taps: [TapSettings; TAPS],
    pub mode: Mode,
    /// 0…1.
    pub drive: f32,
    /// 0…1.
    pub mod_depth: f32,
    pub mod_rate: f32,
    /// 0…1.
    pub diffusion: f32,
    /// 0…1.5 (M/S on the echoes).
    pub width: f32,
    pub duck_db: f32,
    pub duck_thresh_db: f32,
    pub duck_release_ms: f32,
    pub eq: [BandSettings; LOOP_BANDS],
}

impl Default for DelaySettings {
    fn default() -> Self {
        let mut taps = [TapSettings::default(); TAPS];
        taps[0].on = true;
        Self {
            taps,
            mode: Mode::Digital,
            drive: 0.0,
            mod_depth: 0.0,
            mod_rate: 0.8,
            diffusion: 0.0,
            width: 1.0,
            duck_db: 0.0,
            duck_thresh_db: -30.0,
            duck_release_ms: 300.0,
            eq: [BandSettings::default(); LOOP_BANDS],
        }
    }
}

/// A one-pole lowpass's coefficient (`y += a (x − y)`); 1 (none) at or above 0.45 fs.
fn one_pole(fc: Option<f32>, fs: f32) -> f32 {
    match fc {
        Some(f) if f < 0.45 * fs => 1.0 - (-std::f32::consts::TAU * f.max(1.0) / fs).exp(),
        _ => 1.0,
    }
}

/// `|a / (1 − (1 − a) e^{−jw})|`.
fn one_pole_mag(a: f64, w: f64) -> f64 {
    let b = 1.0 - a;
    a / (1.0 - 2.0 * b * w.cos() + b * b).sqrt()
}

/// `|1 − a / (1 − (1 − a) e^{−jw})|` (the highpass that subtracts a one-pole lowpass).
fn hp_mag(a: f64, w: f64) -> f64 {
    let b = 1.0 - a;
    // 1 − a/(1 − b z⁻¹) = (1 − b z⁻¹ − a)/(1 − b z⁻¹) = b (1 − z⁻¹)/(1 − b z⁻¹)  (a + b = 1).
    let num = b * (2.0 - 2.0 * w.cos()).sqrt();
    num / (1.0 - 2.0 * b * w.cos() + b * b).sqrt()
}

/// The loop's gain at `freq` for tap `tap` (linear): the loop EQ and the machine's losses (the
/// small-signal path: drive and the machine's saturation leave quiet signals alone). Echo `k`
/// (from 1) of a tap is `level × loop^k × feedback^(k−1)`.
pub fn loop_gain(s: &DelaySettings, tap: usize, freq: f64, fs: f64) -> f64 {
    let eq_db: f64 = s.eq.iter().filter(|b| b.enabled).map(|b| analog_response_db(b, freq)).sum();
    let w = std::f64::consts::TAU * freq / fs;
    let fsf = fs as f32;
    let time_s = s.taps[tap].time_ms.clamp(MIN_TIME_MS, MAX_TIME_MS) * 0.001;
    let [p1, p2] = s.mode.lp_hz(time_s);
    let hp = if s.mode.hp_hz() > 0.0 { hp_mag(f64::from(one_pole(Some(s.mode.hp_hz()), fsf)), w) } else { 1.0 };
    10f64.powf(eq_db / 20.0) * hp * one_pole_mag(f64::from(one_pole(p1, fsf)), w) * one_pole_mag(f64::from(one_pole(p2, fsf)), w)
}

/// Tap `tap`'s echoes for a mono input of unit amplitude at `freq`: `out[k]` is echo `k + 1`'s
/// amplitude on the left and the right (its pan and ping-pong in it).
pub fn echoes(s: &DelaySettings, tap: usize, freq: f64, fs: f64, out: &mut [[f32; 2]]) {
    let t = &s.taps[tap];
    let h = loop_gain(s, tap, freq, fs);
    let level = 10f64.powf(f64::from(t.level_db) / 20.0);
    let (gl, gr) = pan_gains(t.pan);
    let mut e = if t.pingpong { [h, 0.0] } else { [h, h] };
    for o in out.iter_mut() {
        let (m, side) = (0.5 * (e[0] + e[1]), 0.5 * (e[0] - e[1]));
        *o = [(level * (m * f64::from(gl) + side)).abs() as f32, (level * (m * f64::from(gr) - side)).abs() as f32];
        let fb = f64::from(t.feedback) * h;
        e = if t.pingpong { [e[1] * fb, e[0] * fb] } else { [e[0] * fb, e[1] * fb] };
    }
}

/// The echoes' placement: the mid's gains to the left and right (constant power, unity at the
/// centre); the side is kept.
fn pan_gains(pan: f32) -> (f32, f32) {
    let th = std::f32::consts::FRAC_PI_4 * (1.0 + pan.clamp(-1.0, 1.0));
    (std::f32::consts::SQRT_2 * th.cos(), std::f32::consts::SQRT_2 * th.sin())
}

/// The machine's saturation: `tanh(k x) / k` (unity for quiet signals), none at `k` = 0.
#[inline]
fn saturate(x: f32, k: f32) -> f32 {
    if k < 1e-3 {
        x
    } else {
        (k * x).tanh() / k
    }
}

/// Holds a loop under 2 × SAFE, transparent under SAFE (slope-continuous at the knee).
#[inline]
fn safety(x: f32) -> f32 {
    let a = x.abs();
    if a <= SAFE {
        x
    } else {
        (SAFE + (a - SAFE).tanh() * SAFE).copysign(x)
    }
}

/// A value gliding per control step: where it was and where it goes this step.
#[derive(Debug, Clone, Copy, Default)]
struct Step {
    from: f32,
    to: f32,
}

impl Step {
    fn jump(v: f32) -> Self {
        Self { from: v, to: v }
    }

    /// The next step toward `target` (one-pole, `k` per step), or straight there on a jump.
    fn next(&mut self, target: f32, k: f32, jump: bool) {
        self.from = if jump { target } else { self.to };
        self.to = if jump { target } else { self.to + (target - self.to) * k };
    }

    #[inline]
    fn at(&self, t: f32) -> f32 {
        self.from + (self.to - self.from) * t
    }
}

/// One tap's line, its loop and its glides.
struct TapLine {
    lines: [Delay; 2],
    diff: [[Allpass; 4]; 2],
    /// The diffusion's delay per side, samples.
    ap_total: [f32; 2],
    eq: [BandProcessor; LOOP_BANDS],
    /// Filter states per side: the highpass's lowpass, the two lowpass poles.
    hp: [f32; 2],
    lp: [[f32; 2]; 2],
    /// The time (samples): the read head, and the head faded to.
    time: f64,
    time_to: f64,
    /// The glide's speed, samples per sample.
    vel: f64,
    fade_left: u32,
    gin: Step,
    level: Step,
    fb: Step,
    pp: Step,
    gl: Step,
    gr: Step,
    a_lp: [Step; 2],
    idle: bool,
}

impl TapLine {
    fn new(fs: f32, max: usize) -> Self {
        let ap_len = |ms: f32| ((ms * 0.001 * fs).round() as usize).max(1);
        let diff = std::array::from_fn(|c| std::array::from_fn(|k| Allpass::new(ap_len(DIFF_MS[c][k]))));
        let ap_total = std::array::from_fn(|c| DIFF_MS[c].iter().map(|&ms| ap_len(ms) as f32).sum());
        Self {
            lines: [Delay::new(max), Delay::new(max)],
            diff,
            ap_total,
            eq: std::array::from_fn(|_| {
                let mut p = BandProcessor::new(f64::from(fs), EQ_SMOOTH_MS);
                p.set_structure_fade_ms(EQ_FADE_MS);
                p
            }),
            hp: [0.0; 2],
            lp: [[0.0; 2]; 2],
            time: 0.0,
            time_to: 0.0,
            vel: 0.0,
            fade_left: 0,
            gin: Step::default(),
            level: Step::default(),
            fb: Step::default(),
            pp: Step::default(),
            gl: Step::default(),
            gr: Step::default(),
            a_lp: [Step::jump(1.0); 2],
            idle: true,
        }
    }

    fn clear(&mut self) {
        for l in &mut self.lines {
            l.reset();
        }
        for side in &mut self.diff {
            for ap in side.iter_mut() {
                ap.line.reset();
            }
        }
        self.hp = [0.0; 2];
        self.lp = [[0.0; 2]; 2];
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct DelayTelemetry {
    /// The deepest ducking, dB (≤ 0).
    pub duck_db: f32,
}

pub struct DelayLine {
    fs: f32,
    taps: [TapLine; TAPS],
    /// Per-step one-pole coefficients.
    k_glide: f32,
    /// The glide: its rate (per sample) and its largest acceleration.
    glide_w: f64,
    max_acc: f64,
    xfade: u32,
    /// The machine as weights over the modes (they glide), drive's hardness, the highpass, the
    /// modulation's depth (samples) and the diffusion.
    weights: [f32; 3],
    hard: Step,
    a_hp: Step,
    depth: Step,
    diff_g: Step,
    width: Step,
    mode_now: Mode,
    /// Modulation: a sine/triangle phase per side, the tape's wander per side, the flutter.
    phase: [f32; 2],
    flutter: f32,
    wander: [Wander; 2],
    rate: f32,
    env: f32,
    duck: Step,
    duck_g: f32,
    duck_k: f32,
    step_pos: usize,
    scratch: [[[f32; CONTROL]; 2]; TAPS],
    started: bool,
}

impl DelayLine {
    /// Allocates the lines (MAX_TIME_MS at `fs`, plus the modulation's reach).
    pub fn new(fs: f32, s: &DelaySettings) -> Self {
        let max = ((MAX_TIME_MS + 10.0) * 0.001 * fs) as usize + 64;
        let mut d = Self {
            fs,
            taps: std::array::from_fn(|_| TapLine::new(fs, max)),
            k_glide: 1.0 - (-(CONTROL as f32) / (GLIDE_MS * 0.001 * fs)).exp(),
            glide_w: 1.0 / (TIME_GLIDE_MS * 0.001 * f64::from(fs)),
            max_acc: MAX_SLEW / (ACCEL_MS * 0.001 * f64::from(fs)),
            xfade: (XFADE_MS * 0.001 * fs) as u32,
            weights: [1.0, 0.0, 0.0],
            hard: Step::default(),
            a_hp: Step::default(),
            depth: Step::default(),
            diff_g: Step::default(),
            width: Step::jump(1.0),
            mode_now: Mode::Digital,
            phase: [0.0, 0.25],
            flutter: 0.0,
            wander: [Wander::new(0x9E37_79B9), Wander::new(0x85EB_CA6B)],
            rate: 0.0,
            env: 0.0,
            duck: Step::jump(1.0),
            duck_g: 1.0,
            duck_k: 1.0 - (-1.0 / (0.003 * fs)).exp(),
            step_pos: 0,
            scratch: [[[0.0; CONTROL]; 2]; TAPS],
            started: false,
        };
        d.reset(s);
        d
    }

    pub fn sample_rate(&self) -> f32 {
        self.fs
    }

    /// Silences every line and jumps every glide to `s`.
    pub fn reset(&mut self, s: &DelaySettings) {
        for t in &mut self.taps {
            t.clear();
            t.idle = true;
        }
        self.phase = [0.0, 0.25];
        self.flutter = 0.0;
        self.env = 0.0;
        self.duck_g = 1.0;
        self.step_pos = 0;
        self.started = false;
        self.control(s, true, true);
    }

    fn time_samples(&self, t: &TapSettings) -> f32 {
        t.time_ms.clamp(MIN_TIME_MS, MAX_TIME_MS) * 0.001 * self.fs
    }

    /// One control step: every glide's next target.
    fn control(&mut self, s: &DelaySettings, stereo: bool, jump: bool) {
        let fs = self.fs;
        let k = self.k_glide;
        let one_hot: [f32; 3] = std::array::from_fn(|i| f32::from(u8::from(i == s.mode.index())));
        for (w, t) in self.weights.iter_mut().zip(one_hot) {
            *w = if jump { t } else { *w + (t - *w) * k };
        }
        self.mode_now = s.mode;
        let w = self.weights;
        let hard: f32 = Mode::ALL.iter().zip(w).map(|(m, w)| w * m.hardness()).sum::<f32>() + 6.0 * s.drive.clamp(0.0, 1.0);
        self.hard.next(hard, k, jump);
        let a_hp: f32 = Mode::ALL.iter().zip(w).map(|(m, w)| w * if m.hp_hz() > 0.0 { one_pole(Some(m.hp_hz()), fs) } else { 0.0 }).sum();
        self.a_hp.next(a_hp, k, jump);
        let depth: f32 = Mode::ALL.iter().zip(w).map(|(m, w)| w * m.mod_ms()).sum::<f32>() * s.mod_depth.clamp(0.0, 1.0) * 0.001 * fs;
        self.depth.next(depth, 1.0 - (-(CONTROL as f32) / (DEPTH_GLIDE_MS * 0.001 * fs)).exp(), jump);
        self.diff_g.next(DIFF_MAX * s.diffusion.clamp(0.0, 1.0), k, jump);
        self.width.next(s.width.clamp(0.0, 1.5), k, jump);
        self.rate = s.mod_rate.max(0.01) / fs;
        for wd in &mut self.wander {
            wd.step = self.rate;
        }
        for (i, line) in self.taps.iter_mut().enumerate() {
            let t = &s.taps[i];
            let target = t.time_ms.clamp(MIN_TIME_MS, MAX_TIME_MS) * 0.001 * fs;
            let starting = line.idle && t.on;
            if jump || starting {
                line.time = f64::from(target);
                line.time_to = f64::from(target);
                line.vel = 0.0;
                line.fade_left = 0;
            }
            let lj = jump || starting;
            let on = f32::from(u8::from(t.on));
            // A tap that starts comes in from silence.
            if starting && !jump {
                line.gin = Step::jump(0.0);
                line.level = Step::jump(0.0);
            }
            line.gin.next(on, k, jump);
            line.level.next(on * 10f32.powf(t.level_db / 20.0), k, jump);
            line.fb.next(t.feedback.clamp(0.0, 1.0), k, lj);
            line.pp.next(if t.pingpong && stereo { 1.0 } else { 0.0 }, k, lj);
            let (gl, gr) = pan_gains(t.pan);
            line.gl.next(gl, k, lj);
            line.gr.next(gr, k, lj);
            let time_s = line.time as f32 / fs;
            for p in 0..2 {
                let a: f32 = Mode::ALL.iter().zip(w).map(|(m, w)| w * one_pole(m.lp_hz(time_s)[p], fs)).sum();
                line.a_lp[p].next(a, k, lj);
            }
            if t.on {
                line.idle = false;
            }
        }
        let over = crate::gain_to_db(self.env.max(1e-9)) - s.duck_thresh_db;
        self.duck.next(crate::db_to_gain(-over.clamp(0.0, s.duck_db.max(0.0))), 1.0, jump);
    }

    /// Writes the echoes of `input` (one or two channels) into `wet` (as many). Never allocates.
    #[allow(clippy::needless_range_loop)] // the taps' parallel state, by side
    pub fn process(&mut self, input: [&[f32]; 2], wet: [&mut [f32]; 2], stereo: bool, s: &DelaySettings) -> DelayTelemetry {
        let [wl, wr] = wet;
        let n = input[0].len().min(wl.len()).min(if stereo { input[1].len().min(wr.len()) } else { usize::MAX });
        let fs = self.fs;
        let att = 1.0 - (-1.0 / (DUCK_ATTACK_MS * 0.001 * fs)).exp();
        let rel = 1.0 - (-1.0 / (s.duck_release_ms.max(1.0) * 0.001 * fs)).exp();
        let mut tel = DelayTelemetry::default();
        let mut start = 0;
        while start < n {
            if self.step_pos == 0 {
                self.control(s, stereo, !self.started);
                self.started = true;
            }
            let len = (n - start).min(CONTROL - self.step_pos);
            let p0 = self.step_pos;
            let frac = |j: usize| (p0 + j) as f32 / CONTROL as f32;
            // The modulation per side and sample (shared by every tap: one machine).
            let mut modu = [[0.0f32; CONTROL]; 2];
            let w = self.weights;
            for j in 0..len {
                let d = self.depth.at(frac(j));
                self.flutter = (self.flutter + 9.0 * self.rate).fract();
                let fl = (std::f32::consts::TAU * self.flutter).sin();
                for c in 0..2 {
                    self.phase[c] = (self.phase[c] + self.rate).fract();
                    let ph = self.phase[c];
                    let sine = (std::f32::consts::TAU * ph).sin();
                    let tri = 4.0 * (ph - 0.5).abs() - 1.0;
                    let tape = 0.85 * self.wander[c].next() + 0.15 * fl;
                    modu[c][j] = d * (w[0] * sine + w[1] * tape + w[2] * tri);
                }
            }
            let glides = self.mode_now != Mode::Digital;
            let (gw, max_acc, xfade) = (self.glide_w, self.max_acc, self.xfade);
            let diff_g = self.diff_g;
            let (hard, a_hp) = (self.hard, self.a_hp);
            for (ti, line) in self.taps.iter_mut().enumerate() {
                if line.idle {
                    continue;
                }
                let target = s.taps[ti].time_ms.clamp(MIN_TIME_MS, MAX_TIME_MS) * 0.001 * fs;
                let buf = &mut self.scratch[ti];
                // Reads: everything this step reads was written before it.
                for j in 0..len {
                    if line.fade_left > 0 {
                        line.fade_left -= 1;
                        if line.fade_left == 0 {
                            line.time = line.time_to;
                        }
                    } else if glides {
                        let d = f64::from(target) - line.time;
                        if d.abs() < 1e-4 && line.vel.abs() < 1e-7 {
                            line.time = f64::from(target);
                            line.vel = 0.0;
                        } else {
                            let acc = (gw * gw * d - 2.0 * gw * line.vel).clamp(-max_acc, max_acc);
                            line.vel = (line.vel + acc).clamp(-MAX_SLEW, MAX_SLEW);
                            line.time += line.vel;
                        }
                    } else if (f64::from(target) - line.time).abs() > 1e-3 {
                        line.vel = 0.0;
                        line.time_to = f64::from(target);
                        line.fade_left = xfade.max(1);
                    }
                    let x = if line.fade_left > 0 { 1.0 - line.fade_left as f32 / xfade.max(1) as f32 } else { 0.0 };
                    let (ga, gb) = ((std::f32::consts::FRAC_PI_2 * x).cos(), (std::f32::consts::FRAC_PI_2 * x).sin());
                    for c in 0..2 {
                        let read = |t: f64| {
                            let t = t as f32;
                            let base = t - line.ap_total[c];
                            let m = modu[c][j].max(MIN_READ - base);
                            line.lines[c].read((base + m - j as f32 - 1.0).max(1.0))
                        };
                        let mut v = read(line.time);
                        if line.fade_left > 0 {
                            v = v * ga + read(line.time_to) * gb;
                        }
                        buf[c][j] = v;
                    }
                }
                // The loop EQ, on the step's block.
                {
                    let [bl, br] = buf;
                    for (p, b) in line.eq.iter_mut().zip(&s.eq) {
                        p.set_target(&BandSettings { placement: Placement::Stereo, dynamics: Dynamics::default(), ..*b });
                        p.process(&mut [&mut bl[..len], &mut br[..len]]);
                    }
                }
                // The machine, the diffusion, the feedback and the output.
                for j in 0..len {
                    let t = frac(j);
                    let (k, ah) = (hard.at(t), a_hp.at(t));
                    let (a1, a2) = (line.a_lp[0].at(t), line.a_lp[1].at(t));
                    let g = diff_g.at(t);
                    let mut e = [0.0f32; 2];
                    for c in 0..2 {
                        let mut x = buf[c][j];
                        line.hp[c] += ah * (x - line.hp[c]);
                        x -= line.hp[c];
                        line.lp[0][c] += a1 * (x - line.lp[0][c]);
                        line.lp[1][c] += a2 * (line.lp[0][c] - line.lp[1][c]);
                        x = saturate(line.lp[1][c], k);
                        for ap in &mut line.diff[c] {
                            x = ap.process(x, g);
                        }
                        e[c] = safety(x);
                    }
                    let (fb, pp) = (line.fb.at(t), line.pp.at(t));
                    let gin = line.gin.at(t);
                    let xl = input[0][start + j];
                    let xr = if stereo { input[1][start + j] } else { xl };
                    let mono = 0.5 * (xl + xr);
                    let in_l = (1.0 - pp) * xl + pp * mono;
                    let in_r = (1.0 - pp) * xr;
                    line.lines[0].write(gin * in_l + fb * ((1.0 - pp) * e[0] + pp * e[1]));
                    line.lines[1].write(gin * in_r + fb * ((1.0 - pp) * e[1] + pp * e[0]));
                    let lv = line.level.at(t);
                    let (m, side) = (0.5 * (e[0] + e[1]), 0.5 * (e[0] - e[1]));
                    buf[0][j] = lv * (m * line.gl.at(t) + side);
                    buf[1][j] = lv * (m * line.gr.at(t) - side);
                }
            }
            // The taps' sum, its width and ducking.
            for j in 0..len {
                let t = frac(j);
                let (mut l, mut r) = (0.0f32, 0.0f32);
                for (ti, line) in self.taps.iter().enumerate() {
                    if !line.idle {
                        l += self.scratch[ti][0][j];
                        r += self.scratch[ti][1][j];
                    }
                }
                let i = start + j;
                let xl = input[0][i];
                let xr = if stereo { input[1][i] } else { xl };
                let level = xl.abs().max(xr.abs());
                self.env += (level - self.env) * if level > self.env { att } else { rel };
                self.duck_g += (self.duck.at(t) - self.duck_g) * self.duck_k;
                let duck = self.duck_g;
                tel.duck_db = tel.duck_db.min(crate::gain_to_db(duck));
                if stereo {
                    let wd = self.width.at(t);
                    let (m, side) = (0.5 * (l + r), 0.5 * (l - r) * wd);
                    wl[i] = (m + side) * duck;
                    wr[i] = (m - side) * duck;
                } else {
                    wl[i] = 0.5 * (l + r) * duck;
                }
            }
            self.step_pos += len;
            if self.step_pos == CONTROL {
                self.step_pos = 0;
                // A tap turned off and faded out stops running and forgets its echoes.
                for (ti, line) in self.taps.iter_mut().enumerate() {
                    if !line.idle && !s.taps[ti].on && line.level.to < 1e-5 && line.gin.to < 1e-5 {
                        line.idle = true;
                        line.clear();
                        for p in &mut line.eq {
                            p.reset(&BandSettings::default());
                        }
                    }
                }
            }
            start += len;
        }
        tel
    }

    /// The time tap `tap` reads now, ms (tests and telemetry).
    pub fn time_ms(&self, tap: usize) -> f32 {
        (self.taps[tap].time / f64::from(self.fs) * 1000.0) as f32
    }

    #[doc(hidden)]
    pub fn target_samples(&self, t: &TapSettings) -> f32 {
        self.time_samples(t)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pan_is_unity_at_the_centre_and_constant_power() {
        let (l, r) = pan_gains(0.0);
        assert!((l - 1.0).abs() < 1e-6 && (r - 1.0).abs() < 1e-6);
        for p in [-1.0, -0.3, 0.5, 1.0] {
            let (l, r) = pan_gains(p);
            assert!((l * l + r * r - 2.0).abs() < 1e-5);
        }
    }

    #[test]
    fn the_safety_is_transparent_below_its_knee_and_bounded() {
        assert_eq!(safety(0.9), 0.9);
        assert!(safety(1e6) <= 2.0 * SAFE);
        assert!((safety(1.0001) - 1.0001).abs() < 1e-3);
    }

    #[test]
    fn the_highpass_magnitude_matches_its_formula() {
        let a = f64::from(one_pole(Some(35.0), 48_000.0));
        // Far above its corner a highpass passes; at DC it stops.
        assert!((hp_mag(a, std::f64::consts::TAU * 5000.0 / 48_000.0) - 1.0).abs() < 5e-3);
        assert!(hp_mag(a, 1e-6) < 1e-3);
    }
}
