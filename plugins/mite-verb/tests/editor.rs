//! The Verb editor driven through events, against a stand-in host: no band by default; the decay
//! EQ's Base grip and bands (the curve passes through where they are dropped; one undo step
//! each), their shapes from the right-click menu; the tone EQ's bands; the layer pills; the
//! ducking threshold; the dial groups; Freeze in the footer.

use std::sync::Arc;

use mite_dsp::reverb::decay_at;
use mite_ui::kurbo::Point;
use mite_ui::{DrawCx, Key, MemoryParamHost, Modifiers, MouseButton, ParamHost, Theme, ThemeKind, UiEvent, View};
use mite_verb::editor::{memory_host, settings, Layer, Node, VerbView};
use mite_verb::params::{ids, DECAY, FREEZE};
use mite_verb::VerbShared;

const MODS: Modifiers = Modifiers { shift: false, alt: false, ctrl: false, meta: false };

fn plain(host: &MemoryParamHost, id: &str) -> f32 {
    host.plain(id, host.normalized(id))
}

fn setup() -> (VerbView, MemoryParamHost) {
    let shared = Arc::new(VerbShared::new());
    let dir = std::env::temp_dir().join(format!("mitefx-verb-editor-{}", std::process::id()));
    let mut view = VerbView::new(shared).with_preset_dir(dir);
    let host = memory_host();
    view.tick(&host);
    draw(&mut view, &host);
    (view, host)
}

fn draw(view: &mut VerbView, host: &MemoryParamHost) {
    let theme = Theme::load(ThemeKind::Dark, mite_verb::editor::ACCENT);
    let mut text = mite_ui::Typesetter::new();
    view.draw(&mut DrawCx { scene: &mut mite_ui::vello::Scene::new(), theme: &theme, text: &mut text, host, size: view.logical_size(), fx: &mut Default::default() });
}

fn drag(view: &mut VerbView, host: &MemoryParamHost, from: Point, to: Point) {
    view.event(&UiEvent::PointerMove { pos: from, mods: MODS }, host);
    view.event(&UiEvent::PointerDown { pos: from, button: MouseButton::Left, mods: MODS }, host);
    for k in 1..=8 {
        view.event(&UiEvent::PointerMove { pos: from.lerp(to, f64::from(k) / 8.0), mods: MODS }, host);
    }
    view.event(&UiEvent::PointerUp { pos: to, button: MouseButton::Left, mods: MODS }, host);
    draw(view, host);
}

fn undo(view: &mut VerbView, host: &MemoryParamHost) {
    view.event(&UiEvent::KeyDown { key: Key::Character('z'), mods: Modifiers { ctrl: true, ..MODS } }, host);
}

fn click(view: &mut VerbView, host: &MemoryParamHost, at: Point, button: MouseButton) {
    view.event(&UiEvent::PointerMove { pos: at, mods: MODS }, host);
    view.event(&UiEvent::PointerDown { pos: at, button, mods: MODS }, host);
    view.event(&UiEvent::PointerUp { pos: at, button, mods: MODS }, host);
    draw(view, host);
}

fn double_click(view: &mut VerbView, host: &MemoryParamHost, at: Point) {
    view.event(&UiEvent::PointerMove { pos: at, mods: MODS }, host);
    view.event(&UiEvent::DoubleClick { pos: at, mods: MODS }, host);
    draw(view, host);
}

/// Right-clicks `at` and picks row `row` of the shape menu.
fn pick_shape(view: &mut VerbView, host: &MemoryParamHost, at: Point, row: usize) {
    view.event(&UiEvent::PointerMove { pos: at, mods: MODS }, host);
    view.event(&UiEvent::PointerDown { pos: at, button: MouseButton::Right, mods: MODS }, host);
    draw(view, host);
    let rows = view.menu_rows();
    assert!(rows.len() > row, "menu rows {}", rows.len());
    click(view, host, rows[row].center(), MouseButton::Left);
    assert!(view.menu_rows().is_empty());
}

#[test]
fn no_band_is_on_by_default() {
    let host = memory_host();
    for id in ids().iter().filter(|id| id.ends_with("_on")) {
        assert_eq!(plain(&host, id), 0.0, "{id}");
    }
    // So the decay is the same everywhere.
    let s = settings(&host);
    for f in [50.0, 1000.0, 12_000.0] {
        assert!((decay_at(&s, f, 48_000.0) / 2.0 - 1.0).abs() < 0.01, "{f} Hz");
    }
}

#[test]
fn decay_is_a_dial_and_the_curve_has_no_grip_until_a_band_is_made() {
    let (mut view, host) = setup();
    // Nothing on the flat curve to grab: a press there is not a drag.
    let at = Point::new(600.0, view.decay_y(2.0));
    view.event(&UiEvent::PointerMove { pos: at, mods: MODS }, &host);
    view.event(&UiEvent::PointerDown { pos: at, button: MouseButton::Left, mods: MODS }, &host);
    view.event(&UiEvent::PointerMove { pos: Point::new(600.0, view.decay_y(5.0)), mods: MODS }, &host);
    view.event(&UiEvent::PointerUp { pos: at, button: MouseButton::Left, mods: MODS }, &host);
    assert!((plain(&host, DECAY.id) - DECAY.default).abs() < 1e-4);
    // The DECAY dial sets it.
    let dial = view.dial(DECAY.id).expect("decay dial");
    drag(&mut view, &host, dial, Point::new(dial.x, dial.y - 60.0));
    assert!(plain(&host, DECAY.id) > DECAY.default * 1.5, "{}", plain(&host, DECAY.id));
    undo(&mut view, &host);
    assert!((plain(&host, DECAY.id) - DECAY.default).abs() < 1e-4);
}

#[test]
fn a_double_click_makes_a_decay_bell_through_the_pointer_and_another_removes_it() {
    let (mut view, host) = setup();
    let at = Point::new(400.0, view.decay_y(0.8));
    double_click(&mut view, &host, at);
    assert_eq!(plain(&host, "d1_on"), 1.0);
    assert_eq!(plain(&host, "d1_type"), 0.0);
    assert!(plain(&host, "d1_mult") < 0.6, "{}", plain(&host, "d1_mult"));
    let p = view.node(&host, Node::Decay(0));
    assert!((p.x - at.x).abs() < 2.0 && (p.y - at.y).abs() < 3.0, "{p:?} against {at:?}");
    // Dragged up: the decay there rises past the base.
    let to = Point::new(p.x + 40.0, view.decay_y(4.0));
    drag(&mut view, &host, p, to);
    assert!(plain(&host, "d1_mult") > 1.5, "{}", plain(&host, "d1_mult"));
    // The wheel narrows it.
    let p = view.node(&host, Node::Decay(0));
    let q = plain(&host, "d1_q");
    view.event(&UiEvent::PointerMove { pos: p, mods: MODS }, &host);
    view.event(&UiEvent::Wheel { pos: p, lines: 3.0, mods: MODS }, &host);
    assert!(plain(&host, "d1_q") > q);
    double_click(&mut view, &host, p);
    assert_eq!(plain(&host, "d1_on"), 0.0);
}

#[test]
fn a_decay_band_takes_any_shape_from_its_menu() {
    let (mut view, host) = setup();
    let at = Point::new(900.0, view.decay_y(0.7));
    double_click(&mut view, &host, at);
    let p = view.node(&host, Node::Decay(0));
    // High Shelf: above it the whole top end rings shorter.
    pick_shape(&mut view, &host, p, 2);
    assert_eq!(plain(&host, "d1_type"), 2.0);
    let s = settings(&host);
    assert!(decay_at(&s, 15_000.0, 48_000.0) < decay_at(&s, 200.0, 48_000.0) * 0.6);
    // Remove (the last row).
    let p = view.node(&host, Node::Decay(0));
    pick_shape(&mut view, &host, p, 4);
    assert_eq!(plain(&host, "d1_on"), 0.0);
}

#[test]
fn a_right_click_on_the_plot_makes_a_band_of_the_picked_shape() {
    let (mut view, host) = setup();
    // A low shelf, ringing longer under 200 Hz.
    let at = Point::new(250.0, view.decay_y(4.0));
    pick_shape(&mut view, &host, at, 1);
    assert_eq!(plain(&host, "d1_on"), 1.0);
    assert_eq!(plain(&host, "d1_type"), 1.0);
    let s = settings(&host);
    assert!(decay_at(&s, 30.0, 48_000.0) > 3.0, "{}", decay_at(&s, 30.0, 48_000.0));
}

#[test]
fn eight_decay_bands_at_most() {
    let (mut view, host) = setup();
    for k in 0..9 {
        let at = Point::new(80.0 + 120.0 * f64::from(k), view.decay_y(1.0));
        double_click(&mut view, &host, at);
    }
    for b in 1..=8 {
        assert_eq!(plain(&host, &format!("d{b}_on")), 1.0, "band {b}");
    }
    assert_eq!(plain(&host, "e1_on"), 0.0);
}

#[test]
fn the_tone_layer_makes_eq_bands_of_every_shape() {
    let (mut view, host) = setup();
    assert_eq!(view.layer(), Layer::Decay);
    let pill = view.layer_pill(Layer::Tone).center();
    click(&mut view, &host, pill, MouseButton::Left);
    assert_eq!(view.layer(), Layer::Tone);
    // A double-click: a bell at the pointer's gain.
    let at = Point::new(600.0, view.tone_y(-6.0));
    double_click(&mut view, &host, at);
    assert_eq!(plain(&host, "e1_on"), 1.0);
    assert_eq!(plain(&host, "e1_type"), 0.0);
    assert!((plain(&host, "e1_gain") + 6.0).abs() < 0.3, "{}", plain(&host, "e1_gain"));
    assert_eq!(plain(&host, "d1_on"), 0.0, "the decay EQ is left alone");
    // Dragged: frequency and gain.
    let p = view.node(&host, Node::Tone(0));
    let to = Point::new(p.x + 100.0, view.tone_y(3.0));
    drag(&mut view, &host, p, to);
    assert!((plain(&host, "e1_gain") - 3.0).abs() < 0.3);
    // A right-click on the plot: a low cut; the wheel steps its slope.
    let at = Point::new(120.0, view.tone_y(0.0) + 40.0);
    pick_shape(&mut view, &host, at, 3);
    assert_eq!(plain(&host, "e2_type"), 3.0);
    assert_eq!(plain(&host, "e2_gain"), 0.0);
    let p = view.node(&host, Node::Tone(1));
    let slope = plain(&host, "e2_slope");
    view.event(&UiEvent::PointerMove { pos: p, mods: MODS }, &host);
    view.event(&UiEvent::Wheel { pos: p, lines: 1.0, mods: MODS }, &host);
    assert_eq!(plain(&host, "e2_slope"), slope + 1.0);
    // Its menu offers every EQ shape: Notch.
    pick_shape(&mut view, &host, p, 5);
    assert_eq!(plain(&host, "e2_type"), 5.0);
    // Grabbing a decay band goes back to the decay layer.
    host.set_plain("d1_on", 1.0);
    draw(&mut view, &host);
    let band = view.node(&host, Node::Decay(0));
    drag(&mut view, &host, band, band);
    assert_eq!(view.layer(), Layer::Decay);
}

#[test]
fn the_dials_sit_in_their_groups() {
    let (view, _host) = setup();
    let groups = view.dial_groups();
    let names: Vec<&str> = groups.iter().map(|g| g.0).collect();
    assert_eq!(names, ["SPACE", "DUCK", "EARLY", "SHIMMER", "MOD"]);
    let member = [("SPACE", &["size", "decay", "diffusion", "width", "predelay"][..]), ("MOD", &["mod_depth", "mod_rate"]), ("EARLY", &["early", "early_size", "early_send"]), ("SHIMMER", &["shimmer", "shimmer_pitch", "shimmer_low", "shimmer_high"]), ("DUCK", &["duck", "duck_thresh", "duck_release"])];
    for (name, members) in member {
        let rect = groups.iter().find(|g| g.0 == name).expect("group").1;
        for id in members {
            let c = view.dial(id).expect("dial");
            assert!(rect.contains(c), "{id} outside {name}");
        }
    }
    for (i, a) in groups.iter().enumerate() {
        for b in &groups[i + 1..] {
            assert!(a.1.intersect(b.1).area() == 0.0, "{} overlaps {}", a.0, b.0);
        }
    }
}


#[test]
fn freeze_toggles_from_the_footer() {
    let (mut view, host) = setup();
    let r = view.footer_toggle(FREEZE.id).expect("freeze toggle");
    for want in [1.0, 0.0] {
        view.event(&UiEvent::PointerMove { pos: r.center(), mods: MODS }, &host);
        view.event(&UiEvent::PointerDown { pos: r.center(), button: MouseButton::Left, mods: MODS }, &host);
        view.event(&UiEvent::PointerUp { pos: r.center(), button: MouseButton::Left, mods: MODS }, &host);
        draw(&mut view, &host);
        assert_eq!(plain(&host, FREEZE.id), want);
    }
}
