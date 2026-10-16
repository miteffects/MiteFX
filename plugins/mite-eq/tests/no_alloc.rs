//! The engine's audio path must not allocate or free (BLUEPRINT §5), in any mode, through kernel
//! swaps and mode switches. Each block runs inside nih-plug's own process guard
//! (`assert_no_alloc`, active in debug builds), which aborts on any allocation on that thread;
//! the kernel designer allocates on its own thread, as it should.

use std::time::Duration;

use mite_dsp::eq::{BandSettings, BandType};
use mite_eq::dsp::{EqEngine, PhaseMode};
use mite_eq::params::BANDS;
use assert_no_alloc::assert_no_alloc as process_wrapper;

#[test]
fn engine_process_never_allocates_in_any_mode() {
    let sr = 48_000.0;
    let mut settings = [BandSettings::default(); BANDS];
    settings[0] = BandSettings { enabled: true, kind: BandType::Bell, freq: 1000.0, gain_db: 6.0, q: 1.0, ..Default::default() };
    settings[1] = BandSettings { enabled: true, kind: BandType::HighShelf, freq: 8000.0, gain_db: -3.0, q: 0.7, ..Default::default() };
    settings[1].dynamics.enabled = true;
    // A relative spectral band that comes and goes, so the spectral stage switches on and off.
    settings[2] = BandSettings { enabled: true, kind: BandType::Bell, freq: 3000.0, q: 0.5, ..Default::default() };
    settings[2].dynamics = mite_dsp::eq::Dynamics { enabled: true, spectral: true, relative: true, ..Default::default() };
    let mut eq = EqEngine::new(sr, &settings, 0.0);
    let (mut l, mut r) = (vec![0.1f32; 128], vec![-0.1f32; 128]);
    let mut blocks = 0;
    for (m, (mode, res)) in [
        (PhaseMode::LinearPhase, 2),
        (PhaseMode::NaturalPhase, 2),
        (PhaseMode::LinearPhase, 0),
        (PhaseMode::ZeroLatency, 0),
        (PhaseMode::LinearPhase, 3),
    ]
    .into_iter()
    .enumerate()
    {
        eq.set_mode(mode, res);
        for i in 0..600 {
            // Keep the bands moving so kernels are redesigned and swapped all along.
            settings[0].gain_db = 6.0 * ((i / 40) % 2) as f64;
            settings[2].enabled = (i / 150) % 2 == 0;
            // The shelf's dynamics go spectral in every other mode.
            settings[1].dynamics.spectral = m % 2 == 1;
            // Fresh input every block (feeding the output back in would stack the EQ on itself).
            l.fill(0.1);
            r.fill(-0.1);
            process_wrapper(|| eq.process(&mut [&mut l, &mut r], &settings, 0.0, None, None));
            blocks += 1;
            std::thread::sleep(Duration::from_micros(200));
        }
    }
    assert_eq!(blocks, 3000);
    // It ends with the relative band off (blocks 450–599), so no spectral latency.
    assert_eq!(eq.latency(), mite_eq::dsp::linear_phase_latency(3, f64::from(sr)));
}

/// Proves the guard is live in this build: allocating inside it must abort the process. Run it
/// by hand (`cargo test -p mite_eq --test no_alloc -- --ignored`); it is expected to crash.
#[test]
#[ignore]
fn the_guard_catches_an_allocation() {
    process_wrapper(|| std::hint::black_box(Vec::<u8>::with_capacity(64)));
}
