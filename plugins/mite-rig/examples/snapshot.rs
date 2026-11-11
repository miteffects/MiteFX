//! Renders the Mite Rig editor to a PNG: the example board (gate, NAM's example A2 capture, a
//! cabinet; for "split" also a 3-lane split with a boost, a tone and a dry lane) loaded through
//! the real loader, a guitar-like sound through the real engine first.
//!
//!     cargo run -p mite_rig --example snapshot -- out.png [scale] [dark|light|hc|cb]
//!
//! SNAPSHOT_VARIANT: default, split, menu, browser. SNAPSHOT_CLICK=x,y clicks a point first.

#[path = "common/rig.rs"]
mod rig;

use std::sync::Arc;

use mite_rig::blocks::Kind;
use mite_rig::board::{Item, Place};
use mite_rig::editor::{memory_host, settings, RigView, Selection, ACCENT};
use mite_rig::engine::{RigEngine, RigShared};
use mite_rig::loader::LoaderCore;
use mite_rig::params::RigParams;
use mite_rig::RigUi;
use mite_ui::kurbo::Point;
use mite_ui::render::render_view;
use mite_ui::{Modifiers, MouseButton, Theme, ThemeKind, UiEvent, View};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let path = args.next().unwrap_or_else(|| "mite-rig.png".into());
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
    let fs = 48_000.0;
    let params = Arc::new(RigParams::default());
    let board = params.board.clone();
    rig::fill(&board);
    if v("split") {
        let split = board.insert_split(3, 3);
        let at = board.items().iter().position(|i| matches!(i, Item::Split(s) if s.id == split)).unwrap();
        let boost = board.insert(Kind::Boost, Place::Lane { split: at, lane: 0, index: 0 }).unwrap();
        board.insert(Kind::Tone, Place::Lane { split: at, lane: 1, index: 0 });
        board.insert(Kind::Volume, Place::Lane { split: at, lane: 1, index: 1 });
        board.block(boost).unwrap().live.set(0, 9.0);
    }
    let shared = Arc::new(RigShared::new());
    let core = LoaderCore::new(board.clone(), shared.clone(), params.clone());
    core.set_rate(fs);
    core.build_files();
    let s = settings(&host);
    let mut engine = RigEngine::new(fs, &s);
    if let Some(g) = core.build_graph(true) {
        drop(engine.install_now(g));
    }
    engine.prime();
    let x = rig::guitar(2.0, fs);
    let (mut l, mut r) = (x.clone(), x);
    for (a, b) in l.chunks_mut(256).zip(r.chunks_mut(256)) {
        engine.process(&mut [a, b], &s, &shared);
    }
    let ui = Arc::new(RigUi::with_stores(params.snapshots.clone(), params.preset.clone(), board.clone(), shared.clone()));
    ui.sample_rate.store(fs);
    ui.latency.store(engine.latency() as f32);
    let ui_t3k = ui.tone3000.clone();
    let mut view = RigView::new(ui).with_preset_dir(std::env::temp_dir().join("mite-rig-snapshot-presets"));
    view.settle();
    let mods = Modifiers::default();
    let click = |view: &mut RigView, pos: Point| {
        view.event(&UiEvent::PointerMove { pos, mods }, &host);
        view.event(&UiEvent::PointerDown { pos, button: MouseButton::Left, mods }, &host);
        view.event(&UiEvent::PointerUp { pos, button: MouseButton::Left, mods }, &host);
    };
    if v("split") {
        let fork = view.layout().forks[0].fork;
        click(&mut view, fork);
    }
    if v("menu") {
        let gap = view.layout().gaps[1].at;
        click(&mut view, gap);
    }
    if v("browser") {
        if let Some(Selection::Block(id)) = view.selection() {
            view.open_browser(id);
            let t0 = std::time::Instant::now();
            while t0.elapsed() < std::time::Duration::from_secs(20) {
                view.tick(&host);
                let tv = ui_t3k.view();
                let covers = tv.feed.tones.iter().take(8).filter_map(|t| t.image.clone()).all(|u| ui_t3k.image(&u).is_some());
                if !tv.feed.tones.is_empty() && covers {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
        }
    }
    if let Ok(xy) = std::env::var("SNAPSHOT_CLICK") {
        let mut it = xy.split(',').filter_map(|s| s.trim().parse::<f64>().ok());
        if let (Some(x), Some(y)) = (it.next(), it.next()) {
            click(&mut view, Point::new(x, y));
        }
    }
    view.tick(&host);
    let theme = Theme::load(theme, ACCENT);
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
