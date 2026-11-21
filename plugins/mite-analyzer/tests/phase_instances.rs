//! Two Mite Analyzers in one process, one before a chain and one after it, as a host runs them
//! (block by block, the first one first): each finds the other through the registry, the second
//! measures the chain's delay, the first knows it is the reference. Its own test binary: the
//! registry is process-wide.

use std::sync::Arc;

use mite_analyzer::dsp::AnalyzerEngine;
use mite_analyzer::editor::phase::Status;
use mite_analyzer::editor::{memory_host, AnalyzerView};
use mite_analyzer::params::PAGE;
use mite_analyzer::{AnalyzerShared, INSTANCES};
use mite_ui::View;

#[test]
fn the_instance_after_the_chain_measures_it_and_the_one_before_is_its_reference() {
    let fs = 48_000.0;
    let before = Arc::new(AnalyzerShared::new());
    let after = Arc::new(AnalyzerShared::new());
    let n_before = INSTANCES.register(&before);
    let n_after = INSTANCES.register(&after);
    for s in [&before, &after] {
        s.sample_rate.store(fs);
    }
    let (mut ea, mut eb) = (AnalyzerEngine::new(fs), AnalyzerEngine::new(fs));
    // The second one started a little later: its counts trail by 1000 samples' worth of blocks.
    let mut seed = 9u32;
    let mut line = vec![0.0f32; 300];
    let mut pos = 0;
    for block in 0..(6 * 48_000 / 480) {
        let mut x: Vec<f32> = (0..480)
            .map(|_| {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                0.3 * ((seed as f32 / u32::MAX as f32) * 2.0 - 1.0)
            })
            .collect();
        let (mut xl, mut xr) = (x.clone(), x.clone());
        ea.process(&[&mut xl, &mut xr], &before.tel);
        // The chain: 300 samples of delay.
        for v in x.iter_mut() {
            std::mem::swap(v, &mut line[pos]);
            pos = (pos + 1) % line.len();
        }
        if block >= 2 {
            let (mut yl, mut yr) = (x.clone(), x);
            eb.process(&[&mut yl, &mut yr], &after.tel);
        }
    }
    let host = memory_host();
    host.set_plain(PAGE.id, 1.0);
    let dir = std::env::temp_dir().join(format!("mitefx-analyzer-instances-{}", std::process::id()));
    let mut view_after = AnalyzerView::new(after.clone()).with_preset_dir(dir.clone());
    view_after.tick(&host);
    view_after.settle(&host);
    let lab = view_after.phase();
    assert_eq!(lab.status(), Status::Measuring);
    let d = lab.delay().expect("a delay");
    assert!((d - 300.0).abs() < 0.05, "the chain's delay: {d}");
    let mut view_before = AnalyzerView::new(before.clone()).with_preset_dir(dir);
    view_before.tick(&host);
    view_before.settle(&host);
    assert_eq!(view_before.phase().status(), Status::Upstream(n_after), "the first one is the reference (Analyzer {n_before})");
}
