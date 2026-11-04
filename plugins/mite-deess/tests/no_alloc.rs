//! The engine's audio path must not allocate or free (BLUEPRINT §5), whatever the settings do.

use assert_no_alloc::assert_no_alloc as process_wrapper;
use mite_core::{HistoryTap, SpectrumTap};
use mite_deess::dsp::{record, DeEssEngine};
use mite_dsp::deess::{DeEssSettings, Detection, Reduce};

#[test]
fn engine_process_never_allocates() {
    let history: HistoryTap<{ record::WIDTH }> = HistoryTap::new(512);
    let taps = [SpectrumTap::new(8192), SpectrumTap::new(8192)];
    let mut s = DeEssSettings::default();
    let mut engine = DeEssEngine::new(48_000.0, &s, 0.0);
    let (mut l, mut r) = (vec![0.0f32; 512], vec![0.0f32; 512]);
    let mut seed = 5u32;
    for i in 0..1200 {
        s.detection = if (i / 70) % 2 == 0 { Detection::Relative } else { Detection::Absolute };
        s.reduce = Reduce::from_index((i / 40) % 3);
        s.lookahead = (i / 90) % 3 != 2;
        s.listen = (i / 130) % 4 == 3;
        s.bypass = (i / 150) % 5 == 4;
        s.lo_hz = [3000.0, 5000.0, 9000.0][(i / 33) % 3];
        s.threshold_db = [-40.0, -15.0, -3.0][(i / 25) % 3];
        for (a, b) in l.iter_mut().zip(r.iter_mut()) {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            *a = (seed as f32 / u32::MAX as f32 - 0.5) * if (i / 7) % 2 == 0 { 0.8 } else { 0.01 };
            *b = -*a;
        }
        let n = if i % 3 == 0 { 512 } else { 100 };
        process_wrapper(|| engine.process(&mut [&mut l[..n], &mut r[..n]], &s, -1.0, &history, Some([&taps[0], &taps[1]])));
        if i % 50 == 0 {
            process_wrapper(|| engine.process(&mut [&mut l[..n]], &s, 0.0, &history, None));
        }
        assert!(l.iter().chain(&r).all(|x| x.is_finite()));
    }
    assert!(history.count() > 0);
}
