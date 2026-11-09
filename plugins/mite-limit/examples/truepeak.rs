//! True-peak compliance (docs/dsp/limit-truepeak.md): a battery of hard signals pushed into a
//! −1 dB ceiling at every oversampling factor, the output measured by the BS.1770-4 meter (4×)
//! and a 32× reference interpolator. Prints the worst of each, in dB over the ceiling.
//!
//!     cargo run --release -p mite_limit --example truepeak

#[path = "common/music.rs"]
mod music;

use mite_core::HistoryTap;
use mite_dsp::loudness::TruePeak;
use mite_dsp::{db_to_gain, gain_to_db};
use mite_limit::dsp::{record, LimitEngine, LimitSettings, LoudnessTap, FACTORS};

const FS: f32 = 48_000.0;
const CEILING: f32 = -1.0;

/// The battery: (name, signal at full scale before the gain).
fn battery() -> Vec<(&'static str, Vec<f32>)> {
    let n = FS as usize * 4;
    let t = |i: usize| i as f32 / FS;
    let tau = std::f32::consts::TAU;
    let mut out = Vec::new();
    for (name, f) in [("sine 0.21 fs", 0.21), ("sine 0.37 fs", 0.37), ("sine 0.45 fs", 0.45), ("sine 0.49 fs", 0.49)] {
        // A quarter-sample phase offset puts the crest between samples.
        out.push((name, (0..n).map(|i| (tau * f * (i as f32 + 0.25)).sin()).collect()));
    }
    // BS.2217-like: a tone at fs/4 with its crest exactly between two samples (+3 dB over the
    // samples), and its bursts.
    out.push(("fs/4 at 45°", (0..n).map(|i| (tau * 0.25 * i as f32 + std::f32::consts::FRAC_PI_4).sin()).collect()));
    out.push(("fs/4 bursts", (0..n).map(|i| if (i / 4800) % 2 == 0 { (tau * 0.25 * i as f32 + std::f32::consts::FRAC_PI_4).sin() } else { 0.0 }).collect()));
    out.push(("square 1 kHz", (0..n).map(|i| if (t(i) * 1000.0).fract() < 0.5 { 0.7 } else { -0.7 }).collect()));
    let mut seed = 0x2545_F491_4F6C_DD1Du64;
    out.push(("clipped noise", (0..n).map(|_| {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        ((seed >> 40) as f32 / (1u64 << 24) as f32 * 4.0 - 2.0).clamp(-1.0, 1.0)
    }).collect()));
    let [l, _] = music::music(4.0, FS);
    out.push(("drums and bass", l));
    out
}

fn worst(s: &LimitSettings, x: &[f32]) -> (f32, f32) {
    let mut e = LimitEngine::new(FS, s);
    let h: HistoryTap<{ record::WIDTH }> = HistoryTap::new(64);
    let tap = LoudnessTap::new();
    let (mut l, mut r) = (x.to_vec(), x.iter().map(|v| -v).collect::<Vec<_>>());
    for (a, b) in l.chunks_mut(512).zip(r.chunks_mut(512)) {
        e.process(&mut [a, b], s, &h, &tap);
    }
    // The meters run over the whole output, their readings counted past the first 100 ms: a
    // slice cut out of the middle would start with a step from silence, and its overshoot is the
    // cut's, not the limiter's.
    let peak = |phases: usize| {
        let mut m = TruePeak::new(phases);
        l.iter().enumerate().map(|(i, &v)| (i, m.push(v))).filter(|&(i, _)| i >= 4800).fold(0.0f32, |a, (_, p)| a.max(p))
    };
    (gain_to_db(peak(4)) - CEILING, gain_to_db(peak(32)) - CEILING)
}

fn main() {
    let signals = battery();
    println!("dB over the {CEILING} dB ceiling, worst of the battery ({} signals), +20 dB of gain", signals.len());
    println!("{:<34} {:>10} {:>10}", "", "BS.1770", "32× ref");
    let mut rows: Vec<(String, LimitSettings)> = Vec::new();
    for f in FACTORS {
        rows.push((format!("{f:>2}×, true peak on"), LimitSettings { oversampling: f, ..LimitSettings::default() }));
    }
    rows.push((" 2×, true peak off".into(), LimitSettings { oversampling: 2, true_peak: false, ..LimitSettings::default() }));
    rows.push((" 2×, clipper Hard at the ceiling".into(), LimitSettings { clip_on: true, clip_db: 0.0, clip_curve: 0, ..LimitSettings::default() }));
    rows.push((" 2×, protect 100 %".into(), LimitSettings { protect: 1.0, ..LimitSettings::default() }));
    for (name, base) in rows {
        let s = LimitSettings { gain_db: 20.0, ceiling_db: CEILING, ..base };
        let (mut meter, mut reference) = (f32::MIN, f32::MIN);
        let mut which = "";
        for (sig, x) in &signals {
            let x: Vec<f32> = x.iter().map(|v| v * db_to_gain(-20.0)).collect();
            let (m, r) = worst(&s, &x);
            if r > reference {
                which = sig;
            }
            meter = meter.max(m);
            reference = reference.max(r);
        }
        println!("{name:<34} {meter:>+10.3} {reference:>+10.3}   (worst: {which})");
    }
}
