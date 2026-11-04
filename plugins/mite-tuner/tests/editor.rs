//! The Tuner editor against a stand-in host and a real input tap: the note and cents it reads,
//! the reference moving them, the nearest string, silence, the tuning list.

use std::sync::Arc;

use mite_tuner::dsp::TunerEngine;
use mite_tuner::editor::{memory_host, note_name, TunerView};
use mite_tuner::params::{REFERENCE, TUNING};
use mite_tuner::TunerShared;
use mite_ui::{DrawCx, MemoryParamHost, Modifiers, MouseButton, ParamHost, Theme, ThemeKind, UiEvent, View};

const MODS: Modifiers = Modifiers { shift: false, alt: false, ctrl: false, meta: false };

fn plain(host: &MemoryParamHost, id: &str) -> f32 {
    host.plain(id, host.normalized(id))
}

/// A view that has heard `hz` (a string's harmonics) for half a second.
fn hearing(hz: f64, host: &MemoryParamHost) -> TunerView {
    let shared = Arc::new(TunerShared::new());
    let fs = 48_000.0;
    shared.sample_rate.store(fs as f32);
    let dir = std::env::temp_dir().join(format!("mitefx-tuner-editor-{}", std::process::id()));
    let mut view = TunerView::new(shared.clone()).with_preset_dir(dir);
    let mut engine = TunerEngine::new(fs as f32, false);
    let x: Vec<f32> = (0..24_000)
        .map(|i| {
            let t = i as f64 / fs;
            (1..8).map(|k| (std::f64::consts::TAU * hz * f64::from(k) * t).sin() / f64::from(k)).sum::<f64>() as f32 * 0.2
        })
        .collect();
    for block in x.chunks(960) {
        let (mut l, mut r) = (block.to_vec(), block.to_vec());
        engine.process(&mut [&mut l, &mut r], false, &shared.history, Some(&shared.input));
        view.analyze(host);
    }
    view.tick(host);
    draw(&mut view, host);
    view
}

fn draw(view: &mut TunerView, host: &MemoryParamHost) {
    let theme = Theme::load(ThemeKind::Dark, mite_tuner::editor::ACCENT);
    let mut text = mite_ui::Typesetter::new();
    view.draw(&mut DrawCx { scene: &mut mite_ui::vello::Scene::new(), theme: &theme, text: &mut text, host, size: view.logical_size(), fx: &mut Default::default() });
}

#[test]
fn reads_the_note_and_how_far_off_it_is() {
    let host = memory_host();
    // An A2 eight cents flat.
    let view = hearing(110.0 * 2f64.powf(-8.0 / 1200.0), &host);
    let r = view.reading().expect("a reading");
    assert_eq!(r.note, 45);
    assert_eq!(note_name(r.note), ("A", false, 2));
    assert!((r.cents + 8.0).abs() < 0.3, "{}", r.cents);
    // Standard tuning: the A string.
    assert_eq!(view.nearest_string(&host), Some(1));
}

#[test]
fn the_reference_moves_the_cents() {
    let host = memory_host();
    host.set_plain(REFERENCE.id, 432.0);
    let view = hearing(110.0, &host);
    let r = view.reading().expect("a reading");
    // 110 Hz against A4 = 432: 31.77 cents sharp of that A.
    assert_eq!(r.note, 45);
    assert!((r.cents - 31.77).abs() < 0.3, "{}", r.cents);
}

#[test]
fn sharps_and_other_tunings() {
    let host = memory_host();
    host.set_plain(TUNING.id, 2.0);
    let view = hearing(73.42, &host);
    let r = view.reading().expect("a reading");
    assert_eq!(note_name(r.note), ("D", false, 2));
    // Drop D: the low string is D.
    assert_eq!(view.nearest_string(&host), Some(0));
    let view = hearing(138.59, &host);
    assert_eq!(note_name(view.reading().expect("a reading").note), ("C", true, 3));
}

#[test]
fn silence_reads_nothing() {
    let host = memory_host();
    let view = hearing(0.0, &host);
    assert!(view.reading().is_none());
}

#[test]
fn the_tuning_pill_lists_the_tunings() {
    let host = memory_host();
    let mut view = hearing(110.0, &host);
    let at = view.tuning_pill().center();
    view.event(&UiEvent::PointerMove { pos: at, mods: MODS }, &host);
    view.event(&UiEvent::PointerDown { pos: at, button: MouseButton::Left, mods: MODS }, &host);
    view.event(&UiEvent::PointerUp { pos: at, button: MouseButton::Left, mods: MODS }, &host);
    draw(&mut view, &host);
    let rows = view.menu_rows();
    assert_eq!(rows.len(), mite_tuner::params::TUNINGS.len());
    let bass = rows[8].center();
    view.event(&UiEvent::PointerMove { pos: bass, mods: MODS }, &host);
    view.event(&UiEvent::PointerDown { pos: bass, button: MouseButton::Left, mods: MODS }, &host);
    view.event(&UiEvent::PointerUp { pos: bass, button: MouseButton::Left, mods: MODS }, &host);
    assert_eq!(plain(&host, TUNING.id), 8.0);
    assert!(view.menu_rows().is_empty());
}
