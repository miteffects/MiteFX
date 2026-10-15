//! CPU of Natural Phase and of Linear Phase (BLUEPRINT §8.5: 24 bands at High < 3 % of a core) per resolution, at
//! 48 kHz in 128-sample host blocks, stereo. The kernel holds every band, so the band count
//! changes the design time, not the audio cost.
//!
//!     cargo run --release -p mite_eq --example bench_lp

use mite_dsp::eq::{BandSettings, BandType, Placement};
use mite_eq::dsp::{linear_phase_latency, natural_phase_latency, EqEngine, PhaseMode, LP_RESOLUTIONS};
use mite_eq::params::BANDS;

fn main() {
    let fs = 48_000.0_f32;
    let mut settings = [BandSettings::default(); BANDS];
    for (i, s) in settings.iter_mut().enumerate() {
        *s = BandSettings { enabled: true, kind: BandType::Bell, freq: 40.0 * 1.3f64.powi(i as i32), gain_db: if i % 2 == 0 { 3.0 } else { -3.0 }, q: 1.0, ..Default::default() };
    }
    for (label, mid_side) in [("stereo bands", false), ("with a mid band (2×2 matrix)", true)] {
        settings[5].placement = if mid_side { Placement::Mid } else { Placement::Stereo };
        let runs = std::iter::once((PhaseMode::NaturalPhase, 0, "Natural")).chain(LP_RESOLUTIONS.iter().enumerate().map(|(r, n)| (PhaseMode::LinearPhase, r, *n)));
        for (mode, res, name) in runs {
            let mut eq = EqEngine::new(fs, &settings, 0.0);
            eq.set_mode(mode, res);
            eq.prepare(fs, &settings, 0.0);
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
            let latency = if mode == PhaseMode::NaturalPhase { natural_phase_latency(f64::from(fs)) } else { linear_phase_latency(res, f64::from(fs)) };
            println!(
                "{label:30} {:7}: {cpu:.2} % of one core, latency {latency} samples ({:.1} ms)",
                name,
                latency as f64 / f64::from(fs) * 1000.0
            );
        }
    }
}
