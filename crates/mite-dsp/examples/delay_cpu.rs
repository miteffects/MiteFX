//! Prints the delay's CPU cost (48 kHz, 512-sample blocks) at a few settings.
//!
//!     cargo run --release -p mite-dsp --example delay_cpu

use mite_dsp::delay::{DelayLine, DelaySettings, Mode, TapSettings};
use mite_dsp::eq::{BandSettings, BandType};

fn main() {
    let fs = 48_000.0;
    let n = 48_000 * 10;
    let x: Vec<f32> = (0..n).map(|i| 0.3 * (i as f32 * 0.013).sin()).collect();
    let full = |mode: Mode| {
        let mut s = DelaySettings { mode, mod_depth: 0.5, diffusion: 0.5, drive: 0.3, duck_db: 6.0, ..DelaySettings::default() };
        for (t, tap) in s.taps.iter_mut().enumerate() {
            *tap = TapSettings { on: true, time_ms: 120.0 * (t + 1) as f32, feedback: 0.6, pingpong: t % 2 == 1, ..TapSettings::default() };
        }
        for (b, band) in s.eq.iter_mut().enumerate() {
            *band = BandSettings { enabled: true, kind: if b == 0 { BandType::LowCut } else { BandType::Bell }, freq: 150.0 * 2f64.powi(b as i32), gain_db: -2.0, ..BandSettings::default() };
        }
        s
    };
    for (name, s) in [("default (one tap)", DelaySettings::default()), ("four taps, six bands, digital", full(Mode::Digital)), ("four taps, six bands, tape", full(Mode::Tape)), ("four taps, six bands, BBD", full(Mode::Bbd))] {
        let mut d = DelayLine::new(fs, &s);
        let (mut l, mut r) = (vec![0.0f32; n], vec![0.0f32; n]);
        let t = std::time::Instant::now();
        for ((a, b), c) in l.chunks_mut(512).zip(r.chunks_mut(512)).zip(x.chunks(512)) {
            d.process([c, c], [a, b], true, &s);
        }
        println!("{name:32} {:.2} % of a core", t.elapsed().as_secs_f64() / 10.0 * 100.0);
    }
}
