//! The Analyzer placed after Mite Limit reads what Limit's loudness column reads (same chain,
//! same start, preview and unity off), and differs by exactly the preview's gain when it is on.

use mite_analyzer::dsp::{AnalyzerEngine, Telemetry};
use mite_core::HistoryTap;
use mite_dsp::db_to_gain;
use mite_limit_core::{record, LimitEngine, LimitSettings, LoudnessTap};

fn material(n: usize, db: f32) -> (Vec<f32>, Vec<f32>) {
    let a = db_to_gain(db);
    let mut seed = 0x1234_5678u32;
    let mut noise = move || {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        seed as f32 / u32::MAX as f32 * 2.0 - 1.0
    };
    let mut l = Vec::with_capacity(n);
    let mut r = Vec::with_capacity(n);
    for i in 0..n {
        let t = (i % 24000) as f32 / 48000.0;
        let kick = (t * 55.0 * std::f32::consts::TAU * (1.0 + 2.0 * (-t * 30.0).exp())).sin() * (-t * 8.0).exp();
        // Sections of different loudness, so the range and the gating matter.
        let section = if (i / 96000) % 3 == 1 { 0.3 } else { 1.0 };
        let tone = 0.3 * (i as f32 * 0.0731).sin();
        l.push(a * section * (kick + tone + 0.2 * noise()));
        r.push(a * section * (kick * 0.8 + tone + 0.2 * noise()));
    }
    (l, r)
}

/// Limit then the Analyzer, block by block as a host would. Returns (Limit tap, Analyzer telemetry).
fn chain(s: &LimitSettings, block: usize) -> (LoudnessTap, Telemetry) {
    let fs = 48000.0;
    let mut limit = LimitEngine::new(fs, s);
    let mut analyzer = AnalyzerEngine::new(fs);
    let history: HistoryTap<{ record::WIDTH }> = HistoryTap::new(8192);
    let (tap, tel) = (LoudnessTap::new(), Telemetry::new());
    let (mut l, mut r) = material(48000 * 20, 0.0);
    for (a, b) in l.chunks_mut(block).zip(r.chunks_mut(block)) {
        limit.process(&mut [&mut *a, &mut *b], s, &history, &tap);
        analyzer.process(&[a, b], &tel);
    }
    (tap, tel)
}

#[test]
fn the_analyzer_after_limit_reads_what_limit_reads() {
    for (name, s) in [
        ("default", LimitSettings { gain_db: 6.0, ..LimitSettings::default() }),
        ("1x, no true peak", LimitSettings { gain_db: 6.0, oversampling: 1, true_peak: false, ..LimitSettings::default() }),
        ("hard push 4x", LimitSettings { gain_db: 14.0, oversampling: 4, ..LimitSettings::default() }),
    ] {
        for block in [64, 512, 1000] {
            let (tap, tel) = chain(&s, block);
            let rows = [
                ("M", tap.momentary.load(), tel.momentary.load()),
                ("S", tap.short_term.load(), tel.short_term.load()),
                ("I", tap.integrated.load(), tel.integrated.load()),
                ("LRA", tap.range.load(), tel.range.load()),
                ("TP", tap.true_peak_max.load(), tel.true_peak.load()),
            ];
            for (what, limit, analyzer) in rows {
                println!("{name:18} block {block:4} {what:3}: limit {limit:8.3}  analyzer {analyzer:8.3}  diff {:+.3}", analyzer - limit);
                assert!((analyzer - limit).abs() < 0.1, "{name}, block {block}: {what} limit {limit} vs analyzer {analyzer}");
            }
        }
    }
}

#[test]
fn with_the_preview_on_the_analyzer_hears_the_platform_gain() {
    // Preview is monitoring: Limit's column reads the master, the Analyzer what plays.
    let s = LimitSettings { gain_db: 12.0, preview: true, ..LimitSettings::default() };
    let (tap, tel) = chain(&s, 512);
    let diff = tel.short_term.load() - tap.short_term.load();
    println!("preview: limit S {:.2}, analyzer S {:.2}, platform gain {:.2}", tap.short_term.load(), tel.short_term.load(), tap.platform_gain.load());
    assert!((diff - tap.platform_gain.load()).abs() < 0.5, "differs by {diff}, platform gain {}", tap.platform_gain.load());
}
