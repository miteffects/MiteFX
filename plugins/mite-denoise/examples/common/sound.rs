//! The examples' test sound.

/// Hiss (a little brighter up top) and 50 Hz hum, plus, from `voice_from`, a 180 Hz "voice" with
/// harmonics that comes and goes.
pub fn sound(seconds: f32, fs: f32, voice_from: f32) -> Vec<f32> {
    let n = (seconds * fs) as usize;
    let mut seed = 0x1234_5678u32;
    let mut last = 0.0f32;
    (0..n)
        .map(|i| {
            let t = i as f32 / fs;
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            let white = seed as f32 / u32::MAX as f32 - 0.5;
            let hiss = 0.02 * (white - 0.6 * last);
            last = white;
            let hum = 0.004 * (std::f32::consts::TAU * 50.0 * t).sin();
            let voice = if t >= voice_from && (t * 1.3).fract() < 0.7 {
                (1..12).map(|h| (std::f32::consts::TAU * 180.0 * h as f32 * t).sin() * 0.15 / h as f32).sum::<f32>()
            } else {
                0.0
            };
            hiss + hum + voice
        })
        .collect()
}
