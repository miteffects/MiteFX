//! CPU of the engine on 20 s of stereo music at 48 kHz: one band per model and oversampling
//! factor, and six bands of Tube at 4×.
//!
//!     cargo run --release -p mite_sat --example bench

#[path = "common/music.rs"]
mod music;

use mite_core::HistoryTap;
use mite_dsp::sat::{Model, SatSettings};
use mite_sat::dsp::{Output, SatEngine};

fn main() {
    let fs = 48_000.0_f32;
    let seconds = 20.0;
    let block: usize = std::env::var("BLOCK").ok().and_then(|v| v.parse().ok()).unwrap_or(512);
    let input = music::music(seconds, fs);
    let n = input[0].len();
    let one = |model: Model| {
        let mut s = SatSettings::default();
        s.bands[0].model = model;
        s.bands[0].drive_db = 12.0;
        s
    };
    let mut six = one(Model::Tube);
    six.crossovers = [Some(120.0), Some(500.0), Some(2000.0), Some(6000.0), Some(12_000.0)];
    six.bands = [six.bands[0]; 6];
    let mut cases: Vec<(String, SatSettings, usize)> = Vec::new();
    for m in [Model::Tube, Model::Soft, Model::Fold] {
        for f in [1, 2, 4, 8] {
            cases.push((format!("{} {f}×", m.name()), one(m), f));
        }
    }
    cases.push(("six bands Tube 4×".into(), six, 4));
    for (name, s, factor) in cases {
        let out = Output::default();
        let mut engine = SatEngine::new(fs, factor, &s);
        let history = HistoryTap::new(6000);
        let mut buf = input.clone();
        let t0 = std::time::Instant::now();
        for start in (0..n).step_by(block) {
            let end = (start + block).min(n);
            let [l, r] = &mut buf;
            let mut chans: [&mut [f32]; 2] = [&mut l[start..end], &mut r[start..end]];
            engine.process(&mut chans, &s, factor, &out, &history, None);
        }
        let took = t0.elapsed().as_secs_f64();
        println!("{name:22}: {:.2} % of one core", took / f64::from(seconds) * 100.0);
    }
}
