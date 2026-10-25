//! CPU of the engine on 20 s of stereo noise at 48 kHz, one stage (the comparison's render-time
//! case: threshold −30 dB, 4:1, 10 / 100 ms), plus four stages and the busiest Smart mode.
//!
//!     cargo run --release -p mite_comp --example bench

use mite_comp::dsp::{CompEngine, Output};
use mite_core::HistoryTap;
use mite_dsp::dynamics::{CompSettings, Smart, Stage};

fn main() {
    let fs = 48_000.0_f32;
    let one = {
        let mut s = CompSettings::default();
        s.stages[0] = Stage { on: true, threshold_db: -30.0, ratio: 4.0, attack_ms: 10.0, release_ms: 100.0, detector: mite_dsp::dynamics::Detector::Peak, ..Stage::default() };
        s
    };
    let four = {
        let mut s = one;
        for (k, st) in s.stages.iter_mut().enumerate() {
            *st = Stage { on: true, threshold_db: -40.0 + 8.0 * k as f32, ..one.stages[0] };
        }
        s
    };
    let smart = CompSettings { smart: Smart::Vocal, ..four };
    let seconds = 20.0;
    let block: usize = std::env::var("BLOCK").ok().and_then(|v| v.parse().ok()).unwrap_or(512);
    let n = (seconds * fs) as usize;
    let mut seed = 1u32;
    let mut noise = || {
        seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        (seed >> 8) as f32 / (1u32 << 24) as f32 - 0.5
    };
    // QUIET=1: a short burst, then near-silence fading out (where denormals would show).
    let quiet = std::env::var("QUIET").is_ok();
    let mut sample = |i: usize| if quiet { noise() * 0.5 * (-(i as f32) / 4800.0).exp() } else { noise() * 0.5 };
    let input: [Vec<f32>; 2] = [(0..n).map(&mut sample).collect(), (0..n).map(&mut sample).collect()];
    for (name, s) in [("one stage", one), ("four stages", four), ("four stages, Smart Vocal", smart)] {
        let t_new = std::time::Instant::now();
        let mut engine = CompEngine::new(fs, &s);
        engine.prepare(44_100.0, &s, &Output::default());
        engine.prepare(fs, &s, &Output::default());
        let setup_ms = t_new.elapsed().as_secs_f64() * 1000.0;
        let history = HistoryTap::new(6000);
        let mut buf = input.clone();
        let t0 = std::time::Instant::now();
        for start in (0..n).step_by(block) {
            let end = (start + block).min(n);
            let [l, r] = &mut buf;
            let mut chans: [&mut [f32]; 2] = [&mut l[start..end], &mut r[start..end]];
            engine.process(&mut chans, None, &s, &Output::default(), &history);
        }
        let took = t0.elapsed().as_secs_f64();
        // MITE_BENCH_DUMP=<dir>: write the output (raw f32) to compare builds sample by sample.
        if let Ok(dir) = std::env::var("MITE_BENCH_DUMP") {
            let bytes: Vec<u8> = buf.iter().flat_map(|c| c.iter().flat_map(|v| v.to_le_bytes())).collect();
            std::fs::write(format!("{dir}/{}.f32", name.replace([' ', ','], "_")), bytes).expect("dump");
        }
        println!("{name:26}: {:.3} s for {seconds} s ({:.2} % of one core), set-up {setup_ms:.2} ms", took, took / f64::from(seconds) * 100.0);
    }
}
