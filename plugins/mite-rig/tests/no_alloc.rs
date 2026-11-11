//! The engine's audio path must not allocate or free (BLUEPRINT §5), whatever the board does:
//! graphs swapped in (blocks added, moved, removed; splits made), files swapped, bypasses,
//! mono blocks, odd sizes, another host rate.

use std::path::Path;
use std::sync::Arc;

use assert_no_alloc::assert_no_alloc as process_wrapper;
use mite_rig::blocks::Kind;
use mite_rig::board::Place;
use mite_rig::engine::{RigEngine, RigSettings, RigShared};
use mite_rig::loader::LoaderCore;
use mite_rig::params::RigParams;
use mite_rig::rig::SlotFile;

#[test]
fn engine_process_never_allocates() {
    for fs in [48_000.0f32, 44_100.0] {
        let params = Arc::new(RigParams::default());
        let board = params.board.clone();
        let shared = Arc::new(RigShared::new());
        let core = LoaderCore::new(board.clone(), shared.clone(), params.clone());
        core.set_rate(fs);
        let models = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../crates/mite-nam/tests/models");
        let cap = board.blocks().into_iter().find(|b| b.kind() == Kind::Amp).unwrap().id();
        board.set_file(cap, SlotFile { path: Some(models.join("wavenet.nam")), source: None });
        let mut s = RigSettings::default();
        let mut engine = RigEngine::new(fs, &s);
        let (mut l, mut r) = (vec![0.0f32; 1500], vec![0.0f32; 1500]);
        let mut seed = 3u32;
        for i in 0..300 {
            s.bypass = (i / 90) % 5 == 4;
            match i {
                20 => {
                    board.insert_split(1, 3);
                    board.insert(Kind::Tone, Place::Lane { split: 1, lane: 0, index: 0 });
                }
                60 => board.set_file(cap, SlotFile { path: Some(models.join("lstm.nam")), source: None }),
                100 => {
                    board.insert(Kind::Boost, Place::Main(0));
                    // Every pedal of the suite, after the amp.
                    for kind in [Kind::Comp, Kind::Eq, Kind::Mod, Kind::Tremolo, Kind::Double, Kind::Delay, Kind::Reverb, Kind::Tuner, Kind::Wah, Kind::Octaver] {
                        board.insert(kind, Place::Main(usize::MAX));
                    }
                }
                140 => {
                    let tone = board.blocks().into_iter().find(|b| b.kind() == Kind::Tone).unwrap().id();
                    board.move_block(tone, Place::Main(0));
                }
                180 => board.blocks()[0].live.bypass.store(true, std::sync::atomic::Ordering::Relaxed),
                _ => {}
            }
            core.step();
            for (a, b) in l.iter_mut().zip(r.iter_mut()) {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                *a = (seed as f32 / u32::MAX as f32 - 0.5) * 0.2;
                *b = *a;
            }
            let block = [1500, 64, 513][i % 3];
            process_wrapper(|| engine.process(&mut [&mut l[..block], &mut r[..block]], &s, &shared));
            if i % 50 == 0 {
                process_wrapper(|| engine.process(&mut [&mut l[..block]], &s, &shared));
            }
            assert!(l.iter().chain(&r).all(|x| x.is_finite()));
        }
        process_wrapper(|| engine.reset(&s));
    }
}

/// The macros' audio side (host values, CCs, a CC learned, a new table) never allocates either.
#[test]
fn macros_never_allocate() {
    use mite_rig::macros::{MacroRunner, MacroTable, Target, KNOBS};
    let params = Arc::new(RigParams::default());
    let board = params.board.clone();
    let shared = Arc::new(RigShared::new());
    let wah = board.insert(Kind::Wah, Place::Main(0)).unwrap();
    let comp = board.insert(Kind::Comp, Place::Main(1)).unwrap();
    board.macros.toggle(0, Target { block: wah, param: "position".into() });
    board.macros.toggle(1, Target { block: comp, param: "s1_thresh".into() });
    board.macros.toggle(KNOBS, Target { block: comp, param: String::new() });
    board.macros.set_cc(1, Some(11));
    let mut run = MacroRunner::new();
    for k in 0..50 {
        if k % 10 == 0 {
            shared.macros.collect();
            shared.macros.send(Box::new(MacroTable::compile(&board)));
        }
        if k == 25 {
            board.macros.learning.store(2, std::sync::atomic::Ordering::Release);
        }
        process_wrapper(|| {
            run.take(&shared.macros);
            run.host(0, k as f32 / 50.0);
            run.host(KNOBS, (k % 2) as f32);
            run.cc(&board.macros, 11, 0.3);
            run.cc(&board.macros, 20, 0.7);
            run.apply(&board.macros);
        });
        board.macros.take_learned();
    }
}
