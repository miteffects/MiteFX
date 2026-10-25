//! How long `prepare` takes (it designs the starting kernels on the calling thread), per phase
//! mode, for one high band at 44.1 and 192 kHz.
//!
//!     cargo run --release -p mite_eq --example bench_prepare

use mite_dsp::eq::{BandSettings, BandType};
use mite_eq::dsp::{EqEngine, PhaseMode};
use mite_eq::params::BANDS;

fn main() {
    let mut settings = [BandSettings::default(); BANDS];
    // BENCH_FREQ: the bell's frequency (a low one needs no Zero Latency correction).
    let freq = std::env::var("BENCH_FREQ").ok().and_then(|v| v.parse().ok()).unwrap_or(10_000.0);
    settings[0] = BandSettings { enabled: true, kind: BandType::Bell, freq, gain_db: 12.0, q: 2.0, ..Default::default() };
    for sr in [44_100.0f32, 192_000.0] {
        for (mode, name) in [(PhaseMode::ZeroLatency, "Zero Latency"), (PhaseMode::NaturalPhase, "Natural Phase"), (PhaseMode::LinearPhase, "Linear Phase High")] {
            let mut eq = EqEngine::new(sr, &settings, 0.0);
            eq.set_mode(mode, 2);
            let t0 = std::time::Instant::now();
            eq.prepare(sr, &settings, 0.0);
            let prepared = t0.elapsed().as_secs_f64() * 1000.0;
            // Hosts re-activate with the same settings around a render: that one is cached.
            let t_again = std::time::Instant::now();
            eq.prepare(sr, &settings, 0.0);
            let again = t_again.elapsed().as_secs_f64() * 1000.0;
            // Then 4 s of noise, as one of the comparison's renders.
            let n = (4.0 * sr) as usize;
            let mut seed = 1u32;
            let mut noise = || {
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                (seed >> 8) as f32 / (1u32 << 24) as f32 - 0.5
            };
            let (mut l, mut r): (Vec<f32>, Vec<f32>) = ((0..n).map(|_| noise()).collect(), (0..n).map(|_| noise()).collect());
            let t1 = std::time::Instant::now();
            for start in (0..n).step_by(512) {
                let end = (start + 512).min(n);
                let mut chans: [&mut [f32]; 2] = [&mut l[start..end], &mut r[start..end]];
                eq.process(&mut chans, &settings, 0.0, None, None);
            }
            println!("{sr:6} Hz {name:18}: prepare {prepared:6.1} ms (again {again:4.1} ms), 4 s of audio {:6.1} ms", t1.elapsed().as_secs_f64() * 1000.0);
        }
    }
}
