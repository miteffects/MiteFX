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

#[test]
fn the_sidechain_eq_opens_and_its_nodes_are_the_bands() {
    use mite_gate::params::{key_id, K_FREQ, K_GAIN, K_ON};
    let (mut view, host) = setup();
    assert!(view.eq_area().is_none());
    let at = view.eq_button().center();
    click(&mut view, &host, at);
    let area = view.eq_area().expect("the panel opens");
    // Dragging band 2's node (a bell) turns it on and sets frequency and gain, as one step.
    let node = view.eq_node(&host, 1);
    assert!(area.contains(node));
    let to = Point::new(area.x0 + area.width() * (2000.0f64 / 20.0).ln() / 1000f64.ln(), area.center().y - 6.0 * (area.height() / 2.0 - 12.0) / 18.0);
    drag(&mut view, &host, node, to, MODS);
    assert_eq!(plain(&host, &key_id(1, K_ON.id)), 1.0);
    assert!((plain(&host, &key_id(1, K_FREQ.id)) - 2000.0).abs() < 20.0, "{}", plain(&host, &key_id(1, K_FREQ.id)));
    assert!((plain(&host, &key_id(1, K_GAIN.id)) - 6.0).abs() < 0.1);
    undo(&mut view, &host);
    assert_eq!(plain(&host, &key_id(1, K_ON.id)), 0.0, "one undo takes it all back");
    // Double-click a node: its band on, again: off.
    let node = view.eq_node(&host, 0);
    view.event(&UiEvent::DoubleClick { pos: node, mods: MODS }, &host);
    assert_eq!(plain(&host, &key_id(0, K_ON.id)), 1.0);
    view.event(&UiEvent::DoubleClick { pos: node, mods: MODS }, &host);
    assert_eq!(plain(&host, &key_id(0, K_ON.id)), 0.0);
    view.event(&UiEvent::KeyDown { key: Key::Escape, mods: MODS }, &host);
    assert!(view.eq_area().is_none());
}

#[test]
fn a_click_makes_a_band_and_right_click_gives_every_shape() {
    use mite_gate::params::{key_id, K_FREQ, K_ON, K_Q, K_SLOPE, K_TYPE};
    let (mut view, host) = setup();
    view.open_eq();
    draw(&mut view, &host);
    let area = view.eq_area().expect("open");
    // An empty spot at 500 Hz, +6 dB: the first band that is off (band 1) becomes a bell there.
    let x = area.x0 + area.width() * (500.0f64 / 20.0).ln() / 1000f64.ln();
    let at = Point::new(x, area.center().y - 6.0 * (area.height() / 2.0 - 12.0) / 18.0);
    click(&mut view, &host, at);
    assert_eq!(plain(&host, &key_id(0, K_ON.id)), 1.0);
    assert_eq!(plain(&host, &key_id(0, K_TYPE.id)), 0.0, "a bell");
    assert!((plain(&host, &key_id(0, K_FREQ.id)) - 500.0).abs() < 5.0);
    // Wheel on it: Q.
    let node = view.eq_node(&host, 0);
    let q = plain(&host, &key_id(0, K_Q.id));
    view.event(&UiEvent::Wheel { pos: node, lines: 6.0, mods: MODS }, &host);
    assert!((plain(&host, &key_id(0, K_Q.id)) / q - 2.0).abs() < 0.02, "an octave of Q per 6 notches");
    // Right-click: every shape; pick Low Cut; then the wheel steps its slope.
    view.event(&UiEvent::PointerDown { pos: node, button: MouseButton::Right, mods: MODS }, &host);
    draw(&mut view, &host);
    let rows = view.eq_menu_rows();
    assert_eq!(rows.len(), 11, "ten shapes and Remove");
    click(&mut view, &host, rows[3].center());
    assert_eq!(plain(&host, &key_id(0, K_TYPE.id)), 3.0);
    let node = view.eq_node(&host, 0);
    view.event(&UiEvent::Wheel { pos: node, lines: 1.0, mods: MODS }, &host);
    assert_eq!(plain(&host, &key_id(0, K_SLOPE.id)), 4.0, "24 → 30 dB/oct");
    // Right-click: Remove.
    view.event(&UiEvent::PointerDown { pos: node, button: MouseButton::Right, mods: MODS }, &host);
    draw(&mut view, &host);
    let rows = view.eq_menu_rows();
    click(&mut view, &host, rows[10].center());
    assert_eq!(plain(&host, &key_id(0, K_ON.id)), 0.0);
}

/// The MIDI panel: the footer button opens it (closing the EQ), the power, a key and an octave
/// set the note, a cell the channel, the glyphs the velocity mode; clicks inside it reach nothing
/// under it; Esc closes it.
#[test]
fn the_midi_panel_sets_the_trigger() {
    use mite_gate::params::{MIDI_CHANNEL, MIDI_NOTE, MIDI_ON, MIDI_VELOCITY};
    let (mut view, host) = setup();
    view.open_eq();
    let b = view.midi_button().center();
    click(&mut view, &host, b);
    assert!(view.eq_area().is_none(), "the MIDI panel closes the EQ");
    let at = |view: &GateView, f: &dyn Fn(&mite_gate::editor::MidiPanel) -> mite_ui::kurbo::Rect| f(view.midi_panel().expect("open")).center();
    let c = at(&view, &|p| p.power_button());
    click(&mut view, &host, c);
    assert_eq!(plain(&host, MIDI_ON.id), 1.0);
    // D (white) in the octave shown (36 = C2): 38; then octave 3: 50; then D# (black): 51.
    let c = at(&view, &|p| p.key_rect(2));
    click(&mut view, &host, c);
    assert_eq!(plain(&host, MIDI_NOTE.id), 38.0);
    let c = at(&view, &|p| p.octave_rect(4));
    click(&mut view, &host, c);
    assert_eq!(plain(&host, MIDI_NOTE.id), 50.0);
    let c = at(&view, &|p| p.key_rect(3));
    click(&mut view, &host, c);
    assert_eq!(plain(&host, MIDI_NOTE.id), 51.0);
    let at_key = at(&view, &|p| p.key_rect(3));
    view.event(&UiEvent::Wheel { pos: at_key, lines: 1.0, mods: MODS }, &host);
    assert_eq!(plain(&host, MIDI_NOTE.id), 52.0);
    let c = at(&view, &|p| p.channel_rect(9));
    click(&mut view, &host, c);
    assert_eq!(plain(&host, MIDI_CHANNEL.id), 10.0);
    let c = at(&view, &|p| p.velocity_rect(1));
    click(&mut view, &host, c);
    assert_eq!(plain(&host, MIDI_VELOCITY.id), 1.0);
    // Under the panel lies the history; the threshold does not move on a click in the panel.
    let t = plain(&host, THRESHOLD.id);
    let panel = view.midi_panel().unwrap().rect();
    click(&mut view, &host, Point::new(panel.x0 + 300.0, panel.y1 - 6.0));
    assert_eq!(plain(&host, THRESHOLD.id), t);
    view.event(&UiEvent::KeyDown { key: Key::Escape, mods: MODS }, &host);
    assert!(view.midi_panel().is_none());
}
