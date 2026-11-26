//! The Analyzer editor driven through events against a stand-in host and the real engine: the
//! footer's menus, HOLD, RESET, and the stereo readings on known signals.

use std::sync::atomic::Ordering;
use std::sync::Arc;

use mite_analyzer::dsp::AnalyzerEngine;
use mite_analyzer::editor::{memory_host, AnalyzerView};
use mite_analyzer::AnalyzerShared;
use mite_ui::kurbo::Point;
use mite_ui::{DrawCx, MemoryParamHost, Modifiers, MouseButton, ParamHost, Theme, ThemeKind, UiEvent, View};

const MODS: Modifiers = Modifiers { shift: false, alt: false, ctrl: false, meta: false };

fn plain(host: &MemoryParamHost, id: &str) -> f32 {
    host.plain(id, host.normalized(id))
}

fn draw(view: &mut AnalyzerView, host: &MemoryParamHost) {
    let theme = Theme::load(ThemeKind::Dark, mite_analyzer::editor::ACCENT);
    let mut text = mite_ui::Typesetter::new();
    view.draw(&mut DrawCx { scene: &mut mite_ui::vello::Scene::new(), theme: &theme, text: &mut text, host, size: view.logical_size(), fx: &mut Default::default() });
}

fn click(view: &mut AnalyzerView, host: &MemoryParamHost, pos: Point) {
    view.event(&UiEvent::PointerMove { pos, mods: MODS }, host);
    view.event(&UiEvent::PointerDown { pos, button: MouseButton::Left, mods: MODS }, host);
    view.event(&UiEvent::PointerUp { pos, button: MouseButton::Left, mods: MODS }, host);
}

/// Two seconds of `signal(t) -> (l, r)` through the engine, then an editor reading it.
fn setup(signal: impl Fn(f32) -> (f32, f32)) -> (AnalyzerView, MemoryParamHost, Arc<AnalyzerShared>) {
    let shared = Arc::new(AnalyzerShared::new());
    let host = memory_host();
    let mut engine = AnalyzerEngine::new(48_000.0);
    for b in 0..200 {
        let (mut l, mut r): (Vec<f32>, Vec<f32>) = (0..480).map(|i| signal((b * 480 + i) as f32 / 48_000.0)).unzip();
        engine.process(&[&mut l, &mut r], &shared.tel);
    }
    let dir = std::env::temp_dir().join(format!("mitefx-analyzer-editor-{}", std::process::id()));
    let mut view = AnalyzerView::new(shared.clone()).with_preset_dir(dir);
    view.tick(&host);
    view.settle(&host);
    draw(&mut view, &host);
    (view, host, shared)
}

fn sine(f: f32, t: f32) -> f32 {
    0.3 * (std::f32::consts::TAU * f * t).sin()
}

#[test]
fn the_footer_menus_set_their_parameters() {
    let (mut view, host, _) = setup(|t| (sine(440.0, t), sine(440.0, t)));
    for (name, id, pick) in [("target", "target", 2usize), ("view", "view", 1), ("tilt", "tilt", 0), ("speed", "speed", 2)] {
        let at = view.button(name).expect("button").center();
        click(&mut view, &host, at);
        let rows = view.menu_rows();
        assert!(rows.len() > pick, "{name}: its menu");
        click(&mut view, &host, rows[pick].center());
        assert_eq!(plain(&host, id), pick as f32, "{name}");
        assert!(view.menu_rows().is_empty(), "{name}: closed");
        draw(&mut view, &host);
    }
}

#[test]
fn hold_toggles_and_reset_restarts_the_integration() {
    let (mut view, host, shared) = setup(|t| (sine(440.0, t), sine(440.0, t)));
    let before = plain(&host, "hold");
    let at = view.button("hold").expect("button").center();
        click(&mut view, &host, at);
    assert_ne!(plain(&host, "hold"), before);
    let at = view.button("reset").expect("button").center();
        click(&mut view, &host, at);
    assert!(shared.tel.reset.load(Ordering::Relaxed));
    shared.tel.reset.store(false, Ordering::Relaxed);
    let at = view.button("integrated").expect("button").center();
        click(&mut view, &host, at);
    assert!(shared.tel.reset.load(Ordering::Relaxed), "a click on the integrated reading too");
}

#[test]
fn the_stereo_readings_tell_mono_from_wide_from_out_of_phase() {
    // Mono: correlation +1 everywhere.
    let (view, _, _) = setup(|t| {
        let v = sine(80.0, t) + sine(1000.0, t) + sine(8000.0, t);
        (v, v)
    });
    assert!(view.correlation() > 0.95, "mono {}", view.correlation());
    assert!(view.band_correlation().iter().filter(|c| **c != 0.0).all(|&c| c > 0.9), "{:?}", view.band_correlation());
    // The lows out of phase, the rest mono: the low band goes negative, the others stay positive.
    let (view, _, _) = setup(|t| (sine(80.0, t) + sine(3000.0, t), -sine(80.0, t) + sine(3000.0, t)));
    let b = view.band_correlation();
    assert!(b[0] < -0.9, "the lows out of phase: {b:?}");
    assert!(b[3] > 0.9, "the high mids mono: {b:?}");
}

/// The group-delay plot zooms and scrolls by hand only: the wheel zooms around the pointer, Alt
/// scrolls, a drag scrolls, a double-click puts it back; the music does not move it.
#[test]
fn the_group_delay_plot_zooms_and_scrolls_by_hand() {
    let (mut view, host, shared) = setup(|t| (sine(440.0, t), sine(440.0, t)));
    let page = view.button("phase_page").expect("the phase page");
    click(&mut view, &host, page.center());
    view.tick(&host);
    draw(&mut view, &host);
    let gd = mite_analyzer::editor::phase::GD;
    let start = view.phase().gd_range();
    for _ in 0..30 {
        view.tick(&host);
    }
    assert_eq!(view.phase().gd_range(), start, "steady with the music");
    let at = Point::new(gd.center().x, gd.center().y);
    view.event(&UiEvent::Wheel { pos: at, lines: 3.0, mods: MODS }, &host);
    let zoomed = view.phase().gd_range();
    assert!(zoomed.1 - zoomed.0 < 0.7 * (start.1 - start.0), "zoomed in: {start:?} → {zoomed:?}");
    let mid = (start.0 + start.1) / 2.0;
    assert!(zoomed.0 < mid + 1.0 && zoomed.1 > mid - 1.0, "around the pointer: {zoomed:?}");
    view.event(&UiEvent::Wheel { pos: at, lines: 1.0, mods: Modifiers { alt: true, ..MODS } }, &host);
    let scrolled = view.phase().gd_range();
    assert!(scrolled.0 > zoomed.0 && ((scrolled.1 - scrolled.0) - (zoomed.1 - zoomed.0)).abs() < 1e-9, "scrolled up, same span");
    view.event(&UiEvent::PointerMove { pos: at, mods: MODS }, &host);
    view.event(&UiEvent::PointerDown { pos: at, button: MouseButton::Left, mods: MODS }, &host);
    view.event(&UiEvent::PointerMove { pos: at + (0.0, 40.0), mods: MODS }, &host);
    view.event(&UiEvent::PointerUp { pos: at + (0.0, 40.0), button: MouseButton::Left, mods: MODS }, &host);
    assert!(view.phase().gd_range().0 > scrolled.0, "dragged down: the view moves up");
    view.event(&UiEvent::DoubleClick { pos: at, mods: MODS }, &host);
    assert_eq!(view.phase().gd_range(), start, "back as it was");
    let _ = shared;
}

/// The impulse zooms and scrolls by hand too: centred on 0 until then, back there on a
/// double-click.
#[test]
fn the_impulse_zooms_and_scrolls_by_hand() {
    let (mut view, host, _) = setup(|t| (sine(440.0, t), sine(440.0, t)));
    let page = view.button("phase_page").expect("the phase page");
    click(&mut view, &host, page.center());
    view.tick(&host);
    let ir = mite_analyzer::editor::phase::IR_PLOT;
    assert_eq!(view.phase().ir_window(), (10.0, 10.0), "centred on 0, ±10 ms");
    view.event(&UiEvent::Wheel { pos: ir.center(), lines: 4.0, mods: MODS }, &host);
    let (pre, post) = view.phase().ir_window();
    assert!(pre + post < 12.0 && (pre - post).abs() < 1e-9, "zoomed around 0 (the middle): {pre} {post}");
    view.event(&UiEvent::Wheel { pos: ir.center(), lines: 1.0, mods: Modifiers { alt: true, ..MODS } }, &host);
    assert!(view.phase().ir_window().0 > pre, "scrolled");
    view.event(&UiEvent::DoubleClick { pos: ir.center(), mods: MODS }, &host);
    assert_eq!(view.phase().ir_window(), (10.0, 10.0));
}
