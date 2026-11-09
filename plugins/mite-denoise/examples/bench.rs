//! CPU of the engine on 20 s of the examples' noisy voice, stereo, at 48 and 96 kHz.
//!
//!     cargo run --release -p mite_denoise --example bench

#[path = "common/sound.rs"]
mod sound;

use mite_denoise::dsp::{DenoiseEngine, DisplayTap};
use mite_denoise::params::NoiseProfile;
use mite_dsp::denoise::DenoiseSettings;

fn main() {
    for fs in [48_000.0f32, 96_000.0] {
        let seconds = 20.0;
        let x = sound::sound(seconds, fs, 2.0);
        let (mut l, mut r) = (x.clone(), x.clone());
        let mut e = DenoiseEngine::new(fs);
        let (tap, profile) = (DisplayTap::new(), NoiseProfile::new());
        let s = DenoiseSettings::default();
        let t0 = std::time::Instant::now();
        for (a, b) in l.chunks_mut(512).zip(r.chunks_mut(512)) {
            e.process(&mut [a, b], &s, 0.0, &profile, &tap);
        }
        println!("{} kHz: {:.2} % of one core", fs / 1000.0, t0.elapsed().as_secs_f64() / f64::from(seconds) * 100.0);
    }
}
