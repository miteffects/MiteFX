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
/// A note's new reading takes over in this long.
const SWITCH_MS: f32 = 3.0;
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
}

impl Default for DoublerSettings {
    fn default() -> Self {
        Self {
            voices: 1,
            voice: [VoiceSettings { pan: 1.0, level_db: 0.0 }, VoiceSettings { pan: -0.6, level_db: -3.0 }, VoiceSettings { pan: 0.6, level_db: -3.0 }],
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
        }
    }
}

/// Constant-power pan: the left and right gains (unity at either side, −3 dB at the centre).
pub fn pan_gains(pan: f32) -> (f32, f32) {
    let th = std::f32::consts::FRAC_PI_4 * (1.0 + pan.clamp(-1.0, 1.0));
    (th.cos(), th.sin())
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
    gain: Glide,
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
            gain: Glide::default(),
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
    voices: [Voice; MAX_VOICES],
    k_glide: f32,
    switch_len: u32,
    base: Glide,
    chorus: Glide,
    src_l: Glide,
    src_r: Glide,
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
            line: Delay::new((LINE_MS * 0.001 * fs) as usize + 8),
            voices: [Voice::new(0x1234_5677), Voice::new(0x8BAD_F00D), Voice::new(0x0DEF_ACED)],
            k_glide: 1.0 - (-1.0 / (GLIDE_MS * 0.001 * fs)).exp(),
            switch_len: ((SWITCH_MS * 0.001 * fs) as u32).max(1),
            base: Glide::default(),
            chorus: Glide::default(),
            src_l: Glide::default(),
            src_r: Glide::default(),
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
        self.base.v = s.delay_ms.clamp(MIN_DELAY_MS, MAX_DELAY_MS);
        self.chorus.v = s.chorus.clamp(0.0, 1.0);
        let (l, r) = pan_gains(s.source_pan);
        let g = crate::db_to_gain(s.source_db);
        self.src_l.v = l * g;
        self.src_r.v = r * g;
        for i in 0..MAX_VOICES {
            self.on[i].v = f32::from(u8::from(i < s.voices));
            let note = self.voices[i].pick(s);
            let v = &mut self.voices[i];
            v.note = note;
            let (l, r) = pan_gains(s.voice[i].pan);
            v.gl.v = l;
            v.gr.v = r;
            v.gain.v = crate::db_to_gain(s.voice[i].level_db + note.gain_db);
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
        let v = &mut self.voices[i];
        v.from = from;
        v.fade_left = self.switch_len;
        v.note = note;
        v.drift = 0.0;
    }

    /// Writes the source and the doubles of `input` (one or two channels, summed) into `out`
    /// (left and right). Never allocates.
    pub fn process(&mut self, input: [&[f32]; 2], out: [&mut [f32]; 2], stereo_in: bool, s: &DoublerSettings) -> DoublerTelemetry {
        let [ol, or] = out;
        let n = input[0].len().min(ol.len()).min(or.len()).min(if stereo_in { input[1].len() } else { usize::MAX });
        if !self.started {
            self.settle(s);
            self.started = true;
        }
        let fs = self.fs;
        let k = self.k_glide;
        let mut tel = DoublerTelemetry::default();
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
            let x = if stereo_in { 0.5 * (input[0][j] + input[1][j]) } else { input[0][j] };
            self.line.write(x);
            // The onset detector, on the input's top (a pick's attack).
            self.hp += a_hp * (x - self.hp);
            let top = (x - self.hp).abs();
            self.fast += (top - self.fast) * if top > self.fast { att_fast } else { rel_fast };
            self.slow += (self.fast - self.slow) * k_slow;
            self.since_onset = self.since_onset.saturating_add(1);
            let onset = s.follow_notes && self.fast > gate && self.fast > ONSET_RATIO * self.slow && self.since_onset > refractory;
            if onset {
                self.since_onset = 0;
                tel.onset = true;
            }
            let base = self.base.next(s.delay_ms.clamp(MIN_DELAY_MS, MAX_DELAY_MS), k);
            let chorus = self.chorus.next(s.chorus.clamp(0.0, 1.0), k);
            let sl = self.src_l.next(src_l, k);
            let sr = self.src_r.next(src_r, k);
            let (mut l, mut r) = (sl * x, sr * x);
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
                let mut y = self.line.read(d as f32);
                if v.fade_left > 0 {
                    let t = 1.0 - v.fade_left as f32 / self.switch_len as f32;
                    let old = self.line.read((v.from + wob).max(min_read) as f32);
                    let (ga, gb) = ((std::f32::consts::FRAC_PI_2 * t).cos(), (std::f32::consts::FRAC_PI_2 * t).sin());
                    y = old * ga + y * gb;
                    v.fade_left -= 1;
                }
                v.last_delay = d;
                // Tone: a tilt about 900 Hz (the low part down as the high part goes up).
                let tilt = v.tilt.next(s.tone_db + v.note.tilt_db, k);
                v.lp += self.a_tilt * (y - v.lp);
                let (lo, hi) = (crate::db_to_gain(-0.5 * tilt), crate::db_to_gain(0.5 * tilt));
                let y = lo * v.lp + hi * (y - v.lp);
                let g = v.gain.next(crate::db_to_gain(s.voice[i].level_db + v.note.gain_db), k) * on;
                let (pl, pr) = pan_gains(s.voice[i].pan);
                let (gl, gr) = (v.gl.next(pl, k), v.gr.next(pr, k));
                let y = y * g;
                v.peak = v.peak.max(y.abs());
                l += gl * y;
                r += gr * y;
            }
            ol[j] = l;
            or[j] = r;
        }
        for (i, v) in self.voices.iter().enumerate() {
            tel.delay_ms[i] = (v.last_delay / f64::from(fs) * 1000.0) as f32;
            tel.cents[i] = v.note.cents * if v.drift.abs() >= drift_max * 0.999 { 0.0 } else { 1.0 };
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
