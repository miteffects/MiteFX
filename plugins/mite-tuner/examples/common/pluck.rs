//! Test material: a plucked string (harmonics falling as 1/k, each decaying faster than the last,
//! the pitch drifting `drift` cents over its first second as a string settles), re-plucked every
//! `every` seconds.
#![allow(dead_code)]

pub fn pluck(hz: f64, drift: f64, seconds: f64, every: f64, fs: f64) -> Vec<f32> {
    let n = (seconds * fs) as usize;
    let period = (every * fs) as usize;
    let mut phase = [0.0f64; 10];
    (0..n)
        .map(|i| {
            let t = (i % period) as f64 / fs;
            let f = hz * 2f64.powf(drift * (-t / 0.4).exp() / 1200.0);
            let mut v = 0.0;
            for (k, ph) in phase.iter_mut().enumerate() {
                let k1 = (k + 1) as f64;
                *ph = (*ph + f * k1 / fs).fract();
                v += (std::f64::consts::TAU * *ph).sin() / k1 * (-t * (0.8 + 0.5 * k1)).exp();
            }
            (0.35 * v * (1.0 - (-t / 0.002).exp())) as f32
        })
        .collect()
}
