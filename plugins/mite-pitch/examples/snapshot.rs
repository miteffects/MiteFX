//! Renders the Mite Pitch editor to a PNG with a sung take transferred through the real engine,
//! its second note dragged up a tone.
//!
//!     cargo run -p mite_pitch --example snapshot -- out.png [scale] [dark|light|hc|cb]
//!
//! SNAPSHOT_EMPTY=1: no take.

#[path = "common/take.rs"]
mod common;

use std::sync::Arc;

use mite_dsp::melody::NoteEdit;
use mite_pitch::PitchShared;
use mite_pitch::dsp::PitchEngine;
use mite_pitch::editor::{ACCENT, PitchView, memory_host};
use mite_ui::render::render_view;
use mite_ui::{Theme, ThemeKind, View};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let path = args.next().unwrap_or_else(|| "mite-pitch.png".into());
    let scale: f64 = args.next().map_or(Ok(1.5), |s| s.parse())?;
    let theme = match args.next().as_deref() {
        Some("light") => ThemeKind::Light,
        Some("hc") => ThemeKind::HighContrast,
        Some("cb") => ThemeKind::ColorBlind,
        _ => ThemeKind::Dark,
    };
    let host = memory_host();
    let s = common::session("snapshot");
    if std::env::var("SNAPSHOT_EMPTY").is_err() {
        let mut e = PitchEngine::new(common::FS as f32);
        common::transfer(&mut e, &s, &common::take(), 0);
        s.set_edit(
            1,
            NoteEdit {
                shift_cents: 200.0 - 25.0,
                ..NoteEdit::default()
            },
        );
        s.wait_idle(std::time::Duration::from_secs(30));
        common::play(&mut e, &s, &vec![0.0; 40_000], 0, 512);
    }
    let shared = Arc::new(PitchShared {
        session: s,
        sample_rate: mite_core::AtomicF32::new(common::FS as f32),
    });
    let mut view = PitchView::new(shared);
    view.tick(&host);
    let theme_obj = Theme::load(theme, ACCENT);
    render_view(&mut view, &host, &theme_obj, 1.0)?;
    let image = render_view(&mut view, &host, &theme_obj, scale)?;
    let file = std::io::BufWriter::new(std::fs::File::create(&path)?);
    let mut encoder = png::Encoder::new(file, image.width, image.height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.write_header()?.write_image_data(&image.rgba)?;
    println!("wrote {path} ({}×{})", image.width, image.height);
    Ok(())
}
