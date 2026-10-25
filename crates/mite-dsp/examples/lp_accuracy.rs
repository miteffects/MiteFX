//! Linear Phase accuracy for narrow low bands: the kernel's worst magnitude error against the
//! analog curve, 20 Hz–20 kHz, at each resolution (`cargo run -p mite-dsp --example lp_accuracy`).

use mite_dsp::eq::{analog_response_db, BandSettings, BandType};
use mite_dsp::linear_phase;

fn main() {
    let fs = 44_100.0;
    let b = |kind, freq, gain_db, q, slope| BandSettings { enabled: true, kind, freq, gain_db, q, slope, ..Default::default() };
    let cases = [
        ("Bell 100 Q8", b(BandType::Bell, 100.0, 12.0, 8.0, 3)),
        ("LC 80 24", b(BandType::LowCut, 80.0, 0.0, 0.71, 3)),
        ("Bell 1k", b(BandType::Bell, 1000.0, 12.0, 1.0, 3)),
        ("Bell 40 Q4 -12", b(BandType::Bell, 40.0, -12.0, 4.0, 3)),
    ];
    for (name, s) in cases {
        print!("{name:14}");
        for len in [1024, 2048, 4096, 8192, 16384] {
            let t0 = std::time::Instant::now();
            let k = linear_phase::design(&[s], fs, len);
            let ms = t0.elapsed().as_secs_f64() * 1000.0;
            let h = &k.kernels[0];
            let mut worst = 0.0f64;
            let mut f = 20.0;
            while f <= 20_000.0 {
                let w = 2.0 * std::f64::consts::PI * f / fs;
                let (re, im) = h.iter().enumerate().fold((0.0, 0.0), |(re, im), (n, &v)| (re + v * (w * n as f64).cos(), im - v * (w * n as f64).sin()));
                let got = 10.0 * (re * re + im * im).log10();
                worst = worst.max((got - analog_response_db(&s, f)).abs());
                f *= 1.01;
            }
            print!("  {len:5}: {worst:5.2} ({ms:3.0} ms)");
        }
        println!();
    }
}
