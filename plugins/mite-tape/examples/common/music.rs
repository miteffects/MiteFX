//! A stand-in for music: a bass line, a chord, a hat on every eighth and a kick on the beat, at
//! a mix's level (peaks around −8 dBFS), for the preview and the snapshot.

pub fn music(seconds: f64, fs: f64) -> Vec<f32> {
    let n = (seconds * fs) as usize;
    let beat = (0.5 * fs) as usize;
    let mut seed = 0x1234_5678u32;
    let mut noise = move || {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        seed as f32 / u32::MAX as f32 * 2.0 - 1.0
    };
    let bass = [55.0, 55.0, 73.4, 65.4];
    (0..n)
        .map(|i| {
            let t = i as f64 / fs;
            let b = i / beat;
            let in_beat = (i % beat) as f64 / fs;
            let in_eighth = (i % (beat / 2)) as f64 / fs;
            let f = bass[(b / 2) % bass.len()];
            let bass = 0.22 * (std::f64::consts::TAU * f * t).sin() * (1.0 - 0.3 * in_beat / 0.5);
            let chord: f64 = [220.0, 277.2, 329.6, 440.0].iter().map(|&f| (std::f64::consts::TAU * f * t).sin()).sum::<f64>() * 0.035;
            let kick = 0.5 * (std::f64::consts::TAU * (50.0 + 90.0 * (-in_beat * 30.0).exp()) * in_beat).sin() * (-in_beat * 9.0).exp();
            let hat = f64::from(noise()) * 0.06 * (-in_eighth * 60.0).exp();
            (0.3 * (bass + chord + kick + hat)) as f32
        })
        .collect()
}
