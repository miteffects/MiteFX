//! The Sat editor driven through events, against a stand-in host: shaping the curve by dragging
//! it (one undo step), picking a model, the band's bypass, the wheel, resetting, the
//! oversampling list and keys; then several bands: pulling a crossover in, merging, editing a
//! narrow card, solo, the band menu and removing.

use std::sync::Arc;

use mite_dsp::sat::Model;
use mite_sat::editor::{memory_host, SatView};
use mite_dsp::sat::Layout;
use mite_sat::params::{B_HARM, band_id, cross_id, B_BIAS, B_BYPASS, B_DRIVE, B_DYN, B_DYN_ATT, B_DYN_THR, B_MIX, B_MODEL, B_MUTE, B_SOLO, B_STEREO, B_TONE, OVERSAMPLING, X_FREQ, X_ON};
use mite_sat::SatShared;
use mite_ui::kurbo::Point;
use mite_ui::{DrawCx, Key, MemoryParamHost, Modifiers, MouseButton, ParamHost, Theme, ThemeKind, UiEvent, View};

const MODS: Modifiers = Modifiers { shift: false, alt: false, ctrl: false, meta: false };

fn plain(host: &MemoryParamHost, id: &str) -> f32 {
    host.plain(id, host.normalized(id))
}

fn setup() -> (SatView, MemoryParamHost) {
    let shared = Arc::new(SatShared::new());
    let dir = std::env::temp_dir().join(format!("mitefx-sat-editor-{}", std::process::id()));
    let mut view = SatView::new(shared).with_preset_dir(dir);
    let host = memory_host();
    draw(&mut view, &host);
    (view, host)
}

/// Lays the view out (hit areas are set while drawing), with the layout settled.
fn draw(view: &mut SatView, host: &MemoryParamHost) {
    view.settle(host);
    let theme = Theme::load(ThemeKind::Dark, mite_sat::editor::ACCENT);
    let mut text = mite_ui::Typesetter::new();
    view.draw(&mut DrawCx { scene: &mut mite_ui::vello::Scene::new(), theme: &theme, text: &mut text, host, size: view.logical_size(), fx: &mut Default::default() });
}

fn drag(view: &mut SatView, host: &MemoryParamHost, from: Point, to: Point, mods: Modifiers) {
    view.event(&UiEvent::PointerMove { pos: from, mods }, host);
    view.event(&UiEvent::PointerDown { pos: from, button: MouseButton::Left, mods }, host);
    for k in 1..=8 {
        let t = f64::from(k) / 8.0;
        view.event(&UiEvent::PointerMove { pos: from.lerp(to, t), mods }, host);
    }
    view.event(&UiEvent::PointerUp { pos: to, button: MouseButton::Left, mods }, host);
    draw(view, host);
}

fn click(view: &mut SatView, host: &MemoryParamHost, at: Point) {
    view.event(&UiEvent::PointerMove { pos: at, mods: MODS }, host);
    view.event(&UiEvent::PointerDown { pos: at, button: MouseButton::Left, mods: MODS }, host);
    view.event(&UiEvent::PointerUp { pos: at, button: MouseButton::Left, mods: MODS }, host);
    draw(view, host);
}

#[test]
fn dragging_the_curve_sets_drive_and_bias_as_one_undo_step() {
    let (mut view, host) = setup();
    let c = view.square(&host).center();
    drag(&mut view, &host, c, c + (50.0, -100.0), MODS);
    let (drive, bias) = (plain(&host, &band_id(0, B_DRIVE.id)), plain(&host, &band_id(0, B_BIAS.id)));
    assert!((drive - 20.0).abs() < 0.5, "drive {drive}");
    assert!((bias - 35.0).abs() < 1.0, "bias {bias}");
    view.event(&UiEvent::KeyDown { key: Key::Character('z'), mods: Modifiers { ctrl: true, ..MODS } }, &host);
    assert_eq!(plain(&host, &band_id(0, B_DRIVE.id)), 0.0, "one undo takes the whole drag back");
    assert_eq!(plain(&host, &band_id(0, B_BIAS.id)), 0.0);
}

#[test]
fn alt_drag_sets_tone_and_shift_is_fine() {
    let (mut view, host) = setup();
    let c = view.square(&host).center();
    drag(&mut view, &host, c, c + (0.0, -100.0), Modifiers { alt: true, ..MODS });
    assert!((plain(&host, &band_id(0, B_TONE.id)) - 6.0).abs() < 0.3);
    assert_eq!(plain(&host, &band_id(0, B_DRIVE.id)), 0.0, "Alt leaves drive alone");
    drag(&mut view, &host, c, c + (0.0, -100.0), Modifiers { shift: true, ..MODS });
    assert!((plain(&host, &band_id(0, B_DRIVE.id)) - 4.0).abs() < 0.2);
}

#[test]
fn the_wheel_drives_and_a_double_click_resets() {
    let (mut view, host) = setup();
    let c = view.square(&host).center();
    view.event(&UiEvent::Wheel { pos: c, lines: 4.0, mods: MODS }, &host);
    assert!((plain(&host, &band_id(0, B_DRIVE.id)) - 2.0).abs() < 1e-4);
    host.set_plain(&band_id(0, B_BIAS.id), 40.0);
    view.event(&UiEvent::DoubleClick { pos: c, mods: MODS }, &host);
    assert_eq!(plain(&host, &band_id(0, B_DRIVE.id)), 0.0);
    assert_eq!(plain(&host, &band_id(0, B_BIAS.id)), 0.0);
}

#[test]
fn model_icons_pick_the_model_and_keys_step_through_them() {
    let (mut view, host) = setup();
    // All eight are offered; Tape is the first.
    let tape = view.model_icon(&host, Model::Tape).expect("offered").center();
    click(&mut view, &host, tape);
    assert_eq!(plain(&host, &band_id(0, B_MODEL.id)) as usize, Model::Tape.index());
    let fold = view.model_icon(&host, Model::Fold).expect("offered").center();
    click(&mut view, &host, fold);
    assert_eq!(plain(&host, &band_id(0, B_MODEL.id)) as usize, Model::Fold.index());
    view.event(&UiEvent::KeyDown { key: Key::Right, mods: MODS }, &host);
    assert_eq!(plain(&host, &band_id(0, B_MODEL.id)) as usize, Model::Rectify.index());
    view.event(&UiEvent::KeyDown { key: Key::Up, mods: MODS }, &host);
    assert_eq!(plain(&host, &band_id(0, B_DRIVE.id)), 0.5);
}

#[test]
fn the_power_button_bypasses_the_band() {
    let (mut view, host) = setup();
    let power = view.band_power(&host, 0).expect("the card has one").center();
    click(&mut view, &host, power);
    assert_eq!(plain(&host, &band_id(0, B_BYPASS.id)), 1.0);
}

#[test]
fn the_oversampling_list_sets_the_factor() {
    let (mut view, host) = setup();
    click(&mut view, &host, Point::new(60.0, 704.0 + 28.0));
    let rows = view.oversampling_rows();
    assert_eq!(rows.len(), 5, "the list is open, 1× to 16×");
    click(&mut view, &host, rows[4].center());
    assert_eq!(plain(&host, OVERSAMPLING.id), 4.0, "16×");
    assert!(view.oversampling_rows().is_empty(), "and closed");
}

fn layout(host: &MemoryParamHost) -> Layout {
    Layout::new(&mite_sat::editor::settings(host).crossovers)
}

fn split(host: &MemoryParamHost, at: &[(usize, f32)]) {
    for &(k, f) in at {
        host.set_plain(&cross_id(k, X_ON.id), 1.0);
        host.set_plain(&cross_id(k, X_FREQ.id), f);
    }
}

#[test]
fn pulling_a_crossover_in_makes_a_band_with_the_same_sound() {
    let (mut view, host) = setup();
    host.set_plain(&band_id(0, B_DRIVE.id), 14.0);
    host.set_plain(&band_id(0, B_MODEL.id), Model::Fold.index() as f32);
    draw(&mut view, &host);
    let y = view.rail_y();
    let (from, to) = (Point::new(view.rail_x(20_000.0) - 6.0, y), Point::new(view.rail_x(2000.0), y));
    drag(&mut view, &host, from, to, MODS);
    let l = layout(&host);
    assert_eq!(l.n, 1);
    let slot = l.crossovers[0];
    let f = plain(&host, &cross_id(slot, X_FREQ.id));
    assert!((f / 2000.0 - 1.0).abs() < 0.02, "{f}");
    assert_eq!(plain(&host, &band_id(slot + 1, B_DRIVE.id)), 14.0);
    assert_eq!(plain(&host, &band_id(slot + 1, B_MODEL.id)) as usize, Model::Fold.index());
    assert_eq!(view.selected(), slot + 1);
    // One undo takes the split away.
    view.event(&UiEvent::KeyDown { key: Key::Character('z'), mods: Modifiers { ctrl: true, ..MODS } }, &host);
    assert_eq!(layout(&host).n, 0);
}

#[test]
fn double_clicking_the_rail_splits_until_six_bands() {
    let (mut view, host) = setup();
    for (k, f) in [80.0, 300.0, 1000.0, 3000.0, 9000.0, 16_000.0].into_iter().enumerate() {
        let at = Point::new(view.rail_x(f), view.rail_y());
        view.event(&UiEvent::PointerMove { pos: at, mods: MODS }, &host);
        view.event(&UiEvent::DoubleClick { pos: at, mods: MODS }, &host);
        draw(&mut view, &host);
        assert_eq!(layout(&host).n, (k + 1).min(5), "after {f} Hz");
    }
}

#[test]
fn dragging_a_crossover_onto_its_neighbour_merges_and_the_growing_band_survives() {
    let (mut view, host) = setup();
    split(&host, &[(0, 150.0), (1, 300.0), (2, 3000.0)]);
    host.set_plain(&band_id(2, B_DRIVE.id), 20.0);
    draw(&mut view, &host);
    let y = view.rail_y();
    let (from, to) = (Point::new(view.rail_x(300.0), y), Point::new(view.rail_x(150.0) + 2.0, y));
    drag(&mut view, &host, from, to, MODS);
    let l = layout(&host);
    assert_eq!(l.bands(), &[0, 2, 3]);
    assert_eq!(plain(&host, &band_id(2, B_DRIVE.id)), 20.0);
}

#[test]
fn a_narrow_card_s_curve_drives_its_own_band_and_selects_it() {
    let (mut view, host) = setup();
    split(&host, &[(0, 200.0), (1, 2000.0)]);
    draw(&mut view, &host);
    assert_eq!(view.selected(), 0);
    let sq = view.band_square(&host, 2).expect("band 3 has a card");
    assert!(sq.width() < view.square(&host).width(), "a narrow card's square: {sq:?}");
    drag(&mut view, &host, sq.center(), sq.center() + (0.0, -50.0), MODS);
    assert!((plain(&host, &band_id(2, B_DRIVE.id)) - 10.0).abs() < 0.3);
    assert_eq!(plain(&host, &band_id(0, B_DRIVE.id)), 0.0, "the others stay clean");
    assert_eq!(view.selected(), 2);
    // Now wide, its model icons are on offer.
    let fold = view.model_icon(&host, Model::Fold).expect("offered").center();
    click(&mut view, &host, fold);
    assert_eq!(plain(&host, &band_id(2, B_MODEL.id)) as usize, Model::Fold.index());
    assert_eq!(plain(&host, &band_id(0, B_MODEL.id)) as usize, Model::Tube.index());
}

#[test]
fn keys_solo_mute_and_remove_the_selected_band() {
    let (mut view, host) = setup();
    split(&host, &[(0, 150.0), (1, 3500.0)]);
    view.select(1);
    draw(&mut view, &host);
    view.event(&UiEvent::KeyDown { key: Key::Character('s'), mods: MODS }, &host);
    assert_eq!(plain(&host, &band_id(1, B_SOLO.id)), 1.0);
    view.event(&UiEvent::KeyDown { key: Key::Escape, mods: MODS }, &host);
    assert_eq!(plain(&host, &band_id(1, B_SOLO.id)), 0.0, "Esc lets go of solo");
    view.event(&UiEvent::KeyDown { key: Key::Character('m'), mods: MODS }, &host);
    assert_eq!(plain(&host, &band_id(1, B_MUTE.id)), 1.0);
    view.event(&UiEvent::KeyDown { key: Key::Delete, mods: MODS }, &host);
    draw(&mut view, &host);
    assert_eq!(layout(&host).bands(), &[0, 2]);
    assert_eq!(view.selected(), 0);
}

#[test]
fn the_band_menu_copies_and_pastes_a_band() {
    let (mut view, host) = setup();
    split(&host, &[(0, 400.0)]);
    host.set_plain(&band_id(0, B_DRIVE.id), 9.0);
    host.set_plain(&band_id(0, B_MIX.id), 40.0);
    draw(&mut view, &host);
    view.open_menu(0, &host);
    let rows = view.menu_rows();
    click(&mut view, &host, rows[2].center());
    view.open_menu(1, &host);
    let rows = view.menu_rows();
    assert_eq!(rows.len(), 7, "solo, mute, copy, paste, reset, merge down, remove");
    click(&mut view, &host, rows[3].center());
    assert_eq!(plain(&host, &band_id(1, B_DRIVE.id)), 9.0);
    assert_eq!(plain(&host, &band_id(1, B_MIX.id)), 40.0);
    assert!(view.menu_rows().is_empty(), "closed");
}

#[test]
fn the_layout_settles_and_stays_put() {
    let shared = Arc::new(SatShared::new());
    let mut view = SatView::new(shared);
    let host = memory_host();
    split(&host, &[(0, 500.0)]);
    let mut seen = Vec::new();
    for _ in 0..80 {
        std::thread::sleep(std::time::Duration::from_millis(5));
        view.tick(&host);
        seen.push(view.rail_y());
    }
    assert!(seen.windows(2).all(|w| w[1] <= w[0]), "the rail only rises: {seen:?}");
    assert!(seen.windows(2).rev().take(5).all(|w| w[0] == w[1]), "and rests");
}

/// Ticks long enough for the bars to measure and settle.
fn settle_bars(view: &mut SatView, host: &MemoryParamHost) -> [(f32, bool); 8] {
    // The bars fall with a 300 ms time constant, from as high as 0 dB.
    for _ in 0..100 {
        view.tick(host);
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    view.harmonics()
}

fn shared_at(fs: f32) -> (SatView, MemoryParamHost) {
    let shared = Arc::new(SatShared::new());
    shared.sample_rate.store(fs);
    let dir = std::env::temp_dir().join(format!("mitefx-sat-bars-{}", std::process::id()));
    let mut view = SatView::new(shared).with_preset_dir(dir);
    let host = memory_host();
    draw(&mut view, &host);
    (view, host)
}

#[test]
fn the_bars_show_what_the_band_does() {
    let (mut view, host) = shared_at(48_000.0);
    // Clean: no drive, no harmonics.
    assert!(settle_bars(&mut view, &host).iter().all(|&(db, _)| db < -71.0), "drive 0 makes nothing");
    // Soft is symmetric: odd harmonics only.
    host.set_plain(&band_id(0, B_MODEL.id), Model::Soft.index() as f32);
    host.set_plain(&band_id(0, B_DRIVE.id), 42.0);
    let bars = settle_bars(&mut view, &host);
    assert!(bars[1].0 > -40.0, "H3 {}", bars[1].0);
    assert!(bars[0].0 < -71.0 && bars[2].0 < -71.0, "H2 {} H4 {}", bars[0].0, bars[2].0);
    // Biased, the even ones arrive.
    host.set_plain(&band_id(0, B_BIAS.id), 50.0);
    assert!(settle_bars(&mut view, &host)[0].0 > -40.0, "biased H2");
    // Bypassed: clean again.
    host.set_plain(&band_id(0, B_BYPASS.id), 1.0);
    assert!(settle_bars(&mut view, &host).iter().all(|&(db, _)| db < -71.0), "bypass makes nothing");
}

#[test]
fn a_harmonic_above_nyquist_is_hollow() {
    let (mut view, host) = shared_at(48_000.0);
    host.set_plain(&cross_id(0, X_ON.id), 1.0);
    host.set_plain(&cross_id(0, X_FREQ.id), 6000.0);
    host.set_plain(&band_id(1, B_DRIVE.id), 18.0);
    view.select(1);
    draw(&mut view, &host);
    // Band 2's centre is √(6 k · 20 k) ≈ 11 kHz: H2 (22 kHz) is under 24 kHz, H3 is not.
    let bars = settle_bars(&mut view, &host);
    assert!(!bars[0].1);
    assert!(bars[1..].iter().all(|b| b.1), "{bars:?}");
}

#[test]
fn the_bars_read_and_never_write() {
    let (mut view, host) = shared_at(48_000.0);
    host.set_plain(&cross_id(0, X_ON.id), 1.0);
    draw(&mut view, &host);
    let r = view.bars(&host).expect("the selected card has bars");
    let before = plain(&host, &band_id(0, B_DRIVE.id));
    drag(&mut view, &host, r.center(), r.center() + (0.0, -40.0), MODS);
    assert_eq!(plain(&host, &band_id(0, B_DRIVE.id)), before);
    assert_eq!(view.selected(), 0);
}

#[test]
fn the_stereo_list_sets_the_band_and_a_narrow_card_has_its_badge() {
    let (mut view, host) = shared_at(48_000.0);
    let button = view.band_stereo(&host, 0).expect("the card has one");
    click(&mut view, &host, button.center());
    let rows = view.stereo_rows();
    assert_eq!(rows.len(), 6, "Stereo, Mid, Side, M/S, Left, Right");
    click(&mut view, &host, rows[2].center());
    assert_eq!(plain(&host, &band_id(0, B_STEREO.id)), 2.0);
    assert!(view.stereo_rows().is_empty());
    assert_eq!(mite_sat::editor::settings(&host).bands[0].stereo, mite_dsp::sat::StereoMode::Side);
    host.set_plain(&cross_id(0, X_ON.id), 1.0);
    view.select(0);
    draw(&mut view, &host);
    // A narrow card picks it too (user, 2026-11-10: it was missed): its badge opens the list.
    let badge = view.band_stereo(&host, 1).expect("a narrow card has its badge");
    click(&mut view, &host, badge.center());
    let rows = view.stereo_rows();
    assert_eq!(rows.len(), 6);
    click(&mut view, &host, rows[4].center());
    assert_eq!(mite_sat::editor::settings(&host).bands[1].stereo, mite_dsp::sat::StereoMode::Left);
}

#[test]
fn the_dynamics_dials_sit_on_the_wide_card_and_not_the_narrow_ones() {
    let (mut view, host) = setup();
    let d = view.dial(&band_id(0, B_DYN.id)).expect("the single card shows DYNAMICS");
    drag(&mut view, &host, d, d + (0.0, -60.0), MODS);
    assert!(plain(&host, &band_id(0, B_DYN.id)) > 10.0, "dragging up lifts");
    assert!(view.dial(&band_id(0, B_DYN_THR.id)).is_none(), "the threshold is automatic: no dial");
    assert!(view.dial(&band_id(0, B_DYN_ATT.id)).is_some());
    split(&host, &[(0, 400.0)]);
    draw(&mut view, &host);
    assert!(view.dial(&band_id(0, B_DYN.id)).is_some(), "the selected band's wide card has them");
    assert!(view.dial(&band_id(1, B_DYN.id)).is_none(), "a narrow card does not");
    assert!(view.dial(&band_id(1, B_DRIVE.id)).is_some());
}

#[test]
fn the_matrix_opens_and_routes_a_band() {
    let (mut view, host) = setup();
    let click = |view: &mut SatView, at: Point| {
        view.event(&UiEvent::PointerMove { pos: at, mods: Modifiers::default() }, &host);
        view.event(&UiEvent::PointerDown { pos: at, button: MouseButton::Left, mods: Modifiers::default() }, &host);
        view.event(&UiEvent::PointerUp { pos: at, button: MouseButton::Left, mods: Modifiers::default() }, &host);
    };
    let button = view.matrix_button().center();
    click(&mut view, button);
    let target = view.matrix_panel().expect("open").slot_target_rect(0).center();
    click(&mut view, target);
    let t = mite_sat::params::MOD_TARGETS.iter().position(|n| *n == "Band 1 Drive").expect("target");
    let cell = view.matrix_panel().expect("open").pick_rects()[t].center();
    click(&mut view, cell);
    assert_eq!((plain(&host, "m1_dst").round(), plain(&host, "m1_src"), plain(&host, "m1_amt")), (t as f32, 1.0, 25.0));
    view.event(&UiEvent::KeyDown { key: Key::Escape, mods: Modifiers::default() }, &host);
    assert!(view.matrix_panel().is_none());
}

#[test]
fn drawing_on_the_bars_makes_the_band_harmonic_in_one_undo_step() {
    // On Tube, a drag up the 3rd harmonic's column: the band becomes Harmonic, its recipe seeded
    // from the bars, the 3rd where the drag ended (near the top: close to 0 dB); one undo takes
    // the model and the recipe back.
    let (mut view, host) = setup();
    host.set_plain(&band_id(0, B_DRIVE.id), 30.0);
    draw(&mut view, &host);
    let r = view.bars(&host).expect("the single card has bars");
    let col = |i: usize| r.x0 + 14.0 + (r.width() - 28.0) * (i as f64 + 0.5) / 8.0;
    let (from, to) = (Point::new(col(1), r.y1 - 30.0), Point::new(col(1), r.y0 + 16.0));
    drag(&mut view, &host, from, to, MODS);
    assert_eq!(plain(&host, &band_id(0, B_MODEL.id)), Model::Harmonic.index() as f32);
    let h3 = plain(&host, &band_id(0, B_HARM[1].id));
    assert!(h3 > -6.0, "H3 dragged near the top: {h3} dB");
    view.event(&UiEvent::KeyDown { key: Key::Character('z'), mods: Modifiers { ctrl: true, ..MODS } }, &host);
    assert_eq!(plain(&host, &band_id(0, B_MODEL.id)), Model::Tube.index() as f32, "one undo: the model");
    assert_eq!(plain(&host, &band_id(0, B_HARM[1].id)), B_HARM[1].default, "and the recipe");
}
