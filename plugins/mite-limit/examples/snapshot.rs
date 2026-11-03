//! Renders the Mite Limit editor to a PNG with music run through the real engine first, so the
//! history, meters and loudness are genuine.
//!
//!     cargo run -p mite_limit --example snapshot -- out.png [scale] [dark|light|hc|cb]
//!
//! SNAPSHOT_VARIANT: default, hot (pushed 12 dB), delta, custom, oversampling, quiet (no gain),
//! targets (the list open), preview (as Spotify plays it), custom-target, broadcast (EBU R128).
//! SNAPSHOT_HOVER=x,y hovers a point; SNAPSHOT_RANGE=24 picks the level range.

#[path = "common/music.rs"]
mod music;

use std::sync::Arc;

use mite_limit::dsp::LimitEngine;
use mite_limit::editor::{memory_host, settings, LimitView, ACCENT};
use mite_limit::params::*;
use mite_limit::LimitShared;
use mite_ui::kurbo::Point;
use mite_ui::render::render_view;
use mite_ui::{Modifiers, Theme, ThemeKind, UiEvent, View};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let path = args.next().unwrap_or_else(|| "mite-limit.png".into());
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
    host.set_plain(GAIN.id, if v("quiet") { 0.0 } else if v("hot") { 12.0 } else { 6.0 });
    if v("delta") {
        host.set_plain(DELTA.id, 1.0);
    }
    if v("custom") {
        host.set_plain(STYLE.id, 5.0);
    }
    if v("preview") {
        host.set_plain(PREVIEW.id, 1.0);
    }
    if v("custom-target") {
        host.set_plain(TARGET.id, 8.0);
        host.set_plain(TARGET_CUSTOM.id, -10.0);
    }
    if v("broadcast") {
        host.set_plain(TARGET.id, 6.0);
    }
    let shared = Arc::new(LimitShared::new());
    let sample_rate = 48_000.0;
    shared.sample_rate.store(sample_rate);
    let s = settings(&host);
    let mut engine = LimitEngine::new(sample_rate, &s);
    let [mut l, mut r] = music::music(8.0, sample_rate);
    for (a, b) in l.chunks_mut(480).zip(r.chunks_mut(480)) {
        engine.process(&mut [a, b], &s, &shared.history, &shared.loudness);
    }
    shared.latency.store(engine.latency() as f32);
    if let Some(r) = std::env::var("SNAPSHOT_RANGE").ok().and_then(|v| v.parse::<f32>().ok()) {
        shared.display_range.store(r);
    }

    let mut view = LimitView::new(shared.clone());
    view.tick(&host);
    view.settle();
    let theme_obj = Theme::load(theme, ACCENT);
    render_view(&mut view, &host, &theme_obj, 1.0)?;
    let mods = Modifiers::default();
    if v("oversampling") {
        view.open_oversampling(&host);
    }
    if v("targets") {
        view.open_targets(&host);
    }
    if let Ok(p) = std::env::var("SNAPSHOT_HOVER") {
        if let Some((x, y)) = p.split_once(',') {
            let pos = Point::new(x.trim().parse()?, y.trim().parse()?);
            view.event(&UiEvent::PointerMove { pos, mods }, &host);
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
