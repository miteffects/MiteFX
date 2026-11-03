//! The Limit editor driven through events, against a stand-in host: the ceiling's grip, dragging
//! the waveform for gain (one undo step each), the wheel, double-click resets, picking a style
//! (Custom brings its SLOW dial), the oversampling list, the loudness reset and the keys; the
//! loudness target list, Custom's target dial and the platform preview.

use std::sync::atomic::Ordering;
use std::sync::Arc;

use mite_limit::editor::{memory_host, LimitView};
use mite_limit::params::{ATTACK, BYPASS, DELTA, TRUE_PEAK, CEILING, GAIN, LOOKAHEAD, OVERSAMPLING, PREVIEW, RELEASE, SLOW, STYLE, TARGET, TARGET_CUSTOM};
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

#[test]
fn a_target_is_picked_from_its_list_and_custom_brings_its_dial() {
    let (mut view, host, _) = setup();
    assert!(view.custom_dial(&host).is_none());
    let at = view.target_button().center();
    click(&mut view, &host, at);
    let rows = view.target_rows();
    assert_eq!(rows.len(), 9);
    click(&mut view, &host, rows[6].center());
    assert_eq!(plain(&host, TARGET.id), 6.0, "EBU R128");
    assert!(view.target_rows().is_empty(), "the list closes");
    let at = view.target_button().center();
    click(&mut view, &host, at);
    let rows = view.target_rows();
    click(&mut view, &host, rows[8].center());
    let dial = view.custom_dial(&host).expect("Custom shows its dial");
    view.event(&UiEvent::PointerMove { pos: dial, mods: MODS }, &host);
    view.event(&UiEvent::Wheel { pos: dial, lines: 2.0, mods: MODS }, &host);
    assert!((plain(&host, TARGET_CUSTOM.id) + 13.0).abs() < 0.01, "{}", plain(&host, TARGET_CUSTOM.id));
}

#[test]
fn the_preview_button_switches_the_platform_preview() {
    let (mut view, host, _) = setup();
    let at = view.preview_button().center();
    click(&mut view, &host, at);
    assert_eq!(plain(&host, PREVIEW.id), 1.0);
    click(&mut view, &host, at);
    assert_eq!(plain(&host, PREVIEW.id), 0.0);
}

#[test]
fn the_gain_shape_is_the_limiters_and_its_parts_are_handles() {
    let (mut view, host, _) = setup();
    view.update_shape(&host);
    // The probe burst is 6 dB over: the shape holds −6 dB through it, rests at 0 before the ramp.
    assert!((view.shape_gain_at(20.0) + 6.0).abs() < 0.05);
    assert!(view.shape_gain_at(-5.0).abs() < 1e-3);
    let y = view.shape_rect().center().y;
    // The ramp: drag left for a longer lookahead (one undo step).
    let ramp = Point::new(view.shape_x(-1.0), y);
    drag(&mut view, &host, ramp, ramp - (50.0, 0.0), MODS);
    assert!((plain(&host, LOOKAHEAD.id) - 3.0).abs() < 0.02, "{}", plain(&host, LOOKAHEAD.id));
    undo(&mut view, &host);
    assert_eq!(plain(&host, LOOKAHEAD.id), 2.0);
    // The tail: drag right for a slower release; the shape follows.
    view.update_shape(&host);
    let before = view.shape_gain_at(150.0);
    let tail = Point::new(view.shape_x(300.0), y);
    drag(&mut view, &host, tail, tail + (120.0, 0.0), MODS);
    let release = plain(&host, RELEASE.id);
    assert!((release - 100.0 * std::f32::consts::E).abs() < 3.0, "{release}");
    view.update_shape(&host);
    assert!(view.shape_gain_at(150.0) < before - 0.5, "a slower release holds the gain down longer");
    // The burst: attack; double-click resets.
    let burst = Point::new(view.shape_x(25.0), y);
    drag(&mut view, &host, burst, burst - (120.0, 0.0), MODS);
    assert!(plain(&host, ATTACK.id) < 15.0);
    view.event(&UiEvent::DoubleClick { pos: burst, mods: MODS }, &host);
    assert_eq!(plain(&host, ATTACK.id), 30.0);
}

#[test]
fn the_footer_toggles_switch_on_and_back_off() {
    // User report (2026-11-03): bypass turned on but would not turn off.
    let (mut view, host, _) = setup();
    for (id, default) in [(BYPASS.id, 0.0), (DELTA.id, 0.0), (TRUE_PEAK.id, 1.0)] {
        let at = view.footer_toggle(id).expect("drawn").center();
        click(&mut view, &host, at);
        assert_eq!(plain(&host, id), 1.0 - default, "{id} first click");
        view.tick(&host);
        click(&mut view, &host, at);
        assert_eq!(plain(&host, id), default, "{id} second click");
    }
}
