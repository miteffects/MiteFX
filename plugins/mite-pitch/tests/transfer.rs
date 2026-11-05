//! Transfer and playback through the engine as a host drives it: the take lands on the timeline
//! where it was recorded, unedited it plays back as the input, an edit plays where it was made,
//! and the take comes back from the state (its file) after a reload.

mod common;

use common::*;
use mite_dsp::melody::NoteEdit;
use mite_pitch::dsp::PitchEngine;
use mite_pitch::session::Status;

#[test]
fn a_transfer_finds_the_notes_where_they_were_played() {
    let s = session("find");
    let mut e = PitchEngine::new(FS as f32);
    let x = take();
    transfer(&mut e, &s, &x, 96_000);
    let t = s.take().expect("a take");
    assert_eq!((t.start, t.len), (96_000, x.len()));
    let a = s.analysis().expect("analysis");
    assert_eq!(a.notes.len(), 4);
    assert_eq!(s.edits().len(), 4);
    assert!(
        s.dir().join(format!("{}.f32", t.id)).exists(),
        "the take's file"
    );
}

#[test]
fn unedited_it_plays_back_as_the_input_and_outside_it_the_input_passes() {
    let s = session("null");
    let mut e = PitchEngine::new(FS as f32);
    let x = take();
    transfer(&mut e, &s, &x, 10_000);
    settle(&mut e, &s);
    // Play the take's stretch with silence going in: out comes the take.
    let silence = vec![0.0f32; x.len() + 20_000];
    let out = play(&mut e, &s, &silence, 0, 333);
    let edge = (0.005 * FS) as usize + 1;
    for (i, v) in x.iter().enumerate().skip(edge).take(x.len() - 2 * edge) {
        assert!(
            (out[10_000 + i] - v).abs() < 1e-6,
            "sample {i}: {} vs {v}, around {:?}",
            out[10_000 + i],
            &out[10_000 + i - 3..10_000 + i + 3]
        );
    }
    assert!(
        out[..10_000 - edge].iter().all(|v| *v == 0.0),
        "before the take: the input"
    );
    assert!(
        out[10_000 + x.len() + edge..].iter().all(|v| *v == 0.0),
        "after the take: the input"
    );
    // Stopped, nothing plays.
    let (mut l, mut r) = (vec![0.5f32; 256], vec![0.5f32; 256]);
    e.process(
        &mut [&mut l, &mut r],
        mite_pitch::dsp::Transport {
            playing: false,
            pos: Some(20_000),
        },
        false,
        &s,
    );
    assert!(l.iter().all(|v| *v == 0.5));
}

#[test]
fn an_edit_plays_only_where_its_note_is() {
    let s = session("edit");
    let mut e = PitchEngine::new(FS as f32);
    let x = take();
    transfer(&mut e, &s, &x, 0);
    let a = s.analysis().expect("analysis");
    s.set_edit(
        1,
        NoteEdit {
            shift_cents: 200.0,
            ..NoteEdit::default()
        },
    );
    settle(&mut e, &s);
    let out = play(&mut e, &s, &vec![0.0; x.len()], 0, 512);
    let n0 = &a.notes[0];
    let n1 = &a.notes[1];
    let n3 = &a.notes[3];
    // The first and last notes untouched, the second moved.
    assert!(
        out[n0.start..n0.end - 2000]
            .iter()
            .zip(&x[n0.start..n0.end - 2000])
            .all(|(o, v)| (o - v).abs() < 1e-6)
    );
    assert!(
        out[n3.start + 2000..n3.end]
            .iter()
            .zip(&x[n3.start + 2000..n3.end])
            .all(|(o, v)| (o - v).abs() < 1e-6)
    );
    let moved = out[n1.start..n1.end]
        .iter()
        .zip(&x[n1.start..n1.end])
        .map(|(o, v)| (o - v).abs())
        .fold(0.0f32, f32::max);
    assert!(moved > 0.01, "{moved}");
    // A ranged re-render equals a full one.
    let full = mite_dsp::melody::render(&x, &a, &s.edits());
    let worst = out
        .iter()
        .zip(&full)
        .map(|(o, f)| (o - f).abs())
        .fold(0.0f32, f32::max);
    assert!(worst < 1e-4, "{worst}");
}

#[test]
fn bypass_plays_the_input() {
    let s = session("bypass");
    let mut e = PitchEngine::new(FS as f32);
    let x = take();
    transfer(&mut e, &s, &x, 0);
    s.set_edit(
        0,
        NoteEdit {
            shift_cents: 300.0,
            ..NoteEdit::default()
        },
    );
    settle(&mut e, &s);
    let (mut l, mut r) = (vec![0.25f32; 512], vec![0.25f32; 512]);
    e.process(
        &mut [&mut l, &mut r],
        mite_pitch::dsp::Transport {
            playing: true,
            pos: Some(4000),
        },
        true,
        &s,
    );
    assert!(l.iter().all(|v| *v == 0.25));
}

#[test]
fn a_jump_ends_the_recording() {
    let s = session("jump");
    let mut e = PitchEngine::new(FS as f32);
    let x = take();
    s.set_armed(true);
    play(&mut e, &s, &x[..24_000], 0, 512);
    // The transport jumps back (a loop): the take is what came before.
    play(&mut e, &s, &x[..4096], 0, 512);
    assert!(s.poll());
    assert!(s.wait_idle(std::time::Duration::from_secs(30)));
    assert_eq!(s.take().expect("a take").len, 24_000);
}

#[test]
fn the_take_and_its_edits_come_back_from_the_state() {
    let s = session("reload");
    let mut e = PitchEngine::new(FS as f32);
    let x = take();
    transfer(&mut e, &s, &x, 5000);
    s.set_edit(
        2,
        NoteEdit {
            shift_cents: -150.0,
            gain_db: -3.0,
            ..NoteEdit::default()
        },
    );
    settle(&mut e, &s);
    let json = s.to_json().expect("state");
    let before = play(&mut e, &s, &vec![0.0; x.len() + 5000], 0, 512);
    // A fresh instance (a project reload), reading the same folder.
    let t = session("reload-2");
    t.set_dir(s.dir());
    t.load_json(&json);
    assert!(t.wait_idle(std::time::Duration::from_secs(30)));
    assert_eq!(t.status(), Status::Ready);
    assert_eq!(t.edits(), s.edits());
    let mut e2 = PitchEngine::new(FS as f32);
    settle(&mut e2, &t);
    let after = play(&mut e2, &t, &vec![0.0; x.len() + 5000], 0, 512);
    // (Within rounding: the first instance re-rendered only the edited stretch.)
    let worst = before
        .iter()
        .zip(&after)
        .map(|(b, a)| (b - a).abs())
        .fold(0.0f32, f32::max);
    assert!(worst < 1e-4, "the reloaded take plays the same: {worst}");
    // A missing file says so, and plays nothing.
    let u = session("missing");
    u.load_json(&json);
    assert!(u.wait_idle(std::time::Duration::from_secs(30)));
    assert!(matches!(u.status(), Status::Problem(_)));
}
