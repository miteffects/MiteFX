//! Measures what the user hears: the output's loudness against the input's (auto gain on) for
//! 1, 2 and 3 doubles, and per octave how the left and right sum to mono — the mono sum's power
//! against the two sides' (0 dB: like two independent takes; −: cancelling), the mean and the
//! worst tenth of 50 ms windows.
//!
//!     cargo run --release -p mite_double --example mono_check

#[path = "common/riff.rs"]
mod riff;

use mite_core::HistoryTap;
use mite_double::dsp::{DoubleEngine, Output};
use mite_dsp::doubler::DoublerSettings;

fn band(x: &[f32], lo: f64, hi: f64, fs: f64) -> Vec<f64> {
    // Two RBJ band-pass sections between lo and hi.
    let fc = (lo * hi).sqrt();
    let q = fc / (hi - lo);
    let w = std::f64::consts::TAU * fc / fs;
    let alpha = w.sin() / (2.0 * q);
    let a0 = 1.0 + alpha;
    let (b0, b2, a1, a2) = (alpha / a0, -alpha / a0, -2.0 * w.cos() / a0, (1.0 - alpha) / a0);
    let mut y: Vec<f64> = x.iter().map(|&v| f64::from(v)).collect();
    for _ in 0..2 {
        let (mut x1, mut x2, mut y1, mut y2) = (0.0, 0.0, 0.0, 0.0);
        for v in y.iter_mut() {
            let x0 = *v;
            let out = b0 * x0 + b2 * x2 - a1 * y1 - a2 * y2;
            x2 = x1;
            x1 = x0;
            y2 = y1;
            y1 = out;
            *v = out;
        }
    }
    y
}

fn main() {
    let fs = 48_000.0;
    let x = riff::riff(8.0, fs);
    let power = |v: &[f32]| v.iter().map(|s| f64::from(*s).powi(2)).sum::<f64>() / v.len() as f64;
    for voices in [1, 2, 3] {
        let s = DoublerSettings { voices, ..DoublerSettings::default() };
        let out = Output::default();
        let mut e = DoubleEngine::new(fs as f32, &out);
        let history = HistoryTap::new(64);
        let (mut l, mut r) = (x.clone(), vec![0.0f32; x.len()]);
        for (a, b) in l.chunks_mut(512).zip(r.chunks_mut(512)) {
            e.process(&mut [a, b], false, &s, &out, &history);
        }
        let skip = 48_000;
        let loud = 10.0 * ((power(&l[skip..]) + power(&r[skip..])) / (2.0 * power(&x[skip..]))).log10();
        let mut line = format!("{voices} double(s): output {loud:+.2} dB against the input; mono sum per octave (mean / worst tenth):");
        for (lo, hi) in [(63.0, 125.0), (125.0, 250.0), (250.0, 500.0), (500.0, 1000.0), (1000.0, 2000.0), (2000.0, 4000.0)] {
            let (bl, br) = (band(&l, lo, hi, fs), band(&r, lo, hi, fs));
            let mut ratios: Vec<f64> = (skip..x.len() - 2400)
                .step_by(2400)
                .map(|i| {
                    let (mut m, mut st) = (0.0, 0.0);
                    for k in i..i + 2400 {
                        m += (bl[k] + br[k]).powi(2);
                        st += bl[k].powi(2) + br[k].powi(2);
                    }
                    10.0 * (m / st.max(1e-30)).log10()
                })
                .collect();
            ratios.sort_by(f64::total_cmp);
            let mean = 10.0 * (ratios.iter().map(|d| 10f64.powf(d / 10.0)).sum::<f64>() / ratios.len() as f64).log10();
            line += &format!(" {lo:.0}: {mean:+.1}/{:+.1}", ratios[ratios.len() / 10]);
        }
        println!("{line}");
    }
}
