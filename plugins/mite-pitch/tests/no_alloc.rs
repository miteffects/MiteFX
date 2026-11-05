//! The audio path must not allocate or free (BLUEPRINT §5): recording, playback, renders and
//! recording buffers arriving and leaving.

mod common;

use assert_no_alloc::assert_no_alloc as process_wrapper;
use common::*;
use mite_dsp::melody::NoteEdit;
use mite_pitch::dsp::{PitchEngine, Transport};

#[test]
fn engine_process_never_allocates() {
    let s = session("noalloc");
    let mut e = PitchEngine::new(FS as f32);
    let x = take();
    let (mut l, mut r) = (vec![0.0f32; 512], vec![0.0f32; 512]);
    let mut run = |e: &mut PitchEngine, playing: bool, from: i64, blocks: usize| {
        for k in 0..blocks {
            let n = if k % 3 == 0 { 512 } else { 77 };
            let pos = from + (k * 512) as i64;
            for i in 0..n {
                l[i] = x.get(pos as usize + i).copied().unwrap_or(0.0);
                r[i] = l[i];
            }
            let t = Transport {
                playing,
                pos: Some(pos),
            };
            process_wrapper(|| e.process(&mut [&mut l[..n], &mut r[..n]], t, false, &s));
            process_wrapper(|| e.process(&mut [&mut l[..n]], t, false, &s));
        }
    };
    s.set_armed(true);
    // Recording (a jump every block with 77 samples: each ends and the next arm restarts).
    run(&mut e, true, 0, 60);
    run(&mut e, false, 0, 2);
    s.poll();
    s.wait_idle(std::time::Duration::from_secs(30));
    for k in 0..6 {
        s.set_edit(
            k % s.edits().len().max(1),
            NoteEdit {
                shift_cents: 100.0 * k as f64,
                ..NoteEdit::default()
            },
        );
        s.wait_idle(std::time::Duration::from_secs(30));
        run(&mut e, true, 0, 40);
        s.renders.collect();
    }
    // The plugin's own transport, looping, seeking.
    s.local_play.store(true, std::sync::atomic::Ordering::Release);
    s.loop_start.store(1000, std::sync::atomic::Ordering::Release);
    s.loop_end.store(9000, std::sync::atomic::Ordering::Release);
    s.loop_on.store(true, std::sync::atomic::Ordering::Release);
    run(&mut e, false, 0, 40);
    s.seek.store(true, std::sync::atomic::Ordering::Release);
    run(&mut e, false, 0, 10);
    s.local_play.store(false, std::sync::atomic::Ordering::Release);
    run(&mut e, false, 0, 10);
    s.set_armed(true);
    run(&mut e, true, 0, 20);
}
