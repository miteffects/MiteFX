//! The audio path must not allocate or free (BLUEPRINT §5).

use assert_no_alloc::assert_no_alloc as process_wrapper;
use mite_core::{HistoryTap, SpectrumTap};
use mite_tuner::dsp::{record, TunerEngine};

#[test]
fn engine_process_never_allocates() {
    let history: HistoryTap<{ record::WIDTH }> = HistoryTap::new(256);
    let tap = SpectrumTap::new(16_384);
    let mut engine = TunerEngine::new(48_000.0, false);
    let (mut l, mut r) = (vec![0.1f32; 512], vec![-0.1f32; 512]);
    for i in 0..400 {
        let mute = (i / 30) % 2 == 1;
        let n = if i % 3 == 0 { 512 } else { 77 };
        process_wrapper(|| engine.process(&mut [&mut l[..n], &mut r[..n]], mute, &history, Some(&tap)));
        process_wrapper(|| engine.process(&mut [&mut l[..n]], mute, &history, None));
    }
    assert!(history.count() > 0);
}
