//! Measures the reverb's impulse response at a few settings (`mite_dsp::reverb_quality`): how
//! soon the tail is dense, how periodic and how peaky it is, and its stereo correlation, plus a
//! Gaussian noise tail as the reference, and Zita-rev1's network (Fons Adriaensen's, the
//! algorithm re-implemented here for measurement only) as a well-made one. Per case also the
//! modes' overlap (modes per Hz × a mode's width, from the loops' lengths: under about 1 the
//! modes stand apart and ring) and T30 per octave against the settings. Writes each IR as a WAV
//! with `VERB_WAV=dir`.
//!
//!     cargo run --release -p mite-dsp --example verb_quality

use mite_dsp::reverb::{LINES, Reverb, ReverbSettings, decay_at, line_len};

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

/// Modes per Hz (the loops' whole delay, s) times a mode's −3 dB width (2.2 / T60).
fn modal_overlap(s: &ReverbSettings, fs: f32) -> f64 {
    let total: f64 = (0..LINES).map(|i| f64::from(line_len(i, s.size, fs))).sum::<f64>() / f64::from(fs);
    total * 2.2 / f64::from(s.decay_s)
}

/// An octave band (two second-order band-passes, Q √2), as the tests measure.
fn octave(x: &[f32], fc: f64, fs: f64) -> Vec<f64> {
    let w = std::f64::consts::TAU * fc / fs;
    let (sin, cos) = w.sin_cos();
    let alpha = sin / (2.0 * std::f64::consts::SQRT_2);
    let a0 = 1.0 + alpha;
    let (b0, b2, a1, a2) = (alpha / a0, -alpha / a0, -2.0 * cos / a0, (1.0 - alpha) / a0);
    let mut y: Vec<f64> = x.iter().map(|&v| f64::from(v)).collect();
    for _ in 0..2 {
        let (mut x1, mut x2, mut y1, mut y2) = (0.0, 0.0, 0.0, 0.0);
        for v in y.iter_mut() {
            let x0 = *v;
            let out = b0 * x0 + b2 * x2 - a1 * y1 - a2 * y2;
            (x2, x1, y2, y1) = (x1, x0, y1, out);
            *v = out;
        }
    }
    y
}

/// T30 (Schroeder's backward integration, −5…−35 dB fitted), s.
fn t30(band: &[f64], fs: f64) -> f64 {
    let mut acc = 0.0;
    let mut edc = vec![0.0; band.len()];
    for i in (0..band.len()).rev() {
        acc += band[i] * band[i];
        edc[i] = acc;
    }
    let (mut sx, mut sy, mut sxx, mut sxy, mut n) = (0.0, 0.0, 0.0, 0.0, 0.0);
    for (i, &e) in edc.iter().enumerate() {
        let d = 10.0 * (e / edc[0]).max(1e-30).log10();
        if (-35.0..=-5.0).contains(&d) {
            let t = i as f64 / fs;
            (sx, sy, sxx, sxy, n) = (sx + t, sy + d, sxx + t * t, sxy + t * d, n + 1.0);
        }
    }
    -60.0 / ((n * sxy - sx * sy) / (n * sxx - sx * sx))
}

const OCTAVES: [f64; 4] = [250.0, 1000.0, 4000.0, 8000.0];

fn octave_line(l: &[f32], r: &[f32], fs: f64, target: impl Fn(f64) -> f64) -> String {
    let mono: Vec<f32> = l.iter().zip(r).map(|(a, b)| a + b).collect();
    OCTAVES
        .iter()
        .map(|&fc| format!("{fc:.0} Hz {:+.0} %", (t30(&octave(&mono, fc, fs), fs) / target(fc) - 1.0) * 100.0))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Zita-rev1's late network (Fons Adriaensen, GPL): 8 lines of 125…257 ms, each with one 13…31 ms
/// allpass (±0.6) at its end, a Hadamard mix, a low/mid two-band decay with a gentle high damping,
/// no modulation; the input 0.3 into each line, the outputs two sums of lines. Re-implemented
/// from the published algorithm for measurement; nothing of it is in the plugin.
fn zita_ir(fs: f32, rt: f32, seconds: f32) -> [Vec<f32>; 2] {
    const DIFF: [f32; 8] = [0.020346, 0.024421, 0.031604, 0.027333, 0.022904, 0.029291, 0.013458, 0.019123];
    const DELAY: [f32; 8] = [0.153129, 0.210389, 0.127837, 0.256891, 0.174713, 0.192303, 0.125, 0.219991];
    struct Line {
        ap: Vec<f32>,
        ai: usize,
        c: f32,
        d: Vec<f32>,
        di: usize,
        gmf: f32,
        glo: f32,
        wlo: f32,
        whi: f32,
        slo: f32,
        shi: f32,
    }
    let fdamp = 0.49 * fs + 1.0; // as Zita's damping at its top: the mid T60 halved at Nyquist
    let chi = if fdamp > 0.49 * fs { 2.0 } else { 1.0 - (std::f32::consts::TAU * fdamp / fs).cos() };
    let mut lines: Vec<Line> = (0..8)
        .map(|i| {
            let k1 = (DIFF[i] * fs).round() as usize;
            let k2 = (DELAY[i] * fs).round() as usize;
            let del = DELAY[i];
            let gmf = 0.001f32.powf(del / rt);
            let glo = 0.001f32.powf(del / rt) / gmf - 1.0;
            let g = 0.001f32.powf(del / (0.5 * rt)) / gmf;
            let t = (1.0 - g * g) / (2.0 * g * g * chi);
            Line {
                ap: vec![0.0; k1],
                ai: 0,
                c: if i & 1 == 1 { -0.6 } else { 0.6 },
                d: vec![0.0; k2 - k1],
                di: 0,
                gmf,
                glo,
                wlo: std::f32::consts::TAU * 200.0 / fs,
                whi: ((1.0 + 4.0 * t).sqrt() - 1.0) / (2.0 * t),
                slo: 0.0,
                shi: 0.0,
            }
        })
        .collect();
    let n = (fs * seconds) as usize;
    let (mut ol, mut or) = (vec![0.0f32; n], vec![0.0f32; n]);
    let g = 0.125f32.sqrt();
    for j in 0..n {
        let inp = if j == 0 { 1.0 } else { 0.0 };
        let t = 0.3 * inp;
        let mut x = [0.0f32; 8];
        for (i, ln) in lines.iter_mut().enumerate() {
            let sign = if i % 4 < 2 { 1.0 } else { -1.0 };
            // Both inputs the same impulse: the left feeds lines 0…3, the right 4…7.
            let v = ln.d[ln.di] + sign * t;
            let z = ln.ap[ln.ai];
            let w = v - ln.c * z;
            ln.ap[ln.ai] = w;
            ln.ai = (ln.ai + 1) % ln.ap.len();
            x[i] = z + ln.c * w;
        }
        for h in [1, 2, 4] {
            for i in 0..8 {
                if i & h == 0 {
                    let (a, b) = (x[i], x[i + h]);
                    x[i] = a + b;
                    x[i + h] = a - b;
                }
            }
        }
        ol[j] = x[1] + x[2];
        or[j] = x[1] - x[2];
        for (i, ln) in lines.iter_mut().enumerate() {
            let mut y = g * x[i];
            ln.slo += ln.wlo * (y - ln.slo) + 1e-10;
            y += ln.glo * ln.slo;
            ln.shi += ln.whi * (y - ln.shi);
            ln.d[ln.di] = ln.gmf * ln.shi;
            ln.di = (ln.di + 1) % ln.d.len();
        }
    }
    [ol, or]
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
        println!(
            "    modes overlap {:.2}   T30 against the settings: {}",
            modal_overlap(&s, fs),
            octave_line(&l, &r, f64::from(fs), |f| decay_at(&s, f, f64::from(fs)))
        );
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
    for rt in [1.0f32, 2.0] {
        let [l, r] = zita_ir(fs, rt, 2.0);
        report(&format!("reference: Zita-rev1, {rt} s"), &l, &r, f64::from(fs));
        let total: f32 = 0.153129 + 0.210389 + 0.127837 + 0.256891 + 0.174713 + 0.192303 + 0.125 + 0.219991;
        println!(
            "    modes overlap {:.2}   T30 against {rt} s: {}",
            f64::from(total * 2.2 / rt),
            octave_line(&l, &r, f64::from(fs), |_| f64::from(rt))
        );
    }
}
