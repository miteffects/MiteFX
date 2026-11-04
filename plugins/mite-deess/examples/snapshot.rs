//! Renders the Mite DeEss editor to a PNG with a voice (vowels and sibilants) run through the real
//! engine first, so the history, spectra, reduction and meters are genuine.
//!
//!     cargo run -p mite_deess --example snapshot -- out.png [scale] [dark|light|hc|cb]
//!
//! SNAPSHOT_VARIANT: default, band, wide. SNAPSHOT_HOVER=x,y hovers a point.

#[path = "common/voice.rs"]
mod voice;

use std::sync::Arc;

use mite_deess::dsp::DeEssEngine;
use mite_deess::editor::{memory_host, out_gain, settings, DeEssView, ACCENT};
use mite_deess::DeEssShared;
use mite_ui::kurbo::Point;
use mite_ui::render::render_view;
use mite_ui::{Modifiers, Theme, ThemeKind, UiEvent, View};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let path = args.next().unwrap_or_else(|| "mite-deess.png".into());
    let scale: f64 = args.next().map_or(Ok(1.5), |s| s.parse())?;
    let theme = match args.next().as_deref() {
        Some("light") => ThemeKind::Light,
        Some("hc") => ThemeKind::HighContrast,
        Some("cb") => ThemeKind::ColorBlind,
        _ => ThemeKind::Dark,
    };
    let host = memory_host();
    match std::env::var("SNAPSHOT_VARIANT").as_deref() {
        Ok("band") => host.set_plain("reduce", 1.0),
        Ok("wide") => host.set_plain("reduce", 2.0),
        _ => {}
    }
    let shared = Arc::new(DeEssShared::new());
    let fs = 48_000.0;
    shared.sample_rate.store(fs as f32);
    let s = settings(&host);
    let mut engine = DeEssEngine::new(fs as f32, &s, out_gain(&host));
    // Stop inside a sibilant of a loud phrase, so the reduction shows.
    let x = voice::voice(6.33, fs);
    for block in x.chunks(480) {
        let (mut l, mut r) = (block.to_vec(), block.to_vec());
        engine.process(&mut [&mut l, &mut r], &s, out_gain(&host), &shared.history, Some([&shared.spectrum, &shared.output]));
    }
    let mut view = DeEssView::new(shared.clone());
    view.tick(&host);
    view.settle_spectrum();
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
