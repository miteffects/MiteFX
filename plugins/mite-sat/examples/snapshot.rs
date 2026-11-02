//! Renders the Mite Sat editor to a PNG in one of its states, with music run through the real
//! engine first, so the spectrum, meters and the curve's lit stretch are genuine.
//!
//!     cargo run -p mite_sat --example snapshot -- out.png [scale] [dark|light|hc|cb]
//!
//! SNAPSHOT_VARIANT picks the state: default, clean, fold, hard, rectify, tape, transformer,
//! bypass, delta, dynamics,
//! oversampling, presets, ghost; several bands: bands (three), six, solo, mute, menu, pull (a
//! crossover being pulled in), merge (one dragged onto its neighbour), hollow (a band whose
//! harmonics all lie above Nyquist), stereo (a Mid and an M/S band). SNAPSHOT_HOVER=x,y moves
//! the pointer there.

#[path = "common/music.rs"]
mod music;

use std::sync::Arc;

use mite_dsp::sat::Model;
use mite_sat::dsp::{Output, SatEngine};
use mite_sat::editor::{memory_host, oversampling, SatView, ACCENT};
use mite_sat::params::*;
use mite_sat::SatShared;
use mite_ui::kurbo::Point;
use mite_ui::render::render_view;
use mite_ui::{Modifiers, MouseButton, Theme, ThemeKind, UiEvent, View};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let path = args.next().unwrap_or_else(|| "mite-sat.png".into());
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
    let (model, drive, bias, tone) = match variant.as_str() {
        "clean" => (Model::Tube, 0.0, 0.0, 0.0),
        "fold" => (Model::Fold, 16.0, 0.0, -2.0),
        "hard" => (Model::Hard, 22.0, 0.0, 0.0),
        "rectify" => (Model::Rectify, 14.0, 0.0, 0.0),
        "tape" => (Model::Tape, 16.0, 0.0, 0.0),
        "transformer" => (Model::Transformer, 20.0, 0.0, 0.0),
        _ => (Model::Tube, 12.0, 15.0, 0.0),
    };
    host.set_plain(&band_id(0, B_MODEL.id), model.index() as f32);
    host.set_plain(&band_id(0, B_DRIVE.id), drive);
    host.set_plain(&band_id(0, B_BIAS.id), bias);
    host.set_plain(&band_id(0, B_TONE.id), tone);
    if v("dynamics") || v("bands") {
        // Band 1 compresses after its curve.
        host.set_plain(&band_id(0, B_DYN.id), -60.0);
        host.set_plain(&band_id(0, B_DYN_THR.id), -30.0);
    }
    // Several bands, each with its own model.
    let bands: &[(f32, Model, f32, f32)] = match variant.as_str() {
        "bands" | "solo" | "mute" | "menu" | "pull" | "merge" | "stereo" => &[(180.0, Model::Soft, 9.0, 0.0), (2400.0, Model::Fold, 15.0, 0.0)],
        "six" | "hollow" => &[(90.0, Model::Diode, 18.0, 0.0), (350.0, Model::Soft, 10.0, 0.0), (1200.0, Model::Hard, 20.0, 0.0), (4000.0, Model::Fold, 14.0, 0.0), (11_000.0, Model::Rectify, 12.0, 0.0)],
        _ => &[],
    };
    for (k, &(f, m, d, b)) in bands.iter().enumerate() {
        host.set_plain(&cross_id(k, X_ON.id), 1.0);
        host.set_plain(&cross_id(k, X_FREQ.id), f);
        host.set_plain(&band_id(k + 1, B_MODEL.id), m.index() as f32);
        host.set_plain(&band_id(k + 1, B_DRIVE.id), d);
        host.set_plain(&band_id(k + 1, B_BIAS.id), b);
    }
    if v("solo") {
        host.set_plain(&band_id(2, B_SOLO.id), 1.0);
    }
    if v("stereo") {
        host.set_plain(&band_id(1, B_STEREO.id), 3.0);
        host.set_plain(&band_id(2, B_STEREO.id), 1.0);
    }
    if v("mute") {
        host.set_plain(&band_id(1, B_MUTE.id), 1.0);
    }
    if v("bypass") {
        host.set_plain(&band_id(0, B_BYPASS.id), 1.0);
    }
    if v("delta") {
        host.set_plain(DELTA.id, 1.0);
    }

    // Run the real engine over the music so the telemetry is genuine.
    let shared = Arc::new(SatShared::new());
    let sample_rate = 48_000.0;
    shared.sample_rate.store(sample_rate);
    let settings = mite_sat::editor::settings(&host);
    let factor = oversampling(&host);
    let out = Output { delta: v("delta"), ..Output::default() };
    let mut engine = SatEngine::new(sample_rate, factor, &settings);
    let [mut l, mut r] = music::music(4.0, sample_rate);
    for (a, b) in l.chunks_mut(480).zip(r.chunks_mut(480)) {
        engine.process(&mut [a, b], &settings, factor, &out, &shared.history, Some([&shared.spectrum, &shared.post]));
    }
    shared.latency.store(engine.latency() as f32);

    let mut view = SatView::new(shared.clone());
    if v("six") {
        view.select(3);
    }
    if v("hollow") {
        view.select(5);
    }
    for _ in 0..30 {
        view.tick(&host);
        std::thread::sleep(std::time::Duration::from_millis(16));
    }
    let theme_obj = Theme::load(theme, ACCENT);
    render_view(&mut view, &host, &theme_obj, 1.0)?;
    let mods = Modifiers::default();
    match variant.as_str() {
        "oversampling" => view.open_oversampling(&host),
        "presets" => {
            let at = Point::new(300.0, 28.0);
            view.event(&UiEvent::PointerDown { pos: at, button: MouseButton::Left, mods }, &host);
            view.event(&UiEvent::PointerUp { pos: at, button: MouseButton::Left, mods }, &host);
        }
        "menu" => view.open_menu(2, &host),
        "pull" | "merge" => {
            let y = view.rail_y();
            let (from, to) = if v("pull") { (Point::new(view.rail_x(20_000.0) - 6.0, y), Point::new(view.rail_x(700.0), y)) } else { (Point::new(view.rail_x(2400.0), y), Point::new(view.rail_x(180.0) + 3.0, y)) };
            view.event(&UiEvent::PointerMove { pos: from, mods }, &host);
            view.event(&UiEvent::PointerDown { pos: from, button: MouseButton::Left, mods }, &host);
            for k in 1..=8 {
                view.event(&UiEvent::PointerMove { pos: from.lerp(to, f64::from(k) / 8.0), mods }, &host);
            }
        }
        "ghost" => {
            let at = view.model_icon(&host, Model::Fold).expect("Fold is offered").center();
            view.event(&UiEvent::PointerMove { pos: at, mods }, &host);
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
