//! Prints the de-esser's CPU cost (48 kHz stereo, 512-sample blocks) for each way of reducing.
//!
//!     cargo run --release -p mite-dsp --example deess_cpu

use mite_dsp::deess::{DeEssSettings, DeEsser, Reduce};

fn main() {
    let fs = 48_000.0;
    let n = 48_000 * 10;
    let mut seed = 1u32;
    let x: Vec<f32> = (0..n)
        .map(|_| {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            (seed as f32 / u32::MAX as f32 - 0.5) * 0.5
        })
        .collect();
    for reduce in [Reduce::Shelf, Reduce::Band, Reduce::Wide, Reduce::Spectral] {
        let s = DeEssSettings { reduce, threshold_db: -20.0, ..DeEssSettings::default() };
        let mut d = DeEsser::new(fs, &s);
        let (mut l, mut r) = (x.clone(), x.clone());
        let t = std::time::Instant::now();
        for (a, b) in l.chunks_mut(512).zip(r.chunks_mut(512)) {
            d.process(&mut [a, b], &s);
        }
        println!("{reduce:?}: {:.2} % of a core", t.elapsed().as_secs_f64() / 10.0 * 100.0);
    }
}
