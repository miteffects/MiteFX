//! Times the pitch detector (one window, the editor runs it every 20 ms) at 48 and 96 kHz.
//!
//!     cargo run --release -p mite-dsp --example pitch_bench

use mite_dsp::pitch::{PitchDetector, window_len};

fn main() {
    for fs in [48_000.0, 96_000.0] {
        let mut d = PitchDetector::new(window_len(fs));
        let x: Vec<f32> = (0..d.len())
            .map(|i| {
                (1..8)
                    .map(|k| {
                        (std::f64::consts::TAU * 82.41 * f64::from(k) * i as f64 / fs).sin()
                            / f64::from(k)
                    })
                    .sum::<f64>() as f32
                    * 0.2
            })
            .collect();
        let t = std::time::Instant::now();
        for _ in 0..100 {
            std::hint::black_box(d.detect(&x, fs));
        }
        println!(
            "{fs} Hz, window {}: {:.2} ms a detection",
            d.len(),
            t.elapsed().as_secs_f64() * 10.0
        );
    }
}
