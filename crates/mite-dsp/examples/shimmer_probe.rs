//! Measures the reverb's shimmer on a chord (Shimmer Hall's settings): per second, the share of
//! the tail's energy over 6 kHz, its spectral centroid, the envelope's roughness (modulation
//! 15–80 Hz against the level: grain buzz), and the tail's spectral peakiness (ringing).
//!
//!     cargo run --release -p mite-dsp --example shimmer_probe

use mite_dsp::reverb::{DecayBand, ReverbSettings, Reverb};
use mite_dsp::reverb_quality::peakiness;
use realfft::RealFftPlanner;

const FS: f32 = 48_000.0;

fn render(s: &ReverbSettings, x: &[f32]) -> Vec<f32> {
    let mut r = Reverb::new(FS, s);
    let (mut l, mut rr) = (vec![0.0; x.len()], vec![0.0; x.len()]);
    for (i, ch) in x.chunks(256).enumerate() {
        let (a, b) = (i * 256, i * 256 + ch.len());
        r.process([ch, ch], [&mut l[a..b], &mut rr[a..b]], true, s);
    }
    l
}

fn spectrum(v: &[f32]) -> Vec<f64> {
    let n = v.len();
    let mut p = RealFftPlanner::<f64>::new();
    let fft = p.plan_fft_forward(n);
    let mut inp: Vec<f64> = v.iter().enumerate().map(|(i, &x)| f64::from(x) * (0.5 - 0.5 * (std::f64::consts::TAU * i as f64 / n as f64).cos())).collect();
    let mut out = fft.make_output_vec();
    fft.process(&mut inp, &mut out).unwrap();
    out.iter().map(|c| c.norm_sqr()).collect()
}

fn main() {
    let secs = 10usize;
    let n = secs * FS as usize;
    // A C major chord, 0.6 s, soft attack.
    let x: Vec<f32> = (0..n)
        .map(|i| {
            let t = i as f32 / FS;
            if t > 0.6 { return 0.0; }
            let env = (t / 0.01).min(1.0) * (1.0 - t / 0.6);
            env * [261.6f32, 329.6, 392.0].iter().map(|f| 0.1 * (std::f32::consts::TAU * f * t).sin()).sum::<f32>()
        })
        .collect();
    let base = ReverbSettings {
        predelay_ms: 20.0, size: 0.9, decay_s: 8.0, diffusion: 0.9, early: 0.2, mod_depth: 0.5, mod_rate: 0.4, width: 1.2,
        bands: { let mut b = [DecayBand::default(); 8]; b[0] = DecayBand::low_shelf(300.0, 0.7); b },
        ..ReverbSettings::default()
    };
    let amounts: Vec<f32> = std::env::args().skip(1).filter_map(|a| a.parse().ok()).collect();
    let amounts = if amounts.is_empty() { vec![0.0, 0.4, 1.0] } else { amounts };
    for amt in amounts {
        let s = ReverbSettings { shimmer: amt, shimmer_pitch: 12.0, ..base };
        let y = render(&s, &x);
        println!("shimmer {amt}: peaky tail {:.3} %", peakiness(&y, f64::from(FS), 1.0, 3.0) * 100.0);
        for t in [1usize, 2, 3, 5, 8] {
            let seg = &y[t * 48_000..(t + 1) * 48_000];
            let sp = spectrum(seg);
            let hz = |k: usize| k as f64 * f64::from(FS) / seg.len() as f64;
            let tot: f64 = sp.iter().sum();
            let hi: f64 = sp.iter().enumerate().filter(|(k, _)| hz(*k) > 6000.0).map(|(_, p)| p).sum();
            let cen: f64 = sp.iter().enumerate().map(|(k, p)| hz(k) * p).sum::<f64>() / tot;
            // Envelope: RMS over 2 ms, then its modulation 15–80 Hz relative to its mean.
            let env: Vec<f32> = seg.chunks(96).map(|c| (c.iter().map(|v| v * v).sum::<f32>() / c.len() as f32).sqrt()).collect();
            let mean = env.iter().sum::<f32>() / env.len() as f32;
            let es = spectrum(&env.iter().map(|e| e - mean).collect::<Vec<_>>());
            let ehz = |k: usize| k as f64 * 500.0 / env.len() as f64;
            let rough: f64 = es.iter().enumerate().filter(|(k, _)| (15.0..80.0).contains(&ehz(*k))).map(|(_, p)| p).sum::<f64>().sqrt() / (f64::from(mean) * env.len() as f64);
            let db = 10.0 * (tot / seg.len() as f64).max(1e-30).log10();
            println!("  {t} s: level {db:6.1} dB  over 6 kHz {:5.1} %  centroid {cen:6.0} Hz  roughness {rough:.3}", 100.0 * hi / tot);
        }
    }
}
