//! Renders the Mite DeNoise editor to a PNG, with sound run through the real engine first (a
//! voice-like tone with harmonics over hiss and a little hum), so every curve is genuine.
//!
//!     cargo run -p mite_denoise --example snapshot -- out.png [scale] [dark|light|hc|cb]
//!
//! SNAPSHOT_VARIANT: default (adaptive), learned (a profile, adaptive off), learning, listen,
//! empty (no floor), hot (pointer on the threshold grip). SNAPSHOT_HOVER=x,y hovers a point.

#[path = "common/sound.rs"]
mod sound;

use std::sync::atomic::Ordering;
use std::sync::Arc;

use mite_denoise::dsp::DenoiseEngine;
use mite_denoise::editor::{memory_host, out_gain, settings, DenoiseView, Grip, ACCENT};
use mite_denoise::params::*;
use mite_denoise::DenoiseShared;
use mite_ui::kurbo::Point;
use mite_ui::render::render_view;
use mite_ui::{Modifiers, Theme, ThemeKind, UiEvent, View};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let path = args.next().unwrap_or_else(|| "mite-denoise.png".into());
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
    host.set_plain(REDUCTION.id, 18.0);
    if v("learned") || v("empty") {
        host.set_plain(ADAPTIVE.id, 0.0);
    }
    if v("listen") {
        host.set_plain(LISTEN.id, 1.0);
    }
    let shared = Arc::new(DenoiseShared::new());
    let fs = 48_000.0;
    shared.sample_rate.store(fs);
    let mut engine = DenoiseEngine::new(fs);
    shared.latency.store(engine.latency() as f32);
    let x = sound::sound(6.0, fs, 2.0);
    let (mut l, mut r) = (x.clone(), x.clone());
    let block = 480;
    for (k, (a, b)) in l.chunks_mut(block).zip(r.chunks_mut(block)).enumerate() {
        // Learn the first 1.5 s (noise only) for "learned"; keep learning for "learning".
        let t = (k * block) as f32 / fs;
        if v("learned") || v("learning") {
            shared.tap.learn.store(v("learning") || t < 1.5, Ordering::Relaxed);
        }
        let s = settings(&host);
        engine.process(&mut [a, b], &s, out_gain(&host), &shared.profile, &shared.tap);
    }
    let mut view = DenoiseView::new(shared.clone());
    view.tick(&host);
    view.settle();
    let theme_obj = Theme::load(theme, ACCENT);
    render_view(&mut view, &host, &theme_obj, 1.0)?;
    let mods = Modifiers::default();
    let hover = if v("hot") { view.grip(Grip::Threshold) } else { None };
    let hover = hover.or_else(|| {
        std::env::var("SNAPSHOT_HOVER").ok().and_then(|p| {
            let (x, y) = p.split_once(',')?;
            Some(Point::new(x.trim().parse().ok()?, y.trim().parse().ok()?))
        })
    });
    if let Some(pos) = hover {
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
