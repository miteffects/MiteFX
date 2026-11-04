//! Test material: a voice — vowels (a harmonic series with formant-ish weights, gliding pitch) and
//! sibilants (noise band-passed about 7 kHz, sharp onsets) between them, loud and quiet phrases.

pub fn voice(seconds: f64, fs: f64) -> Vec<f32> {
    let n = (seconds * fs) as usize;
    let mut x = vec![0.0f32; n];
    let mut seed = 21u32;
    let w = std::f64::consts::TAU * 7000.0 / fs;
    let alpha = w.sin() / 3.0;
    let a0 = 1.0 + alpha;
    let (b0, b2, a1, a2) = (alpha / a0, -alpha / a0, -2.0 * w.cos() / a0, (1.0 - alpha) / a0);
    let mut st = [[0.0f64; 4]; 2];
    let mut phase = 0.0f64;
    for (i, v) in x.iter_mut().enumerate() {
        let t = i as f64 / fs;
        // A syllable every 0.4 s: 0.28 s of vowel, then 0.09 s of "s".
        let syl = t % 0.4;
        let phrase = if (t / 3.2) as usize % 2 == 0 { 1.0 } else { 0.25 };
        let f0 = 180.0 + 30.0 * (t * 1.3).sin();
        phase = (phase + f0 / fs).fract();
        let env = if syl < 0.28 { (syl / 0.02).min(1.0) * ((0.28 - syl) / 0.03).min(1.0) } else { 0.0 };
        let vowel: f64 = (1..30).map(|k| {
            let f = f0 * f64::from(k);
            let formant = (-((f - 700.0) / 300.0).powi(2)).exp() + 0.5 * (-((f - 1200.0) / 400.0).powi(2)).exp() + 0.15 * (-((f - 2800.0) / 600.0).powi(2)).exp();
            (std::f64::consts::TAU * phase * f64::from(k)).sin() * formant
        }).sum();
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        let mut nz = f64::from(seed) / f64::from(u32::MAX) - 0.5;
        for s in &mut st {
            let y = b0 * nz + b2 * s[1] - a1 * s[2] - a2 * s[3];
            s[1] = s[0];
            s[0] = nz;
            s[3] = s[2];
            s[2] = y;
            nz = y;
        }
        let s_env = if (0.29..0.38).contains(&syl) { ((syl - 0.29) / 0.004).min(1.0) * ((0.38 - syl) / 0.02).min(1.0) } else { 0.0 };
        *v = (phrase * (0.35 * env * vowel + 2.2 * s_env * nz)) as f32;
    }
    x
}
