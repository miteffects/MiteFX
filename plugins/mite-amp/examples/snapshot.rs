//! Renders the Mite Amp editor to a PNG, with the example rig loaded through the real loader and
//! a guitar-like sound played through the real engine first, so every curve and meter is genuine.
//!
//!     cargo run -p mite_amp --example snapshot -- out.png [scale] [dark|light|hc|cb]
//!
//! SNAPSHOT_VARIANT: default, empty (nothing loaded), tone (the tone stack moved), browser (the
//! TONE3000 panel), failed (a broken file in the pedal slot). SNAPSHOT_HOVER=x,y hovers a point.

#[path = "common/rig.rs"]
mod rig;

use std::sync::Arc;

use mite_amp::dsp::{AmpEngine, AmpShared};
use mite_amp::editor::{memory_host, settings, AmpView, ACCENT};
use mite_amp::params::*;
use mite_amp::rig::{Slot, SlotFile};
use mite_amp::AmpUi;
use mite_ui::kurbo::Point;
use mite_ui::render::render_view;
use mite_ui::{Modifiers, Theme, ThemeKind, UiEvent, View};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let path = args.next().unwrap_or_else(|| "mite-amp.png".into());
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
    if v("tone") {
        host.set_plain(BASS.id, 4.0);
        host.set_plain(MIDDLE.id, -6.0);
        host.set_plain(TREBLE.id, 3.0);
        host.set_plain(LOW_CUT.id, 90.0);
        host.set_plain(HIGH_CUT.id, 7000.0);
    }
    if std::env::var("SNAPSHOT_NOCAB").is_ok() {
        host.set_plain(CAB.id, 0.0);
    }
    if std::env::var("SNAPSHOT_NOPEDAL").is_ok() {
        host.set_plain(PEDAL.id, 0.0);
    }
    let fs = 48_000.0;
    let params = Arc::new(AmpParams::default());
    let shared = Arc::new(AmpShared::new());
    if !v("empty") {
        let _core = rig::load(&params, &shared, fs, false);
    }
    if v("failed") {
        let bad = std::env::temp_dir().join("mite-amp-broken.nam");
        std::fs::write(&bad, "{ not a model")?;
        params.rig.set_file(Slot::Pedal, SlotFile { path: Some(bad), source: None });
        let core = mite_amp::loader::LoaderCore::new(params.rig.clone(), shared.clone(), params.clone());
        core.set_rate(fs);
        core.step();
    }
    let ui = Arc::new(AmpUi::with_stores(params.snapshots.clone(), params.preset.clone(), params.rig.clone(), shared.clone()));
    ui.sample_rate.store(fs);
    let s = settings(&host);
    let mut engine = AmpEngine::new(fs, &s);
    let x = rig::guitar(3.0, fs);
    let (mut l, mut r) = (x.clone(), x);
    for (a, b) in l.chunks_mut(256).zip(r.chunks_mut(256)) {
        engine.process(&mut [a, b], &s, &shared);
    }
    ui.latency.store(engine.latency() as f32);
    if std::env::var("SNAPSHOT_LEVELS").is_ok() {
        let peak = |v: &[f32]| 20.0 * v.iter().fold(0.0f32, |m, x| m.max(x.abs())).log10();
        let rms = |v: &[f32]| 10.0 * (v.iter().map(|x| x * x).sum::<f32>() / v.len() as f32).log10();
        println!("in peak {:.1} rms {:.1} | out peak {:.1} rms {:.1}", peak(&rig::guitar(3.0, fs)), rms(&rig::guitar(3.0, fs)), peak(&l), rms(&l));
    }
    let ui_t3k = ui.tone3000.clone();
    let mut view = AmpView::new(ui).with_preset_dir(std::env::temp_dir().join("mite-amp-snapshot-presets"));
    view.settle();
    if v("browser") || v("detail") {
        view.open_browser(Slot::Amp);
        // Live TONE3000 (the signed-in session on this computer): wait for the cards, their
        // covers and, for "detail", the first tone's files.
        let t0 = std::time::Instant::now();
        let mut opened = false;
        while t0.elapsed() < std::time::Duration::from_secs(25) {
            view.tick(&host);
            let tv = ui_t3k.view();
            if v("detail") && !opened && !tv.feed.tones.is_empty() {
                ui_t3k.open_tone(tv.feed.tones[0].clone());
                opened = true;
            }
            let covers = tv.feed.tones.iter().take(8).filter_map(|t| t.image.clone()).all(|u| ui_t3k.image(&u).is_some());
            if !tv.feed.tones.is_empty() && covers && (!v("detail") || tv.models.is_some()) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
    }
    view.tick(&host);
    if let Ok(xy) = std::env::var("SNAPSHOT_CLICK") {
        let mut it = xy.split(',').filter_map(|s| s.trim().parse::<f64>().ok());
        if let (Some(x), Some(y)) = (it.next(), it.next()) {
            let pos = Point::new(x, y);
            view.event(&UiEvent::PointerMove { pos, mods: Modifiers::default() }, &host);
            view.event(&UiEvent::PointerDown { pos, button: mite_ui::MouseButton::Left, mods: Modifiers::default() }, &host);
            view.event(&UiEvent::PointerUp { pos, button: mite_ui::MouseButton::Left, mods: Modifiers::default() }, &host);
        }
    }
    if let Ok(xy) = std::env::var("SNAPSHOT_HOVER") {
        let mut it = xy.split(',').filter_map(|s| s.trim().parse::<f64>().ok());
        if let (Some(x), Some(y)) = (it.next(), it.next()) {
            view.event(&UiEvent::PointerMove { pos: Point::new(x, y), mods: Modifiers::default() }, &host);
        }
    }
    let theme = Theme::load(theme, ACCENT);
    // A first frame lays out the text; the second is the one kept.
    render_view(&mut view, &host, &theme, 1.0)?;
    let image = render_view(&mut view, &host, &theme, scale)?;
    let file = std::io::BufWriter::new(std::fs::File::create(&path)?);
    let mut encoder = png::Encoder::new(file, image.width, image.height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.write_header()?.write_image_data(&image.rgba)?;
    println!("wrote {path} ({}×{})", image.width, image.height);
    Ok(())
}
