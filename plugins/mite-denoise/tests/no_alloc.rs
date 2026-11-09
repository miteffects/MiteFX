//! The engine's audio path must not allocate or free (BLUEPRINT §5), whatever the settings and
//! the editor do: learning on and off, a profile loaded mid-stream, mono blocks, odd sizes.

use std::sync::atomic::Ordering;

use assert_no_alloc::assert_no_alloc as process_wrapper;
use mite_denoise::dsp::{DenoiseEngine, DisplayTap};
use mite_denoise::params::NoiseProfile;
use mite_dsp::denoise::{DenoiseSettings, PROFILE_POINTS};

#[test]
fn engine_process_never_allocates() {
    let mut s = DenoiseSettings::default();
    let tap = DisplayTap::new();
    let profile = NoiseProfile::new();
    let mut engine = DenoiseEngine::new(48_000.0);
    let (mut l, mut r) = (vec![0.0f32; 1500], vec![0.0f32; 1500]);
    let mut seed = 3u32;
    for i in 0..600 {
        s.adaptive = (i / 100) % 2 == 0;
        s.listen = (i / 70) % 4 == 3;
        s.bypass = (i / 90) % 5 == 4;
        s.smoothing = [0.0, 0.4, 1.0][(i / 40) % 3];
        s.reduction_db = [0.0, 12.0, 60.0][(i / 50) % 3];
        tap.learn.store((i / 120) % 2 == 1, Ordering::Relaxed);
        if i == 333 {
            profile.store(&[-100.0; PROFILE_POINTS]);
        }
        for (a, b) in l.iter_mut().zip(r.iter_mut()) {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            *a = (seed as f32 / u32::MAX as f32 - 0.5) * 0.1;
            *b = -*a;
        }
        let block = [1500, 64, 513][i % 3];
        process_wrapper(|| engine.process(&mut [&mut l[..block], &mut r[..block]], &s, -3.0, &profile, &tap));
        if i % 50 == 0 {
            process_wrapper(|| engine.process(&mut [&mut l[..block]], &s, 0.0, &profile, &tap));
        }
        assert!(l.iter().chain(&r).all(|x| x.is_finite()));
    }
    assert!(profile.is_learned());
}
