//! The Rig editor driven through events: adding a block from a +, adding a split, moving a block
//! by dragging it into a lane, its light switching its bypass, its knobs moving its values.

use std::sync::Arc;

use mite_rig::blocks::Kind;
use mite_rig::board::Place;
use mite_rig::editor::{memory_host, RigView, Selection};
use mite_rig::RigUi;
use mite_ui::kurbo::Point;
use mite_ui::{DrawCx, MemoryParamHost, Modifiers, MouseButton, Theme, ThemeKind, UiEvent, View};

const MODS: Modifiers = Modifiers { shift: false, alt: false, ctrl: false, meta: false };

fn draw(view: &mut RigView, host: &MemoryParamHost) {
    let theme = Theme::load(ThemeKind::Dark, mite_rig::editor::ACCENT);
    let mut text = mite_ui::Typesetter::new();
    view.draw(&mut DrawCx { scene: &mut mite_ui::vello::Scene::new(), theme: &theme, text: &mut text, host, size: view.logical_size(), fx: &mut Default::default() });
}

fn setup() -> (RigView, MemoryParamHost, Arc<RigUi>) {
    let ui = Arc::new(RigUi::new());
    let host = memory_host();
    let dir = std::env::temp_dir().join(format!("mitefx-rig-editor-{}", std::process::id()));
    let mut view = RigView::new(ui.clone()).with_preset_dir(dir);
    view.tick(&host);
    draw(&mut view, &host);
    (view, host, ui)
}

fn click(view: &mut RigView, host: &MemoryParamHost, at: Point) {
    view.event(&UiEvent::PointerMove { pos: at, mods: MODS }, host);
    view.event(&UiEvent::PointerDown { pos: at, button: MouseButton::Left, mods: MODS }, host);
    view.event(&UiEvent::PointerUp { pos: at, button: MouseButton::Left, mods: MODS }, host);
    view.tick(host);
}

fn drag(view: &mut RigView, host: &MemoryParamHost, from: Point, to: Point) {
    view.event(&UiEvent::PointerMove { pos: from, mods: MODS }, host);
    view.event(&UiEvent::PointerDown { pos: from, button: MouseButton::Left, mods: MODS }, host);
    for k in 1..=10 {
        view.event(&UiEvent::PointerMove { pos: from.lerp(to, f64::from(k) / 10.0), mods: MODS }, host);
    }
    view.event(&UiEvent::PointerUp { pos: to, button: MouseButton::Left, mods: MODS }, host);
    view.tick(host);
}

#[test]
fn a_plus_adds_the_block_picked_from_its_menu() {
    let (mut view, host, ui) = setup();
    let gap = view.layout().gaps[0].at;
    click(&mut view, &host, gap);
    let row = view.menu_row(view.menu_index(Some(Kind::Tone), 0).expect("the menu is open")).unwrap();
    click(&mut view, &host, row.center());
    let blocks = ui.board.blocks();
    assert_eq!(blocks[0].kind(), Kind::Tone);
    assert_eq!(view.selection(), Some(Selection::Block(blocks[0].id())));
    draw(&mut view, &host);
}

#[test]
fn a_split_is_added_and_a_block_dragged_into_its_lane() {
    let (mut view, host, ui) = setup();
    // The + after the cab: "Split · 3 lanes".
    let last = view.layout().gaps.last().unwrap().at;
    click(&mut view, &host, last);
    let row = view.menu_row(view.menu_index(None, 3).unwrap()).unwrap();
    click(&mut view, &host, row.center());
    assert!(matches!(view.selection(), Some(Selection::Split(_))));
    let layout = view.layout();
    assert_eq!(layout.forks[0].lanes, 3);
    // Drag the gate (first tile) into the split's middle lane.
    let gate = layout.tiles[0];
    let lane_gap = layout.gaps.iter().find(|g| matches!(g.place, Place::Lane { lane: 1, index: 0, .. })).unwrap().at;
    drag(&mut view, &host, gate.rect.center(), lane_gap);
    assert!(matches!(ui.board.place_of(gate.id), Some(Place::Lane { lane: 1, index: 0, .. })), "{:?}", ui.board.place_of(gate.id));
    draw(&mut view, &host);
}

#[test]
fn a_tiles_light_bypasses_it_and_its_knobs_move_its_values() {
    let (mut view, host, ui) = setup();
    let layout = view.layout();
    let gate = layout.tiles[0];
    let led = view.led_of(&gate).unwrap();
    click(&mut view, &host, led);
    assert!(ui.board.block(gate.id).unwrap().live.bypassed());
    click(&mut view, &host, gate.rect.center());
    assert_eq!(view.selection(), Some(Selection::Block(gate.id)));
    let k = view.knob_at("threshold").expect("the gate's knob");
    view.event(&UiEvent::PointerMove { pos: k, mods: MODS }, &host);
    view.event(&UiEvent::PointerDown { pos: k, button: MouseButton::Left, mods: MODS }, &host);
    view.event(&UiEvent::PointerMove { pos: k - (0.0, 60.0), mods: MODS }, &host);
    view.event(&UiEvent::PointerUp { pos: k - (0.0, 60.0), button: MouseButton::Left, mods: MODS }, &host);
    let v = ui.board.block(gate.id).unwrap().live.value(0);
    assert!(v > -70.0, "{v}");
    draw(&mut view, &host);
}

#[test]
fn a_block_dragged_from_the_palette_lands_on_the_plus_it_is_dropped_on() {
    let (mut view, host, ui) = setup();
    let wah = RigView::palette_rect(Some(Kind::Wah)).unwrap().center();
    // The + between the gate and the amp.
    let gap = view.layout().gaps[1].at;
    drag(&mut view, &host, wah, gap);
    let kinds: Vec<Kind> = ui.board.blocks().iter().map(|b| b.kind()).collect();
    assert_eq!(kinds[..2], [Kind::Gate, Kind::Wah]);
    // A click adds after the selected block (the new wah).
    click(&mut view, &host, RigView::palette_rect(Some(Kind::Delay)).unwrap().center());
    let kinds: Vec<Kind> = ui.board.blocks().iter().map(|b| b.kind()).collect();
    assert_eq!(kinds[..3], [Kind::Gate, Kind::Wah, Kind::Delay]);
    draw(&mut view, &host);
}
