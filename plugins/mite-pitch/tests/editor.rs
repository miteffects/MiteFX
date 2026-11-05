//! The Pitch editor against a real take: dragging a note retunes it (snapped, Alt free),
//! double-click centres it, undo and redo, the transfer button arms.

mod common;

use common::*;
use mite_pitch::PitchShared;
use mite_pitch::dsp::PitchEngine;
use mite_pitch::editor::{PitchView, memory_host};
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
    assert_eq!(view.selected(), Some(1));
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
