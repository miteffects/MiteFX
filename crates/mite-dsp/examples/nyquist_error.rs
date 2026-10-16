//! How far each band type's response strays from its analog prototype up to 20 kHz at 44.1 kHz
//! (BLUEPRINT §8.6: within 0.1 dB, Zero Latency), where the prototype is within 24 dB of unity:
//! the matched IIR alone, and with the minimum-phase correction (`min_phase`) of a few lengths.
//!
//!     cargo run --release -p mite-dsp --example nyquist_error

use mite_dsp::eq::{analog_response_db, design, BandSettings, BandType, Placement, DEFAULT_SLOPE};
use mite_dsp::min_phase::{zero_latency_correction, TAPS_48K};
use realfft::num_complex::Complex;

fn fir_db(h: &[f32], w: f64) -> f64 {
    let z: Complex<f64> = h.iter().enumerate().map(|(n, &v)| Complex::from_polar(f64::from(v), -w * n as f64)).sum();
    20.0 * z.norm().log10()
}

fn main() {
    let fs = 44_100.0;
    let w = |f: f64| 2.0 * std::f64::consts::PI * f / fs;
    // 0: the IIR alone; 1: with the correction, its length chosen per setting.
    let taps = [0usize, 1];
    for kind in BandType::ALL {
        let mut worst = [0.0f64; 2];
        let mut over = [0usize; 2];
        let (mut longest, mut lengths) = (0usize, Vec::new());
        let mut count = 0;
        let mut at = [String::new(), String::new()];
        for &freq in &[1000.0, 2000.0, 4000.0, 8000.0, 12_000.0, 16_000.0, 20_000.0] {
            for &gain in &[-18.0, -6.0, 6.0, 18.0] {
                for &q in &[0.3, 0.7, 2.0, 8.0] {
                    let s = BandSettings { enabled: true, kind, freq, gain_db: gain, q, slope: DEFAULT_SLOPE, placement: Placement::Stereo, dynamics: Default::default() };
                    let sections = design(&s, fs);
                    count += 1;
                    for (t, &n) in taps.iter().enumerate() {
                        let fir = if n == 0 { None } else { zero_latency_correction(&[s], fs, &TAPS_48K, 8192, false).map(|c| c.lr[0].clone()) };
                        if let Some(h) = &fir {
                            longest = longest.max(h.len());
                            lengths.push(h.len());
                        }
                        let mut band_worst = 0.0f64;
                        let mut f = 20.0;
                        while f <= 20_000.0 {
                            let analog = analog_response_db(&s, f);
                            if analog > -24.0 {
                                let digital = sections.response_db(w(f)) + fir.as_ref().map_or(0.0, |h| fir_db(h, w(f)));
                                band_worst = band_worst.max((digital - analog).abs());
                            }
                            f *= 1.02;
                        }
                        if band_worst > 0.1 {
                            over[t] += 1;
                        }
                        if band_worst > worst[t] {
                            worst[t] = band_worst;
                            at[t] = format!("{freq} Hz {gain:+} dB Q {q}");
                        }
                    }
                }
            }
        }
        println!("{kind:?} ({count} settings):");
        for (t, &n) in taps.iter().enumerate() {
            let label = if n == 0 { "IIR only" } else { "corrected" };
            println!("  {label:>10}: worst {:7.3} dB ({}), {} over 0.1 dB", worst[t], at[t], over[t]);
        }
        lengths.sort_unstable();
        let median = lengths.get(lengths.len() / 2).copied().unwrap_or(0);
        println!("  kernel length: median {median}, longest {longest} ({} of {count} settings need one)", lengths.len());
    }
}
