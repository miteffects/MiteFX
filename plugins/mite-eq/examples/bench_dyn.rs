//! CPU of 8 bands in the engine, static vs dynamic with the level swinging through the threshold
//! (the worst case for dynamic redesigns). BLUEPRINT §8.6 budget: 8 dynamic bands < 1.5 %.
//! A third run keeps the output stage busy (mix, delta, auto gain, balance and a solo); a fourth
//! is the Zero Latency correction's worst case (resonant shelves near Nyquist: the longest
//! kernels).
//!
//!     cargo run --release -p mite_eq --example bench_dyn

use mite_dsp::eq::{BandSettings, BandType, Dynamics};
use mite_eq::dsp::{EqEngine, Output};
use mite_eq::params::BANDS;
fn main() {
    let fs = 48_000.0_f32;
    for (dynamic, busy, worst) in [(false, false, false), (true, false, false), (false, true, false), (false, false, true)] {
        let mut settings = [BandSettings::default(); BANDS];
        for (i, s) in settings.iter_mut().take(8).enumerate() {
            *s = BandSettings {
                enabled: true,
                kind: BandType::Bell,
                freq: 60.0 * 2f64.powi(i as i32),
                gain_db: 2.0,
                q: 1.0,
                dynamics: Dynamics { enabled: dynamic, threshold_db: -40.0, ..Default::default() },
                ..Default::default()
            };
            if worst {
                *s = BandSettings { enabled: true, kind: BandType::HighShelf, freq: 16_000.0 + 500.0 * i as f64, gain_db: 6.0, q: 8.0, ..Default::default() };
            }
        }
        let mut eq = EqEngine::new(fs, &settings, 0.0);
        if busy {
            eq.set_output(Output { mix: 0.7, delta: true, auto_gain: true, pan: 0.3, solo: Some(3), ..Default::default() });
        }
        let mut seed = 1u32;
        let secs = 20;
        let (mut l, mut r) = (vec![0.0f32; 512], vec![0.0f32; 512]);
        let t = std::time::Instant::now();
        let mut n = 0u64;
        for _ in 0..(secs * 48_000 / 512) {
            for (a, b) in l.iter_mut().zip(r.iter_mut()) {
                seed ^= seed << 13; seed ^= seed >> 17; seed ^= seed << 5;
                // Level swings ±20 dB around the threshold at 3 Hz: the dynamic gain never rests.
                n += 1;
                let swing = 10f32.powf((n as f32 * 3.0 / 48_000.0 * std::f32::consts::TAU).sin());
                *a = (seed as f32 / u32::MAX as f32 - 0.5) * 0.05 * swing;
                *b = *a;
            }
            eq.process(&mut [&mut l, &mut r], &settings, 0.0, None, None);
        }
        let cpu = t.elapsed().as_secs_f64() / secs as f64 * 100.0;
        let stage = if busy { ", output stage busy" } else if worst { ", Zero Latency correction worst case" } else { "" };
        println!("8 bands, dynamic={dynamic}{stage}: {cpu:.2} % of one core (stereo, 48 kHz)");
    }
}
