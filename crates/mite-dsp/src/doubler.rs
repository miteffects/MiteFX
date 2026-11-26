//! Mite Double's engine (docs/design/double-brief.md §1): a second (third, fourth) take of a
//! guitar part from the first. A real double is the same part played again: every note lands a
//! little early or late, a few cents off, a little louder or softer, brighter or duller, each
//! note differently. So each voice reads the input from a delay line with a **per-note** timing,
//! detune, level and tone, chosen afresh at every note the onset detector hears. The switch to a
//! note's new reading happens in the gap before that note reaches the voice (both heads still read
//! the previous note's tail, crossfaded over 3 ms): the attack arrives whole, at its new time.
//! Within a note the detune is a slow change of the delay (reading faster or slower: the pitch
//! a few cents off), easing off before it runs too far; CHORUS adds smoothed random motion on top.
//!
//! The source and each voice are placed in the stereo field (constant power) at their own level.
//! The input is summed to mono (a guitar part). Deterministic: the same input gives the same
//! output, for any block size.

use crate::reverb::{Delay, Rng, Wander};

pub const MAX_VOICES: usize = 3;
/// The shortest a note's reading may be (the onset has to be heard before it reaches a voice).
pub const MIN_DELAY_MS: f32 = 8.0;
pub const MAX_DELAY_MS: f32 = 60.0;
pub const MAX_TIMING_MS: f32 = 40.0;
pub const MAX_PITCH_CENTS: f32 = 25.0;
/// The chorus's reach at 100 %.
const CHORUS_MS: f32 = 3.0;
/// How far a note's detune may move its reading before easing off.
const DRIFT_MS: f32 = 6.0;
/// The line: the longest reading and the reach of the chorus.
const LINE_MS: f32 = MAX_DELAY_MS + 0.5 * MAX_TIMING_MS + CHORUS_MS + 10.0;
/// A note's new reading takes over in at least this long, and as long as there is room before
/// the note arrives, up to `SWITCH_MAX_MS` (an equal-gain fade: the two readings are the same
/// signal a little apart, so an equal-power fade would bump the level).
const SWITCH_MS: f32 = 3.0;
const SWITCH_MAX_MS: f32 = 8.0;
/// A delay change larger than this, ms, crossfades instead of gliding (a turned knob moves less
/// per host block and glides).
const JUMP_MS: f32 = 2.0;
/// The onset is heard this late at most; a switch must be done before the note reaches a reading.
const ONSET_LAG_MS: f32 = 2.5;
/// The reading's interpolator: a windowed sinc (Kaiser, β = 8, cut at 0.92 of Nyquist), this many
/// taps, tabulated at `SINC_PHASES` fractions. As flat at every fraction as it is anywhere, so a
/// sliding reading (the detune, the chorus) adds no roughness (cubic Lagrange's treble rose and
/// fell with the fraction: at 10 kHz the slide's junk was only 34 dB under the tone).
const SINC_TAPS: usize = 32;
const SINC_PHASES: usize = 512;
/// Without notes (FOLLOW NOTES off), new readings come every this long (random within).
const FREE_SECONDS: (f32, f32) = (1.5, 3.0);
/// Settings glide over this.
const GLIDE_MS: f32 = 20.0;
/// The onset detector: a fast and a slow envelope of the input's top (a pick's attack), a note
/// when the fast one rises past `ONSET_RATIO` × the slow one, over `ONSET_GATE_DB`, at most one
/// per `REFRACTORY_MS`.
const ONSET_RATIO: f32 = 2.0;
const ONSET_GATE_DB: f32 = -50.0;
const REFRACTORY_MS: f32 = 60.0;
/// The tone's tilt pivots here.
const TILT_HZ: f32 = 900.0;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VoiceSettings {
    /// −1…1.
    pub pan: f32,
    pub level_db: f32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DoublerSettings {
    /// How many voices play (0…3).
    pub voices: usize,
    pub voice: [VoiceSettings; MAX_VOICES],
    pub source_pan: f32,
    pub source_db: f32,
    /// The doubles' mean lag, ms (MIN_DELAY_MS…MAX_DELAY_MS).
    pub delay_ms: f32,
    /// How far a note may land from that, ms (its full spread).
    pub timing_ms: f32,
    /// How far a note may be off pitch, cents (either way).
    pub pitch_cents: f32,
    /// How much the level and tone vary from note to note, 0…1.
    pub variation: f32,
    pub chorus: f32,
    pub chorus_rate: f32,
    /// The doubles' tone: a tilt about 900 Hz, dB (+: brighter).
    pub tone_db: f32,
    /// New readings at every note (on) or every few seconds (off).
    pub follow_notes: bool,
    /// Keep the output as loud as the input (see [`auto_gain_db`], [`doubles_trim_db`]).
    pub auto_gain: bool,
    /// Every double's lows (under `bass_hz`) taken from the source, in phase with it: the low end
    /// stays whole summed to mono (see [`MonoBass`]).
    pub mono_bass: bool,
    pub bass_hz: f32,
}

impl Default for DoublerSettings {
    fn default() -> Self {
        Self {
            voices: 1,
            voice: [
                VoiceSettings {
                    pan: 1.0,
                    level_db: 0.0,
                },
                VoiceSettings {
                    pan: -0.6,
                    level_db: -3.0,
                },
                VoiceSettings {
                    pan: 0.6,
                    level_db: -3.0,
                },
            ],
            source_pan: -1.0,
            source_db: 0.0,
            delay_ms: 16.0,
            timing_ms: 14.0,
            pitch_cents: 7.0,
            variation: 0.5,
            chorus: 0.15,
            chorus_rate: 0.6,
            tone_db: 0.0,
            follow_notes: true,
            auto_gain: true,
            mono_bass: true,
            bass_hz: 150.0,
        }
    }
}

/// Auto gain, first part (dB, ≤ 0, applied to every double): the doubles together as loud as the
/// loudest one alone, so a second and third take add width and weight, not level (user,
/// 2026-11-05: "auto gain doesnt work at least on takes 2 and 3": it turned everything down
/// instead, the source with it). Doubles add as powers (each is another take, not in phase).
pub fn doubles_trim_db(s: &DoublerSettings) -> f32 {
    if !s.auto_gain {
        return 0.0;
    }
    let p = |db: f32| crate::db_to_gain(2.0 * db);
    let active = &s.voice[..s.voices.min(MAX_VOICES)];
    let sum: f32 = active.iter().map(|v| p(v.level_db)).sum();
    let loudest = active.iter().map(|v| p(v.level_db)).fold(0.0, f32::max);
    if sum <= 0.0 {
        0.0
    } else {
        (10.0 * (loudest / sum).log10()).min(0.0)
    }
}

/// Auto gain, second part (dB, on the whole output): as loud as the input played on both sides
/// (bypass). The source and the (trimmed) doubles add as powers, each placed at constant power:
/// the output carries the input's power × (source² + loudest double²) against 2 × for the input on
/// both sides. Held to −12…+6 dB.
pub fn auto_gain_db(s: &DoublerSettings) -> f32 {
    if !s.auto_gain {
        return 0.0;
    }
    let p = |db: f32| crate::db_to_gain(2.0 * db);
    let loudest = s.voice[..s.voices.min(MAX_VOICES)]
        .iter()
        .map(|v| p(v.level_db))
        .fold(0.0, f32::max);
    let total = p(s.source_db) + loudest;
    (10.0 * (2.0 / total.max(1e-9)).log10()).clamp(-12.0, 6.0)
}

/// A second-order Butterworth section (transposed direct form II).
#[derive(Debug, Clone, Copy, Default)]
struct Biquad {
    b: [f64; 3],
    a: [f64; 2],
    z: [f64; 2],
}

impl Biquad {
    fn set(&mut self, fc: f64, fs: f64, high: bool) {
        let w = std::f64::consts::TAU * fc / fs;
        let (sin, cos) = w.sin_cos();
        let alpha = sin / std::f64::consts::SQRT_2;
        let a0 = 1.0 + alpha;
        let (b0, b1) = if high {
            ((1.0 + cos) / 2.0, -(1.0 + cos))
        } else {
            ((1.0 - cos) / 2.0, 1.0 - cos)
        };
        self.b = [b0 / a0, b1 / a0, b0 / a0];
        self.a = [-2.0 * cos / a0, (1.0 - alpha) / a0];
    }

    #[inline]
    fn process(&mut self, x: f64) -> f64 {
        let y = self.b[0] * x + self.z[0];
        self.z[0] = self.b[1] * x - self.a[0] * y + self.z[1];
        self.z[1] = self.b[2] * x - self.a[1] * y;
        y
    }
}

/// A Linkwitz–Riley crossover (two Butterworth sections each side): its low and high parts sum
/// to an allpass, so the source, passed through the same split, stays in phase with the lows it
/// lends the doubles.
#[derive(Debug, Clone, Copy, Default)]
struct Lr4 {
    low: [Biquad; 2],
    high: [Biquad; 2],
}

impl Lr4 {
    fn set(&mut self, fc: f64, fs: f64) {
        for q in &mut self.low {
            q.set(fc, fs, false);
        }
        for q in &mut self.high {
            q.set(fc, fs, true);
        }
    }

    #[inline]
    fn low(&mut self, x: f64) -> f64 {
        let y = self.low[0].process(x);
        self.low[1].process(y)
    }

    #[inline]
    fn high(&mut self, x: f64) -> f64 {
        let y = self.high[0].process(x);
        self.high[1].process(y)
    }
}

/// Mono bass: under the split the output is the source's lows alone, on both sides as the input
/// itself (at the source's level, wherever it sits); the source and every double keep only their
/// highs. So the low end is the input's whatever the doubles do: whole summed to mono, no pile-up
/// as takes are added (a first design lent each double the source's lows: three doubles stacked
/// them +1.2 dB), and the side the source sits on rebuilds it exactly (lows + highs: an allpass).
struct MonoBass {
    source: Lr4,
    voices: [Lr4; MAX_VOICES],
    fc: f64,
    /// How much of the split is in (0…1, gliding: switching it never clicks).
    mix: Glide,
    count: u32,
}

/// The doubles' highpass sits this far under the split: the source's lows and a double's highs
/// are different signals, so they add as powers, and two Linkwitz–Riley halves at one frequency
/// leave a 3 dB hole there (−0.6 dB on noise); a little overlap fills it (the hole −0.45 dB at
/// worst), the double's leak under the split ≥ 14 dB down an octave below.
const DOUBLE_HP: f64 = 0.7;

impl MonoBass {
    fn set(&mut self, fs: f32) {
        let f = self.fc;
        self.source.set(f, f64::from(fs));
        for v in &mut self.voices {
            v.set(f * DOUBLE_HP, f64::from(fs));
        }
    }
}

/// Constant-power pan: the left and right gains (unity at either side, −3 dB at the centre).
pub fn pan_gains(pan: f32) -> (f32, f32) {
    let th = std::f32::consts::FRAC_PI_4 * (1.0 + pan.clamp(-1.0, 1.0));
    (th.cos(), th.sin())
}

/// A value gliding toward its target through two one-poles, so the glide starts and ends at
/// zero speed (one pole sets off at full speed: Source Level −48 → +6 dB clicked +8 dB).
#[derive(Debug, Clone, Copy, Default)]
struct Glide2 {
    a: f32,
    v: f32,
}

impl Glide2 {
    fn at(v: f32) -> Self {
        Self { a: v, v }
    }

    #[inline]
    fn next(&mut self, target: f32, k: f32) -> f32 {
        self.a += (target - self.a) * k;
        self.v += (self.a - self.v) * k;
        self.v
    }
}

/// A value gliding toward its target, one pole per sample.
#[derive(Debug, Clone, Copy, Default)]
struct Glide {
    v: f32,
}

impl Glide {
    #[inline]
    fn next(&mut self, target: f32, k: f32) -> f32 {
        self.v += (target - self.v) * k;
        self.v
    }
}

/// What a voice plays this note.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Note {
    /// Its lag from the mean, ms.
    pub offset_ms: f32,
    pub cents: f32,
    pub gain_db: f32,
    pub tilt_db: f32,
}

struct Voice {
    rng: Rng,
    wander: Wander,
    note: Note,
    /// The reading's drift from the note's lag (the detune at work), samples.
    drift: f64,
    /// A switch under way: the old reading (samples, held) and samples left.
    from: f64,
    fade_left: u32,
    fade_len: u32,
    gain: Glide2,
    tilt: Glide,
    lp: f32,
    gl: Glide,
    gr: Glide,
    /// Samples to the next new reading without notes.
    free_left: u32,
    /// The total reading last used, samples (telemetry).
    last_delay: f64,
    peak: f32,
}

impl Voice {
    fn new(seed: u32) -> Self {
        Self {
            rng: Rng(seed),
            wander: Wander::new(seed.wrapping_mul(0x9E37_79B9) | 1),
            note: Note::default(),
            drift: 0.0,
            from: 0.0,
            fade_left: 0,
            fade_len: 1,
            gain: Glide2::default(),
            tilt: Glide::default(),
            lp: 0.0,
            gl: Glide::default(),
            gr: Glide::default(),
            free_left: 0,
            last_delay: 0.0,
            peak: 0.0,
        }
    }

    /// A value spread like a player's: −1…1, most near the middle (the mean of two uniforms).
    fn spread(&mut self) -> f32 {
        self.rng.next() + self.rng.next() - 1.0
    }

    fn pick(&mut self, s: &DoublerSettings) -> Note {
        Note {
            offset_ms: 0.5 * s.timing_ms.clamp(0.0, MAX_TIMING_MS) * self.spread(),
            cents: s.pitch_cents.clamp(0.0, MAX_PITCH_CENTS) * self.spread(),
            gain_db: 2.0 * s.variation.clamp(0.0, 1.0) * self.spread(),
            tilt_db: 2.0 * s.variation.clamp(0.0, 1.0) * self.spread(),
        }
    }
}

/// The windowed-sinc reading (see `SINC_TAPS`): coefficients per fraction, each set summing to 1.
pub(crate) struct Sinc {
    table: Vec<[f32; SINC_TAPS]>,
}

impl Sinc {
    pub(crate) fn new() -> Self {
        let half = (SINC_TAPS / 2) as f64;
        let i0 = |x: f64| {
            // The zeroth-order modified Bessel function, by its series.
            let (mut sum, mut term) = (1.0, 1.0);
            for k in 1..32 {
                term *= (x / 2.0) / k as f64;
                sum += term * term;
            }
            sum
        };
        let beta = 8.0;
        let cut = 0.92;
        let table = (0..=SINC_PHASES)
            .map(|p| {
                let f = p as f64 / SINC_PHASES as f64;
                let mut c = [0.0f64; SINC_TAPS];
                for (n, v) in c.iter_mut().enumerate() {
                    // Tap n reads the sample (n − half + 1) after the whole delay.
                    let t = n as f64 - (half - 1.0) - f;
                    let x = std::f64::consts::PI * cut * t;
                    let sinc = if x.abs() < 1e-12 { 1.0 } else { x.sin() / x };
                    let r = t / half;
                    let w = if r.abs() <= 1.0 {
                        i0(beta * (1.0 - r * r).sqrt()) / i0(beta)
                    } else {
                        0.0
                    };
                    *v = sinc * w;
                }
                let sum: f64 = c.iter().sum();
                c.map(|v| (v / sum) as f32)
            })
            .collect();
        Self { table }
    }

    /// The signal `d` samples ago on `line` (d ≥ SINC_TAPS / 2).
    #[inline]
    pub(crate) fn read(&self, line: &Delay, d: f64) -> f32 {
        let whole = d.floor();
        let pos = (d - whole) * SINC_PHASES as f64;
        let p = (pos as usize).min(SINC_PHASES - 1);
        let a = (pos - p as f64) as f32;
        let (c0, c1) = (&self.table[p], &self.table[p + 1]);
        let k = whole as usize;
        let mut y = 0.0;
        for n in 0..SINC_TAPS {
            let c = c0[n] + (c1[n] - c0[n]) * a;
            y += c * line.tap(k + n + 1 - SINC_TAPS / 2);
        }
        y
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct DoublerTelemetry {
    /// Each voice's reading at the end of the block, ms, and its detune now, cents.
    pub delay_ms: [f32; MAX_VOICES],
    pub cents: [f32; MAX_VOICES],
    /// Each voice's peak this block.
    pub peak: [f32; MAX_VOICES],
    /// Whether a note began in this block.
    pub onset: bool,
}

pub struct Doubler {
    fs: f32,
    line: Delay,
    sinc: Sinc,
    bass: MonoBass,
    out_gain: Glide2,
    /// The source's level alone (its lows, under mono bass).
    src_g: Glide2,
    voices: [Voice; MAX_VOICES],
    k_glide: f32,
    switch_len: u32,
    base: Glide,
    chorus: Glide,
    src_l: Glide2,
    src_r: Glide2,
    on: [Glide; MAX_VOICES],
    a_tilt: f32,
    hp: f32,
    fast: f32,
    slow: f32,
    since_onset: u32,
    started: bool,
}

impl Doubler {
    pub fn new(fs: f32) -> Self {
        let mut d = Self {
            fs,
            line: Delay::new((LINE_MS * 0.001 * fs) as usize + SINC_TAPS + 8),
            sinc: Sinc::new(),
            bass: MonoBass {
                source: Lr4::default(),
                voices: [Lr4::default(); MAX_VOICES],
                fc: 0.0,
                mix: Glide::default(),
                count: 0,
            },
            out_gain: Glide2::at(1.0),
            src_g: Glide2::at(1.0),
            voices: [
                Voice::new(0x1234_5677),
                Voice::new(0x8BAD_F00D),
                Voice::new(0x0DEF_ACED),
            ],
            k_glide: 1.0 - (-1.0 / (GLIDE_MS * 0.001 * fs)).exp(),
            switch_len: ((SWITCH_MS * 0.001 * fs) as u32).max(1),
            base: Glide::default(),
            chorus: Glide::default(),
            src_l: Glide2::default(),
            src_r: Glide2::default(),
            on: [Glide::default(); MAX_VOICES],
            a_tilt: 1.0 - (-std::f32::consts::TAU * TILT_HZ / fs).exp(),
            hp: 0.0,
            fast: 0.0,
            slow: 0.0,
            since_onset: u32::MAX,
            started: false,
        };
        d.reset();
        d
    }

    pub fn sample_rate(&self) -> f32 {
        self.fs
    }

    pub fn reset(&mut self) {
        self.line.reset();
        for (i, v) in self.voices.iter_mut().enumerate() {
            *v = Voice::new([0x1234_5677, 0x8BAD_F00D, 0x0DEF_ACED][i]);
        }
        self.hp = 0.0;
        self.bass.source = Lr4::default();
        self.bass.voices = [Lr4::default(); MAX_VOICES];
        self.bass.fc = 0.0;
        self.bass.count = 0;
        self.fast = 0.0;
        self.slow = 0.0;
        self.since_onset = u32::MAX;
        self.started = false;
    }

    fn ms(&self, ms: f32) -> f64 {
        f64::from(ms * 0.001 * self.fs)
    }

    /// Each voice's note now (tests and the editor).
    pub fn notes(&self) -> [Note; MAX_VOICES] {
        std::array::from_fn(|i| self.voices[i].note)
    }

    /// Jumps every glide to `s` (the first block, so nothing fades in from nothing).
    fn settle(&mut self, s: &DoublerSettings) {
        self.out_gain = Glide2::at(crate::db_to_gain(auto_gain_db(s)));
        self.src_g = Glide2::at(crate::db_to_gain(s.source_db));
        self.bass.mix.v = f32::from(u8::from(s.mono_bass));
        self.base.v = s.delay_ms.clamp(MIN_DELAY_MS, MAX_DELAY_MS);
        self.chorus.v = s.chorus.clamp(0.0, 1.0);
        let (l, r) = pan_gains(s.source_pan);
        let g = crate::db_to_gain(s.source_db);
        self.src_l = Glide2::at(l * g);
        self.src_r = Glide2::at(r * g);
        for i in 0..MAX_VOICES {
            self.on[i].v = f32::from(u8::from(i < s.voices));
            let note = self.voices[i].pick(s);
            let v = &mut self.voices[i];
            v.note = note;
            let (l, r) = pan_gains(s.voice[i].pan);
            v.gl.v = l;
            v.gr.v = r;
            v.gain = Glide2::at(crate::db_to_gain(s.voice[i].level_db + note.gain_db + doubles_trim_db(s)));
            v.tilt.v = s.tone_db + note.tilt_db;
            v.free_left = 0;
        }
    }

    /// The note's reading (lag) for voice `i`, samples, at the mean `base_ms`, never shorter than
    /// MIN_DELAY_MS.
    fn note_delay(&self, base_ms: f32, note: &Note) -> f64 {
        self.ms((base_ms + note.offset_ms).max(MIN_DELAY_MS))
    }

    /// A new note for every voice (a heard onset, or the free clock): its reading takes over now.
    fn switch(&mut self, i: usize, s: &DoublerSettings) {
        let base = self.base.v;
        let old = self.voices[i].note;
        let from = self.note_delay(base, &old) + self.voices[i].drift;
        let note = self.voices[i].pick(s);
        // As long as there is room before the note reaches either reading.
        let room = from.min(self.note_delay(base, &note)) - self.ms(ONSET_LAG_MS);
        let len = (room as u32).clamp(
            self.switch_len,
            (self.ms(SWITCH_MAX_MS) as u32).max(self.switch_len),
        );
        let v = &mut self.voices[i];
        v.from = from;
        v.fade_left = len;
        v.fade_len = len;
        v.note = note;
        v.drift = 0.0;
    }

    /// Writes the source and the doubles of `input` (one or two channels, summed) into `out`
    /// (left and right). Never allocates.
    pub fn process(
        &mut self,
        input: [&[f32]; 2],
        out: [&mut [f32]; 2],
        stereo_in: bool,
        s: &DoublerSettings,
    ) -> DoublerTelemetry {
        let [ol, or] = out;
        let n = input[0]
            .len()
            .min(ol.len())
            .min(or.len())
            .min(if stereo_in {
                input[1].len()
            } else {
                usize::MAX
            });
        if !self.started {
            self.settle(s);
            self.started = true;
        }
        let fs = self.fs;
        let k = self.k_glide;
        // Two poles in series, each at half the time.
        let k2 = 1.0 - (1.0 - k) * (1.0 - k);
        let mut tel = DoublerTelemetry::default();
        // Mono bass: the split's frequency glides on a log scale, redesigned every 32 samples.
        let fc = f64::from(s.bass_hz.clamp(40.0, 400.0));
        if self.bass.fc == 0.0 {
            self.bass.fc = fc;
            self.bass.set(fs);
        }
        let bass_target = f32::from(u8::from(s.mono_bass));
        let trim = doubles_trim_db(s);
        let out_target = crate::db_to_gain(auto_gain_db(s));
        let src_level = crate::db_to_gain(s.source_db);
        let (src_l, src_r) = {
            let (l, r) = pan_gains(s.source_pan);
            let g = crate::db_to_gain(s.source_db);
            (l * g, r * g)
        };
        let att_fast = 1.0 - (-1.0 / (0.0005 * fs)).exp();
        let rel_fast = 1.0 - (-1.0 / (0.010 * fs)).exp();
        let k_slow = 1.0 - (-1.0 / (0.040 * fs)).exp();
        let gate = crate::db_to_gain(ONSET_GATE_DB);
        let refractory = (REFRACTORY_MS * 0.001 * fs) as u32;
        let a_hp = 1.0 - (-std::f32::consts::TAU * 150.0 / fs).exp();
        let chorus_ms = CHORUS_MS;
        let rate = s.chorus_rate.clamp(0.01, 10.0) / fs;
        for v in &mut self.voices {
            v.wander.step = rate;
            v.peak = 0.0;
        }
        let drift_max = self.ms(DRIFT_MS);
        let min_read = self.ms(2.0);
        let chorus_reach = self.ms(chorus_ms);
        for j in 0..n {
            let x = if stereo_in {
                0.5 * (input[0][j] + input[1][j])
            } else {
                input[0][j]
            };
            self.line.write(x);
            // The onset detector, on the input's top (a pick's attack).
            self.hp += a_hp * (x - self.hp);
            let top = (x - self.hp).abs();
            self.fast += (top - self.fast) * if top > self.fast { att_fast } else { rel_fast };
            self.slow += (self.fast - self.slow) * k_slow;
            self.since_onset = self.since_onset.saturating_add(1);
            let onset = s.follow_notes
                && self.fast > gate
                && self.fast > ONSET_RATIO * self.slow
                && self.since_onset > refractory;
            if onset {
                self.since_onset = 0;
                tel.onset = true;
            }
            // A delay that jumps (automation, a preset): every double crossfades to its reading at
            // the new delay, once no double is mid-fade (until then the delay holds). Glided, a
            // jump read the line up to 2.6× too fast, backwards (Delay 8 → 60 ms clicked +26 dB).
            let target = s.delay_ms.clamp(MIN_DELAY_MS, MAX_DELAY_MS);
            let base = if (target - self.base.v).abs() > JUMP_MS {
                if self.voices.iter().all(|v| v.fade_left == 0) {
                    let len = (self.ms(SWITCH_MAX_MS) as u32).max(self.switch_len);
                    let old = self.base.v;
                    for i in 0..MAX_VOICES {
                        let from = self.note_delay(old, &self.voices[i].note) + self.voices[i].drift;
                        let v = &mut self.voices[i];
                        v.from = from;
                        v.fade_left = len;
                        v.fade_len = len;
                    }
                    self.base.v = target;
                }
                self.base.v
            } else {
                self.base.next(target, k)
            };
            let chorus = self.chorus.next(s.chorus.clamp(0.0, 1.0), k);
            let sl = self.src_l.next(src_l, k2);
            let sr = self.src_r.next(src_r, k2);
            self.bass.count = (self.bass.count + 1) % 32;
            if self.bass.count == 0 && self.bass.fc != fc {
                let ratio = fc / self.bass.fc;
                self.bass.fc = if (ratio - 1.0).abs() < 1e-3 {
                    fc
                } else {
                    self.bass.fc * ratio.powf(0.08)
                };
                self.bass.set(fs);
            }
            let bass = self.bass.mix.next(bass_target, k);
            let (low, src) = if bass > 0.0 {
                let lo = self.bass.source.low(f64::from(x)) as f32;
                let hi = self.bass.source.high(f64::from(x)) as f32;
                (
                    lo * bass * self.src_g.next(src_level, k2),
                    x + (hi - x) * bass,
                )
            } else {
                (0.0, x)
            };
            let (mut l, mut r) = (sl * src + low, sr * src + low);
            for i in 0..MAX_VOICES {
                let on = self.on[i].next(f32::from(u8::from(i < s.voices)), k);
                if on < 1e-6 && i >= s.voices {
                    continue;
                }
                let free = !s.follow_notes && {
                    let v = &mut self.voices[i];
                    if v.free_left == 0 {
                        let (lo, hi) = FREE_SECONDS;
                        v.free_left = ((lo + (hi - lo) * v.rng.next()) * fs) as u32;
                        true
                    } else {
                        v.free_left -= 1;
                        false
                    }
                };
                if onset || free {
                    self.switch(i, s);
                }
                let note_delay = self.note_delay(base, &self.voices[i].note);
                let v = &mut self.voices[i];
                // The detune at work: reading faster (sharp) or slower (flat), easing off as the
                // drift nears its reach (and the reading its shortest).
                let reach = drift_max.min(note_delay - min_read - chorus_reach).max(0.0);
                let rate = -(2f64.powf(f64::from(v.note.cents) / 1200.0) - 1.0);
                if reach > 0.0 {
                    let k = (v.drift / reach).clamp(-1.0, 1.0);
                    v.drift += rate * (1.0 - k * k);
                }
                let wob = f64::from(chorus * chorus_ms * 0.001 * fs * v.wander.next());
                let d = (note_delay + v.drift + wob).max(min_read);
                let mut y = self.sinc.read(&self.line, d);
                if v.fade_left > 0 {
                    // An S curve: a straight fade kinks where it starts and ends (a click as the
                    // readings jump to a delay 52 ms away).
                    let t = 1.0 - v.fade_left as f32 / v.fade_len as f32;
                    let t = t * t * (3.0 - 2.0 * t);
                    let old = self.sinc.read(&self.line, (v.from + wob).max(min_read));
                    y = old + (y - old) * t;
                    v.fade_left -= 1;
                }
                v.last_delay = d;
                // Tone: a tilt about 900 Hz (the low part down as the high part goes up).
                let tilt = v.tilt.next(s.tone_db + v.note.tilt_db, k);
                v.lp += self.a_tilt * (y - v.lp);
                let (lo, hi) = (
                    crate::db_to_gain(-0.5 * tilt),
                    crate::db_to_gain(0.5 * tilt),
                );
                let y = lo * v.lp + hi * (y - v.lp);
                // Mono bass: the double's highs only.
                let y = if bass > 0.0 {
                    y + (self.bass.voices[i].high(f64::from(y)) as f32 - y) * bass
                } else {
                    y
                };
                let g = v.gain.next(
                    crate::db_to_gain(s.voice[i].level_db + v.note.gain_db + trim),
                    k2,
                ) * on;
                let (pl, pr) = pan_gains(s.voice[i].pan);
                let (gl, gr) = (v.gl.next(pl, k), v.gr.next(pr, k));
                let y = y * g;
                v.peak = v.peak.max(y.abs());
                l += gl * y;
                r += gr * y;
            }
            let g = self.out_gain.next(out_target, k2);
            ol[j] = l * g;
            or[j] = r * g;
        }
        for (i, v) in self.voices.iter().enumerate() {
            tel.delay_ms[i] = (v.last_delay / f64::from(fs) * 1000.0) as f32;
            tel.cents[i] = v.note.cents
                * if v.drift.abs() >= drift_max * 0.999 {
                    0.0
                } else {
                    1.0
                };
            tel.peak[i] = v.peak;
        }
        tel
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pan_is_constant_power_and_whole_at_the_sides() {
        assert_eq!(pan_gains(-1.0), (1.0, 0.0));
        let (l, r) = pan_gains(1.0);
        assert!(l.abs() < 1e-6 && (r - 1.0).abs() < 1e-6);
        let (l, r) = pan_gains(0.3);
        assert!((l * l + r * r - 1.0).abs() < 1e-5);
    }
}
