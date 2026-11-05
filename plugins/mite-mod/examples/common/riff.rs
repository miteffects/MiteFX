//! Test material: a guitar-like riff (eighth notes at 120 bpm, a power-chord-ish root and fifth,
//! each note plucked: harmonics falling as 1/k, each decaying faster than the last, a pick click).

pub fn riff(seconds: f64, fs: f64) -> Vec<f32> {
    let n = (seconds * fs) as usize;
    let step = (0.25 * fs) as usize;
    let roots = [82.41, 82.41, 98.0, 82.41, 110.0, 82.41, 98.0, 73.42];
    let mut x = vec![0.0f32; n];
    let mut seed = 5u32;
    for (k, at) in (0..n).step_by(step).enumerate() {
        let f0 = roots[k % roots.len()];
        let len = (0.6 * fs) as usize;
        for i in 0..len.min(n - at) {
            let t = i as f64 / fs;
            let mut v = 0.0;
            for f in [f0, f0 * 1.5, f0 * 2.0] {
                for h in 1..10 {
                    let hf = f64::from(h);
                    v += (std::f64::consts::TAU * f * hf * t).sin() / hf * (-t * (2.0 + 1.2 * hf)).exp();
                }
            }
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            let click = (seed as f64 / f64::from(u32::MAX) - 0.5) * (-t / 0.002).exp();
            // Each note stops the last (a picked, muted riff).
            let gate = if i < step { 1.0 } else { (-((i - step) as f64) / (0.01 * fs)).exp() };
            x[at + i] += (0.12 * (v * (1.0 - (-t / 0.0015).exp()) + 0.6 * click) * gate) as f32;
        }
    }
    x
}
