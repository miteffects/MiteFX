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
    host.notify_model_updates();
    assert!(mite_ara::testing::calls().iter().any(|c| c == "data changed"));
    host.edit_cycle();
    assert_eq!(mite_ara::testing::violations(), Vec::<String>::new(), "ARA's threading rules");
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

#[test]
fn an_editor_instance_and_a_playback_instance_as_reaper_binds_them_over_vst3() {
    let x = take();
    let mut host = TestHost::new(mite_ara::factory(&mite_pitch::ara::INFO));
    let dur = x.len() as f64 / FS;
    let (_src, _md, rg) = host.add_clip(Audio { fs: FS, l: x.clone(), r: x.clone() }, c"src-r", c"mod-r", 1.0, 0.0, dur);
    // The visible FX: editor renderer and view (6); a hidden one: playback renderer (1).
    let editor = session("ara-ed");
    let player = session("ara-pl");
    let inst_ed = unsafe { mite_ara::bind(host.dc, 7, 6) };
    let inst_pl = unsafe { mite_ara::bind(host.dc, 7, 1) };
        // As REAPER: the editor gets the track, the player the clip.
    host.show_track(inst_ed.extension());
    host.render(inst_pl.extension(), rg);
    assert!(inst_ed.document.wait_reads(Duration::from_secs(10)));
    editor.bind_ara(inst_ed);
    player.bind_ara(inst_pl);
    assert!(editor.wait_idle(Duration::from_secs(30)) && player.wait_idle(Duration::from_secs(30)));
    assert_eq!(editor.analysis().expect("the editor shows the clip").notes.len(), 4);
    // The editor instance passes its input; the player renders the clip (silence before it).
    let (mut e_ed, mut e_pl) = (PitchEngine::new(FS as f32), PitchEngine::new(FS as f32));
    song(&mut e_ed, &editor, 0, 480);
    song(&mut e_pl, &player, 0, 480);
    let through = song(&mut e_ed, &editor, 0, 48_000 + x.len());
    assert!(through.iter().all(|v| *v == 0.5), "the editor instance passes its input");
    let out = song(&mut e_pl, &player, 0, 48_000 + x.len());
    assert!(out[..48_000 - 241].iter().all(|v| *v == 0.0));
    assert!((out[48_000 + 30_000] - x[30_000]).abs() < 1e-6);
    // An edit in the editor reaches the player through the document.
    editor.set_edit(0, NoteEdit { shift_cents: 100.0, ..NoteEdit::default() });
    assert!(editor.wait_idle(Duration::from_secs(30)));
    player.sync_ara();
    assert!(player.wait_idle(Duration::from_secs(30)));
    assert_eq!(player.edits(), editor.edits());
    let out = song(&mut e_pl, &player, 48_000, x.len());
    let a = editor.analysis().expect("analysis");
    let n0 = &a.notes[0];
    let moved = out[n0.start..n0.end].iter().zip(&x[n0.start..n0.end]).map(|(o, v)| (o - v).abs()).fold(0.0f32, f32::max);
    assert!(moved > 0.01, "{moved}");
    host.notify_model_updates();
    host.edit_cycle();
    assert_eq!(mite_ara::testing::violations(), Vec::<String>::new());
}

#[test]
fn the_player_plays_the_editors_render_without_rendering_again() {
    let x = take();
    let mut host = TestHost::new(mite_ara::factory(&mite_pitch::ara::INFO));
    let dur = x.len() as f64 / FS;
    let (_src, _md, rg) = host.add_clip(Audio { fs: FS, l: x.clone(), r: x.clone() }, c"src-s", c"mod-s", 0.0, 0.0, dur);
    let editor = session("ara-share-ed");
    let player = session("ara-share-pl");
    let inst_ed = unsafe { mite_ara::bind(host.dc, 7, 6) };
    let inst_pl = unsafe { mite_ara::bind(host.dc, 7, 1) };
    host.show_track(inst_ed.extension());
    host.render(inst_pl.extension(), rg);
    assert!(inst_ed.document.wait_reads(Duration::from_secs(10)));
    editor.bind_ara(inst_ed);
    player.bind_ara(inst_pl);
    assert!(editor.wait_idle(Duration::from_secs(30)) && player.wait_idle(Duration::from_secs(30)));
    let mut e_pl = PitchEngine::new(FS as f32);
    song(&mut e_pl, &player, 0, 480);
    let made = player.renders_made.load(std::sync::atomic::Ordering::Acquire);
    // Three edits in a row, as a drag makes them: each reaches the player already rendered.
    for k in 1..=3 {
        editor.set_edit(1, NoteEdit { shift_cents: 40.0 * f64::from(k), ..NoteEdit::default() });
        assert!(editor.wait_idle(Duration::from_secs(30)));
        player.sync_ara();
        assert!(player.wait_idle(Duration::from_secs(30)));
    }
    assert_eq!(player.renders_made.load(std::sync::atomic::Ordering::Acquire), made, "the player rendered again");
    // What it plays is the editor's render: the same as rendering the edits from scratch.
    let a = editor.analysis().expect("analysis");
    let mut want = mite_dsp::melody::render(&x, &a, &editor.edits());
    want.resize(x.len(), 0.0);
    let out = song(&mut e_pl, &player, 0, x.len());
    let n1 = &a.notes[1];
    let err = out[n1.start..n1.end].iter().zip(&want[n1.start..n1.end]).map(|(o, w)| (o - w).abs()).fold(0.0f32, f32::max);
    assert!(err < 1e-5, "{err}");
    assert_eq!(mite_ara::testing::violations(), Vec::<String>::new());
}

#[test]
fn without_the_editors_render_the_player_renders_only_the_changed_note() {
    // A player alone (no editor shares a render): the document's state changes, as when the host
    // restores an edit; the player re-renders the note, and it equals a whole render.
    let x = take();
    let mut host = TestHost::new(mite_ara::factory(&mite_pitch::ara::INFO));
    let dur = x.len() as f64 / FS;
    let (_src, _md, rg) = host.add_clip(Audio { fs: FS, l: x.clone(), r: x.clone() }, c"src-p", c"mod-p", 0.0, 0.0, dur);
    let player = session("ara-partial-pl");
    let inst = unsafe { mite_ara::bind(host.dc, 7, 7) };
    host.give(inst.extension(), rg);
    assert!(inst.document.wait_reads(Duration::from_secs(10)));
    let doc = inst.document.clone();
    player.bind_ara(inst);
    assert!(player.wait_idle(Duration::from_secs(30)));
    let a = player.analysis().expect("analysis");
    let mut edits = vec![NoteEdit::default(); a.notes.len()];
    edits[2].shift_cents = -150.0;
    let id = doc.snapshot().modifications.values().find(|m| m.persistent_id == "mod-p").map(|m| m.id).expect("mod");
    doc.set_state(id, mite_pitch::session::edits_json(&edits, &a, player.scale()), false);
    player.sync_ara();
    assert!(player.wait_idle(Duration::from_secs(30)));
    assert_eq!(player.edits(), edits);
    let mut e = PitchEngine::new(FS as f32);
    song(&mut e, &player, 0, 480);
    let out = song(&mut e, &player, 0, x.len());
    let mut want = mite_dsp::melody::render(&x, &a, &edits);
    want.resize(x.len(), 0.0);
    let err = out.iter().zip(&want).skip(480).map(|(o, w)| (o - w).abs()).fold(0.0f32, f32::max);
    assert!(err < 1e-5, "{err}");
}

#[test]
fn the_songs_bars_and_beats_come_from_the_musical_context() {
    let x = take();
    let mut host = TestHost::new(mite_ara::factory(&mite_pitch::ara::INFO));
    let (s, _e, _md) = bound(&mut host, &x, c"mod-g");
    assert!(s.grid().is_none(), "no tempo given yet");
    // 90 bpm in 3/4: a bar is 2 s.
    host.set_time(&[(0.0, 0.0), (2.0 / 3.0, 1.0)], &[(3, 4, 0.0)]);
    s.sync_ara();
    assert!(s.wait_idle(Duration::from_secs(30)));
    let g = s.grid().expect("the host's tempo map");
    assert_eq!(g.bar_beat(4.0), (3, 1.0));
    assert!((g.seconds_at(3.0) - 2.0).abs() < 1e-9);
}
