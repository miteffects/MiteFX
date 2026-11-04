//! The Double editor driven through events, against a stand-in host: the pucks place the source
//! and the doubles (one undo step a drag), a ghost brings a double in, the count and follow pills,
//! the double's menu, the dial groups.

use std::sync::Arc;

use mite_double::editor::{memory_host, DoubleView, Pill, Puck};
use mite_double::params::*;
use mite_double::DoubleShared;
use mite_ui::kurbo::Point;
use mite_ui::{DrawCx, Key, MemoryParamHost, Modifiers, MouseButton, ParamHost, Theme, ThemeKind, UiEvent, View};

const MODS: Modifiers = Modifiers { shift: false, alt: false, ctrl: false, meta: false };

fn plain(host: &MemoryParamHost, id: &str) -> f32 {
    host.plain(id, host.normalized(id))
}

fn setup() -> (DoubleView, MemoryParamHost) {
    let dir = std::env::temp_dir().join(format!("mitefx-double-editor-{}", std::process::id()));
    let mut view = DoubleView::new(Arc::new(DoubleShared::new())).with_preset_dir(dir);
    let host = memory_host();
    view.tick(&host);
    draw(&mut view, &host);
    (view, host)
}

fn draw(view: &mut DoubleView, host: &MemoryParamHost) {
    let theme = Theme::load(ThemeKind::Dark, mite_double::editor::ACCENT);
    let mut text = mite_ui::Typesetter::new();
    view.draw(&mut DrawCx { scene: &mut mite_ui::vello::Scene::new(), theme: &theme, text: &mut text, host, size: view.logical_size(), fx: &mut Default::default() });
}

fn drag(view: &mut DoubleView, host: &MemoryParamHost, from: Point, to: Point) {
    view.event(&UiEvent::PointerMove { pos: from, mods: MODS }, host);
    view.event(&UiEvent::PointerDown { pos: from, button: MouseButton::Left, mods: MODS }, host);
    for k in 1..=8 {
        view.event(&UiEvent::PointerMove { pos: from.lerp(to, f64::from(k) / 8.0), mods: MODS }, host);
    }
    view.event(&UiEvent::PointerUp { pos: to, button: MouseButton::Left, mods: MODS }, host);
    draw(view, host);
}

fn click(view: &mut DoubleView, host: &MemoryParamHost, at: Point, button: MouseButton) {
    view.event(&UiEvent::PointerMove { pos: at, mods: MODS }, host);
    view.event(&UiEvent::PointerDown { pos: at, button, mods: MODS }, host);
    view.event(&UiEvent::PointerUp { pos: at, button, mods: MODS }, host);
    draw(view, host);
}

#[test]
fn the_source_and_a_double_are_placed_by_dragging() {
    let (mut view, host) = setup();
    let s = view.puck(&host, Puck::Source);
    let to = Point::new(view.pan_x(-0.4), view.db_y(-6.0));
    drag(&mut view, &host, s, to);
    assert!((plain(&host, SOURCE_PAN.id) + 40.0).abs() < 1.0, "{}", plain(&host, SOURCE_PAN.id));
    assert!((plain(&host, SOURCE_LEVEL.id) + 6.0).abs() < 0.3);
    let v = view.puck(&host, Puck::Voice(0));
    let to = Point::new(view.pan_x(0.7), view.db_y(-3.0));
    drag(&mut view, &host, v, to);
    assert!((plain(&host, "v1_pan") - 70.0).abs() < 1.0);
    assert!((plain(&host, "v1_level") + 3.0).abs() < 0.3);
    // One undo step: the double back where it was, the source still moved.
    view.event(&UiEvent::KeyDown { key: Key::Character('z'), mods: Modifiers { ctrl: true, ..MODS } }, &host);
    assert_eq!(plain(&host, "v1_pan"), 100.0);
    assert!((plain(&host, SOURCE_PAN.id) + 40.0).abs() < 1.0);
}

#[test]
fn a_ghost_brings_a_double_in_and_the_menu_takes_it_out() {
    let (mut view, host) = setup();
    assert_eq!(DoubleView::voices(&host), 1);
    let ghost = view.puck(&host, Puck::Voice(2));
    click(&mut view, &host, ghost, MouseButton::Left);
    assert_eq!(DoubleView::voices(&host), 3);
    // Right-click double 2: Remove takes it and the third out.
    let p = view.puck(&host, Puck::Voice(1));
    view.event(&UiEvent::PointerMove { pos: p, mods: MODS }, &host);
    view.event(&UiEvent::PointerDown { pos: p, button: MouseButton::Right, mods: MODS }, &host);
    draw(&mut view, &host);
    let rows = view.menu_rows();
    assert_eq!(rows.len(), 2);
    click(&mut view, &host, rows[1].center(), MouseButton::Left);
    assert_eq!(DoubleView::voices(&host), 1);
}

#[test]
fn double_click_puts_a_puck_back() {
    let (mut view, host) = setup();
    host.set_plain("v1_pan", 20.0);
    host.set_plain("v1_level", -12.0);
    draw(&mut view, &host);
    let p = view.puck(&host, Puck::Voice(0));
    view.event(&UiEvent::PointerMove { pos: p, mods: MODS }, &host);
    view.event(&UiEvent::DoubleClick { pos: p, mods: MODS }, &host);
    assert_eq!(plain(&host, "v1_pan"), 100.0);
    assert_eq!(plain(&host, "v1_level"), 0.0);
}

#[test]
fn the_pills_set_the_count_and_whether_takes_follow_the_notes() {
    let (mut view, host) = setup();
    let at = view.pill(Pill::Count(2)).center();
    click(&mut view, &host, at, MouseButton::Left);
    assert_eq!(DoubleView::voices(&host), 2);
    let at = view.pill(Pill::Follow).center();
    click(&mut view, &host, at, MouseButton::Left);
    assert_eq!(plain(&host, FOLLOW.id), 0.0);
}

#[test]
fn the_dials_sit_in_their_groups() {
    let (view, _) = setup();
    let groups = view.dial_groups();
    let names: Vec<&str> = groups.iter().map(|(n, _)| *n).collect();
    assert_eq!(names, ["TAKE", "CHORUS", "TONE", "DOUBLES"]);
    let inside = |id: &str, group: &str| {
        let p = view.dial(id).unwrap_or_else(|| panic!("no dial {id}"));
        let r = groups.iter().find(|(n, _)| *n == group).expect("group").1;
        assert!(r.contains(p), "{id} outside {group}");
    };
    for id in [DELAY.id, TIMING.id, PITCH.id, VARIATION.id] {
        inside(id, "TAKE");
    }
    inside(CHORUS.id, "CHORUS");
    inside(CHORUS_RATE.id, "CHORUS");
    inside(TONE.id, "TONE");
    let doubles = groups[3].1;
    for pill in [Pill::Count(1), Pill::Count(3), Pill::Follow] {
        assert!(doubles.contains(view.pill(pill).center()));
    }
}
