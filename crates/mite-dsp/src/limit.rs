//! The brickwall limiter behind Mite Limit (docs/design/limit-brief.md §1). The law comes from
//! Nova's Limiter.h, chosen because it can be proved never to overshoot; the code is new.
//!
//! Per channel, with `v[n]` the peak the detector sees and the ceiling at 1 (the plugin scales
//! in and out, so a gliding ceiling stays exact):
//!
//! ```text
//! g[n] = min(1, 1 / v[n])              the gain each sample needs
//! m[n] = min(g[n … n+L−1])             sliding minimum over the lookahead (audio delayed L−1)
//! s[n] = box_A₂(box_A₁(m))[n]           two boxcars, A₁ + A₂ − 1 = L: an S-shaped ramp
//! r[n] = release(s)[n] ≤ s[n]           the styles shape the way back up, never above s
//!                                       (fast and slow envelopes joined by a smooth max: no corner)
//! y[n] = x[n − L + 1] · r[n]
//! ```
//!
//! Every `m[j]` inside the two averages at `n` has `j ≥ n − (L − 1)`, so its window holds the
//! sample being output, so `s ≤ g` of that sample and `|y| ≤ 1`. The ramp spans the whole
//! lookahead (Nova's "attack is the lookahead"): a shorter one would only reach its depth early and
//! hold flat until the peak. The attack control is the slow envelope's (§1.3). The gains are kept as
//! integers in units of 2⁻²⁴, rounded down, so the averages are exact sums with no drift and the
//! proof holds bit for bit: `|y|` exceeds 1 by at most the rounding of `1 / v` (tested).
//!
//! True peak: the detector interpolates the stream (`loudness::TruePeak`) and judges each sample
//! on the largest magnitude on either side of it, so the gain is down before a peak between
//! samples; it adds `TruePeak::DELAY + 1` samples of delay.
//!
//! Nothing here allocates outside [`Limiter::new`].

use crate::loudness::TruePeak;

/// Gains as integers: 1.0 is `ONE`.
const ONE: u32 = 1 << 24;
const ONE_F: f32 = ONE as f32;
/// Below this gain reduction (dB) the release counts as at rest.
const REST_DB: f32 = -1e-4;

/// A limiter style: how the gain comes back up (brief §1.3). A row of numbers, as Comp's
/// Characters.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Style {
    /// × the attack control (the slow envelope's attack).
    pub attack: f32,
    /// × the release control.
    pub release: f32,
    /// How much the slow envelope holds the gain down (0 = a single release).
    pub slow: f32,
    /// The slow envelope's release, × the (scaled) release.
    pub slow_ratio: f32,
    /// Program dependence: how much sustained reduction slows the fast release.
    pub program: f32,
}

/// The styles in their frozen order (append only): Transparent, Punch, Dense, Bus, Safe, Custom.
pub const STYLES: [Style; 6] = [
    Style { attack: 1.0, release: 1.0, slow: 0.3, slow_ratio: 4.0, program: 0.3 },
    Style { attack: 0.6, release: 0.6, slow: 0.6, slow_ratio: 6.0, program: 0.4 },
    Style { attack: 0.4, release: 0.35, slow: 0.85, slow_ratio: 8.0, program: 0.9 },
    Style { attack: 1.5, release: 1.8, slow: 0.6, slow_ratio: 4.0, program: 0.5 },
    Style { attack: 2.0, release: 2.5, slow: 0.5, slow_ratio: 4.0, program: 0.2 },
    Style { attack: 1.0, release: 1.0, slow: 0.5, slow_ratio: 6.0, program: 0.5 },
];
/// The style whose slow share is the `slow` dial.
pub const CUSTOM: usize = 5;

/// Sliding minimum over the last `window` values (a monotonic deque in a fixed ring).
#[derive(Clone, Debug)]
struct SlidingMin {
    val: Vec<u32>,
    pos: Vec<u64>,
    head: usize,
    count: usize,
    t: u64,
    window: usize,
}

impl SlidingMin {
    fn new(max_window: usize) -> Self {
        let cap = max_window + 2;
        Self { val: vec![ONE; cap], pos: vec![0; cap], head: 0, count: 0, t: 0, window: 1 }
    }

    fn reset(&mut self, window: usize) {
        self.head = 0;
        self.count = 0;
        self.t = 0;
        self.window = window.clamp(1, self.val.len() - 2);
    }

    #[inline]
    fn push(&mut self, x: u32) -> u32 {
        let cap = self.val.len();
        while self.count > 0 && self.val[(self.head + self.count - 1) % cap] >= x {
            self.count -= 1;
        }
        let back = (self.head + self.count) % cap;
        self.val[back] = x;
        self.pos[back] = self.t;
        self.count += 1;
        while self.pos[self.head] + self.window as u64 <= self.t {
            self.head = (self.head + 1) % cap;
            self.count -= 1;
        }
        self.t += 1;
        self.val[self.head]
    }
}

/// Boxcar mean over the last `window` values, exact (integer sum), rounded down.
#[derive(Clone, Debug)]
struct BoxAvg {
    buf: Vec<u32>,
    w: usize,
    /// Values held (up to the window).
    n: usize,
    sum: u64,
    window: usize,
}

impl BoxAvg {
    fn new(max_window: usize) -> Self {
        Self { buf: vec![ONE; max_window + 1], w: 0, n: 0, sum: 0, window: 1 }
    }

    fn reset(&mut self, window: usize) {
        self.buf.fill(ONE);
        self.w = 0;
        self.n = 0;
        self.sum = 0;
        self.window = window.clamp(1, self.buf.len() - 1);
    }

    /// The value pushed `back` pushes ago (0 = the newest).
    #[inline]
    fn ago(&self, back: usize) -> u32 {
        let cap = self.buf.len();
        self.buf[(self.w + 2 * cap - 1 - back) % cap]
    }

    #[inline]
    fn push(&mut self, x: u32) -> u32 {
        let cap = self.buf.len();
        if self.n == self.window {
            self.sum -= u64::from(self.ago(self.window - 1));
        } else {
            self.n += 1;
        }
        self.buf[self.w] = x;
        self.w = (self.w + 1) % cap;
        self.sum += u64::from(x);
        (self.sum / self.n as u64) as u32
    }
}

/// Fast and slow release envelopes in dB, both at or under the guaranteed gain (§1.3).
#[derive(Clone, Copy, Debug, Default)]
struct Release {
    fast: f32,
    slow: f32,
}

/// Release coefficients for one setting, at the engine rate.
#[derive(Clone, Copy, Debug)]
struct ReleaseCoeffs {
    fast: f32,
    slow_up: f32,
    slow_down: f32,
    slow: f32,
    program: f32,
}

impl ReleaseCoeffs {
    fn new(style: &Style, attack_ms: f32, release_ms: f32, slow: f32, rate: f32) -> Self {
        let coef = |ms: f32| 1.0 - (-1.0 / (0.001 * ms.max(0.01) * rate)).exp();
        let rel = release_ms * style.release;
        Self { fast: coef(rel), slow_up: coef(rel * style.slow_ratio), slow_down: coef(attack_ms * style.attack), slow, program: style.program }
    }
}

impl Release {
    /// Takes the guaranteed gain `s_db` (≤ 0) and returns the gain to apply (≤ `s_db`).
    #[inline]
    fn step(&mut self, s_db: f32, c: &ReleaseCoeffs) -> f32 {
        // Program dependence: sustained reduction (the slow envelope) slows the fast release.
        let scale = 1.0 / (1.0 + c.program * (-self.slow / 3.0));
        if s_db <= self.fast {
            self.fast = s_db;
        } else {
            self.fast = (self.fast + (s_db - self.fast) * c.fast * scale).min(s_db);
        }
        let k = if s_db < self.slow { c.slow_down } else { c.slow_up * scale };
        self.slow = (self.slow + (s_db - self.slow) * k).min(0.0);
        // The deeper of the two reductions, without a corner where they cross: a smooth maximum
        // that rounds only within half the shallower depth of the crossing (exact elsewhere, 0 at
        // rest), and is never shallower than either, so the gain stays at or under `fast` and so
        // under the guaranteed one.
        let (a, b) = (-self.fast, -(c.slow * self.slow));
        let (deep, gap, w) = (a.max(b), (a - b).abs(), 0.5 * a.min(b));
        let round = if gap < w { (w - gap) * (w - gap) / (4.0 * w) } else { 0.0 };
        -(deep + round)
    }

    fn at_rest(&self) -> bool {
        self.fast > REST_DB && self.slow > REST_DB
    }
}

/// A delay line of `len` samples.
#[derive(Clone, Debug)]
struct Delay {
    buf: Vec<f32>,
    w: usize,
}

impl Delay {
    fn new(max: usize) -> Self {
        Self { buf: vec![0.0; max + 1], w: 0 }
    }

    fn reset(&mut self) {
        self.buf.fill(0.0);
        self.w = 0;
    }

    /// Pushes `x`, returns the sample from `len` pushes ago.
    #[inline]
    fn run(&mut self, x: f32, len: usize) -> f32 {
        let cap = self.buf.len();
        self.buf[self.w] = x;
        let out = self.buf[(self.w + cap - len) % cap];
        self.w = (self.w + 1) % cap;
        out
    }
}

/// The settings that move freely while running.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Settings {
    /// Attack, ms: how fast sustained reduction builds the slow envelope (× the style's scale).
    pub attack_ms: f32,
    /// Release, ms.
    pub release_ms: f32,
    pub style: usize,
    /// Custom's slow share, 0…1.
    pub slow: f32,
    /// Stereo links, 0…1: transients (the attack) and release.
    pub link_transients: f32,
    pub link_release: f32,
}

impl Default for Settings {
    fn default() -> Self {
        Self { attack_ms: 30.0, release_ms: 100.0, style: 0, slow: 0.5, link_transients: 1.0, link_release: 1.0 }
    }
}

/// One stereo (or mono) limiter at the engine rate, its ceiling at 1.
#[derive(Clone, Debug)]
pub struct Limiter {
    rate: f32,
    max_lookahead: usize,
    lookahead: usize,
    true_peak: bool,
    detectors: [TruePeak; 2],
    last_peak: [f32; 2],
    min: [SlidingMin; 2],
    box1: [BoxAvg; 2],
    box2: [BoxAvg; 2],
    release: [Release; 2],
    coeffs: ReleaseCoeffs,
    settings: Settings,
    delay: [Delay; 2],
}

impl Limiter {
    /// Allocates. `rate` is the engine rate (the base rate × the oversampling), `max_lookahead`
    /// the longest lookahead in samples at it, `phases` the true-peak interpolation.
    pub fn new(rate: f32, max_lookahead: usize, phases: usize) -> Self {
        let l = max_lookahead.max(1);
        let tp = TruePeak::new(phases);
        let settings = Settings::default();
        let mut lim = Self {
            rate,
            max_lookahead: l,
            lookahead: 1,
            true_peak: false,
            detectors: [tp.clone(), tp],
            last_peak: [0.0; 2],
            min: [SlidingMin::new(l), SlidingMin::new(l)],
            box1: [BoxAvg::new(l), BoxAvg::new(l)],
            box2: [BoxAvg::new(l), BoxAvg::new(l)],
            release: [Release::default(); 2],
            coeffs: ReleaseCoeffs::new(&STYLES[0], settings.attack_ms, settings.release_ms, 0.0, rate),
            settings,
            delay: [Delay::new(l + TruePeak::DELAY + 1), Delay::new(l + TruePeak::DELAY + 1)],
        };
        lim.configure(1, false, settings);
        lim
    }

    /// Sets the lookahead (samples, 1 = none: a clipper) and the detector, and clears the state.
    /// No allocation; the plugin crossfades two limiters across a change.
    pub fn configure(&mut self, lookahead: usize, true_peak: bool, settings: Settings) {
        self.lookahead = lookahead.clamp(1, self.max_lookahead);
        self.true_peak = true_peak && self.detectors[0].phases() > 1;
        self.settings = settings;
        let (a1, a2) = self.boxes();
        for c in 0..2 {
            self.detectors[c].reset();
            self.min[c].reset(self.lookahead);
            self.box1[c].reset(a1);
            self.box2[c].reset(a2);
            self.release[c] = Release::default();
            self.delay[c].reset();
        }
        self.last_peak = [0.0; 2];
        self.coeffs = self.release_coeffs(&settings);
    }

    /// Clears the state, keeping the configuration.
    pub fn reset(&mut self) {
        self.configure(self.lookahead, self.true_peak, self.settings);
    }

    /// The delay from input to output, samples at the engine rate.
    pub fn latency(&self) -> usize {
        Self::latency_for(self.lookahead, self.true_peak)
    }

    pub fn latency_for(lookahead: usize, true_peak: bool) -> usize {
        lookahead.max(1) - 1 + if true_peak { TruePeak::DELAY + 1 } else { 0 }
    }

    pub fn lookahead(&self) -> usize {
        self.lookahead
    }

    /// The ramp's two boxcars: `A₁ + A₂ − 1` = the lookahead.
    fn boxes(&self) -> (usize, usize) {
        let a1 = self.lookahead.div_ceil(2);
        (a1, self.lookahead - a1 + 1)
    }

    fn release_coeffs(&self, s: &Settings) -> ReleaseCoeffs {
        let i = s.style.min(STYLES.len() - 1);
        let style = &STYLES[i];
        let slow = if i == CUSTOM { s.slow.clamp(0.0, 1.0) } else { style.slow };
        ReleaseCoeffs::new(style, s.attack_ms, s.release_ms, slow, self.rate)
    }

    /// Updates the free settings (attack, release, style, links). The envelopes carry on from
    /// where they are, so a change never jumps the gain.
    pub fn set(&mut self, settings: Settings) {
        if settings != self.settings {
            self.settings = settings;
            self.coeffs = self.release_coeffs(&settings);
        }
    }

    /// One frame (`right` ignored when `stereo` is false). Returns the output and the gain each
    /// channel got.
    #[inline]
    pub fn process(&mut self, x: [f32; 2], stereo: bool) -> ([f32; 2], [f32; 2]) {
        let chans = if stereo { 2 } else { 1 };
        let mut v = [0.0f32; 2];
        for c in 0..chans {
            v[c] = if self.true_peak {
                let q = self.detectors[c].push(x[c]);
                let both = q.max(self.last_peak[c]);
                self.last_peak[c] = q;
                both
            } else {
                x[c].abs()
            };
        }
        if stereo && self.settings.link_transients > 0.0 {
            let loud = v[0].max(v[1]);
            for p in &mut v {
                *p += (loud - *p) * self.settings.link_transients;
            }
        }
        let mut s = [1.0f32; 2];
        for c in 0..chans {
            let g = if v[c] > 1.0 { ((ONE_F / v[c]) as u32).min(ONE) } else { ONE };
            let m = self.min[c].push(g);
            let b = self.box2[c].push(self.box1[c].push(m));
            s[c] = b as f32 / ONE_F;
        }
        let mut r = s;
        for c in 0..chans {
            let rel = &mut self.release[c];
            if s[c] < 1.0 || !rel.at_rest() {
                let s_db = 20.0 * s[c].max(1e-12).log10();
                let db = rel.step(s_db, &self.coeffs);
                r[c] = (10f32.powf(db / 20.0)).min(s[c]);
            }
        }
        if stereo && self.settings.link_release > 0.0 {
            let low = r[0].min(r[1]);
            for g in &mut r {
                *g += (low - *g) * self.settings.link_release;
            }
        }
        let len = self.latency();
        let mut y = [0.0f32; 2];
        for c in 0..chans {
            y[c] = self.delay[c].run(x[c], len) * r[c];
        }
        (y, r)
    }
}

/// The gain shape probe's burst: how far over the ceiling, and how long, ms.
pub const PROBE_OVER_DB: f32 = 6.0;
pub const PROBE_BURST_MS: f32 = 50.0;
/// How long after the burst the probe runs, ms.
pub const PROBE_TAIL_MS: f32 = 3000.0;

/// The gain the limiter applies (dB, against time in ms from the burst's start) to a probe:
/// a level 6 dB under the ceiling, then [`PROBE_OVER_DB`] over it for [`PROBE_BURST_MS`], then
/// under again: the ramp before the burst, the hold through it, the release after. The same code
/// as the audio, at `rate` (the editor's picture of the gain shape; allocates).
pub fn probe(settings: &Settings, lookahead_ms: f32, rate: f32) -> Vec<(f32, f32)> {
    let look = (lookahead_ms.max(0.0) * 0.001 * rate).round() as usize + 1;
    let mut lim = Limiter::new(rate, look, 1);
    lim.configure(look, false, *settings);
    let ms = |n: usize| n as f32 * 1000.0 / rate;
    let pre = look + (0.002 * rate) as usize;
    let burst = (PROBE_BURST_MS * 0.001 * rate) as usize;
    let tail = (PROBE_TAIL_MS * 0.001 * rate) as usize;
    let (quiet, loud) = (crate::db_to_gain(-6.0), crate::db_to_gain(PROBE_OVER_DB));
    // Settle on the quiet level first, then the burst; the output's time is the input's plus the
    // latency, so the gain is reported at the input's time (burst start = 0).
    let total = pre + burst + tail + lim.latency();
    let mut out = Vec::with_capacity(total);
    for n in 0..total {
        let x = if n >= pre && n < pre + burst { loud } else { quiet };
        let (_, g) = lim.process([x, x], false);
        if let Some(input_n) = n.checked_sub(lim.latency()) {
            out.push((ms(input_n) - ms(pre), crate::gain_to_db(g[0])));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A small seeded generator (xorshift), so the fuzz is repeatable.
    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }
        fn unit(&mut self) -> f32 {
            (self.next() >> 40) as f32 / (1u64 << 24) as f32
        }
        fn range(&mut self, lo: usize, hi: usize) -> usize {
            lo + (self.next() % (hi - lo + 1) as u64) as usize
        }
    }

    /// Music-like test material: noise bursts, a low sine, impulses, near-Nyquist tones, silences,
    /// at levels up to +30 dB over the ceiling.
    fn material(rng: &mut Rng, n: usize) -> Vec<[f32; 2]> {
        let mut out = vec![[0.0f32; 2]; n];
        let mut i = 0;
        while i < n {
            let len = rng.range(8, 4000).min(n - i);
            let level = 10f32.powf((rng.unit() * 50.0 - 20.0) / 20.0);
            let kind = rng.range(0, 4);
            let f = rng.unit() * 0.5;
            for k in 0..len {
                let t = (i + k) as f32;
                let (l, r) = match kind {
                    0 => (rng.unit() * 2.0 - 1.0, rng.unit() * 2.0 - 1.0),
                    1 => ((t * 0.01).sin(), (t * 0.013).sin()),
                    2 => (if k % 97 == 0 { 1.0 } else { 0.0 }, if k % 89 == 0 { -1.0 } else { 0.0 }),
                    3 => ((t * std::f32::consts::PI * (0.9 + 0.1 * f)).sin(), (t * std::f32::consts::TAU * f).cos()),
                    _ => (0.0, 0.0),
                };
                out[i + k] = [l * level, r * level * (0.3 + rng.unit())];
            }
            i += len;
        }
        out
    }

    #[test]
    fn it_never_overshoots_the_ceiling() {
        let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
        let mut lim = Limiter::new(48000.0, 512, 4);
        let mut worst = 0.0f32;
        for case in 0..160 {
            let lookahead = rng.range(1, 512);
            let true_peak = case % 2 == 0;
            let settings = Settings {
                attack_ms: 0.1 + rng.unit() * 100.0,
                release_ms: 1.0 + rng.unit() * 1000.0,
                style: rng.range(0, STYLES.len() - 1),
                slow: rng.unit(),
                link_transients: if case % 3 == 0 { 0.0 } else { rng.unit() },
                link_release: if case % 5 == 0 { 0.0 } else { rng.unit() },
            };
            lim.configure(lookahead, true_peak, settings);
            let x = material(&mut rng, 6000);
            for (k, f) in x.iter().enumerate() {
                // Attack, release, style and links move mid-stream.
                if k == 3000 {
                    let s = Settings { attack_ms: 0.1 + rng.unit() * 100.0, release_ms: 1.0 + rng.unit() * 300.0, style: rng.range(0, 5), ..settings };
                    lim.set(s);
                }
                let (y, _) = lim.process(*f, true);
                worst = worst.max(y[0].abs()).max(y[1].abs());
            }
        }
        assert!(worst <= 1.0 + 1e-6, "peak {worst}");
        assert!(worst > 0.99, "the material should reach the ceiling: {worst}");
    }

    #[test]
    fn below_the_ceiling_it_is_a_pure_delay() {
        for true_peak in [false, true] {
            let mut lim = Limiter::new(48000.0, 256, 4);
            lim.configure(97, true_peak, Settings::default());
            let x: Vec<f32> = (0..4000).map(|i| 0.5 * (i as f32 * 0.07).sin()).collect();
            let d = lim.latency();
            assert_eq!(d, 96 + if true_peak { TruePeak::DELAY + 1 } else { 0 });
            for (i, &v) in x.iter().enumerate() {
                let (y, g) = lim.process([v, -v], true);
                let want = if i >= d { x[i - d] } else { 0.0 };
                assert_eq!(g, [1.0, 1.0]);
                assert_eq!(y, [want, -want], "sample {i}");
            }
        }
    }

    #[test]
    fn the_ramp_is_an_s_curve_over_the_lookahead_that_ends_on_the_peak() {
        // A step to +6 dB: the gain falls over the lookahead, smoothly, and is at −6 dB on the step.
        let lookahead = 120;
        let attack = lookahead;
        let mut lim = Limiter::new(48000.0, 256, 1);
        lim.configure(lookahead, false, Settings { release_ms: 50.0, ..Settings::default() });
        let mut gains = Vec::new();
        for i in 0..1000 {
            let x = if i >= 500 { 2.0 } else { 0.5 };
            gains.push(lim.process([x, x], false).1[0]);
        }
        let step_out = 500 + lim.latency();
        assert!((gains[step_out] - 0.5).abs() < 1e-6, "{}", gains[step_out]);
        assert_eq!(gains[step_out - attack], 1.0);
        // S-shaped: slow at both ends, fastest in the middle, never rising.
        let d: Vec<f32> = (step_out - attack..step_out).map(|i| gains[i] - gains[i + 1]).collect();
        assert!(d.iter().all(|&v| v >= 0.0));
        let mid = d[attack / 2];
        assert!(d[2] < 0.2 * mid && d[attack - 3] < 0.2 * mid, "{} {} {}", d[2], mid, d[attack - 3]);
    }

    #[test]
    fn link_holds_the_image_and_unlinked_channels_are_independent() {
        let mut lim = Limiter::new(48000.0, 64, 1);
        let mut run = |link: f32| {
            lim.configure(32, false, Settings { link_transients: link, link_release: link, ..Settings::default() });
            let mut last = ([0.0; 2], [0.0; 2]);
            for _ in 0..2000 {
                last = lim.process([2.0, 0.5], true);
            }
            last
        };
        let (y, g) = run(1.0);
        assert!((g[0] - g[1]).abs() < 1e-6 && (y[0] - 1.0).abs() < 1e-5);
        let (y, g) = run(0.0);
        assert_eq!(g[1], 1.0);
        assert!((y[0] - 1.0).abs() < 1e-5 && (y[1] - 0.5).abs() < 1e-6);
    }

    #[test]
    fn release_returns_to_unity_and_styles_differ() {
        let rate = 48000.0;
        let recover = |style: usize| {
            let mut lim = Limiter::new(rate, 256, 1);
            lim.configure(96, false, Settings { style, release_ms: 100.0, ..Settings::default() });
            // 2 s at +12 dB, then quiet: how long until the gain is within 0.5 dB of unity.
            for _ in 0..96000 {
                lim.process([4.0, 4.0], false);
            }
            let mut n = 0;
            loop {
                let (_, g) = lim.process([0.1, 0.1], false);
                n += 1;
                if g[0] > 10f32.powf(-0.5 / 20.0) || n > 10 * 48000 {
                    return n as f32 / rate;
                }
            }
        };
        let times: Vec<f32> = (0..STYLES.len()).map(recover).collect();
        assert!(times.iter().all(|&t| t < 10.0), "{times:?}");
        // Safe and Bus come back slower than Transparent; Dense holds longer after sustained
        // reduction than its fast release alone would (the slow envelope).
        assert!(times[4] > times[0] && times[3] > times[0], "{times:?}");
        assert!(times[2] > 0.035 * 3.0, "{times:?}");
    }

    #[test]
    fn true_peak_holds_between_the_samples() {
        // Tones near Nyquist pushed 12 dB over: the output's 4× true peak stays near the ceiling.
        for f in [0.21f32, 0.37, 0.45, 0.49] {
            let mut lim = Limiter::new(48000.0, 256, 4);
            lim.configure(96, true, Settings::default());
            let mut y = Vec::new();
            for i in 0..20000 {
                let v = 4.0 * (i as f32 * std::f32::consts::TAU * f + 0.4).sin();
                y.push(lim.process([v, v], false).0[0]);
            }
            // The meter runs over the whole output; only the steady part counts (a slice's cut
            // would be a step, and its overshoot is not the limiter's).
            let mut meter = TruePeak::new(4);
            let tp = y.iter().enumerate().map(|(i, &v)| (i, meter.push(v))).filter(|&(i, _)| i > 4000 && i + 100 < y.len()).fold(0.0f32, |m, (_, p)| m.max(p));
            let tp = crate::gain_to_db(tp);
            assert!(tp < 0.01, "{f} of the rate: {tp} dBTP");
        }
    }

    #[test]
    fn the_probe_ramps_over_the_lookahead_holds_the_burst_and_comes_back() {
        let rate = 48000.0;
        let s = Settings { release_ms: 50.0, ..Settings::default() };
        let shape = probe(&s, 2.0, rate);
        let at = |t: f32| shape.iter().min_by(|a, b| (a.0 - t).abs().total_cmp(&(b.0 - t).abs())).map_or(0.0, |p| p.1);
        // Untouched before the lookahead, down by the excess on the burst, back after it.
        assert!(at(-2.2).abs() < 1e-3, "{}", at(-2.2));
        assert!(at(-1.0) < -0.5 && at(-1.0) > -5.5, "{}", at(-1.0));
        assert!((at(0.0) + PROBE_OVER_DB).abs() < 0.05, "{}", at(0.0));
        assert!((at(40.0) + PROBE_OVER_DB).abs() < 0.05);
        assert!(at(PROBE_BURST_MS + 2000.0) > -0.2, "{}", at(PROBE_BURST_MS + 2000.0));
        // A longer release comes back later.
        let slow = probe(&Settings { release_ms: 400.0, ..s }, 2.0, rate);
        let slow_at = |t: f32| slow.iter().min_by(|a, b| (a.0 - t).abs().total_cmp(&(b.0 - t).abs())).map_or(0.0, |p| p.1);
        assert!(slow_at(PROBE_BURST_MS + 100.0) < at(PROBE_BURST_MS + 100.0) - 1.0);
        // No lookahead: no ramp, the gain drops on the burst itself.
        let none = probe(&s, 0.0, rate);
        assert!(none.iter().filter(|p| p.0 < 0.0).all(|p| p.1.abs() < 1e-3));
    }
}
