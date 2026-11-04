//! Mite DeEss's engine (docs/design/deess-brief.md §1).
//!
//! **Detection.** The input's top, band-passed between LO and HI (two Butterworth highpass and two
//! lowpass sections: the trigger band), and the whole input, each as RMS over 2 ms. RELATIVE (the
//! default): a sibilant is the band loud against the voice, `band − whole` in dB over THRESHOLD,
//! whatever the voice's level (a loud line is not de-essed more than a quiet one). ABSOLUTE: the
//! band's level over THRESHOLD (dBFS). Under −60 dBFS nothing is a sibilant. Linked: the louder side.
//!
//! **Reduction:** by as much as the band pokes over (a 4 dB soft knee), at most RANGE: in relative
//! mode the band is brought down to the threshold. Attack 0.5 ms (with the lookahead the reduction
//! is already there when the sibilant arrives), RELEASE as set, faster the further the band falls
//! under the threshold (smart release: a vowel after a sibilant comes back clear). Applied as a dynamic **shelf**
//! (above LO), a dynamic **bell** (the trigger band only, the air above it untouched) or **wide**
//! (all of it), redesigned every sample (RBJ: at 0 dB both filters are exactly the identity, so
//! nothing is touched while nothing is reduced — tested bit for bit).
//!
//! **Lookahead:** the audio is delayed 3 ms behind the detection (that is the latency); switching
//! it fades out, changes, fades in (5 ms each). **Listen:** the trigger band alone.

pub const LOOKAHEAD_MS: f32 = 3.0;
const RMS_MS: f32 = 2.0;
const ATTACK_MS: f32 = 0.5;
const KNEE_DB: f32 = 4.0;
const GATE_DB: f32 = -60.0;
const DIP_MS: f32 = 5.0;
const GLIDE_MS: f32 = 20.0;
/// Smart release: the release time divided by `1 + below / SMART_DB`, `below` how far the trigger
/// band sits under the threshold — a vowel after a sibilant (20 dB under) gets it back in a fifth
/// of the time, a sibilant just under the threshold at the set pace. (A plain 60 ms release left
/// 3…8 dB on the next vowel's first 50…100 ms and dulled it.)
const SMART_DB: f32 = 5.0;

/// What a sibilant is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Detection {
    /// The band against the whole voice.
    #[default]
    Relative,
    /// The band's level.
    Absolute,
}

impl Detection {
    pub const NAMES: [&'static str; 2] = ["Relative", "Absolute"];

    pub fn from_index(i: usize) -> Self {
        if i == 1 {
            Self::Absolute
        } else {
            Self::Relative
        }
    }
}

/// What is turned down.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Reduce {
    /// Everything above LO (a high shelf).
    #[default]
    Shelf,
    /// The trigger band only (a bell between LO and HI).
    Band,
    /// The whole signal.
    Wide,
}

impl Reduce {
    pub const NAMES: [&'static str; 3] = ["Shelf", "Band", "Wide"];

    pub fn from_index(i: usize) -> Self {
        [Self::Shelf, Self::Band, Self::Wide][i.min(2)]
    }

    pub fn index(self) -> usize {
        self as usize
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DeEssSettings {
    pub threshold_db: f32,
    pub detection: Detection,
    pub range_db: f32,
    pub release_ms: f32,
    pub lo_hz: f32,
    pub hi_hz: f32,
    pub reduce: Reduce,
    pub lookahead: bool,
    pub listen: bool,
    /// The input as it came (delayed by the lookahead, so switching keeps time).
    pub bypass: bool,
}

impl Default for DeEssSettings {
    fn default() -> Self {
        Self { threshold_db: -15.0, detection: Detection::Relative, range_db: 12.0, release_ms: 60.0, lo_hz: 5000.0, hi_hz: 12_000.0, reduce: Reduce::Shelf, lookahead: true, listen: false, bypass: false }
    }
}

impl DeEssSettings {
    /// The band, ordered and inside the audio range at `fs`.
    fn band(&self, fs: f32) -> (f32, f32) {
        let top = 0.45 * fs;
        let lo = self.lo_hz.clamp(1000.0, top * 0.9);
        let hi = self.hi_hz.clamp(lo * 1.2, top.max(lo * 1.2));
        (lo, hi)
    }
}

/// A biquad's coefficients, normalised (`a0` = 1).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Coeffs {
    b: [f64; 3],
    a: [f64; 2],
}

impl Coeffs {
    const IDENTITY: Self = Self { b: [1.0, 0.0, 0.0], a: [0.0, 0.0] };

    fn butter(fc: f64, fs: f64, high: bool) -> Self {
        let w = std::f64::consts::TAU * fc / fs;
        let (sin, cos) = w.sin_cos();
        let alpha = sin / std::f64::consts::SQRT_2;
        let a0 = 1.0 + alpha;
        let (b0, b1) = if high { ((1.0 + cos) / 2.0, -(1.0 + cos)) } else { ((1.0 - cos) / 2.0, 1.0 - cos) };
        Self { b: [b0 / a0, b1 / a0, b0 / a0], a: [-2.0 * cos / a0, (1.0 - alpha) / a0] }
    }

    /// RBJ high shelf (S = 1) of `gain_db` at `fc`: the identity at 0 dB, exactly.
    fn shelf(fc: f64, fs: f64, gain_db: f64) -> Self {
        let a = 10f64.powf(gain_db / 40.0);
        let w = std::f64::consts::TAU * fc / fs;
        let (sin, cos) = w.sin_cos();
        let alpha = sin / 2.0 * std::f64::consts::SQRT_2;
        let sa = 2.0 * a.sqrt() * alpha;
        let a0 = (a + 1.0) - (a - 1.0) * cos + sa;
        Self {
            b: [a * ((a + 1.0) + (a - 1.0) * cos + sa) / a0, -2.0 * a * ((a - 1.0) + (a + 1.0) * cos) / a0, a * ((a + 1.0) + (a - 1.0) * cos - sa) / a0],
            a: [2.0 * ((a - 1.0) - (a + 1.0) * cos) / a0, ((a + 1.0) - (a - 1.0) * cos - sa) / a0],
        }
    }

    /// RBJ peaking bell of `gain_db` over `lo…hi`: the identity at 0 dB, exactly.
    fn bell(lo: f64, hi: f64, fs: f64, gain_db: f64) -> Self {
        let fc = (lo * hi).sqrt();
        let octaves = (hi / lo).log2();
        let a = 10f64.powf(gain_db / 40.0);
        let w = std::f64::consts::TAU * fc / fs;
        let (sin, cos) = w.sin_cos();
        let alpha = sin * (std::f64::consts::LN_2 / 2.0 * octaves * w / sin).sinh();
        let a0 = 1.0 + alpha / a;
        Self { b: [(1.0 + alpha * a) / a0, -2.0 * cos / a0, (1.0 - alpha * a) / a0], a: [-2.0 * cos / a0, (1.0 - alpha / a) / a0] }
    }

    /// Its magnitude at `freq`, dB.
    pub fn response_db(&self, freq: f64, fs: f64) -> f64 {
        let w = std::f64::consts::TAU * freq / fs;
        let z1 = (w.cos(), -w.sin());
        let z2 = ((2.0 * w).cos(), -(2.0 * w).sin());
        let num = (self.b[0] + self.b[1] * z1.0 + self.b[2] * z2.0, self.b[1] * z1.1 + self.b[2] * z2.1);
        let den = (1.0 + self.a[0] * z1.0 + self.a[1] * z2.0, self.a[0] * z1.1 + self.a[1] * z2.1);
        10.0 * ((num.0 * num.0 + num.1 * num.1) / (den.0 * den.0 + den.1 * den.1)).log10()
    }
}

/// The band's reduction is three bells, one a third of the band each (1.2 × as wide), sharing it:
/// flat across the band and steep outside (one bell over the band took 2.5 dB at 15 kHz and 3 dB
/// at 3.5 kHz for 12 in the band; three take 1.4 at each).
const BAND_BELLS: usize = 3;
const BAND_SHARE: f64 = 1.0 / 1.56;

/// The reduction's sections for `gr_db` (≥ 0) of reduction (unused ones the identity).
pub fn reduction_filter(s: &DeEssSettings, gr_db: f32, fs: f32) -> [Coeffs; BAND_BELLS] {
    let (lo, hi) = s.band(fs);
    let (lo, hi, fs) = (f64::from(lo), f64::from(hi), f64::from(fs));
    let g = -f64::from(gr_db.max(0.0));
    let mut c = [Coeffs::IDENTITY; BAND_BELLS];
    match s.reduce {
        Reduce::Shelf => c[0] = Coeffs::shelf(lo, fs, g),
        Reduce::Band => {
            let octaves = (hi / lo).log2();
            let third = octaves / BAND_BELLS as f64;
            for (k, sec) in c.iter_mut().enumerate() {
                let centre = lo * 2f64.powf(third * (k as f64 + 0.5));
                let half = 0.5 * third * 1.2;
                *sec = Coeffs::bell(centre * 2f64.powf(-half), centre * 2f64.powf(half), fs, g * BAND_SHARE);
            }
        }
        Reduce::Wide => {}
    }
    c
}

/// The processing's response at `freq` for `gr_db` of reduction (what the editor draws).
pub fn response_db(s: &DeEssSettings, gr_db: f32, freq: f64, fs: f32) -> f64 {
    match s.reduce {
        Reduce::Wide => -f64::from(gr_db.max(0.0)),
        _ => reduction_filter(s, gr_db, fs).iter().map(|c| c.response_db(freq, f64::from(fs))).sum(),
    }
}

/// How far over its threshold a reading is, through the soft knee.
pub fn over_db(level_db: f32, threshold_db: f32) -> f32 {
    crate::dynamics::soft_above(level_db - threshold_db, KNEE_DB)
}

#[derive(Debug, Clone, Copy, Default)]
struct State {
    z: [f64; 2],
}

impl State {
    #[inline]
    fn run(&mut self, c: &Coeffs, x: f64) -> f64 {
        let y = c.b[0] * x + self.z[0];
        self.z[0] = c.b[1] * x - c.a[0] * y + self.z[1];
        self.z[1] = c.b[2] * x - c.a[1] * y;
        y
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct DeEssTelemetry {
    /// The deepest reduction, dB (≥ 0).
    pub gr_db: f32,
    /// The trigger band's and the whole input's loudest RMS, dBFS.
    pub band_db: f32,
    pub whole_db: f32,
}

pub struct DeEsser {
    fs: f32,
    /// The trigger band per side: two highpass and two lowpass sections.
    band_c: [Coeffs; 2],
    band_s: [[State; 4]; 2],
    designed: (f32, f32),
    band_pow: [f64; 2],
    whole_pow: [f64; 2],
    k_rms: f64,
    gr: f32,
    k_att: f32,
    filt: [[State; BAND_BELLS]; 2],
    /// Relative (0) to absolute (1) detection, gliding: switching never jumps the reduction.
    absolute: f32,
    delay: [Vec<f32>; 2],
    pos: usize,
    /// Samples of lookahead in use, and the dip while it changes (gain 1 → 0 → 1).
    look: usize,
    dip: f32,
    dip_step: f32,
    dip_dir: f32,
    threshold: f32,
    k_glide: f32,
    /// Listen and bypass, gliding (10 ms): neither clicks.
    listen: f32,
    bypass: f32,
    k_switch: f32,
}

impl DeEsser {
    pub fn new(fs: f32, s: &DeEssSettings) -> Self {
        let len = (LOOKAHEAD_MS * 0.001 * fs) as usize + 2;
        let mut d = Self {
            fs,
            band_c: [Coeffs::IDENTITY; 2],
            band_s: [[State::default(); 4]; 2],
            designed: (0.0, 0.0),
            band_pow: [0.0; 2],
            whole_pow: [0.0; 2],
            k_rms: 1.0 - (-1.0 / f64::from(RMS_MS * 0.001 * fs)).exp(),
            gr: 0.0,
            k_att: 1.0 - (-1.0 / (ATTACK_MS * 0.001 * fs)).exp(),
            filt: [[State::default(); BAND_BELLS]; 2],
            absolute: f32::from(u8::from(s.detection == Detection::Absolute)),
            delay: [vec![0.0; len], vec![0.0; len]],
            pos: 0,
            look: 0,
            dip: 1.0,
            dip_step: 1.0 / (DIP_MS * 0.001 * fs).max(1.0),
            dip_dir: 0.0,
            threshold: s.threshold_db,
            k_glide: 1.0 - (-1.0 / (GLIDE_MS * 0.001 * fs)).exp(),
            listen: 0.0,
            bypass: 0.0,
            k_switch: 1.0 - (-1.0 / (0.010 * fs)).exp(),
        };
        d.reset(s);
        d
    }

    pub fn sample_rate(&self) -> f32 {
        self.fs
    }

    /// The lookahead for `s`, samples.
    pub fn latency_for(s: &DeEssSettings, fs: f32) -> usize {
        if s.lookahead {
            (LOOKAHEAD_MS * 0.001 * fs).round() as usize
        } else {
            0
        }
    }

    /// The latency now, samples.
    pub fn latency(&self) -> usize {
        self.look
    }

    pub fn reset(&mut self, s: &DeEssSettings) {
        self.band_s = [[State::default(); 4]; 2];
        self.band_pow = [0.0; 2];
        self.whole_pow = [0.0; 2];
        self.gr = 0.0;
        self.filt = [[State::default(); BAND_BELLS]; 2];
        self.absolute = f32::from(u8::from(s.detection == Detection::Absolute));
        for d in &mut self.delay {
            d.iter_mut().for_each(|v| *v = 0.0);
        }
        self.pos = 0;
        self.look = Self::latency_for(s, self.fs);
        self.dip = 1.0;
        self.dip_dir = 0.0;
        self.threshold = s.threshold_db;
        self.designed = (0.0, 0.0);
        self.listen = f32::from(u8::from(s.listen));
        self.bypass = f32::from(u8::from(s.bypass));
    }

    fn design(&mut self, s: &DeEssSettings) {
        let band = s.band(self.fs);
        if band != self.designed {
            let fs = f64::from(self.fs);
            self.band_c = [Coeffs::butter(f64::from(band.0), fs, true), Coeffs::butter(f64::from(band.1), fs, false)];
            self.designed = band;
        }
    }

    /// Processes one or two channels in place. Never allocates.
    #[allow(clippy::needless_range_loop)] // the sides' parallel state, by sample and side
    pub fn process(&mut self, channels: &mut [&mut [f32]], s: &DeEssSettings) -> DeEssTelemetry {
        let n_ch = channels.len().min(2);
        let mut tel = DeEssTelemetry { gr_db: 0.0, band_db: -200.0, whole_db: -200.0 };
        if n_ch == 0 {
            return tel;
        }
        self.design(s);
        let len = channels.iter().map(|c| c.len()).min().unwrap_or(0);
        let fs = self.fs;
        let k_rel = 1.0 - (-1.0 / (s.release_ms.max(1.0) * 0.001 * fs)).exp();
        let gate = 10f64.powf(f64::from(GATE_DB) / 10.0);
        let want_look = Self::latency_for(s, fs);
        let size = self.delay[0].len();
        for i in 0..len {
            // A lookahead change: fade out, switch, fade in.
            if want_look != self.look && self.dip_dir == 0.0 {
                self.dip_dir = -1.0;
            }
            if self.dip_dir != 0.0 {
                self.dip += self.dip_dir * self.dip_step;
                if self.dip <= 0.0 {
                    self.dip = 0.0;
                    self.look = want_look;
                    self.dip_dir = 1.0;
                } else if self.dip >= 1.0 {
                    self.dip = 1.0;
                    self.dip_dir = 0.0;
                }
            }
            self.threshold += (s.threshold_db - self.threshold) * self.k_glide;
            self.absolute += (f32::from(u8::from(s.detection == Detection::Absolute)) - self.absolute) * self.k_glide;
            let snap = |v: &mut f32, on: bool, k: f32| {
                let t = f32::from(u8::from(on));
                *v += (t - *v) * k;
                if (*v - t).abs() < 1e-6 {
                    *v = t;
                }
            };
            snap(&mut self.listen, s.listen, self.k_switch);
            snap(&mut self.bypass, s.bypass, self.k_switch);
            // Detection, linked: the louder side's.
            let mut over = f32::MIN;
            let mut nearest = f32::MIN;
            let mut band_out = [0.0f32; 2];
            for c in 0..n_ch {
                let x = f64::from(channels[c][i]);
                let st = &mut self.band_s[c];
                let mut b = st[0].run(&self.band_c[0], x);
                b = st[1].run(&self.band_c[0], b);
                b = st[2].run(&self.band_c[1], b);
                b = st[3].run(&self.band_c[1], b);
                band_out[c] = b as f32;
                self.band_pow[c] += (b * b - self.band_pow[c]) * self.k_rms;
                self.whole_pow[c] += (x * x - self.whole_pow[c]) * self.k_rms;
                let (bp, wp) = (self.band_pow[c].max(1e-20), self.whole_pow[c].max(1e-20));
                let band_db = (10.0 * bp.log10()) as f32;
                let whole_db = (10.0 * wp.log10()) as f32;
                tel.band_db = tel.band_db.max(band_db);
                tel.whole_db = tel.whole_db.max(whole_db);
                if wp < gate {
                    continue;
                }
                let level = band_db - whole_db * (1.0 - self.absolute);
                over = over.max(over_db(level, self.threshold));
                nearest = nearest.max(level - self.threshold);
            }
            let target = if over == f32::MIN { 0.0 } else { over.clamp(0.0, s.range_db.max(0.0)) };
            let below = if nearest == f32::MIN { 30.0 } else { (-nearest).max(0.0) };
            let k_down = 1.0 - (1.0 - k_rel).powf(1.0 + below / SMART_DB);
            self.gr += (target - self.gr) * if target > self.gr { self.k_att } else { k_down };
            if self.gr < 1e-6 {
                self.gr = 0.0;
            }
            tel.gr_db = tel.gr_db.max(self.gr);
            let coeffs = reduction_filter(s, self.gr, fs);
            let wide = if s.reduce == Reduce::Wide { crate::db_to_gain(-self.gr) } else { 1.0 };
            for c in 0..n_ch {
                let x = channels[c][i];
                let d = &mut self.delay[c];
                d[self.pos] = x;
                let xd = d[(self.pos + size - self.look) % size];
                // At no reduction the filter is the identity: the input as it was.
                let mut y = f64::from(xd);
                for (st, co) in self.filt[c].iter_mut().zip(&coeffs) {
                    y = st.run(co, y);
                }
                let y = y as f32 * wide;
                // Each blend as a·(1 − t) + b·t, so its ends are exact (bypass gives the input).
                let y = y * (1.0 - self.listen) + band_out[c] * self.listen;
                let y = y * (1.0 - self.bypass) + xd * self.bypass;
                channels[c][i] = y * self.dip;
            }
            self.pos = (self.pos + 1) % size;
        }
        tel
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_filters_are_the_identity_at_no_reduction() {
        for s in [DeEssSettings::default(), DeEssSettings { reduce: Reduce::Band, ..DeEssSettings::default() }] {
            for c in reduction_filter(&s, 0.0, 48_000.0) {
                assert_eq!(c.b[0], 1.0);
                assert_eq!((c.b[1], c.b[2]), (c.a[0], c.a[1]));
            }
        }
    }

    #[test]
    fn a_reduction_is_where_it_is_said() {
        let fs = 48_000.0;
        let shelf = DeEssSettings::default();
        assert!((response_db(&shelf, 10.0, 16_000.0, fs) + 10.0).abs() < 0.5);
        assert!(response_db(&shelf, 10.0, 500.0, fs).abs() < 0.1);
        let band = DeEssSettings { reduce: Reduce::Band, ..shelf };
        assert!((response_db(&band, 10.0, (5000.0f64 * 12_000.0).sqrt(), fs) + 10.0).abs() < 0.5);
        assert!(response_db(&band, 10.0, 16_000.0, fs) > -1.5);
        assert!(response_db(&band, 10.0, 3000.0, fs) > -1.5);
    }
}
