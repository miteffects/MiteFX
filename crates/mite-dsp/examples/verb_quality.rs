//! Measures the reverb's impulse response at a few settings (`mite_dsp::reverb_quality`): how
//! soon the tail is dense, how periodic and how peaky it is, and its stereo correlation, plus a
//! Gaussian noise tail as the reference. Writes each IR as a WAV with `VERB_WAV=dir`.
//!
//!     cargo run --release -p mite-dsp --example verb_quality

use mite_dsp::reverb::{Reverb, ReverbSettings};

/// The absorption the first engine had built in: low ×1.2 below 250 Hz, high ×0.5 over 5 kHz.
const ROOM: ((f32, f32), (f32, f32)) = ((250.0, 1.2), (5000.0, 0.5));
use mite_dsp::reverb_quality::{
    dense_after, highpass, peakiness, peaks, periodic_lags, periodicity,
};

fn ir(s: &ReverbSettings, fs: f32, seconds: f32) -> [Vec<f32>; 2] {
    let n = (fs * seconds) as usize;
    let mut r = Reverb::new(fs, s);
    let mut x = vec![0.0f32; n];
    x[0] = 1.0;
    let (mut l, mut rr) = (vec![0.0f32; n], vec![0.0f32; n]);
    for ((a, b), c) in l.chunks_mut(512).zip(rr.chunks_mut(512)).zip(x.chunks(512)) {
        r.process([c, c], [a, b], true, s);
    }
    [l, rr]
}

fn report(name: &str, l: &[f32], r: &[f32], fs: f64) {
    let (l, r) = (&highpass(l, 150.0, fs)[..], &highpass(r, 150.0, fs)[..]);
    let dot: f64 = l
        .iter()
        .zip(r)
        .skip((0.08 * fs) as usize)
        .map(|(a, b)| f64::from(*a) * f64::from(*b))
        .sum();
    let e = |v: &[f32]| {
        v.iter()
            .skip((0.08 * fs) as usize)
            .map(|a| f64::from(*a).powi(2))
            .sum::<f64>()
    };
    let corr = dot / (e(l) * e(r)).sqrt();
    println!(
        "{name:28} dense after {:5.0} ms   periodicity {:.3}   peaky {:6.3} % (late {:6.3} %)   L/R ρ {:+.2}",
        dense_after(l, fs) * 1000.0,
        periodicity(l, fs, 0.15, 0.65),
        peakiness(l, fs, 0.1, 0.9) * 100.0,
        peakiness(l, fs, 0.5, 1.5) * 100.0,
        corr
    );
    if std::env::var_os("VERB_LAGS").is_some() {
        let lags: Vec<String> = periodic_lags(l, fs, 0.15, 0.65, 5)
            .iter()
            .map(|(ms, c)| format!("{ms:.2} ms {c:+.3}"))
            .collect();
        println!("    lags: {}", lags.join(", "));
        let pk: Vec<String> = peaks(l, fs, 0.1, 0.9, 12)
            .iter()
            .map(|(f, d)| format!("{f:.0} Hz {d:+.1}"))
            .collect();
        println!("    peaks: {}", pk.join(", "));
    }
}

fn write_wav(path: &str, l: &[f32], r: &[f32], fs: u32) {
    let mut out = Vec::new();
    let data = (l.len() * 8) as u32;
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&3u16.to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&fs.to_le_bytes());
    out.extend_from_slice(&(fs * 8).to_le_bytes());
    out.extend_from_slice(&8u16.to_le_bytes());
    out.extend_from_slice(&32u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data.to_le_bytes());
    for (a, b) in l.iter().zip(r) {
        out.extend_from_slice(&a.to_le_bytes());
        out.extend_from_slice(&b.to_le_bytes());
    }
    std::fs::write(path, out).expect("wav");
}

fn main() {
    let fs = 48_000.0;
    let wav = std::env::var("VERB_WAV").ok();
    let cases = [
        (
            "size 0, 1.0 s",
            ReverbSettings {
                size: 0.0,
                decay_s: 1.0,
                ..ReverbSettings::default()
            }
            .with_shelves(ROOM.0, ROOM.1),
        ),
        (
            "size 0, 2.0 s",
            ReverbSettings {
                size: 0.0,
                decay_s: 2.0,
                ..ReverbSettings::default()
            }
            .with_shelves(ROOM.0, ROOM.1),
        ),
        (
            "size 0, 1.0 s, flat",
            ReverbSettings {
                size: 0.0,
                decay_s: 1.0,
                ..ReverbSettings::default()
            },
        ),
        (
            "size 0, 2.0 s, no mod",
            ReverbSettings {
                size: 0.0,
                decay_s: 2.0,
                mod_depth: 0.0,
                ..ReverbSettings::default()
            }
            .with_shelves(ROOM.0, ROOM.1),
        ),
        (
            "size 25, 1.2 s",
            ReverbSettings {
                size: 0.25,
                decay_s: 1.2,
                ..ReverbSettings::default()
            }
            .with_shelves(ROOM.0, ROOM.1),
        ),
        (
            "default (50, 2 s)",
            ReverbSettings::default().with_shelves(ROOM.0, ROOM.1),
        ),
        ("50, 2 s, flat decay", ReverbSettings::default()),
        (
            "size 0, 2 s, flat decay",
            ReverbSettings {
                size: 0.0,
                ..ReverbSettings::default()
            },
        ),
        (
            "size 100, 4 s",
            ReverbSettings {
                size: 1.0,
                decay_s: 4.0,
                ..ReverbSettings::default()
            }
            .with_shelves(ROOM.0, ROOM.1),
        ),
    ];
    for (name, s) in cases {
        let [l, r] = ir(&s, fs, 2.0);
        report(name, &l, &r, f64::from(fs));
        if let Some(dir) = &wav {
            write_wav(
                &format!("{dir}/{}.wav", name.replace([' ', ',', '(', ')', '.'], "_")),
                &l,
                &r,
                fs as u32,
            );
        }
    }
    // The reference: Gaussian-ish noise under a 1 s exponential decay.
    let mut seed = 9u32;
    let mut noise = || {
        let mut s = 0.0;
        for _ in 0..6 {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            s += seed as f32 / u32::MAX as f32 - 0.5;
        }
        s
    };
    let n = (fs * 2.0) as usize;
    let l: Vec<f32> = (0..n)
        .map(|i| noise() * (-6.9 * i as f32 / fs).exp())
        .collect();
    let r: Vec<f32> = (0..n)
        .map(|i| noise() * (-6.9 * i as f32 / fs).exp())
        .collect();
    report("reference: noise, 1 s", &l, &r, f64::from(fs));
}
