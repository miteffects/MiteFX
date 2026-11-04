//! The engine's audio path must not allocate or free (BLUEPRINT §5), whatever the settings do.

use assert_no_alloc::assert_no_alloc as process_wrapper;
use mite_core::{HistoryTap, SpectrumTap};
use mite_delay::dsp::{record, DelayEngine, Output};
use mite_dsp::delay::{DelaySettings, Mode, TapSettings};
use mite_dsp::eq::{BandSettings, BandType};

#[test]
fn engine_process_never_allocates() {
    let sr = 48_000.0;
    let mut s = DelaySettings::default();
    let history: HistoryTap<{ record::WIDTH }> = HistoryTap::new(1024);
    let mut engine = DelayEngine::new(sr, &s, &Output::default());
    let (mut l, mut r) = (vec![0.0f32; 256], vec![0.0f32; 256]);
    let taps = [SpectrumTap::new(8192), SpectrumTap::new(8192)];
    for i in 0..1600 {
        s.mode = Mode::from_index((i / 40) % 3);
        s.taps[0].time_ms = [10.0, 250.0, 5000.0][(i / 70) % 3];
        s.taps[0].pingpong = (i / 90) % 2 == 1;
        s.taps[1] = TapSettings { on: (i / 65) % 2 == 0, time_ms: 100.0 + (i % 9) as f32 * 40.0, feedback: 0.9, pan: -0.5, ..TapSettings::default() };
        s.taps[2].on = (i / 110) % 3 == 2;
        s.eq[0] = BandSettings { enabled: (i / 55) % 3 != 0, kind: BandType::from_index((i / 33) % 10), freq: 200.0 + (i % 9) as f64 * 900.0, gain_db: 6.0, slope: (i / 20) % 9, ..BandSettings::default() };
        s.duck_db = if (i / 130) % 2 == 0 { 0.0 } else { 12.0 };
        s.mod_depth = ((i / 60) % 2) as f32;
        s.diffusion = ((i / 45) % 2) as f32;
        s.drive = ((i / 75) % 2) as f32;
        let out = Output { mix: if (i / 80) % 2 == 0 { 1.0 } else { 0.3 }, bypass: (i / 150) % 4 == 3, gain_db: -2.0 };
        for (k, (a, b)) in l.iter_mut().zip(r.iter_mut()).enumerate() {
            let x = (((i * 256 + k) as f32) * 0.05).sin() * if (i / 7) % 2 == 0 { 0.5 } else { 0.001 };
            *a = x;
            *b = -x * 0.7;
        }
        let block = if i % 3 == 0 { 256 } else { 100 };
        process_wrapper(|| engine.process(&mut [&mut l[..block], &mut r[..block]], &s, &out, &history, Some([&taps[0], &taps[1]])));
        if i % 50 == 0 {
            process_wrapper(|| engine.process(&mut [&mut l[..block]], &s, &out, &history, None));
        }
        assert!(l.iter().chain(&r).all(|x| x.is_finite()));
    }
    assert!(history.count() > 0);
}
