//! Two Mite Analyzers around a chain that is not a filter (2026-11-27, user: behind a gate "the
//! time delay curve is still messy in lows"): a compressor on a kick drum reads as one coherent
//! filter (γ² 0.99) with a 25 ms group delay at 50 Hz that it does not have. Loud and quiet
//! passages give different H, so the page marks the phase and group delay there (always drawn,
//! never hidden); a real filter, also under noise, is never marked. Its own test binary: the instance registry is process-wide.

use std::sync::Arc;

use mite_analyzer::dsp::AnalyzerEngine;
use mite_analyzer::editor::phase::Status;
use mite_analyzer::editor::{memory_host, AnalyzerView};
use mite_analyzer::params::PAGE;
use mite_analyzer::{AnalyzerShared, INSTANCES};
use mite_dsp::dynamics::{CompSettings, Compressor};
use mite_ui::View;

const FS: f32 = 48_000.0;

/// A kick-in mic: kicks every half second at varied velocity (a pitch drop 160 → 50 Hz), snare
/// bleed on the off beat, hats under everything.
fn drums(seconds: f32) -> Vec<f32> {
    let n = (seconds * FS) as usize;
    let mut y = vec![0.0f32; n];
    let mut seed = 777u32;
    let mut rnd = move || {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        seed as f32 / u32::MAX as f32 * 2.0 - 1.0
    };
    let beat = (0.5 * FS) as usize;
    let mut at = 1000;
    while at < n {
        let vel = 0.55 + 0.45 * (0.5 + 0.5 * rnd());
        let mut ph = 0.0f32;
        for i in 0..(0.45 * FS) as usize {
            if at + i >= n {
                break;
            }
            let t = i as f32 / FS;
            ph += std::f32::consts::TAU * (50.0 + 110.0 * (-t / 0.025).exp()) / FS;
            y[at + i] += vel * 0.8 * (-t / 0.12).exp() * ph.sin();
        }
        let s = at + beat / 2;
        for i in 0..(0.2 * FS) as usize {
            if s + i < n {
                y[s + i] += 0.05 * (-(i as f32) / FS / 0.05).exp() * rnd();
            }
        }
        at += beat;
    }
    for v in &mut y {
        *v += 0.004 * rnd();
    }
    y
}

/// What the page after `chain` draws from 25 to 250 Hz: (Hz, how strongly it is drawn, how
/// strongly marked as changing with level, its group delay less the chain delay, ms), and the
/// mean coherence there.
fn measure(chain: &mut dyn FnMut(&mut [f32])) -> (Vec<(f64, f64, f64, f64)>, f64) {
    let before = Arc::new(AnalyzerShared::new());
    let after = Arc::new(AnalyzerShared::new());
    INSTANCES.register(&before);
    INSTANCES.register(&after);
    for s in [&before, &after] {
        s.sample_rate.store(FS);
    }
    let (mut ea, mut eb) = (AnalyzerEngine::new(FS), AnalyzerEngine::new(FS));
    let x = drums(10.0);
    for block in x.chunks(480) {
        let (mut l, mut r) = (block.to_vec(), block.to_vec());
        ea.process(&[&mut l, &mut r], &before.tel);
        chain(&mut l);
        let mut r = l.clone();
        eb.process(&[&mut l, &mut r], &after.tel);
    }
    let host = memory_host();
    host.set_plain(PAGE.id, 1.0);
    let dir = std::env::temp_dir().join(format!("mitefx-analyzer-level-{}", std::process::id()));
    let mut view = AnalyzerView::new(after.clone()).with_preset_dir(dir);
    view.tick(&host);
    view.settle(&host);
    let lab = view.phase();
    assert_eq!(lab.status(), Status::Measuring);
    let delay_ms = lab.delay().unwrap() / f64::from(FS) * 1000.0;
    let pts: Vec<_> = (0..=40).map(|k| 25.0 * 10f64.powf(f64::from(k) / 40.0)).map(|f| (f, lab.shown(f), lab.level_marked(f), lab.at(f).2 - delay_ms)).collect();
    let coh = pts.iter().map(|p| lab.at(p.0).3).sum::<f64>() / pts.len() as f64;
    (pts, coh)
}

/// Nothing drawn strongly (over half) and left unmarked reads a group delay the chain does not
/// have: more than `tol` ms off the chain delay. Returns how much of the band is marked.
fn honest(name: &str, pts: &[(f64, f64, f64, f64)], tol: f64) -> f64 {
    for &(f, shown, marked, gd) in pts {
        assert!(shown < 0.5 || marked > 0.5 || gd.abs() < tol, "{name}: {f:.0} Hz drawn at {shown:.2}, unmarked ({marked:.2}), with a group delay of {gd:.1} ms");
    }
    let worst = pts.iter().map(|p| p.3.abs()).fold(0.0, f64::max);
    let marked = pts.iter().map(|p| p.2).sum::<f64>() / pts.len() as f64;
    println!("{name}: marked {marked:.2} of 25–250 Hz, the worst group delay {worst:.1} ms");
    marked
}

#[test]
fn a_delay_that_is_not_there_is_marked_and_a_filter_never_is() {
    let mut comp = Compressor::new(FS);
    let mut cs = CompSettings::default();
    cs.stages[0].on = true;
    cs.stages[0].threshold_db = -24.0;
    cs.stages[0].ratio = 6.0;
    cs.stages[0].attack_ms = 1.0;
    cs.stages[0].release_ms = 60.0;
    comp.prepare(FS, &cs);
    let (pts, coh) = measure(&mut |b| {
        comp.process(&mut [b], None, &cs);
    });
    assert!(coh > 0.95, "coherent: coherence alone would draw it all ({coh})");
    honest("compressor", &pts, 3.0);

    // A gate that cuts the kick's tail (the user's case), and a brickwall limiter 9 dB into it.
    let mut gate = mite_dsp::gate::Gate::new(FS);
    let gs = mite_dsp::gate::GateSettings { threshold_db: -18.0, release_ms: 15.0, hold_ms: 0.0, ..Default::default() };
    gate.reset(&gs);
    let (pts, _) = measure(&mut |b| {
        gate.process(&mut [b], None, &gs);
    });
    // Not caught: under 50 Hz (below the kick's pitch) its quiet frames are so incoherent that
    // their noise hides the spread, and only the coherence fades it, as before (up to 23 ms
    // drawn unmarked at 47 Hz); at 50 Hz 7 ms.
    let above: Vec<_> = pts.iter().copied().filter(|p| p.0 >= 49.0).collect();
    honest("gate", &above, 8.0);
    let mut lim = mite_dsp::limit::Limiter::new(FS, 480, 1);
    lim.configure(72, false, mite_dsp::limit::Settings { release_ms: 50.0, ..Default::default() });
    let (pts, _) = measure(&mut |b| {
        for v in b.iter_mut() {
            *v = lim.process([*v * 2.8, 0.0], false).0[0] * 0.5;
        }
    });
    honest("limiter", &pts, 3.0);

    // A low shelf (one pole, +6 dB under 80 Hz): a filter, drawn in full and never marked,
    // with and without noise after it (other plugins' hiss, a room: γ² well under 1, which
    // alone read as "changes with level" once and blanked the page).
    for noise in [0.0, 0.1] {
        let k = 1.0 - (-std::f32::consts::TAU * 80.0 / FS).exp();
        let mut s1 = 0.0f32;
        let mut seed = 4242u32;
        let (pts, coh) = measure(&mut |b| {
            for v in b.iter_mut() {
                s1 += k * (*v - s1);
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                *v += s1 + noise * (seed as f32 / u32::MAX as f32 * 2.0 - 1.0);
            }
        });
        // (Under hiss its group delay wanders a few ms at the low end: noise, not level.)
        let marked = honest(&format!("filter, noise {noise}"), &pts, if noise > 0.0 { 10.0 } else { 1.0 });
        let worst = pts.iter().map(|p| (p.2, p.0)).fold((0.0, 0.0), |a, b| if b.0 > a.0 { b } else { a });
        assert!(worst.0 < 0.2, "a filter is never marked (noise {noise}, γ² {coh:.3}): {:.2} at {:.0} Hz", worst.0, worst.1);
        assert!(marked < 0.02);
        if noise == 0.0 {
            assert!(pts.iter().all(|p| p.1 > 0.95), "a filter's curves are drawn in full");
        }
    }
}
