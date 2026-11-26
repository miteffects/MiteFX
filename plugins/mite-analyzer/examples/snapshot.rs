//! Renders the Mite Analyzer editor to a PNG, with music (a drum loop, bass and chords, a little
//! wider at the top) measured by the real engine first, so every reading is genuine.
//!
//!     cargo run -p mite_analyzer --example snapshot -- out.png [scale] [dark|light|hc|cb]
//!
//! SNAPSHOT_VARIANT (phase-db: the impulse in dB): default, ms (Mid / Side), lr (Left / Right), menu (the target list open),
//! phase (the phase page: the music through a test chain, a 60 Hz high-pass, a 12 kHz low-pass
//! and 128 samples of delay, against the dry music on the sidechain), phase-menu (its reference
//! list open).
//! SNAPSHOT_HOVER=x,y hovers a point.

#[path = "common/music.rs"]
#[allow(dead_code)]
mod music;

use std::sync::Arc;

use mite_analyzer::dsp::AnalyzerEngine;
use mite_analyzer::editor::{memory_host, AnalyzerView, ACCENT};
use mite_analyzer::params::*;
use mite_analyzer::AnalyzerShared;
use mite_ui::kurbo::Point;
use mite_ui::render::render_view;
use mite_ui::{Modifiers, MouseButton, Theme, ThemeKind, UiEvent, View};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let path = args.next().unwrap_or_else(|| "mite-analyzer.png".into());
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
    if v("ms") {
        host.set_plain(VIEW.id, 1.0);
    }
    if v("lr") {
        host.set_plain(VIEW.id, 2.0);
    }
    let shared = Arc::new(AnalyzerShared::new());
    let fs = 48_000.0;
    shared.sample_rate.store(fs);
    let mut engine = AnalyzerEngine::new(fs);
    let [mut l, mut r] = music::music(40.0, fs);
    if variant.starts_with("phase") {
        host.set_plain(PAGE.id, 1.0);
        let dry = [l.clone(), r.clone()];
        for x in [&mut l, &mut r] {
            *x = chain(x, fs);
        }
        for (i, (a, b)) in l.chunks_mut(480).zip(r.chunks_mut(480)).enumerate() {
            let sc: [&[f32]; 2] = [&dry[0][i * 480..i * 480 + a.len()], &dry[1][i * 480..i * 480 + a.len()]];
            engine.process_with(&[a, b], Some(&sc), &shared.tel);
        }
    } else {
        for (a, b) in l.chunks_mut(480).zip(r.chunks_mut(480)) {
            engine.process(&[a, b], &shared.tel);
        }
    }
    let mut view = AnalyzerView::new(shared.clone());
    view.tick(&host);
    view.settle(&host);
    let theme_obj = Theme::load(theme, ACCENT);
    render_view(&mut view, &host, &theme_obj, 1.0)?;
    let mods = Modifiers::default();
    if v("menu") || v("phase-menu") {
        let pos = view.button(if v("menu") { "target" } else { "source" }).expect("button").center();
        view.event(&UiEvent::PointerMove { pos, mods }, &host);
        view.event(&UiEvent::PointerDown { pos, button: MouseButton::Left, mods }, &host);
        view.event(&UiEvent::PointerUp { pos, button: MouseButton::Left, mods }, &host);
    }
    if v("phase-db") {
        // The impulse in dB.
        let pos = mite_analyzer::editor::phase::IR_DB_BUTTON.center();
        view.event(&UiEvent::PointerDown { pos, button: MouseButton::Left, mods }, &host);
        view.event(&UiEvent::PointerUp { pos, button: MouseButton::Left, mods }, &host);
    }
    if let Some(pos) = std::env::var("SNAPSHOT_HOVER").ok().and_then(|p| {
        let (x, y) = p.split_once(',')?;
        Some(Point::new(x.trim().parse().ok()?, y.trim().parse().ok()?))
    }) {
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

/// The phase variant's test chain: RBJ high-pass 60 Hz and low-pass 12 kHz (Q 0.707), then 128
/// samples of delay.
fn chain(x: &[f32], fs: f32) -> Vec<f32> {
    let fs = f64::from(fs);
    let biquad = |x: Vec<f64>, f0: f64, high: bool| -> Vec<f64> {
        let w = std::f64::consts::TAU * f0 / fs;
        let (c, alpha) = (w.cos(), w.sin() / (2.0 * std::f64::consts::FRAC_1_SQRT_2));
        let a0 = 1.0 + alpha;
        let (b0, b1) = if high { ((1.0 + c) / 2.0, -(1.0 + c)) } else { ((1.0 - c) / 2.0, 1.0 - c) };
        let (b0, b1, b2, a1, a2) = (b0 / a0, b1 / a0, b0 / a0, -2.0 * c / a0, (1.0 - alpha) / a0);
        let (mut s1, mut s2) = (0.0, 0.0);
        x.into_iter()
            .map(|v| {
                let y = b0 * v + s1;
                s1 = b1 * v - a1 * y + s2;
                s2 = b2 * v - a2 * y;
                y
            })
            .collect()
    };
    let y = biquad(biquad(x.iter().map(|&v| f64::from(v)).collect(), 60.0, true), 12_000.0, false);
    (0..y.len()).map(|i| if i >= 128 { y[i - 128] as f32 } else { 0.0 }).collect()
}
