//! Renders the Mite Mod editor to a PNG with a guitar riff run through the real engine first,
//! so the lanes, rings, correlation and meters are genuine.
//!
//!     cargo run -p mite_mod --example snapshot -- out.png [scale] [dark|light|hc|cb]
//!
//! SNAPSHOT_VARIANT: a mode (chorus, ensemble, flanger, tz, phaser, vibrato, rotary: slow, then
//! fast for the last 3 s, micro) or synced.
//! SNAPSHOT_HOVER=x,y hovers a point.

#[path = "common/riff.rs"]
mod riff;

use std::sync::Arc;

use mite_mod::dsp::ModEngine;
use mite_mod::editor::{memory_host, output, settings, ModView, ACCENT};
use mite_mod::ModShared;
use mite_ui::kurbo::Point;
use mite_ui::render::render_view;
use mite_ui::{Modifiers, Theme, ThemeKind, UiEvent, View};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let path = args.next().unwrap_or_else(|| "mite-mod.png".into());
    let scale: f64 = args.next().map_or(Ok(1.5), |s| s.parse())?;
    let theme = match args.next().as_deref() {
        Some("light") => ThemeKind::Light,
        Some("hc") => ThemeKind::HighContrast,
        Some("cb") => ThemeKind::ColorBlind,
        _ => ThemeKind::Dark,
    };
    let host = memory_host();
    match std::env::var("SNAPSHOT_VARIANT").as_deref() {
        Ok("ensemble") => host.set_plain("mode", 1.0),
        Ok("flanger") => {
            host.set_plain("mode", 2.0);
            host.set_plain("feedback", 60.0);
            host.set_plain("depth", 70.0);
        }
        Ok("tz") => {
            host.set_plain("mode", 2.0);
            host.set_plain("thru_zero", 1.0);
            host.set_plain("manual", 2.5);
        }
        Ok("phaser") => {
            host.set_plain("mode", 3.0);
            host.set_plain("stages", 3.0);
            host.set_plain("feedback", 50.0);
        }
        Ok("vibrato") => {
            host.set_plain("mode", 4.0);
            host.set_plain("rate", 5.0);
        }
        Ok("rotary") => {
            host.set_plain("mode", 5.0);
            host.set_plain("depth", 70.0);
            host.set_plain("spread", 120.0);
            host.set_plain("mix", 100.0);
        }
        Ok("micro") => {
            host.set_plain("mode", 6.0);
            host.set_plain("depth", 30.0);
            host.set_plain("rate", 0.4);
        }
        Ok("synced") => {
            host.set_plain("sync", 1.0);
            host.set_plain("shape", 5.0);
        }
        Ok("chorus4") => {
            host.set_plain("voices", 3.0);
            host.set_plain("shape", 1.0);
        }
        _ => {}
    }
    let shared = Arc::new(ModShared::new());
    let fs = 48_000.0;
    shared.sample_rate.store(fs as f32);
    let (s, out) = (settings(&host), output(&host));
    let mut engine = ModEngine::new(fs as f32, &s, &out);
    let x = riff::riff(6.0, fs);
    let rotary = std::env::var("SNAPSHOT_VARIANT").as_deref() == Ok("rotary");
    for (k, block) in x.chunks(480).enumerate() {
        let (mut l, mut r) = (block.to_vec(), vec![0.0f32; block.len()]);
        let fast = mite_dsp::modulation::ModSettings { rotor: mite_dsp::modulation::Rotor::Fast, ..s };
        let s = if rotary && k * 480 >= 3 * 48_000 { fast } else { s };
        if rotary && k * 480 == 3 * 48_000 {
            host.set_plain("rotor", 1.0);
        }
        engine.process(&mut [&mut l, &mut r], false, &s, &out, &mite_dsp::modulation::Transport::default(), &shared.history);
    }
    let mut view = ModView::new(shared.clone());
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
