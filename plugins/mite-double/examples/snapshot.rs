//! Renders the Mite Double editor to a PNG with a guitar riff run through the real engine first,
//! so the lanes, rings, correlation and meters are genuine.
//!
//!     cargo run -p mite_double --example snapshot -- out.png [scale] [dark|light|hc|cb]
//!
//! SNAPSHOT_VARIANT: default, quad (three doubles). SNAPSHOT_HOVER=x,y hovers a point.

#[path = "common/riff.rs"]
mod riff;

use std::sync::Arc;

use mite_double::dsp::DoubleEngine;
use mite_double::editor::{memory_host, output, settings, DoubleView, ACCENT};
use mite_double::DoubleShared;
use mite_ui::kurbo::Point;
use mite_ui::render::render_view;
use mite_ui::{Modifiers, Theme, ThemeKind, UiEvent, View};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let path = args.next().unwrap_or_else(|| "mite-double.png".into());
    let scale: f64 = args.next().map_or(Ok(1.5), |s| s.parse())?;
    let theme = match args.next().as_deref() {
        Some("light") => ThemeKind::Light,
        Some("hc") => ThemeKind::HighContrast,
        Some("cb") => ThemeKind::ColorBlind,
        _ => ThemeKind::Dark,
    };
    let host = memory_host();
    if std::env::var("SNAPSHOT_VARIANT").as_deref() == Ok("quad") {
        host.set_plain("voices", 2.0);
        host.set_plain("timing", 22.0);
        host.set_plain("pitch", 10.0);
    }
    let shared = Arc::new(DoubleShared::new());
    let fs = 48_000.0;
    shared.sample_rate.store(fs as f32);
    let (s, out) = (settings(&host), output(&host));
    let mut engine = DoubleEngine::new(fs as f32, &out);
    let x = riff::riff(6.0, fs);
    for block in x.chunks(480) {
        let (mut l, mut r) = (block.to_vec(), vec![0.0f32; block.len()]);
        engine.process(&mut [&mut l, &mut r], false, &s, &out, &shared.history);
    }
    let mut view = DoubleView::new(shared.clone());
    view.tick(&host);
    let theme_obj = Theme::load(theme, ACCENT);
    render_view(&mut view, &host, &theme_obj, 1.0)?;
    if let Ok(p) = std::env::var("SNAPSHOT_HOVER") {
        if let Some((x, y)) = p.split_once(',') {
            view.event(&UiEvent::PointerMove { pos: Point::new(x.trim().parse()?, y.trim().parse()?), mods: Modifiers::default() }, &host);
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
