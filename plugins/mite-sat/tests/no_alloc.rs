//! The engine's audio path must not allocate or free (BLUEPRINT §5), whatever the settings do:
//! every block runs inside nih-plug's process guard (`assert_no_alloc`, active in debug builds),
//! which aborts on any allocation on that thread.

use assert_no_alloc::assert_no_alloc as process_wrapper;
use mite_core::{HistoryTap, SpectrumTap};
use mite_dsp::sat::{Model, SatSettings};
use mite_sat::dsp::{record, Output, SatEngine};
use mite_core::modmatrix::MatrixTap;
use mite_dsp::modmatrix::{MatrixSettings, Slot, Source};
use mite_plugin::matrix::MatrixRunner;

#[test]
fn engine_process_never_allocates() {
    let sr = 48_000.0;
    let mut s = SatSettings::default();
    s.bands[0].drive_db = 12.0;
    let history = HistoryTap::new(1024);
    let spectrum = SpectrumTap::new(8192);
    let mut engine = SatEngine::new(sr, 2, &s);
    let (mut l, mut r) = (vec![0.0f32; 256], vec![0.0f32; 256]);
    let mut runner = MatrixRunner::default();
    runner.prepare(sr, 256);
    let tap = MatrixTap::new();
    for i in 0..2400 {
        // Everything switching all along: bands made and removed, models, drive to and from 0
        // (bands waking up), bypass, solo and mute, oversampling, auto gain and the output stage.
        s.crossovers = [
            ((i / 40) % 2 == 0).then_some(150.0),
            ((i / 70) % 3 != 0).then_some(1000.0 + (i % 9) as f32 * 50.0),
            ((i / 110) % 2 == 1).then_some(4000.0),
            ((i / 90) % 4 == 3).then_some(60.0),
            ((i / 130) % 2 == 1).then_some(12_000.0),
        ];
        for b in 0..6 {
            s.bands[b].model = Model::ALL[(i / 25 + b) % 8];
            s.bands[b].drive_db = if (i / (30 + b)) % 3 == 0 { 0.0 } else { 6.0 + b as f32 * 4.0 };
            s.bands[b].bias = if (i / 45) % 2 == 0 { 0.3 } else { -0.5 };
            s.bands[b].tone_db = if (i / 55) % 2 == 0 { 0.0 } else { 6.0 };
        }
        s.bands[1].bypass = (i / 60) % 2 == 1;
        s.bands[2].solo = (i / 150) % 3 == 2;
        s.bands[3].mute = (i / 60) % 2 == 1;
        s.auto_gain = (i / 140) % 2 == 0;
        let factor = [1, 2, 4, 8][(i / 170) % 4];
        let out = Output { mix: if (i / 80) % 2 == 0 { 1.0 } else { 0.5 }, delta: (i / 100) % 2 == 1, bypass: (i / 210) % 4 == 3, gain_db: -3.0, pan: 0.2 };
        for (k, (a, b)) in l.iter_mut().zip(r.iter_mut()).enumerate() {
            let x = (((i * 256 + k) as f32) * 0.05).sin() * if (i / 7) % 2 == 0 { 0.5 } else { 0.01 };
            *a = x;
            *b = -x * 0.7;
        }
        let block = if i % 3 == 0 { 256 } else { 100 };
        // Through the matrix, routed half the time (a drive, a crossover, the output).
        let mut ms = MatrixSettings::default();
        if (i / 70) % 2 == 1 {
            ms.slots[0] = Slot { source: Source::Lfo1, target: 1, amount: 0.5 };
            ms.slots[1] = Slot { source: Source::Envelope, target: 38, amount: -0.3 };
            ms.slots[2] = Slot { source: Source::Lfo2, target: 43, amount: 0.2 };
        }
        process_wrapper(|| {
            runner.run(&ms, &mite_dsp::modulation::Transport::default(), &mut [&mut l[..block], &mut r[..block]], None, &tap, 44, |sub, _, offsets| {
                if offsets.iter().any(|o| *o != 0.0) {
                    let (mut sm, mut om) = (s, out);
                    mite_sat::params::modulate_settings(&mut sm, &mut om, offsets);
                    engine.process(sub, &sm, factor, &om, &history, Some([&spectrum, &spectrum]));
                } else {
                    engine.process(sub, &s, factor, &out, &history, Some([&spectrum, &spectrum]));
                }
            });
        });
        if i % 50 == 0 {
            process_wrapper(|| engine.process(&mut [&mut l[..block]], &s, factor, &out, &history, Some([&spectrum, &spectrum])));
        }
        assert!(l.iter().chain(&r).all(|x| x.is_finite()));
    }
    assert!(history.count() > 0);
    let _ = record::WIDTH;
}
