//! The Mod editor driven through events, against a stand-in host: the mode tiles, each mode's
//! own controls, the puck on the response view, dragging the time view, the shapes, sync and the
//! division menu, the dial groups, the rotary's speed (pills and a click on the cabinet).

use std::sync::Arc;

use mite_dsp::modulation::{Mode, Rotor, Shape, DIVISIONS};
use mite_mod::editor::{memory_host, ModView, Pill};
use mite_mod::params::*;
use mite_mod::ModShared;
use mite_ui::kurbo::Point;
use mite_ui::{DrawCx, Key, MemoryParamHost, Modifiers, MouseButton, ParamHost, Theme, ThemeKind, UiEvent, View};

const MODS: Modifiers = Modifiers { shift: false, alt: false, ctrl: false, meta: false };

fn plain(host: &MemoryParamHost, id: &str) -> f32 {
    host.plain(id, host.normalized(id))
}

fn setup() -> (ModView, MemoryParamHost) {
    let dir = std::env::temp_dir().join(format!("mitefx-mod-editor-{}", std::process::id()));
    let mut view = ModView::new(Arc::new(ModShared::new())).with_preset_dir(dir);
    let host = memory_host();
    view.tick(&host);
    draw(&mut view, &host);
    (view, host)
}

fn draw(view: &mut ModView, host: &MemoryParamHost) {
    let theme = Theme::load(ThemeKind::Dark, mite_mod::editor::ACCENT);
    let mut text = mite_ui::Typesetter::new();
    view.draw(&mut DrawCx { scene: &mut mite_ui::vello::Scene::new(), theme: &theme, text: &mut text, host, size: view.logical_size(), fx: &mut Default::default() });
}

fn drag(view: &mut ModView, host: &MemoryParamHost, from: Point, to: Point) {
    view.event(&UiEvent::PointerMove { pos: from, mods: MODS }, host);
    view.event(&UiEvent::PointerDown { pos: from, button: MouseButton::Left, mods: MODS }, host);
    for k in 1..=8 {
        view.event(&UiEvent::PointerMove { pos: from.lerp(to, f64::from(k) / 8.0), mods: MODS }, host);
    }
    view.event(&UiEvent::PointerUp { pos: to, button: MouseButton::Left, mods: MODS }, host);
    draw(view, host);
}

fn click(view: &mut ModView, host: &MemoryParamHost, at: Point) {
    view.event(&UiEvent::PointerMove { pos: at, mods: MODS }, host);
    view.event(&UiEvent::PointerDown { pos: at, button: MouseButton::Left, mods: MODS }, host);
    view.event(&UiEvent::PointerUp { pos: at, button: MouseButton::Left, mods: MODS }, host);
    draw(view, host);
}

#[test]
fn the_tiles_pick_the_mode_and_its_controls_follow() {
    let (mut view, host) = setup();
    assert!(view.dial(&host, DELAY.id).is_some());
    assert!(view.dial(&host, FEEDBACK.id).is_none());
    let at = view.pill(Pill::Mode(Mode::Flanger)).center();
    click(&mut view, &host, at);
    assert_eq!(ModView::mode(&host), Mode::Flanger);
    assert!(view.dial(&host, MANUAL.id).is_some() && view.dial(&host, FEEDBACK.id).is_some());
    assert!(view.dial(&host, DELAY.id).is_none());
    let at = view.pill(Pill::ThruZero).center();
    click(&mut view, &host, at);
    assert_eq!(plain(&host, THRU_ZERO.id), 1.0);
    let at = view.pill(Pill::Mode(Mode::Phaser)).center();
    click(&mut view, &host, at);
    let at = view.pill(Pill::Stages(6)).center();
    click(&mut view, &host, at);
    assert_eq!(plain(&host, STAGES.id), 6.0);
    // Vibrato: no MIX dial (all wet).
    let at = view.pill(Pill::Mode(Mode::Vibrato)).center();
    click(&mut view, &host, at);
    assert!(view.dial(&host, MIX.id).is_none());
}

#[test]
fn the_puck_sets_where_it_sweeps_and_how_hard_it_rings_in_one_undo_step() {
    let (mut view, host) = setup();
    host.set_plain(MODE.id, 3.0);
    draw(&mut view, &host);
    let p = view.puck(&host);
    drag(&mut view, &host, p, p + (120.0, -60.0));
    assert!(plain(&host, CENTRE.id) > 1200.0, "{}", plain(&host, CENTRE.id));
    assert!(plain(&host, FEEDBACK.id) > 20.0, "{}", plain(&host, FEEDBACK.id));
    view.event(&UiEvent::KeyDown { key: Key::Character('z'), mods: Modifiers { ctrl: true, ..MODS } }, &host);
    assert!((plain(&host, CENTRE.id) - CENTRE.default).abs() < 0.01);
    assert!((plain(&host, FEEDBACK.id) - FEEDBACK.default).abs() < 0.01);
    // The flanger's puck sets MANUAL (right: shorter, the first notch higher).
    host.set_plain(MODE.id, 2.0);
    draw(&mut view, &host);
    let p = view.puck(&host);
    drag(&mut view, &host, p, p + (100.0, 0.0));
    assert!(plain(&host, MANUAL.id) < MANUAL.default);
}

#[test]
fn dragging_the_time_view_sets_the_delay_and_double_click_puts_it_back() {
    let (mut view, host) = setup();
    let c = view.plot().center();
    drag(&mut view, &host, c, c - (0.0, 60.0));
    assert!(plain(&host, DELAY.id) > DELAY.default);
    view.event(&UiEvent::DoubleClick { pos: c, mods: MODS }, &host);
    assert_eq!(plain(&host, DELAY.id), DELAY.default);
}

#[test]
fn shapes_sync_and_the_division_menu() {
    let (mut view, host) = setup();
    let at = view.pill(Pill::Shape(Shape::Random)).center();
    click(&mut view, &host, at);
    assert_eq!(plain(&host, SHAPE.id), 5.0);
    let at = view.pill(Pill::Division).center();
    click(&mut view, &host, at);
    let rows = view.menu_rows();
    assert_eq!(rows.len(), DIVISIONS.len());
    click(&mut view, &host, rows[9].center());
    assert_eq!(plain(&host, DIVISION.id), 9.0);
    assert_eq!(plain(&host, SYNC.id), 1.0, "picking a division syncs");
    let at = view.pill(Pill::Sync).center();
    click(&mut view, &host, at);
    assert_eq!(plain(&host, SYNC.id), 0.0);
}

#[test]
fn the_dials_sit_in_their_groups() {
    let (view, host) = setup();
    let groups = view.dial_groups();
    let inside = |id: &str, group: &str| {
        let p = view.dial(&host, id).unwrap_or_else(|| panic!("no dial {id}"));
        let r = groups.iter().find(|(n, _)| *n == group).expect("group").1;
        assert!(r.contains(p), "{id} outside {group}");
    };
    for id in [RATE.id, DEPTH.id, SPREAD.id] {
        inside(id, "MOTION");
    }
    for id in [MIX.id, WIDTH.id, LOW_CUT.id, HIGH_CUT.id] {
        inside(id, "SOUND");
    }
    inside(DELAY.id, "MODE");
    let mode = groups.iter().find(|(n, _)| *n == "MODE").expect("mode").1;
    assert!(mode.contains(view.pill(Pill::Voices(4)).center()));
}

#[test]
fn dragging_the_puck_stays_well_under_a_frame() {
    // Each drag event changes the settings, so the sweep's span is worked out again on the next
    // draw: an event and a draw together must leave room in a 120 fps frame (release builds;
    // debug builds are slower, so this only reports there).
    let (mut view, host) = setup();
    for mode in [2.0, 3.0] {
        host.set_plain(MODE.id, mode);
        host.set_plain(STAGES.id, 6.0);
        draw(&mut view, &host);
        let p = view.puck(&host);
        view.event(&UiEvent::PointerMove { pos: p, mods: MODS }, &host);
        view.event(&UiEvent::PointerDown { pos: p, button: MouseButton::Left, mods: MODS }, &host);
        let t0 = std::time::Instant::now();
        let n = 10;
        for k in 1..=n {
            view.event(&UiEvent::PointerMove { pos: p + (f64::from(k) * 7.0, -f64::from(k) * 3.0), mods: MODS }, &host);
            draw(&mut view, &host);
        }
        let per = t0.elapsed().as_secs_f64() * 1000.0 / f64::from(n);
        view.event(&UiEvent::PointerUp { pos: p, button: MouseButton::Left, mods: MODS }, &host);
        println!("mode {mode}: {per:.2} ms an event and draw");
        if !cfg!(debug_assertions) {
            assert!(per < 4.0, "mode {mode}: {per:.2} ms");
        }
    }
}

#[test]
fn the_rotary_has_its_speed_its_cabinet_and_no_lfo() {
    let (mut view, host) = setup();
    let at = view.pill(Pill::Mode(Mode::Rotary)).center();
    click(&mut view, &host, at);
    assert_eq!(ModView::mode(&host), Mode::Rotary);
    // Its own dials in MODE; no RATE (the rotors set their speed), the LFO's pills gone.
    let groups = view.dial_groups();
    let mode = groups.iter().find(|(n, _)| *n == "MODE").expect("mode").1;
    for id in [BALANCE.id, INERTIA.id] {
        assert!(mode.contains(view.dial(&host, id).unwrap_or_else(|| panic!("no dial {id}"))));
    }
    assert!(view.dial(&host, RATE.id).is_none());
    assert!(view.dial(&host, DEPTH.id).is_some() && view.dial(&host, MIX.id).is_some());
    let shape = view.pill(Pill::Shape(Shape::Square)).center();
    click(&mut view, &host, shape);
    assert_eq!(plain(&host, SHAPE.id), 0.0, "the shapes are hidden in rotary");
    // The speed: a pill, and a click on the cabinet toggles slow and fast.
    let at = view.pill(Pill::Rotor(Rotor::Brake)).center();
    click(&mut view, &host, at);
    assert_eq!(plain(&host, ROTOR.id), 2.0);
    let cabinet = view.plot().center();
    click(&mut view, &host, cabinet);
    assert_eq!(plain(&host, ROTOR.id), 1.0);
    click(&mut view, &host, cabinet);
    assert_eq!(plain(&host, ROTOR.id), 0.0);
    // Drawing it leaves room in a 120 fps frame (release builds).
    let t0 = std::time::Instant::now();
    for _ in 0..10 {
        draw(&mut view, &host);
    }
    let per = t0.elapsed().as_secs_f64() * 100.0;
    println!("rotary: {per:.2} ms a draw");
    if !cfg!(debug_assertions) {
        assert!(per < 4.0, "{per:.2} ms");
    }
}

#[test]
fn micro_has_detune_and_delay_and_a_drag_sets_the_detune() {
    let (mut view, host) = setup();
    let at = view.pill(Pill::Mode(Mode::Micro)).center();
    click(&mut view, &host, at);
    assert_eq!(ModView::mode(&host), Mode::Micro);
    let groups = view.dial_groups();
    let mode = groups.iter().find(|(n, _)| *n == "MODE").expect("mode").1;
    for id in [DETUNE.id, DELAY.id] {
        assert!(mode.contains(view.dial(&host, id).unwrap_or_else(|| panic!("no dial {id}"))));
    }
    assert!(view.dial(&host, RATE.id).is_some() && view.dial(&host, MIX.id).is_some());
    let before = plain(&host, DETUNE.id);
    let c = view.plot().center();
    drag(&mut view, &host, c, c - (0.0, 60.0));
    assert!(plain(&host, DETUNE.id) > before + 1.0, "{} → {}", before, plain(&host, DETUNE.id));
    view.event(&UiEvent::DoubleClick { pos: c, mods: MODS }, &host);
    assert_eq!(plain(&host, DETUNE.id), DETUNE.default);
    // The seven tiles fit left of the meters.
    assert!(view.pill(Pill::Mode(Mode::Micro)).x1 <= 1172.0);
}
