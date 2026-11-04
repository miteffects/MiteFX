//! Renders the Mite Multiband editor to a PNG with music run through the real engine first.
//!
//!     cargo run -p mite_multiband --example snapshot -- out.png [scale] [dark|light|hc|cb]
//!
//! SNAPSHOT_VARIANT: default (three bands), single, six, solo, bypass (band 1), auto (auto gain), delta, eq (the sidechain EQ open,
//! bands keyed from it). SNAPSHOT_SELECT=2 picks a band.

#[path = "common/music.rs"]
mod music;

use std::sync::Arc;

use mite_multiband::dsp::MbEngine;
use mite_multiband::editor::{memory_host, settings, MbView, ACCENT};
use mite_multiband::params::*;
use mite_multiband::MbShared;
use mite_ui::render::render_view;
use mite_ui::{Theme, ThemeKind, View};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let path = args.next().unwrap_or_else(|| "mite-multiband.png".into());
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
    let crossovers: &[(usize, f32)] = if v("single") { &[] } else if v("six") { &[(0, 90.0), (1, 300.0), (2, 1200.0), (3, 4000.0), (4, 10000.0)] } else { &[(0, 150.0), (2, 2500.0)] };
    for &(k, f) in crossovers {
        host.set_plain(&cross_id(k, X_ON.id), 1.0);
        host.set_plain(&cross_id(k, X_FREQ.id), f);
    }
    for b in 0..6 {
        host.set_plain(&band_id(b, B_THRESH.id), -30.0 + b as f32 * 2.0);
        host.set_plain(&band_id(b, B_RATIO.id), 3.0);
    }
    host.set_plain(&band_id(3, B_QUAD.id), 3.0);
    if v("solo") {
        host.set_plain(&band_id(1, B_SOLO.id), 1.0);
    }
    if v("auto") {
        host.set_plain(AUTO_GAIN.id, 1.0);
    }
    if v("bypass") {
        host.set_plain(&band_id(0, B_BYPASS.id), 1.0);
    }
    if v("delta") {
        host.set_plain(DELTA.id, 1.0);
    }
    if v("eq") {
        host.set_plain(SC_SOURCE.id, 1.0);
        host.set_plain(&band_id(0, B_EXTERNAL.id), 1.0);
        host.set_plain(&band_id(1, B_EXTERNAL.id), 1.0);
        host.set_plain(&key_id(0, K_ON.id), 1.0);
        host.set_plain(&key_id(1, K_ON.id), 1.0);
        host.set_plain(&key_id(1, K_GAIN.id), 6.0);
    }
    let shared = Arc::new(MbShared::new());
    let sample_rate = 48_000.0;
    shared.sample_rate.store(sample_rate);
    let s = settings(&host);
    let mut engine = MbEngine::new(sample_rate, &s);
    let [mut l, mut r] = music::music(6.0, sample_rate);
    // The music keys itself through the sidechain input too (as if routed there).
    let key = music::music(6.0, sample_rate);
    for (i, (a, b)) in l.chunks_mut(480).zip(r.chunks_mut(480)).enumerate() {
        let k: [&[f32]; 2] = [&key[0][i * 480..i * 480 + a.len()], &key[1][i * 480..i * 480 + a.len()]];
        engine.process_keyed(&mut [a, b], Some(&k), &s, &shared.history, Some([&shared.spectrum, &shared.post]), Some(&shared.key_spectrum));
    }
    shared.latency.store(engine.latency() as f32);
    let mut view = MbView::new(shared.clone());
    if let Some(b) = std::env::var("SNAPSHOT_SELECT").ok().and_then(|v| v.parse().ok()) {
        view.select(b);
    }
    view.settle(&host);
    if v("eq") {
        view.open_eq();
        view.settle_spectrum();
    }
    // Let the meters, peaks and spectrum ease in as they would live.
    for _ in 0..30 {
        view.tick(&host);
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let theme_obj = Theme::load(theme, ACCENT);
    let image = render_view(&mut view, &host, &theme_obj, scale)?;
    let file = std::io::BufWriter::new(std::fs::File::create(&path)?);
    let mut encoder = png::Encoder::new(file, image.width, image.height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.write_header()?.write_image_data(&image.rgba)?;
    println!("wrote {path} ({}×{})", image.width, image.height);
    Ok(())
}
