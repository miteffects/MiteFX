//! The engine's audio path must not allocate or free (BLUEPRINT §5), whatever the settings do.

use assert_no_alloc::assert_no_alloc as process_wrapper;
use mite_core::{HistoryTap, SpectrumTap};
use mite_dsp::gate::{Detector, GateSettings, Mode};
use mite_gate::dsp::{record, GateEngine, Output};

#[test]
fn engine_process_never_allocates() {
    let sr = 48_000.0;
    let mut s = GateSettings::default();
    let history: HistoryTap<{ record::WIDTH }> = HistoryTap::new(1024);
    let mut engine = GateEngine::new(sr, &s, &Output::default());
    let (mut l, mut r) = (vec![0.0f32; 256], vec![0.0f32; 256]);
    let key = vec![0.3f32; 256];
    let tap = SpectrumTap::new(8192);
    for i in 0..1600 {
        s.mode = Mode::ALL[(i / 40) % 3];
        s.detector = if (i / 70) % 2 == 0 { Detector::Peak } else { Detector::Rms };
        s.lookahead_ms = [0.0, 2.0, 10.0][(i / 90) % 3];
        s.external_key = (i / 110) % 3 == 2;
        s.key_bands[0].enabled = (i / 50) % 2 == 0;
        s.key_bands[1].freq = 500.0 + (i % 7) as f64 * 100.0;
        s.key_bands[1].enabled = true;
        s.key_bands[5].enabled = (i / 45) % 2 == 1;
        s.key_bands[5].slope = (i / 20) % 9;
        s.listen = (i / 130) % 5 == 4;
        s.link = if (i / 60) % 2 == 0 { 1.0 } else { 0.0 };
        let out = Output { mix: if (i / 80) % 2 == 0 { 1.0 } else { 0.5 }, delta: (i / 100) % 3 == 2, bypass: (i / 150) % 4 == 3, gain_db: -2.0 };
        for (k, (a, b)) in l.iter_mut().zip(r.iter_mut()).enumerate() {
            let x = (((i * 256 + k) as f32) * 0.05).sin() * if (i / 7) % 2 == 0 { 0.5 } else { 0.001 };
            *a = x;
            *b = -x * 0.7;
        }
        let block = if i % 3 == 0 { 256 } else { 100 };
        let k: [&[f32]; 1] = [&key[..block]];
        process_wrapper(|| engine.process(&mut [&mut l[..block], &mut r[..block]], Some(&k), &s, &out, &history, Some(&tap)));
        if i % 50 == 0 {
            process_wrapper(|| engine.process(&mut [&mut l[..block]], None, &s, &out, &history, Some(&tap)));
        }
        assert!(l.iter().chain(&r).all(|x| x.is_finite()));
    }
    assert!(history.count() > 0);
}
