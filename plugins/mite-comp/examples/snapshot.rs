//! Renders the Mite Comp editor to a PNG in one of the design's states, with a drum-like signal
//! run through the real engine first, so the history, meters and live dot are genuine.
//!
//!     cargo run -p mite_comp --example snapshot -- out.png [scale] [dark|light|hc|cb]
//!
//! SNAPSHOT_VARIANT picks the state, as the design's `variant` prop: default, stages,
//! stage-selected, create, character, custom, smart, sidechain, listen, stereo, lookahead, delta,
//! empty, presets, menu.

use std::sync::Arc;

use mite_comp::dsp::{CompEngine, Output};
use mite_comp::editor::{memory_host, CompView, ACCENT};
use mite_comp::params::*;
use mite_comp::CompShared;
use mite_dsp::dynamics::{CompSettings, Detector, Quadrant, Smart, Stage, STAGES, STYLES};
use mite_ui::kurbo::Point;
use mite_ui::render::render_view;
use mite_ui::{Modifiers, MouseButton, Theme, ThemeKind, UiEvent, View};

const LEVELLER: Stage = Stage { on: true, quadrant: Quadrant::UpCompress, threshold_db: -32.0, ratio: 2.0, knee_db: 6.0, range_db: 12.0, attack_ms: 30.0, release_ms: 800.0, detector: Detector::Rms };
const GATE: Stage = Stage { on: true, quadrant: Quadrant::Expand, threshold_db: -48.0, ratio: 4.0, knee_db: 4.0, range_db: 60.0, attack_ms: 0.5, release_ms: 60.0, detector: Detector::Peak };
const COMP: Stage = Stage { on: true, quadrant: Quadrant::Compress, threshold_db: -18.0, ratio: 4.0, knee_db: 6.0, range_db: 60.0, attack_ms: 10.0, release_ms: 120.0, detector: Detector::Rms };
const LIMIT: Stage = Stage { on: true, quadrant: Quadrant::Compress, threshold_db: -6.0, ratio: 20.0, knee_db: 2.0, range_db: 60.0, attack_ms: 0.3, release_ms: 50.0, detector: Detector::Peak };

fn set_stage(host: &mite_ui::MemoryParamHost, slot: usize, s: &Stage) {
    let id = |p: &str| stage_id(slot, p);
    host.set_plain(&id(S_ON.id), if s.on { 1.0 } else { 0.0 });
    host.set_plain(&id(S_QUAD.id), s.quadrant as usize as f32);
    host.set_plain(&id(S_THRESH.id), s.threshold_db);
    host.set_plain(&id(S_RATIO.id), s.ratio);
    host.set_plain(&id(S_KNEE.id), s.knee_db);
    host.set_plain(&id(S_ATTACK.id), s.attack_ms);
    host.set_plain(&id(S_RELEASE.id), s.release_ms);
    host.set_plain(&id(S_RANGE.id), s.range_db);
    host.set_plain(&id(S_DET.id), s.detector as usize as f32);
}

/// The design's test signal: a drum-like level envelope with a quiet passage, as noise.
fn signal(seconds: f32, sample_rate: f32) -> Vec<f32> {
    let n = (seconds * sample_rate) as usize;
    let slice = (sample_rate * 6.0 / 195.0) as usize;
    let mut seed = 0x2545_F491_u32;
    (0..n)
        .map(|i| {
            let k = (i / slice) as i64;
            let beat = k.rem_euclid(32) as f32;
            let kf = k as f32;
            let mut v = -27.0 + 5.0 * (kf * 0.13).sin() + 3.0 * (kf * 0.41 + 1.0).sin() + 1.5 * (kf * 1.7).sin();
            v += 17.0 * (-beat / 2.2).exp() + if beat >= 16.0 { 12.0 * (-(beat - 16.0) / 3.0).exp() } else { 0.0 };
            let ph = k.rem_euclid(150);
            if ph > 118 && ph < 138 {
                v = -52.0 + 2.0 * (kf * 0.9).sin();
            }
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            let white = (seed as f32 / u32::MAX as f32) * 2.0 - 1.0;
            white * 1.6 * mite_dsp::db_to_gain(v.clamp(-60.0, -1.0))
        })
        .collect()
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let path = args.next().unwrap_or_else(|| "mite-comp.png".into());
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
    let mut stages = [Stage::default(); STAGES];
    if v("stages") || v("stage-selected") {
        stages[..4].copy_from_slice(&[LEVELLER, GATE, COMP, LIMIT]);
    } else if !v("empty") {
        stages[0] = COMP;
    }
    for (slot, s) in stages.iter().enumerate() {
        set_stage(&host, slot, s);
    }
    let mut settings = CompSettings { stages, ..CompSettings::default() };
    let character = if v("character") { 3 } else if v("stages") || v("stage-selected") { 4 } else { 0 };
    host.set_plain(CHARACTER.id, character as f32);
    settings.style = STYLES[character];
    if v("smart") {
        host.set_plain(SMART.id, 2.0);
        settings.smart = Smart::Vocal;
    }
    if v("stereo") {
        host.set_plain(STEREO.id, 1.0);
        host.set_plain(LINK.id, 50.0);
        settings.mid_side = true;
        settings.link = 0.5;
    }
    if v("lookahead") {
        host.set_plain(LOOKAHEAD.id, 5.0);
        host.set_plain(HOLD.id, 20.0);
        host.set_plain(MAKEUP.id, 4.2);
        settings.lookahead_ms = 5.0;
        settings.hold_ms = 20.0;
    }
    if v("sidechain") || v("listen") {
        host.set_plain(SC_SOURCE.id, 1.0);
        for (b, (kind, freq, gain)) in [(3.0, 120.0, 0.0), (0.0, 300.0, -4.0), (0.0, 4500.0, 6.0)].into_iter().enumerate() {
            host.set_plain(&key_id(b, K_ON.id), 1.0);
            host.set_plain(&key_id(b, K_TYPE.id), kind);
            host.set_plain(&key_id(b, K_FREQ.id), freq);
            host.set_plain(&key_id(b, K_GAIN.id), gain);
        }
    }
    if v("listen") {
        host.set_plain(LISTEN.id, 1.0);
        settings.listen = true;
    }
    if v("delta") {
        host.set_plain(DELTA.id, 1.0);
    }
    if !v("empty") {
        host.set_plain(AUTO_MAKEUP.id, 1.0);
        settings.auto_makeup = true;
    }

    // Run the real engine over the signal so the history holds genuine records.
    let shared = Arc::new(CompShared::new());
    let sample_rate = 48_000.0;
    shared.sample_rate.store(sample_rate);
    let mut engine = CompEngine::new(sample_rate, &settings);
    let out = Output { delta: v("delta"), ..Output::default() };
    let input = signal(7.0, sample_rate);
    let (mut l, mut r) = (input.clone(), input.iter().map(|x| x * 0.9).collect::<Vec<_>>());
    for (a, b) in l.chunks_mut(480).zip(r.chunks_mut(480)) {
        engine.process(&mut [a, b], None, &settings, &out, &shared.history);
    }
    shared.latency.store(engine.latency() as f32);

    let mut view = CompView::new(shared.clone());
    view.tick(&host);
    let theme_obj = Theme::load(theme, ACCENT);
    render_view(&mut view, &host, &theme_obj, 1.0)?;
    let mods = Modifiers::default();
    match variant.as_str() {
        "stage-selected" => view.select(Some(2)),
        "custom" => {
            host.set_plain(CHARACTER.id, 6.0);
            view.open_panel("custom");
        }
        "sidechain" | "listen" => view.open_panel("sidechain"),
        "lookahead" => view.open_panel("detector"),
        "stereo" => view.open_panel("stereo"),
        "character" => view.open_menu("character", &host),
        "menu" => view.open_menu("smart", &host),
        "presets" => {
            let at = Point::new(300.0, 28.0);
            view.event(&UiEvent::PointerDown { pos: at, button: MouseButton::Left, mods }, &host);
            view.event(&UiEvent::PointerUp { pos: at, button: MouseButton::Left, mods }, &host);
        }
        "create" => {
            // Pulling the line below itself near the top of the curve: stage 2, compress.
            let start = Point::new(796.0 + 52.0 / 60.0 * 384.0, 56.0 + 16.0 + 8.0 / 60.0 * 384.0);
            view.event(&UiEvent::PointerMove { pos: start, mods }, &host);
            view.event(&UiEvent::PointerDown { pos: start, button: MouseButton::Left, mods }, &host);
            view.event(&UiEvent::PointerMove { pos: start + (14.0, 52.0), mods }, &host);
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
