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

fn main() {
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
