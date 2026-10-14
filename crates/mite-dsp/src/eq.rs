//! Parametric EQ bands: prototypes, matched design, smoothing and processing.
//!
//! Shared by Mite EQ and, later, every plugin with an EQ section (Comp sidechain EQ, Verb decay
//! EQ, Delay per-tap EQ — BLUEPRINT §6 "shared engines").

use std::f64::consts::{FRAC_1_SQRT_2, PI};

use crate::filter::{matched, matched_symmetric, Analog, Coeffs, Tdf2};
use crate::smooth::LinearSmoother;

/// Band types, in their frozen parameter order (append only; the index is saved state).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BandType {
    #[default]
    Bell,
    LowShelf,
    HighShelf,
    LowCut,
    HighCut,
    Notch,
    BandPass,
    TiltShelf,
}

impl BandType {
    pub const ALL: [Self; 8] = [
        Self::Bell,
        Self::LowShelf,
        Self::HighShelf,
        Self::LowCut,
        Self::HighCut,
        Self::Notch,
        Self::BandPass,
        Self::TiltShelf,
    ];
    pub const NAMES: [&'static str; 8] =
        ["Bell", "Low Shelf", "High Shelf", "Low Cut", "High Cut", "Notch", "Band Pass", "Tilt Shelf"];

    pub fn from_index(i: usize) -> Self {
        Self::ALL.get(i).copied().unwrap_or_default()
    }

    /// Whether the gain parameter does anything for this type.
    pub fn has_gain(self) -> bool {
        matches!(self, Self::Bell | Self::LowShelf | Self::HighShelf | Self::TiltShelf)
    }

    /// Whether the slope parameter does anything for this type.
    pub fn has_slope(self) -> bool {
        matches!(self, Self::LowCut | Self::HighCut)
    }
}

/// Which part of a stereo signal a band processes, in its frozen parameter order (append only).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Placement {
    #[default]
    Stereo,
    Left,
    Right,
    Mid,
    Side,
}

impl Placement {
    pub const ALL: [Self; 5] = [Self::Stereo, Self::Left, Self::Right, Self::Mid, Self::Side];
    pub const NAMES: [&'static str; 5] = ["Stereo", "Left", "Right", "Mid", "Side"];

    pub fn from_index(i: usize) -> Self {
        Self::ALL.get(i).copied().unwrap_or_default()
    }
}

/// Cut slopes in dB/octave, in their frozen parameter order.
pub const SLOPES_DB: [u32; 9] = [6, 12, 18, 24, 30, 36, 48, 72, 96];
pub const SLOPE_NAMES: [&str; 9] =
    ["6 dB/oct", "12 dB/oct", "18 dB/oct", "24 dB/oct", "30 dB/oct", "36 dB/oct", "48 dB/oct", "72 dB/oct", "96 dB/oct"];
/// Index into [`SLOPES_DB`] of the default slope (24 dB/oct).
pub const DEFAULT_SLOPE: usize = 3;

/// Most sections a band can need: 96 dB/oct = order 16 = 8 second-order sections.
pub const MAX_SECTIONS: usize = 9;

pub const FREQ_MIN: f64 = 10.0;
pub const FREQ_MAX: f64 = 30_000.0;
pub const GAIN_RANGE_DB: f64 = 30.0;
pub const Q_MIN: f64 = 0.025;
pub const Q_MAX: f64 = 40.0;

/// One band's settings, as the host parameters describe them.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BandSettings {
    pub enabled: bool,
    pub kind: BandType,
    pub freq: f64,
    pub gain_db: f64,
    pub q: f64,
    /// Index into [`SLOPES_DB`].
    pub slope: usize,
    pub placement: Placement,
}

impl Default for BandSettings {
    fn default() -> Self {
        Self { enabled: false, kind: BandType::Bell, freq: 1000.0, gain_db: 0.0, q: FRAC_1_SQRT_2, slope: DEFAULT_SLOPE, placement: Placement::Stereo }
    }
}

impl BandSettings {
    /// Whether `other` needs a different filter structure (a crossfade instead of smoothing).
    fn structure_differs(&self, other: &Self) -> bool {
        self.enabled != other.enabled || self.kind != other.kind || self.slope != other.slope || self.placement != other.placement
    }
}

/// A band's digital sections. Fixed capacity, so designing never allocates.
#[derive(Debug, Clone, Copy)]
pub struct Sections {
    coeffs: [Coeffs; MAX_SECTIONS],
    len: usize,
}

impl Default for Sections {
    fn default() -> Self {
        Self { coeffs: [Coeffs::IDENTITY; MAX_SECTIONS], len: 0 }
    }
}

impl Sections {
    fn push(&mut self, c: Coeffs) {
        if self.len < MAX_SECTIONS {
            self.coeffs[self.len] = c;
            self.len += 1;
        }
    }

    pub fn as_slice(&self) -> &[Coeffs] {
        &self.coeffs[..self.len]
    }

    /// Digital response in dB at `w` radians/sample.
    pub fn response_db(&self, w: f64) -> f64 {
        self.as_slice().iter().map(|c| 10.0 * c.magnitude_sq(w).log10()).sum()
    }
}

/// Butterworth Q of section `k` (0-based) in an order-`order` cascade.
/// Q of the `k`-th pole pair of an order-`order` Butterworth filter; `k = 0` is the sharpest.
fn butterworth_q(order: usize, k: usize) -> f64 {
    1.0 / (2.0 * ((2 * k + 1) as f64 * PI / (2 * order) as f64).sin())
}

/// Calls `f` with every analog section of the band, plus whether it is a cut (|H| < 1 at the
/// centre) so it can be designed by inversion.
fn for_each_prototype(s: &BandSettings, mut f: impl FnMut(Analog, bool)) {
    let q = s.q.clamp(Q_MIN, Q_MAX);
    let a = 10f64.powf(s.gain_db.clamp(-GAIN_RANGE_DB, GAIN_RANGE_DB) / 40.0);
    let cut = s.gain_db < 0.0;
    match s.kind {
        BandType::Bell => f(Analog::second(1.0, a / q, 1.0, 1.0, 1.0 / (a * q), 1.0), cut),
        BandType::LowShelf => {
            let r = a.sqrt();
            f(Analog::second(a, a * r / q, a * a, a, r / q, 1.0), cut);
        }
        BandType::HighShelf => {
            let r = a.sqrt();
            f(Analog::second(a * a, a * r / q, a, 1.0, r / q, a), cut);
        }
        BandType::TiltShelf => {
            // First-order tilt pivoting on f0: −g/2 at DC, +g/2 at the top, unity at f0.
            f(Analog::first(a, 1.0, 1.0, a), false);
        }
        BandType::Notch => f(Analog::second(1.0, 0.0, 1.0, 1.0, 1.0 / q, 1.0), false),
        BandType::BandPass => f(Analog::second(0.0, 1.0 / q, 0.0, 1.0, 1.0 / q, 1.0), false),
        BandType::LowCut | BandType::HighCut => {
            let high = s.kind == BandType::HighCut;
            let order = (SLOPES_DB[s.slope.min(SLOPES_DB.len() - 1)] / 6) as usize;
            let pairs = order / 2;
            // The user Q sets the resonance of the sharpest section (= Butterworth at Q 0.71).
            let resonance = q / FRAC_1_SQRT_2;
            for k in 0..pairs {
                let mut qk = butterworth_q(order, k);
                if k == 0 {
                    qk *= resonance;
                }
                f(
                    if high {
                        Analog::second(0.0, 0.0, 1.0, 1.0, 1.0 / qk, 1.0)
                    } else {
                        Analog::second(1.0, 0.0, 0.0, 1.0, 1.0 / qk, 1.0)
                    },
                    false,
                );
            }
            if order % 2 == 1 {
                f(if high { Analog::first(0.0, 1.0, 1.0, 1.0) } else { Analog::first(1.0, 0.0, 1.0, 1.0) }, false);
            }
        }
    }
}

/// Designs the band's digital sections at `sample_rate`.
pub fn design(s: &BandSettings, sample_rate: f64) -> Sections {
    let mut out = Sections::default();
    if !s.enabled {
        return out;
    }
    let w0 = 2.0 * PI * s.freq.clamp(FREQ_MIN, FREQ_MAX) / sample_rate;
    for_each_prototype(s, |proto, cut| {
        out.push(if proto.is_first_order() { matched(&proto, w0) } else { matched_symmetric(&proto, w0, cut) });
    });
    out
}

/// The analog target response in dB at `freq`, what the band is designed to match.
pub fn analog_response_db(s: &BandSettings, freq: f64) -> f64 {
    if !s.enabled {
        return 0.0;
    }
    let x = freq / s.freq.clamp(FREQ_MIN, FREQ_MAX);
    let mut db = 0.0;
    for_each_prototype(s, |proto, _| db += 10.0 * proto.magnitude_sq(x).log10());
    db
}

/// Samples between coefficient redesigns while a parameter is moving.
const CONTROL_INTERVAL: u32 = 16;
/// Crossfade time for changes that can't be smoothed (type, slope, on/off).
const STRUCTURE_FADE_MS: f32 = 8.0;

/// One band, stereo, with smoothing. Allocation-free after construction.
pub struct BandProcessor {
    sample_rate: f64,
    /// Type/slope/enabled currently running.
    active: BandSettings,
    target: BandSettings,
    log_freq: LinearSmoother,
    gain: LinearSmoother,
    log_q: LinearSmoother,
    /// 0 = band bypassed, 1 = band fully in.
    mix: LinearSmoother,
    sections: Sections,
    state: [[Tdf2; MAX_SECTIONS]; 2],
    countdown: u32,
}

impl BandProcessor {
    pub fn new(sample_rate: f64, smoothing_ms: f32) -> Self {
        let mut p = Self {
            sample_rate,
            active: BandSettings::default(),
            target: BandSettings::default(),
            log_freq: LinearSmoother::new(0.0),
            gain: LinearSmoother::new(0.0),
            log_q: LinearSmoother::new(0.0),
            mix: LinearSmoother::new(0.0),
            sections: Sections::default(),
            state: [[Tdf2::default(); MAX_SECTIONS]; 2],
            countdown: 0,
        };
        p.prepare(sample_rate, smoothing_ms, &BandSettings::default());
        p
    }

    /// Off the audio thread: sets rates and jumps to `settings` without smoothing.
    pub fn prepare(&mut self, sample_rate: f64, smoothing_ms: f32, settings: &BandSettings) {
        self.sample_rate = sample_rate;
        let sr = sample_rate as f32;
        for s in [&mut self.log_freq, &mut self.gain, &mut self.log_q] {
            s.set_ramp(sr, smoothing_ms);
        }
        self.mix.set_ramp(sr, STRUCTURE_FADE_MS);
        self.reset(settings);
    }

    /// Jumps to `settings` and clears filter state. Allocation-free.
    pub fn reset(&mut self, settings: &BandSettings) {
        self.active = *settings;
        self.target = *settings;
        self.log_freq.reset(settings.freq.ln() as f32);
        self.gain.reset(settings.gain_db as f32);
        self.log_q.reset(settings.q.ln() as f32);
        self.mix.reset(if settings.enabled { 1.0 } else { 0.0 });
        self.redesign();
        self.state = [[Tdf2::default(); MAX_SECTIONS]; 2];
    }

    pub fn set_target(&mut self, s: &BandSettings) {
        self.target = *s;
        self.log_freq.set_target(s.freq.ln() as f32);
        self.gain.set_target(s.gain_db as f32);
        self.log_q.set_target(s.q.ln() as f32);
        if s.structure_differs(&self.active) {
            // Fade the band out; the swap happens once it is silent (see `advance`).
            self.mix.set_target(0.0);
        }
    }

    fn redesign(&mut self) {
        let settings = BandSettings {
            freq: f64::from(self.log_freq.current()).exp(),
            gain_db: f64::from(self.gain.current()),
            q: f64::from(self.log_q.current()).exp(),
            ..self.active
        };
        self.sections = design(&settings, self.sample_rate);
    }

    /// The settings the band is producing right now (smoothed values, active structure).
    pub fn current(&self) -> BandSettings {
        BandSettings {
            freq: f64::from(self.log_freq.current()).exp(),
            gain_db: f64::from(self.gain.current()),
            q: f64::from(self.log_q.current()).exp(),
            ..self.active
        }
    }

    /// True when the band contributes nothing and needs no work: disabled and fully faded out.
    pub fn is_idle(&self) -> bool {
        !self.active.enabled && !self.target.enabled && !self.mix.is_smoothing() && self.mix.current() == 0.0
    }

    #[inline]
    fn advance(&mut self) -> f64 {
        let moving = self.log_freq.is_smoothing() || self.gain.is_smoothing() || self.log_q.is_smoothing();
        self.log_freq.next_value();
        self.gain.next_value();
        self.log_q.next_value();
        let mix = self.mix.next_value();

        if mix == 0.0 && self.target.structure_differs(&self.active) {
            self.active.enabled = self.target.enabled;
            self.active.kind = self.target.kind;
            self.active.slope = self.target.slope;
            self.active.placement = self.target.placement;
            self.state = [[Tdf2::default(); MAX_SECTIONS]; 2];
            self.redesign();
            self.countdown = CONTROL_INTERVAL;
            self.mix.set_target(if self.active.enabled { 1.0 } else { 0.0 });
        } else if moving {
            if self.countdown == 0 {
                self.redesign();
                self.countdown = CONTROL_INTERVAL;
            }
            self.countdown -= 1;
        }
        f64::from(mix)
    }

    /// Processes up to two channels in place. With one channel every placement processes it.
    pub fn process(&mut self, channels: &mut [&mut [f32]]) {
        if self.is_idle() {
            return;
        }
        let frames = channels.first().map_or(0, |c| c.len());
        let placement = if channels.len() >= 2 { self.active.placement } else { Placement::Stereo };
        for i in 0..frames {
            let mix = self.advance();
            if mix == 0.0 {
                continue;
            }
            let sections = self.sections.as_slice();
            let filter = |state: &mut [Tdf2; MAX_SECTIONS], x: f64| {
                let mut y = x;
                for (section, st) in sections.iter().zip(state.iter_mut()) {
                    y = st.process(section, y);
                }
                if mix == 1.0 { y } else { x + mix * (y - x) }
            };
            match placement {
                Placement::Stereo => {
                    for (ch, state) in channels.iter_mut().zip(self.state.iter_mut()) {
                        ch[i] = filter(state, f64::from(ch[i])) as f32;
                    }
                }
                Placement::Left | Placement::Right => {
                    let c = usize::from(placement == Placement::Right);
                    channels[c][i] = filter(&mut self.state[0], f64::from(channels[c][i])) as f32;
                }
                Placement::Mid | Placement::Side => {
                    let (l, r) = (f64::from(channels[0][i]), f64::from(channels[1][i]));
                    let (mut m, mut s) = (0.5 * (l + r), 0.5 * (l - r));
                    if placement == Placement::Mid {
                        m = filter(&mut self.state[0], m);
                    } else {
                        s = filter(&mut self.state[0], s);
                    }
                    channels[0][i] = (m + s) as f32;
                    channels[1][i] = (m - s) as f32;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FS: f64 = 48_000.0;

    fn sine(freq: f64, len: usize) -> Vec<f32> {
        (0..len).map(|n| (2.0 * PI * freq * n as f64 / FS).sin() as f32 * 0.5).collect()
    }

    fn rms(x: &[f32]) -> f64 {
        (x.iter().map(|&v| f64::from(v).powi(2)).sum::<f64>() / x.len() as f64).sqrt()
    }

    fn band(kind: BandType, freq: f64, gain_db: f64, q: f64) -> BandSettings {
        BandSettings { enabled: true, kind, freq, gain_db, q, slope: DEFAULT_SLOPE, placement: Placement::Stereo }
    }

    #[test]
    fn disabled_band_nulls_bit_exactly() {
        let mut p = BandProcessor::new(FS, 20.0);
        let input = sine(440.0, 2048);
        let mut buf = input.clone();
        p.process(&mut [&mut buf]);
        assert_eq!(buf, input);
    }

    #[test]
    fn bell_gain_is_measured_in_the_signal() {
        let s = band(BandType::Bell, 1000.0, 6.0, 1.0);
        let mut p = BandProcessor::new(FS, 20.0);
        p.reset(&s);
        let input = sine(1000.0, 48_000);
        let mut buf = input.clone();
        p.process(&mut [&mut buf]);
        let gain = 20.0 * (rms(&buf[24_000..]) / rms(&input[24_000..])).log10();
        assert!((gain - 6.0).abs() < 0.05, "{gain}");
    }

    #[test]
    fn digital_follows_analog_for_every_type() {
        for kind in BandType::ALL {
            for freq in [80.0, 1000.0, 6000.0] {
                let s = band(kind, freq, -7.5, 1.3);
                let sections = design(&s, FS);
                let mut f = 20.0;
                while f < 16_000.0 {
                    let analog = analog_response_db(&s, f);
                    // Cuts are compared where they are within 12 dB of the passband: further down
                    // the steep skirt a dB of difference is inaudible.
                    let floor = if kind.has_slope() || kind == BandType::BandPass { -12.0 } else { -24.0 };
                    if analog > floor {
                        let digital = sections.response_db(2.0 * PI * f / FS);
                        // High bands carry the second-order Nyquist limit (see filter.rs tests).
                        let limit = if freq > 4000.0 { 1.0 } else { 0.35 };
                        assert!((digital - analog).abs() < limit, "{kind:?} {freq} Hz at {f:.0} Hz: {digital:.2} vs {analog:.2}");
                    }
                    f *= 1.05;
                }
            }
        }
    }

    #[test]
    fn cut_slopes_have_their_steepness() {
        for (slope, &db_per_oct) in SLOPES_DB.iter().enumerate() {
            let s = BandSettings { slope, ..band(BandType::LowCut, 1000.0, 0.0, FRAC_1_SQRT_2) };
            let sections = design(&s, FS);
            // Three and five octaves below the corner the asymptote dominates.
            let at = |f: f64| sections.response_db(2.0 * PI * f / FS);
            let measured = at(125.0) - at(31.25);
            let expected = 2.0 * f64::from(db_per_oct);
            let tolerance = (0.005 * expected).max(0.5);
            assert!((measured - expected).abs() < tolerance, "{db_per_oct} dB/oct: {measured:.2}");
        }
    }

    #[test]
    fn moving_frequency_is_click_free() {
        let mut p = BandProcessor::new(FS, 20.0);
        p.reset(&band(BandType::Bell, 100.0, 12.0, 2.0));
        let mut buf = vec![0.25_f32; 4800];
        p.process(&mut [&mut buf]);
        p.set_target(&band(BandType::Bell, 10_000.0, 12.0, 2.0));
        let mut buf = vec![0.25_f32; 4800];
        p.process(&mut [&mut buf]);
        // DC input: any discontinuity shows up as a jump between neighbouring samples.
        let worst = buf.windows(2).map(|w| (w[1] - w[0]).abs()).fold(0.0, f32::max);
        assert!(worst < 0.01, "{worst}");
    }

    #[test]
    fn type_change_crossfades_instead_of_clicking() {
        let mut p = BandProcessor::new(FS, 20.0);
        p.reset(&band(BandType::Bell, 1000.0, 12.0, 1.0));
        let input = sine(1000.0, 9600);
        let mut warm = input.clone();
        p.process(&mut [&mut warm]);
        p.set_target(&band(BandType::HighCut, 200.0, 0.0, FRAC_1_SQRT_2));
        let mut buf = input.clone();
        p.process(&mut [&mut buf]);
        let step = buf.windows(2).map(|w| (w[1] - w[0]).abs()).fold(0.0, f32::max);
        // A 1 kHz sine at +12 dB moves at most ~0.26 per sample; a hard swap would jump further.
        assert!(step < 0.3, "{step}");
        // After the fade the high cut at 200 Hz has removed most of the 1 kHz tone.
        assert!(rms(&buf[4800..]) < 0.05 * rms(&input[4800..]));
    }

    #[test]
    fn enabling_and_disabling_ends_bit_exact() {
        let mut p = BandProcessor::new(FS, 20.0);
        p.set_target(&band(BandType::Bell, 1000.0, 6.0, 1.0));
        let mut buf = sine(1000.0, 4800);
        p.process(&mut [&mut buf]);
        p.set_target(&BandSettings { enabled: false, ..band(BandType::Bell, 1000.0, 6.0, 1.0) });
        let mut fade = sine(1000.0, 4800);
        p.process(&mut [&mut fade]);
        assert!(p.is_idle());
        let input = sine(1000.0, 1024);
        let mut after = input.clone();
        p.process(&mut [&mut after]);
        assert_eq!(after, input);
    }

    fn gain_db_at(p: &mut BandProcessor, freq: f64, channel: usize, other: f32) -> (f64, Vec<f32>) {
        let x = sine(freq, 48_000);
        let mut l = x.clone();
        let mut r: Vec<f32> = x.iter().map(|v| v * other).collect();
        p.process(&mut [&mut l, &mut r]);
        let out = if channel == 0 { &l } else { &r };
        let reference: Vec<f32> = if channel == 0 { x.clone() } else { x.iter().map(|v| v * other).collect() };
        (20.0 * (rms(&out[24_000..]) / rms(&reference[24_000..])).log10(), if channel == 0 { r } else { l })
    }

    #[test]
    fn left_and_right_placement_touch_only_their_channel() {
        for (placement, c) in [(Placement::Left, 0), (Placement::Right, 1)] {
            let s = BandSettings { placement, ..band(BandType::Bell, 1000.0, 6.0, 1.0) };
            let mut p = BandProcessor::new(FS, 20.0);
            p.reset(&s);
            let (gain, other) = gain_db_at(&mut p, 1000.0, c, 1.0);
            assert!((gain - 6.0).abs() < 0.05, "{placement:?} {gain}");
            assert_eq!(other, sine(1000.0, 48_000), "{placement:?} touched the other channel");
        }
    }

    #[test]
    fn mid_and_side_placement_process_only_their_component() {
        // Identical channels are pure mid: a side band leaves them bit-exact, a mid band boosts.
        let mut p = BandProcessor::new(FS, 20.0);
        p.reset(&BandSettings { placement: Placement::Side, ..band(BandType::Bell, 1000.0, 6.0, 1.0) });
        let (gain, _) = gain_db_at(&mut p, 1000.0, 0, 1.0);
        assert!(gain.abs() < 1e-6, "side band changed mid signal: {gain}");
        let mut p = BandProcessor::new(FS, 20.0);
        p.reset(&BandSettings { placement: Placement::Mid, ..band(BandType::Bell, 1000.0, 6.0, 1.0) });
        let (gain, _) = gain_db_at(&mut p, 1000.0, 0, 1.0);
        assert!((gain - 6.0).abs() < 0.05, "{gain}");
        // Opposite channels are pure side.
        let mut p = BandProcessor::new(FS, 20.0);
        p.reset(&BandSettings { placement: Placement::Side, ..band(BandType::Bell, 1000.0, 6.0, 1.0) });
        let (gain, _) = gain_db_at(&mut p, 1000.0, 0, -1.0);
        assert!((gain - 6.0).abs() < 0.05, "{gain}");
    }

    #[test]
    fn placement_change_crossfades() {
        let s = band(BandType::Bell, 1000.0, 12.0, 1.0);
        let mut p = BandProcessor::new(FS, 20.0);
        p.reset(&s);
        let x = sine(1000.0, 4800);
        let (mut l, mut r) = (x.clone(), x.clone());
        p.process(&mut [&mut l, &mut r]);
        p.set_target(&BandSettings { placement: Placement::Left, ..s });
        let (mut l, mut r) = (x.clone(), x.clone());
        p.process(&mut [&mut l, &mut r]);
        let jump = r.windows(2).map(|w| (w[1] - w[0]).abs()).fold(0.0_f32, f32::max);
        // A 1 kHz sine at 12 dB (amplitude 2) moves at most ~0.27 per sample.
        assert!(jump < 0.3, "right channel jumped {jump}");
        assert_eq!(p.current().placement, Placement::Left);
    }

    #[test]
    fn every_cut_slope_is_butterworth_at_default_q() {
        // −3 dB at the cutoff and no overshoot anywhere, for every slope: changing the slope must
        // never add a bump.
        for kind in [BandType::LowCut, BandType::HighCut] {
            for (slope, db_oct) in SLOPES_DB.iter().enumerate() {
                let s = BandSettings { slope, ..band(kind, 1000.0, 0.0, FRAC_1_SQRT_2) };
                let at_cutoff = analog_response_db(&s, 1000.0);
                assert!((at_cutoff + 3.0103).abs() < 0.01, "{kind:?} {db_oct} dB/oct: {at_cutoff}");
                // Every corner, up to near Nyquist: corner-matched high-pass sections used to lift
                // the top end by up to 0.5 dB for corners above ~5 kHz.
                for f0 in [30.0, 1000.0, 5000.0, 12_000.0, 18_000.0] {
                    let sec = design(&BandSettings { freq: f0, ..s }, FS);
                    let peak = (1..4000).map(|i| sec.response_db(PI * f64::from(i) / 4000.0)).fold(f64::MIN, f64::max);
                    let limit = if kind == BandType::HighCut && db_oct % 12 != 0 { 0.25 } else { 0.01 };
                    assert!(peak < limit, "{kind:?} {db_oct} dB/oct at {f0} Hz overshoots {peak}");
                }
            }
        }
    }
}
