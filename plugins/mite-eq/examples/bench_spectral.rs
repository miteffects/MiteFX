//! CPU of the spectral stage (spectral compressors, fixed and relative) at 48 kHz in 128-sample
//! host blocks, stereo noise, Zero Latency.
//!
//!     cargo run --release -p mite_eq --example bench_spectral

use mite_dsp::eq::{BandSettings, BandType, Dynamics};
use mite_eq::dsp::{spectral_latency, EqEngine};
use mite_eq::params::BANDS;

fn main() {
    let fs = 48_000.0_f32;
    for (label, count, relative) in [("1 relative band", 1, true), ("8 relative bands", 8, true), ("24 relative bands", 24, true), ("24 fixed-threshold bands", 24, false)] {
        let mut settings = [BandSettings::default(); BANDS];
        for (i, s) in settings.iter_mut().take(count).enumerate() {
            let freq = 40.0 * 1.3f64.powi(i as i32);
            *s = BandSettings { enabled: true, kind: BandType::Bell, freq, q: 0.5, dynamics: Dynamics { enabled: true, spectral: true, relative, ..Default::default() }, ..Default::default() };
        }
        let mut eq = EqEngine::new(fs, &settings, 0.0);
        let (mut l, mut r) = (vec![0.0f32; 128], vec![0.0f32; 128]);
        let mut seed = 1u32;
        let secs = 10;
        let t = std::time::Instant::now();
        for _ in 0..(secs * 48_000 / 128) {
            for (a, b) in l.iter_mut().zip(r.iter_mut()) {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                *a = (seed as f32 / u32::MAX as f32 - 0.5) * 0.1;
                *b = -*a;
            }
            eq.process(&mut [&mut l, &mut r], &settings, 0.0, None, None);
        }
        let cpu = t.elapsed().as_secs_f64() / f64::from(secs) * 100.0;
        let latency = spectral_latency(f64::from(fs));
        println!("{label:26}: {cpu:.2} % of one core, latency {latency} samples ({:.1} ms)", latency as f64 / f64::from(fs) * 1000.0);
    }
}
