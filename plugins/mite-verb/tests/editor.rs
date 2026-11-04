//! The Verb editor driven through events, against a stand-in host: the decay curve's three grips
//! (the curve passes through where they are dropped; one undo step each), the cuts, the ducking
//! threshold, double-click resets, the wheel and Freeze in the footer.

use std::sync::Arc;

use mite_dsp::reverb::decay_at;
use mite_ui::kurbo::Point;
use mite_ui::{DrawCx, Key, MemoryParamHost, Modifiers, MouseButton, ParamHost, Theme, ThemeKind, UiEvent, View};
use mite_verb::editor::{memory_host, settings, Node, VerbView};
use mite_verb::params::{DECAY, DUCK_THRESH, FREEZE, HI_CUT, HI_MULT, LO_FREQ, LO_MULT};
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

#[test]
fn the_mid_grip_puts_the_curve_through_the_pointer() {
    let (mut view, host) = setup();
    let grip = view.node(&host, Node::Mid);
    let to = Point::new(grip.x, view.decay_y(5.0));
    drag(&mut view, &host, grip, to);
    let s = settings(&host);
    let f = (f64::from(s.lo_freq) * f64::from(s.hi_freq)).sqrt();
    let t = decay_at(&s, f, 48_000.0);
    assert!((t / 5.0 - 1.0).abs() < 0.02, "{t}");
    assert!((view.node(&host, Node::Mid).y - to.y).abs() < 2.0);
    undo(&mut view, &host);
    assert!((plain(&host, DECAY.id) - DECAY.default).abs() < 1e-4);
}

#[test]
fn the_low_grip_moves_its_frequency_and_its_decay_in_one_step() {
    let (mut view, host) = setup();
    let grip = view.node(&host, Node::Low);
    // Reachable: at 4× (the most) the curve at the low frequency is about 3.2 s.
    let to = Point::new(grip.x + 80.0, view.decay_y(3.0));
    drag(&mut view, &host, grip, to);
    assert!(plain(&host, LO_FREQ.id) > 300.0, "{}", plain(&host, LO_FREQ.id));
    assert!(plain(&host, LO_MULT.id) > 1.5, "{}", plain(&host, LO_MULT.id));
    let at = view.node(&host, Node::Low);
    assert!((at.x - to.x).abs() < 2.0 && (at.y - to.y).abs() < 2.0, "{at:?} against {to:?}");
    undo(&mut view, &host);
    assert!((plain(&host, LO_FREQ.id) - LO_FREQ.default).abs() < 0.01);
    assert!((plain(&host, LO_MULT.id) - LO_MULT.default).abs() < 1e-4);
}

#[test]
fn the_high_grip_down_shortens_the_highs() {
    let (mut view, host) = setup();
    let grip = view.node(&host, Node::High);
    let to = Point::new(grip.x, view.decay_y(0.5));
    drag(&mut view, &host, grip, to);
    assert!(plain(&host, HI_MULT.id) < 0.3, "{}", plain(&host, HI_MULT.id));
    // Double-click resets both of its parameters.
    let at = view.node(&host, Node::High);
    view.event(&UiEvent::DoubleClick { pos: at, mods: MODS }, &host);
    assert!((plain(&host, HI_MULT.id) - HI_MULT.default).abs() < 1e-4);
}

#[test]
fn the_high_cut_follows_the_pointer_across() {
    let (mut view, host) = setup();
    let grip = view.cut_grip(&host, true).center();
    drag(&mut view, &host, grip, Point::new(grip.x - 200.0, grip.y));
    let f = plain(&host, HI_CUT.id);
    assert!((4000.0..12_000.0).contains(&f), "{f}");
    assert!((view.cut_grip(&host, true).center().x - (grip.x - 200.0)).abs() < 2.0);
}

#[test]
fn the_ducking_threshold_drags_at_the_history_scale() {
    let (mut view, host) = setup();
    let grip = view.duck_grip(&host).center();
    let to = Point::new(grip.x, view.level_y(-18.0));
    drag(&mut view, &host, grip, to);
    assert!((plain(&host, DUCK_THRESH.id) + 18.0).abs() < 0.2, "{}", plain(&host, DUCK_THRESH.id));
    // The wheel steps it.
    let at = view.duck_grip(&host).center();
    view.event(&UiEvent::PointerMove { pos: at, mods: MODS }, &host);
    view.event(&UiEvent::Wheel { pos: at, lines: 2.0, mods: MODS }, &host);
    assert!(plain(&host, DUCK_THRESH.id) > -18.0);
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

#[test]
fn a_double_click_on_the_plot_makes_a_bell_through_the_pointer_and_another_removes_it() {
    let (mut view, host) = setup();
    // Between the low and mid grips, under the curve: a shorter decay at 600 Hz.
    let x = (view.node(&host, Node::Low).x + view.node(&host, Node::Mid).x) / 2.0;
    let at = Point::new(x, view.decay_y(0.8));
    view.event(&UiEvent::PointerMove { pos: at, mods: MODS }, &host);
    view.event(&UiEvent::DoubleClick { pos: at, mods: MODS }, &host);
    draw(&mut view, &host);
    assert_eq!(plain(&host, "d1_on"), 1.0);
    assert!(plain(&host, "d1_mult") < 0.6, "{}", plain(&host, "d1_mult"));
    let p = view.node(&host, Node::Band(0));
    assert!((p.x - at.x).abs() < 2.0 && (p.y - at.y).abs() < 3.0, "{p:?} against {at:?}");
    // Dragged up: the decay there rises past the base curve.
    let to = Point::new(p.x + 40.0, view.decay_y(4.0));
    drag(&mut view, &host, p, to);
    assert!(plain(&host, "d1_mult") > 1.5, "{}", plain(&host, "d1_mult"));
    // The wheel narrows it.
    let p = view.node(&host, Node::Band(0));
    let q = plain(&host, "d1_q");
    view.event(&UiEvent::PointerMove { pos: p, mods: MODS }, &host);
    view.event(&UiEvent::Wheel { pos: p, lines: 3.0, mods: MODS }, &host);
    assert!(plain(&host, "d1_q") > q);
    // A double-click on it removes it.
    view.event(&UiEvent::DoubleClick { pos: p, mods: MODS }, &host);
    assert_eq!(plain(&host, "d1_on"), 0.0);
}

#[test]
fn four_bells_at_most() {
    let (mut view, host) = setup();
    for k in 0..5 {
        let at = Point::new(120.0 + 90.0 * f64::from(k), view.decay_y(1.0));
        view.event(&UiEvent::DoubleClick { pos: at, mods: MODS }, &host);
        draw(&mut view, &host);
    }
    for b in 1..=4 {
        assert_eq!(plain(&host, &format!("d{b}_on")), 1.0, "band {b}");
    }
}
