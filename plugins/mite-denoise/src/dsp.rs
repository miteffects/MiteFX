//! Mite DeNoise's engine: the denoiser (`mite_dsp::denoise`), the output gain, learning (the
//! editor asks, the engine learns and hands the profile to the state), a profile loaded with the
//! state, and the telemetry: the spectra, floor, threshold and gain per frequency, and the
//! peaks in and out.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use mite_core::AtomicF32;
use mite_dsp::denoise::{DenoiseSettings, Denoiser, DISPLAY_POINTS, PROFILE_POINTS, SILENT_DB};
use mite_dsp::{db_to_gain, gain_to_db, LinearSmoother, MINUS_INFINITY_DB};

use crate::params::NoiseProfile;

/// The editor's picture of the last frame (dB at `DISPLAY_POINTS` frequencies), the peaks in
/// and out since the editor last took them, and learning (editor → audio: wanted; audio →
/// editor: running).
pub struct DisplayTap {
    pub input: Vec<AtomicF32>,
    pub output: Vec<AtomicF32>,
    pub noise: Vec<AtomicF32>,
    pub threshold: Vec<AtomicF32>,
    pub gain: Vec<AtomicF32>,
    /// Frames published so far.
    pub frames: AtomicU64,
    pub peak_in: AtomicF32,
    pub peak_out: AtomicF32,
    pub learn: AtomicBool,
    pub learning: AtomicBool,
}

impl DisplayTap {
    pub fn new() -> Self {
        let row = |v: f32| (0..DISPLAY_POINTS).map(|_| AtomicF32::new(v)).collect();
        Self {
            input: row(SILENT_DB),
            output: row(SILENT_DB),
            noise: row(SILENT_DB),
            threshold: row(SILENT_DB),
            gain: row(0.0),
            frames: AtomicU64::new(0),
            peak_in: AtomicF32::new(MINUS_INFINITY_DB),
            peak_out: AtomicF32::new(MINUS_INFINITY_DB),
            learn: AtomicBool::new(false),
            learning: AtomicBool::new(false),
        }
    }

    /// The peaks since the last call, dBFS (in, out).
    pub fn take_peaks(&self) -> (f32, f32) {
        let take = |a: &AtomicF32| {
            let v = a.load();
            a.store(MINUS_INFINITY_DB);
            v
        };
        (take(&self.peak_in), take(&self.peak_out))
    }
}

impl Default for DisplayTap {
    fn default() -> Self {
        Self::new()
    }
}

pub struct DenoiseEngine {
    denoiser: Denoiser,
    gain: LinearSmoother,
    /// The profile version the denoiser holds.
    seen: u64,
    points: Vec<f32>,
}

impl DenoiseEngine {
    pub fn new(sample_rate: f32) -> Self {
        let mut e = Self { denoiser: Denoiser::new(f64::from(sample_rate)), gain: LinearSmoother::new(1.0), seen: 0, points: vec![SILENT_DB; PROFILE_POINTS] };
        e.prepare(sample_rate, 0.0);
        e
    }

    /// Sizes everything for `sample_rate`. Allocates.
    pub fn prepare(&mut self, sample_rate: f32, gain_db: f32) {
        if self.denoiser.sample_rate() != f64::from(sample_rate) {
            self.denoiser = Denoiser::new(f64::from(sample_rate));
            // The new one has no profile yet: read it again.
            self.seen = u64::MAX;
        }
        self.gain.set_ramp(sample_rate, 20.0);
        self.reset(gain_db);
    }

    pub fn reset(&mut self, gain_db: f32) {
        self.denoiser.reset();
        self.gain.reset(db_to_gain(gain_db));
    }

    /// The latency, samples: one frame.
    pub fn latency(&self) -> usize {
        self.denoiser.latency()
    }

    pub fn has_profile(&self) -> bool {
        self.denoiser.has_profile()
    }

    /// One host block in place (one or two channels).
    pub fn process(&mut self, channels: &mut [&mut [f32]], s: &DenoiseSettings, gain_db: f32, profile: &NoiseProfile, tap: &DisplayTap) {
        let n_ch = channels.len().min(2);
        if n_ch == 0 {
            return;
        }
        // A profile loaded with the state (or cleared): take it.
        let version = profile.version.load(Ordering::Acquire);
        if version != self.seen {
            for (p, a) in self.points.iter_mut().zip(&profile.points) {
                *p = a.load();
            }
            self.denoiser.set_profile(&self.points);
            self.seen = version;
        }
        // Learning: the editor asks; when it stops, the average is the profile.
        if self.denoiser.set_learning(tap.learn.load(Ordering::Relaxed)) {
            self.denoiser.profile(&mut self.points);
            self.seen = profile.store(&self.points);
        }
        tap.learning.store(self.denoiser.learning(), Ordering::Relaxed);
        let peak = |c: &[f32]| c.iter().fold(0.0f32, |m, v| m.max(v.abs()));
        let pin = channels.iter().take(n_ch).map(|c| peak(c)).fold(0.0, f32::max);
        if n_ch == 2 {
            let (a, b) = channels.split_at_mut(1);
            self.denoiser.process(a[0], Some(&mut *b[0]), s);
        } else {
            self.denoiser.process(channels[0], None, s);
        }
        self.gain.set_target(if s.bypass { 1.0 } else { db_to_gain(gain_db) });
        let len = channels.iter().take(n_ch).map(|c| c.len()).min().unwrap_or(0);
        for i in 0..len {
            let g = self.gain.next_value();
            for c in channels.iter_mut().take(n_ch) {
                c[i] *= g;
            }
        }
        let pout = channels.iter().take(n_ch).map(|c| peak(c)).fold(0.0, f32::max);
        tap.peak_in.store(tap.peak_in.load().max(gain_to_db(pin)));
        tap.peak_out.store(tap.peak_out.load().max(gain_to_db(pout)));
        let (d, fresh) = self.denoiser.display();
        if fresh {
            for (dst, src) in [(&tap.input, &d.input), (&tap.output, &d.output), (&tap.noise, &d.noise), (&tap.threshold, &d.threshold), (&tap.gain, &d.gain)] {
                for (a, &v) in dst.iter().zip(src) {
                    a.store(v);
                }
            }
            tap.frames.fetch_add(1, Ordering::Release);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn learning_hands_the_profile_to_the_state_and_a_loaded_one_comes_back() {
        let fs = 48_000.0;
        let profile = NoiseProfile::new();
        let tap = DisplayTap::new();
        let mut e = DenoiseEngine::new(fs);
        let s = DenoiseSettings { adaptive: false, ..DenoiseSettings::default() };
        let mut seed = 1u32;
        let mut noise = |n: usize| -> Vec<f32> {
            (0..n)
                .map(|_| {
                    seed ^= seed << 13;
                    seed ^= seed >> 17;
                    seed ^= seed << 5;
                    (seed as f32 / u32::MAX as f32 - 0.5) * 0.1
                })
                .collect()
        };
        tap.learn.store(true, Ordering::Relaxed);
        for _ in 0..100 {
            let (mut l, mut r) = (noise(480), noise(480));
            e.process(&mut [&mut l, &mut r], &s, 0.0, &profile, &tap);
        }
        assert!(tap.learning.load(Ordering::Relaxed));
        assert!(!profile.is_learned());
        tap.learn.store(false, Ordering::Relaxed);
        let (mut l, mut r) = (noise(480), noise(480));
        e.process(&mut [&mut l, &mut r], &s, 0.0, &profile, &tap);
        assert!(profile.is_learned() && e.has_profile());
        // Saved and loaded into another engine: it has the same profile.
        let json = profile.to_json().expect("learned");
        let other = NoiseProfile::new();
        other.load_json(&json);
        let mut f = DenoiseEngine::new(fs);
        let (mut l, mut r) = (noise(480), noise(480));
        f.process(&mut [&mut l, &mut r], &s, 0.0, &other, &tap);
        assert!(f.has_profile());
        // Cleared: none.
        other.clear();
        f.process(&mut [&mut l, &mut r], &s, 0.0, &other, &tap);
        assert!(!f.has_profile());
        assert!(tap.frames.load(Ordering::Relaxed) > 0);
    }
}
