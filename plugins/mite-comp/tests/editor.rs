//! The Comp editor driven through events, against a stand-in host: making stages by pulling the
//! line, editing them on the history and the curve, undo, presets and Character audition.

use std::sync::Arc;

use mite_comp::editor::{memory_host, CompView};
use mite_comp::params::{stage_id, CHARACTER, S_AUTO_REL, S_ON, S_QUAD, S_RATIO, S_THRESH};
use mite_comp::CompShared;
use mite_dsp::dynamics::{curve_db, Quadrant, Stage, STAGES};
use mite_ui::kurbo::Point;
use mite_ui::render::render_view;
use mite_ui::{DrawCx, Key, MemoryParamHost, Modifiers, MouseButton, ParamHost, Theme, ThemeKind, UiEvent, View};

const MODS: Modifiers = Modifiers { shift: false, alt: false, ctrl: false, meta: false };
/// Transfer square: x 796, y 72, 384 px for −60…0 dB.
fn sq(input_db: f32, output_db: f32) -> Point {
    Point::new(796.0 + f64::from(input_db + 60.0) / 60.0 * 384.0, 72.0 + f64::from(-output_db) / 60.0 * 384.0)
}

fn plain(host: &MemoryParamHost, id: &str) -> f32 {
    host.plain(id, host.normalized(id))
}

fn setup() -> (CompView, MemoryParamHost, Arc<CompShared>) {
    let shared = Arc::new(CompShared::new());
    // The full 60 dB level range (Auto would fit the stages), so positions below are fixed.
    shared.display_range.store(60.0);
    let dir = std::env::temp_dir().join(format!("mitefx-comp-editor-{}", std::process::id()));
    let mut view = CompView::new(shared.clone()).with_preset_dir(dir);
    let host = memory_host();
    draw(&mut view, &host);
    (view, host, shared)
}

/// With the multiple stages option on and no stage yet (the stage gestures' tests).
fn setup_multi() -> (CompView, MemoryParamHost, Arc<CompShared>) {
    let (mut view, host, shared) = setup();
    shared.multi_stage.store(true, std::sync::atomic::Ordering::Relaxed);
    host.set_plain(&stage_id(0, S_ON.id), 0.0);
    draw(&mut view, &host);
    (view, host, shared)
}

/// Lays the view out (hit areas are set while drawing).
fn draw(view: &mut CompView, host: &MemoryParamHost) {
    let theme = Theme::load(ThemeKind::Dark, mite_comp::editor::ACCENT);
    let mut text = mite_ui::Typesetter::new();
    view.draw(&mut DrawCx { scene: &mut mite_ui::vello::Scene::new(), theme: &theme, text: &mut text, host, size: view.logical_size(), fx: &mut Default::default() });
}

fn drag(view: &mut CompView, host: &MemoryParamHost, from: Point, to: Point) {
    view.event(&UiEvent::PointerMove { pos: from, mods: MODS }, host);
    view.event(&UiEvent::PointerDown { pos: from, button: MouseButton::Left, mods: MODS }, host);
    for k in 1..=8 {
        let t = f64::from(k) / 8.0;
        view.event(&UiEvent::PointerMove { pos: from.lerp(to, t), mods: MODS }, host);
    }
    view.event(&UiEvent::PointerUp { pos: to, button: MouseButton::Left, mods: MODS }, host);
    draw(view, host);
}

fn stages(host: &MemoryParamHost) -> [Stage; STAGES] {
    std::array::from_fn(|i| Stage {
        on: plain(host, &stage_id(i, S_ON.id)) >= 0.5,
        quadrant: Quadrant::from_index(plain(host, &stage_id(i, S_QUAD.id)).round() as usize),
        threshold_db: plain(host, &stage_id(i, S_THRESH.id)),
        ratio: plain(host, &stage_id(i, S_RATIO.id)),
        ..Stage::default()
    })
}

#[test]
fn pulling_the_line_down_makes_a_compressor_and_up_lifts() {
    let (mut view, host, _) = setup_multi();
    // Down from −20 dB on the diagonal: a compressor at −20.
    drag(&mut view, &host, sq(-20.0, -20.0), sq(-20.0, -20.0) + (0.0, 60.0));
    let s = stages(&host);
    assert!(s[0].on);
    assert_eq!(s[0].quadrant, Quadrant::Compress);
    assert!((s[0].threshold_db + 20.0).abs() < 0.5, "{}", s[0].threshold_db);
    assert!(s[0].ratio > 3.5, "a 60 px pull is past 4:1: {}", s[0].ratio);
    // Up from −50 dB, below that threshold: upward compression (quiet parts come up).
    drag(&mut view, &host, sq(-50.0, -50.0), sq(-50.0, -50.0) - (0.0, 50.0));
    let s = stages(&host);
    assert!(s[1].on);
    assert_eq!(s[1].quadrant, Quadrant::UpCompress);
}

#[test]
fn editing_a_higher_stage_never_moves_a_lower_one() {
    let (mut view, host, _) = setup_multi();
    for db in [-44.0, -20.0] {
        let at = sq(db, curve_db(&stages(&host), db, 0.0));
        drag(&mut view, &host, at, at + (0.0, 50.0));
    }
    let s = stages(&host);
    assert!(s[0].on && s[1].on);
    let before = partial(&s, 0);
    // Stage 2: its ratio up (its node pulled down), then its threshold 8 dB down on the history
    // grip, still above stage 1's.
    let x = s[1].threshold_db + 12.0;
    let node = sq(x, curve_db(&s, x, 0.0));
    drag(&mut view, &host, node, node + (0.0, 30.0));
    let t = stages(&host)[1].threshold_db;
    let y = 80.0 + f64::from(-t) / 60.0 * 572.0;
    drag(&mut view, &host, Point::new(755.0, y), Point::new(755.0, y + 572.0 * 8.0 / 60.0));
    let after = stages(&host);
    assert!((after[1].threshold_db - (t - 8.0)).abs() < 0.5, "{t} → {}", after[1].threshold_db);
    assert!(after[1].ratio > s[1].ratio, "{} → {}", s[1].ratio, after[1].ratio);
    // Stage 1's node, and the curve below stage 2's knee, are where they were.
    assert_eq!(after[0], s[0]);
    let only_low = { let mut l = after; l[1].on = false; l };
    assert!((partial(&after, 0) - before).abs() < 1e-4);
    for i in 0..=20 {
        let x = -60.0 + i as f32;
        assert!((curve_db(&after, x, 0.0) - curve_db(&only_low, x, 0.0)).abs() < 1e-4, "at {x}");
    }
}

/// Stage `slot`'s node height, on the curve it and the stages below it make.
fn partial(s: &[Stage; STAGES], slot: usize) -> f32 {
    mite_dsp::dynamics::partial_curve_db(s, slot, s[slot].threshold_db + 12.0, 0.0)
}

#[test]
fn a_short_press_makes_nothing_and_a_fifth_stage_is_refused() {
    let (mut view, host, _) = setup_multi();
    drag(&mut view, &host, sq(-30.0, -30.0), sq(-30.0, -30.0) + (0.0, 3.0));
    assert!(!stages(&host).iter().any(|s| s.on));
    // Each pull starts away from the nodes already made (they sit 12 dB above their thresholds).
    for (k, db) in [-55.0, -38.0, -21.0, -4.0, -30.0].into_iter().enumerate() {
        let at = sq(db, curve_db(&stages(&host), db, 0.0));
        drag(&mut view, &host, at, at + (0.0, 70.0));
        assert_eq!(stages(&host).iter().filter(|s| s.on).count(), (k + 1).min(4));
    }
}

#[test]
fn one_undo_takes_a_pulled_stage_away() {
    let (mut view, host, _) = setup_multi();
    drag(&mut view, &host, sq(-24.0, -24.0), sq(-24.0, -24.0) + (0.0, 60.0));
    assert!(stages(&host)[0].on);
    view.event(&UiEvent::KeyDown { key: Key::Character('z'), mods: Modifiers { ctrl: true, ..MODS } }, &host);
    assert!(!stages(&host)[0].on, "undone");
}

#[test]
fn the_threshold_grip_on_the_history_moves_the_threshold() {
    let (mut view, host, _) = setup_multi();
    drag(&mut view, &host, sq(-24.0, -24.0), sq(-24.0, -24.0) + (0.0, 60.0));
    let before = stages(&host)[0].threshold_db;
    // The grip sits at the history's right edge, at the threshold's level (−60…0 dB over 572 px
    // from y 80).
    let y = 80.0 + f64::from(-before) / 60.0 * 572.0;
    drag(&mut view, &host, Point::new(755.0, y), Point::new(755.0, y - 57.2));
    let after = stages(&host)[0].threshold_db;
    assert!((after - before - 6.0).abs() < 0.3, "{before} → {after}");
}

#[test]
fn double_clicking_a_node_makes_it_inert_then_removes_it() {
    let (mut view, host, _) = setup_multi();
    drag(&mut view, &host, sq(-30.0, -30.0), sq(-30.0, -30.0) + (0.0, 60.0));
    let s = stages(&host);
    let node_x = s[0].threshold_db + 12.0;
    let node = sq(node_x, curve_db(&s, node_x, 0.0));
    view.event(&UiEvent::DoubleClick { pos: node, mods: MODS }, &host);
    assert!(stages(&host)[0].ratio <= 1.001);
    draw(&mut view, &host);
    let s = stages(&host);
    let node = sq(node_x, curve_db(&s, node_x, 0.0));
    view.event(&UiEvent::DoubleClick { pos: node, mods: MODS }, &host);
    assert!(!stages(&host)[0].on);
}

#[test]
fn clicking_a_stage_card_selects_that_stage() {
    let (mut view, host, _) = setup_multi();
    drag(&mut view, &host, sq(-40.0, -40.0), sq(-40.0, -40.0) + (0.0, 60.0));
    drag(&mut view, &host, sq(-15.0, -15.0) + (0.0, 8.0), sq(-15.0, -15.0) + (0.0, 70.0));
    let s = stages(&host);
    assert!(s[0].on && s[1].on);
    // Card 2 (x 796 + 98…) is the stage with the higher threshold, slot 1.
    let card2 = Point::new(796.0 + 98.0 + 45.0, 56.0 + 420.0 + 88.0);
    view.event(&UiEvent::PointerDown { pos: card2, button: MouseButton::Left, mods: MODS }, &host);
    view.event(&UiEvent::PointerUp { pos: card2, button: MouseButton::Left, mods: MODS }, &host);
    // The stage panel's Delete removes the selected stage: slot 1 goes.
    view.event(&UiEvent::KeyDown { key: Key::Delete, mods: MODS }, &host);
    let s = stages(&host);
    assert!(s[0].on && !s[1].on, "card 2 selected stage 2");
}

#[test]
fn a_factory_preset_loads_its_stages() {
    let (mut view, host, _) = setup();
    // ‹ › steps: next from Init is Vocal Leveller (a leveller and a compressor).
    let next = Point::new(456.0, 28.0);
    view.event(&UiEvent::PointerDown { pos: next, button: MouseButton::Left, mods: MODS }, &host);
    view.event(&UiEvent::PointerUp { pos: next, button: MouseButton::Left, mods: MODS }, &host);
    let s = stages(&host);
    assert!(s[0].on && s[1].on);
    assert_eq!(s[0].quadrant, Quadrant::UpCompress);
}

#[test]
fn hovering_a_character_auditions_it_without_touching_the_parameter() {
    let (mut view, host, shared) = setup();
    view.open_menu("character", &host);
    draw(&mut view, &host);
    // Rows run up from the footer button; hover each until one auditions.
    let mut heard = None;
    for y in (400..700).step_by(10) {
        view.event(&UiEvent::PointerMove { pos: Point::new(60.0, f64::from(y)), mods: MODS }, &host);
        if let Some(a) = shared.audition() {
            heard = Some(a);
            break;
        }
    }
    assert!(heard.is_some(), "a hovered Character auditions");
    assert_eq!(plain(&host, CHARACTER.id), 0.0, "the parameter is untouched");
    view.event(&UiEvent::KeyDown { key: Key::Escape, mods: MODS }, &host);
    assert_eq!(shared.audition(), None, "leaving the menu ends the audition");
}

#[test]
fn every_state_renders() {
    let (mut view, host, _) = setup();
    drag(&mut view, &host, sq(-24.0, -24.0), sq(-24.0, -24.0) + (0.0, 60.0));
    let theme = Theme::load(ThemeKind::Dark, mite_comp::editor::ACCENT);
    for panel in ["custom", "sidechain", "detector", "stereo"] {
        view.open_panel(panel);
        if let Err(e) = render_view(&mut view, &host, &theme, 1.0) {
            assert!(e.contains("GPU"), "{e}");
            return;
        }
    }
}

#[test]
fn the_stage_panel_switches_auto_release() {
    let (mut view, host, _) = setup_multi();
    drag(&mut view, &host, sq(-20.0, -20.0), sq(-20.0, -20.0) + (0.0, 60.0));
    assert!(stages(&host)[0].on);
    view.select(Some(0));
    draw(&mut view, &host);
    // Under the detector dropdown: x 288…343, y 616…650.
    let at = Point::new(315.0, 633.0);
    for want in [1.0, 0.0] {
        view.event(&UiEvent::PointerMove { pos: at, mods: MODS }, &host);
        view.event(&UiEvent::PointerDown { pos: at, button: MouseButton::Left, mods: MODS }, &host);
        view.event(&UiEvent::PointerUp { pos: at, button: MouseButton::Left, mods: MODS }, &host);
        draw(&mut view, &host);
        assert_eq!(plain(&host, &stage_id(0, S_AUTO_REL.id)), want);
    }
}

fn click(view: &mut CompView, host: &MemoryParamHost, at: Point) {
    view.event(&UiEvent::PointerMove { pos: at, mods: MODS }, host);
    view.event(&UiEvent::PointerDown { pos: at, button: MouseButton::Left, mods: MODS }, host);
    view.event(&UiEvent::PointerUp { pos: at, button: MouseButton::Left, mods: MODS }, host);
    draw(view, host);
}

/// The header's multiple stages switch (x 760…800).
const STAGES_SWITCH: Point = Point::new(780.0, 28.0);

#[test]
fn a_fresh_comp_is_one_compressor_that_compresses() {
    let (mut view, host, shared) = setup();
    let s = stages(&host);
    assert!(s[0].on && s[1..].iter().all(|st| !st.on), "stage 1 on by default");
    assert!(s[0].quadrant == Quadrant::Compress && plain(&host, &stage_id(0, S_RATIO.id)) > 1.0);
    // Pulling the line edits nothing new; Delete keeps the stage.
    drag(&mut view, &host, sq(-40.0, -40.0), sq(-40.0, -40.0) + (0.0, 60.0));
    assert_eq!(stages(&host).iter().filter(|st| st.on).count(), 1);
    view.event(&UiEvent::KeyDown { key: Key::Delete, mods: MODS }, &host);
    assert!(stages(&host)[0].on);
    // The compact controls under the square: Expand (the second quadrant icon) and auto release.
    click(&mut view, &host, Point::new(796.0 + 46.0 + 20.0, 56.0 + 420.0 + 16.0));
    assert_eq!(plain(&host, &stage_id(0, S_QUAD.id)), 1.0);
    click(&mut view, &host, Point::new(796.0 + 384.0 - 42.0, 56.0 + 420.0 + 16.0));
    assert_eq!(plain(&host, &stage_id(0, S_AUTO_REL.id)), 1.0);
    assert!(!shared.multi_stage.load(std::sync::atomic::Ordering::Relaxed));
}

#[test]
fn the_multiple_stages_option_shows_the_strip_and_deleting_down_to_one_leaves_it() {
    let (mut view, host, shared) = setup();
    let multi = || shared.multi_stage.load(std::sync::atomic::Ordering::Relaxed);
    click(&mut view, &host, STAGES_SWITCH);
    assert!(multi());
    click(&mut view, &host, STAGES_SWITCH);
    assert!(!multi(), "with one stage it switches back off");
    click(&mut view, &host, STAGES_SWITCH);
    // Now pulling the line makes a second stage, and the switch holds while there are two.
    drag(&mut view, &host, sq(-40.0, -40.0), sq(-40.0, -40.0) + (0.0, 60.0));
    assert_eq!(stages(&host).iter().filter(|st| st.on).count(), 2);
    click(&mut view, &host, STAGES_SWITCH);
    assert!(multi(), "held on while two stages exist");
    // Deleting one (selected: the one just made) goes back to a single compressor.
    view.event(&UiEvent::KeyDown { key: Key::Delete, mods: MODS }, &host);
    draw(&mut view, &host);
    assert_eq!(stages(&host).iter().filter(|st| st.on).count(), 1);
    assert!(!multi());
}

#[test]
fn a_range_picked_by_hand_never_hides_a_deeper_threshold() {
    // The user's case: 24 dB picked by hand and the threshold pulled under it. Every threshold must
    // still be drawn where it is (it used to sit at the floor, so −30 and −50 looked alike).
    let (mut view, host, shared) = setup();
    shared.display_range.store(24.0);
    for (threshold, range) in [(-12.0, 24.0), (-24.0, 36.0), (-35.0, 48.0), (-50.0, 60.0)] {
        host.set_plain(&stage_id(0, S_THRESH.id), threshold);
        view.tick(&host);
        draw(&mut view, &host);
        assert_eq!(view.level_range(), range, "threshold {threshold}");
    }
    // And the history's grip keeps going: dragged down from −20 dB, the threshold follows past −24.
    host.set_plain(&stage_id(0, S_THRESH.id), -20.0);
    // The displays glide to a new range (120 ms): let them settle before aiming at the grip.
    for _ in 0..80 {
        std::thread::sleep(std::time::Duration::from_millis(15));
        view.tick(&host);
    }
    draw(&mut view, &host);
    let r = f64::from(view.level_range());
    let y = 80.0 + 20.0 / r * 572.0;
    drag(&mut view, &host, Point::new(755.0, y), Point::new(755.0, y + 15.0 / r * 572.0));
    let after = stages(&host)[0].threshold_db;
    assert!((after + 35.0).abs() < 0.5, "dragged to {after}");
}

#[test]
fn smart_vocal_sets_ratio_and_knee_so_their_dials_show_auto() {
    // Pro-C's Vocal: automatic knee and ratio, only the threshold matters. The RATIO dial under
    // the square (second of the top row) does nothing then; with Smart off it turns again.
    let (mut view, host, _) = setup();
    let ratio = stage_id(0, S_RATIO.id);
    let dial = Point::new(796.0 + 384.0 / 3.0 * 1.5, 56.0 + 420.0 + 70.0);
    let before = plain(&host, &ratio);
    host.set_plain(mite_comp::params::SMART.id, 2.0);
    draw(&mut view, &host);
    drag(&mut view, &host, dial, dial - (0.0, 80.0));
    assert_eq!(plain(&host, &ratio), before, "Vocal: the ratio is automatic");
    view.event(&UiEvent::KeyDown { key: Key::PageUp, mods: MODS }, &host);
    assert_eq!(plain(&host, &ratio), before);
    host.set_plain(mite_comp::params::SMART.id, 0.0);
    draw(&mut view, &host);
    drag(&mut view, &host, dial, dial - (0.0, 80.0));
    assert!(plain(&host, &ratio) > before, "Off: the dial turns the ratio");
}
