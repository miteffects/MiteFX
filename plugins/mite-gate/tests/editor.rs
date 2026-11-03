//! The Gate editor driven through events, against a stand-in host: the threshold's and the
//! close threshold's grips (one undo step each), the threshold node and the range in the
//! square, wheel and double-click, the modes (the expander brings RATIO), the detector, the
//! sidechain source list, the footer toggles both ways and the keys.

use std::sync::Arc;

use mite_gate::editor::{memory_host, GateView};
use mite_gate::params::{BYPASS, DELTA, DETECTOR, HYSTERESIS, LISTEN, MODE, RANGE, RATIO, SC_SOURCE, THRESHOLD};
use mite_gate::GateShared;
use mite_ui::kurbo::Point;
use mite_ui::{DrawCx, Key, MemoryParamHost, Modifiers, MouseButton, ParamHost, Theme, ThemeKind, UiEvent, View};

const MODS: Modifiers = Modifiers { shift: false, alt: false, ctrl: false, meta: false };

fn plain(host: &MemoryParamHost, id: &str) -> f32 {
    host.plain(id, host.normalized(id))
}

fn setup() -> (GateView, MemoryParamHost) {
    let shared = Arc::new(GateShared::new());
    let dir = std::env::temp_dir().join(format!("mitefx-gate-editor-{}", std::process::id()));
    let mut view = GateView::new(shared).with_preset_dir(dir);
    let host = memory_host();
    view.tick(&host);
    view.settle();
    draw(&mut view, &host);
    (view, host)
}

fn draw(view: &mut GateView, host: &MemoryParamHost) {
    let theme = Theme::load(ThemeKind::Dark, mite_gate::editor::ACCENT);
    let mut text = mite_ui::Typesetter::new();
    view.draw(&mut DrawCx { scene: &mut mite_ui::vello::Scene::new(), theme: &theme, text: &mut text, host, size: view.logical_size(), fx: &mut Default::default() });
}

fn drag(view: &mut GateView, host: &MemoryParamHost, from: Point, to: Point, mods: Modifiers) {
    view.event(&UiEvent::PointerMove { pos: from, mods }, host);
    view.event(&UiEvent::PointerDown { pos: from, button: MouseButton::Left, mods }, host);
    for k in 1..=8 {
        view.event(&UiEvent::PointerMove { pos: from.lerp(to, f64::from(k) / 8.0), mods }, host);
    }
    view.event(&UiEvent::PointerUp { pos: to, button: MouseButton::Left, mods }, host);
    draw(view, host);
}

fn click(view: &mut GateView, host: &MemoryParamHost, at: Point) {
    view.event(&UiEvent::PointerMove { pos: at, mods: MODS }, host);
    view.event(&UiEvent::PointerDown { pos: at, button: MouseButton::Left, mods: MODS }, host);
    view.event(&UiEvent::PointerUp { pos: at, button: MouseButton::Left, mods: MODS }, host);
    draw(view, host);
}

fn undo(view: &mut GateView, host: &MemoryParamHost) {
    view.event(&UiEvent::KeyDown { key: Key::Character('z'), mods: Modifiers { ctrl: true, ..MODS } }, host);
}

#[test]
fn the_threshold_grip_follows_the_pointer_at_the_level_scale() {
    let (mut view, host) = setup();
    let grip = view.threshold_grip(&host).center();
    let to = Point::new(grip.x, view.level_y(-25.0));
    drag(&mut view, &host, grip, to, MODS);
    assert!((plain(&host, THRESHOLD.id) + 25.0).abs() < 0.05, "{}", plain(&host, THRESHOLD.id));
    assert!((view.threshold_grip(&host).center().y - to.y).abs() < 1.0);
    undo(&mut view, &host);
    assert_eq!(plain(&host, THRESHOLD.id), -40.0);
}

#[test]
fn the_close_grip_sets_the_hysteresis() {
    let (mut view, host) = setup();
    let grip = view.close_grip(&host).center();
    let to = Point::new(grip.x, view.level_y(-50.0));
    drag(&mut view, &host, grip, to, MODS);
    assert!((plain(&host, HYSTERESIS.id) - 10.0).abs() < 0.05, "{}", plain(&host, HYSTERESIS.id));
    // The close grip is drawn at threshold − hysteresis.
    assert!((view.close_grip(&host).center().y - to.y).abs() < 1.0);
}

#[test]
fn in_the_square_the_node_is_the_threshold_and_the_rest_is_the_range() {
    let (mut view, host) = setup();
    let node = view.threshold_node(&host);
    let sq = view.square();
    drag(&mut view, &host, node, node + (sq.width() * 10.0 / 60.0, 0.0), MODS);
    assert!((plain(&host, THRESHOLD.id) + 30.0).abs() < 0.1, "{}", plain(&host, THRESHOLD.id));
    let empty = Point::new(sq.x1 - 30.0, sq.y1 - 30.0);
    drag(&mut view, &host, empty, empty - (0.0, sq.height() * 20.0 / 60.0), MODS);
    assert!((plain(&host, RANGE.id) + 60.0).abs() < 0.1, "{}", plain(&host, RANGE.id));
    view.event(&UiEvent::DoubleClick { pos: empty, mods: MODS }, &host);
    assert_eq!(plain(&host, RANGE.id), -80.0);
}

#[test]
fn modes_detector_and_the_expanders_ratio() {
    let (mut view, host) = setup();
    assert!(view.dial(RATIO.id).is_none());
    let at = view.mode_button(1).center();
    click(&mut view, &host, at);
    assert_eq!(plain(&host, MODE.id), 1.0);
    assert!(view.dial(RATIO.id).is_some());
    let at = view.detector_button(1).center();
    click(&mut view, &host, at);
    assert_eq!(plain(&host, DETECTOR.id), 1.0);
}

#[test]
fn the_source_list_and_the_footer_toggles_both_ways() {
    let (mut view, host) = setup();
    let at = view.source_button().center();
    click(&mut view, &host, at);
    let rows = view.source_rows();
    assert_eq!(rows.len(), 2);
    click(&mut view, &host, rows[1].center());
    assert_eq!(plain(&host, SC_SOURCE.id), 1.0);
    for id in [LISTEN.id, DELTA.id, BYPASS.id] {
        let at = view.footer_toggle(id).expect("drawn").center();
        click(&mut view, &host, at);
        assert_eq!(plain(&host, id), 1.0, "{id} on");
        click(&mut view, &host, at);
        assert_eq!(plain(&host, id), 0.0, "{id} off");
    }
}

#[test]
fn wheel_and_keys_move_the_threshold() {
    let (mut view, host) = setup();
    let grip = view.threshold_grip(&host).center();
    view.event(&UiEvent::PointerMove { pos: grip, mods: MODS }, &host);
    view.event(&UiEvent::Wheel { pos: grip, lines: 2.0, mods: MODS }, &host);
    assert_eq!(plain(&host, THRESHOLD.id), -39.0);
    view.event(&UiEvent::KeyDown { key: Key::Up, mods: MODS }, &host);
    assert_eq!(plain(&host, THRESHOLD.id), -38.0);
}
