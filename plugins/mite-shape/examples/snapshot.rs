//! Renders the Mite Shape editor to a PNG in one of the design's states, with a drum loop run
//! through the real engine first, so the history, hits, meters and spectrum are genuine.
//!
//!     cargo run -p mite_shape --example snapshot -- out.png [scale] [dark|light|hc|cb]
//!
//! SNAPSHOT_VARIANT picks the state, as the design's `variant` prop: default, multiband,
//! band-selected, split, merge, solo, mute, stereo, detector, lookahead, delta, empty, presets,
//! menu.

#[path = "common/drums.rs"]
mod drums;

use std::sync::Arc;

use mite_shape::dsp::{Output, ShapeEngine};
use mite_shape::editor::{memory_host, ShapeView, ACCENT};
use mite_shape::params::*;
use mite_shape::ShapeShared;
use mite_ui::kurbo::Point;
use mite_ui::render::render_view;
use mite_ui::{MemoryParamHost, Modifiers, MouseButton, Theme, ThemeKind, UiEvent, View};

fn set_band(host: &MemoryParamHost, b: usize, attack: f32, sustain: f32, time: f32, transition: f32) {
    host.set_plain(&band_id(b, B_ATTACK.id), attack);
    host.set_plain(&band_id(b, B_SUSTAIN.id), sustain);
    host.set_plain(&band_id(b, B_TIME.id), time);
    host.set_plain(&band_id(b, B_TRANSITION.id), transition);
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let path = args.next().unwrap_or_else(|| "mite-shape.png".into());
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
    let multi = ["multiband", "band-selected", "solo", "mute", "menu", "merge"].iter().any(|n| v(n));
    if multi {
        host.set_plain(&cross_id(0, X_ON.id), 1.0);
        host.set_plain(&cross_id(0, X_FREQ.id), 150.0);
        host.set_plain(&cross_id(1, X_ON.id), 1.0);
        host.set_plain(&cross_id(1, X_FREQ.id), if v("merge") { 290.0 } else { 3500.0 });
        set_band(&host, 0, 5.0, -3.0, 30.0, 100.0);
        set_band(&host, 1, 3.0, -6.0, 40.0, 80.0);
        set_band(&host, 2, -3.0, -2.0, 15.0, 120.0);
        if v("merge") {
            host.set_plain(&cross_id(2, X_ON.id), 1.0);
            host.set_plain(&cross_id(2, X_FREQ.id), 3500.0);
            set_band(&host, 3, -3.0, -2.0, 15.0, 120.0);
        }
    } else if !v("empty") {
        set_band(&host, 0, 6.0, -4.0, 25.0, 100.0);
    }
    if v("solo") {
        host.set_plain(&band_id(1, B_SOLO.id), 1.0);
    }
    if v("mute") {
        host.set_plain(&band_id(2, B_MUTE.id), 1.0);
    }
    if v("stereo") {
        host.set_plain(STEREO.id, 1.0);
        host.set_plain(LINK.id, 50.0);
    }
    if v("detector") {
        host.set_plain(DET_HP.id, 90.0);
        host.set_plain(DET_LP.id, 6000.0);
        host.set_plain(LISTEN.id, 1.0);
    }
    if v("lookahead") {
        host.set_plain(LOOKAHEAD.id, 2.0);
    }
    if v("delta") {
        host.set_plain(DELTA.id, 1.0);
    }
    if v("clip") {
        host.set_plain(CLIP.id, 1.0);
        host.set_plain(CLIP_CEILING.id, -6.0);
        host.set_plain(&band_id(0, B_ATTACK.id), 9.0);
        host.set_plain(OUT_GAIN.id, 3.0);
    }
    if v("length") {
        host.set_plain(&band_id(0, B_LENGTH.id), 80.0);
        host.set_plain(&band_id(0, B_SUSTAIN.id), 6.0);
    }

    // Run the real engine over the loop so the history holds genuine records.
    let shared = Arc::new(ShapeShared::new());
    let sample_rate = 48_000.0;
    shared.sample_rate.store(sample_rate);
    let settings = mite_shape::editor::settings(&host);
    let out = Output { delta: v("delta"), clip: v("clip"), ceiling_db: -6.0, gain_db: if v("clip") { 3.0 } else { 0.0 }, ..Output::default() };
    let mut engine = ShapeEngine::new(sample_rate, &settings);
    let [mut l, mut r] = drums::drum_loop(7.0, sample_rate);
    for (a, b) in l.chunks_mut(480).zip(r.chunks_mut(480)) {
        engine.process(&mut [a, b], &settings, &out, &shared.history, Some([&shared.spectrum, &shared.post]));
    }
    shared.latency.store(engine.latency() as f32);

    let mut view = ShapeView::new(shared.clone());
    if v("band-selected") || v("solo") || v("menu") {
        view.select(1);
    }
    if v("merge") {
        view.select(2);
    }
    view.settle(&host);
    for _ in 0..30 {
        view.tick(&host);
        std::thread::sleep(std::time::Duration::from_millis(16));
    }
    let theme_obj = Theme::load(theme, ACCENT);
    render_view(&mut view, &host, &theme_obj, 1.0)?;
    let mods = Modifiers::default();
    let rail_x = |f: f64| 14.0 + 1152.0 * (f / 20.0).ln() / 1000f64.ln();
    match variant.as_str() {
        "detector" => view.open_panel("detector"),
        "stereo" => view.open_panel("stereo"),
        "menu" => view.open_menu(1, &host),
        "presets" => {
            let at = Point::new(300.0, 28.0);
            view.event(&UiEvent::PointerDown { pos: at, button: MouseButton::Left, mods }, &host);
            view.event(&UiEvent::PointerUp { pos: at, button: MouseButton::Left, mods }, &host);
        }
        "split" => {
            // Pulling a crossover in from the high end to 2 kHz.
            let y = 56.0 + 340.0 + 18.0;
            let start = Point::new(rail_x(20_000.0) - 6.0, y);
            view.event(&UiEvent::PointerMove { pos: start, mods }, &host);
            view.event(&UiEvent::PointerDown { pos: start, button: MouseButton::Left, mods }, &host);
            view.event(&UiEvent::PointerMove { pos: Point::new(rail_x(2000.0), y), mods }, &host);
        }
        "merge" => {
            // Dragging the 290 Hz crossover onto the one at 150 Hz.
            let y = 56.0 + 300.0 + 40.0;
            let start = Point::new(rail_x(290.0), y);
            view.event(&UiEvent::PointerMove { pos: start, mods }, &host);
            view.event(&UiEvent::PointerDown { pos: start, button: MouseButton::Left, mods }, &host);
            view.event(&UiEvent::PointerMove { pos: Point::new(rail_x(150.0) + 2.0, y), mods }, &host);
        }
        _ => {}
    }
    if let Ok(p) = std::env::var("SNAPSHOT_HOVER") {
        if let Some((x, y)) = p.split_once(',') {
            let pos = Point::new(x.trim().parse()?, y.trim().parse()?);
            view.event(&UiEvent::PointerMove { pos, mods }, &host);
        }
    }
    view.tick(&host);
    let image = render_view(&mut view, &host, &theme_obj, scale)?;
    let file = std::io::BufWriter::new(std::fs::File::create(&path)?);
    let mut encoder = png::Encoder::new(file, image.width, image.height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.write_header()?.write_image_data(&image.rgba)?;
    println!("wrote {path} ({}×{})", image.width, image.height);
    Ok(())
}
