//! Times EQ Match's fit on a rough target (a mix's difference curve, made of ripples and
//! noise) for several band limits, and prints how close it gets.

use std::time::Instant;

use mite_dsp::analyzer::{spectrum_point_freq, spectrum_point_oct, SPECTRUM_POINTS};
use mite_dsp::eq::analog_response_db;
use mite_dsp::matching::{fit, match_target, match_weight};

fn main() {
    let f: Vec<f64> = (0..SPECTRUM_POINTS).map(spectrum_point_freq).collect();
    let mut seed = 1u32;
    let mut noise = || {
        seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        f64::from(seed >> 8) / f64::from(1u32 << 24) - 0.5
    };
    let source: Vec<f64> = f.iter().map(|&x| -40.0 + 6.0 * (x.log2() * 0.9).sin() + 2.0 * noise()).collect();
    let reference: Vec<f64> = f.iter().map(|&x| -35.0 + 4.0 * (x.log2() * 1.6).cos() - 3.0 * (x / 8000.0).log2().max(0.0) + 2.0 * noise()).collect();
    let weight = match_weight(&f, &reference, &source, -100.0);
    let target = match_target(&reference, &source, &weight, spectrum_point_oct(), 1.0 / 3.0, 1.0);
    for bands in [4, 8, 12, 24] {
        let start = Instant::now();
        let fitted = fit(&f, &target, &weight, bands);
        let ms = start.elapsed().as_secs_f64() * 1e3;
        let rms = {
            let (mut sum, mut total) = (0.0, 0.0);
            for i in 0..f.len() {
                let got: f64 = fitted.iter().map(|b| analog_response_db(b, f[i])).sum();
                sum += weight[i] * (target[i] - got).powi(2);
                total += weight[i];
            }
            (sum / total).sqrt()
        };
        println!("{bands:2} bands: {} placed, {ms:7.1} ms, rms error {rms:.2} dB", fitted.len());
    }
}
