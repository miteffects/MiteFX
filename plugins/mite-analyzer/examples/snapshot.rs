//! Renders the Mite Analyzer editor to a PNG, with music (a drum loop, bass and chords, a little
//! wider at the top) measured by the real engine first, so every reading is genuine.
//!
//!     cargo run -p mite_analyzer --example snapshot -- out.png [scale] [dark|light|hc|cb]
//!
//! SNAPSHOT_VARIANT: default, ms (Mid / Side), lr (Left / Right), menu (the target list open).
//! SNAPSHOT_HOVER=x,y hovers a point.

#[path = "common/music.rs"]
#[allow(dead_code)]
mod music;

use std::sync::Arc;

use mite_analyzer::dsp::AnalyzerEngine;
use mite_analyzer::editor::{memory_host, AnalyzerView, ACCENT};
use mite_analyzer::params::*;
use mite_analyzer::AnalyzerShared;
use mite_ui::kurbo::Point;
use mite_ui::render::render_view;
use mite_ui::{Modifiers, MouseButton, Theme, ThemeKind, UiEvent, View};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let path = args.next().unwrap_or_else(|| "mite-analyzer.png".into());
    let scale: f64 = args.next().map_or(Ok(1.5), |s| s.parse())?;
    let theme = match args.next().as_deref() {
        Some("light") => ThemeKind::Light,
        Some("hc") => ThemeKind::HighContrast,
        Some("cb") => ThemeKind::ColorBlind,
        _ => ThemeKind::Dark,
    };
    let variant = std::env::var("SNAPSHOT_VARIANT").unwrap_or_else(|_| "default".into());
    let v = |name: &str| variant == name;
    let host = memory_host();
    if v("ms") {
        host.set_plain(VIEW.id, 1.0);
    }
    if v("lr") {
        host.set_plain(VIEW.id, 2.0);
    }
    let shared = Arc::new(AnalyzerShared::new());
    let fs = 48_000.0;
    shared.sample_rate.store(fs);
    let mut engine = AnalyzerEngine::new(fs);
    let [mut l, mut r] = music::music(40.0, fs);
    for (a, b) in l.chunks_mut(480).zip(r.chunks_mut(480)) {
        engine.process(&[a, b], &shared.tel);
    }
    let mut view = AnalyzerView::new(shared.clone());
    view.tick(&host);
    view.settle(&host);
    let theme_obj = Theme::load(theme, ACCENT);
    render_view(&mut view, &host, &theme_obj, 1.0)?;
    let mods = Modifiers::default();
    if v("menu") {
        let pos = view.button("target").expect("button").center();
        view.event(&UiEvent::PointerMove { pos, mods }, &host);
        view.event(&UiEvent::PointerDown { pos, button: MouseButton::Left, mods }, &host);
        view.event(&UiEvent::PointerUp { pos, button: MouseButton::Left, mods }, &host);
    }
    if let Some(pos) = std::env::var("SNAPSHOT_HOVER").ok().and_then(|p| {
        let (x, y) = p.split_once(',')?;
        Some(Point::new(x.trim().parse().ok()?, y.trim().parse().ok()?))
    }) {
        view.event(&UiEvent::PointerMove { pos, mods }, &host);
    }
    let image = render_view(&mut view, &host, &theme_obj, scale)?;
    let file = std::io::BufWriter::new(std::fs::File::create(&path)?);
    let mut encoder = png::Encoder::new(file, image.width, image.height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.write_header()?.write_image_data(&image.rgba)?;
    println!("wrote {path} ({}×{})", image.width, image.height);
    Ok(())
}
