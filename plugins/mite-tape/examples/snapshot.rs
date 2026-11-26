//! Renders the Mite Tape editor to a PNG with music run through the real engine first, so the
//! spectra, the deck's meter and trace, the response and the meters are genuine.
//!
//!     cargo run -p mite_tape --example snapshot -- out.png [scale] [dark|light|hc|cb]
//!
//! SNAPSHOT_VARIANT: default, worn (3.75 ips, wow, flutter, hiss), hot (level +12), print (30 ips,
//! CCIR, a light touch), bypass.
//! SNAPSHOT_HOVER=x,y hovers a point.

#[path = "common/music.rs"]
mod music;

use std::sync::Arc;

use mite_tape::dsp::TapeEngine;
use mite_tape::editor::{memory_host, settings, TapeView, ACCENT};
use mite_tape::TapeShared;
use mite_ui::kurbo::Point;
use mite_ui::render::render_view;
use mite_ui::{Modifiers, Theme, ThemeKind, UiEvent, View};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let path = args.next().unwrap_or_else(|| "mite-tape.png".into());
    let scale: f64 = args.next().map_or(Ok(1.5), |s| s.parse())?;
    let theme = match args.next().as_deref() {
        Some("light") => ThemeKind::Light,
        Some("hc") => ThemeKind::HighContrast,
        Some("cb") => ThemeKind::ColorBlind,
        _ => ThemeKind::Dark,
    };
    let host = memory_host();
    match std::env::var("SNAPSHOT_VARIANT").as_deref() {
        Ok("worn") => {
            for (id, v) in [("speed", 0.0), ("wow", 70.0), ("flutter", 50.0), ("hiss", 40.0), ("level", 6.0)] {
                host.set_plain(id, v);
            }
        }
        Ok("hot") => host.set_plain("level", 12.0),
        Ok("print") => {
            for (id, v) in [("speed", 3.0), ("eq", 1.0), ("wow", 0.0), ("flutter", 2.0), ("level", -4.0)] {
                host.set_plain(id, v);
            }
        }
        Ok("bypass") => host.set_plain("bypass", 1.0),
        _ => {}
    }
    let shared = Arc::new(TapeShared::new());
    let fs = 48_000.0;
    shared.sample_rate.store(fs as f32);
    let s = settings(&host);
    let mut engine = TapeEngine::new(fs as f32, &s);
    let x = music::music(3.0, fs);
    let mut view = TapeView::new(shared.clone());
    for block in x.chunks(800) {
        let (mut l, mut r) = (block.to_vec(), block.to_vec());
        shared.process(&mut engine, &mut [&mut l, &mut r], &s);
        // A frame per 800 samples: the deck's trace follows.
        view.tick(&host);
    }
    view.settle(&host);
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
