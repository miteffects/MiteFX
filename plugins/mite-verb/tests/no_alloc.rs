//! The engine's audio path must not allocate or free (BLUEPRINT §5), whatever the settings do.

use assert_no_alloc::assert_no_alloc as process_wrapper;
use mite_core::{HistoryTap, SpectrumTap};
use mite_dsp::reverb::ReverbSettings;
use mite_verb::dsp::{record, Output, VerbEngine};

#[test]
fn engine_process_never_allocates() {
    let sr = 48_000.0;
    let mut s = ReverbSettings::default();
    let history: HistoryTap<{ record::WIDTH }> = HistoryTap::new(1024);
    let mut engine = VerbEngine::new(sr, &s, &Output::default());
    let (mut l, mut r) = (vec![0.0f32; 256], vec![0.0f32; 256]);
    let taps = [SpectrumTap::new(8192), SpectrumTap::new(8192)];
    for i in 0..1600 {
        s.size = ((i / 40) % 5) as f32 / 4.0;
        s.predelay_ms = [0.0, 30.0, 500.0][(i / 70) % 3];
        s.decay_s = [0.1, 2.0, 30.0][(i / 90) % 3];
        s.freeze = (i / 110) % 3 == 2;
        s.lo_mult = [0.25, 1.0, 4.0][(i / 50) % 3];
        s.hi_freq = 1000.0 + (i % 7) as f32 * 2000.0;
        s.duck_db = if (i / 130) % 2 == 0 { 0.0 } else { 12.0 };
        s.mod_depth = ((i / 60) % 2) as f32;
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
