//! The engine's audio path must not allocate or free (BLUEPRINT §5), whatever the settings do:
//! every block runs inside nih-plug's process guard (`assert_no_alloc`, active in debug builds),
//! which aborts on any allocation on that thread.

use assert_no_alloc::assert_no_alloc as process_wrapper;
use mite_core::{HistoryTap, SpectrumTap};
use mite_dsp::shape::ShapeSettings;
use mite_shape::dsp::{record, Output, ShapeEngine};

#[test]
fn engine_process_never_allocates() {
    let sr = 48_000.0;
    let mut s = ShapeSettings::default();
    s.bands[0].attack_db = 6.0;
    s.bands[0].sustain_db = -6.0;
    let history = HistoryTap::new(1024);
    let spectrum = SpectrumTap::new(8192);
    let mut engine = ShapeEngine::new(sr, &s);
    let (mut l, mut r) = (vec![0.0f32; 256], vec![0.0f32; 256]);
    for i in 0..2400 {
        // Everything switching all along: bands made and removed, solo and mute, stereo modes,
        // link, lookahead, the detector filter, Listen, and the output stage.
        s.crossovers = [
            ((i / 40) % 2 == 0).then_some(150.0),
            ((i / 70) % 3 != 0).then_some(1000.0 + (i % 9) as f32 * 50.0),
            ((i / 110) % 2 == 1).then_some(4000.0),
            ((i / 90) % 4 == 3).then_some(60.0),
        ];
        s.bands[2].sustain_db = if (i / 30) % 2 == 0 { 4.0 } else { -8.0 };
        s.bands[1].solo = (i / 150) % 3 == 2;
        s.bands[3].mute = (i / 60) % 2 == 1;
        s.mid_side = (i / 120) % 2 == 1;
        s.link = if (i / 50) % 2 == 0 { 1.0 } else { 0.3 };
        s.lookahead_ms = [0.0, 2.0, 5.0][(i / 130) % 3];
        s.detector_hp = if (i / 80) % 2 == 0 { 20.0 } else { 120.0 };
        s.detector_lp = if (i / 100) % 2 == 0 { 20_000.0 } else { 5000.0 };
        s.listen = (i / 170) % 5 == 4;
        let out = Output { mix: if (i / 80) % 2 == 0 { 1.0 } else { 0.5 }, delta: (i / 100) % 2 == 1, bypass: (i / 210) % 4 == 3, auto_gain: (i / 140) % 2 == 0, gain_db: -3.0, pan: 0.2 };
        for (k, (a, b)) in l.iter_mut().zip(r.iter_mut()).enumerate() {
            let x = (((i * 256 + k) as f32) * 0.05).sin() * if (i / 7) % 2 == 0 { 0.5 } else { 0.01 };
            *a = x;
            *b = -x * 0.7;
        }
        let block = if i % 3 == 0 { 256 } else { 100 };
        process_wrapper(|| {
            engine.process(&mut [&mut l[..block], &mut r[..block]], &s, &out, &history, Some([&spectrum, &spectrum]));
        });
        if i % 50 == 0 {
            process_wrapper(|| engine.process(&mut [&mut l[..block]], &s, &out, &history, Some([&spectrum, &spectrum])));
        }
        assert!(l.iter().chain(&r).all(|x| x.is_finite()));
    }
    assert!(history.count() > 0);
    let _ = record::WIDTH;
}
