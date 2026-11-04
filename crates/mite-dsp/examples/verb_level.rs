//! Prints the reverb's level against its input (white noise) and its CPU cost, at a few settings.
//!
//!     cargo run --release -p mite-dsp --example verb_level

use mite_dsp::reverb::{Reverb, ReverbSettings};

fn main() {
    // What the editor pays when a setting moves: the decay curve at 240 points, four bells on.
    let mut s = ReverbSettings::default();
    for (k, b) in s.bands.iter_mut().enumerate() {
        *b = mite_dsp::reverb::DecayBand { on: true, freq: 200.0 * 3f32.powi(k as i32), mult: 0.7, q: 1.0 };
    }
    let freqs: Vec<f64> = (0..240).map(|i| 20.0 * 1000f64.powf(i as f64 / 239.0)).collect();
    let mut out = vec![0.0; 240];
    let t = std::time::Instant::now();
    for _ in 0..100 {
        mite_dsp::reverb::decay_curve(&s, &freqs, 48_000.0, &mut out);
    }
    println!("decay curve, 240 points, four bells: {:.2} ms", t.elapsed().as_secs_f64() * 10.0);
    let fs = 48_000.0;
    let mut seed = 3u32;
    let mut noise = move || {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        (seed as f32 / u32::MAX as f32 - 0.5) * 0.5
    };
    let n = 48_000 * 10;
    let x: Vec<f32> = (0..n).map(|_| noise()).collect();
    let rms = |v: &[f32]| (v.iter().map(|s| f64::from(*s).powi(2)).sum::<f64>() / v.len() as f64).sqrt();
    for (name, s) in [
        ("default", ReverbSettings::default()),
        ("no early", ReverbSettings { early: 0.0, ..ReverbSettings::default() }),
        ("early only", ReverbSettings { decay_s: 0.1, ..ReverbSettings::default() }),
        ("small 0.6 s", ReverbSettings { size: 0.15, decay_s: 0.6, ..ReverbSettings::default() }),
        ("hall 4 s", ReverbSettings { size: 0.85, decay_s: 4.0, ..ReverbSettings::default() }),
        ("huge 15 s", ReverbSettings { size: 1.0, decay_s: 15.0, ..ReverbSettings::default() }),
    ] {
        let mut r = Reverb::new(fs, &s);
        let (mut l, mut rr) = (vec![0.0f32; n], vec![0.0f32; n]);
        let t = std::time::Instant::now();
        for ((a, b), c) in l.chunks_mut(512).zip(rr.chunks_mut(512)).zip(x.chunks(512)) {
            r.process([c, c], [a, b], true, &s);
        }
        let cpu = t.elapsed().as_secs_f64() / 10.0 * 100.0;
        let db = 20.0 * (rms(&l[n / 2..]) / rms(&x[n / 2..])).log10();
        println!("{name:12} wet {db:+.1} dB against the input, {cpu:.2} % of a core");
    }
}
