//! The Pitch editor against a real take: dragging a note retunes it (snapped, Alt free),
//! double-click centres it, undo and redo, the transfer button arms.

mod common;

use common::*;
use mite_pitch::PitchShared;
use mite_pitch::dsp::PitchEngine;
use mite_pitch::editor::{PitchView, Tool, memory_host};
use mite_pitch::session::Snap;
use mite_pitch::session::Status;
use mite_ui::kurbo::Point;
use mite_ui::{
    DrawCx, Key, MemoryParamHost, Modifiers, MouseButton, Theme, ThemeKind, UiEvent, View,
};
use std::sync::Arc;

const MODS: Modifiers = Modifiers {
    shift: false,
    alt: false,
    ctrl: false,
    meta: false,
};

fn draw(view: &mut PitchView, host: &MemoryParamHost) {
    let theme = Theme::load(ThemeKind::Dark, mite_pitch::editor::ACCENT);
    let mut text = mite_ui::Typesetter::new();
    view.draw(&mut DrawCx {
        scene: &mut mite_ui::vello::Scene::new(),
        theme: &theme,
        text: &mut text,
        host,
        size: view.logical_size(),
        fx: &mut Default::default(),
    });
}

fn ready(tag: &str) -> (PitchView, MemoryParamHost, Arc<PitchShared>) {
    let s = session(tag);
    let mut e = PitchEngine::new(FS as f32);
    transfer(&mut e, &s, &take(), 0);
    let shared = Arc::new(PitchShared {
        session: s,
        sample_rate: mite_core::AtomicF32::new(FS as f32),
    });
    let host = memory_host();
    let mut view = PitchView::new(shared.clone());
    view.tick(&host);
    draw(&mut view, &host);
    (view, host, shared)
}

fn drag(view: &mut PitchView, host: &MemoryParamHost, from: Point, to: Point, mods: Modifiers) {
    view.event(&UiEvent::PointerMove { pos: from, mods }, host);
    view.event(
        &UiEvent::PointerDown {
            pos: from,
            button: MouseButton::Left,
            mods,
        },
        host,
    );
    view.event(&UiEvent::PointerMove { pos: to, mods }, host);
    view.event(
        &UiEvent::PointerUp {
            pos: to,
            button: MouseButton::Left,
            mods,
        },
        host,
    );
}

#[test]
fn dragging_a_note_retunes_it_in_semitones() {
    let (mut view, host, shared) = ready("ed-drag");
    let s = &shared.session;
    let a = s.analysis().expect("analysis");
    let r = view.note_rect(1).expect("in view");
    let row = r.height() / 0.8;
    drag(
        &mut view,
        &host,
        r.center(),
        r.center() - (0.0, 2.2 * row),
        MODS,
    );
    assert_eq!(view.selected(), vec![1]);
    let e = s.edits()[1];
    let landed = a.notes[1].cents + e.shift_cents;
    assert!(
        (landed / 100.0 - (landed / 100.0).round()).abs() < 1e-9,
        "on a semitone: {landed}"
    );
    assert!(
        (e.shift_cents - (200.0 + (a.notes[1].cents / 100.0).round() * 100.0 - a.notes[1].cents))
            .abs()
            < 1e-6,
        "{}",
        e.shift_cents
    );
    // The others untouched.
    assert!(
        s.edits()
            .iter()
            .enumerate()
            .all(|(i, e)| i == 1 || e.is_neutral())
    );
    // Alt: free.
    let r = view.note_rect(1).expect("in view");
    let alt = Modifiers { alt: true, ..MODS };
    drag(
        &mut view,
        &host,
        r.center(),
        r.center() + (0.0, 0.37 * row),
        alt,
    );
    let landed = a.notes[1].cents + s.edits()[1].shift_cents;
    assert!(
        (landed / 100.0 - (landed / 100.0).round()).abs() > 0.05,
        "{landed}"
    );
    // Undo twice: as sung; redo once.
    let undo = |view: &mut PitchView, shift| {
        view.event(
            &UiEvent::KeyDown {
                key: Key::Character('z'),
                mods: Modifiers {
                    ctrl: true,
                    shift,
                    ..MODS
                },
            },
            &host,
        )
    };
    undo(&mut view, false);
    undo(&mut view, false);
    assert!(s.edits()[1].is_neutral());
    undo(&mut view, true);
    let landed = a.notes[1].cents + s.edits()[1].shift_cents;
    assert!((landed / 100.0 - (landed / 100.0).round()).abs() < 1e-9);
    assert!(s.wait_idle(std::time::Duration::from_secs(30)));
}

#[test]
fn double_click_centres_a_note() {
    let (mut view, host, shared) = ready("ed-centre");
    let s = &shared.session;
    let a = s.analysis().expect("analysis");
    // Note 2 was sung 15 cents sharp of its semitone.
    let r = view.note_rect(2).expect("in view");
    view.event(
        &UiEvent::PointerMove {
            pos: r.center(),
            mods: MODS,
        },
        &host,
    );
    view.event(
        &UiEvent::DoubleClick {
            pos: r.center(),
            mods: MODS,
        },
        &host,
    );
    let landed = a.notes[2].cents + s.edits()[2].shift_cents;
    assert!(
        (landed - (a.notes[2].cents / 100.0).round() * 100.0).abs() < 1e-6,
        "{landed}"
    );
    draw(&mut view, &host);
    assert!(s.wait_idle(std::time::Duration::from_secs(30)));
}

#[test]
fn transfer_arms_and_disarms() {
    let s = session("ed-arm");
    let shared = Arc::new(PitchShared {
        session: s.clone(),
        sample_rate: mite_core::AtomicF32::new(FS as f32),
    });
    let host = memory_host();
    let mut view = PitchView::new(shared);
    draw(&mut view, &host);
    let at = view.transfer_button().center();
    let click = |view: &mut PitchView| {
        view.event(
            &UiEvent::PointerMove {
                pos: at,
                mods: MODS,
            },
            &host,
        );
        view.event(
            &UiEvent::PointerDown {
                pos: at,
                button: MouseButton::Left,
                mods: MODS,
            },
            &host,
        );
        view.event(
            &UiEvent::PointerUp {
                pos: at,
                button: MouseButton::Left,
                mods: MODS,
            },
            &host,
        );
    };
    click(&mut view);
    assert_eq!(s.status(), Status::Armed);
    draw(&mut view, &host);
    click(&mut view);
    assert_eq!(s.status(), Status::Empty);
    draw(&mut view, &host);
}

fn click(view: &mut PitchView, host: &MemoryParamHost, at: Point, mods: Modifiers) {
    view.event(&UiEvent::PointerMove { pos: at, mods }, host);
    view.event(&UiEvent::PointerDown { pos: at, button: MouseButton::Left, mods }, host);
    view.event(&UiEvent::PointerUp { pos: at, button: MouseButton::Left, mods }, host);
}

fn settle(shared: &PitchShared) {
    assert!(shared.session.wait_idle(std::time::Duration::from_secs(30)));
}

#[test]
fn free_snap_moves_by_the_cent_and_scale_snap_stays_in_the_key() {
    let (mut view, host, shared) = ready("ed-snap");
    let s = &shared.session;
    let a = s.analysis().expect("analysis");
    let at = view.snap_button(Snap::Off).center();
    click(&mut view, &host, at, MODS);
    assert_eq!(s.scale().snap, Snap::Off);
    let r = view.note_rect(0).expect("in view");
    let px = 37.0 / view.cents_per_px();
    drag(&mut view, &host, r.center(), r.center() - (0.0, px), MODS);
    assert!((s.edits()[0].shift_cents - 37.0).abs() < 0.5, "{}", s.edits()[0].shift_cents);
    // Shift: ten times finer.
    let r = view.note_rect(0).expect("in view");
    drag(&mut view, &host, r.center(), r.center() - (0.0, px), Modifiers { shift: true, ..MODS });
    assert!((s.edits()[0].shift_cents - 40.7).abs() < 0.5, "{}", s.edits()[0].shift_cents);
    // Scale snap, C major: a note dragged onto C# lands on C or D.
    let mut sc = s.scale();
    sc.tonic = 0;
    sc.minor = false;
    sc.snap = Snap::Scale;
    s.set_scale(sc);
    let r = view.note_rect(0).expect("in view");
    let to = 6100.0 - (a.notes[0].cents + s.edits()[0].shift_cents);
    let up = to / view.cents_per_px();
    drag(&mut view, &host, r.center(), r.center() - (0.0, up), MODS);
    let landed = a.notes[0].cents + s.edits()[0].shift_cents;
    assert!((landed - 6000.0).abs() < 1e-6 || (landed - 6200.0).abs() < 1e-6, "{landed}");
    settle(&shared);
}

#[test]
fn sideways_moves_the_note_and_edges_stretch_it() {
    let (mut view, host, shared) = ready("ed-time");
    let s = &shared.session;
    let r = view.note_rect(1).expect("in view");
    drag(&mut view, &host, r.center(), r.center() + (40.0, 1.0), MODS);
    let e = s.edits()[1];
    assert!(e.offset > 0.0 && e.shift_cents == 0.0, "{e:?}");
    // The right edge: longer, the start where it was.
    let r = view.note_rect(1).expect("in view");
    drag(&mut view, &host, Point::new(r.x1 - 2.0, r.center().y), Point::new(r.x1 + 30.0, r.center().y), MODS);
    let r2 = view.note_rect(1).expect("in view");
    assert!((r2.x0 - r.x0).abs() < 0.5 && r2.x1 > r.x1 + 25.0, "{r:?} → {r2:?}");
    // The left edge: the end stays.
    drag(&mut view, &host, Point::new(r2.x0 + 2.0, r2.center().y), Point::new(r2.x0 + 20.0, r2.center().y), MODS);
    let r3 = view.note_rect(1).expect("in view");
    assert!((r3.x1 - r2.x1).abs() < 0.5 && r3.x0 > r2.x0 + 15.0, "{r2:?} → {r3:?}");
    settle(&shared);
}

#[test]
fn the_amount_tools_and_double_click_back() {
    let (mut view, host, shared) = ready("ed-amount");
    let s = &shared.session;
    for (tool, key) in [(Tool::Vibrato, '2'), (Tool::Drift, '3'), (Tool::Formant, '4'), (Tool::Level, '5')] {
        view.event(&UiEvent::KeyDown { key: Key::Character(key), mods: MODS }, &host);
        assert_eq!(view.tool(), tool);
        let r = view.note_rect(2).expect("in view");
        drag(&mut view, &host, r.center(), r.center() + (0.0, 50.0), MODS);
        let e = s.edits()[2];
        match tool {
            Tool::Vibrato => assert!((e.modulation - 0.5).abs() < 1e-6, "{e:?}"),
            Tool::Drift => assert!((e.drift - 0.5).abs() < 1e-6, "{e:?}"),
            Tool::Formant => assert!(e.formant_semitones < -3.0, "{e:?}"),
            Tool::Level => assert!(e.gain_db < -8.0, "{e:?}"),
            _ => {}
        }
        view.event(&UiEvent::DoubleClick { pos: r.center(), mods: MODS }, &host);
    }
    assert!(s.edits()[2].is_neutral(), "{:?}", s.edits()[2]);
    draw(&mut view, &host);
    settle(&shared);
}

#[test]
fn split_and_join_with_undo() {
    let (mut view, host, shared) = ready("ed-split");
    let s = &shared.session;
    let n = s.analysis().expect("analysis").notes.len();
    let at = view.tool_button(Tool::Split).center();
    click(&mut view, &host, at, MODS);
    let r = view.note_rect(2).expect("in view");
    click(&mut view, &host, r.center(), MODS);
    assert_eq!(s.analysis().expect("analysis").notes.len(), n + 1);
    assert_eq!(s.edits().len(), n + 1);
    draw(&mut view, &host);
    // Where the halves meet: join.
    let r = view.note_rect(2).expect("in view");
    click(&mut view, &host, Point::new(r.x1, r.center().y), MODS);
    assert_eq!(s.analysis().expect("analysis").notes.len(), n);
    // Undo: split again; undo again: as transferred.
    let undo = |view: &mut PitchView| view.event(&UiEvent::KeyDown { key: Key::Character('z'), mods: Modifiers { ctrl: true, ..MODS } }, &host);
    undo(&mut view);
    assert_eq!(s.analysis().expect("analysis").notes.len(), n + 1);
    undo(&mut view);
    assert_eq!(s.analysis().expect("analysis").notes.len(), n);
    settle(&shared);
    // The split comes back from the state.
    s.split(1, (s.analysis().expect("a").notes[1].start + s.analysis().expect("a").notes[1].end) / 2);
    settle(&shared);
    let json = s.to_json().expect("state");
    let t = session("ed-split-2");
    t.set_dir(s.dir());
    t.load_json(&json);
    assert!(t.wait_idle(std::time::Duration::from_secs(30)));
    assert_eq!(t.analysis().expect("analysis").bounds(), s.analysis().expect("analysis").bounds());
    assert_eq!(t.scale(), s.scale());
}

#[test]
fn the_correct_macro_pulls_the_selection_onto_the_snap() {
    let (mut view, host, shared) = ready("ed-macro");
    let s = &shared.session;
    let a = s.analysis().expect("analysis");
    // Nothing selected: every note; half way.
    let sl = view.centre_slider();
    let mid = Point::new(sl.x0 + 0.5 * sl.width(), sl.center().y);
    drag(&mut view, &host, Point::new(sl.x0, sl.center().y), mid, MODS);
    for (n, e) in a.notes.iter().zip(s.edits()) {
        let off = n.cents - (n.cents / 100.0).round() * 100.0;
        assert!((e.shift_cents + 0.5 * off).abs() < 1e-6, "{off} {}", e.shift_cents);
    }
    // Drift, all the way, on a selection of one.
    let at = view.note_rect(3).expect("in view").center();
    click(&mut view, &host, at, MODS);
    let sl = view.drift_slider();
    drag(&mut view, &host, Point::new(sl.x0, sl.center().y), Point::new(sl.x1 + 20.0, sl.center().y), MODS);
    let e = s.edits();
    assert!(e[3].drift.abs() < 1e-9 && e[2].drift == 1.0, "{:?}", e);
    settle(&shared);
}

#[test]
fn the_transition_tool_sets_the_glide_from_the_pointer() {
    let (mut view, host, shared) = ready("ed-glide");
    let s = &shared.session;
    // Split note 2 so its halves are legato, move the second half up.
    let n2 = s.analysis().expect("analysis").notes[2].clone();
    assert!(s.split(2, (n2.start + n2.end) / 2));
    settle(&shared);
    view.tick(&host);
    let mut e = s.edits()[3];
    e.shift_cents = 200.0;
    s.set_edit(3, e);
    view.event(&UiEvent::KeyDown { key: Key::Character('6'), mods: MODS }, &host);
    assert_eq!(view.tool(), Tool::Transition);
    let r = view.note_rect(3).expect("in view");
    // From its start to 80 ms in.
    let x80 = view.x_of(s.analysis().expect("a").notes[3].start as f64 / FS + 0.08);
    drag(&mut view, &host, Point::new(r.x0 + 1.0, r.center().y), Point::new(x80, r.center().y), MODS);
    let ms = s.edits()[3].transition_ms;
    assert!((ms - 80.0).abs() < 2.0, "{ms}");
    draw(&mut view, &host);
    // Double-click: a step again.
    view.event(&UiEvent::DoubleClick { pos: r.center(), mods: MODS }, &host);
    assert_eq!(s.edits()[3].transition_ms, 0.0);
    settle(&shared);
}

#[test]
fn the_ruler_sets_the_cursor_and_the_loop_and_space_plays() {
    use std::sync::atomic::Ordering;
    let (mut view, host, shared) = ready("ed-transport");
    let s = &shared.session;
    let ruler = view.ruler();
    let at = Point::new(view.x_of(1.0), ruler.center().y);
    click(&mut view, &host, at, MODS);
    assert!((view.cue_s() - 1.0).abs() < 0.01, "{}", view.cue_s());
    // A drag: the loop, on.
    let (p0, p1) = (Point::new(view.x_of(0.5), ruler.center().y), Point::new(view.x_of(1.5), ruler.center().y));
    drag(&mut view, &host, p0, p1, MODS);
    assert!(s.loop_on.load(Ordering::Acquire));
    let (a, b) = (s.loop_start.load(Ordering::Acquire) as f64 / FS, s.loop_end.load(Ordering::Acquire) as f64 / FS);
    assert!((a - 0.5).abs() < 0.01 && (b - 1.5).abs() < 0.01, "{a} {b}");
    view.event(&UiEvent::KeyDown { key: Key::Character('l'), mods: MODS }, &host);
    assert!(!s.loop_on.load(Ordering::Acquire));
    view.event(&UiEvent::KeyDown { key: Key::Character(' '), mods: MODS }, &host);
    assert!(s.local_play.load(Ordering::Acquire));
    let at = view.play_button().center();
    click(&mut view, &host, at, MODS);
    assert!(!s.local_play.load(Ordering::Acquire));
    draw(&mut view, &host);
}
