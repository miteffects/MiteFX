//! The engine's audio path must not allocate or free (BLUEPRINT §5), whatever the settings do.

use assert_no_alloc::assert_no_alloc as process_wrapper;
use mite_core::{HistoryTap, SpectrumTap};
use mite_dsp::dynamics::Quadrant;
use mite_dsp::multiband::{MbSettings, BANDS};
use mite_multiband::dsp::{record, MbEngine};

#[test]
fn engine_process_never_allocates() {
    let sr = 48_000.0;
    let mut s = MbSettings::default();
    let history: HistoryTap<{ record::WIDTH }> = HistoryTap::new(1024);
    let (pre, post) = (SpectrumTap::new(8192), SpectrumTap::new(8192));
    let mut engine = MbEngine::new(sr, &s);
    let (mut l, mut r) = (vec![0.0f32; 256], vec![0.0f32; 256]);
    let key = vec![0.3f32; 256];
    for i in 0..1600 {
        // Bands appearing and merging, every quadrant, solo, mute, bypass, external keys,
        // lookahead changes (each behind its dip), mix, delta, bypass.
        s.crossovers = [((i / 40) % 2 == 0).then_some(150.0), ((i / 70) % 3 != 0).then_some(1200.0), ((i / 110) % 2 == 1).then_some(4000.0), ((i / 90) % 4 == 3).then_some(60.0), ((i / 130) % 2 == 1).then_some(12_000.0)];
        for b in 0..BANDS {
            s.bands[b].stage.on = true;
            s.bands[b].stage.quadrant = Quadrant::from_index((i / 25 + b) % 4);
            s.bands[b].stage.threshold_db = -30.0;
            s.bands[b].external = (i / 60 + b) % 3 == 0;
        }
        s.bands[1].solo = (i / 150) % 3 == 2;
        s.bands[2].mute = (i / 60) % 2 == 1;
        s.bands[3].bypass = (i / 80) % 2 == 1;
        s.lookahead_ms = [0.0, 3.0, 20.0][(i / 200) % 3];
        s.mix = if (i / 75) % 2 == 0 { 1.0 } else { 0.5 };
        s.delta = (i / 100) % 3 == 2;
        s.bypass = (i / 210) % 4 == 3;
        for (k, (a, b)) in l.iter_mut().zip(r.iter_mut()).enumerate() {
            let x = (((i * 256 + k) as f32) * 0.05).sin() * if (i / 7) % 2 == 0 { 0.5 } else { 0.001 };
            *a = x;
            *b = -x * 0.7;
        }
        let block = if i % 3 == 0 { 256 } else { 100 };
        let k: [&[f32]; 1] = [&key[..block]];
        process_wrapper(|| engine.process(&mut [&mut l[..block], &mut r[..block]], Some(&k), &s, &history, Some([&pre, &post])));
        if i % 50 == 0 {
            process_wrapper(|| engine.process(&mut [&mut l[..block]], None, &s, &history, None));
        }
        assert!(l.iter().chain(&r).all(|x| x.is_finite()));
    }
    assert!(history.count() > 0);
}
