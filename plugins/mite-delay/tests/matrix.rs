//! The modulation matrix through Mite Delay's engine: a route moves the sound (a synced tap's time
//! too), nothing routed is the plain engine bit for bit, and the result is the same for every
//! host block size.

use mite_core::modmatrix::MatrixTap;
use mite_core::HistoryTap;
use mite_delay::dsp::{record, DelayEngine};
use mite_delay::editor::memory_host;
use mite_delay::params::{modulated_settings, MOD_TARGETS, MOD_TARGET_IDS};
use mite_dsp::modmatrix::{LfoSettings, MatrixSettings, Slot, Source};
use mite_dsp::modulation::Transport;
use mite_plugin::matrix::MatrixRunner;
use mite_ui::ParamHost;

const FS: f32 = 48_000.0;
const TEMPO: f32 = 120.0;

fn render(ms: &MatrixSettings, block: usize, x: &[f32]) -> (Vec<f32>, f32) {
    let host = memory_host();
    let p = |id: &str| host.plain(id, host.normalized(id));
    let (s0, out0) = modulated_settings(p, TEMPO, &[0.0; 64]);
    let mut engine = DelayEngine::new(FS, &s0, &out0);
    let mut runner = MatrixRunner::default();
    runner.prepare(FS, 4096);
    let history: HistoryTap<{ record::WIDTH }> = HistoryTap::new(64);
    let tap = MatrixTap::new();
    let (mut l, mut r) = (x.to_vec(), x.to_vec());
    let mut peak = 0.0f32;
    for (a, b) in l.chunks_mut(block).zip(r.chunks_mut(block)) {
        runner.run(ms, &Transport::default(), &mut [a, b], None, &tap, MOD_TARGETS.len(), |sub, _, offsets| {
            let (s, out) = modulated_settings(p, TEMPO, offsets);
            engine.process(sub, &s, &out, &history, None);
        });
        peak = peak.max(tap.offsets.iter().map(|o| o.load().abs()).fold(0.0, f32::max));
    }
    (l, peak)
}

fn route(target: &str, amount: f32) -> MatrixSettings {
    let mut ms = MatrixSettings::default();
    ms.lfos[0] = LfoSettings { rate_hz: 2.0, ..LfoSettings::default() };
    let t = MOD_TARGET_IDS.iter().position(|id| *id == target).expect("target");
    ms.slots[0] = Slot { source: Source::Lfo1, target: t, amount };
    ms
}

fn input() -> Vec<f32> {
    (0..48_000).map(|i| if i % 6000 < 400 { 0.5 * (std::f32::consts::TAU * 440.0 * i as f32 / FS).sin() } else { 0.0 }).collect()
}

#[test]
fn a_route_moves_the_sound_and_nothing_routed_is_the_plain_engine() {
    let x = input();
    let (plain, none) = render(&MatrixSettings::default(), 512, &x);
    let host = memory_host();
    let (s, out) = modulated_settings(|id| host.plain(id, host.normalized(id)), TEMPO, &[0.0; 64]);
    let mut engine = DelayEngine::new(FS, &s, &out);
    let history: HistoryTap<{ record::WIDTH }> = HistoryTap::new(64);
    let (mut l, mut r) = (x.clone(), x.clone());
    for (a, b) in l.chunks_mut(512).zip(r.chunks_mut(512)) {
        engine.process(&mut [a, b], &s, &out, &history, None);
    }
    assert_eq!(plain, l);
    assert_eq!(none, 0.0);
    // Tap 1's level and its (synced) time both change the echoes.
    for target in ["t1_level", "t1_time"] {
        let (moved, peak) = render(&route(target, 0.3), 512, &x);
        assert!((peak - 0.3).abs() < 0.02, "{target}: offset peaks at {peak}");
        let diff = moved.iter().zip(&plain).map(|(a, b)| (a - b).abs()).fold(0.0, f32::max);
        assert!(diff > 0.02, "{target}: the route changed nothing ({diff})");
    }
}

#[test]
fn a_routed_matrix_sounds_the_same_for_any_block_size() {
    let x = input();
    let ms = route("t1_feedback", 0.4);
    let (a, _) = render(&ms, 512, &x);
    for block in [1, 7, 333] {
        assert_eq!(a, render(&ms, block, &x).0, "block {block}");
    }
}
