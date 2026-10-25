//! CPU of the engine on 20 s of a stereo drum loop at 48 kHz: one band, three, and five with
//! lookahead.
//!
//!     cargo run --release -p mite_shape --example bench

#[path = "common/drums.rs"]
mod drums;

use mite_core::HistoryTap;
use mite_dsp::shape::ShapeSettings;
use mite_shape::dsp::{Output, ShapeEngine};

fn main() {
    let fs = 48_000.0_f32;
    let one = {
        let mut s = ShapeSettings::default();
        s.bands[0].attack_db = 6.0;
        s.bands[0].sustain_db = -4.0;
        s
    };
    let three = ShapeSettings { crossovers: [Some(150.0), Some(3500.0), None, None], bands: [one.bands[0]; 5], ..one };
    let five = ShapeSettings { crossovers: [Some(120.0), Some(500.0), Some(2000.0), Some(8000.0)], lookahead_ms: 2.0, ..three };
    let seconds = 20.0;
    let block: usize = std::env::var("BLOCK").ok().and_then(|v| v.parse().ok()).unwrap_or(512);
    let input = drums::drum_loop(seconds, fs);
    let n = input[0].len();
    for (name, s) in [("one band", one), ("three bands", three), ("five bands, lookahead", five)] {
        let mut engine = ShapeEngine::new(fs, &s);
        let history = HistoryTap::new(6000);
        let mut buf = input.clone();
        let t0 = std::time::Instant::now();
        for start in (0..n).step_by(block) {
            let end = (start + block).min(n);
            let [l, r] = &mut buf;
            let mut chans: [&mut [f32]; 2] = [&mut l[start..end], &mut r[start..end]];
            engine.process(&mut chans, &s, &Output::default(), &history, None);
        }
        let took = t0.elapsed().as_secs_f64();
        println!("{name:22}: {took:.3} s for {seconds} s ({:.2} % of one core)", took / f64::from(seconds) * 100.0);
    }
}
