//! CPU of the engine on 20 s of stereo music at 48 kHz, 9 dB into the ceiling: every
//! oversampling factor, true peak on and off; then each stage of slices 4–7 at 2×.
//!
//!     cargo run --release -p mite_limit --example bench

#[path = "common/music.rs"]
mod music;

use mite_core::HistoryTap;
use mite_limit::dsp::{LimitEngine, LimitSettings, LoudnessTap, FACTORS};

fn main() {
    let fs = 48_000.0_f32;
    let seconds = 20.0;
    let block: usize = std::env::var("BLOCK").ok().and_then(|v| v.parse().ok()).unwrap_or(512);
    let input = music::music(seconds, fs);
    let n = input[0].len();
    let base = LimitSettings { gain_db: 9.0, ..LimitSettings::default() };
    let mut runs: Vec<(String, LimitSettings)> = Vec::new();
    for tp in [true, false] {
        for f in FACTORS {
            runs.push((format!("{f:>2}× true peak {}", if tp { "on " } else { "off" }), LimitSettings { oversampling: f, true_peak: tp, ..base }));
        }
    }
    runs.push((" 2× clipper (Soft)".into(), LimitSettings { clip_on: true, ..base }));
    runs.push((" 2× protect 100 %".into(), LimitSettings { protect: 1.0, ..base }));
    runs.push((" 2× unity".into(), LimitSettings { unity: true, ..base }));
    runs.push((" 2× dither 16 bit strong".into(), LimitSettings { dither: 3, shaping: 2, ..base }));
    runs.push((" 2× DC filter".into(), LimitSettings { dc_filter: true, ..base }));
    runs.push((" 2× all of them".into(), LimitSettings { clip_on: true, protect: 1.0, unity: true, dither: 3, shaping: 2, dc_filter: true, ..base }));
    {
        for (name, s) in runs {
            let mut engine = LimitEngine::new(fs, &s);
            let history = HistoryTap::new(6000);
            let tap = LoudnessTap::new();
            let mut buf = input.clone();
            let t0 = std::time::Instant::now();
            for start in (0..n).step_by(block) {
                let end = (start + block).min(n);
                let [l, r] = &mut buf;
                engine.process(&mut [&mut l[start..end], &mut r[start..end]], &s, &history, &tap);
            }
            let took = t0.elapsed().as_secs_f64();
            println!("{name}: {:.2} % of one core", took / f64::from(seconds) * 100.0);
        }
    }
}
