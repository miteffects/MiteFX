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

/// The lookahead's reach, ms (user, 2026-11-05: "dynamic lookahead" → a time, not on/off).
pub const MAX_LOOKAHEAD_MS: f32 = 10.0;
/// A new lookahead time is put in once it has stayed put this long, ms: turning the dial changes
/// the latency, and each change is a dip, so it waits for the hand to stop (one dip, not a stutter).
const LOOK_SETTLE_MS: f32 = 150.0;
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
/// Auto gain's loudness meters (K-weighted mean squares, in and out) integrate over this, s: long
/// enough that the gain is a level, never a pump.
const AUTO_METER_S: f32 = 3.0;
/// Auto gain's gain glides over this, s.
const AUTO_GLIDE_S: f32 = 0.3;
/// Auto gain's reach, dB: a de-esser only takes away, so it only gives back, up to this.
pub const AUTO_MAX_DB: f32 = 6.0;

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
    /// Only the frequencies that poke over, each on its own (see [`Spectral`]).
    Spectral,
}

impl Reduce {
    pub const NAMES: [&'static str; 4] = ["Shelf", "Band", "Wide", "Spectral"];

    pub fn from_index(i: usize) -> Self {
        [Self::Shelf, Self::Band, Self::Wide, Self::Spectral][i.min(3)]
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
    /// How far the audio runs behind the detector, ms (0…`MAX_LOOKAHEAD_MS`): the reduction is in
    /// place this long before the sibilant arrives. It is the latency (SPECTRAL has its own).
    pub lookahead_ms: f32,
    pub listen: bool,
    /// The input as it came (delayed by the lookahead, so switching keeps time).
    pub bypass: bool,
    /// Give back the loudness the reduction took (user, 2026-11-05: "also make auto gain for
    /// de-ess"): the output as loud as the input (K-weighted, over 3 s), so Bypass compares fairly.
    pub auto_gain: bool,
}

impl Default for DeEssSettings {
    fn default() -> Self {
        Self {
            threshold_db: -15.0,
            detection: Detection::Relative,
            range_db: 12.0,
            release_ms: 60.0,
            lo_hz: 5000.0,
            hi_hz: 12_000.0,
            reduce: Reduce::Shelf,
            lookahead_ms: 3.0,
            listen: false,
            bypass: false,
            auto_gain: false,
        }
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
    const IDENTITY: Self = Self {
        b: [1.0, 0.0, 0.0],
        a: [0.0, 0.0],
    };

    fn butter(fc: f64, fs: f64, high: bool) -> Self {
        let w = std::f64::consts::TAU * fc / fs;
        let (sin, cos) = w.sin_cos();
        let alpha = sin / std::f64::consts::SQRT_2;
        let a0 = 1.0 + alpha;
        let (b0, b1) = if high {
            ((1.0 + cos) / 2.0, -(1.0 + cos))
        } else {
            ((1.0 - cos) / 2.0, 1.0 - cos)
        };
        Self {
            b: [b0 / a0, b1 / a0, b0 / a0],
            a: [-2.0 * cos / a0, (1.0 - alpha) / a0],
        }
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
            b: [
                a * ((a + 1.0) + (a - 1.0) * cos + sa) / a0,
                -2.0 * a * ((a - 1.0) + (a + 1.0) * cos) / a0,
                a * ((a + 1.0) + (a - 1.0) * cos - sa) / a0,
            ],
            a: [
                2.0 * ((a - 1.0) - (a + 1.0) * cos) / a0,
                ((a + 1.0) - (a - 1.0) * cos - sa) / a0,
            ],
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
        Self {
            b: [
                (1.0 + alpha * a) / a0,
                -2.0 * cos / a0,
                (1.0 - alpha * a) / a0,
            ],
            a: [-2.0 * cos / a0, (1.0 - alpha / a) / a0],
        }
    }

    /// Its magnitude at `freq`, dB.
    pub fn response_db(&self, freq: f64, fs: f64) -> f64 {
        let w = std::f64::consts::TAU * freq / fs;
        let z1 = (w.cos(), -w.sin());
        let z2 = ((2.0 * w).cos(), -(2.0 * w).sin());
        let num = (
            self.b[0] + self.b[1] * z1.0 + self.b[2] * z2.0,
            self.b[1] * z1.1 + self.b[2] * z2.1,
        );
        let den = (
            1.0 + self.a[0] * z1.0 + self.a[1] * z2.0,
            self.a[0] * z1.1 + self.a[1] * z2.1,
        );
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
                *sec = Coeffs::bell(
                    centre * 2f64.powf(-half),
                    centre * 2f64.powf(half),
                    fs,
                    g * BAND_SHARE,
                );
            }
        }
        Reduce::Wide | Reduce::Spectral => {}
    }
    c
}

/// The processing's response at `freq` for `gr_db` of reduction (what the editor draws).
pub fn response_db(s: &DeEssSettings, gr_db: f32, freq: f64, fs: f32) -> f64 {
    match s.reduce {
        Reduce::Wide => -f64::from(gr_db.max(0.0)),
        // Per frequency, as it happens: the editor draws `DeEsser::spectral_gains`.
        Reduce::Spectral => 0.0,
        _ => reduction_filter(s, gr_db, fs)
            .iter()
            .map(|c| c.response_db(freq, f64::from(fs)))
            .sum(),
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
    /// What auto gain gives back now, dB (0 when off).
    pub auto_db: f32,
}

/// Auto gain: the input's and the output's loudness (K-weighted, both sides, over
/// `AUTO_METER_S`, only while there is sound) and the gain that makes them equal, gliding.
struct AutoGain {
    k_in: [crate::loudness::KWeighting; 2],
    k_out: [crate::loudness::KWeighting; 2],
    p_in: f64,
    p_out: f64,
    k_meter: f64,
    gain: f32,
    k_glide: f32,
    /// On (1) or off (0), gliding: switching never jumps.
    on: f32,
}

impl AutoGain {
    fn new(fs: f32) -> Self {
        let kw = crate::loudness::KWeighting::new(f64::from(fs));
        Self {
            k_in: [kw; 2],
            k_out: [kw; 2],
            p_in: 0.0,
            p_out: 0.0,
            k_meter: 1.0 - (-1.0 / f64::from(AUTO_METER_S * fs)).exp(),
            gain: 1.0,
            k_glide: 1.0 - (-1.0 / (AUTO_GLIDE_S * fs)).exp(),
            on: 0.0,
        }
    }

    fn reset(&mut self, on: bool) {
        for k in self.k_in.iter_mut().chain(&mut self.k_out) {
            k.reset();
        }
        self.p_in = 0.0;
        self.p_out = 0.0;
        self.gain = 1.0;
        self.on = f32::from(u8::from(on));
    }

    /// The gain in force (1 when off).
    #[inline]
    fn now(&self) -> f32 {
        1.0 + (self.gain - 1.0) * self.on
    }
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
    /// The lookahead asked for, and for how many samples it has been asked (`LOOK_SETTLE_MS`).
    pending: usize,
    pending_age: u32,
    settle: u32,
    dip: f32,
    dip_step: f32,
    dip_dir: f32,
    threshold: f32,
    k_glide: f32,
    /// Listen and bypass, gliding (10 ms): neither clicks.
    listen: f32,
    bypass: f32,
    k_switch: f32,
    spectral: Spectral,
    /// Whether the spectral path is the one heard (switched at the bottom of a dip, as the
    /// latency changes with it).
    spectral_on: bool,
    auto: AutoGain,
}

/// The spectral mode's frame: 512 samples at 48 kHz (scaled with the rate), a quarter hop.
pub fn spectral_size(fs: f32) -> usize {
    ((512.0 * fs / 48_000.0) as usize)
        .next_power_of_two()
        .clamp(256, 4096)
}

/// The spectral reduction: the input in frames (`Stft`: 512 samples at 48 kHz, 94 Hz a bin, a
/// 2.7 ms hop), every bin of the trigger band against its own threshold. That threshold is the
/// broadband one spread over the band: RELATIVE, the voice's power (the whole frame) shared evenly
/// by the band's bins, + THRESHOLD; ABSOLUTE, THRESHOLD shared evenly. So a sibilant spread evenly
/// across the band is reduced as BAND would, and a peaky one (a whistle, a lisp) only where it
/// peaks. Each bin: as far as it pokes over (the knee), at most RANGE, at once, letting go at the
/// smart release; its gain shared a little with its neighbours (0.2 / 0.6 / 0.2). The band's edges
/// taper over a sixth of an octave.
struct Spectral {
    stft: crate::stft::Stft,
    buf: [Vec<f32>; 2],
    /// Per bin: the reduction now (dB), its target, the band's weight.
    gain_db: Vec<f64>,
    target: Vec<f64>,
    power: Vec<f64>,
    weight: Vec<f64>,
    weight_for: (f32, f32),
    bin_hz: f64,
    /// The band's reduction, energy-weighted, the most in the frames since it was read (dB).
    gr_db: f32,
}

const SPEC_CHUNK: usize = 256;

impl Spectral {
    fn new(fs: f32) -> Self {
        let size = spectral_size(fs);
        let stft = crate::stft::Stft::new(size);
        let bins = stft.bins();
        Self {
            bin_hz: f64::from(fs) / size as f64,
            stft,
            buf: [vec![0.0; SPEC_CHUNK], vec![0.0; SPEC_CHUNK]],
            gain_db: vec![0.0; bins],
            target: vec![0.0; bins],
            power: vec![0.0; bins],
            weight: vec![0.0; bins],
            weight_for: (0.0, 0.0),
            gr_db: 0.0,
        }
    }

    fn reset(&mut self) {
        self.stft.reset();
        self.gain_db.iter_mut().for_each(|g| *g = 0.0);
        self.gr_db = 0.0;
    }

    fn set_band(&mut self, lo: f32, hi: f32) {
        if (lo, hi) == self.weight_for {
            return;
        }
        let taper = 2f64.powf(1.0 / 6.0);
        let (lo, hi) = (f64::from(lo), f64::from(hi));
        for (k, w) in self.weight.iter_mut().enumerate() {
            let f = k as f64 * self.bin_hz;
            *w = if f >= lo && f <= hi {
                1.0
            } else if f < lo && f > lo / taper {
                0.5 - 0.5 * (std::f64::consts::PI * (f - lo / taper) / (lo - lo / taper)).cos()
            } else if f > hi && f < hi * taper {
                0.5 + 0.5 * (std::f64::consts::PI * (f - hi) / (hi * taper - hi)).cos()
            } else {
                0.0
            };
        }
        self.weight_for = (lo as f32, hi as f32);
    }
}

/// What a spectral frame needs to know.
#[derive(Clone, Copy)]
struct FrameParams {
    threshold_db: f64,
    absolute: f64,
    range_db: f64,
    k_rel: f64,
    full_scale_sq: f64,
}

#[allow(clippy::too_many_arguments)] // the frame's parts, borrowed apart from the STFT
fn spectral_frame(
    l: &mut [realfft::num_complex::Complex<f64>],
    r: &mut [realfft::num_complex::Complex<f64>],
    gain_db: &mut [f64],
    target: &mut [f64],
    power: &mut [f64],
    weight: &[f64],
    p: FrameParams,
    gr_out: &mut f32,
) {
    let bins = l.len().min(r.len()).min(gain_db.len());
    for k in 0..bins {
        power[k] = if k > 0 && k < bins - 1 {
            (l[k].norm_sqr() + r[k].norm_sqr()) / (2.0 * p.full_scale_sq)
        } else {
            0.0
        };
    }
    let whole: f64 = power[..bins].iter().sum();
    let n_band: f64 = weight[..bins].iter().sum::<f64>().max(1.0);
    let gate = 10f64.powf(f64::from(GATE_DB) / 10.0);
    let reference = 10.0 * (whole.max(1e-30) / n_band).log10() * (1.0 - p.absolute)
        + 10.0 * (1.0 / n_band).log10() * p.absolute
        + p.threshold_db;
    let level = |k: usize| 10.0 * power[k].max(1e-30).log10();
    for k in 0..bins {
        target[k] = 0.0;
        if weight[k] > 0.0 && whole >= gate && power[k] > 0.0 {
            let over = f64::from(over_db(level(k) as f32, reference as f32));
            target[k] = over.min(p.range_db).max(0.0) * weight[k];
        }
    }
    let (mut num, mut den) = (0.0, 0.0);
    let mut prev = 0.0;
    for k in 0..bins {
        let next = if k + 1 < bins { target[k + 1] } else { 0.0 };
        let t = 0.2 * prev + 0.6 * target[k] + 0.2 * next;
        prev = target[k];
        let g = &mut gain_db[k];
        if t > *g {
            *g = t;
        } else {
            // Smart release, per bin: faster the further it sits under its threshold.
            let below = (reference - level(k)).clamp(0.0, 60.0);
            let k_down = 1.0 - (1.0 - p.k_rel).powf(1.0 + below / f64::from(SMART_DB));
            *g += (t - *g) * k_down;
            if *g < 1e-6 {
                *g = 0.0;
            }
        }
        let lin = if *g > 0.0 {
            10f64.powf(-*g / 20.0)
        } else {
            1.0
        };
        if *g > 0.0 {
            l[k] *= lin;
            r[k] *= lin;
        }
        let pk = power[k] * weight[k];
        num += pk;
        den += pk * lin * lin;
    }
    if num > 1e-30 && den > 0.0 {
        *gr_out = gr_out.max((10.0 * (num / den).log10()) as f32);
    }
}

impl DeEsser {
    pub fn new(fs: f32, s: &DeEssSettings) -> Self {
        let len = ((MAX_LOOKAHEAD_MS * 0.001 * fs).ceil() as usize).max(spectral_size(fs)) + 2;
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
            pending: 0,
            pending_age: 0,
            settle: (LOOK_SETTLE_MS * 0.001 * fs) as u32,
            dip: 1.0,
            dip_step: 1.0 / (DIP_MS * 0.001 * fs).max(1.0),
            dip_dir: 0.0,
            threshold: s.threshold_db,
            k_glide: 1.0 - (-1.0 / (GLIDE_MS * 0.001 * fs)).exp(),
            listen: 0.0,
            bypass: 0.0,
            k_switch: 1.0 - (-1.0 / (0.010 * fs)).exp(),
            spectral: Spectral::new(fs),
            spectral_on: false,
            auto: AutoGain::new(fs),
        };
        d.reset(s);
        d
    }

    pub fn sample_rate(&self) -> f32 {
        self.fs
    }

    /// The latency for `s`, samples: the spectral frame, or the lookahead.
    pub fn latency_for(s: &DeEssSettings, fs: f32) -> usize {
        if s.reduce == Reduce::Spectral {
            spectral_size(fs)
        } else {
            (s.lookahead_ms.clamp(0.0, MAX_LOOKAHEAD_MS) * 0.001 * fs).round() as usize
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
        self.pending = self.look;
        self.pending_age = 0;
        self.dip = 1.0;
        self.dip_dir = 0.0;
        self.threshold = s.threshold_db;
        self.designed = (0.0, 0.0);
        self.listen = f32::from(u8::from(s.listen));
        self.bypass = f32::from(u8::from(s.bypass));
        self.spectral.reset();
        self.spectral_on = s.reduce == Reduce::Spectral;
        self.auto.reset(s.auto_gain);
    }

    /// The spectral reduction per bin now (dB, ≥ 0), and a bin's width (Hz).
    pub fn spectral_gains(&self) -> (&[f64], f64) {
        (&self.spectral.gain_db, self.spectral.bin_hz)
    }

    /// Whether the spectral path is the one heard.
    pub fn spectral_on(&self) -> bool {
        self.spectral_on
    }

    fn design(&mut self, s: &DeEssSettings) {
        let band = s.band(self.fs);
        if band != self.designed {
            let fs = f64::from(self.fs);
            self.band_c = [
                Coeffs::butter(f64::from(band.0), fs, true),
                Coeffs::butter(f64::from(band.1), fs, false),
            ];
            self.designed = band;
        }
    }

    /// Processes one or two channels in place. Never allocates.
    #[allow(clippy::needless_range_loop)] // the sides' parallel state, by sample and side
    pub fn process(&mut self, channels: &mut [&mut [f32]], s: &DeEssSettings) -> DeEssTelemetry {
        let n_ch = channels.len().min(2);
        let mut tel = DeEssTelemetry {
            gr_db: 0.0,
            band_db: -200.0,
            whole_db: -200.0,
            auto_db: 0.0,
        };
        if n_ch == 0 {
            return tel;
        }
        self.design(s);
        let len = channels.iter().map(|c| c.len()).min().unwrap_or(0);
        let fs = self.fs;
        let k_rel = 1.0 - (-1.0 / (s.release_ms.max(1.0) * 0.001 * fs)).exp();
        let gate = 10f64.powf(f64::from(GATE_DB) / 10.0);
        let want_look = Self::latency_for(s, fs);
        let want_spectral = s.reduce == Reduce::Spectral;
        let size = self.delay[0].len();
        let (lo, hi) = s.band(fs);
        self.spectral.set_band(lo, hi);
        let mut spec_gr = 0.0f32;
        let mut chunk_start = 0;
        while chunk_start < len {
            let chunk_end = (chunk_start + SPEC_CHUNK).min(len);
            let n = chunk_end - chunk_start;
            // The spectral path runs always (its frames warm), heard only while on.
            {
                let p = FrameParams {
                    threshold_db: f64::from(self.threshold),
                    absolute: f64::from(self.absolute),
                    range_db: f64::from(s.range_db.max(0.0)),
                    k_rel: 1.0
                        - (-(self.spectral.stft.hop() as f64)
                            / (f64::from(s.release_ms.max(1.0)) * 0.001 * f64::from(fs)))
                        .exp(),
                    full_scale_sq: self.spectral.stft.full_scale().powi(2),
                };
                let Spectral {
                    stft,
                    buf,
                    gain_db,
                    target,
                    power,
                    weight,
                    gr_db,
                    ..
                } = &mut self.spectral;
                let [bl, br] = buf;
                bl[..n].copy_from_slice(&channels[0][chunk_start..chunk_end]);
                br[..n].copy_from_slice(&channels[n_ch - 1][chunk_start..chunk_end]);
                stft.process(&mut bl[..n], &mut br[..n], &mut |a, b| {
                    spectral_frame(a, b, gain_db, target, power, weight, p, gr_db)
                });
                spec_gr = spec_gr.max(*gr_db);
                *gr_db = 0.0;
            }
            for i in chunk_start..chunk_end {
                // A latency change (lookahead, or the spectral mode in or out): fade out, switch,
                // fade in.
                if want_look != self.pending {
                    self.pending = want_look;
                    self.pending_age = 0;
                } else {
                    self.pending_age = self.pending_age.saturating_add(1);
                }
                let settled = self.pending_age >= self.settle;
                if ((want_look != self.look && settled) || want_spectral != self.spectral_on)
                    && self.dip_dir == 0.0
                {
                    self.dip_dir = -1.0;
                }
                if self.dip_dir != 0.0 {
                    self.dip += self.dip_dir * self.dip_step;
                    if self.dip <= 0.0 {
                        self.dip = 0.0;
                        self.look = want_look;
                        self.spectral_on = want_spectral;
                        self.dip_dir = 1.0;
                    } else if self.dip >= 1.0 {
                        self.dip = 1.0;
                        self.dip_dir = 0.0;
                    }
                }
                self.threshold += (s.threshold_db - self.threshold) * self.k_glide;
                self.absolute += (f32::from(u8::from(s.detection == Detection::Absolute))
                    - self.absolute)
                    * self.k_glide;
                let snap = |v: &mut f32, on: bool, k: f32| {
                    let t = f32::from(u8::from(on));
                    *v += (t - *v) * k;
                    if (*v - t).abs() < 1e-6 {
                        *v = t;
                    }
                };
                snap(&mut self.listen, s.listen, self.k_switch);
                snap(&mut self.bypass, s.bypass, self.k_switch);
                snap(&mut self.auto.on, s.auto_gain, self.k_switch);
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
                let target = if over == f32::MIN {
                    0.0
                } else {
                    over.clamp(0.0, s.range_db.max(0.0))
                };
                let below = if nearest == f32::MIN {
                    30.0
                } else {
                    (-nearest).max(0.0)
                };
                let k_down = 1.0 - (1.0 - k_rel).powf(1.0 + below / SMART_DB);
                self.gr += (target - self.gr) * if target > self.gr { self.k_att } else { k_down };
                if self.gr < 1e-6 {
                    self.gr = 0.0;
                }
                if !self.spectral_on {
                    tel.gr_db = tel.gr_db.max(self.gr);
                }
                let coeffs = reduction_filter(s, self.gr, fs);
                let wide = if s.reduce == Reduce::Wide {
                    crate::db_to_gain(-self.gr)
                } else {
                    1.0
                };
                let sounding = nearest != f32::MIN;
                let makeup = self.auto.now();
                let (mut e_in, mut e_out) = (0.0f64, 0.0f64);
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
                    let y = if self.spectral_on {
                        self.spectral.buf[c.min(1)][i - chunk_start]
                    } else {
                        y as f32 * wide
                    };
                    // Auto gain meters the reduction alone (before its own gain: no loop), and gives
                    // back to the processed signal only (bypass and listen stay as they are).
                    let (ki, ko) = (self.auto.k_in[c].run(xd), self.auto.k_out[c].run(y));
                    e_in += ki * ki;
                    e_out += ko * ko;
                    let y = y * makeup;
                    // Each blend as a·(1 − t) + b·t, so its ends are exact (bypass gives the input).
                    let y = y * (1.0 - self.listen) + band_out[c] * self.listen;
                    let y = y * (1.0 - self.bypass) + xd * self.bypass;
                    channels[c][i] = y * self.dip;
                }
                self.pos = (self.pos + 1) % size;
                let a = &mut self.auto;
                if sounding {
                    a.p_in += (e_in - a.p_in) * a.k_meter;
                    a.p_out += (e_out - a.p_out) * a.k_meter;
                }
                let target = if a.p_out > 1e-14 && a.p_in > 1e-14 {
                    ((a.p_in / a.p_out).sqrt() as f32).clamp(1.0, crate::db_to_gain(AUTO_MAX_DB))
                } else {
                    1.0
                };
                a.gain += (target - a.gain) * a.k_glide;
            }
            chunk_start = chunk_end;
        }
        if self.spectral_on {
            tel.gr_db = tel.gr_db.max(spec_gr);
        }
        tel.auto_db = crate::gain_to_db(self.auto.now());
        tel
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_filters_are_the_identity_at_no_reduction() {
        for s in [
            DeEssSettings::default(),
            DeEssSettings {
                reduce: Reduce::Band,
                ..DeEssSettings::default()
            },
        ] {
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
        let band = DeEssSettings {
            reduce: Reduce::Band,
            ..shelf
        };
        assert!((response_db(&band, 10.0, (5000.0f64 * 12_000.0).sqrt(), fs) + 10.0).abs() < 0.5);
        assert!(response_db(&band, 10.0, 16_000.0, fs) > -1.5);
        assert!(response_db(&band, 10.0, 3000.0, fs) > -1.5);
    }
}
