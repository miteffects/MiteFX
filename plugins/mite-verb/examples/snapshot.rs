//! Renders the Mite Verb editor to a PNG with music (then silence, so the tail falls) run through
//! the real engine first, so the history, spectra and meters are genuine.
//!
//!     cargo run -p mite_verb --example snapshot -- out.png [scale] [dark|light|hc|cb]
//!
//! SNAPSHOT_VARIANT: default, hall (a large dark hall), bands (two decay bells), freeze, duck.
//! SNAPSHOT_HOVER=x,y hovers a point.

#[path = "common/music.rs"]
mod music;

use std::sync::Arc;

use mite_ui::kurbo::Point;
use mite_ui::render::render_view;
use mite_ui::{Modifiers, Theme, ThemeKind, UiEvent, View};
use mite_verb::dsp::VerbEngine;
use mite_verb::editor::{memory_host, output, settings, VerbView, ACCENT};
use mite_verb::params::*;
use mite_verb::VerbShared;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let path = args.next().unwrap_or_else(|| "mite-verb.png".into());
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
    if v("hall") {
        host.set_plain(SIZE.id, 85.0);
        host.set_plain(DECAY.id, 4.0);
        host.set_plain(LO_MULT.id, 1.5);
        host.set_plain(HI_MULT.id, 0.35);
        host.set_plain(HI_FREQ.id, 3500.0);
        host.set_plain(HI_CUT.id, 9000.0);
        host.set_plain(LO_CUT.id, 60.0);
    }
    if v("bands") {
        host.set_plain("d2_on", 1.0);
        host.set_plain("d2_freq", 700.0);
        host.set_plain("d2_mult", 0.6);
        host.set_plain("d3_on", 1.0);
        host.set_plain("d3_freq", 3000.0);
        host.set_plain("d3_mult", 1.6);
        host.set_plain("d3_q", 2.0);
    }
    if v("duck") {
        host.set_plain(DUCK.id, 12.0);
        host.set_plain(DUCK_THRESH.id, -28.0);
    }
    let shared = Arc::new(VerbShared::new());
    let sample_rate = 48_000.0;
    shared.sample_rate.store(sample_rate);
    let (s, out) = (settings(&host), output(&host));
    let mut engine = VerbEngine::new(sample_rate, &s, &out);
    let [mut l, mut r] = music::music(6.0, sample_rate);
    let gap = (sample_rate * 5.0) as usize;
    for x in l[gap..].iter_mut().chain(r[gap..].iter_mut()) {
        *x = 0.0;
    }
    let frozen = settings(&host);
    let frozen = mite_dsp::reverb::ReverbSettings { freeze: true, ..frozen };
    for (k, (a, b)) in l.chunks_mut(480).zip(r.chunks_mut(480)).enumerate() {
        let s = if v("freeze") && k * 480 > gap - 24_000 { &frozen } else { &s };
        engine.process(&mut [a, b], s, &out, &shared.history, Some([&shared.spectrum, &shared.wet]));
    }
    if v("freeze") {
        host.set_plain(FREEZE.id, 1.0);
    }
    let mut view = VerbView::new(shared.clone());
    view.tick(&host);
    view.settle_spectrum();
    let theme_obj = Theme::load(theme, ACCENT);
    render_view(&mut view, &host, &theme_obj, 1.0)?;
    let mods = Modifiers::default();
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
