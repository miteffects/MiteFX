//! The engine's audio path must not allocate or free (BLUEPRINT §5), whatever the settings do:
//! every block runs inside nih-plug's process guard (`assert_no_alloc`, active in debug builds),
//! which aborts on any allocation on that thread.

use assert_no_alloc::assert_no_alloc as process_wrapper;
use mite_core::HistoryTap;
use mite_limit::dsp::{record, LimitEngine, LimitSettings, LoudnessTap, FACTORS};

#[test]
fn engine_process_never_allocates() {
    let sr = 48_000.0;
    let mut s = LimitSettings { gain_db: 12.0, ..LimitSettings::default() };
    let history: HistoryTap<{ record::WIDTH }> = HistoryTap::new(1024);
    let tap = LoudnessTap::new();
    let mut engine = LimitEngine::new(sr, &s);
    let (mut l, mut r) = (vec![0.0f32; 256], vec![0.0f32; 256]);
    for i in 0..1600 {
        // Everything switching: oversampling, lookahead, true peak (each behind the dip), styles,
        // release, links, ceiling, delta, bypass, the loudness reset, mono blocks, and the
        // clipper, protection, unity, dither and the DC filter.
        s.oversampling = FACTORS[(i / 150) % 5];
        s.lookahead_ms = [0.0, 2.0, 10.0][(i / 70) % 3];
        s.true_peak = (i / 110) % 2 == 0;
        s.style = (i / 30) % 6;
        s.release_ms = if (i / 40) % 2 == 0 { 5.0 } else { 900.0 };
        s.link_transients = if (i / 50) % 2 == 0 { 1.0 } else { 0.0 };
        s.ceiling_db = if (i / 60) % 2 == 0 { -1.0 } else { -6.0 };
        s.delta = (i / 90) % 3 == 2;
        s.bypass = (i / 130) % 4 == 3;
        // The clipper, protection (on and off: behind the dip), unity, dither, the DC filter.
        s.clip_on = (i / 45) % 3 != 0;
        s.clip_curve = (i / 35) % 3;
        s.clip_db = [0.0, 3.0, 12.0][(i / 55) % 3];
        s.protect = [0.0, 0.5, 1.0][(i / 95) % 3];
        s.unity = (i / 65) % 2 == 1;
        s.dither = (i / 75) % 4;
        s.shaping = (i / 85) % 3;
        s.dc_filter = (i / 105) % 2 == 1;
        if i % 200 == 0 {
            tap.reset.store(true, std::sync::atomic::Ordering::Relaxed);
        }
        for (k, (a, b)) in l.iter_mut().zip(r.iter_mut()).enumerate() {
            let x = (((i * 256 + k) as f32) * 0.05).sin() * if (i / 7) % 2 == 0 { 0.9 } else { 0.01 };
            *a = x;
            *b = -x * 0.7;
        }
        let block = if i % 3 == 0 { 256 } else { 100 };
        process_wrapper(|| engine.process(&mut [&mut l[..block], &mut r[..block]], &s, &history, &tap));
        if i % 50 == 0 {
            process_wrapper(|| engine.process(&mut [&mut l[..block]], &s, &history, &tap));
        }
        assert!(l.iter().chain(&r).all(|x| x.is_finite()));
    }
    assert!(history.count() > 0);
}
