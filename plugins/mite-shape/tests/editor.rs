//! The Shape editor driven through events, against a stand-in host: making bands by pulling a
//! crossover in or double-clicking the rail, merging them by dragging one crossover onto
//! another, shaping on the scope, solo, removing a band and undo.

use std::sync::Arc;

use mite_dsp::shape::Layout;
use mite_shape::editor::{memory_host, settings, ShapeView};
use mite_shape::params::{band_id, cross_id, B_ATTACK, B_SOLO, B_SUSTAIN, X_FREQ, X_ON};
use mite_shape::ShapeShared;
use mite_ui::kurbo::Point;
use mite_ui::{DrawCx, Key, MemoryParamHost, Modifiers, MouseButton, ParamHost, Theme, ThemeKind, UiEvent, View};

const MODS: Modifiers = Modifiers { shift: false, alt: false, ctrl: false, meta: false };
/// The rail's centre line, one band and several (display top 56 + 340 + 18, + 300 + 40).
const RAIL_SINGLE: f64 = 414.0;
const RAIL_MULTI: f64 = 396.0;

fn rail_x(f: f64) -> f64 {
    14.0 + 1152.0 * (f / 20.0).ln() / 1000f64.ln()
}

fn plain(host: &MemoryParamHost, id: &str) -> f32 {
    host.plain(id, host.normalized(id))
}

fn setup() -> (ShapeView, MemoryParamHost) {
    let shared = Arc::new(ShapeShared::new());
    let dir = std::env::temp_dir().join(format!("mitefx-shape-editor-{}", std::process::id()));
    let mut view = ShapeView::new(shared).with_preset_dir(dir);
    let host = memory_host();
    draw(&mut view, &host);
    (view, host)
}

/// Lays the view out (hit areas are set while drawing), with the layout settled.
fn draw(view: &mut ShapeView, host: &MemoryParamHost) {
    view.settle(host);
    let theme = Theme::load(ThemeKind::Dark, mite_shape::editor::ACCENT);
    let mut text = mite_ui::Typesetter::new();
    view.draw(&mut DrawCx { scene: &mut mite_ui::vello::Scene::new(), theme: &theme, text: &mut text, host, size: view.logical_size(), fx: &mut Default::default() });
}

fn drag(view: &mut ShapeView, host: &MemoryParamHost, from: Point, to: Point) {
    view.event(&UiEvent::PointerMove { pos: from, mods: MODS }, host);
    view.event(&UiEvent::PointerDown { pos: from, button: MouseButton::Left, mods: MODS }, host);
    for k in 1..=8 {
        let t = f64::from(k) / 8.0;
        view.event(&UiEvent::PointerMove { pos: from.lerp(to, t), mods: MODS }, host);
    }
    view.event(&UiEvent::PointerUp { pos: to, button: MouseButton::Left, mods: MODS }, host);
    draw(view, host);
}

fn double_click(view: &mut ShapeView, host: &MemoryParamHost, at: Point) {
    view.event(&UiEvent::PointerMove { pos: at, mods: MODS }, host);
    view.event(&UiEvent::DoubleClick { pos: at, mods: MODS }, host);
    draw(view, host);
}

fn click(view: &mut ShapeView, host: &MemoryParamHost, at: Point) {
    view.event(&UiEvent::PointerMove { pos: at, mods: MODS }, host);
    view.event(&UiEvent::PointerDown { pos: at, button: MouseButton::Left, mods: MODS }, host);
    view.event(&UiEvent::PointerUp { pos: at, button: MouseButton::Left, mods: MODS }, host);
    draw(view, host);
}

fn layout(host: &MemoryParamHost) -> Layout {
    Layout::new(&settings(host).crossovers)
}

#[test]
fn pulling_a_crossover_in_makes_a_band_with_the_same_settings() {
    let (mut view, host) = setup();
    host.set_plain(&band_id(0, B_ATTACK.id), 6.0);
    host.set_plain(&band_id(0, B_SUSTAIN.id), -4.0);
    draw(&mut view, &host);
    // From the ghost grip at the high end to 2 kHz.
    drag(&mut view, &host, Point::new(rail_x(20_000.0) - 6.0, RAIL_SINGLE), Point::new(rail_x(2000.0), RAIL_SINGLE));
    let l = layout(&host);
    assert_eq!(l.n, 1);
    let slot = l.crossovers[0];
    let f = plain(&host, &cross_id(slot, X_FREQ.id));
    assert!((f / 2000.0 - 1.0).abs() < 0.02, "{f}");
    // Both sides keep the band's settings, and the new one is selected.
    assert_eq!(plain(&host, &band_id(slot + 1, B_ATTACK.id)), 6.0);
    assert_eq!(plain(&host, &band_id(slot + 1, B_SUSTAIN.id)), -4.0);
    assert_eq!(view.selected(), slot + 1);
}

#[test]
fn double_clicking_the_rail_splits_until_five_bands() {
    let (mut view, host) = setup();
    for (k, f) in [100.0, 500.0, 2000.0, 8000.0, 15_000.0].into_iter().enumerate() {
        let y = if layout(&host).n == 0 { RAIL_SINGLE } else { RAIL_MULTI };
        double_click(&mut view, &host, Point::new(rail_x(f), y));
        assert_eq!(layout(&host).n, (k + 1).min(4), "after {f} Hz");
    }
}

#[test]
fn dragging_a_crossover_onto_its_neighbour_merges_and_the_growing_band_survives() {
    let (mut view, host) = setup();
    for (k, f) in [(0, 150.0), (1, 300.0), (2, 3000.0)] {
        host.set_plain(&cross_id(k, X_ON.id), 1.0);
        host.set_plain(&cross_id(k, X_FREQ.id), f);
    }
    // The band above 300 Hz (slot 2) has its own attack.
    host.set_plain(&band_id(2, B_ATTACK.id), 9.0);
    draw(&mut view, &host);
    assert_eq!(layout(&host).n, 3);
    // 300 Hz down onto 150 Hz: the band between (slot 1) goes, the one above grows down.
    drag(&mut view, &host, Point::new(rail_x(300.0), RAIL_MULTI), Point::new(rail_x(150.0) + 2.0, RAIL_MULTI));
    let l = layout(&host);
    assert_eq!(l.n, 2);
    assert_eq!(l.bands(), &[0, 2, 3]);
    let f = plain(&host, &cross_id(1, X_FREQ.id));
    assert!((f / 150.0 - 1.0).abs() < 0.01, "{f}");
    assert_eq!(plain(&host, &band_id(2, B_ATTACK.id)), 9.0);
}

#[test]
fn dragging_a_crossover_moves_it_and_keeps_it_clear_of_its_neighbours() {
    let (mut view, host) = setup();
    for (k, f) in [(0, 200.0), (1, 2000.0)] {
        host.set_plain(&cross_id(k, X_ON.id), 1.0);
        host.set_plain(&cross_id(k, X_FREQ.id), f);
    }
    draw(&mut view, &host);
    drag(&mut view, &host, Point::new(rail_x(2000.0), RAIL_MULTI), Point::new(rail_x(1000.0), RAIL_MULTI));
    let f = plain(&host, &cross_id(1, X_FREQ.id));
    assert!((f / 1000.0 - 1.0).abs() < 0.02, "{f}");
    // Close to (but not onto) the one below: pushed a third of an octave clear.
    drag(&mut view, &host, Point::new(rail_x(1000.0), RAIL_MULTI), Point::new(rail_x(210.0), RAIL_MULTI));
    assert_eq!(layout(&host).n, 2);
    let f = plain(&host, &cross_id(1, X_FREQ.id));
    assert!(f >= 200.0 * 2f32.powf(1.0 / 3.0) * 0.999, "{f}");
}

#[test]
fn the_scope_sets_attack_before_the_handover_and_sustain_after_it() {
    let (mut view, host) = setup();
    // Single band: the scope spans x 10…798 from y 450, 206 tall; 25 ms sits a quarter across.
    let attack_at = Point::new(80.0, 540.0);
    drag(&mut view, &host, attack_at, attack_at - (0.0, 30.0));
    let a = plain(&host, &band_id(0, B_ATTACK.id));
    assert!(a > 3.5 && a < 5.0, "{a}");
    let sustain_at = Point::new(600.0, 560.0);
    drag(&mut view, &host, sustain_at, sustain_at + (0.0, 40.0));
    let s = plain(&host, &band_id(0, B_SUSTAIN.id));
    assert!(s < -4.5 && s > -7.0, "{s}");
}

#[test]
fn solo_hears_one_band_and_removing_a_band_merges_it_down() {
    let (mut view, host) = setup();
    for (k, f) in [(0, 150.0), (1, 3500.0)] {
        host.set_plain(&cross_id(k, X_ON.id), 1.0);
        host.set_plain(&cross_id(k, X_FREQ.id), f);
    }
    view.select(1);
    draw(&mut view, &host);
    view.event(&UiEvent::KeyDown { key: Key::Character('s'), mods: MODS }, &host);
    assert_eq!(plain(&host, &band_id(1, B_SOLO.id)), 1.0);
    view.event(&UiEvent::KeyDown { key: Key::Character('s'), mods: MODS }, &host);
    assert_eq!(plain(&host, &band_id(1, B_SOLO.id)), 0.0);
    view.event(&UiEvent::KeyDown { key: Key::Delete, mods: MODS }, &host);
    draw(&mut view, &host);
    assert_eq!(layout(&host).bands(), &[0, 2]);
    assert_eq!(view.selected(), 0);
}

#[test]
fn one_undo_takes_a_split_away() {
    let (mut view, host) = setup();
    double_click(&mut view, &host, Point::new(rail_x(1000.0), RAIL_SINGLE));
    assert_eq!(layout(&host).n, 1);
    view.event(&UiEvent::KeyDown { key: Key::Character('z'), mods: Modifiers { ctrl: true, ..MODS } }, &host);
    assert_eq!(layout(&host).n, 0, "undone");
}

#[test]
fn a_click_on_a_band_selects_it() {
    let (mut view, host) = setup();
    for (k, f) in [(0, 150.0), (1, 3500.0)] {
        host.set_plain(&cross_id(k, X_ON.id), 1.0);
        host.set_plain(&cross_id(k, X_FREQ.id), f);
    }
    draw(&mut view, &host);
    click(&mut view, &host, Point::new(rail_x(8000.0), RAIL_MULTI + 30.0));
    assert_eq!(view.selected(), 2);
}

#[test]
fn the_clip_ceiling_drags_on_the_level_lane() {
    let (mut view, host) = setup();
    host.set_plain(mite_shape::params::CLIP.id, 1.0);
    draw(&mut view, &host);
    // Level lane (one band): −48…0 dB over y 68…250; the grip sits at the ceiling (−1 dB) at x 1127.
    let y = |db: f64| 68.0 + -db / 48.0 * 182.0;
    drag(&mut view, &host, Point::new(1127.0, y(-1.0)), Point::new(1127.0, y(-7.0)));
    let c = plain(&host, mite_shape::params::CLIP_CEILING.id);
    assert!((c + 7.0).abs() < 0.2, "{c}");
}

#[test]
fn the_scope_sets_the_sustain_length_and_its_right_edge_is_endless() {
    let (mut view, host) = setup();
    let id = band_id(0, mite_shape::params::B_LENGTH.id);
    // Endless: the handle sits at the scope's right edge (x 786, y 467); drag it to 200 ms.
    let x200 = 16.0 + 776.0 * ((25.0f64 + 200.0) / 400.0).sqrt();
    drag(&mut view, &host, Point::new(786.0, 467.0), Point::new(x200, 467.0));
    let v = plain(&host, &id);
    assert!((v - 200.0).abs() < 8.0, "{v}");
    drag(&mut view, &host, Point::new(x200, 467.0), Point::new(796.0, 467.0));
    assert_eq!(plain(&host, &id), mite_shape::params::B_LENGTH.max);
}
