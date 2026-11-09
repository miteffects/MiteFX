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
