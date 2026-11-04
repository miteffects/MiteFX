//! How clean a detuned double is: a high sine through one double at a fixed detune, the share of
//! the output's energy that is not the (shifted) sine — the roughness an interpolator adds as the
//! reading slides (dB, lower is cleaner).
//!
//!     cargo run --release -p mite-dsp --example doubler_purity

use mite_dsp::doubler::{Doubler, DoublerSettings};

fn main() {
    let fs = 48_000.0f32;
    for hz in [2000.0f32, 6000.0, 10_000.0] {
        let n = 48_000;
        let x: Vec<f32> = (0..n).map(|i| 0.5 * (std::f32::consts::TAU * hz * i as f32 / fs).sin()).collect();
        let s = DoublerSettings { source_db: -200.0, chorus: 0.0, variation: 0.0, timing_ms: 0.0, pitch_cents: 15.0, follow_notes: false, ..DoublerSettings::default() };
        let mut d = Doubler::new(fs);
        let (mut l, mut r) = (vec![0.0f32; n], vec![0.0f32; n]);
        for ((a, b), c) in l.chunks_mut(480).zip(r.chunks_mut(480)).zip(x.chunks(480)) {
            d.process([c, c], [a, b], false, &s);
        }
        // 8192 samples from 0.2 s in (the detune at work, before it eases), Blackman window.
        let seg = &r[9600..9600 + 8192];
        let w = |i: usize| { let t = i as f32 / 8191.0; 0.42 - 0.5 * (std::f32::consts::TAU * t).cos() + 0.08 * (2.0 * std::f32::consts::TAU * t).cos() };
        let spec: Vec<f64> = (0..4096).map(|k| {
            let (mut re, mut im) = (0.0f64, 0.0f64);
            for (i, &v) in seg.iter().enumerate() {
                let ph = std::f64::consts::TAU * k as f64 * i as f64 / 8192.0;
                re += f64::from(v * w(i)) * ph.cos();
                im -= f64::from(v * w(i)) * ph.sin();
            }
            re * re + im * im
        }).collect();
        let peak = (0..4096).max_by(|&a, &b| spec[a].total_cmp(&spec[b])).unwrap_or(0);
        let main: f64 = spec[peak.saturating_sub(6)..(peak + 7).min(4096)].iter().sum();
        let total: f64 = spec.iter().sum();
        println!("{hz} Hz: the rest {:.1} dB under the tone", 10.0 * ((total - main) / main).log10());
    }
}
