//! Zero Latency at 44.1 kHz for the comparison bands: the IIR plus its minimum-phase correction
//! against the analog prototype, worst magnitude and phase error 20 Hz–20 kHz where the curve is
//! within 30 dB of its peak (`cargo run -p mite-dsp --example zl_phase`).

use mite_dsp::eq::{BandSettings, BandType, analog_response, design};
use mite_dsp::min_phase::{TAPS_48K, zero_latency_correction};
use realfft::num_complex::Complex;

fn main() {
    let fs = 44_100.0;
    let b = |kind, freq, gain_db, q, slope| BandSettings {
        enabled: true,
        kind,
        freq,
        gain_db,
        q,
        slope,
        ..Default::default()
    };
    let cases = [
        ("Bell 1k", b(BandType::Bell, 1000.0, 12.0, 1.0, 3)),
        ("Bell 10k", b(BandType::Bell, 10_000.0, 12.0, 2.0, 3)),
        ("Bell 16k -12", b(BandType::Bell, 16_000.0, -12.0, 0.7, 3)),
        ("HS 8k", b(BandType::HighShelf, 8000.0, 6.0, 0.71, 3)),
        ("HS 12k Q2", b(BandType::HighShelf, 12_000.0, 6.0, 2.0, 3)),
        ("LC 80 24", b(BandType::LowCut, 80.0, 0.0, 0.71, 3)),
        ("HC 12k 24", b(BandType::HighCut, 12_000.0, 0.0, 0.71, 3)),
        ("HC 15k 96", b(BandType::HighCut, 15_000.0, 0.0, 0.71, 8)),
        ("Notch 15k", b(BandType::Notch, 15_000.0, 0.0, 2.0, 3)),
        ("BP 2k", b(BandType::BandPass, 2000.0, 0.0, 1.0, 3)),
        ("Tilt 1k", b(BandType::TiltShelf, 1000.0, 6.0, 0.71, 3)),
        ("Flat tilt", b(BandType::FlatTilt, 1000.0, 6.0, 0.71, 3)),
        ("All pass", b(BandType::AllPass, 1000.0, 0.0, 1.0, 3)),
    ];
    for (name, s) in cases {
        let t0 = std::time::Instant::now();
        let fir = zero_latency_correction(
            &[s],
            fs,
            &std::env::var("ZL_TAPS")
                .ok()
                .map(|v| {
                    v.split(",")
                        .map(|t| t.parse::<usize>().unwrap())
                        .collect::<Vec<_>>()
                })
                .unwrap_or(TAPS_48K.to_vec()),
            std::env::var("ZL_FFT")
                .ok()
                .map(|v| v.parse().unwrap())
                .unwrap_or(8192),
            false,
        )
        .map(|c| c.lr[0].clone())
        .unwrap_or_else(|| vec![1.0]);
        let ms = t0.elapsed().as_secs_f64() * 1000.0;
        let sec = design(&s, fs);
        let freqs: Vec<f64> = (0..240)
            .map(|i| 20.0 * 1000f64.powf(i as f64 / 239.0))
            .collect();
        let resp: Vec<(Complex<f64>, Complex<f64>)> = freqs
            .iter()
            .map(|&f| {
                let w = std::f64::consts::TAU * f / fs;
                let h: Complex<f64> = fir
                    .iter()
                    .enumerate()
                    .map(|(n, &v)| Complex::from_polar(f64::from(v), -w * n as f64))
                    .sum();
                (h * sec.response(w), analog_response(&s, f))
            })
            .collect();
        let top = resp
            .iter()
            .map(|(_, a)| 20.0 * a.norm().log10())
            .fold(f64::MIN, f64::max);
        let (mut mag, mut ph, mut at) = (0.0f64, 0.0f64, 0.0);
        for (&f, (d, a)) in freqs.iter().zip(&resp) {
            if 20.0 * a.norm().log10() > top - 30.0 {
                let e = d / a;
                mag = mag.max((20.0 * e.norm().log10()).abs());
                let p = e.arg().to_degrees().abs();
                if p > ph {
                    ph = p;
                    at = f;
                }
            }
        }
        if std::env::var("ZL_TRACE").ok().as_deref() == Some(name) {
            for (&f, (d, a)) in freqs.iter().zip(&resp).step_by(8) {
                let iir = sec.response(std::f64::consts::TAU * f / fs);
                println!(
                    "  {f:7.0} Hz  analog {:7.1} dB {:7.1}°  total {:7.1}°  iir {:7.1}°  err {:6.1}°",
                    20.0 * a.norm().log10(),
                    a.arg().to_degrees(),
                    d.arg().to_degrees(),
                    iir.arg().to_degrees(),
                    (d / a).arg().to_degrees()
                );
            }
        }
        println!(
            "{name:14} mag {mag:5.2} dB  phase {ph:6.1}° @{at:6.0}  taps {:4}  {ms:.1} ms",
            fir.len()
        );
    }
}
