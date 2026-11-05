//! The engine's audio path must not allocate or free (BLUEPRINT §5), whatever the settings do.

use assert_no_alloc::assert_no_alloc as process_wrapper;
use mite_core::HistoryTap;
use mite_dsp::modulation::{Mode, ModSettings, Shape, Transport, STAGE_COUNTS};
use mite_mod::dsp::{record, ModEngine, Output};

#[test]
fn engine_process_never_allocates() {
    let history: HistoryTap<{ record::WIDTH }> = HistoryTap::new(512);
    let mut s = ModSettings::default();
    let mut engine = ModEngine::new(48_000.0, &s, &Output::default());
    let (mut l, mut r) = (vec![0.0f32; 512], vec![0.0f32; 512]);
    for i in 0..1500 {
        s.mode = Mode::from_index((i / 40) % 5);
        s.thru_zero = (i / 90) % 2 == 0;
        s.shape = Shape::from_index((i / 33) % 6);
        s.voices = 1 + (i / 25) % 4;
        s.stages = STAGE_COUNTS[(i / 27) % STAGE_COUNTS.len()];
        s.sync = (i / 70) % 2 == 0;
        s.feedback = [-0.9, 0.0, 0.9][(i / 45) % 3];
        s.low_cut_hz = [20.0, 500.0][(i / 60) % 2];
        let out = Output { bypass: (i / 150) % 4 == 3, gain_db: -1.0 };
        let t = Transport { tempo: Some(120.0), beats: Some(i as f64 * 0.02), playing: i % 3 != 0 };
        for (k, v) in l.iter_mut().enumerate() {
            *v = ((i * 512 + k) as f32 * 0.03).sin() * 0.5;
        }
        let n = if i % 3 == 0 { 512 } else { 100 };
        process_wrapper(|| engine.process(&mut [&mut l[..n], &mut r[..n]], i % 2 == 0, &s, &out, &t, &history));
        assert!(l.iter().chain(&r).all(|x| x.is_finite()));
    }
    assert!(history.count() > 0);
}
