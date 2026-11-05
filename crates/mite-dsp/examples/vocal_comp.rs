//! Mite Comp's Vocal mode on a sung line: phrases at different levels (a verse sung softly, a
//! chorus belted, a quiet ending), syllables varying within each. Prints each phrase's loudness
//! in and out (K-weighted, the voice only, gaps left out) and the spread, with auto makeup on.
//!
//!     cargo run --release -p mite-dsp --example vocal_comp

use mite_dsp::dynamics::{CompSettings, Compressor, Smart, Stage, STAGES};
use mite_dsp::loudness::KWeighting;

const FS: f32 = 48_000.0;

/// A voice: a vowel (220 Hz with harmonics, vibrato) in syllables of 120…400 ms with 40…120 ms
/// gaps, each syllable ±4 dB around its phrase's level. Returns the signal and, per sample, the
/// phrase it belongs to (None in gaps).
fn voice(phrases: &[(f32, f32)]) -> (Vec<f32>, Vec<Option<usize>>) {
    let mut seed = 7u32;
    let mut rnd = || {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        seed as f32 / u32::MAX as f32
    };
    let (mut x, mut tag) = (Vec::new(), Vec::new());
    let mut phase = 0.0f32;
    for (p, &(seconds, db)) in phrases.iter().enumerate() {
        let end = x.len() + (seconds * FS) as usize;
        while x.len() < end {
            let syl = ((0.12 + 0.28 * rnd()) * FS) as usize;
            let gap = ((0.04 + 0.08 * rnd()) * FS) as usize;
            let g = 10f32.powf((db + 8.0 * (rnd() - 0.5)) / 20.0);
            let f0 = 180.0 + 120.0 * rnd();
            for i in 0..syl {
                let env = (i as f32 / (0.015 * FS)).min(1.0) * ((syl - i) as f32 / (0.03 * FS)).min(1.0);
                let t = x.len() as f32 / FS;
                phase += f0 * (1.0 + 0.004 * (std::f32::consts::TAU * 5.5 * t).sin()) / FS;
                let v: f32 = (1..14).map(|k| (std::f32::consts::TAU * phase * k as f32).sin() / (k as f32).powf(1.2)).sum();
                x.push(g * env * v * 0.5);
                tag.push(Some(p));
            }
            for _ in 0..gap {
                x.push(0.0);
                tag.push(None);
            }
        }
    }
    (x, tag)
}

fn phrase_db(y: &[f32], tag: &[Option<usize>], n: usize) -> Vec<f64> {
    let mut k = KWeighting::new(f64::from(FS));
    let mut acc = vec![(0.0f64, 0usize); n];
    for (v, t) in y.iter().zip(tag) {
        let w = k.run(*v);
        if let Some(p) = t {
            acc[*p].0 += w * w;
            acc[*p].1 += 1;
        }
    }
    acc.iter().map(|(e, c)| 10.0 * (e / (*c).max(1) as f64).max(1e-20).log10()).collect()
}

/// How different two renders are: the difference's level against the first's, dB.
fn diff_db(a: &[f32], b: &[f32]) -> f64 {
    let e: f64 = a.iter().zip(b).map(|(x, y)| f64::from(x - y).powi(2)).sum();
    let p: f64 = a.iter().map(|x| f64::from(*x).powi(2)).sum();
    10.0 * (e / p.max(1e-20)).max(1e-20).log10()
}

fn main() {
    if std::env::args().nth(1).as_deref() == Some("attack") {
        let (x, _) = voice(&[(6.0, -20.0), (6.0, -14.0)]);
        let amount: f32 = std::env::var("AMOUNT").ok().and_then(|v| v.parse().ok()).unwrap_or(0.7);
        let det = match std::env::var("DET").as_deref() { Ok("peak") => mite_dsp::dynamics::Detector::Peak, _ => mite_dsp::dynamics::Detector::Rms };
        let auto = std::env::var("AUTO").map_or(true, |v| v != "0");
        let render = |smart: Smart, attack: f32, ratio: f32, knee: f32| {
            let mut stages = [Stage::default(); STAGES];
            stages[0] = Stage { on: true, threshold_db: -24.0, ratio, knee_db: knee, attack_ms: attack, release_ms: 120.0, detector: det, ..Stage::default() };
            let s = CompSettings { stages, smart, auto_makeup: auto, smart_amount: amount, sibilance: 0.5, ..CompSettings::default() };
            let mut c = Compressor::new(FS);
            c.prepare(FS, &s);
            let mut y = x.clone();
            for b in y.chunks_mut(512) {
                c.process(&mut [b], None, &s);
            }
            y
        };
        // Each syllable's first 30 ms against its whole, dB (how much of the attack gets through).
        let (_, tag) = voice(&[(6.0, -20.0), (6.0, -14.0)]);
        let onsets: Vec<usize> = (1..tag.len()).filter(|&i| tag[i].is_some() && tag[i - 1].is_none()).collect();
        let punch = |y: &[f32]| {
            let rms = |r: std::ops::Range<usize>| (y[r.clone()].iter().map(|v| f64::from(*v).powi(2)).sum::<f64>() / r.len() as f64).sqrt();
            let mut acc = 0.0;
            for &o in &onsets {
                let mut end = o;
                while end < tag.len() && tag[end].is_some() {
                    end += 1;
                }
                if end - o > 4800 {
                    acc += 20.0 * (rms(o..o + 1440) / rms(o + 2400..end)).log10();
                }
            }
            acc / onsets.len() as f64
        };
        println!("input: onsets {:+.1} dB against the syllable", punch(&x));
        // The gain at times after an onset that follows a long gap (the 10th syllable), dB.
        let o = onsets[10];
        for att in [0.05, 10.0, 50.0, 250.0] {
            let y = render(Smart::Vocal, att, 4.0, 6.0);
            let g = |ms: f64| {
                let i = o + (ms * 48.0) as usize;
                let r = |v: &[f32]| (v[i - 48..i + 48].iter().map(|s| f64::from(*s).powi(2)).sum::<f64>()).sqrt();
                20.0 * (r(&y) / r(&x)).log10()
            };
            let pts: Vec<String> = [2.0, 5.0, 10.0, 20.0, 50.0, 100.0, 200.0, 300.0].iter().map(|&t| format!("{:6.1}", g(t))).collect();
            println!("vocal attack {att:6.2}: gain at 2,5,10,20,50,100,200,300 ms: {}", pts.join(" "));
        }
        for (name, smart, ratio, knee) in [("off 4:1", Smart::Off, 4.0, 6.0), ("vocal", Smart::Vocal, 4.0, 6.0)] {
            for att in [0.05, 10.0, 50.0, 250.0] {
                println!("{name:10} attack {att:6.2} ms: onsets {:+.2} dB against the syllable", punch(&render(smart, att, ratio, knee)));
            }
            let a = render(smart, 0.05, ratio, knee);
            let b = render(smart, 250.0, ratio, knee);
            let reduced = diff_db(&x, &a);
            println!("{name:10} attack 0.05 vs 250 ms: difference {:6.1} dB under the output; the compression itself {:6.1} dB", diff_db(&a, &b), reduced);
        }
        return;
    }
    if std::env::args().nth(1).as_deref() == Some("level") {
        // Threshold −28 dB (the user's comparison with Pro-C 3, Auto Gain on), auto makeup on: a
        // line of four phrases sung three times; the last time round is measured (auto makeup
        // has heard the song by then). Loudness in and out per phrase, and overall.
        // STEADY=1: a voice that stays at one level.
        let line = if std::env::var("STEADY").is_ok() { [(6.0, -14.0); 4] } else { [(6.0, -16.0), (6.0, -10.0), (6.0, -20.0), (6.0, -13.0)] };
        let phrases: Vec<(f32, f32)> = line.iter().cycle().take(12).cloned().collect();
        let (x, tag) = voice(&phrases);
        let n = phrases.len();
        let last = |v: Vec<f64>| v[n - 4..].to_vec();
        let input = last(phrase_db(&x, &tag, n));
        let mean = |v: &[f64]| 10.0 * (v.iter().map(|d| 10f64.powf(d / 10.0)).sum::<f64>() / v.len() as f64).log10();
        let show = |name: &str, v: &[f64]| {
            let d: Vec<String> = v.iter().map(|d| format!("{d:6.1}")).collect();
            println!("{name:22} {}   overall {:6.1}  ({:+5.1} dB)", d.join(" "), mean(v), mean(v) - mean(&input));
        };
        show("input", &input);
        let t: f32 = std::env::var("T").ok().and_then(|v| v.parse().ok()).unwrap_or(-28.0);
        for (name, smart, ratio, knee, auto) in [("vocal, makeup off", Smart::Vocal, 4.0, 6.0, false), ("vocal, auto makeup", Smart::Vocal, 4.0, 6.0, true), ("off 4:1, auto makeup", Smart::Off, 4.0, 6.0, true)] {
            let mut stages = [Stage::default(); STAGES];
            stages[0] = Stage { on: true, threshold_db: t, ratio, knee_db: knee, attack_ms: 10.0, release_ms: 120.0, ..Stage::default() };
            let s = CompSettings { stages, smart, auto_makeup: auto, ..CompSettings::default() };
            let mut c = Compressor::new(FS);
            c.prepare(FS, &s);
            let mut y = x.clone();
            for b in y.chunks_mut(512) {
                c.process(&mut [b], None, &s);
            }
            show(name, &last(phrase_db(&y, &tag, n)));
            if auto {
                println!("{:22} auto makeup now {:+.1} dB", "", c.auto_db());
            }
        }
        return;
    }
    let phrases = [(6.0, -30.0), (6.0, -20.0), (6.0, -12.0), (6.0, -26.0), (6.0, -16.0), (6.0, -34.0)];
    let (x, tag) = voice(&phrases);
    let n = phrases.len();
    let input = phrase_db(&x, &tag, n);
    let run = |smart: Smart, threshold: f32, ratio: f32, knee: f32, auto: bool| {
        let mut stages = [Stage::default(); STAGES];
        stages[0] = Stage { on: true, threshold_db: threshold, ratio, knee_db: knee, attack_ms: 10.0, release_ms: 120.0, ..Stage::default() };
        let s = CompSettings { stages, smart, auto_makeup: auto, ..CompSettings::default() };
        let mut c = Compressor::new(FS);
        c.prepare(FS, &s);
        let mut y = x.clone();
        for b in y.chunks_mut(512) {
            c.process(&mut [b], None, &s);
        }
        phrase_db(&y, &tag, n)
    };
    let spread = |v: &[f64]| v.iter().cloned().fold(f64::MIN, f64::max) - v.iter().cloned().fold(f64::MAX, f64::min);
    let show = |name: &str, v: &[f64]| {
        let d: Vec<String> = v.iter().map(|d| format!("{d:6.1}")).collect();
        println!("{name:28} {}   spread {:5.1} dB", d.join(" "), spread(v));
    };
    show("input", &input);
    for auto in [false, true] {
        println!("auto makeup {auto}");
        for t in [-30.0, -24.0, -18.0] {
            show(&format!("off 4:1 at {t}"), &run(Smart::Off, t, 4.0, 6.0, auto));
            show(&format!("vocal at {t}"), &run(Smart::Vocal, t, 4.0, 6.0, auto));
        }
    }
}
