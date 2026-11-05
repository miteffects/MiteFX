//! The modulation matrix's engine, shared by Mite Delay, Mite Sat and Mite Mod: two LFOs, an
//! envelope follower and eight slots, each moving one target by a share of its knob's travel
//! (the plugin applies the offsets: `mite_core::modmatrix::modulate`).
//!
//! It runs in control steps of [`CONTROL`] samples counted from the start, whatever the host's
//! blocks (so the output is the same for every block size): a plugin processes each step with
//! the settings its offsets give ([`Matrix::steps`]). The sources are smoothed over
//! [`SMOOTH_MS`] (a square or a sample-and-hold steps, but what it moves never clicks), and the
//! targets glide as each plugin glides a knob. Nothing allocates.

use crate::modulation::Transport;

pub const LFOS: usize = 2;
pub const SLOTS: usize = 8;
/// Samples per control step.
pub const CONTROL: usize = 32;
/// The sources' smoothing, ms.
pub const SMOOTH_MS: f32 = 2.0;
/// The envelope reads its level over this range (dBFS, after its gain): −60 dB is 0, 0 dB is 1.
pub const ENV_FLOOR_DB: f32 = -60.0;
/// Each division's length, quarter notes (the `lfo{N}_div` order).
pub const DIV_QUARTERS: [f64; 16] = [0.125, 1.0 / 6.0, 0.25, 0.375, 1.0 / 3.0, 0.5, 0.75, 2.0 / 3.0, 1.0, 1.5, 4.0 / 3.0, 2.0, 3.0, 4.0, 8.0, 16.0];

/// An LFO's shape. The index is saved state (append only).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LfoShape {
    #[default]
    Sine,
    Triangle,
    SawUp,
    SawDown,
    Square,
    SmoothRandom,
    SampleHold,
}

impl LfoShape {
    pub const ALL: [LfoShape; 7] = [Self::Sine, Self::Triangle, Self::SawUp, Self::SawDown, Self::Square, Self::SmoothRandom, Self::SampleHold];

    pub fn from_index(i: usize) -> Self {
        Self::ALL.get(i).copied().unwrap_or_default()
    }

    pub fn index(self) -> usize {
        self as usize
    }
}

/// What a slot listens to. The index is saved state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Source {
    #[default]
    Off,
    Lfo1,
    Lfo2,
    Envelope,
}

impl Source {
    pub const ALL: [Source; 4] = [Self::Off, Self::Lfo1, Self::Lfo2, Self::Envelope];

    pub fn from_index(i: usize) -> Self {
        Self::ALL.get(i).copied().unwrap_or_default()
    }

    pub fn index(self) -> usize {
        self as usize
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LfoSettings {
    pub rate_hz: f32,
    pub sync: bool,
    /// Into [`DIV_QUARTERS`].
    pub division: usize,
    pub shape: LfoShape,
    /// Where in its cycle it starts, 0…1.
    pub phase: f32,
}

impl Default for LfoSettings {
    fn default() -> Self {
        Self { rate_hz: 1.0, sync: false, division: 8, shape: LfoShape::Sine, phase: 0.0 }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EnvSettings {
    pub attack_ms: f32,
    pub release_ms: f32,
    pub gain_db: f32,
    /// Follow the sidechain (where there is one) instead of the input.
    pub sidechain: bool,
}

impl Default for EnvSettings {
    fn default() -> Self {
        Self { attack_ms: 10.0, release_ms: 200.0, gain_db: 0.0, sidechain: false }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Slot {
    pub source: Source,
    /// Into the plugin's target table (0: none).
    pub target: usize,
    /// −1…1 of the target's travel.
    pub amount: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct MatrixSettings {
    pub lfos: [LfoSettings; LFOS],
    pub env: EnvSettings,
    pub slots: [Slot; SLOTS],
}

impl MatrixSettings {
    /// Whether any slot moves anything.
    pub fn active(&self) -> bool {
        self.slots.iter().any(|s| s.source != Source::Off && s.target != 0 && s.amount != 0.0)
    }
}

/// A random value per cycle, −1…1, from the cycle's number (the same on every run).
fn cycle_value(k: i64, seed: u64) -> f32 {
    let mut h = (k as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ seed.wrapping_mul(0xC2B2_AE3D_27D4_EB4F);
    h ^= h >> 29;
    h = h.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    h ^= h >> 32;
    (h >> 40) as f32 / (1u64 << 24) as f32 * 2.0 - 1.0
}

/// An LFO at total phase `p` (cycles: the fraction is the place in the cycle, the whole part
/// picks the random shapes' values), −1…1. `seed` tells the two LFOs' random values apart.
pub fn lfo_at(shape: LfoShape, p: f64, seed: u64) -> f32 {
    let f = p.rem_euclid(1.0) as f32;
    let k = p.floor() as i64;
    match shape {
        LfoShape::Sine => (std::f32::consts::TAU * f).sin(),
        LfoShape::Triangle => {
            if f < 0.25 {
                4.0 * f
            } else if f < 0.75 {
                2.0 - 4.0 * f
            } else {
                4.0 * f - 4.0
            }
        }
        LfoShape::SawUp => 2.0 * f - 1.0,
        LfoShape::SawDown => 1.0 - 2.0 * f,
        LfoShape::Square => {
            if f < 0.5 {
                1.0
            } else {
                -1.0
            }
        }
        LfoShape::SmoothRandom => {
            let (a, b) = (cycle_value(k, seed), cycle_value(k + 1, seed));
            a + (b - a) * f * f * (3.0 - 2.0 * f)
        }
        LfoShape::SampleHold => cycle_value(k, seed),
    }
}

/// An LFO's rate, Hz (synced to `tempo` when asked and known).
pub fn lfo_rate(s: &LfoSettings, tempo: Option<f64>) -> f64 {
    match (s.sync, tempo) {
        (true, Some(bpm)) if bpm > 0.0 => bpm / 60.0 / DIV_QUARTERS[s.division.min(DIV_QUARTERS.len() - 1)],
        _ => f64::from(s.rate_hz.clamp(0.01, 100.0)),
    }
}

/// The matrix's readings after a step, for the editor.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct MatrixReadings {
    /// LFO 1, LFO 2 (−1…1), the envelope (0…1), smoothed.
    pub sources: [f32; 3],
    /// Each LFO's total phase (cycles) and rate (Hz).
    pub phases: [f64; LFOS],
    pub rates: [f64; LFOS],
}

pub struct Matrix {
    fs: f32,
    phase: [f64; LFOS],
    env: f32,
    smooth: [f32; 3],
    smooth_k: f32,
    /// Samples into the current control step.
    pos: usize,
}

impl Matrix {
    pub fn new(fs: f32) -> Self {
        Self { fs, phase: [0.0; LFOS], env: 0.0, smooth: [0.0; 3], smooth_k: 1.0 - (-(CONTROL as f32) / (SMOOTH_MS * 0.001 * fs)).exp(), pos: 0 }
    }

    pub fn sample_rate(&self) -> f32 {
        self.fs
    }

    pub fn reset(&mut self) {
        self.phase = [0.0; LFOS];
        self.env = 0.0;
        self.smooth = [0.0; 3];
        self.pos = 0;
    }

    /// The sources now (before smoothing).
    fn raw(&self, s: &MatrixSettings) -> [f32; 3] {
        let lfo = |i: usize| lfo_at(s.lfos[i].shape, self.phase[i] + f64::from(s.lfos[i].phase), 0x5EED + i as u64);
        let db = 20.0 * self.env.max(1e-9).log10() + s.env.gain_db;
        [lfo(0), lfo(1), ((db - ENV_FLOOR_DB) / -ENV_FLOOR_DB).clamp(0.0, 1.0)]
    }

    /// Each target's offset from the smoothed sources (`offsets[0]`, "none", stays 0).
    fn offsets(&self, s: &MatrixSettings, offsets: &mut [f32]) {
        offsets.iter_mut().for_each(|o| *o = 0.0);
        for slot in &s.slots {
            if slot.target == 0 || slot.target >= offsets.len() {
                continue;
            }
            let v = match slot.source {
                Source::Off => continue,
                Source::Lfo1 => self.smooth[0],
                Source::Lfo2 => self.smooth[1],
                Source::Envelope => self.smooth[2],
            };
            offsets[slot.target] += slot.amount.clamp(-1.0, 1.0) * v;
        }
    }

    /// Runs `len` samples in control steps: before each, works the offsets out (into `offsets`,
    /// one per target) and calls `step` with the step's sample range; after it, moves the LFOs
    /// on and follows `level(i)` (the envelope's input, a sample's absolute level) over it.
    /// When nothing is routed the whole stretch is one call with every offset 0.
    pub fn steps(&mut self, s: &MatrixSettings, transport: &Transport, len: usize, offsets: &mut [f32], level: impl Fn(usize) -> f32, mut step: impl FnMut(std::ops::Range<usize>, &[f32])) -> MatrixReadings {
        let fsd = f64::from(self.fs);
        let active = s.active();
        if !active {
            offsets.iter_mut().for_each(|o| *o = 0.0);
            step(0..len, offsets);
        }
        let att = (-1.0 / (s.env.attack_ms.max(0.01) * 0.001 * self.fs)).exp();
        let rel = (-1.0 / (s.env.release_ms.max(0.01) * 0.001 * self.fs)).exp();
        let rates = [lfo_rate(&s.lfos[0], transport.tempo), lfo_rate(&s.lfos[1], transport.tempo)];
        let mut start = 0;
        while start < len {
            let n = (CONTROL - self.pos).min(len - start);
            if self.pos == 0 {
                // A new step: the smoothed sources and what they move.
                let raw = self.raw(s);
                for (v, r) in self.smooth.iter_mut().zip(raw) {
                    *v += (r - *v) * self.smooth_k;
                }
                if active {
                    self.offsets(s, offsets);
                }
            }
            if active {
                step(start..start + n, offsets);
            }
            for i in start..start + n {
                let x = level(i);
                let k = if x > self.env { att } else { rel };
                self.env = x + (self.env - x) * k;
            }
            // The LFOs move once a step completes, by a whole step: the same additions in the
            // same order whatever the host's blocks (a phase summed per host block rounds
            // differently).
            if self.pos + n == CONTROL {
                for (l, lfo) in s.lfos.iter().enumerate() {
                    let inc = rates[l] / fsd * CONTROL as f64;
                    // Synced and playing: locked to the song position, nudged (never a jump).
                    let mut nudge = 0.0;
                    if lfo.sync && transport.playing {
                        if let (Some(beats), Some(tempo)) = (transport.beats, transport.tempo) {
                            let at = beats + (start + n) as f64 * tempo / 60.0 / fsd;
                            let want = at / DIV_QUARTERS[lfo.division.min(DIV_QUARTERS.len() - 1)];
                            let err = (want - (self.phase[l] + inc) + 0.5).rem_euclid(1.0) - 0.5;
                            nudge = 0.1 * err;
                        }
                    }
                    self.phase[l] = (self.phase[l] + inc + nudge).rem_euclid(1_048_576.0);
                }
            }
            self.pos = (self.pos + n) % CONTROL;
            start += n;
        }
        MatrixReadings { sources: self.smooth, phases: self.phase, rates }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FS: f32 = 48_000.0;

    fn routed(source: Source, target: usize, amount: f32) -> MatrixSettings {
        let mut s = MatrixSettings::default();
        s.slots[0] = Slot { source, target, amount };
        s
    }

    /// The offsets of target 1 at each step's start, over `len` samples in blocks of `block`.
    fn trace(s: &MatrixSettings, len: usize, block: usize, level: f32) -> Vec<(usize, f32)> {
        let mut m = Matrix::new(FS);
        let mut offsets = [0.0f32; 4];
        let mut out = Vec::new();
        let mut done = 0;
        while done < len {
            let n = block.min(len - done);
            m.steps(s, &Transport::default(), n, &mut offsets, |_| level, |r, o| out.push((done + r.start, o[1])));
            done += n;
        }
        out
    }

    #[test]
    fn nothing_routed_is_one_call_with_no_offsets() {
        let s = MatrixSettings::default();
        assert!(!s.active());
        let t = trace(&s, 1000, 1000, 0.5);
        assert_eq!(t, vec![(0, 0.0)]);
    }

    #[test]
    fn the_steps_fall_on_the_same_samples_for_any_block_size() {
        let s = routed(Source::Lfo1, 1, 0.5);
        let at = |block: usize| {
            // Each step's (start, offset), merged where a host block split one.
            let mut v: Vec<(usize, f32)> = Vec::new();
            for (start, o) in trace(&s, 48_000, block, 0.0) {
                if v.last().is_none_or(|l| l.1 != o || start % CONTROL == 0) {
                    v.push((start, o));
                }
            }
            v.retain(|(st, _)| st % CONTROL == 0);
            v
        };
        let a = at(48_000);
        for block in [1, 7, 100, 333] {
            assert_eq!(at(block), a, "block {block}");
        }
    }

    #[test]
    fn an_lfo_sweeps_its_amount_and_the_envelope_follows_the_level() {
        // 1 Hz sine at 50 %: the offset reaches ±0.5 over a second.
        let s = routed(Source::Lfo1, 1, 0.5);
        let t = trace(&s, 48_000, 512, 0.0);
        let (lo, hi) = t.iter().fold((f32::MAX, f32::MIN), |(a, b), (_, o)| (a.min(*o), b.max(*o)));
        assert!((hi - 0.5).abs() < 0.01 && (lo + 0.5).abs() < 0.01, "{lo} … {hi}");
        // The envelope at −20 dBFS reads 2/3 (of −60…0 dB) once it has risen.
        let s = routed(Source::Envelope, 1, 1.0);
        let t = trace(&s, 24_000, 512, 0.1);
        assert!((t.last().expect("t").1 - 2.0 / 3.0).abs() < 0.01, "{:?}", t.last());
        // Negative amounts move the other way.
        let s = routed(Source::Envelope, 1, -0.5);
        assert!((trace(&s, 24_000, 512, 0.1).last().expect("t").1 + 1.0 / 3.0).abs() < 0.01);
    }

    #[test]
    fn every_shape_stays_in_range_and_the_random_ones_repeat() {
        for shape in LfoShape::ALL {
            for i in 0..1000 {
                let v = lfo_at(shape, i as f64 * 0.0137, 7);
                assert!((-1.0..=1.0).contains(&v), "{shape:?}: {v}");
            }
        }
        assert_eq!(lfo_at(LfoShape::SampleHold, 3.2, 1), lfo_at(LfoShape::SampleHold, 3.9, 1));
        assert_ne!(lfo_at(LfoShape::SampleHold, 3.2, 1), lfo_at(LfoShape::SampleHold, 4.2, 1));
    }

    #[test]
    fn a_synced_lfo_locks_to_the_song() {
        // 120 BPM, a quarter-note cycle: playing from beat 6, the phase settles on the beat.
        let mut s = routed(Source::Lfo1, 1, 1.0);
        s.lfos[0] = LfoSettings { sync: true, division: 8, ..LfoSettings::default() };
        let mut m = Matrix::new(FS);
        let mut offsets = [0.0f32; 4];
        let mut beats = 6.3;
        let mut r = MatrixReadings::default();
        for _ in 0..400 {
            let t = Transport { tempo: Some(120.0), beats: Some(beats), playing: true };
            r = m.steps(&s, &t, 512, &mut offsets, |_| 0.0, |_, _| {});
            beats += 512.0 / f64::from(FS) * 2.0;
        }
        let err = (beats - r.phases[0] + 0.5).rem_euclid(1.0) - 0.5;
        assert!(err.abs() < 0.01, "{err}");
        assert!((r.rates[0] - 2.0).abs() < 1e-9);
    }
}
