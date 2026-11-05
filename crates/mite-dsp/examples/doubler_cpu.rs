//! Prints the doubler's CPU cost (48 kHz, 512-sample blocks), one and three doubles.
//!
//!     cargo run --release -p mite-dsp --example doubler_cpu

use mite_dsp::doubler::{Doubler, DoublerSettings};

fn main() {
    let fs = 48_000.0;
    let n = 48_000 * 10;
    let x: Vec<f32> = (0..n)
        .map(|i| 0.3 * (i as f32 * 0.013).sin() * if (i / 12_000) % 2 == 0 { 1.0 } else { 0.2 })
        .collect();
    for voices in [1, 3] {
        let s = DoublerSettings {
            voices,
            chorus: 0.5,
            ..DoublerSettings::default()
        };
        let mut d = Doubler::new(fs);
        let (mut l, mut r) = (vec![0.0f32; n], vec![0.0f32; n]);
        let t = std::time::Instant::now();
        for ((a, b), c) in l.chunks_mut(512).zip(r.chunks_mut(512)).zip(x.chunks(512)) {
            d.process([c, c], [a, b], false, &s);
        }
        println!(
            "{voices} double(s): {:.2} % of a core",
            t.elapsed().as_secs_f64() / 10.0 * 100.0
        );
    }
}
