//! The Amp editor driven through events, against a stand-in host: the cards' switches and
//! buttons, the tone grips (bands up and down, cuts sideways), the strip's switches, the TONE3000
//! panel opening and closing.

use std::sync::Arc;

use mite_amp::editor::{memory_host, AmpView, CardButton, Grip, Toggle};
use mite_amp::params::{AMP, BASS, CAB, LOW_CUT, NORMALIZE, PEDAL_BASS, PEDAL_GAIN, SIZE};
use mite_amp::rig::{Slot, SlotFile, Status};
use mite_amp::AmpUi;
use mite_ui::kurbo::{Point, Rect};
use mite_ui::{DrawCx, Key, MemoryParamHost, Modifiers, MouseButton, ParamHost, Theme, ThemeKind, UiEvent, View};

const MODS: Modifiers = Modifiers { shift: false, alt: false, ctrl: false, meta: false };

fn plain(host: &MemoryParamHost, id: &str) -> f32 {
    host.plain(id, host.normalized(id))
}

fn draw(view: &mut AmpView, host: &MemoryParamHost) {
    let theme = Theme::load(ThemeKind::Dark, mite_amp::editor::ACCENT);
    let mut text = mite_ui::Typesetter::new();
    view.draw(&mut DrawCx { scene: &mut mite_ui::vello::Scene::new(), theme: &theme, text: &mut text, host, size: view.logical_size(), fx: &mut Default::default() });
}

fn setup() -> (AmpView, MemoryParamHost, Arc<AmpUi>) {
    let ui = Arc::new(AmpUi::new());
    let host = memory_host();
    let dir = std::env::temp_dir().join(format!("mitefx-amp-editor-{}", std::process::id()));
    let mut view = AmpView::new(ui.clone()).with_preset_dir(dir);
    view.tick(&host);
    draw(&mut view, &host);
    (view, host, ui)
}

fn click(view: &mut AmpView, host: &MemoryParamHost, at: Point) {
    view.event(&UiEvent::PointerMove { pos: at, mods: MODS }, host);
    view.event(&UiEvent::PointerDown { pos: at, button: MouseButton::Left, mods: MODS }, host);
    view.event(&UiEvent::PointerUp { pos: at, button: MouseButton::Left, mods: MODS }, host);
}

fn drag(view: &mut AmpView, host: &MemoryParamHost, from: Point, to: Point) {
    view.event(&UiEvent::PointerMove { pos: from, mods: MODS }, host);
    view.event(&UiEvent::PointerDown { pos: from, button: MouseButton::Left, mods: MODS }, host);
    for k in 1..=8 {
        view.event(&UiEvent::PointerMove { pos: from.lerp(to, f64::from(k) / 8.0), mods: MODS }, host);
    }
    view.event(&UiEvent::PointerUp { pos: to, button: MouseButton::Left, mods: MODS }, host);
}

fn centre(r: Rect) -> Point {
    r.center()
}

#[test]
fn the_cards_switch_their_slots_and_empty_them() {
    let (mut view, host, ui) = setup();
    let at = centre(view.button(Slot::Amp, CardButton::Power));
    click(&mut view, &host, at);
    assert_eq!(plain(&host, AMP.id), 0.0);
    let at = centre(view.button(Slot::Cab, CardButton::Power));
    click(&mut view, &host, at);
    assert_eq!(plain(&host, CAB.id), 0.0);
    ui.rig.set_file(Slot::Amp, SlotFile { path: Some("/nowhere/plexi.nam".into()), source: None });
    assert_eq!(ui.rig.status(Slot::Amp), Status::Loading);
    let at = centre(view.button(Slot::Amp, CardButton::Clear));
    click(&mut view, &host, at);
    assert!(ui.rig.file(Slot::Amp).path.is_none());
    view.tick(&host);
    draw(&mut view, &host);
}

#[test]
fn the_tone_grips_move_their_parameters_and_undo() {
    let (mut view, host, _) = setup();
    let g = view.grip_at(&host, Grip::Bass);
    drag(&mut view, &host, g, g - (0.0, 40.0));
    let bass = plain(&host, BASS.id);
    assert!(bass > 4.0 && bass < 12.0, "{bass}");
    view.event(&UiEvent::KeyDown { key: Key::Character('z'), mods: Modifiers { ctrl: true, ..MODS } }, &host);
    assert_eq!(plain(&host, BASS.id), 0.0);
    // The low cut drags sideways, from off at 20 Hz to about 100 Hz.
    let c = view.grip_at(&host, Grip::LowCut);
    let x100 = mite_amp::editor::freq_x(100.0);
    drag(&mut view, &host, c, Point::new(c.x + (x100 - mite_amp::editor::freq_x(20.0)), c.y));
    let lc = plain(&host, LOW_CUT.id);
    assert!((lc / 100.0 - 1.0).abs() < 0.05, "{lc}");
    // Double-click: off again.
    let c = view.grip_at(&host, Grip::LowCut);
    view.event(&UiEvent::DoubleClick { pos: c, mods: MODS }, &host);
    assert_eq!(plain(&host, LOW_CUT.id), 20.0);
}

#[test]
fn the_strip_switches_flip() {
    let (mut view, host, _) = setup();
    let at = centre(view.toggle(Toggle::Normalize));
    click(&mut view, &host, at);
    assert_eq!(plain(&host, NORMALIZE.id), 0.0);
    let at = centre(view.toggle(Toggle::Size));
    click(&mut view, &host, at);
    assert_eq!(plain(&host, SIZE.id), 1.0);
}

#[test]
fn tone3000_opens_over_the_display_and_closes() {
    let (mut view, host, _) = setup();
    let at = centre(view.button(Slot::Cab, CardButton::Tone3000));
    click(&mut view, &host, at);
    assert_eq!(view.browser_open(), Some(Slot::Cab));
    draw(&mut view, &host);
    // While it is open the cards underneath do not answer.
    let at = centre(view.button(Slot::Amp, CardButton::Power));
    click(&mut view, &host, at);
    assert_eq!(plain(&host, AMP.id), 1.0);
    view.event(&UiEvent::KeyDown { key: Key::Escape, mods: MODS }, &host);
    assert_eq!(view.browser_open(), None);
}

#[test]
fn selecting_a_card_gives_the_strip_and_the_grips_to_that_slot() {
    let (mut view, host, _) = setup();
    // The Amp is selected first: its bass grip moves the amp's bass.
    assert!(view.dial(PEDAL_GAIN.id).is_none());
    let card = view.button(Slot::Pedal, CardButton::Open);
    let body = Point::new(card.x0 + 200.0, card.y0 - 40.0);
    click(&mut view, &host, body);
    assert!(view.dial(PEDAL_GAIN.id).is_some(), "the pedal's dials are shown");
    let g = view.grip_at(&host, Grip::Bass);
    drag(&mut view, &host, g, g - (0.0, 40.0));
    assert!(plain(&host, PEDAL_BASS.id) > 4.0, "{}", plain(&host, PEDAL_BASS.id));
    assert_eq!(plain(&host, BASS.id), 0.0);
    draw(&mut view, &host);
}
