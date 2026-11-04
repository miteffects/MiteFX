//! Renders the Mite Delay editor to a PNG with music (then silence) run through
//! the real engine first, so the history, spectra and meters are genuine.
//!
//!     cargo run -p mite_delay --example snapshot -- out.png [scale] [dark|light|hc|cb]
//!
//! SNAPSHOT_VARIANT: default, taps (four taps and a loop EQ), tape (the same through Tape), duck.
//! SNAPSHOT_HOVER=x,y hovers a point; SNAPSHOT_CLICK=x,y clicks one, SNAPSHOT_RCLICK=x,y
//! right-clicks one (a menu).

#[path = "common/music.rs"]
mod music;

use std::sync::Arc;

use mite_ui::kurbo::Point;
use mite_ui::render::render_view;
use mite_ui::{Modifiers, Theme, ThemeKind, UiEvent, View};
use mite_delay::dsp::DelayEngine;
use mite_delay::editor::{memory_host, output, settings, DelayView, ACCENT};
use mite_delay::params::*;
use mite_delay::DelayShared;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let path = args.next().unwrap_or_else(|| "mite-delay.png".into());
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
    if v("taps") || v("tape") {
        // Rhythm taps: four taps, panned, one ping-pong; a loop EQ.
        for (id, x) in [
            ("t1_note", 5.0), ("t1_feedback", 0.0), ("t1_level", -4.0), ("t1_pan", -70.0),
            ("t2_on", 1.0), ("t2_note", 10.0), ("t2_feedback", 0.0), ("t2_level", -6.0), ("t2_pan", 70.0),
            ("t3_on", 1.0), ("t3_note", 11.0), ("t3_feedback", 45.0), ("t3_level", -3.0),
            ("t4_on", 1.0), ("t4_note", 16.0), ("t4_feedback", 50.0), ("t4_level", -9.0), ("t4_pingpong", 1.0),
            ("e1_on", 1.0), ("e1_type", 3.0), ("e1_freq", 200.0), ("e1_slope", 1.0),
            ("e2_on", 1.0), ("e2_type", 0.0), ("e2_freq", 2500.0), ("e2_gain", 4.0), ("e2_q", 1.4),
        ] {
            host.set_plain(id, x);
        }
    }
    if v("tape") {
        host.set_plain(MODE.id, 1.0);
        host.set_plain(DRIVE.id, 40.0);
        host.set_plain(MOD_DEPTH.id, 30.0);
    }
    if v("duck") {
        host.set_plain(DUCK.id, 12.0);
        host.set_plain(DUCK_THRESH.id, -28.0);
    }
    let shared = Arc::new(DelayShared::new());
    let sample_rate = 48_000.0;
    shared.sample_rate.store(sample_rate);
    let (s, out) = (settings(&host, mite_delay::DEFAULT_TEMPO), output(&host));
    let mut engine = DelayEngine::new(sample_rate, &s, &out);
    let [mut l, mut r] = music::music(6.0, sample_rate);
    let gap = (sample_rate * 5.5) as usize;
    for x in l[gap..].iter_mut().chain(r[gap..].iter_mut()) {
        *x = 0.0;
    }
    for (a, b) in l.chunks_mut(480).zip(r.chunks_mut(480)) {
        engine.process(&mut [a, b], &s, &out, &shared.history, Some([&shared.spectrum, &shared.wet]));
    }
    let mut view = DelayView::new(shared.clone());
    view.tick(&host);
    view.settle_spectrum();
    let theme_obj = Theme::load(theme, ACCENT);
    render_view(&mut view, &host, &theme_obj, 1.0)?;
    let mods = Modifiers::default();
    if let Ok(p) = std::env::var("SNAPSHOT_CLICK") {
        if let Some((x, y)) = p.split_once(',') {
            let pos = Point::new(x.trim().parse()?, y.trim().parse()?);
            view.event(&UiEvent::PointerMove { pos, mods }, &host);
            view.event(&UiEvent::PointerDown { pos, button: mite_ui::MouseButton::Left, mods }, &host);
            view.event(&UiEvent::PointerUp { pos, button: mite_ui::MouseButton::Left, mods }, &host);
        }
    }
    if let Ok(p) = std::env::var("SNAPSHOT_RCLICK") {
        if let Some((x, y)) = p.split_once(',') {
            let pos = Point::new(x.trim().parse()?, y.trim().parse()?);
            view.event(&UiEvent::PointerMove { pos, mods }, &host);
            view.event(&UiEvent::PointerDown { pos, button: mite_ui::MouseButton::Right, mods }, &host);
        }
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
