//! The Delay editor driven through events, against a stand-in host: one tap by default; a tap's
//! grip moves its time (to notes while synced) and level, its second echo its feedback; taps
//! made and removed; the tap menu (sync, ping-pong); the machine and tap pills; the loop EQ's
//! bands; the dial groups.

use std::sync::Arc;

use mite_delay::editor::{memory_host, settings, DelayView, Node, Pill};
use mite_delay::params::*;
use mite_delay::{DelayShared, DEFAULT_TEMPO};
use mite_dsp::delay::{loop_gain, Mode};
use mite_ui::kurbo::Point;
use mite_ui::{DrawCx, Key, MemoryParamHost, Modifiers, MouseButton, ParamHost, Theme, ThemeKind, UiEvent, View};

const MODS: Modifiers = Modifiers { shift: false, alt: false, ctrl: false, meta: false };

fn plain(host: &MemoryParamHost, id: &str) -> f32 {
    host.plain(id, host.normalized(id))
}

fn setup() -> (DelayView, MemoryParamHost) {
    let shared = Arc::new(DelayShared::new());
    let dir = std::env::temp_dir().join(format!("mitefx-delay-editor-{}", std::process::id()));
    let mut view = DelayView::new(shared).with_preset_dir(dir);
    let host = memory_host();
    view.tick(&host);
    draw(&mut view, &host);
    (view, host)
}

fn draw(view: &mut DelayView, host: &MemoryParamHost) {
    let theme = Theme::load(ThemeKind::Dark, mite_delay::editor::ACCENT);
    let mut text = mite_ui::Typesetter::new();
    view.draw(&mut DrawCx { scene: &mut mite_ui::vello::Scene::new(), theme: &theme, text: &mut text, host, size: view.logical_size(), fx: &mut Default::default() });
}

fn drag(view: &mut DelayView, host: &MemoryParamHost, from: Point, to: Point) {
    view.event(&UiEvent::PointerMove { pos: from, mods: MODS }, host);
    view.event(&UiEvent::PointerDown { pos: from, button: MouseButton::Left, mods: MODS }, host);
    for k in 1..=8 {
        view.event(&UiEvent::PointerMove { pos: from.lerp(to, f64::from(k) / 8.0), mods: MODS }, host);
    }
    view.event(&UiEvent::PointerUp { pos: to, button: MouseButton::Left, mods: MODS }, host);
    draw(view, host);
}

fn click(view: &mut DelayView, host: &MemoryParamHost, at: Point, button: MouseButton) {
    view.event(&UiEvent::PointerMove { pos: at, mods: MODS }, host);
    view.event(&UiEvent::PointerDown { pos: at, button, mods: MODS }, host);
    view.event(&UiEvent::PointerUp { pos: at, button, mods: MODS }, host);
    draw(view, host);
}

fn double_click(view: &mut DelayView, host: &MemoryParamHost, at: Point) {
    view.event(&UiEvent::PointerMove { pos: at, mods: MODS }, host);
    view.event(&UiEvent::DoubleClick { pos: at, mods: MODS }, host);
    view.tick(host);
    draw(view, host);
}

fn undo(view: &mut DelayView, host: &MemoryParamHost) {
    view.event(&UiEvent::KeyDown { key: Key::Character('z'), mods: Modifiers { ctrl: true, ..MODS } }, host);
}

/// Right-clicks `at` and picks row `row` of its menu.
fn pick(view: &mut DelayView, host: &MemoryParamHost, at: Point, row: usize) {
    view.event(&UiEvent::PointerMove { pos: at, mods: MODS }, host);
    view.event(&UiEvent::PointerDown { pos: at, button: MouseButton::Right, mods: MODS }, host);
    draw(view, host);
    let rows = view.menu_rows();
    assert!(rows.len() > row, "menu rows {}", rows.len());
    click(view, host, rows[row].center(), MouseButton::Left);
    assert!(view.menu_rows().is_empty());
}

#[test]
fn one_tap_by_default_at_a_quarter_note() {
    let (view, host) = setup();
    let on: Vec<String> = ids().into_iter().filter(|id| id.ends_with("_on") && plain(&host, id) >= 0.5).collect();
    assert_eq!(on, ["t1_on"]);
    let p = view.node(&host, Node::Tap(0));
    assert!((p.x - view.time_x(0.5)).abs() < 0.5, "{p:?}");
    assert!((p.y - view.level_y(0.0)).abs() < 0.5);
}

#[test]
fn dragging_a_synced_tap_snaps_to_notes_and_sets_its_level() {
    let (mut view, host) = setup();
    let p = view.node(&host, Node::Tap(0));
    // To 375 ms, 6 dB down: a dotted eighth.
    let to = Point::new(view.time_x(0.37), view.level_y(-6.0));
    drag(&mut view, &host, p, to);
    assert_eq!(NOTES[plain(&host, "t1_note") as usize], "1/8.");
    assert!((plain(&host, "t1_level") + 6.0).abs() < 0.3, "{}", plain(&host, "t1_level"));
    assert_eq!(settings(&host, DEFAULT_TEMPO).taps[0].time_ms, 375.0);
    // One undo step puts it back.
    undo(&mut view, &host);
    assert_eq!(plain(&host, "t1_note") as usize, QUARTER);
    assert_eq!(plain(&host, "t1_level"), 0.0);
}

#[test]
fn a_free_tap_follows_the_pointer_in_ms() {
    let (mut view, host) = setup();
    host.set_plain("t1_sync", 0.0);
    draw(&mut view, &host);
    let p = view.node(&host, Node::Tap(0));
    let window = view.window();
    let to = Point::new(view.time_x(0.333), p.y);
    drag(&mut view, &host, p, to);
    // The window is held while dragging, so the time is where the pointer let go.
    assert!((plain(&host, "t1_time") - 333.0).abs() < 333.0 * 0.01, "{} (window {window})", plain(&host, "t1_time"));
}

#[test]
fn the_second_echo_sets_the_feedback() {
    let (mut view, host) = setup();
    let p = view.node(&host, Node::Feedback(0));
    // Its level 12 dB under the first echo's: feedback a quarter (the loop is flat).
    let to = Point::new(p.x, view.level_y(-12.0));
    drag(&mut view, &host, p, to);
    let s = settings(&host, DEFAULT_TEMPO);
    let h = loop_gain(&s, 0, 1000.0, 48_000.0) as f32;
    let fb = plain(&host, "t1_feedback") / 100.0 * h;
    assert!((20.0 * fb.log10() + 12.0).abs() < 0.3, "{fb}");
    let q = view.node(&host, Node::Feedback(0));
    assert!((q.y - view.level_y(-12.0)).abs() < 1.5, "{q:?}");
}

#[test]
fn double_click_makes_a_tap_and_removes_one() {
    let (mut view, host) = setup();
    let at = Point::new(view.time_x(0.25), view.level_y(-9.0));
    double_click(&mut view, &host, at);
    assert_eq!(plain(&host, "t2_on"), 1.0);
    assert_eq!(NOTES[plain(&host, "t2_note") as usize], "1/8");
    assert!((plain(&host, "t2_level") + 9.0).abs() < 0.3);
    assert_eq!(view.selected(), 1);
    // Four at most.
    for x in [0.1, 0.15, 0.2] {
        let at = Point::new(view.time_x(x), view.level_y(-20.0));
        double_click(&mut view, &host, at);
    }
    assert!((0..4).all(|t| plain(&host, &tap_id(t, "on")) == 1.0));
    // Double-click on a tap removes it.
    let p = view.node(&host, Node::Tap(1));
    double_click(&mut view, &host, p);
    assert_eq!(plain(&host, "t2_on"), 0.0);
}

#[test]
fn the_tap_menu_frees_its_time_and_makes_it_ping_pong() {
    let (mut view, host) = setup();
    let p = view.node(&host, Node::Tap(0));
    pick(&mut view, &host, p, 0);
    // Free, and nothing moved.
    assert_eq!(plain(&host, "t1_sync"), 0.0);
    assert!((plain(&host, "t1_time") - 500.0).abs() < 0.5);
    let p = view.node(&host, Node::Tap(0));
    pick(&mut view, &host, p, 1);
    assert_eq!(plain(&host, "t1_pingpong"), 1.0);
    // And back to synced: the nearest note.
    let p = view.node(&host, Node::Tap(0));
    pick(&mut view, &host, p, 0);
    assert_eq!(plain(&host, "t1_sync"), 1.0);
    assert_eq!(plain(&host, "t1_note") as usize, QUARTER);
    let p = view.node(&host, Node::Tap(0));
    pick(&mut view, &host, p, 2);
    assert_eq!(plain(&host, "t1_on"), 0.0);
}

#[test]
fn the_pills_pick_the_machine_and_switch_the_selected_tap() {
    let (mut view, host) = setup();
    let at = view.pill(Pill::Mode(Mode::Bbd)).center();
    click(&mut view, &host, at, MouseButton::Left);
    assert_eq!(plain(&host, MODE.id), 2.0);
    let at = view.pill(Pill::PingPong).center();
    click(&mut view, &host, at, MouseButton::Left);
    assert_eq!(plain(&host, "t1_pingpong"), 1.0);
    let at = view.pill(Pill::Sync).center();
    click(&mut view, &host, at, MouseButton::Left);
    assert_eq!(plain(&host, "t1_sync"), 0.0);
}

#[test]
fn the_wheel_on_a_tap_pans_it() {
    let (mut view, host) = setup();
    let p = view.node(&host, Node::Tap(0));
    view.event(&UiEvent::PointerMove { pos: p, mods: MODS }, &host);
    view.event(&UiEvent::Wheel { pos: p, lines: 3.0, mods: MODS }, &host);
    assert!(plain(&host, "t1_pan") > 10.0, "{}", plain(&host, "t1_pan"));
}

#[test]
fn loop_bands_are_made_dragged_and_shaped() {
    let (mut view, host) = setup();
    let at = Point::new(view.freq_x(2000.0), view.eq_y(6.0));
    double_click(&mut view, &host, at);
    assert_eq!(plain(&host, "e1_on"), 1.0);
    assert!((plain(&host, "e1_gain") - 6.0).abs() < 0.3);
    assert!((plain(&host, "e1_freq") / 2000.0 - 1.0).abs() < 0.03);
    let p = view.node(&host, Node::Band(0));
    let to = Point::new(view.freq_x(500.0), view.eq_y(-3.0));
    drag(&mut view, &host, p, to);
    assert!((plain(&host, "e1_freq") / 500.0 - 1.0).abs() < 0.03);
    assert!((plain(&host, "e1_gain") + 3.0).abs() < 0.3);
    // Right-click empty plot: a high cut.
    let at = Point::new(view.freq_x(8000.0), view.eq_y(0.0));
    pick(&mut view, &host, at, 4);
    assert_eq!(plain(&host, "e2_on"), 1.0);
    assert_eq!(plain(&host, "e2_type"), 4.0);
}

#[test]
fn the_dials_sit_in_their_groups() {
    let (view, _host) = setup();
    let groups = view.dial_groups();
    let names: Vec<&str> = groups.iter().map(|(n, _)| *n).collect();
    assert_eq!(names, ["MACHINE", "MOD", "DUCK", "SPACE", "TAP"]);
    let inside = |id: &str, group: &str| {
        let p = view.dial(id).unwrap_or_else(|| panic!("no dial {id}"));
        let r = groups.iter().find(|(n, _)| *n == group).expect("group").1;
        assert!(r.contains(p), "{id} at {p:?} outside {group} {r:?}");
    };
    inside(DRIVE.id, "MACHINE");
    inside(MOD_DEPTH.id, "MOD");
    inside(MOD_RATE.id, "MOD");
    inside(DUCK.id, "DUCK");
    inside(DUCK_THRESH.id, "DUCK");
    inside(DUCK_RELEASE.id, "DUCK");
    inside(DIFFUSION.id, "SPACE");
    inside(WIDTH.id, "SPACE");
    inside("t1_level", "TAP");
    inside("t1_feedback", "TAP");
    inside("t1_pan", "TAP");
    // The pills too, apart from each other and from the dials.
    for m in Mode::ALL {
        let r = view.pill(Pill::Mode(m));
        assert!(groups[0].1.contains(r.center()));
        assert!(!r.inflate(20.0, 20.0).contains(view.dial(DRIVE.id).expect("drive")));
    }
}

#[test]
fn grabbing_another_tap_selects_it() {
    let (mut view, host) = setup();
    let at = Point::new(view.time_x(0.125), view.level_y(-6.0));
    double_click(&mut view, &host, at);
    let first = view.node(&host, Node::Tap(0));
    click(&mut view, &host, first, MouseButton::Left);
    assert_eq!(view.selected(), 0);
    assert!(view.dial("t1_level").is_some() && view.dial("t2_level").is_none());
}
