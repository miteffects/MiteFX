//! How far a double's peaks go over the input's on a loud, sustained, distorted riff (each note
//! switches the double's reading while the last one still rings), and the worst click-like step.
//!
//!     cargo run --release -p mite-dsp --example doubler_overs

use mite_dsp::doubler::{Doubler, DoublerSettings};

fn main() {
    let fs = 48_000.0f32;
    let n = 48_000 * 8;
    // A high-gain chord: root and fifth through tanh(8x), re-picked every 250 ms, never decaying much.
    let roots = [82.41f32, 98.0, 110.0, 73.42];
    let x: Vec<f32> = (0..n)
        .map(|i| {
            let note = i / 12_000;
            let f = roots[note % 4];
            let t = (i % 12_000) as f32 / fs;
            let v = (std::f32::consts::TAU * f * t).sin() + 0.8 * (std::f32::consts::TAU * f * 1.5 * t).sin();
            0.7 * (8.0 * v * (0.6 + 0.4 * (-t / 0.05).exp())).tanh()
        })
        .collect();
    let peak_in = x.iter().fold(0.0f32, |m, v| m.max(v.abs()));
    let s = DoublerSettings { source_db: -120.0, chorus: 0.0, variation: 0.0, ..DoublerSettings::default() };
    let mut d = Doubler::new(fs);
    let (mut l, mut r) = (vec![0.0f32; n], vec![0.0f32; n]);
    for ((a, b), c) in l.chunks_mut(480).zip(r.chunks_mut(480)).zip(x.chunks(480)) {
        d.process([c, c], [a, b], false, &s);
    }
    let peak = r.iter().fold(0.0f32, |m, v| m.max(v.abs()));
    let d2 = |v: &[f32]| (1..v.len() - 1).map(|i| (v[i + 1] - 2.0 * v[i] + v[i - 1]).abs()).fold(0.0f32, f32::max);
    println!("double peak {:+.2} dB over the input's; worst second difference {:.4} (input {:.4})", 20.0 * (peak / peak_in).log10(), d2(&r[4800..]), d2(&x));
}
