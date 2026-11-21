//! The phase page against chains with known answers: the real engine (the sidechain carries the
//! signal before the chain), the editor's measurement, settled.

use std::sync::Arc;

use mite_analyzer::dsp::AnalyzerEngine;
use mite_analyzer::editor::phase::Status;
use mite_analyzer::editor::{memory_host, AnalyzerView};
use mite_analyzer::params::PAGE;
use mite_analyzer::{AnalyzerShared, SOURCE_SIDECHAIN};
use mite_ui::View;

fn noise(n: usize, seed: u32) -> Vec<f32> {
    let mut s = seed;
    (0..n)
        .map(|_| {
            s ^= s << 13;
            s ^= s >> 17;
            s ^= s << 5;
            0.3 * ((s as f32 / u32::MAX as f32) * 2.0 - 1.0)
        })
        .collect()
}

/// `seconds` of noise through `chain` (per channel), the dry noise on the sidechain; the editor
/// on the phase page, settled.
fn measure(seconds: f32, chain: impl Fn(&[f32]) -> Vec<f32>) -> AnalyzerView {
    let fs = 48_000.0;
    let shared = Arc::new(AnalyzerShared::new());
    shared.sample_rate.store(fs);
    shared.phase_source.store(SOURCE_SIDECHAIN, std::sync::atomic::Ordering::Relaxed);
    let n = (seconds * fs) as usize;
    let dry = [noise(n, 3), noise(n, 5)];
    let mut wet = [chain(&dry[0]), chain(&dry[1])];
    let mut engine = AnalyzerEngine::new(fs);
    let [wl, wr] = &mut wet;
    for (i, (a, b)) in wl.chunks_mut(512).zip(wr.chunks_mut(512)).enumerate() {
        let sc: [&[f32]; 2] = [&dry[0][i * 512..i * 512 + a.len()], &dry[1][i * 512..i * 512 + a.len()]];
        engine.process_with(&[a, b], Some(&sc), &shared.tel);
    }
    let host = memory_host();
    host.set_plain(PAGE.id, 1.0);
    let dir = std::env::temp_dir().join(format!("mitefx-analyzer-phase-{}", std::process::id()));
    let mut view = AnalyzerView::new(shared).with_preset_dir(dir);
    view.tick(&host);
    view.settle(&host);
    view
}

#[test]
fn a_delayed_inverted_chain_reads_its_delay_polarity_and_flat_magnitude() {
    let view = measure(5.0, |x| (0..x.len()).map(|i| if i >= 256 { -x[i - 256] } else { 0.0 }).collect());
    let lab = view.phase();
    assert_eq!(lab.status(), Status::Measuring);
    let d = lab.delay().expect("a delay");
    assert!((d - 256.0).abs() < 0.05, "delay {d}");
    assert!(lab.inverted());
    for hz in [40.0, 100.0, 1000.0, 10_000.0] {
        let (db, deg, gd_ms, coh) = lab.at(hz);
        assert!(db.abs() < 0.1, "{hz} Hz: {db} dB");
        assert!((deg.abs() - 180.0).abs() < 1.0, "{hz} Hz: {deg}°");
        assert!((gd_ms - 256.0 / 48.0).abs() < 0.02, "{hz} Hz: group delay {gd_ms} ms");
        assert!(coh > 0.98, "{hz} Hz: coherence {coh}");
    }
}

#[test]
fn a_one_pole_lowpass_reads_minus_45_degrees_at_its_corner() {
    // y += a·(x − y): a one-pole low-pass, corner fc where a = 1 − e^(−2π fc / fs).
    let fc = 1000.0f64;
    let a = 1.0 - (-std::f64::consts::TAU * fc / 48_000.0).exp();
    let view = measure(5.0, |x| {
        let mut y = 0.0f64;
        x.iter()
            .map(|&v| {
                y += a * (f64::from(v) - y);
                y as f32
            })
            .collect()
    });
    let lab = view.phase();
    let d = lab.delay().expect("a delay");
    // The impulse peaks at once: no bulk delay.
    assert!(d.abs() < 1.0, "delay {d}");
    // Relative to that delay, the exact phase of this filter at 1 kHz (−44.5° plus the delay's
    // share), magnitude −3 dB.
    let w = std::f64::consts::TAU * fc / 48_000.0;
    let h = realfft::num_complex::Complex::new(a, 0.0) / (realfft::num_complex::Complex::new(1.0, 0.0) - realfft::num_complex::Complex::from_polar(1.0 - a, -w));
    let want = (h.arg() + w * d).to_degrees();
    let (db, deg, _, _) = lab.at(fc);
    assert!((deg - want).abs() < 1.5, "phase {deg}° against {want}°");
    assert!((db - 20.0 * h.norm().log10()).abs() < 0.2, "{db} dB");
}

#[test]
fn nothing_on_the_sidechain_is_no_measurement() {
    let view = measure(3.0, |_| vec![0.0; 3 * 48_000]);
    // The sidechain plays but the measured signal is silent: nothing lines up.
    assert_ne!(view.phase().status(), Status::Measuring);
    assert!(view.phase().delay().is_none());
}
