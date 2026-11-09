//! Spectral noise reduction (Mite DeNoise; docs/design/denoise-brief.md): a gate per frequency
//! on an [`Stft`], against a noise floor that is either learned from a stretch of noise only or
//! tracked all the time (adaptive).
//!
//! Per frame, per bin `k` (powers on the spectra's scale, a full-scale sine on a bin = 1):
//!
//! ```text
//! P[k]   = mean over channels of |X|²              linked: one gain for both, the image holds
//! N[k]   = the learned floor, or the tracked one    (minimum statistics, below)
//! T[k]   = N[k] · threshold · emphasis(f)            dB above the floor, tilted about 1 kHz
//! D[k]   = P smoothed over two frames and ±w bins    (w grows with frequency: SMOOTHING)
//! g*[k]  = −reduction · knee((T − D) / KNEE_DB)       0 at or over T, full reduction KNEE_DB under it
//! g[k]   = g* smoothed over ±w bins, then in time     ATTACK as it opens, RELEASE as it closes
//! Y[k]   = 10^(g/20) · X[k]                           LISTEN: (1 − 10^(g/20)) · X, what was taken
//! ```
//!
//! **Adaptive floor** (Martin's minimum statistics, simplified): the power smoothed over a few
//! frames, its minimum over the last `ADAPT_S` seconds (in `ADAPT_SUBS` sub-windows so it can
//! rise again), times a bias for the minimum of a noisy estimate sitting under its mean.
//!
//! The learned profile is kept as a power density (per Hz) at [`PROFILE_POINTS`] log-spaced
//! frequencies, so it carries across sample rates and frame sizes, and the plugin saves it.
//!
//! Gains change once per frame; the 75 %-overlapped windows crossfade them, so nothing steps.
//! Leaving every gain at 1 (bypass, or nothing to reduce) gives the input back exactly, delayed
//! by the frame size. Allocation-free after [`Denoiser::new`].

use realfft::num_complex::Complex;

use crate::stft::Stft;

/// The learned profile's points, log-spaced over [`PROFILE_HZ`].
pub const PROFILE_POINTS: usize = 512;
pub const PROFILE_HZ: (f64, f64) = (20.0, 20_000.0);
/// The display's points, log-spaced over the same span.
pub const DISPLAY_POINTS: usize = 256;
/// The level the display and the profile use for nothing, dB.
pub const SILENT_DB: f32 = -150.0;
/// How far under the threshold full reduction is reached, dB.
pub const KNEE_DB: f64 = 6.0;
/// The adaptive floor's memory, seconds, in this many sub-windows.
pub const ADAPT_S: f64 = 1.5;
pub const ADAPT_SUBS: usize = 4;
/// The most the minimum is lifted. Each bin's own lift is its long-term mean over its minimum
/// (how far its scatter reaches down), up to this: random noise scatters deep (the smoothing
/// falls fast, `ADAPT_SMOOTH`), a steady hum hardly at all. A single fixed lift (13.8, set on
/// white noise) put the floor 11 dB over a hum (2026-11-09). The cap stops a voice over the
/// noise from lifting the floor with it.
const ADAPT_BIAS: f64 = 20.0;
/// The adaptive tracker's power smoothing per frame, rising and falling. Falling fast matters:
/// at 0.8 both ways a voice 48 dB over the noise fell only 19 dB through a 0.23 s pause, so its
/// harmonics never reached the floor and the tracker took the voice for noise (2026-11-09).
const ADAPT_SMOOTH: (f64, f64) = (0.8, 0.4);
/// Frames a learned profile needs at least.
pub const MIN_LEARN_FRAMES: usize = 8;

/// The log-spaced frequency of point `i` of `n` over [`PROFILE_HZ`].
pub fn point_freq(i: usize, n: usize) -> f64 {
    PROFILE_HZ.0 * (PROFILE_HZ.1 / PROFILE_HZ.0).powf(i as f64 / (n - 1) as f64)
}

/// The frame size at `sample_rate`: about 43 ms (2048 at 44.1/48 kHz), so the bins stay ~23 Hz
/// apart at every rate.
pub fn frame_size(sample_rate: f64) -> usize {
    if sample_rate <= 50_000.0 {
        2048
    } else if sample_rate <= 100_000.0 {
        4096
    } else {
        8192
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DenoiseSettings {
    /// How far above the noise floor a sound passes untouched, dB.
    pub threshold_db: f32,
    /// The most any frequency is turned down, dB (≥ 0).
    pub reduction_db: f32,
    /// How fast a frequency opens when a sound rises out of the noise, and closes after, ms.
    pub attack_ms: f32,
    pub release_ms: f32,
    /// Smoothing across frequencies, 0…1 (0: each bin on its own; 1: a third of an octave).
    pub smoothing: f32,
    /// The threshold's tilt about 1 kHz: dB at 10 kHz (and minus that at 100 Hz).
    pub emphasis_db: f32,
    /// Track the floor all the time instead of using the learned one.
    pub adaptive: bool,
    /// Hear only what is taken away.
    pub listen: bool,
    pub bypass: bool,
}

impl Default for DenoiseSettings {
    fn default() -> Self {
        Self { threshold_db: 6.0, reduction_db: 12.0, attack_ms: 10.0, release_ms: 150.0, smoothing: 0.4, emphasis_db: 0.0, adaptive: true, listen: false, bypass: false }
    }
}

/// Where a point reads the bins: from bin `k0` to `k1` (exclusive) averaged, or, when it falls
/// between two bins, `k0` and `k0 + 1` mixed by `frac`.
#[derive(Clone, Copy, Debug)]
struct PointMap {
    k0: usize,
    k1: usize,
    frac: f64,
}

fn point_maps(n: usize, bins: usize, bin_hz: f64) -> Vec<PointMap> {
    (0..n)
        .map(|i| {
            let f = point_freq(i, n);
            let ratio = (PROFILE_HZ.1 / PROFILE_HZ.0).powf(0.5 / (n - 1) as f64);
            let (lo, hi) = (f / ratio / bin_hz, f * ratio / bin_hz);
            if hi - lo >= 1.0 {
                let k0 = (lo.ceil() as usize).min(bins - 1);
                PointMap { k0, k1: (hi.floor() as usize + 1).clamp(k0 + 1, bins), frac: 0.0 }
            } else {
                let x = (f / bin_hz).min((bins - 2) as f64);
                PointMap { k0: x.floor() as usize, k1: 0, frac: x.fract() }
            }
        })
        .collect()
}

fn read_point(m: &PointMap, v: &[f64]) -> f64 {
    if m.k1 > m.k0 {
        v[m.k0..m.k1].iter().sum::<f64>() / (m.k1 - m.k0) as f64
    } else {
        v[m.k0] * (1.0 - m.frac) + v[m.k0 + 1] * m.frac
    }
}

fn db(p: f64) -> f64 {
    10.0 * p.max(1e-30).log10()
}

/// What one frame drew, for the editor: dB at [`DISPLAY_POINTS`] frequencies.
#[derive(Clone, Debug)]
pub struct Display {
    pub input: Vec<f32>,
    pub output: Vec<f32>,
    /// The floor in use and the threshold over it.
    pub noise: Vec<f32>,
    pub threshold: Vec<f32>,
    /// The gain each frequency got, dB (≤ 0).
    pub gain: Vec<f32>,
}

/// Everything a frame works on (apart from the STFT, so its callback can borrow it).
struct Core {
    sample_rate: f64,
    hop: usize,
    bin_hz: f64,
    /// 1 / (the full-scale magnitude)².
    scale: f64,
    learned: Vec<f64>,
    has_learned: bool,
    learning: bool,
    learn_acc: Vec<f64>,
    learn_frames: usize,
    /// Adaptive: the smoothed power, this sub-window's minimum, the last sub-windows' minima
    /// (`bins × ADAPT_SUBS`), frames into the sub-window, the next slot, and the floor.
    smoothed: Vec<f64>,
    sub_min: Vec<f64>,
    ring: Vec<f64>,
    sub_frames: usize,
    sub_len: usize,
    slot: usize,
    tracked: Vec<f64>,
    /// The power's long-term mean (over `ADAPT_S`).
    mean: Vec<f64>,
    frames: usize,
    det: Vec<f64>,
    gain_db: Vec<f64>,
    work: Vec<f64>,
    prefix: Vec<f64>,
    power: Vec<f64>,
    out_power: Vec<f64>,
    floor: Vec<f64>,
    thresh: Vec<f64>,
    display_maps: Vec<PointMap>,
    display: Display,
    /// Frames since the display was last read.
    pub fresh: bool,
}

impl Core {
    /// Each bin's half-width for smoothing at `amount` (at least one bin each side): up to a
    /// third of an octave in all, ±(2^(1/6) − 1) of the frequency.
    fn width(k: usize, amount: f64) -> usize {
        1 + (k as f64 * amount * 0.122).round() as usize
    }

    /// `v` averaged over ±width bins, into `out`.
    fn smooth(prefix: &mut [f64], v: &[f64], out: &mut [f64], width: impl Fn(usize) -> usize + Copy) {
        let n = v.len();
        prefix[0] = 0.0;
        for k in 0..n {
            prefix[k + 1] = prefix[k] + v[k];
        }
        for (k, o) in out.iter_mut().enumerate() {
            let w = width(k);
            let (a, b) = (k.saturating_sub(w), (k + w + 1).min(n));
            *o = (prefix[b] - prefix[a]) / (b - a) as f64;
        }
    }

    fn frame(&mut self, left: &mut [Complex<f64>], right: &mut [Complex<f64>], s: &DenoiseSettings, mono: bool) {
        let bins = left.len();
        for k in 0..bins {
            let p = if mono { left[k].norm_sqr() } else { 0.5 * (left[k].norm_sqr() + right[k].norm_sqr()) };
            self.power[k] = p * self.scale;
        }
        self.frames += 1;
        if self.learning {
            for (a, p) in self.learn_acc.iter_mut().zip(&self.power) {
                *a += p;
            }
            self.learn_frames += 1;
        }
        // The adaptive floor runs always, so switching to it finds it ready.
        for k in 0..bins {
            let p = self.power[k];
            let a = if p > self.smoothed[k] { ADAPT_SMOOTH.0 } else { ADAPT_SMOOTH.1 };
            self.smoothed[k] = if self.frames == 1 { p } else { a * self.smoothed[k] + (1.0 - a) * p };
            self.sub_min[k] = self.sub_min[k].min(self.smoothed[k]);
        }
        self.sub_frames += 1;
        if self.sub_frames >= self.sub_len {
            for k in 0..bins {
                self.ring[k * ADAPT_SUBS + self.slot] = self.sub_min[k];
                self.sub_min[k] = f64::INFINITY;
            }
            self.slot = (self.slot + 1) % ADAPT_SUBS;
            self.sub_frames = 0;
        }
        let mean_c = (self.hop as f64 / (ADAPT_S * self.sample_rate)).min(1.0);
        for k in 0..bins {
            self.mean[k] = if self.frames == 1 { self.power[k] } else { self.mean[k] + (self.power[k] - self.mean[k]) * mean_c };
            let held = self.ring[k * ADAPT_SUBS..(k + 1) * ADAPT_SUBS].iter().fold(self.sub_min[k], |m, &v| m.min(v));
            self.tracked[k] = if held.is_finite() && held > 0.0 { held * (self.mean[k] / held).clamp(1.0, ADAPT_BIAS) } else { 0.0 };
        }
        let have_floor = s.adaptive || self.has_learned;
        for k in 0..bins {
            self.floor[k] = if s.adaptive { self.tracked[k] } else if self.has_learned { self.learned[k] } else { 0.0 };
        }
        // The threshold: the floor, the offset, the tilt about 1 kHz.
        let tilt = f64::from(s.emphasis_db);
        for k in 0..bins {
            let f = (k as f64 * self.bin_hz).clamp(PROFILE_HZ.0, PROFILE_HZ.1);
            let t_db = f64::from(s.threshold_db) + tilt * (f / 1000.0).log10();
            self.thresh[k] = self.floor[k] * 10f64.powf(t_db / 10.0);
        }
        // The detector: two frames, then across frequencies.
        for k in 0..bins {
            self.det[k] = 0.5 * self.det[k] + 0.5 * self.power[k];
        }
        let amount = f64::from(s.smoothing.clamp(0.0, 1.0));
        let widths = |k: usize| Self::width(k, amount);
        let mut work = std::mem::take(&mut self.work);
        let mut prefix = std::mem::take(&mut self.prefix);
        Self::smooth(&mut prefix, &self.det, &mut work, widths);
        // The target gain per bin, then smoothed across frequencies (in dB).
        let reduction = f64::from(s.reduction_db.max(0.0));
        let mut target = std::mem::take(&mut self.out_power);
        for k in 0..bins {
            target[k] = if have_floor && self.thresh[k] > 0.0 {
                let under = (db(self.thresh[k]) - db(work[k])) / KNEE_DB;
                let t = under.clamp(0.0, 1.0);
                -reduction * t * t * (3.0 - 2.0 * t)
            } else {
                0.0
            };
        }
        Self::smooth(&mut prefix, &target, &mut work, widths);
        self.out_power = target;
        self.prefix = prefix;
        // In time: open at the attack, close at the release.
        let coef = |ms: f32| 1.0 - (-(self.hop as f64) / (0.001 * f64::from(ms.max(0.1)) * self.sample_rate)).exp();
        let (up, down) = (coef(s.attack_ms), coef(s.release_ms));
        for (g, &to) in self.gain_db.iter_mut().zip(&work) {
            *g += (to - *g) * if to > *g { up } else { down };
        }
        self.work = work;
        for k in 0..bins {
            let g = 10f64.powf(self.gain_db[k] / 20.0);
            let m = if s.bypass {
                1.0
            } else if s.listen {
                1.0 - g
            } else {
                g
            };
            left[k] *= m;
            right[k] *= m;
            self.out_power[k] = self.power[k] * m * m;
        }
        // The display.
        for (i, map) in self.display_maps.iter().enumerate() {
            let d = &mut self.display;
            d.input[i] = db(read_point(map, &self.power)) as f32;
            d.output[i] = db(read_point(map, &self.out_power)) as f32;
            let floor = read_point(map, &self.floor);
            d.noise[i] = if floor > 0.0 { db(floor) as f32 } else { SILENT_DB };
            let th = read_point(map, &self.thresh);
            d.threshold[i] = if th > 0.0 { db(th) as f32 } else { SILENT_DB };
            d.gain[i] = read_point(map, &self.gain_db) as f32;
        }
        self.fresh = true;
    }
}

pub struct Denoiser {
    stft: Stft,
    core: Core,
    /// The right channel for mono input (its output is dropped).
    spare: Vec<f32>,
}

impl Denoiser {
    /// Allocates.
    pub fn new(sample_rate: f64) -> Self {
        let size = frame_size(sample_rate);
        let stft = Stft::new(size);
        let bins = stft.bins();
        let hop = stft.hop();
        let bin_hz = sample_rate / size as f64;
        let full = stft.full_scale();
        let sub_len = ((ADAPT_S * sample_rate / hop as f64) / ADAPT_SUBS as f64).round().max(1.0) as usize;
        let zeros = || vec![0.0; bins];
        let core = Core {
            sample_rate,
            hop,
            bin_hz,
            scale: 1.0 / (full * full),
            learned: zeros(),
            has_learned: false,
            learning: false,
            learn_acc: zeros(),
            learn_frames: 0,
            smoothed: zeros(),
            sub_min: vec![f64::INFINITY; bins],
            ring: vec![f64::INFINITY; bins * ADAPT_SUBS],
            sub_frames: 0,
            sub_len,
            slot: 0,
            tracked: zeros(),
            mean: zeros(),
            frames: 0,
            det: zeros(),
            gain_db: zeros(),
            work: zeros(),
            prefix: vec![0.0; bins + 1],
            power: zeros(),
            out_power: zeros(),
            floor: zeros(),
            thresh: zeros(),
            display_maps: point_maps(DISPLAY_POINTS, bins, bin_hz),
            display: Display { input: vec![SILENT_DB; DISPLAY_POINTS], output: vec![SILENT_DB; DISPLAY_POINTS], noise: vec![SILENT_DB; DISPLAY_POINTS], threshold: vec![SILENT_DB; DISPLAY_POINTS], gain: vec![0.0; DISPLAY_POINTS] },
            fresh: false,
        };
        Self { stft, core, spare: vec![0.0; 1024] }
    }

    pub fn sample_rate(&self) -> f64 {
        self.core.sample_rate
    }

    /// The delay, samples: one frame.
    pub fn latency(&self) -> usize {
        self.stft.latency()
    }

    /// Clears the signal and the envelopes (the learned profile stays; the tracked floor starts
    /// over).
    pub fn reset(&mut self) {
        self.stft.reset();
        let c = &mut self.core;
        for v in [&mut c.smoothed, &mut c.tracked, &mut c.mean, &mut c.det, &mut c.gain_db] {
            v.fill(0.0);
        }
        c.sub_min.fill(f64::INFINITY);
        c.ring.fill(f64::INFINITY);
        c.sub_frames = 0;
        c.slot = 0;
        c.frames = 0;
    }

    /// Starts (true) or ends (false) learning. Ending after at least [`MIN_LEARN_FRAMES`] frames
    /// makes the average the learned profile and returns true.
    pub fn set_learning(&mut self, on: bool) -> bool {
        let c = &mut self.core;
        if on == c.learning {
            return false;
        }
        c.learning = on;
        if on {
            c.learn_acc.fill(0.0);
            c.learn_frames = 0;
            return false;
        }
        if c.learn_frames < MIN_LEARN_FRAMES {
            return false;
        }
        let n = c.learn_frames as f64;
        for (l, a) in c.learned.iter_mut().zip(&c.learn_acc) {
            *l = a / n;
        }
        c.has_learned = true;
        true
    }

    pub fn learning(&self) -> bool {
        self.core.learning
    }

    pub fn has_profile(&self) -> bool {
        self.core.has_learned
    }

    /// The learned profile as a power density, dB per Hz, at [`PROFILE_POINTS`] frequencies
    /// (`SILENT_DB` with none). Allocation-free: writes into `out`.
    pub fn profile(&self, out: &mut [f32]) {
        let c = &self.core;
        if !c.has_learned {
            out.fill(SILENT_DB);
            return;
        }
        let n = out.len();
        for (i, o) in out.iter_mut().enumerate() {
            let f = point_freq(i, n);
            let x = (f / c.bin_hz).min((c.learned.len() - 2) as f64);
            let (k, t) = (x.floor() as usize, x.fract());
            // Over the bins this point spans at high frequencies, the mean; else interpolated.
            let ratio = (PROFILE_HZ.1 / PROFILE_HZ.0).powf(0.5 / (n - 1) as f64);
            let (lo, hi) = (f / ratio / c.bin_hz, f * ratio / c.bin_hz);
            let p = if hi - lo >= 1.0 {
                let k0 = (lo.ceil() as usize).min(c.learned.len() - 1);
                let k1 = (hi.floor() as usize + 1).clamp(k0 + 1, c.learned.len());
                c.learned[k0..k1].iter().sum::<f64>() / (k1 - k0) as f64
            } else {
                c.learned[k] * (1.0 - t) + c.learned[k + 1] * t
            };
            *o = if p > 0.0 { db(p / c.bin_hz) as f32 } else { SILENT_DB };
        }
    }

    /// Sets the learned profile from [`Denoiser::profile`]'s points (none if they are all
    /// silent). Allocation-free.
    pub fn set_profile(&mut self, points: &[f32]) {
        let c = &mut self.core;
        let n = points.len();
        if n < 2 || points.iter().all(|&p| p <= SILENT_DB + 1.0) {
            c.has_learned = false;
            c.learned.fill(0.0);
            return;
        }
        let span = (PROFILE_HZ.1 / PROFILE_HZ.0).ln();
        for (k, l) in c.learned.iter_mut().enumerate() {
            let f = (k as f64 * c.bin_hz).clamp(PROFILE_HZ.0, PROFILE_HZ.1);
            let x = (f / PROFILE_HZ.0).ln() / span * (n - 1) as f64;
            let (i, t) = ((x.floor() as usize).min(n - 2), x - x.floor().min((n - 2) as f64));
            let d = f64::from(points[i]) * (1.0 - t) + f64::from(points[i + 1]) * t;
            *l = 10f64.powf(d / 10.0) * c.bin_hz;
        }
        c.has_learned = true;
    }

    /// The last frame's picture, and whether a frame came since the last call.
    pub fn display(&mut self) -> (&Display, bool) {
        let fresh = std::mem::take(&mut self.core.fresh);
        (&self.core.display, fresh)
    }

    /// Runs a block through in place (`right` `None` for mono). Allocation-free for blocks of any
    /// length.
    pub fn process(&mut self, left: &mut [f32], right: Option<&mut [f32]>, s: &DenoiseSettings) {
        let (stft, core) = (&mut self.stft, &mut self.core);
        match right {
            Some(r) => stft.process(left, r, &mut |a, b| core.frame(a, b, s, false)),
            None => {
                for chunk in left.chunks_mut(self.spare.len()) {
                    let spare = &mut self.spare[..chunk.len()];
                    spare.fill(0.0);
                    stft.process(chunk, spare, &mut |a, b| core.frame(a, b, s, true));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FS: f64 = 48_000.0;

    fn noise(n: usize, seed: u64, rms: f32) -> Vec<f32> {
        let mut s = seed;
        // A sum of four uniforms: near Gaussian, its RMS set.
        (0..n)
            .map(|_| {
                let mut v = 0.0;
                for _ in 0..4 {
                    s ^= s << 13;
                    s ^= s >> 7;
                    s ^= s << 17;
                    v += (s >> 40) as f32 / (1u64 << 24) as f32 - 0.5;
                }
                v * rms * 1.732
            })
            .collect()
    }

    fn rms(v: &[f32]) -> f32 {
        (v.iter().map(|x| x * x).sum::<f32>() / v.len() as f32).sqrt()
    }

    fn run(d: &mut Denoiser, l: &mut [f32], r: &mut [f32], s: &DenoiseSettings) {
        for (a, b) in l.chunks_mut(300).zip(r.chunks_mut(300)) {
            d.process(a, Some(b), s);
        }
    }

    /// The level of a sine at `f` in `v` (correlation).
    fn tone_level(v: &[f32], f: f64) -> f64 {
        let (mut c, mut s) = (0.0, 0.0);
        for (i, &x) in v.iter().enumerate() {
            let w = std::f64::consts::TAU * f * i as f64 / FS;
            c += f64::from(x) * w.cos();
            s += f64::from(x) * w.sin();
        }
        2.0 * (c * c + s * s).sqrt() / v.len() as f64
    }

    #[test]
    fn with_nothing_to_reduce_it_is_the_input_delayed() {
        for s in [DenoiseSettings { bypass: true, ..DenoiseSettings::default() }, DenoiseSettings { adaptive: false, ..DenoiseSettings::default() }] {
            let mut d = Denoiser::new(FS);
            let x = noise(30_000, 3, 0.1);
            let (mut l, mut r) = (x.clone(), x.clone());
            run(&mut d, &mut l, &mut r, &s);
            let lat = d.latency();
            let worst = (2 * lat..x.len()).map(|i| (l[i] - x[i - lat]).abs()).fold(0.0f32, f32::max);
            assert!(worst < 1e-6, "{s:?}: {worst}");
        }
    }

    #[test]
    fn a_learned_floor_takes_the_noise_down_by_the_reduction() {
        let s = DenoiseSettings { adaptive: false, ..DenoiseSettings::default() };
        let mut d = Denoiser::new(FS);
        // Learn a second of noise.
        let x = noise(FS as usize * 4, 7, 0.05);
        d.set_learning(true);
        let (mut l, mut r) = (x[..48_000].to_vec(), x[..48_000].to_vec());
        run(&mut d, &mut l, &mut r, &s);
        assert!(d.set_learning(false));
        assert!(d.has_profile());
        let (mut l, mut r) = (x[48_000..].to_vec(), x[48_000..].to_vec());
        run(&mut d, &mut l, &mut r, &s);
        let down = 20.0 * (rms(&l[48_000..]) / rms(&x[96_000..])).log10();
        // Measured 2026-11-09: −11.0 dB for 12.
        assert!((down + 12.0).abs() < 1.5, "{down:.1} dB");
    }

    #[test]
    fn a_tone_over_the_noise_stays_and_the_noise_around_it_goes() {
        let s = DenoiseSettings { adaptive: false, ..DenoiseSettings::default() };
        let mut d = Denoiser::new(FS);
        let n = noise(FS as usize * 5, 11, 0.02);
        d.set_learning(true);
        let (mut l, mut r) = (n[..48_000].to_vec(), n[..48_000].to_vec());
        run(&mut d, &mut l, &mut r, &s);
        d.set_learning(false);
        let tone: Vec<f32> = (0..n.len() - 48_000).map(|i| 0.3 * (std::f64::consts::TAU * 1000.0 * i as f64 / FS).sin() as f32).collect();
        let x: Vec<f32> = tone.iter().zip(&n[48_000..]).map(|(a, b)| a + b).collect();
        let (mut l, mut r) = (x.clone(), x.clone());
        run(&mut d, &mut l, &mut r, &s);
        let lat = d.latency();
        let body = &l[48_000..];
        let level = tone_level(body, 1000.0);
        assert!((20.0 * (level / 0.3).log10()).abs() < 0.5, "tone {:.2} dB", 20.0 * (level / 0.3).log10());
        // The rest: the output less the tone, against the noise in.
        let resid: Vec<f32> = (48_000..l.len()).map(|i| l[i] - tone[i - lat]).collect();
        let down = 20.0 * (rms(&resid) / rms(&n[96_000..])).log10();
        assert!(down < -8.0, "noise only {down:.1} dB down");
    }

    #[test]
    fn the_adaptive_floor_finds_the_noise_without_learning() {
        let s = DenoiseSettings::default();
        let mut d = Denoiser::new(FS);
        let x = noise(FS as usize * 6, 5, 0.05);
        let (mut l, mut r) = (x.clone(), x.clone());
        run(&mut d, &mut l, &mut r, &s);
        let down = 20.0 * (rms(&l[48_000 * 4..]) / rms(&x[48_000 * 4..])).log10();
        // Measured 2026-11-09: −10.8 dB (the tracked floor moves a little per bin).
        assert!((down + 12.0).abs() < 2.5, "{down:.1} dB");
        // Its floor sits on the learned one.
        let mut e = Denoiser::new(FS);
        e.set_learning(true);
        let (mut l, mut r) = (x.clone(), x.clone());
        run(&mut e, &mut l, &mut r, &DenoiseSettings { adaptive: false, ..s });
        e.set_learning(false);
        let (shown, _) = d.display();
        let tracked: f32 = shown.noise[40..220].iter().sum::<f32>() / 180.0;
        let mut l2 = x[..48_000].to_vec();
        let mut r2 = l2.clone();
        run(&mut e, &mut l2, &mut r2, &DenoiseSettings { adaptive: false, ..s });
        let (shown, _) = e.display();
        let learned: f32 = shown.noise[40..220].iter().sum::<f32>() / 180.0;
        // Measured 2026-11-09: 0.2 dB apart.
        assert!((tracked - learned).abs() < 1.0, "tracked {tracked:.1} dB, learned {learned:.1} dB");
    }

    #[test]
    fn listen_is_what_was_taken() {
        let mut a = Denoiser::new(FS);
        let mut b = Denoiser::new(FS);
        let x: Vec<f32> = noise(48_000 * 3, 9, 0.05).iter().enumerate().map(|(i, v)| v + 0.2 * (i as f32 * 0.07).sin()).collect();
        let (mut l1, mut r1) = (x.clone(), x.clone());
        run(&mut a, &mut l1, &mut r1, &DenoiseSettings::default());
        let (mut l2, mut r2) = (x.clone(), x.clone());
        run(&mut b, &mut l2, &mut r2, &DenoiseSettings { listen: true, ..DenoiseSettings::default() });
        let lat = a.latency();
        let worst = (2 * lat..x.len()).map(|i| (l1[i] + l2[i] - x[i - lat]).abs()).fold(0.0f32, f32::max);
        assert!(worst < 1e-5, "{worst}");
    }

    #[test]
    fn the_profile_carries_across_sample_rates() {
        let mut d = Denoiser::new(48_000.0);
        let x = noise(48_000 * 2, 13, 0.05);
        d.set_learning(true);
        let (mut l, mut r) = (x.clone(), x.clone());
        run(&mut d, &mut l, &mut r, &DenoiseSettings { adaptive: false, ..DenoiseSettings::default() });
        d.set_learning(false);
        let mut points = vec![0.0f32; PROFILE_POINTS];
        d.profile(&mut points);
        // The same noise at 96 kHz has the same density per Hz: the same points.
        let mut e = Denoiser::new(96_000.0);
        e.set_profile(&points);
        let mut back = vec![0.0f32; PROFILE_POINTS];
        e.profile(&mut back);
        let worst = points.iter().zip(&back).skip(20).take(450).map(|(a, b)| (a - b).abs()).fold(0.0f32, f32::max);
        assert!(worst < 1.0, "{worst} dB");
        // Mono runs too.
        let mut m = x.clone();
        e.process(&mut m, None, &DenoiseSettings::default());
        assert!(m.iter().all(|v| v.is_finite()));
    }

    #[test]
    fn the_adaptive_floor_does_not_take_a_voice_with_pauses_for_noise() {
        // A 180 Hz tone with harmonics, 0.54 s on and 0.23 s off, over noise: in the pauses the
        // floor must find the noise, so the tone's harmonics stay far over it.
        let mut d = Denoiser::new(FS);
        let n = noise(FS as usize * 6, 21, 0.01);
        let x: Vec<f32> = n
            .iter()
            .enumerate()
            .map(|(i, v)| {
                let t = i as f64 / FS;
                let voice = if (t * 1.3).fract() < 0.7 { (1..8).map(|h| (std::f64::consts::TAU * 180.0 * h as f64 * t).sin() * 0.15 / h as f64).sum::<f64>() } else { 0.0 };
                v + voice as f32
            })
            .collect();
        let (mut l, mut r) = (x.clone(), x.clone());
        run(&mut d, &mut l, &mut r, &DenoiseSettings::default());
        let (shown, _) = d.display();
        // The floor at the third harmonic (540 Hz) against the floor an octave off it.
        let at = |hz: f64| {
            let i = ((hz / 20.0).ln() / 1000f64.ln() * (DISPLAY_POINTS - 1) as f64).round() as usize;
            shown.noise[i]
        };
        let (on, off) = (at(540.0), at(780.0));
        // Measured 2026-11-09: 3.1 dB over (at 0.8 both ways, the floor rode the voice).
        assert!(on - off < 6.0, "floor at the harmonic {on:.1} dB, beside it {off:.1} dB");
    }

    #[test]
    fn the_adaptive_floor_sits_on_a_steady_hum_not_over_it() {
        // A 50 Hz hum 30 dB over faint noise: the floor at 50 Hz is the hum's own level.
        let mut d = Denoiser::new(FS);
        let n = noise(FS as usize * 4, 31, 0.001);
        let x: Vec<f32> = n.iter().enumerate().map(|(i, v)| v + 0.05 * (std::f64::consts::TAU * 50.0 * i as f64 / FS).sin() as f32).collect();
        let (mut l, mut r) = (x.clone(), x.clone());
        run(&mut d, &mut l, &mut r, &DenoiseSettings::default());
        let (shown, _) = d.display();
        let i = ((50.0f64 / 20.0).ln() / 1000f64.ln() * (DISPLAY_POINTS - 1) as f64).round() as usize;
        let over = shown.noise[i] - shown.input[i];
        // Measured 2026-11-09: 0.2 dB under (a fixed lift of 13.8 put it 11 dB over).
        assert!(over.abs() < 2.0, "the floor {over:.1} dB off the hum");
    }
}
