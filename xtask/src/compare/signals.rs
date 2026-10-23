//! Test signals. Every signal is stereo (both channels the same unless stated) and starts with
//! silence, so latency and pre-ringing show before the event.

use std::f64::consts::TAU;

/// Where the event of an impulse starts, seconds.
pub const IMPULSE_AT_S: f64 = 1.0;
/// Stepped tones: each level lasts this long; the gain is read over its last 40 %.
pub const STEP_S: f64 = 0.4;
pub const STEP_LEVELS_DB: std::ops::RangeInclusive<i32> = -60..=0;
/// The tone of the dynamics tests: an exact number of samples per period at 44.1 and 48 kHz
/// (882 Hz at 44.1 kHz is 50 samples; at 48 kHz 960 Hz is 50 samples).
pub fn tone_hz(rate: u32) -> f64 {
    f64::from(rate) / 50.0
}

fn db(v: f64) -> f64 {
    10f64.powf(v / 20.0)
}

/// A unit impulse at [`IMPULSE_AT_S`] in `seconds` of silence.
pub fn impulse(rate: u32, seconds: f64) -> Vec<Vec<f32>> {
    let n = (seconds * f64::from(rate)) as usize;
    let mut x = vec![0.0f32; n];
    x[(IMPULSE_AT_S * f64::from(rate)) as usize] = 1.0;
    vec![x.clone(), x]
}

/// The tone held at each level of [`STEP_LEVELS_DB`] (dBFS peak) for [`STEP_S`], after 0.5 s of
/// silence.
pub fn stepped_tone(rate: u32) -> Vec<Vec<f32>> {
    let f = tone_hz(rate);
    let lead = (0.5 * f64::from(rate)) as usize;
    let step = (STEP_S * f64::from(rate)) as usize;
    let levels: Vec<i32> = STEP_LEVELS_DB.collect();
    let mut x = vec![0.0f32; lead + step * levels.len() + lead];
    for (k, &l) in levels.iter().enumerate() {
        for i in 0..step {
            let t = (lead + k * step + i) as f64 / f64::from(rate);
            x[lead + k * step + i] = (db(f64::from(l)) * (TAU * f * t).sin()) as f32;
        }
    }
    vec![x.clone(), x]
}

/// The tone in level steps `(level dBFS, seconds)`, each starting on a zero crossing.
pub fn tone_steps(rate: u32, steps: &[(f64, f64)]) -> Vec<Vec<f32>> {
    let f = tone_hz(rate);
    let mut x = Vec::new();
    for &(level, secs) in steps {
        let n = (secs * f64::from(rate) / 50.0).round() as usize * 50;
        let start = x.len();
        x.extend((0..n).map(|i| (db(level) * (TAU * f * (start + i) as f64 / f64::from(rate)).sin()) as f32));
    }
    vec![x.clone(), x]
}

/// A sine at `freq` and `level` dBFS for `seconds`, after 0.5 s of silence.
pub fn sine(rate: u32, freq: f64, level: f64, seconds: f64) -> Vec<Vec<f32>> {
    let lead = (0.5 * f64::from(rate)) as usize;
    let n = (seconds * f64::from(rate)) as usize;
    let mut x = vec![0.0f32; lead + n];
    for i in 0..n {
        x[lead + i] = (db(level) * (TAU * freq * i as f64 / f64::from(rate)).sin()) as f32;
    }
    vec![x.clone(), x]
}

/// Left and right at different levels of the tone (the stereo link test).
pub fn split_tone(rate: u32, left_db: f64, right_db: f64, seconds: f64) -> Vec<Vec<f32>> {
    let l = tone_steps(rate, &[(-200.0, 0.5), (left_db, seconds)]).remove(0);
    let r = tone_steps(rate, &[(-200.0, 0.5), (right_db, seconds)]).remove(0);
    vec![l, r]
}

/// Pink-ish noise (Paul Kellet's filter over a fixed-seed generator) at about −18 dBFS RMS.
pub fn pink_noise(rate: u32, seconds: f64) -> Vec<Vec<f32>> {
    let n = (seconds * f64::from(rate)) as usize;
    let mut seed = 0x1234_5678u32;
    let mut b = [0.0f64; 7];
    let mut x = Vec::with_capacity(n);
    for _ in 0..n {
        seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        let white = f64::from(seed >> 8) / f64::from(1u32 << 24) * 2.0 - 1.0;
        b[0] = 0.99886 * b[0] + white * 0.0555179;
        b[1] = 0.99332 * b[1] + white * 0.0750759;
        b[2] = 0.96900 * b[2] + white * 0.1538520;
        b[3] = 0.86650 * b[3] + white * 0.3104856;
        b[4] = 0.55000 * b[4] + white * 0.5329522;
        b[5] = -0.7616 * b[5] - white * 0.0168980;
        let pink = b.iter().sum::<f64>() + white * 0.5362;
        b[6] = white * 0.115926;
        x.push((pink * 0.03) as f32);
    }
    vec![x.clone(), x]
}

/// A drum-like burst train: 60 ms decaying noise hits every 250 ms at `level` dBFS peak.
pub fn hits(rate: u32, level: f64, seconds: f64) -> Vec<Vec<f32>> {
    let n = (seconds * f64::from(rate)) as usize;
    let period = (0.25 * f64::from(rate)) as usize;
    let mut seed = 0x0bad_f00du32;
    let mut x = vec![0.0f32; n];
    for (i, v) in x.iter_mut().enumerate() {
        seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        let white = f64::from(seed >> 8) / f64::from(1u32 << 24) * 2.0 - 1.0;
        let t = (i % period) as f64 / f64::from(rate);
        if i >= period {
            *v = (db(level) * white * (-t / 0.015).exp()) as f32;
        }
    }
    vec![x.clone(), x]
}
