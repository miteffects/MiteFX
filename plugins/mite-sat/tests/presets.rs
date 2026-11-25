//! Every factory preset through the real engine: finite, and its level within reach of the input
//! (auto gain is on in all of them; dynamics and band output are deliberate level changes, so the
//! bound is loose: a preset that blows up or goes silent fails).

use mite_core::HistoryTap;
use mite_sat::dsp::{record, Output, SatEngine};
use mite_sat::editor::{memory_host, oversampling, settings};
use mite_sat::presets::factory;

/// Two seconds of stereo: a decaying 60 Hz hit every half second over a chord and some noise.
fn material(fs: f32) -> [Vec<f32>; 2] {
    let n = 2 * fs as usize;
    let mut seed = 1u32;
    let mut noise = move || {
        seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        (seed >> 8) as f32 / (1u32 << 24) as f32 - 0.5
    };
    std::array::from_fn(|c| {
        (0..n)
            .map(|i| {
                let t = i as f64 / f64::from(fs);
                let hit = t % 0.5;
                let kick = 0.5 * (-hit / 0.08).exp() * (std::f64::consts::TAU * 60.0 * hit).sin();
                let chord: f64 = [220.0, 277.2, 329.6, 1760.0].iter().map(|f| 0.05 * (std::f64::consts::TAU * f * t + c as f64).sin()).sum();
                (kick + chord) as f32 + 0.02 * noise()
            })
            .collect()
    })
}

#[test]
fn every_factory_preset_plays_finite_and_near_the_input_level() {
    let fs = 48_000.0;
    let x = material(fs);
    let rms = |v: &[f32]| (v.iter().map(|a| f64::from(*a).powi(2)).sum::<f64>() / v.len() as f64).sqrt();
    let level_in = rms(&x[0]);
    for preset in factory() {
        let host = memory_host();
        for (id, v) in &preset.values {
            host.set_plain(id, *v);
        }
        let s = settings(&host);
        let factor = oversampling(&host);
        let out = Output::default();
        let mut engine = SatEngine::new(fs, factor, &s);
        let history = HistoryTap::<{ record::WIDTH }>::new(64);
        let mut y = x.clone();
        let [l, r] = &mut y;
        for (a, b) in l.chunks_mut(512).zip(r.chunks_mut(512)) {
            engine.process(&mut [a, b], &s, factor, false, &out, &mite_dsp::tape::MachineSettings::default(), &history, None);
        }
        assert!(y.iter().flatten().all(|v| v.is_finite()), "{}", preset.name);
        let tail = fs as usize..;
        let db = 20.0 * (rms(&y[0][tail]) / level_in).log10();
        println!("{:<14} {db:+.1} dB", preset.name);
        if std::env::var_os("PRESET_LEVELS").is_none() { assert!(db.abs() < 6.0, "{}: {db:.1} dB against the input", preset.name); }
    }
}
