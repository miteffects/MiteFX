//! The DeEss editor driven through events, against a stand-in host: the band's edges and the band
//! dragged, the threshold grip, the pills, double-click back to defaults, the groups.

use std::sync::Arc;

use mite_deess::editor::{memory_host, DeEssView, Grip, Pill};
use mite_deess::params::*;
use mite_deess::DeEssShared;
use mite_dsp::deess::{Detection, Reduce};
use mite_ui::kurbo::Point;
use mite_ui::{DrawCx, Key, MemoryParamHost, Modifiers, MouseButton, ParamHost, Theme, ThemeKind, UiEvent, View};

const MODS: Modifiers = Modifiers { shift: false, alt: false, ctrl: false, meta: false };

fn plain(host: &MemoryParamHost, id: &str) -> f32 {
    host.plain(id, host.normalized(id))
}

fn setup() -> (DeEssView, MemoryParamHost) {
    let dir = std::env::temp_dir().join(format!("mitefx-deess-editor-{}", std::process::id()));
    let mut view = DeEssView::new(Arc::new(DeEssShared::new())).with_preset_dir(dir);
    let host = memory_host();
    view.tick(&host);
    draw(&mut view, &host);
    (view, host)
}

fn draw(view: &mut DeEssView, host: &MemoryParamHost) {
    let theme = Theme::load(ThemeKind::Dark, mite_deess::editor::ACCENT);
    let mut text = mite_ui::Typesetter::new();
    view.draw(&mut DrawCx { scene: &mut mite_ui::vello::Scene::new(), theme: &theme, text: &mut text, host, size: view.logical_size(), fx: &mut Default::default() });
}

fn drag(view: &mut DeEssView, host: &MemoryParamHost, from: Point, to: Point) {
    view.event(&UiEvent::PointerMove { pos: from, mods: MODS }, host);
    view.event(&UiEvent::PointerDown { pos: from, button: MouseButton::Left, mods: MODS }, host);
    for k in 1..=8 {
        view.event(&UiEvent::PointerMove { pos: from.lerp(to, f64::from(k) / 8.0), mods: MODS }, host);
    }
    view.event(&UiEvent::PointerUp { pos: to, button: MouseButton::Left, mods: MODS }, host);
    draw(view, host);
}

fn click(view: &mut DeEssView, host: &MemoryParamHost, at: Point) {
    view.event(&UiEvent::PointerMove { pos: at, mods: MODS }, host);
    view.event(&UiEvent::PointerDown { pos: at, button: MouseButton::Left, mods: MODS }, host);
    view.event(&UiEvent::PointerUp { pos: at, button: MouseButton::Left, mods: MODS }, host);
    draw(view, host);
}

#[test]
fn the_band_edges_follow_the_pointer() {
    let (mut view, host) = setup();
    let p = view.grip(&host, Grip::Lo);
    let to = Point::new(view.freq_x(4000.0), p.y);
    drag(&mut view, &host, p, to);
    assert!((plain(&host, LO.id) / 4000.0 - 1.0).abs() < 0.02, "{}", plain(&host, LO.id));
    let p = view.grip(&host, Grip::Hi);
    let to = Point::new(view.freq_x(15_000.0), p.y);
    drag(&mut view, &host, p, to);
    assert!((plain(&host, HI.id) / 15_000.0 - 1.0).abs() < 0.02);
    // The band itself moves both, by the same ratio.
    let p = view.grip(&host, Grip::Band);
    let to = Point::new(p.x - (view.freq_x(4000.0) - view.freq_x(2000.0)), p.y);
    drag(&mut view, &host, p, to);
    assert!((plain(&host, LO.id) / 2000.0 - 1.0).abs() < 0.03, "{}", plain(&host, LO.id));
    assert!((plain(&host, HI.id) / 7500.0 - 1.0).abs() < 0.03, "{}", plain(&host, HI.id));
    // One undo step: the band back.
    view.event(&UiEvent::KeyDown { key: Key::Character('z'), mods: Modifiers { ctrl: true, ..MODS } }, &host);
    assert!((plain(&host, LO.id) / 4000.0 - 1.0).abs() < 0.02);
}

#[test]
fn an_edge_cannot_pass_the_other() {
    let (mut view, host) = setup();
    let p = view.grip(&host, Grip::Lo);
    let to = Point::new(view.freq_x(19_000.0), p.y);
    drag(&mut view, &host, p, to);
    assert!(plain(&host, LO.id) * 1.2 <= plain(&host, HI.id) + 1.0);
}

#[test]
fn the_threshold_grip_moves_the_threshold_and_double_click_puts_it_back() {
    let (mut view, host) = setup();
    let p = view.grip(&host, Grip::Threshold);
    let to = Point::new(p.x, p.y + (view.level_y(-10.0) - view.level_y(0.0)));
    drag(&mut view, &host, p, to);
    assert!((plain(&host, THRESHOLD.id) + 25.0).abs() < 0.3, "{}", plain(&host, THRESHOLD.id));
    let p = view.grip(&host, Grip::Threshold);
    view.event(&UiEvent::PointerMove { pos: p, mods: MODS }, &host);
    view.event(&UiEvent::DoubleClick { pos: p, mods: MODS }, &host);
    assert_eq!(plain(&host, THRESHOLD.id), THRESHOLD.default);
}

#[test]
fn the_pills_set_detection_reduction_and_lookahead() {
    let (mut view, host) = setup();
    let at = view.pill(Pill::Detection(Detection::Absolute)).center();
    click(&mut view, &host, at);
    assert_eq!(plain(&host, DETECTION.id), 1.0);
    let at = view.pill(Pill::Reduce(Reduce::Band)).center();
    click(&mut view, &host, at);
    assert_eq!(plain(&host, REDUCE.id), 1.0);
    let at = view.pill(Pill::Lookahead).center();
    click(&mut view, &host, at);
    assert_eq!(plain(&host, LOOKAHEAD.id), 0.0);
}

#[test]
fn the_dials_and_pills_sit_in_their_groups() {
    let (view, _) = setup();
    let groups = view.dial_groups();
    let names: Vec<&str> = groups.iter().map(|(n, _)| *n).collect();
    assert_eq!(names, ["DETECT", "REDUCE", "REDUCTION"]);
    assert!(groups[0].1.contains(view.dial(THRESHOLD.id).expect("threshold")));
    assert!(groups[1].1.contains(view.dial(RANGE.id).expect("range")));
    assert!(groups[1].1.contains(view.dial(RELEASE.id).expect("release")));
    for pill in [Pill::Detection(Detection::Relative), Pill::Lookahead] {
        assert!(groups[0].1.contains(view.pill(pill).center()));
    }
    for r in [Reduce::Shelf, Reduce::Band, Reduce::Wide] {
        let rect = view.pill(Pill::Reduce(r));
        assert!(groups[1].1.contains(rect.center()));
        assert!(!rect.contains(view.dial(RANGE.id).expect("range")));
    }
}
