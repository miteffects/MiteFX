//! Natural Phase at 44.1 kHz for the comparison bands: pre-ringing of the impulse response and the
//! worst error against the analog response, 20 Hz–20 kHz (`cargo run -p mite-dsp --example np_prering`).

use mite_dsp::eq::{analog_response, design, BandSettings, BandType};
use mite_dsp::filter::Tdf2;
use mite_dsp::natural_phase;
use realfft::num_complex::Complex;

fn main() {
    let fs: f64 = std::env::var("FS").ok().and_then(|v| v.parse().ok()).unwrap_or(44_100.0);
    let b = |kind, freq, gain_db, q, slope| BandSettings { enabled: true, kind, freq, gain_db, q, slope, ..Default::default() };
    let cases = [
        ("Bell 100 Q8", b(BandType::Bell, 100.0, 12.0, 8.0, 3)),
        ("LS 100", b(BandType::LowShelf, 100.0, 6.0, 0.71, 3)),
        ("LC 80 24", b(BandType::LowCut, 80.0, 0.0, 0.71, 3)),
        ("Bell 1k", b(BandType::Bell, 1000.0, 12.0, 1.0, 3)),
        ("Bell 10k", b(BandType::Bell, 10_000.0, 12.0, 2.0, 3)),
        ("Bell 16k -12", b(BandType::Bell, 16_000.0, -12.0, 0.7, 3)),
        ("HS 8k", b(BandType::HighShelf, 8000.0, 6.0, 0.71, 3)),
        ("HS 12k Q2", b(BandType::HighShelf, 12_000.0, 6.0, 2.0, 3)),
        ("HC 12k 24", b(BandType::HighCut, 12_000.0, 0.0, 0.71, 3)),
        ("HC 15k 96", b(BandType::HighCut, 15_000.0, 0.0, 0.71, 8)),
        ("Notch 15k", b(BandType::Notch, 15_000.0, 0.0, 2.0, 3)),
        ("BP 2k", b(BandType::BandPass, 2000.0, 0.0, 1.0, 3)),
        ("Tilt 1k", b(BandType::TiltShelf, 1000.0, 6.0, 0.71, 3)),
        ("Flat tilt", b(BandType::FlatTilt, 1000.0, 6.0, 0.71, 3)),
        ("All pass", b(BandType::AllPass, 1000.0, 0.0, 1.0, 3)),
    ];
    let ratio = ((fs / 48_000.0).round().max(1.0) as usize).next_power_of_two();
    let delay = natural_phase::DELAY_48K * ratio;
    let n = (1usize << 15) * ratio;
    for (name, s) in cases {
        let t0 = std::time::Instant::now();
        let k = natural_phase::design(&[s], fs, delay, natural_phase::TAPS_48K * ratio);
        let design_ms = t0.elapsed().as_secs_f64() * 1000.0;
        let sec = design(&s, fs);
        // Impulse through the IIR, then the kernel.
        let mut st = vec![Tdf2::default(); sec.as_slice().len()];
        let iir: Vec<f64> = (0..n).map(|i| sec.as_slice().iter().zip(&mut st).fold(if i == 0 { 1.0 } else { 0.0 }, |x, (c, t)| t.process(c, x))).collect();
        let h = &k.kernels[0];
        let mut y = vec![0.0; n];
        for (i, &v) in iir.iter().enumerate().take(n - h.len()) {
            if v != 0.0 {
                for (j, &c) in h.iter().enumerate() {
                    y[i + j] += v * c;
                }
            }
        }
        let peak = y.iter().fold(0.0f64, |m, v| m.max(v.abs()));
        let first = y.iter().position(|v| v.abs() > peak * 1e-3).unwrap();
        let before = y[..delay - 1].iter().fold(0.0f64, |m, v| m.max(v.abs()));
        let pre_ms = if first + 1 < delay { (delay - first) as f64 * 1000.0 / fs } else { 0.0 };
        let (mut mag, mut ph, mut at) = (0.0f64, 0.0f64, 0.0);
        let mut f = 20.0;
        while f <= 20_000.0 {
            let w = 2.0 * std::f64::consts::PI * f / fs;
            let r: Complex<f64> = y.iter().enumerate().take(n).map(|(i, &v)| Complex::from_polar(v, -w * i as f64)).sum::<Complex<f64>>() * Complex::from_polar(1.0, w * delay as f64);
            let a = analog_response(&s, f);
            if 20.0 * a.norm().log10() > -30.0 {
                let e = r / a;
                let m = (20.0 * e.norm().log10()).abs();
                if m > mag {
                    mag = m;
                    at = f;
                }
                ph = ph.max(e.arg().to_degrees().abs());
            }
            f *= 1.01;
        }
        let mut over = f64::MIN;
        let mut f = 20_000.0;
        while f < fs / 2.0 {
            let w = 2.0 * std::f64::consts::PI * f / fs;
            let r: Complex<f64> = y.iter().enumerate().take(n).map(|(i, &v)| Complex::from_polar(v, -w * i as f64)).sum();
            let d = sec.response(w);
            let a = analog_response(&s, f);
            over = over.max(20.0 * (r.norm() / d.norm().max(1e-12)).log10() - 20.0 * (a.norm() / d.norm().max(1e-12)).log10());
            f += 100.0;
        }
        print!("top {over:+5.1} dB  ");
        println!("{name:14} pre {pre_ms:5.2} ms ({:6.1} dB)  mag {mag:6.3} dB @{at:5.0}  phase {ph:5.1}°  design {design_ms:.1} ms", 20.0 * (before / peak).log10());
    }
}
