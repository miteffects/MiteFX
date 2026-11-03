//! Renders the Mite Gate editor to a PNG with a drum loop (and its bleed) run through the real
//! engine first, so the history, meters and the live dot are genuine.
//!
//!     cargo run -p mite_gate --example snapshot -- out.png [scale] [dark|light|hc|cb]
//!
//! SNAPSHOT_VARIANT: default, expander, ducker, rms, delta, listen, source (the list open), eq
//! (the sidechain EQ open, two bands on).
//! SNAPSHOT_HOVER=x,y hovers a point; SNAPSHOT_RANGE=48 picks the level range.

#[path = "common/music.rs"]
mod music;

use std::sync::Arc;

use mite_gate::dsp::GateEngine;
use mite_gate::editor::{memory_host, output, settings, GateView, ACCENT};
use mite_gate::params::*;
use mite_gate::GateShared;
use mite_ui::kurbo::Point;
use mite_ui::render::render_view;
use mite_ui::{Modifiers, Theme, ThemeKind, UiEvent, View};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let path = args.next().unwrap_or_else(|| "mite-gate.png".into());
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
    host.set_plain(THRESHOLD.id, -20.0);
    host.set_plain(HYSTERESIS.id, 6.0);
    if v("expander") {
        host.set_plain(MODE.id, 1.0);
        host.set_plain(RANGE.id, -24.0);
    }
    if v("ducker") {
        host.set_plain(MODE.id, 2.0);
        host.set_plain(RANGE.id, -12.0);
    }
    if v("rms") {
        host.set_plain(DETECTOR.id, 1.0);
    }
    if v("delta") {
        host.set_plain(DELTA.id, 1.0);
    }
    if v("listen") {
        host.set_plain(LISTEN.id, 1.0);
        host.set_plain(&key_id(0, K_ON.id), 1.0);
    }
    let shared = Arc::new(GateShared::new());
    let sample_rate = 48_000.0;
    shared.sample_rate.store(sample_rate);
    let (s, out) = (settings(&host), output(&host));
    let mut engine = GateEngine::new(sample_rate, &s, &out);
    // The loop with a quiet bleed under it (what a gate is for).
    let [mut l, mut r] = music::drum_loop(8.0, sample_rate);
    let mut seed = 0x1234_5678u32;
    for (a, b) in l.iter_mut().zip(r.iter_mut()) {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        let n = (seed as f32 / u32::MAX as f32 - 0.5) * 0.004;
        *a += n;
        *b += n;
    }
    for (a, b) in l.chunks_mut(480).zip(r.chunks_mut(480)) {
        engine.process(&mut [a, b], None, &s, &out, &shared.history, Some(&shared.key_spectrum));
    }
    shared.latency.store(engine.latency() as f32);
    if let Some(r) = std::env::var("SNAPSHOT_RANGE").ok().and_then(|v| v.parse::<f32>().ok()) {
        shared.display_range.store(r);
    }
    let mut view = GateView::new(shared.clone());
    view.tick(&host);
    view.settle();
    let theme_obj = Theme::load(theme, ACCENT);
    render_view(&mut view, &host, &theme_obj, 1.0)?;
    let mods = Modifiers::default();
    if v("eq") {
        host.set_plain(&key_id(0, K_ON.id), 1.0);
        host.set_plain(&key_id(1, K_ON.id), 1.0);
        host.set_plain(&key_id(1, K_GAIN.id), 6.0);
        host.set_plain(&key_id(5, K_ON.id), 1.0);
        host.set_plain(&key_id(5, K_SLOPE.id), 6.0);
        view.open_eq();
        view.settle_spectrum();
    }
    if v("source") {
        let at = view.source_button().center();
        view.event(&UiEvent::PointerDown { pos: at, button: mite_ui::MouseButton::Left, mods }, &host);
    }
    if let Ok(p) = std::env::var("SNAPSHOT_HOVER") {
        if let Some((x, y)) = p.split_once(',') {
            view.event(&UiEvent::PointerMove { pos: Point::new(x.trim().parse()?, y.trim().parse()?), mods }, &host);
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
