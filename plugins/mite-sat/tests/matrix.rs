//! The modulation matrix through Mite Sat's engine: a route moves the sound (a band's drive, a
//! crossover), nothing routed is the plain engine bit for bit, and the result is the same for
//! every host block size.

use mite_core::modmatrix::MatrixTap;
use mite_core::HistoryTap;
use mite_dsp::modmatrix::{LfoSettings, MatrixSettings, Slot, Source};
use mite_dsp::modulation::Transport;
use mite_dsp::sat::SatSettings;
use mite_plugin::matrix::MatrixRunner;
use mite_sat::dsp::{record, Output, SatEngine};
use mite_sat::params::{modulate_settings, MOD_TARGETS, MOD_TARGET_IDS};

const FS: f32 = 48_000.0;

fn base() -> SatSettings {
    let mut s = SatSettings::default();
    s.bands[0].drive_db = 12.0;
    s.crossovers[0] = Some(500.0);
    s.bands[1].drive_db = 6.0;
    s
}

fn render(ms: &MatrixSettings, block: usize, x: &[f32]) -> (Vec<f32>, f32) {
    let (s0, out0) = (base(), Output::default());
    let mut engine = SatEngine::new(FS, 2, &s0);
    engine.prepare(FS, 2, &s0, &out0);
    let mut runner = MatrixRunner::default();
    runner.prepare(FS, 4096);
    let history: HistoryTap<{ record::WIDTH }> = HistoryTap::new(64);
    let tap = MatrixTap::new();
    let (mut l, mut r) = (x.to_vec(), x.to_vec());
    let mut peak = 0.0f32;
    for (a, b) in l.chunks_mut(block).zip(r.chunks_mut(block)) {
        runner.run(ms, &Transport::default(), &mut [a, b], None, &tap, MOD_TARGETS.len(), |sub, _, offsets| {
            let (mut s, mut out) = (s0, out0);
            modulate_settings(&mut s, &mut out, offsets);
            engine.process(sub, &s, 2, &out, &history, None);
        });
        peak = peak.max(tap.offsets.iter().map(|o| o.load().abs()).fold(0.0, f32::max));
    }
    (l, peak)
}

fn route(target: &str, amount: f32) -> MatrixSettings {
    let mut ms = MatrixSettings::default();
    ms.lfos[0] = LfoSettings { rate_hz: 3.0, ..LfoSettings::default() };
    let t = MOD_TARGET_IDS.iter().position(|id| *id == target).expect("target");
    ms.slots[5] = Slot { source: Source::Lfo1, target: t, amount };
    ms
}

fn input() -> Vec<f32> {
    (0..48_000).map(|i| 0.3 * (std::f32::consts::TAU * 220.0 * i as f32 / FS).sin() + 0.1 * (std::f32::consts::TAU * 1800.0 * i as f32 / FS).sin()).collect()
}

#[test]
fn a_route_moves_the_sound_and_nothing_routed_is_the_plain_engine() {
    let x = input();
    let (plain, none) = render(&MatrixSettings::default(), 512, &x);
    let s = base();
    let mut engine = SatEngine::new(FS, 2, &s);
    engine.prepare(FS, 2, &s, &Output::default());
    let history: HistoryTap<{ record::WIDTH }> = HistoryTap::new(64);
    let (mut l, mut r) = (x.clone(), x.clone());
    for (a, b) in l.chunks_mut(512).zip(r.chunks_mut(512)) {
        engine.process(&mut [a, b], &s, 2, &Output::default(), &history, None);
    }
    assert_eq!(plain, l);
    assert_eq!(none, 0.0);
    for target in ["b1_drive", "x1_freq", "out_gain"] {
        let (moved, peak) = render(&route(target, 0.3), 512, &x);
        assert!((peak - 0.3).abs() < 0.02, "{target}: offset peaks at {peak}");
        assert!(moved.iter().all(|v| v.is_finite()));
        let diff = moved.iter().zip(&plain).map(|(a, b)| (a - b).abs()).fold(0.0, f32::max);
        assert!(diff > 0.01, "{target}: the route changed nothing ({diff})");
    }
}

#[test]
fn a_routed_matrix_sounds_the_same_for_any_block_size() {
    let x = input();
    // ±14 dB around 12 dB of drive: the band also goes quiet (drive 0) and wakes up.
    let ms = route("b1_drive", 0.4);
    let (a, _) = render(&ms, 512, &x);
    for block in [1, 7, 333] {
        let b = render(&ms, block, &x).0;
        let first = a.iter().zip(&b).position(|(p, q)| p != q);
        let worst = a.iter().zip(&b).map(|(p, q)| (p - q).abs()).fold(0.0f32, f32::max);
        assert!(first.is_none(), "block {block}: first difference at {first:?}, worst {worst}");
    }
}
