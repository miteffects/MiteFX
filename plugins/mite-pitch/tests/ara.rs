//! Mite Pitch bound to an ARA document (a host in Rust plays REAPER's part): the clip comes in
//! without a transfer, it plays where the region sits on the song timeline (silence elsewhere),
//! unedited it is the clip, an edit plays and reaches the host's archive, and a fresh document
//! restored from that archive plays the edit again.

mod common;

use std::time::Duration;

use common::*;
use mite_ara::testing::{Audio, TestHost};
use mite_dsp::melody::NoteEdit;
use mite_pitch::dsp::{PitchEngine, Transport};
use mite_pitch::session::{Session, Status};

/// Plays song time `from`..`from + n` (host playing), input full of 0.5 (which ARA ignores).
fn song(e: &mut PitchEngine, s: &Session, from: i64, n: usize) -> Vec<f32> {
    let mut out = Vec::with_capacity(n);
    let mut pos = from;
    while out.len() < n {
        let k = 480.min(n - out.len());
        let (mut l, mut r) = (vec![0.5f32; k], vec![0.5f32; k]);
        e.process(&mut [&mut l, &mut r], Transport { playing: true, pos: Some(pos) }, false, s);
        out.extend_from_slice(&l);
        pos += k as i64;
    }
    out
}

fn bound(host: &mut TestHost, x: &[f32], mod_id: &std::ffi::CStr) -> (std::sync::Arc<Session>, PitchEngine, mite_ara::sys::ARAAudioModificationRef) {
    let dur = x.len() as f64 / FS;
    let (_src, md, rg) = host.add_clip(Audio { fs: FS, l: x.to_vec(), r: x.to_vec() }, c"src-1", mod_id, 1.0, 0.0, dur);
    let s = session("ara");
    let inst = unsafe { mite_ara::bind(host.dc, 7, 7) };
    host.give(inst.extension(), rg);
    assert!(inst.document.wait_reads(Duration::from_secs(10)));
    s.bind_ara(inst);
    assert!(s.wait_idle(Duration::from_secs(30)));
    let mut e = PitchEngine::new(FS as f32);
    // One block to pick up the render.
    song(&mut e, &s, 0, 480);
    (s, e, md)
}

#[test]
fn a_clip_plays_where_its_region_sits_and_edits_persist_through_the_archive() {
    let x = take();
    let mut host = TestHost::new(mite_ara::factory(&mite_pitch::ara::INFO));
    let (s, mut e, md) = bound(&mut host, &x, c"mod-1");
    assert_eq!(s.status(), Status::Ready);
    assert_eq!(s.analysis().expect("analysis").notes.len(), 4, "the clip's notes, without a transfer");
    let t = s.take().expect("take");
    assert_eq!(t.start, FS as i64, "the clip starts at song time 1 s");
    assert!(s.to_json().is_none(), "the plugin state keeps no take under ARA");
    // Unedited: the clip, at 1 s on; silence before (ARA ignores the input).
    let out = song(&mut e, &s, 0, 48_000 + x.len() + 4800);
    let fade = 241;
    assert!(out[..48_000 - fade].iter().all(|v| *v == 0.0));
    let worst = out[48_000 + fade..48_000 + x.len() - fade].iter().zip(&x[fade..x.len() - fade]).map(|(o, v)| (o - v).abs()).fold(0.0f32, f32::max);
    assert!(worst < 1e-6, "{worst}");
    assert!(out[48_000 + x.len() + fade..].iter().all(|v| *v == 0.0));
    // An edit: it plays, and the host's document holds it.
    s.set_edit(1, NoteEdit { shift_cents: 200.0, ..NoteEdit::default() });
    assert!(s.wait_idle(Duration::from_secs(30)));
    let edited = song(&mut e, &s, 48_000, x.len());
    let a = s.analysis().expect("analysis");
    let n1 = &a.notes[1];
    let moved = edited[n1.start..n1.end].iter().zip(&x[n1.start..n1.end]).map(|(o, v)| (o - v).abs()).fold(0.0f32, f32::max);
    assert!(moved > 0.01, "{moved}");
    assert!(mite_ara::testing::calls().iter().any(|c| c == "data changed"));
    let archive = host.store(&[md]);
    // A fresh document (the project reopened): restored before the plugin binds.
    let mut host2 = TestHost::new(mite_ara::factory(&mite_pitch::ara::INFO));
    let dur = x.len() as f64 / FS;
    let (_src, _md2, rg2) = host2.add_clip(Audio { fs: FS, l: x.clone(), r: x.clone() }, c"src-1", c"mod-1", 1.0, 0.0, dur);
    assert!(host2.restore(&archive));
    let s2 = session("ara-2");
    let inst2 = unsafe { mite_ara::bind(host2.dc, 7, 7) };
    host2.give(inst2.extension(), rg2);
    assert!(inst2.document.wait_reads(Duration::from_secs(10)));
    s2.bind_ara(inst2);
    assert!(s2.wait_idle(Duration::from_secs(30)));
    assert_eq!(s2.edits(), s.edits());
    let mut e2 = PitchEngine::new(FS as f32);
    song(&mut e2, &s2, 0, 480);
    let again = song(&mut e2, &s2, 48_000, x.len());
    let worst = again.iter().zip(&edited).map(|(a, b)| (a - b).abs()).fold(0.0f32, f32::max);
    assert!(worst < 1e-4, "{worst}");
}

#[test]
fn moving_the_region_moves_what_plays() {
    let x = take();
    let mut host = TestHost::new(mite_ara::factory(&mite_pitch::ara::INFO));
    let dur = x.len() as f64 / FS;
    let (_src, md, rg) = host.add_clip(Audio { fs: FS, l: x.clone(), r: x.clone() }, c"src-m", c"mod-m", 1.0, 0.0, dur);
    let s = session("ara-move");
    let inst = unsafe { mite_ara::bind(host.dc, 7, 7) };
    host.give(inst.extension(), rg);
    assert!(inst.document.wait_reads(Duration::from_secs(10)));
    s.bind_ara(inst);
    assert!(s.wait_idle(Duration::from_secs(30)));
    // To song time 3 s, playing only from 0.5 s of the clip.
    host.move_region(md, rg, 3.0, 0.5, dur - 0.5);
    s.sync_ara();
    assert!(s.wait_idle(Duration::from_secs(30)));
    let mut e = PitchEngine::new(FS as f32);
    song(&mut e, &s, 0, 480);
    let out = song(&mut e, &s, 3 * 48_000, 24_000);
    assert!((out[10_000] - x[24_000 + 10_000]).abs() < 1e-6);
    // The audio thread playing ARA segments never allocates.
    let (mut l, mut r) = (vec![0.0f32; 480], vec![0.0f32; 480]);
    for k in 0..200 {
        let t = Transport { playing: true, pos: Some(2 * 48_000 + k * 480) };
        assert_no_alloc::assert_no_alloc(|| e.process(&mut [&mut l, &mut r], t, false, &s));
    }
    // The editor's take starts where the clip's start would be: 3 − 0.5 s.
    assert_eq!(s.take().expect("take").start, (2.5 * FS) as i64);
}
