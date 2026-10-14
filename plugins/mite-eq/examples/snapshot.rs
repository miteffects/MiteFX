//! Renders the Mite EQ editor to a PNG, set up like the mockup (4 bands, band 3 selected), with
//! pink noise run through the real engine so the analyzer shows genuine pre/post spectra.
//!
//!     cargo run -p mite_eq --example snapshot -- out.png [scale] [dark|light|hc|cb]

use std::sync::Arc;

use mite_core::{ParamDisplay, ParamScale, ParamSpec};
use mite_dsp::eq::{BandSettings, BandType};
use mite_eq::dsp::EqEngine;
use mite_eq::editor::EqView;
use mite_eq::params::{band_id, BANDS, FREQ, GAIN, ON, OUT_GAIN, Q, SLOPE, STEREO, TYPE};
use mite_eq::EqShared;
use mite_ui::kurbo::Point;
use mite_ui::render::render_view;
use mite_ui::{MemoryParamHost, Modifiers, MouseButton, ParamHost, Theme, ThemeKind, UiEvent, View};

/// Stand-in spec for choice/bool parameters in the in-memory host: 0 … n−1, linear.
fn index_spec(id: &'static str, count: usize, default: usize) -> ParamSpec {
    ParamSpec {
        id,
        name: id,
        unit: "",
        min: 0.0,
        max: (count - 1) as f32,
        default: default as f32,
        scale: ParamScale::Linear,
        smoothing_ms: 0.0,
        decimals: 0,
        wheel_step: 1.0,
        wheel_step_fine: 1.0,
        display: ParamDisplay::Number,
    }
}

fn leak(s: String) -> &'static str {
    Box::leak(s.into_boxed_str())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let path = args.next().unwrap_or_else(|| "mite-eq.png".into());
    let scale: f64 = args.next().map_or(Ok(1.5), |s| s.parse())?;
    let theme = match args.next().as_deref() {
        Some("light") => ThemeKind::Light,
        Some("hc") => ThemeKind::HighContrast,
        Some("cb") => ThemeKind::ColorBlind,
        _ => ThemeKind::Dark,
    };

    let mut host = MemoryParamHost::new().with(OUT_GAIN);
    for b in 0..BANDS {
        host = host
            .with_id(leak(band_id(b, ON.id)), index_spec("on", 2, 0))
            .with_id(leak(band_id(b, TYPE.id)), index_spec("type", TYPE.choices.len(), 0))
            .with_id(leak(band_id(b, FREQ.id)), FREQ)
            .with_id(leak(band_id(b, GAIN.id)), GAIN)
            .with_id(leak(band_id(b, Q.id)), Q)
            .with_id(leak(band_id(b, SLOPE.id)), index_spec("slope", SLOPE.choices.len(), SLOPE.default))
            .with_id(leak(band_id(b, STEREO.id)), index_spec("stereo", STEREO.choices.len(), 0));
    }
    let mock = [
        (BandType::LowCut, 32.0, 0.0, 0.71),
        (BandType::Bell, 160.0, 3.5, 0.9),
        (BandType::Bell, 2800.0, -3.0, 2.6),
        (BandType::HighShelf, 9500.0, 2.5, 0.71),
    ];
    let mut settings = [BandSettings::default(); BANDS];
    for (b, &(kind, freq, gain, q)) in mock.iter().enumerate() {
        host.set_plain(&band_id(b, ON.id), 1.0);
        host.set_plain(&band_id(b, TYPE.id), kind as usize as f32);
        host.set_plain(&band_id(b, FREQ.id), freq);
        host.set_plain(&band_id(b, GAIN.id), gain);
        host.set_plain(&band_id(b, Q.id), q);
        // Bands 2 and 3 on the sides, band 4 on the right channel: shows the per-placement colours.
        host.set_plain(&band_id(b, STEREO.id), [0.0, 4.0, 4.0, 2.0][b]);
        settings[b] = BandSettings { enabled: true, kind, freq: f64::from(freq), gain_db: f64::from(gain), q: f64::from(q), slope: 3, ..Default::default() };
    }

    // Pink-ish noise (Paul Kellet's filter) through the real engine feeds both analyzer taps.
    let shared = Arc::new(EqShared::new());
    shared.sample_rate.store(48_000.0);
    let mut engine = EqEngine::new(48_000.0, &settings, 0.0);
    let mut seed = 0x2545_F491_u32;
    let (mut b0, mut b1, mut b2) = (0.0_f32, 0.0_f32, 0.0_f32);
    let mut block = vec![0.0_f32; 512];
    for _ in 0..64 {
        for s in &mut block {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            let white = (seed as f32 / u32::MAX as f32) * 2.0 - 1.0;
            b0 = 0.99765 * b0 + white * 0.0990460;
            b1 = 0.96300 * b1 + white * 0.2965164;
            b2 = 0.57000 * b2 + white * 1.0526913;
            *s = (b0 + b1 + b2 + white * 0.1848) * 0.06;
        }
        let mut l = block.clone();
        let mut r = block.clone();
        let (pl, pr) = engine.process(&mut [&mut l, &mut r], &settings, 0.0, Some(&shared.pre), Some(&shared.post));
        shared.meter.store(pl, pr);
    }

    let mut view = EqView::new(shared);
    // Let the analyzer's ballistics settle as they would live (~0.5 s of 8 ms ticks).
    for _ in 0..60 {
        view.tick(&host);
        std::thread::sleep(std::time::Duration::from_millis(8));
    }
    // Select band 3 the way a user would: click its node (mockup: 2.8 kHz, −3 dB).
    let x = 1200.0 * (2800.0_f64 / 10.0).ln() / 3000.0_f64.ln();
    let y = 56.0 + 306.0 + 3.0 * 264.0 / 18.0;
    let at = Point::new(x, y);
    let mods = Modifiers::default();
    view.event(&UiEvent::PointerDown { pos: at, button: MouseButton::Left, mods }, &host);
    view.event(&UiEvent::PointerUp { pos: at, button: MouseButton::Left, mods }, &host);
    let _ = host.normalized(OUT_GAIN.id);
    // Optional further clicks in design units, e.g. SNAPSHOT_CLICK="380,600" to open a dropdown.
    // Each is preceded by a render so the view knows where its controls are.
    if let Ok(clicks) = std::env::var("SNAPSHOT_CLICK") {
        for p in clicks.split(';') {
            let Some((x, y)) = p.split_once(',') else { continue };
            let at = Point::new(x.trim().parse()?, y.trim().parse()?);
            render_view(&mut view, &host, &Theme::load(theme, mite_eq::editor::ACCENT), 1.0)?;
            view.event(&UiEvent::PointerMove { pos: at, mods }, &host);
            view.event(&UiEvent::PointerDown { pos: at, button: MouseButton::Left, mods }, &host);
            view.event(&UiEvent::PointerUp { pos: at, button: MouseButton::Left, mods }, &host);
        }
    }

    let image = render_view(&mut view, &host, &Theme::load(theme, mite_eq::editor::ACCENT), scale)?;
    let file = std::io::BufWriter::new(std::fs::File::create(&path)?);
    let mut encoder = png::Encoder::new(file, image.width, image.height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.write_header()?.write_image_data(&image.rgba)?;
    println!("wrote {path} ({}×{})", image.width, image.height);
    Ok(())
}
