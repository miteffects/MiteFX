//! The engine's audio path must not allocate or free (BLUEPRINT §5): stereo and mono blocks, odd
//! sizes, resets of the integration mid-stream.

use std::sync::atomic::Ordering;

use assert_no_alloc::assert_no_alloc as process_wrapper;
use mite_analyzer::dsp::{AnalyzerEngine, Telemetry};

#[test]
fn engine_process_never_allocates() {
    let tel = Telemetry::new();
    let mut engine = AnalyzerEngine::new(48_000.0);
    let (mut l, mut r) = (vec![0.0f32; 1500], vec![0.0f32; 1500]);
    for i in 0..400 {
        for (k, (a, b)) in l.iter_mut().zip(r.iter_mut()).enumerate() {
            *a = ((i * 1500 + k) as f32 * 0.03).sin() * 0.5;
            *b = -*a * 0.8;
        }
        if i % 50 == 0 {
            tel.reset.store(true, Ordering::Relaxed);
        }
        let block = [1500, 1, 333, 512][i % 4];
        if i % 7 == 0 {
            process_wrapper(|| engine.process(&[&mut l[..block]], &tel));
        } else {
            process_wrapper(|| engine.process(&[&mut l[..block], &mut r[..block]], &tel));
        }
    }
    assert!(tel.history.count() > 0);
}
