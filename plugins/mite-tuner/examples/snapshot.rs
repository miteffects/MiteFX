//! Renders the Mite Tuner editor to a PNG with a plucked string run through the real engine
//! first, so the reading, the trace and the meter are genuine.
//!
//!     cargo run -p mite_tuner --example snapshot -- out.png [scale] [dark|light|hc|cb]
//!
//! SNAPSHOT_HZ: the string's frequency (default 110 × 2^(−0.08/12): an A 8 cents flat);
//! SNAPSHOT_SILENT=1: nothing played; SNAPSHOT_CLICK=x,y clicks a point (the tuning pill).

#[path = "common/pluck.rs"]
mod common;

use std::sync::Arc;

use mite_tuner::dsp::TunerEngine;
use mite_tuner::editor::{memory_host, TunerView, ACCENT};
use mite_tuner::TunerShared;
use mite_ui::kurbo::Point;
use mite_ui::render::render_view;
use mite_ui::{Modifiers, MouseButton, Theme, ThemeKind, UiEvent, View};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let path = args.next().unwrap_or_else(|| "mite-tuner.png".into());
    let scale: f64 = args.next().map_or(Ok(1.5), |s| s.parse())?;
    let theme = match args.next().as_deref() {
        Some("light") => ThemeKind::Light,
        Some("hc") => ThemeKind::HighContrast,
        Some("cb") => ThemeKind::ColorBlind,
        _ => ThemeKind::Dark,
    };
    let hz: f64 = std::env::var("SNAPSHOT_HZ").ok().and_then(|s| s.parse().ok()).unwrap_or(110.0 * 2f64.powf(-0.08 / 12.0));
    let host = memory_host();
    let shared = Arc::new(TunerShared::new());
    let fs = 48_000.0;
    shared.sample_rate.store(fs as f32);
    let mut view = TunerView::new(shared.clone());
    let mut engine = TunerEngine::new(fs as f32, false);
    if std::env::var("SNAPSHOT_SILENT").is_err() {
        // Plucked from a little sharp, settling; the view reads it as it goes (a trace).
        let x = common::pluck(hz, 12.0, 3.0, 3.0, fs);
        for block in x.chunks(960) {
            let (mut l, mut r) = (block.to_vec(), block.to_vec());
            engine.process(&mut [&mut l, &mut r], false, &shared.history, Some(&shared.input));
            view.tick(&host);
            view.analyze(&host);
        }
    }
    let theme_obj = Theme::load(theme, ACCENT);
    render_view(&mut view, &host, &theme_obj, 1.0)?;
    if let Ok(p) = std::env::var("SNAPSHOT_CLICK") {
        if let Some((x, y)) = p.split_once(',') {
            let pos = Point::new(x.trim().parse()?, y.trim().parse()?);
            let mods = Modifiers::default();
            view.event(&UiEvent::PointerMove { pos, mods }, &host);
            view.event(&UiEvent::PointerDown { pos, button: MouseButton::Left, mods }, &host);
            view.event(&UiEvent::PointerUp { pos, button: MouseButton::Left, mods }, &host);
        }
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
