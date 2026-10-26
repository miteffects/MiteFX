//! The Sat editor driven through events, against a stand-in host: shaping the curve by dragging
//! it (one undo step), picking a model, the band's bypass, the wheel, resetting, the
//! oversampling list, and keys.

use std::sync::Arc;

use mite_dsp::sat::Model;
use mite_sat::editor::{memory_host, SatView};
use mite_sat::params::{band_id, B_BIAS, B_BYPASS, B_DRIVE, B_MODEL, B_TONE, OVERSAMPLING};
use mite_sat::SatShared;
use mite_ui::kurbo::Point;
use mite_ui::{DrawCx, Key, MemoryParamHost, Modifiers, MouseButton, ParamHost, Theme, ThemeKind, UiEvent, View};

const MODS: Modifiers = Modifiers { shift: false, alt: false, ctrl: false, meta: false };

fn plain(host: &MemoryParamHost, id: &str) -> f32 {
    host.plain(id, host.normalized(id))
}

fn setup() -> (SatView, MemoryParamHost) {
    let shared = Arc::new(SatShared::new());
    let dir = std::env::temp_dir().join(format!("mitefx-sat-editor-{}", std::process::id()));
    let mut view = SatView::new(shared).with_preset_dir(dir);
    let host = memory_host();
    draw(&mut view, &host);
    (view, host)
}

/// Lays the view out (hit areas are set while drawing).
fn draw(view: &mut SatView, host: &MemoryParamHost) {
    let theme = Theme::load(ThemeKind::Dark, mite_sat::editor::ACCENT);
    let mut text = mite_ui::Typesetter::new();
    view.draw(&mut DrawCx { scene: &mut mite_ui::vello::Scene::new(), theme: &theme, text: &mut text, host, size: view.logical_size(), fx: &mut Default::default() });
}

fn drag(view: &mut SatView, host: &MemoryParamHost, from: Point, to: Point, mods: Modifiers) {
    view.event(&UiEvent::PointerMove { pos: from, mods }, host);
    view.event(&UiEvent::PointerDown { pos: from, button: MouseButton::Left, mods }, host);
    for k in 1..=8 {
        let t = f64::from(k) / 8.0;
        view.event(&UiEvent::PointerMove { pos: from.lerp(to, t), mods }, host);
    }
    view.event(&UiEvent::PointerUp { pos: to, button: MouseButton::Left, mods }, host);
    draw(view, host);
}

fn click(view: &mut SatView, host: &MemoryParamHost, at: Point) {
    view.event(&UiEvent::PointerMove { pos: at, mods: MODS }, host);
    view.event(&UiEvent::PointerDown { pos: at, button: MouseButton::Left, mods: MODS }, host);
    view.event(&UiEvent::PointerUp { pos: at, button: MouseButton::Left, mods: MODS }, host);
    draw(view, host);
}

#[test]
fn dragging_the_curve_sets_drive_and_bias_as_one_undo_step() {
    let (mut view, host) = setup();
    let c = view.square().center();
    drag(&mut view, &host, c, c + (50.0, -100.0), MODS);
    let (drive, bias) = (plain(&host, &band_id(0, B_DRIVE.id)), plain(&host, &band_id(0, B_BIAS.id)));
    assert!((drive - 12.0).abs() < 0.5, "drive {drive}");
    assert!((bias - 35.0).abs() < 1.0, "bias {bias}");
    view.event(&UiEvent::KeyDown { key: Key::Character('z'), mods: Modifiers { ctrl: true, ..MODS } }, &host);
    assert_eq!(plain(&host, &band_id(0, B_DRIVE.id)), 0.0, "one undo takes the whole drag back");
    assert_eq!(plain(&host, &band_id(0, B_BIAS.id)), 0.0);
}

#[test]
fn alt_drag_sets_tone_and_shift_is_fine() {
    let (mut view, host) = setup();
    let c = view.square().center();
    drag(&mut view, &host, c, c + (0.0, -100.0), Modifiers { alt: true, ..MODS });
    assert!((plain(&host, &band_id(0, B_TONE.id)) - 6.0).abs() < 0.3);
    assert_eq!(plain(&host, &band_id(0, B_DRIVE.id)), 0.0, "Alt leaves drive alone");
    drag(&mut view, &host, c, c + (0.0, -100.0), Modifiers { shift: true, ..MODS });
    assert!((plain(&host, &band_id(0, B_DRIVE.id)) - 2.4).abs() < 0.2);
}

#[test]
fn the_wheel_drives_and_a_double_click_resets() {
    let (mut view, host) = setup();
    let c = view.square().center();
    view.event(&UiEvent::Wheel { pos: c, lines: 4.0, mods: MODS }, &host);
    assert!((plain(&host, &band_id(0, B_DRIVE.id)) - 2.0).abs() < 1e-4);
    host.set_plain(&band_id(0, B_BIAS.id), 40.0);
    view.event(&UiEvent::DoubleClick { pos: c, mods: MODS }, &host);
    assert_eq!(plain(&host, &band_id(0, B_DRIVE.id)), 0.0);
    assert_eq!(plain(&host, &band_id(0, B_BIAS.id)), 0.0);
}

#[test]
fn model_icons_pick_the_model_and_keys_step_through_them() {
    let (mut view, host) = setup();
    assert!(view.model_icon(Model::Tape).is_none(), "not offered until it is built");
    let fold = view.model_icon(Model::Fold).expect("offered").center();
    click(&mut view, &host, fold);
    assert_eq!(plain(&host, &band_id(0, B_MODEL.id)) as usize, Model::Fold.index());
    view.event(&UiEvent::KeyDown { key: Key::Right, mods: MODS }, &host);
    assert_eq!(plain(&host, &band_id(0, B_MODEL.id)) as usize, Model::Rectify.index());
    view.event(&UiEvent::KeyDown { key: Key::Up, mods: MODS }, &host);
    assert_eq!(plain(&host, &band_id(0, B_DRIVE.id)), 0.5);
}

#[test]
fn the_power_button_bypasses_the_band() {
    let (mut view, host) = setup();
    click(&mut view, &host, Point::new(1166.0 - 36.0, 56.0 + 252.0 + 30.0));
    assert_eq!(plain(&host, &band_id(0, B_BYPASS.id)), 1.0);
}

#[test]
fn the_oversampling_list_sets_the_factor() {
    let (mut view, host) = setup();
    click(&mut view, &host, Point::new(60.0, 704.0 + 28.0));
    let rows = view.oversampling_rows();
    assert_eq!(rows.len(), 4, "the list is open");
    click(&mut view, &host, rows[3].center());
    assert_eq!(plain(&host, OVERSAMPLING.id), 3.0, "8×");
    assert!(view.oversampling_rows().is_empty(), "and closed");
}
