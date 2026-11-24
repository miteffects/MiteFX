//! Mite Shape's engine (BLUEPRINT §6.10): a transient designer with no threshold, on one band or
//! split into up to five by LR4 crossovers.
//!
//! Each band runs the EQ's transient detector ([`ShapeDetector`], a differential envelope follower)
//! on its own region: its attack amount follows each onset, its sustain amount each decay, and a
//! steady signal is left alone. The split is a cascade of LR4 crossovers with allpass
//! compensation, so the bands sum back to an allpass of the input: with one band there are no
//! filters at all and neutral settings null exactly; with more, the dry path for mix and delta is
//! the same band sum, so mixing never combs.
//!
//! Slots never swap (as Comp's stages): crossover slot k, when on, owns band slot k + 1, the
//! region above it up to the next crossover; band slot 0 is always the lowest region. The editor
//! numbers bands by frequency.

use crate::crossover::{Cross, Split};
use crate::eq::{DEFAULT_TRANSIENT_MS, DEFAULT_TRANSITION, ShapeDetector};
use crate::filter::{Coeffs, Tdf2};

pub use crate::crossover::{FREQ_MAX, FREQ_MIN, MIN_GAP_OCT};

pub const BANDS: usize = 5;
pub const CROSSOVERS: usize = BANDS - 1;
/// Attack and sustain amounts reach ±this, dB.
pub const AMOUNT_DB: f32 = 18.0;
pub const MAX_LOOKAHEAD_MS: f32 = 5.0;
/// Where each crossover slot sits when it is first made (the design's defaults), Hz.
pub const DEFAULT_CROSSOVERS: [f32; CROSSOVERS] = [120.0, 500.0, 2000.0, 8000.0];
/// The detector filter is open (not run) at or past these, Hz.
pub const DETECTOR_HP_OPEN: f32 = 20.5;
pub const DETECTOR_LP_OPEN: f32 = 19_900.0;

/// Samples between gain updates (the gain ramps linearly in between): 0.17 ms at 48 kHz.
const CONTROL: usize = 8;
/// Amounts, times, band mix and output and crossover frequencies glide with this time constant.
const GLIDE_MS: f64 = 20.0;
/// Solo and mute fade, ms.
const SOLO_MS: f64 = 10.0;
/// A change of the split (a band made or removed, L/R ↔ M/S) crossfades between the old and the
/// new structure over this long, ms.
const STRUCTURE_MS: f64 = 10.0;
/// Below the floor a lift fades out over this many dB.
const FLOOR_FADE_DB: f64 = 6.0;
/// Sustain lengths reach this long, ms; the top of the range is "endless" (`f32::INFINITY`).
pub const LENGTH_MAX_MS: f32 = 2000.0;
/// When a sustain length runs out, the gain returns to 0 dB along an S curve this long (a share
/// of the length, within these bounds, ms).
const LENGTH_FADE: f64 = 0.3;
const LENGTH_FADE_MS: (f64, f64) = (5.0, 150.0);
/// A hit is reported (for the history's tick) this long after it starts, when its sharpness has
/// grown to its peak, ms.
const HIT_REPORT_MS: f64 = 2.0;

/// One band's settings.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BandShape {
    pub attack_db: f32,
    pub sustain_db: f32,
    /// How long attack lasts (it has faded to about a twentieth by then), ms.
    pub transient_ms: f32,
    /// How gradually sustain comes in, as a share (0…2) of the transient time.
    pub transition: f32,
    /// The band's own dry/wet, 0…1.
    pub mix: f32,
    /// The band's fader in the sum, dB.
    pub out_db: f32,
    /// How long sustain acts after the transient time before the gain returns to 0 dB, ms
    /// (`f32::INFINITY`: for as long as the sound decays).
    pub length_ms: f32,
    pub mute: bool,
    pub solo: bool,
}

impl Default for BandShape {
    fn default() -> Self {
        Self {
            attack_db: 0.0,
            sustain_db: 0.0,
            transient_ms: DEFAULT_TRANSIENT_MS as f32,
            transition: DEFAULT_TRANSITION as f32,
            mix: 1.0,
            out_db: 0.0,
            length_ms: f32::INFINITY,
            mute: false,
            solo: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ShapeSettings {
    /// Each crossover slot's frequency, Hz, or `None` while it is off.
    pub crossovers: [Option<f32>; CROSSOVERS],
    pub bands: [BandShape; BANDS],
    /// What the detector hears with one band: high-pass and low-pass, Hz (open at the ends).
    pub detector_hp: f32,
    pub detector_lp: f32,
    /// Hear what the detector hears instead of the result.
    pub listen: bool,
    pub lookahead_ms: f32,
    pub mid_side: bool,
    /// How tightly the two channels' detectors are tied, 0…1.
    pub link: f32,
    /// A lift (positive gain) fades out below this detected level, dBFS.
    pub floor_db: f32,
}

impl Default for ShapeSettings {
    fn default() -> Self {
        Self {
            crossovers: [None; CROSSOVERS],
            bands: [BandShape::default(); BANDS],
            detector_hp: FREQ_MIN,
            detector_lp: FREQ_MAX,
            listen: false,
            lookahead_ms: 0.0,
            mid_side: false,
            link: 1.0,
            floor_db: -60.0,
        }
    }
}

impl ShapeSettings {
    pub fn latency(&self, sample_rate: f32) -> usize {
        lookahead_samples(self.lookahead_ms, sample_rate)
    }
}

fn lookahead_samples(ms: f32, sample_rate: f32) -> usize {
    (ms.clamp(0.0, MAX_LOOKAHEAD_MS) * 0.001 * sample_rate).round() as usize
}

/// The split the crossovers make (`crate::crossover::Layout` for five bands).
pub type Layout = crate::crossover::Layout<BANDS>;

/// The structure a core runs: the split and the stereo mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Key {
    layout: Layout,
    mid_side: bool,
}

/// One band slot's detector side, per channel.
#[derive(Debug, Clone, Copy)]
struct BandState {
    det: [ShapeDetector; 2],
    /// The linear gain applied now and its per-sample step to the next update.
    gain: [f64; 2],
    step: [f64; 2],
    /// The hit last reported for the history (its age when seen), so each is reported once.
    reported: bool,
    last_age: u32,
}

impl BandState {
    fn new(fs: f64) -> Self {
        let det = ShapeDetector::new(fs, DEFAULT_TRANSIENT_MS, DEFAULT_TRANSITION);
        Self {
            det: [det; 2],
            gain: [1.0; 2],
            step: [0.0; 2],
            reported: true,
            last_age: 0,
        }
    }
}

/// Everything one structure runs: filter states, band delays and detectors. Two of them crossfade
/// when the structure changes. Allocation-free once sized.
struct Core {
    key: Key,
    split: Split<BANDS>,
    /// The detector filter by channel: high-pass ×2, low-pass ×2.
    det_filter: [[Tdf2; 4]; 2],
    bands: [BandState; BANDS],
    /// Lookahead delays: band slot × channel rings, then the Listen ring per channel.
    rings: Vec<f32>,
    ring_len: usize,
}

impl Core {
    fn new(fs: f64, ring_len: usize) -> Self {
        Self {
            key: Key {
                layout: Layout::new(&[None; CROSSOVERS]),
                mid_side: false,
            },
            split: Split::default(),
            det_filter: [[Tdf2::default(); 4]; 2],
            bands: [BandState::new(fs); BANDS],
            rings: vec![0.0; (BANDS + 1) * 2 * ring_len],
            ring_len,
        }
    }

    fn reset(&mut self) {
        self.split.reset();
        self.det_filter = [[Tdf2::default(); 4]; 2];
        for b in &mut self.bands {
            for d in &mut b.det {
                d.reset(0.0);
            }
            b.gain = [1.0; 2];
            b.step = [0.0; 2];
            b.reported = true;
        }
        self.rings.fill(0.0);
    }

    /// Becomes a copy of `other` (no allocation: both are sized alike).
    fn copy_from(&mut self, other: &Core) {
        self.key = other.key;
        self.split = other.split;
        self.det_filter = other.det_filter;
        self.bands = other.bands;
        self.rings.copy_from_slice(&other.rings);
    }

    fn ring(&self, slot: usize, c: usize) -> std::ops::Range<usize> {
        ring(self.ring_len, slot, c)
    }

    /// Moves to `key` from the structure it holds: a new crossover starts from rest and its new
    /// band from the band it split; an L/R ↔ M/S switch carries every linear state across.
    fn restructure(&mut self, key: Key, crossovers: &[Option<f32>; CROSSOVERS]) {
        let old = self.key;
        if old.mid_side != key.mid_side {
            let (ka, kb) = if key.mid_side { (0.5, 0.5) } else { (1.0, 1.0) };
            let mix = |a: &mut Tdf2, b: &mut Tdf2| {
                let (x, y) = (*a, *b);
                *a = x.combine(&y, ka, kb);
                *b = x.combine(&y, ka, -kb);
            };
            self.split.switch_mid_side(key.mid_side);
            for s in 0..4 {
                let (l, r) = self.det_filter.split_at_mut(1);
                mix(&mut l[0][s], &mut r[0][s]);
            }
            for slot in 0..=BANDS {
                let (a, b) = (self.ring(slot, 0), self.ring(slot, 1));
                for i in 0..self.ring_len {
                    let (x, y) = (
                        f64::from(self.rings[a.start + i]),
                        f64::from(self.rings[b.start + i]),
                    );
                    self.rings[a.start + i] = (ka * x + kb * y) as f32;
                    self.rings[b.start + i] = (ka * x - kb * y) as f32;
                }
            }
        }
        // A band merged away: what its lookahead holds joins the band below it, which takes its
        // region, so nothing drops out for the lookahead's length.
        for p in 1..=old.layout.n {
            let gone = old.layout.owners[p];
            if key.layout.position(gone).is_some() {
                continue;
            }
            let into = (0..p)
                .rev()
                .map(|q| old.layout.owners[q])
                .find(|&b| key.layout.position(b).is_some())
                .unwrap_or(0);
            for c in 0..2 {
                let (from, to) = (self.ring(gone, c), self.ring(into, c));
                for i in 0..self.ring_len {
                    self.rings[to.start + i] += self.rings[from.start + i];
                }
            }
        }
        // A new band takes its parent's detector and gain, but starts with an empty lookahead:
        // the parent still holds the whole region it delayed and plays it out.
        let parents = self.split.restructure(&old.layout, &key.layout, crossovers);
        for (slot, parent) in parents.iter().enumerate() {
            let Some(parent) = *parent else { continue };
            self.bands[slot] = self.bands[parent];
            for c in 0..2 {
                let r = self.ring(slot, c);
                self.rings[r].fill(0.0);
            }
        }
        self.key = key;
    }
}

/// Where band slot `slot`'s ring for channel `c` lies (slot `BANDS` is Listen's).
fn ring(len: usize, slot: usize, c: usize) -> std::ops::Range<usize> {
    let start = (slot * 2 + c) * len;
    start..start + len
}

/// Per-chunk readings for the history, by band slot.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ShapeTelemetry {
    /// The gain with the largest size this chunk, per channel, dB (signed).
    pub gain_db: [[f32; 2]; BANDS],
    /// The attack weight at its highest (0…1).
    pub weight: [f32; BANDS],
    /// The sharpness (0…1) of a hit reported this chunk, else 0.
    pub hit: [f32; BANDS],
    /// The band's peak level as its detector hears it, dBFS.
    pub level_db: [f32; BANDS],
}

impl Default for ShapeTelemetry {
    fn default() -> Self {
        Self {
            gain_db: [[0.0; 2]; BANDS],
            weight: [0.0; BANDS],
            hit: [0.0; BANDS],
            level_db: [crate::MINUS_INFINITY_DB; BANDS],
        }
    }
}

/// A band's settings as they glide (the values the cores use).
#[derive(Debug, Clone, Copy)]
struct Eased {
    attack: f64,
    sustain: f64,
    transient_ms: f64,
    transition: f64,
    mix: f64,
    out: f64,
    length_ms: f64,
    audible: f64,
    /// The times the detectors were last set to.
    times: (f64, f64),
}

impl Eased {
    fn at(b: &BandShape, audible: bool) -> Self {
        let times = (f64::from(b.transient_ms), f64::from(b.transition));
        Self {
            attack: f64::from(b.attack_db),
            sustain: f64::from(b.sustain_db),
            transient_ms: times.0,
            transition: times.1,
            mix: f64::from(b.mix),
            out: crate::db_to_gain(b.out_db).into(),
            length_ms: f64::from(b.length_ms),
            audible: if audible { 1.0 } else { 0.0 },
            times,
        }
    }
}

pub struct Shaper {
    fs: f64,
    cores: [Core; 2],
    /// The core playing, and the crossfade into the other (its position 0…1) while one runs.
    active: usize,
    fade: Option<f64>,
    eased: [Eased; BANDS],
    /// Crossover frequencies as they glide (ln Hz) and their sections.
    log_freq: [f64; CROSSOVERS],
    cross: [Cross; CROSSOVERS],
    /// The detector filter's sections (high-pass, low-pass) and whether each runs.
    det_hp: (Coeffs, bool, f32),
    det_lp: (Coeffs, bool, f32),
    countdown: usize,
    write: usize,
    /// Scratch for one sample's band signals.
    glide: f64,
    solo_step: f64,
    fade_step: f64,
    hit_report: u32,
}

/// Which bands are heard: muted ones are not; with any soloed, only those.
fn audible(s: &ShapeSettings, layout: &Layout) -> [bool; BANDS] {
    let any_solo = layout.bands().iter().any(|&b| s.bands[b].solo);
    std::array::from_fn(|b| !s.bands[b].mute && (!any_solo || s.bands[b].solo))
}

impl Shaper {
    /// Allocates.
    pub fn new(sample_rate: f32, s: &ShapeSettings) -> Self {
        let mut shaper = Self {
            fs: f64::from(sample_rate),
            cores: [
                Core::new(f64::from(sample_rate), 1),
                Core::new(f64::from(sample_rate), 1),
            ],
            active: 0,
            fade: None,
            eased: [Eased::at(&BandShape::default(), true); BANDS],
            log_freq: DEFAULT_CROSSOVERS.map(|f| f64::from(f).ln()),
            cross: DEFAULT_CROSSOVERS.map(|f| Cross::new(f64::from(f), f64::from(sample_rate))),
            det_hp: (Coeffs::IDENTITY, false, 0.0),
            det_lp: (Coeffs::IDENTITY, false, 0.0),
            countdown: 0,
            write: 0,
            glide: 0.0,
            solo_step: 0.0,
            fade_step: 0.0,
            hit_report: 1,
        };
        shaper.prepare(sample_rate, s);
        shaper
    }

    /// Sizes everything for `sample_rate`. Allocates.
    pub fn prepare(&mut self, sample_rate: f32, s: &ShapeSettings) {
        self.fs = f64::from(sample_rate);
        let ring_len = lookahead_samples(MAX_LOOKAHEAD_MS, sample_rate) + 1;
        self.cores = [Core::new(self.fs, ring_len), Core::new(self.fs, ring_len)];
        self.glide = 1.0 - (-(CONTROL as f64) * 1000.0 / (GLIDE_MS * self.fs)).exp();
        self.solo_step = CONTROL as f64 * 1000.0 / (SOLO_MS * self.fs);
        self.fade_step = 1000.0 / (STRUCTURE_MS * self.fs);
        self.hit_report = (HIT_REPORT_MS * 0.001 * self.fs).round() as u32;
        self.reset(s);
    }

    /// Clears the state and snaps every glide to `s`.
    #[allow(clippy::needless_range_loop)] // slots indexed across several arrays
    pub fn reset(&mut self, s: &ShapeSettings) {
        let key = Key {
            layout: Layout::new(&s.crossovers),
            mid_side: s.mid_side,
        };
        let heard = audible(s, &key.layout);
        for (b, e) in self.eased.iter_mut().enumerate() {
            *e = Eased::at(&s.bands[b], heard[b]);
        }
        for k in 0..CROSSOVERS {
            let f = s.crossovers[k].unwrap_or(DEFAULT_CROSSOVERS[k]);
            self.log_freq[k] = f64::from(f).ln();
            self.cross[k] = Cross::new(f64::from(f), self.fs);
        }
        self.set_detector_filter(s);
        for core in &mut self.cores {
            core.reset();
            core.key = key;
            for (b, band) in core.bands.iter_mut().enumerate() {
                for d in &mut band.det {
                    d.set_times(
                        self.fs,
                        f64::from(s.bands[b].transient_ms),
                        f64::from(s.bands[b].transition),
                    );
                }
            }
        }
        self.active = 0;
        self.fade = None;
        self.countdown = 0;
        self.write = 0;
    }

    /// The structure playing now (it follows a change once its crossfade is done).
    pub fn layout(&self) -> Layout {
        self.cores[self.active].key.layout
    }

    fn set_detector_filter(&mut self, s: &ShapeSettings) {
        let single = Layout::new(&s.crossovers).n == 0;
        let hp_on = single && s.detector_hp > DETECTOR_HP_OPEN;
        let lp_on =
            single && s.detector_lp < DETECTOR_LP_OPEN && f64::from(s.detector_lp) < 0.45 * self.fs;
        if hp_on && (!self.det_hp.1 || self.det_hp.2 != s.detector_hp) {
            self.det_hp.0 = Cross::new(f64::from(s.detector_hp), self.fs).hp;
        }
        if lp_on && (!self.det_lp.1 || self.det_lp.2 != s.detector_lp) {
            self.det_lp.0 = Cross::new(f64::from(s.detector_lp), self.fs).lp;
        }
        self.det_hp.1 = hp_on;
        self.det_hp.2 = s.detector_hp;
        self.det_lp.1 = lp_on;
        self.det_lp.2 = s.detector_lp;
    }

    /// Once per control interval: glides the settings, follows a structure change, and works out
    /// every band's next gain in both cores.
    #[allow(clippy::needless_range_loop)] // slots indexed across several arrays
    fn control(&mut self, s: &ShapeSettings, n_ch: usize, telemetry: &mut ShapeTelemetry) {
        let key = Key {
            layout: Layout::new(&s.crossovers),
            mid_side: s.mid_side && n_ch == 2,
        };
        // A new structure: copy the playing core, restructure it, crossfade into it.
        if self.fade.is_none() && key != self.cores[self.active].key {
            let next = 1 - self.active;
            let (a, b) = self.cores.split_at_mut(1);
            let (from, to) = if next == 1 {
                (&a[0], &mut b[0])
            } else {
                (&b[0], &mut a[0])
            };
            to.copy_from(from);
            // A crossover just switched on starts where it is set, not gliding from where it was.
            for p in 0..key.layout.n {
                let k = key.layout.crossovers[p];
                if !from.key.layout.crossovers[..from.key.layout.n].contains(&k) {
                    let f = s.crossovers[k].unwrap_or(DEFAULT_CROSSOVERS[k]);
                    self.log_freq[k] = f64::from(f).ln();
                    self.cross[k] = Cross::new(f64::from(f), self.fs);
                }
            }
            to.restructure(key, &s.crossovers);
            self.fade = Some(0.0);
            // Every detector takes its band's times again (a new band came from another).
            for e in &mut self.eased {
                e.times = (-1.0, -1.0);
            }
        }
        for k in 0..CROSSOVERS {
            if let Some(f) = s.crossovers[k] {
                let target = f64::from(f.clamp(FREQ_MIN, FREQ_MAX)).ln();
                let d = target - self.log_freq[k];
                if d.abs() > 1e-6 {
                    self.log_freq[k] = if d.abs() < 1e-4 {
                        target
                    } else {
                        self.log_freq[k] + self.glide * d
                    };
                    self.cross[k] = Cross::new(self.log_freq[k].exp(), self.fs);
                }
            }
        }
        self.set_detector_filter(s);
        let heard = audible(s, &key.layout);
        let ease = |v: &mut f64, target: f64, k: f64| {
            let d = target - *v;
            *v = if d.abs() < 1e-5 { target } else { *v + k * d };
        };
        for b in 0..BANDS {
            let t = &s.bands[b];
            let e = &mut self.eased[b];
            ease(
                &mut e.attack,
                f64::from(t.attack_db.clamp(-AMOUNT_DB, AMOUNT_DB)),
                self.glide,
            );
            ease(
                &mut e.sustain,
                f64::from(t.sustain_db.clamp(-AMOUNT_DB, AMOUNT_DB)),
                self.glide,
            );
            ease(&mut e.transient_ms, f64::from(t.transient_ms), self.glide);
            ease(&mut e.transition, f64::from(t.transition), self.glide);
            ease(&mut e.mix, f64::from(t.mix.clamp(0.0, 1.0)), self.glide);
            ease(
                &mut e.out,
                f64::from(crate::db_to_gain(t.out_db)),
                self.glide,
            );
            // Lengths glide in octaves; to or from endless they jump (the S curve is the glide).
            let length = f64::from(t.length_ms);
            if length.is_finite() && e.length_ms.is_finite() {
                let mut ln = e.length_ms.ln();
                ease(&mut ln, length.ln(), self.glide);
                e.length_ms = ln.exp();
            } else {
                e.length_ms = length;
            }
            let a = if heard[b] { 1.0 } else { 0.0 };
            e.audible = if a > e.audible {
                (e.audible + self.solo_step).min(a)
            } else {
                (e.audible - self.solo_step).max(a)
            };
        }
        let floor = f64::from(s.floor_db);
        let fading = self.fade.is_some();
        for ci in 0..2 {
            if ci != self.active && !fading {
                continue;
            }
            let core = &mut self.cores[ci];
            let shown = ci == if fading { 1 - self.active } else { self.active };
            for &b in core.key.layout.bands() {
                let e = self.eased[b];
                let band = &mut core.bands[b];
                let retime = (e.transient_ms, e.transition) != e.times;
                for c in 0..n_ch {
                    let det = &mut band.det[c];
                    if retime {
                        det.set_times(self.fs, e.transient_ms, e.transition);
                    }
                    // A lift fades out below the floor, so a tail is never pulled up out of noise.
                    let level_db = 20.0 * det.level().max(1e-10).log10();
                    let lift = ((level_db - floor + FLOOR_FADE_DB) / FLOOR_FADE_DB).clamp(0.0, 1.0);
                    let sustain = if e.sustain > 0.0 {
                        e.sustain * lift
                    } else {
                        e.sustain
                    };
                    let since = det.since_hit().map(|n| f64::from(n) * 1000.0 / self.fs);
                    let g = det.gain_db(
                        e.attack,
                        sustain * sustain_scale(e.transient_ms, e.length_ms, since),
                    );
                    let target = 10f64.powf(g / 20.0);
                    band.step[c] = (target - band.gain[c]) / CONTROL as f64;
                    if shown {
                        let t = &mut telemetry.gain_db[b][c];
                        if g.abs() > f64::from(t.abs()) {
                            *t = g as f32;
                        }
                    }
                }
                if shown {
                    let det = &band.det[0];
                    telemetry.weight[b] = telemetry.weight[b].max(det.weight() as f32);
                    match det.hit_age() {
                        Some(age) => {
                            if age < band.last_age {
                                band.reported = false;
                            }
                            band.last_age = age;
                            if !band.reported && age >= self.hit_report {
                                band.reported = true;
                                telemetry.hit[b] = telemetry.hit[b].max(det.sharpness() as f32);
                            }
                        }
                        None => {
                            band.reported = false;
                            band.last_age = 0;
                        }
                    }
                }
            }
        }
        for e in &mut self.eased {
            e.times = (e.transient_ms, e.transition);
        }
    }

    /// The lookahead in force, samples (the latency).
    pub fn latency(&self, s: &ShapeSettings) -> usize {
        s.latency(self.fs as f32)
    }

    /// Shapes one chunk in place (`channels`: one or two) and writes the dry band sum (the
    /// allpass the split makes, delayed like the result) into `dry`, for mix and delta.
    #[allow(clippy::needless_range_loop)] // bands and channels indexed across several arrays
    pub fn process(
        &mut self,
        channels: &mut [&mut [f32]],
        dry: &mut [&mut [f32]],
        s: &ShapeSettings,
    ) -> ShapeTelemetry {
        let mut telemetry = ShapeTelemetry::default();
        let n_ch = channels.len().min(2);
        if n_ch == 0 {
            return telemetry;
        }
        let len = channels.iter().map(|c| c.len()).min().unwrap_or(0);
        let delay = s.latency(self.fs as f32).min(self.cores[0].ring_len - 1);
        let link = f64::from(s.link.clamp(0.0, 1.0));
        let (hp, lp) = (self.det_hp, self.det_lp);
        for i in 0..len {
            if self.countdown == 0 {
                self.control(s, n_ch, &mut telemetry);
                self.countdown = CONTROL;
            }
            self.countdown -= 1;
            let x = [f64::from(channels[0][i]), f64::from(channels[n_ch - 1][i])];
            let fading = self.fade;
            let mut out = [[0.0f64; 2]; 2];
            let mut dry_out = [[0.0f64; 2]; 2];
            for ci in 0..2 {
                if ci != self.active && fading.is_none() {
                    continue;
                }
                let core = &mut self.cores[ci];
                let key = core.key;
                let layout = key.layout;
                let ms = key.mid_side;
                let input = if ms {
                    [0.5 * (x[0] + x[1]), 0.5 * (x[0] - x[1])]
                } else {
                    x
                };
                // Split each channel into its bands (by position from the bottom).
                let mut sig = [[0.0f64; BANDS]; 2];
                let mut listen = [0.0f64; 2];
                for c in 0..n_ch {
                    core.split
                        .process(c, input[c], &layout, &self.cross, &mut sig[c]);
                    if layout.n == 0 {
                        // One band: the detector hears the input through its own filter.
                        let f = &mut core.det_filter[c];
                        let mut d = input[c];
                        if hp.1 {
                            let t = f[0].process(&hp.0, d);
                            d = f[1].process(&hp.0, t);
                        }
                        if lp.1 {
                            let t = f[2].process(&lp.0, d);
                            d = f[3].process(&lp.0, t);
                        }
                        listen[c] = d;
                    }
                }
                // Detect, then apply the gains to the delayed bands.
                let w = self.write;
                let read = (w + core.ring_len - delay) % core.ring_len;
                let shown = ci
                    == if fading.is_some() {
                        1 - self.active
                    } else {
                        self.active
                    };
                let mut y = [0.0f64; 2];
                let mut d_sum = [0.0f64; 2];
                for p in 0..=layout.n {
                    let b = layout.owners[p];
                    let heard_now = if layout.n == 0 {
                        listen
                    } else {
                        [sig[0][p], sig[n_ch - 1][p]]
                    };
                    let lv = [heard_now[0].abs(), heard_now[n_ch - 1].abs()];
                    let both = lv[0].max(lv[1]);
                    let e = self.eased[b];
                    let band = &mut core.bands[b];
                    for c in 0..n_ch {
                        band.det[c].process(lv[c] + link * (both - lv[c]));
                    }
                    if shown {
                        let peak = 20.0 * both.max(1e-10).log10();
                        telemetry.level_db[b] = telemetry.level_db[b].max(peak as f32);
                    }
                    for c in 0..n_ch {
                        band.gain[c] += band.step[c];
                        let r = ring(core.ring_len, b, c);
                        // Written even with no lookahead: a lookahead switched on later reads
                        // the last moments from here (unwritten, they were stale, and the sound
                        // jumped back in a lookahead after the latency dip: the §10 ramp test).
                        core.rings[r.start + w] = sig[c][p] as f32;
                        let delayed = if delay == 0 { sig[c][p] } else { f64::from(core.rings[r.start + read]) };
                        let g = band.gain[c];
                        y[c] += delayed * ((1.0 - e.mix) + e.mix * g) * e.out * e.audible;
                        d_sum[c] += delayed;
                    }
                }
                if s.listen {
                    // What the detector hears, delayed like the result.
                    for c in 0..n_ch {
                        y[c] = if layout.n == 0 {
                            let r = ring(core.ring_len, BANDS, c);
                            core.rings[r.start + w] = listen[c] as f32;
                            if delay == 0 { listen[c] } else { f64::from(core.rings[r.start + read]) }
                        } else {
                            // Several bands: the bands as they are heard, unshaped.
                            let mut v = 0.0;
                            for p in 0..=layout.n {
                                let b = layout.owners[p];
                                let r = ring(core.ring_len, b, c);
                                let delayed = if delay == 0 {
                                    sig[c][p]
                                } else {
                                    f64::from(core.rings[r.start + read])
                                };
                                v += delayed * self.eased[b].audible;
                            }
                            v
                        };
                    }
                }
                if ms {
                    y = [y[0] + y[1], y[0] - y[1]];
                    d_sum = [d_sum[0] + d_sum[1], d_sum[0] - d_sum[1]];
                }
                out[ci] = y;
                dry_out[ci] = d_sum;
            }
            self.write = (self.write + 1) % self.cores[0].ring_len;
            let (y, d) = match fading {
                Some(t) => {
                    // An S-shaped crossfade into the new structure.
                    let u = t * t * (3.0 - 2.0 * t);
                    let (a, b) = (self.active, 1 - self.active);
                    let mix = |o: &[[f64; 2]; 2], c: usize| o[a][c] * (1.0 - u) + o[b][c] * u;
                    (
                        [mix(&out, 0), mix(&out, 1)],
                        [mix(&dry_out, 0), mix(&dry_out, 1)],
                    )
                }
                None => (out[self.active], dry_out[self.active]),
            };
            if let Some(t) = fading {
                let t = t + self.fade_step;
                if t >= 1.0 {
                    self.active = 1 - self.active;
                    self.fade = None;
                } else {
                    self.fade = Some(t);
                }
            }
            for c in 0..n_ch {
                channels[c][i] = y[c] as f32;
                if let Some(ch) = dry.get_mut(c) {
                    ch[i] = d[c] as f32;
                }
            }
        }
        telemetry
    }
}

/// How much of the sustain amount applies `since_ms` after a hit started: all of it through the
/// transient time and for `length_ms` after it, then back to none along an S curve
/// (smoothstep, 30 % of the length, 5–150 ms). Endless lengths keep it; before any hit, a finite
/// length has nothing to sustain.
pub fn sustain_scale(transient_ms: f64, length_ms: f64, since_ms: Option<f64>) -> f64 {
    if !length_ms.is_finite() {
        return 1.0;
    }
    let Some(since) = since_ms else { return 0.0 };
    let fade = (LENGTH_FADE * length_ms).clamp(LENGTH_FADE_MS.0, LENGTH_FADE_MS.1);
    let u = ((since - transient_ms - length_ms) / fade).clamp(0.0, 1.0);
    1.0 - u * u * (3.0 - 2.0 * u)
}

/// One point of a synthetic hit through a band's detector, as the editor's scopes draw it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HitPoint {
    /// Time since the hit (as it is heard), ms.
    pub t_ms: f64,
    /// The hit's level as it comes in, dBFS.
    pub level_db: f64,
    /// The band's gain then, dB, and its attack weight.
    pub gain_db: f64,
    pub weight: f64,
}

/// A test hit (a 3 ms rise into a 145 ms decay, peaking at −6 dBFS, after silence) through a band's
/// real detector at `rate` levels per second, from `-pre_ms` to `span_ms`. With lookahead the gain
/// leads the hit, as it does in the plugin.
pub fn simulate_hit(
    band: &BandShape,
    lookahead_ms: f64,
    rate: f64,
    pre_ms: f64,
    span_ms: f64,
) -> Vec<HitPoint> {
    let hit = |t_ms: f64| {
        if t_ms < 0.0 {
            0.0
        } else {
            (1.0 - (-t_ms / 3.0).exp()) * (-t_ms / 145.0).exp()
        }
    };
    let peak = (0..2000)
        .map(|i| hit(f64::from(i) * 0.1))
        .fold(0.0, f64::max);
    let level = |t_ms: f64| 0.5 * hit(t_ms) / peak;
    let mut det = ShapeDetector::new(
        rate,
        f64::from(band.transient_ms),
        f64::from(band.transition),
    );
    let dt = 1000.0 / rate;
    let steps = ((pre_ms + span_ms) / dt).ceil() as usize + 1;
    let lead = (lookahead_ms / dt).round() as usize;
    // The detector runs `lead` steps ahead of what is heard.
    let mut gains = Vec::with_capacity(steps + lead);
    for i in 0..steps + lead {
        let t = -pre_ms + i as f64 * dt;
        det.process(level(t));
        let since = det.since_hit().map(|n| f64::from(n) * dt);
        let scale = sustain_scale(
            f64::from(band.transient_ms),
            f64::from(band.length_ms),
            since,
        );
        let g = det.gain_db(
            f64::from(band.attack_db),
            f64::from(band.sustain_db) * scale,
        );
        gains.push((g, det.weight()));
    }
    (0..steps)
        .map(|i| {
            let t = -pre_ms + i as f64 * dt;
            let (gain_db, weight) = if i + lead < gains.len() {
                gains[i + lead]
            } else {
                (0.0, 0.0)
            };
            HitPoint {
                t_ms: t,
                level_db: 20.0 * level(t).max(1e-6).log10(),
                gain_db,
                weight,
            }
        })
        .collect()
}

/// The clip stage's curve: unchanged up to `(1 − softness) · ceiling`, then easing into the
/// ceiling (tanh), so the slope never jumps and the peak never passes it; `softness` 0 is a hard
/// clip. Odd-symmetric. `ceiling` linear, `softness` 0…1.
#[inline]
pub fn soft_clip(x: f32, ceiling: f32, softness: f32) -> f32 {
    let knee = ceiling * (1.0 - softness.clamp(0.0, 1.0));
    let a = x.abs();
    if a <= knee {
        return x;
    }
    let room = ceiling - knee;
    let y = if room <= 1e-9 {
        ceiling
    } else {
        knee + room * ((a - knee) / room).tanh()
    };
    y.copysign(x)
}

#[cfg(test)]
mod tests {
    use std::f64::consts::PI;

    use super::*;

    const FS: f32 = 48_000.0;

    fn noise(n: usize, seed: u32) -> Vec<f32> {
        let mut x = seed;
        (0..n)
            .map(|_| {
                x = x.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                ((x >> 8) as f32 / (1u32 << 24) as f32 - 0.5) * 0.5
            })
            .collect()
    }

    /// A drum-like test signal: noise bursts with fast attacks and decays, 8 per second.
    fn hits(n: usize) -> Vec<f32> {
        let raw = noise(n, 7);
        (0..n)
            .map(|i| raw[i] * (-((i % 6000) as f32) / 900.0).exp())
            .collect()
    }

    fn run(
        shaper: &mut Shaper,
        input: &[f32],
        s: &ShapeSettings,
        block: usize,
    ) -> (Vec<f32>, Vec<f32>) {
        let mut y = input.to_vec();
        let mut d = vec![0.0; input.len()];
        for (a, b) in y.chunks_mut(block).zip(d.chunks_mut(block)) {
            shaper.process(&mut [a], &mut [b], s);
        }
        (y, d)
    }

    fn split3() -> ShapeSettings {
        ShapeSettings {
            crossovers: [Some(150.0), Some(3500.0), None, None],
            ..ShapeSettings::default()
        }
    }

    /// A fixed render through splits, a band made, an M/S switch and lookahead, hashed bit for bit:
    /// the crossover's extraction into `crate::crossover` must leave Shape's output unchanged.
    /// (Re-blessed 2026-10-26: with lookahead, a new band now starts with an empty delay and a
    /// merged band's delay joins the band below. Re-blessed 2026-10-30: the differential detector.
    /// Re-blessed 2026-11-24: the split on state-variable sections, within −250 dB of the biquads.)
    #[test]
    fn render_checksum_is_unchanged() {
        let n = 20_000;
        let l = hits(n);
        let r: Vec<f32> = noise(n, 11)
            .iter()
            .zip(&l)
            .map(|(a, b)| 0.3 * a + 0.8 * b)
            .collect();
        let mut s = split3();
        s.lookahead_ms = 2.0;
        s.bands[0].attack_db = 6.0;
        s.bands[1].sustain_db = -8.0;
        s.bands[2].attack_db = -4.0;
        s.bands[2].sustain_db = 5.0;
        let mut sh = Shaper::new(FS, &s);
        let (mut yl, mut yr) = (l.clone(), r.clone());
        let (mut dl, mut dr) = (vec![0.0; n], vec![0.0; n]);
        let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
        let mut at = 0;
        for (k, block) in [64usize, 333, 1, 1024, 7].iter().cycle().enumerate() {
            if at >= n {
                break;
            }
            if k == 6 {
                s.crossovers[3] = Some(900.0);
            }
            if k == 12 {
                s.mid_side = true;
            }
            if k == 20 {
                s.crossovers[0] = None;
            }
            let end = (at + block).min(n);
            sh.process(
                &mut [&mut yl[at..end], &mut yr[at..end]],
                &mut [&mut dl[at..end], &mut dr[at..end]],
                &s,
            );
            at = end;
        }
        for v in yl.iter().chain(&yr).chain(&dl).chain(&dr) {
            hash = (hash ^ u64::from(v.to_bits())).wrapping_mul(0x0100_0000_01b3);
        }
        assert_eq!(hash, 0x2fee_34bc_a624_81ed, "checksum {hash:#x}");
    }

    #[test]
    fn neutral_single_band_is_exact() {
        let s = ShapeSettings::default();
        let input = hits(24_000);
        let mut sh = Shaper::new(FS, &s);
        let (y, d) = run(&mut sh, &input, &s, 256);
        assert_eq!(y, input);
        assert_eq!(d, input);
    }

    #[test]
    fn the_bands_sum_flat() {
        // Neutral with three bands: the result is the dry band sum (an allpass), flat in magnitude.
        let s = split3();
        let mut sh = Shaper::new(FS, &s);
        let mut impulse = vec![0.0f32; 16_384];
        impulse[0] = 1.0;
        let (y, d) = run(&mut sh, &impulse, &s, 512);
        assert!(y.iter().zip(&d).all(|(a, b)| (a - b).abs() < 1e-6));
        for f in [30.0, 150.0, 700.0, 3500.0, 12_000.0] {
            let w = 2.0 * PI * f / f64::from(FS);
            let (re, im) = y.iter().enumerate().fold((0.0, 0.0), |(re, im), (i, &v)| {
                (
                    re + f64::from(v) * (w * i as f64).cos(),
                    im - f64::from(v) * (w * i as f64).sin(),
                )
            });
            let db = 10.0 * (re * re + im * im).log10();
            assert!(db.abs() < 0.01, "{f} Hz: {db:.4} dB");
        }
    }

    #[test]
    fn weights_add_to_one_and_follow_the_regions() {
        let s = split3();
        let layout = Layout::new(&s.crossovers);
        assert_eq!(layout.bands(), &[0, 1, 2]);
        for f in [20.0, 100.0, 150.0, 1000.0, 3500.0, 19_000.0] {
            let w = layout.weights(&s.crossovers, f);
            assert!((w.iter().sum::<f64>() - 1.0).abs() < 1e-12, "{f}");
        }
        assert!(layout.weights(&s.crossovers, 40.0)[0] > 0.99);
        assert!(layout.weights(&s.crossovers, 900.0)[1] > 0.99);
        assert!(layout.weights(&s.crossovers, 15_000.0)[2] > 0.99);
        // A crossover made below the others owns the region above it, up to the next.
        let mut x = s.crossovers;
        x[3] = Some(60.0);
        let l = Layout::new(&x);
        assert_eq!(l.bands(), &[0, 4, 1, 2]);
        assert_eq!(l.region(4, &x), Some((60.0, 150.0)));
    }

    /// Peak level over windows right after each burst's onset and in its tail, dB re the input.
    fn attack_and_tail(y: &[f32], x: &[f32], from: usize) -> (f64, f64) {
        let rms = |v: &[f32], a: usize, b: usize| {
            (v[a..b].iter().map(|s| f64::from(*s).powi(2)).sum::<f64>() / (b - a) as f64).sqrt()
        };
        let (mut att, mut tail) = (0.0, 0.0);
        let mut k = 0;
        for start in (from..y.len() - 6000).step_by(6000) {
            att += 20.0 * (rms(y, start, start + 240) / rms(x, start, start + 240)).log10();
            tail += 20.0
                * (rms(y, start + 3000, start + 5000) / rms(x, start + 3000, start + 5000)).log10();
            k += 1;
        }
        (att / f64::from(k), tail / f64::from(k))
    }

    #[test]
    fn attack_up_and_sustain_down_shape_each_hit() {
        let mut s = ShapeSettings::default();
        s.bands[0].attack_db = 9.0;
        s.bands[0].sustain_db = -9.0;
        let input = hits(48_000);
        let mut sh = Shaper::new(FS, &s);
        let (y, _) = run(&mut sh, &input, &s, 333);
        let (att, tail) = attack_and_tail(&y, &input, 12_000);
        assert!(att > 4.0, "attack {att:.1} dB");
        assert!(tail < -7.0, "tail {tail:.1} dB");
    }

    #[test]
    fn the_shape_does_not_depend_on_level() {
        let mut s = ShapeSettings::default();
        s.bands[0].attack_db = 9.0;
        s.bands[0].sustain_db = -6.0;
        s.floor_db = -80.0;
        let input = hits(48_000);
        let quiet: Vec<f32> = input.iter().map(|v| v * 0.2).collect();
        let (mut a, mut b) = (Shaper::new(FS, &s), Shaper::new(FS, &s));
        let (ya, _) = run(&mut a, &input, &s, 256);
        let (yb, _) = run(&mut b, &quiet, &s, 256);
        let (la, lb) = (
            attack_and_tail(&ya, &input, 12_000),
            attack_and_tail(&yb, &quiet, 12_000),
        );
        assert!(
            (la.0 - lb.0).abs() < 0.5 && (la.1 - lb.1).abs() < 0.5,
            "{la:?} against {lb:?}"
        );
    }

    #[test]
    fn block_size_does_not_change_the_output() {
        let mut s = split3();
        s.bands[1].attack_db = 6.0;
        s.bands[2].sustain_db = -6.0;
        s.lookahead_ms = 2.0;
        let input = hits(20_000);
        let mut reference = None;
        for block in [1, 7, 64, 512] {
            let mut sh = Shaper::new(FS, &s);
            let (y, _) = run(&mut sh, &input, &s, block);
            match &reference {
                None => reference = Some(y),
                Some(r) => assert_eq!(&y, r, "block {block}"),
            }
        }
    }

    #[test]
    fn lookahead_is_a_pure_delay_when_neutral() {
        let s = ShapeSettings {
            lookahead_ms: 2.0,
            ..ShapeSettings::default()
        };
        let input = noise(4800, 3);
        let mut sh = Shaper::new(FS, &s);
        let (y, _) = run(&mut sh, &input, &s, 128);
        let lat = s.latency(FS);
        assert_eq!(lat, 96);
        for i in lat..input.len() {
            assert_eq!(y[i], input[i - lat]);
        }
    }

    /// The largest jump between neighbouring samples, against the signal's own.
    fn steps(y: &[f32]) -> f32 {
        y.windows(2)
            .map(|w| (w[1] - w[0]).abs())
            .fold(0.0, f32::max)
    }

    fn tone(n: usize, f: f32) -> Vec<f32> {
        (0..n)
            .map(|i| (std::f32::consts::TAU * f * i as f32 / FS).sin() * 0.5)
            .collect()
    }

    /// With lookahead, a band made or merged away keeps the level: the new band starts with an
    /// empty delay (the band it split from still holds what it delayed), and a merged band's
    /// delayed signal joins the band that takes its region. Copying the parent's delay used to
    /// play its region twice for the lookahead's length, and dropping a merged band's lost it.
    #[test]
    fn with_lookahead_a_split_or_merge_keeps_the_level() {
        let mut s = ShapeSettings {
            lookahead_ms: 5.0,
            ..ShapeSettings::default()
        };
        let input = tone(48_000, 5000.0);
        let mut sh = Shaper::new(FS, &s);
        let mut y = input.clone();
        let mut d = vec![0.0; input.len()];
        for (k, (a, b)) in y.chunks_mut(480).zip(d.chunks_mut(480)).enumerate() {
            s.crossovers[1] = (20..60).contains(&k).then_some(200.0);
            sh.process(&mut [a], &mut [b], &s);
        }
        // Each 1 ms window's peak (five cycles) from well after the start.
        for (w, chunk) in y[4800..].chunks(48).enumerate() {
            let peak = chunk.iter().fold(0.0f32, |m, v| m.max(v.abs()));
            assert!((0.47..0.53).contains(&peak), "window {w}: {peak}");
        }
    }

    #[test]
    fn making_and_removing_a_band_does_not_click() {
        let mut s = ShapeSettings::default();
        s.bands[0].attack_db = 4.0;
        s.bands[0].sustain_db = -6.0;
        let input = tone(48_000, 220.0);
        let natural = steps(&input);
        let mut sh = Shaper::new(FS, &s);
        let mut y = input.clone();
        let mut d = vec![0.0; input.len()];
        for (k, (a, b)) in y.chunks_mut(480).zip(d.chunks_mut(480)).enumerate() {
            // Split at 200 Hz (right on the tone, the hardest place), then merge again.
            s.crossovers[1] = (20..60).contains(&k).then_some(200.0);
            if k == 20 {
                s.bands[2] = s.bands[0];
            }
            sh.process(&mut [a], &mut [b], &s);
        }
        // From after the tone's own onset (its first hit) on.
        let worst = (4800..y.len() - 1)
            .max_by(|&a, &b| (y[a + 1] - y[a]).abs().total_cmp(&(y[b + 1] - y[b]).abs()))
            .unwrap_or(0);
        assert!(
            steps(&y[4800..]) < natural * 1.3,
            "{} against {natural} at {worst}",
            steps(&y[4800..])
        );
    }

    #[test]
    fn a_stereo_mode_switch_does_not_click() {
        let mut s = split3();
        s.bands[1].sustain_db = -6.0;
        let (l, r) = (tone(24_000, 330.0), tone(24_000, 523.0));
        let natural = steps(&l).max(steps(&r));
        let mut sh = Shaper::new(FS, &s);
        let (mut yl, mut yr) = (l.clone(), r.clone());
        let (mut dl, mut dr) = (vec![0.0; l.len()], vec![0.0; l.len()]);
        for (k, (((a, b), c), d)) in yl
            .chunks_mut(256)
            .zip(yr.chunks_mut(256))
            .zip(dl.chunks_mut(256))
            .zip(dr.chunks_mut(256))
            .enumerate()
        {
            s.mid_side = k >= 40;
            sh.process(&mut [a, b], &mut [c, d], &s);
        }
        assert!(steps(&yl).max(steps(&yr)) < natural * 1.3);
    }

    #[test]
    fn solo_and_mute_choose_the_bands_heard() {
        let mut s = split3();
        s.bands[2].mute = true;
        let layout = Layout::new(&s.crossovers);
        assert_eq!(audible(&s, &layout), [true, true, false, true, true]);
        s.bands[1].solo = true;
        assert_eq!(audible(&s, &layout), [false, true, false, false, false]);
        // A soloed band alone sounds like its region: a 60 Hz tone in band 1 is gone.
        let input = tone(24_000, 60.0);
        let mut sh = Shaper::new(FS, &s);
        let (y, _) = run(&mut sh, &input, &s, 256);
        let tail = &y[12_000..];
        let peak = tail.iter().fold(0.0f32, |m, v| m.max(v.abs()));
        assert!(peak < 0.02, "{peak}");
    }

    #[test]
    fn the_floor_keeps_quiet_tails_down() {
        let mut s = ShapeSettings::default();
        s.bands[0].sustain_db = 12.0;
        s.floor_db = -30.0;
        // A tone dropping 30 dB to −50 dBFS: its tail is under the floor, so sustain lifts nothing.
        let input: Vec<f32> = tone(24_000, 440.0)
            .iter()
            .enumerate()
            .map(|(i, v)| v * if i < 6000 { 0.19 } else { 0.006 })
            .collect();
        let mut sh = Shaper::new(FS, &s);
        let (y, _) = run(&mut sh, &input, &s, 256);
        let ratio = steps(&y[12_000..]) / steps(&input[12_000..]);
        assert!((ratio - 1.0).abs() < 0.02, "{ratio}");
    }

    #[test]
    fn steady_noise_never_retriggers_and_a_hit_over_it_does() {
        // White noise at −20 dBFS for a second, then 9 dB louder: one hit at the start, one at
        // the step, nothing in between (it once retriggered every few ms in noise).
        let mut det = ShapeDetector::new(f64::from(FS), DEFAULT_TRANSIENT_MS, DEFAULT_TRANSITION);
        let raw = noise(96_000, 9);
        let mut starts = Vec::new();
        for (i, v) in raw.iter().enumerate() {
            let g = if i < 48_000 { 0.2 } else { 0.56 };
            det.process(f64::from(v.abs() * g));
            if det.hit_age() == Some(1) {
                starts.push(i);
            }
        }
        assert_eq!(starts.len(), 2, "{starts:?}");
        assert!(
            starts[0] < 100 && (48_000..48_200).contains(&starts[1]),
            "{starts:?}"
        );
    }

    #[test]
    fn the_scope_runs_the_real_detector() {
        let band = BandShape {
            attack_db: 6.0,
            sustain_db: -4.0,
            ..BandShape::default()
        };
        let pts = simulate_hit(&band, 0.0, 4000.0, 10.0, 400.0);
        let at = |ms: f64| {
            pts.iter()
                .min_by(|a, b| (a.t_ms - ms).abs().total_cmp(&(b.t_ms - ms).abs()))
                .map_or(0.0, |p| p.gain_db)
        };
        assert!(at(-5.0).abs() < 1e-9);
        assert!(at(5.0) > 4.0, "{}", at(5.0));
        assert!((at(300.0) + 4.0).abs() < 0.2, "{}", at(300.0));
        // Lookahead: the gain leads the hit.
        let ahead = simulate_hit(&band, 2.0, 4000.0, 10.0, 400.0);
        let lead = |p: &[HitPoint]| p.iter().find(|q| q.gain_db > 1.0).map_or(0.0, |q| q.t_ms);
        assert!(lead(&ahead) < lead(&pts) - 1.5);
    }
}
