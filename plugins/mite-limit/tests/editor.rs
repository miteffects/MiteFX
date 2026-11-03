//! The Limit editor driven through events, against a stand-in host: the ceiling's grip, dragging
//! the waveform for gain (one undo step each), the wheel, double-click resets, picking a style
//! (Custom brings its SLOW dial), the oversampling list, the loudness reset and the keys.

use std::sync::atomic::Ordering;
use std::sync::Arc;

use mite_limit::editor::{memory_host, LimitView};
use mite_limit::params::{CEILING, GAIN, OVERSAMPLING, SLOW, STYLE};
use mite_limit::LimitShared;
use mite_ui::kurbo::Point;
use mite_ui::{DrawCx, Key, MemoryParamHost, Modifiers, MouseButton, ParamHost, Theme, ThemeKind, UiEvent, View};

const MODS: Modifiers = Modifiers { shift: false, alt: false, ctrl: false, meta: false };

fn plain(host: &MemoryParamHost, id: &str) -> f32 {
    host.plain(id, host.normalized(id))
}

fn setup() -> (LimitView, MemoryParamHost, Arc<LimitShared>) {
    let shared = Arc::new(LimitShared::new());
    let dir = std::env::temp_dir().join(format!("mitefx-limit-editor-{}", std::process::id()));
    let mut view = LimitView::new(shared.clone()).with_preset_dir(dir);
    let host = memory_host();
    view.tick(&host);
    view.settle();
    draw(&mut view, &host);
    (view, host, shared)
}

fn draw(view: &mut LimitView, host: &MemoryParamHost) {
    let theme = Theme::load(ThemeKind::Dark, mite_limit::editor::ACCENT);
    let mut text = mite_ui::Typesetter::new();
    view.draw(&mut DrawCx { scene: &mut mite_ui::vello::Scene::new(), theme: &theme, text: &mut text, host, size: view.logical_size(), fx: &mut Default::default() });
}

fn drag(view: &mut LimitView, host: &MemoryParamHost, from: Point, to: Point, mods: Modifiers) {
    view.event(&UiEvent::PointerMove { pos: from, mods }, host);
    view.event(&UiEvent::PointerDown { pos: from, button: MouseButton::Left, mods }, host);
    for k in 1..=8 {
        let t = f64::from(k) / 8.0;
        view.event(&UiEvent::PointerMove { pos: from.lerp(to, t), mods }, host);
    }
    view.event(&UiEvent::PointerUp { pos: to, button: MouseButton::Left, mods }, host);
    draw(view, host);
}

fn click(view: &mut LimitView, host: &MemoryParamHost, at: Point) {
    view.event(&UiEvent::PointerMove { pos: at, mods: MODS }, host);
    view.event(&UiEvent::PointerDown { pos: at, button: MouseButton::Left, mods: MODS }, host);
    view.event(&UiEvent::PointerUp { pos: at, button: MouseButton::Left, mods: MODS }, host);
    draw(view, host);
}

fn undo(view: &mut LimitView, host: &MemoryParamHost) {
    view.event(&UiEvent::KeyDown { key: Key::Character('z'), mods: Modifiers { ctrl: true, ..MODS } }, host);
}

#[test]
fn the_ceiling_grip_follows_the_pointer_at_the_level_scale() {
    let (mut view, host, _) = setup();
    let grip = view.ceiling_grip(&host).center();
    let to = Point::new(grip.x, view.level_y(-6.0));
    drag(&mut view, &host, grip, to, MODS);
    let c = plain(&host, CEILING.id);
    assert!((c + 6.0).abs() < 0.05, "ceiling {c}");
    // The grip is drawn where the value is.
    assert!((view.ceiling_grip(&host).center().y - to.y).abs() < 1.0);
    undo(&mut view, &host);
    assert_eq!(plain(&host, CEILING.id), -1.0, "one undo takes the whole drag back");
}

#[test]
fn dragging_the_waveform_sets_the_gain_and_the_music_follows_the_pointer() {
    let (mut view, host, _) = setup();
    let from = Point::new(400.0, view.level_y(-12.0));
    let to = Point::new(420.0, view.level_y(-3.0));
    drag(&mut view, &host, from, to, MODS);
    let g = plain(&host, GAIN.id);
    assert!((g - 9.0).abs() < 0.1, "gain {g}");
    // Shift is fine.
    drag(&mut view, &host, to, from, Modifiers { shift: true, ..MODS });
    assert!((plain(&host, GAIN.id) - (9.0 - 1.8)).abs() < 0.1);
    // Double-click resets.
    view.event(&UiEvent::DoubleClick { pos: from, mods: MODS }, &host);
    assert_eq!(plain(&host, GAIN.id), 0.0);
    // Gain never goes under 0 dB.
    drag(&mut view, &host, to, from, MODS);
    assert_eq!(plain(&host, GAIN.id), 0.0);
}

#[test]
fn the_wheel_and_double_click_on_the_grip() {
    let (mut view, host, _) = setup();
    let grip = view.ceiling_grip(&host).center();
    view.event(&UiEvent::PointerMove { pos: grip, mods: MODS }, &host);
    view.event(&UiEvent::Wheel { pos: grip, lines: -3.0, mods: MODS }, &host);
    assert!((plain(&host, CEILING.id) + 1.3).abs() < 0.01, "{}", plain(&host, CEILING.id));
    let grip = view.ceiling_grip(&host).center();
    view.event(&UiEvent::DoubleClick { pos: grip, mods: MODS }, &host);
    assert_eq!(plain(&host, CEILING.id), -1.0);
    view.event(&UiEvent::Wheel { pos: Point::new(300.0, 300.0), lines: 4.0, mods: MODS }, &host);
    assert_eq!(plain(&host, GAIN.id), 2.0);
}

#[test]
fn a_style_is_a_click_and_custom_brings_its_slow_dial() {
    let (mut view, host, _) = setup();
    assert!(view.dial(SLOW.id).is_none());
    let at = view.style_button(2).center();
    click(&mut view, &host, at);
    assert_eq!(plain(&host, STYLE.id), 2.0);
    let at = view.style_button(5).center();
    click(&mut view, &host, at);
    assert_eq!(plain(&host, STYLE.id), 5.0);
    assert!(view.dial(SLOW.id).is_some());
}

#[test]
fn the_oversampling_list_and_the_loudness_reset() {
    let (mut view, host, shared) = setup();
    view.open_oversampling(&host);
    draw(&mut view, &host);
    let rows = view.oversampling_rows();
    assert_eq!(rows.len(), 5);
    click(&mut view, &host, rows[4].center());
    assert_eq!(plain(&host, OVERSAMPLING.id), 4.0);
    let at = view.loudness_rect().center();
    click(&mut view, &host, at);
    assert!(shared.loudness.reset.load(Ordering::Relaxed));
}

#[test]
fn arrow_keys_nudge_the_gain() {
    let (mut view, host, _) = setup();
    view.event(&UiEvent::KeyDown { key: Key::Up, mods: MODS }, &host);
    view.event(&UiEvent::KeyDown { key: Key::Up, mods: Modifiers { shift: true, ..MODS } }, &host);
    assert!((plain(&host, GAIN.id) - 1.1).abs() < 1e-4);
}
