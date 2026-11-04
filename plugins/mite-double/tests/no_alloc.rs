//! The engine's audio path must not allocate or free (BLUEPRINT §5), whatever the settings do.

use assert_no_alloc::assert_no_alloc as process_wrapper;
use mite_core::HistoryTap;
use mite_double::dsp::{record, DoubleEngine, Output};
use mite_dsp::doubler::DoublerSettings;

#[test]
fn engine_process_never_allocates() {
    let history: HistoryTap<{ record::WIDTH }> = HistoryTap::new(512);
    let mut s = DoublerSettings::default();
    let mut engine = DoubleEngine::new(48_000.0, &Output::default());
    let (mut l, mut r) = (vec![0.0f32; 512], vec![0.0f32; 512]);
    for i in 0..1200 {
        s.voices = (i / 40) % 4;
        s.follow_notes = (i / 90) % 3 != 2;
        s.delay_ms = [8.0, 20.0, 60.0][(i / 70) % 3];
        s.timing_ms = [0.0, 14.0, 40.0][(i / 50) % 3];
        s.pitch_cents = [0.0, 7.0, 25.0][(i / 60) % 3];
        s.chorus = ((i / 30) % 2) as f32;
        s.tone_db = [-6.0, 0.0, 6.0][(i / 45) % 3];
        let out = Output { bypass: (i / 150) % 4 == 3, gain_db: -1.0, auto_gain: i % 2 == 0 };
        for (k, v) in l.iter_mut().enumerate() {
            // Plucks: a burst every 12 blocks.
            let t = (i % 12) * 512 + k;
            *v = (t as f32 * 0.03).sin() * (-(t as f32) / 4000.0).exp() * 0.5;
        }
        let n = if i % 3 == 0 { 512 } else { 100 };
        process_wrapper(|| engine.process(&mut [&mut l[..n], &mut r[..n]], i % 2 == 0, &s, &out, &history));
        assert!(l.iter().chain(&r).all(|x| x.is_finite()));
    }
    assert!(history.count() > 0);
}
