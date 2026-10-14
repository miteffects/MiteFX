//! Renders the Mite Gain editor to a PNG without a host or window.
//!
//!     cargo run -p mite_gain --example snapshot -- out.png [scale] [dark|light|hc|cb] [accent]

use std::sync::Arc;

use mite_core::StereoMeter;
use mite_gain::editor::{GainView, ACCENT};
use mite_gain::params::GAIN;
use mite_ui::render::render_view;
use mite_ui::{MemoryParamHost, Theme, ThemeKind, View};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let path = args.next().unwrap_or_else(|| "mite-gain.png".into());
    let scale: f64 = args.next().map_or(Ok(2.0), |s| s.parse())?;
    let theme = match args.next().as_deref() {
        Some("light") => ThemeKind::Light,
        Some("hc") => ThemeKind::HighContrast,
        Some("cb") => ThemeKind::ColorBlind,
        _ => ThemeKind::Dark,
    };
    let accent = args.next().unwrap_or_else(|| ACCENT.to_owned());

    let meter = Arc::new(StereoMeter::new(-150.0));
    meter.store(-9.0, -4.5);
    let host = MemoryParamHost::new().with(GAIN);
    host.set_plain(GAIN.id, -0.8);

    let mut view = GainView::new(meter);
    view.tick(&host);
    let image = render_view(&mut view, &host, &Theme::load(theme, &accent), scale)?;

    let file = std::io::BufWriter::new(std::fs::File::create(&path)?);
    let mut encoder = png::Encoder::new(file, image.width, image.height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.write_header()?.write_image_data(&image.rgba)?;
    println!("wrote {path} ({}×{})", image.width, image.height);
    Ok(())
}
