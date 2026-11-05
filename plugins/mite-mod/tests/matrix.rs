//! The modulation matrix through Mite Mod's engine: a route moves the sound, nothing routed is
//! the plain engine bit for bit, and the result is the same for every host block size.

use mite_core::modmatrix::MatrixTap;
use mite_core::HistoryTap;
use mite_dsp::modmatrix::{LfoSettings, MatrixSettings, Slot, Source};
use mite_dsp::modulation::Transport;
use mite_mod::dsp::{record, transport_at, ModEngine};
use mite_mod::editor::memory_host;
use mite_mod::params::{modulated_settings, MOD_TARGETS, MOD_TARGET_IDS};
use mite_ui::ParamHost;
use mite_plugin::matrix::MatrixRunner;

const FS: f32 = 48_000.0;

fn render(ms: &MatrixSettings, block: usize, x: &[f32]) -> (Vec<f32>, Vec<f32>, f32) {
    let host = memory_host();
    let p = |id: &str| host.plain(id, host.normalized(id));
    let (s0, out0) = modulated_settings(p, &[0.0; 64]);
    let mut engine = ModEngine::new(FS, &s0, &out0);
    let mut runner = MatrixRunner::default();
    runner.prepare(FS, 4096);
    let history: HistoryTap<{ record::WIDTH }> = HistoryTap::new(64);
    let tap = MatrixTap::new();
    let (mut l, mut r) = (x.to_vec(), x.to_vec());
    let mut peak_offset = 0.0f32;
    for (a, b) in l.chunks_mut(block).zip(r.chunks_mut(block)) {
        let mut ch: [&mut [f32]; 2] = [a, b];
        runner.run(ms, &Transport::default(), &mut ch, None, &tap, MOD_TARGETS.len(), |sub, range, offsets| {
            let (s, out) = modulated_settings(p, offsets);
            engine.process(sub, true, &s, &out, &transport_at(&Transport::default(), range.start, f64::from(FS)), &history);
        });
        peak_offset = peak_offset.max(tap.offset(8).abs());
    }
    (l, r, peak_offset)
}

fn mix_route() -> MatrixSettings {
    let mut ms = MatrixSettings::default();
    ms.lfos[0] = LfoSettings { rate_hz: 3.0, ..LfoSettings::default() };
    let mix = MOD_TARGET_IDS.iter().position(|t| *t == "mix").expect("mix");
    ms.slots[2] = Slot { source: Source::Lfo1, target: mix, amount: 0.5 };
    ms
}

#[test]
fn a_route_moves_the_sound_and_nothing_routed_is_the_plain_engine() {
    let x: Vec<f32> = (0..48_000).map(|i| 0.4 * (std::f32::consts::TAU * 440.0 * i as f32 / FS).sin()).collect();
    let (plain_l, _, none) = render(&MatrixSettings::default(), 512, &x);
    // The plain engine, without the runner.
    let host = memory_host();
    let (s, out) = modulated_settings(|id| host.plain(id, host.normalized(id)), &[0.0; 64]);
    let mut engine = ModEngine::new(FS, &s, &out);
    let history: HistoryTap<{ record::WIDTH }> = HistoryTap::new(64);
    let (mut l, mut r) = (x.clone(), x.clone());
    for (a, b) in l.chunks_mut(512).zip(r.chunks_mut(512)) {
        engine.process(&mut [a, b], true, &s, &out, &Transport::default(), &history);
    }
    assert_eq!(plain_l, l);
    assert_eq!(none, 0.0);
    let (moved, _, peak) = render(&mix_route(), 512, &x);
    assert!((peak - 0.5).abs() < 0.02, "the mix offset peaks at {peak}");
    let diff: f32 = moved.iter().zip(&plain_l).map(|(a, b)| (a - b).abs()).fold(0.0, f32::max);
    assert!(diff > 0.05, "the route changed nothing ({diff})");
}

#[test]
fn a_routed_matrix_sounds_the_same_for_any_block_size() {
    let x: Vec<f32> = (0..24_000).map(|i| 0.4 * (std::f32::consts::TAU * 330.0 * i as f32 / FS).sin()).collect();
    let (a, b, _) = render(&mix_route(), 512, &x);
    for block in [1, 7, 333] {
        let (c, d, _) = render(&mix_route(), block, &x);
        assert_eq!((&a, &b), (&c, &d), "block {block}");
    }
}
