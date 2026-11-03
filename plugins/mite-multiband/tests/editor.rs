//! The Multiband editor driven through events: splitting by double-clicking the rail, dragging a
//! band's square (threshold across, ratio up, one undo step), the quadrants (picking one turns the
//! band on, the lit one again off), the band's power, solo and mute, and the footer toggles.

use std::sync::Arc;

use mite_multiband::editor::{memory_host, MbView};
use mite_multiband::params::{band_id, cross_id, B_BYPASS, B_ON, B_QUAD, B_RATIO, B_THRESH, BYPASS, DELTA, X_ON};
use mite_multiband::MbShared;
use mite_ui::kurbo::Point;
use mite_ui::{DrawCx, Key, MemoryParamHost, Modifiers, MouseButton, ParamHost, Theme, ThemeKind, UiEvent, View};

const MODS: Modifiers = Modifiers { shift: false, alt: false, ctrl: false, meta: false };

fn plain(host: &MemoryParamHost, id: &str) -> f32 {
    host.plain(id, host.normalized(id))
}

fn setup() -> (MbView, MemoryParamHost) {
    let dir = std::env::temp_dir().join(format!("mitefx-mb-editor-{}", std::process::id()));
    let mut view = MbView::new(Arc::new(MbShared::new())).with_preset_dir(dir);
    let host = memory_host();
    draw(&mut view, &host);
    (view, host)
}

fn draw(view: &mut MbView, host: &MemoryParamHost) {
    view.settle(host);
    let theme = Theme::load(ThemeKind::Dark, mite_multiband::editor::ACCENT);
    let mut text = mite_ui::Typesetter::new();
    view.draw(&mut DrawCx { scene: &mut mite_ui::vello::Scene::new(), theme: &theme, text: &mut text, host, size: view.logical_size(), fx: &mut Default::default() });
}

fn drag(view: &mut MbView, host: &MemoryParamHost, from: Point, to: Point) {
    view.event(&UiEvent::PointerMove { pos: from, mods: MODS }, host);
    view.event(&UiEvent::PointerDown { pos: from, button: MouseButton::Left, mods: MODS }, host);
    for k in 1..=8 {
        view.event(&UiEvent::PointerMove { pos: from.lerp(to, f64::from(k) / 8.0), mods: MODS }, host);
    }
    view.event(&UiEvent::PointerUp { pos: to, button: MouseButton::Left, mods: MODS }, host);
    draw(view, host);
}

fn click(view: &mut MbView, host: &MemoryParamHost, at: Point) {
    view.event(&UiEvent::PointerMove { pos: at, mods: MODS }, host);
    view.event(&UiEvent::PointerDown { pos: at, button: MouseButton::Left, mods: MODS }, host);
    view.event(&UiEvent::PointerUp { pos: at, button: MouseButton::Left, mods: MODS }, host);
    draw(view, host);
}

#[test]
fn double_clicking_the_rail_splits_into_bands() {
    let (mut view, host) = setup();
    let at = Point::new(view.rail_x(1000.0), view.rail_y());
    view.event(&UiEvent::PointerMove { pos: at, mods: MODS }, &host);
    view.event(&UiEvent::DoubleClick { pos: at, mods: MODS }, &host);
    draw(&mut view, &host);
    assert!((0..5).any(|k| plain(&host, &cross_id(k, X_ON.id)) == 1.0), "a crossover appears");
}

#[test]
fn the_square_sets_threshold_and_ratio_as_one_step() {
    let (mut view, host) = setup();
    let sq = view.band_square(&host, 0).expect("one band");
    let c = sq.center();
    drag(&mut view, &host, c, c + (40.0, -60.0));
    let (t, r) = (plain(&host, &band_id(0, B_THRESH.id)), plain(&host, &band_id(0, B_RATIO.id)));
    assert!((t - (-20.0 + 6.0)).abs() < 0.2, "threshold {t}");
    assert!((r - 4.0).abs() < 0.1, "ratio {r}");
    view.event(&UiEvent::KeyDown { key: Key::Character('z'), mods: Modifiers { ctrl: true, ..MODS } }, &host);
    assert_eq!(plain(&host, &band_id(0, B_THRESH.id)), -20.0);
    assert!((plain(&host, &band_id(0, B_RATIO.id)) - 2.0).abs() < 1e-4, "one undo takes it all back");
}

#[test]
fn quadrants_power_and_footer_toggles() {
    let (mut view, host) = setup();
    let up = view.quad_button(&host, 0, 2).expect("drawn");
    click(&mut view, &host, up.center());
    assert_eq!(plain(&host, &band_id(0, B_QUAD.id)), 2.0);
    click(&mut view, &host, up.center());
    assert_eq!(plain(&host, &band_id(0, B_ON.id)), 0.0, "the lit quadrant again: off");
    let power = view.band_power(&host, 0).expect("drawn");
    click(&mut view, &host, power.center());
    assert_eq!(plain(&host, &band_id(0, B_BYPASS.id)), 1.0);
    for id in [DELTA.id, BYPASS.id] {
        let at = view.footer_toggle(id).expect("drawn").center();
        click(&mut view, &host, at);
        assert_eq!(plain(&host, id), 1.0, "{id} on");
        click(&mut view, &host, at);
        assert_eq!(plain(&host, id), 0.0, "{id} off");
    }
}
