//! The engine's audio path must not allocate or free (BLUEPRINT §5), whatever the settings do.

use assert_no_alloc::assert_no_alloc as process_wrapper;
use mite_core::{SpectrumTap, StereoMeter};
use mite_dsp::tape::{Speed, Standard};
use mite_tape::dsp::TapeEngine;
use mite_tape::params::TapeSettings;

#[test]
fn engine_process_never_allocates() {
    let taps = [SpectrumTap::new(8192), SpectrumTap::new(8192)];
    let meters = [StereoMeter::new(-150.0), StereoMeter::new(-150.0)];
    let mut s = TapeSettings::default();
    let mut engine = TapeEngine::new(48_000.0, &s);
    let (mut l, mut r) = (vec![0.0f32; 1024], vec![0.0f32; 1024]);
    let mut seed = 5u32;
    for i in 0..600 {
        s.machine.speed = Speed::from_index((i / 60) % 4);
        s.machine.standard = Standard::from_index((i / 90) % 2);
        s.machine.drive_db = [-6.0, 0.0, 18.0][(i / 25) % 3];
        s.machine.wow = [0.0, 0.5, 1.0][(i / 33) % 3];
        s.machine.hiss = [0.0, 1.0][(i / 40) % 2];
        s.mix = [1.0, 0.0, 0.5][(i / 45) % 3];
        s.bypass = (i / 70) % 4 == 3;
        for (a, b) in l.iter_mut().zip(r.iter_mut()) {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            *a = (seed as f32 / u32::MAX as f32 - 0.5) * if (i / 7) % 2 == 0 { 0.8 } else { 0.01 };
            *b = -*a;
        }
        let n = if i % 3 == 0 { 1024 } else { 100 };
        process_wrapper(|| engine.process(&mut [&mut l[..n], &mut r[..n]], &s, Some([&meters[0], &meters[1]]), Some([&taps[0], &taps[1]])));
        if i % 50 == 0 {
            process_wrapper(|| engine.process(&mut [&mut l[..n]], &s, None, None));
        }
        assert!(l.iter().chain(&r).all(|x| x.is_finite()));
    }
}
