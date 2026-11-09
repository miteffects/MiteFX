//! The DeNoise editor driven through events, against a stand-in host and the real engine: the
//! threshold line's grips, LEARN (start, stop, forget), ADAPTIVE, the dials.

use std::sync::atomic::Ordering;
use std::sync::Arc;

use mite_denoise::dsp::DenoiseEngine;
use mite_denoise::editor::{memory_host, out_gain, settings, DenoiseView, Grip};
use mite_denoise::params::{ADAPTIVE, EMPHASIS, REDUCTION, THRESHOLD};
use mite_denoise::DenoiseShared;
use mite_ui::kurbo::Point;
use mite_ui::{DrawCx, Key, MemoryParamHost, Modifiers, MouseButton, ParamHost, Theme, ThemeKind, UiEvent, View};

const MODS: Modifiers = Modifiers { shift: false, alt: false, ctrl: false, meta: false };

fn plain(host: &MemoryParamHost, id: &str) -> f32 {
    host.plain(id, host.normalized(id))
}

fn draw(view: &mut DenoiseView, host: &MemoryParamHost) {
    let theme = Theme::load(ThemeKind::Dark, mite_denoise::editor::ACCENT);
    let mut text = mite_ui::Typesetter::new();
    view.draw(&mut DrawCx { scene: &mut mite_ui::vello::Scene::new(), theme: &theme, text: &mut text, host, size: view.logical_size(), fx: &mut Default::default() });
}

/// Two seconds of noise through the engine, so the floor and threshold are drawn.
fn run(engine: &mut DenoiseEngine, shared: &DenoiseShared, host: &MemoryParamHost, seconds: f32) {
    let mut seed = 9u32;
    for _ in 0..(seconds * 100.0) as usize {
        let (mut l, mut r): (Vec<f32>, Vec<f32>) = (0..480)
            .map(|_| {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                let v = (seed as f32 / u32::MAX as f32 - 0.5) * 0.02;
                (v, -v)
            })
            .unzip();
        engine.process(&mut [&mut l, &mut r], &settings(host), out_gain(host), &shared.profile, &shared.tap);
    }
}

fn setup() -> (DenoiseView, MemoryParamHost, Arc<DenoiseShared>, DenoiseEngine) {
    let shared = Arc::new(DenoiseShared::new());
    let host = memory_host();
    let mut engine = DenoiseEngine::new(48_000.0);
    run(&mut engine, &shared, &host, 2.0);
    let dir = std::env::temp_dir().join(format!("mitefx-denoise-editor-{}", std::process::id()));
    let mut view = DenoiseView::new(shared.clone()).with_preset_dir(dir);
    view.tick(&host);
    view.settle();
    draw(&mut view, &host);
    (view, host, shared, engine)
}

fn drag(view: &mut DenoiseView, host: &MemoryParamHost, from: Point, to: Point) {
    view.event(&UiEvent::PointerMove { pos: from, mods: MODS }, host);
    view.event(&UiEvent::PointerDown { pos: from, button: MouseButton::Left, mods: MODS }, host);
    for k in 1..=8 {
        view.event(&UiEvent::PointerMove { pos: from.lerp(to, f64::from(k) / 8.0), mods: MODS }, host);
    }
    view.event(&UiEvent::PointerUp { pos: to, button: MouseButton::Left, mods: MODS }, host);
}

fn click(view: &mut DenoiseView, host: &MemoryParamHost, at: Point) {
    view.event(&UiEvent::PointerMove { pos: at, mods: MODS }, host);
    view.event(&UiEvent::PointerDown { pos: at, button: MouseButton::Left, mods: MODS }, host);
    view.event(&UiEvent::PointerUp { pos: at, button: MouseButton::Left, mods: MODS }, host);
}

#[test]
fn the_threshold_grip_moves_the_threshold_at_the_plots_scale_and_undoes() {
    let (mut view, host, _, _) = setup();
    let g = view.grip(Grip::Threshold).expect("a floor: the grip is drawn");
    let per_db = view.level_y(-61.0) - view.level_y(-60.0);
    drag(&mut view, &host, g, g - (0.0, 4.0 * per_db));
    assert!((plain(&host, THRESHOLD.id) - 10.0).abs() < 0.2, "{}", plain(&host, THRESHOLD.id));
    view.event(&UiEvent::KeyDown { key: Key::Character('z'), mods: Modifiers { ctrl: true, ..MODS } }, &host);
    assert_eq!(plain(&host, THRESHOLD.id), 6.0);
    // The emphasis grip tilts.
    let g = view.grip(Grip::Emphasis).expect("drawn");
    drag(&mut view, &host, g, g + (0.0, 3.0 * per_db));
    assert!((plain(&host, EMPHASIS.id) + 3.0).abs() < 0.2, "{}", plain(&host, EMPHASIS.id));
    // Double-click: flat again.
    let g = view.grip(Grip::Emphasis).expect("drawn");
    view.event(&UiEvent::DoubleClick { pos: g, mods: MODS }, &host);
    assert_eq!(plain(&host, EMPHASIS.id), 0.0);
}

#[test]
fn learn_starts_and_stops_and_the_profile_is_kept_then_forgotten() {
    let (mut view, host, shared, mut engine) = setup();
    let learn = view.learn_button().center();
    click(&mut view, &host, learn);
    assert!(shared.tap.learn.load(Ordering::Relaxed));
    run(&mut engine, &shared, &host, 1.0);
    assert!(shared.tap.learning.load(Ordering::Relaxed));
    click(&mut view, &host, learn);
    run(&mut engine, &shared, &host, 0.05);
    assert!(shared.profile.is_learned() && engine.has_profile());
    // With ADAPTIVE off, the learned floor is used: still a threshold to grab.
    let adapt = view.adaptive_button().center();
    click(&mut view, &host, adapt);
    assert_eq!(plain(&host, ADAPTIVE.id), 0.0);
    run(&mut engine, &shared, &host, 0.5);
    view.tick(&host);
    view.settle();
    assert!(view.grip(Grip::Threshold).is_some());
    // Double-click LEARN: forgotten, and with ADAPTIVE off there is no floor.
    view.event(&UiEvent::DoubleClick { pos: learn, mods: MODS }, &host);
    assert!(!shared.profile.is_learned());
    run(&mut engine, &shared, &host, 0.5);
    assert!(!engine.has_profile());
    view.tick(&host);
    view.settle();
    assert!(view.grip(Grip::Threshold).is_none(), "no floor, no grip");
}

#[test]
fn the_dials_set_their_parameters() {
    let (mut view, host, _, _) = setup();
    let d = view.dial(REDUCTION.id).expect("REDUCTION dial");
    drag(&mut view, &host, d, d - (0.0, 60.0));
    assert!(plain(&host, REDUCTION.id) > 12.0, "{}", plain(&host, REDUCTION.id));
    for id in ["threshold", "attack", "release", "smoothing", "emphasis"] {
        assert!(view.dial(id).is_some(), "{id}");
    }
}
