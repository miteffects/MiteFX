//! Renders the Mite EQ editor to a PNG, set up like the mockup (4 bands, band 3 selected), with
//! pink noise run through the real engine so the analyzer shows genuine pre/post spectra.
//!
//!     cargo run -p mite_eq --example snapshot -- out.png [scale] [dark|light|hc|cb]

use std::sync::Arc;

use mite_dsp::eq::{BandSettings, BandType};
use mite_eq::dsp::EqEngine;
use mite_eq::editor::EqView;
use mite_eq::params::{band_id, BANDS, DMODE, DYN, SUSTAIN, TRANSIENT, TRANSITION, SPECTRAL, SREL, FREQ, GAIN, ON, OUT_GAIN, Q, RANGE, STEREO, THRESH, TYPE};
use mite_eq::EqShared;
use mite_ui::kurbo::Point;
use mite_ui::render::render_view;
use mite_ui::{Modifiers, MouseButton, ParamHost, Theme, ThemeKind, UiEvent, View};

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

    let host = mite_eq::editor::memory_host();
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
        // Band 2 on the sides, band 3 in the middle, band 4 on the right channel: shows the
        // per-placement colours.
        host.set_plain(&band_id(b, STEREO.id), [0.0, 4.0, 3.0, 2.0][b]);
        settings[b] = BandSettings { enabled: true, kind, freq: f64::from(freq), gain_db: f64::from(gain), q: f64::from(q), slope: 3, ..Default::default() };
        settings[b].placement = mite_dsp::eq::Placement::from_index([0, 4, 3, 2][b]);
    }
    // Band 3 is dynamic (mockup "dynamic band"): ducks up to 6 dB above −40 dBFS. SNAPSHOT_STATIC
    // leaves it static.
    if std::env::var_os("SNAPSHOT_STATIC").is_none() {
        host.set_plain(&band_id(2, DYN.id), 1.0);
        host.set_plain(&band_id(2, THRESH.id), -40.0);
        settings[2].dynamics = mite_dsp::eq::Dynamics { enabled: true, threshold_db: -40.0, ..Default::default() };
        // SNAPSHOT_TRANSIENT shows it as a transient shaper instead: +6 dB attack, −4 dB sustain.
        if std::env::var_os("SNAPSHOT_TRANSIENT").is_some() {
            host.set_plain(&band_id(2, DMODE.id), 1.0);
            // SNAPSHOT_TRANSIENT="attack,sustain[,transition %]" picks other amounts.
            let values: Vec<f32> =
                std::env::var("SNAPSHOT_TRANSIENT").unwrap_or_default().split(',').filter_map(|v| v.trim().parse().ok()).collect();
            let (attack, sustain) = if values.len() >= 2 { (values[0], values[1]) } else { (6.0, -4.0) };
            let transition = values.get(2).copied().unwrap_or(TRANSITION.default);
            host.set_plain(&band_id(2, TRANSIENT.id), attack);
            host.set_plain(&band_id(2, SUSTAIN.id), sustain);
            host.set_plain(&band_id(2, TRANSITION.id), transition);
            settings[2].dynamics.transition = f64::from(transition) / 100.0;
            let d = &mut settings[2].dynamics;
            (d.mode, d.transient_db, d.sustain_db) = (mite_dsp::eq::DynamicsMode::Transient, f64::from(attack), f64::from(sustain));
        }
    }

    // SNAPSHOT_SPECTRAL makes band 3's compressor spectral; SNAPSHOT_RELATIVE also makes its
    // threshold relative (6 dB over the surroundings). Either way a ringing 2.8 kHz tone joins the noise, so the
    // live cuts drawn are the engine's own.
    let spectral = std::env::var_os("SNAPSHOT_SPECTRAL").is_some();
    let relative = std::env::var_os("SNAPSHOT_RELATIVE").is_some();
    let spectral = spectral || relative;
    if spectral {
        host.set_plain(&band_id(2, SPECTRAL.id), 1.0);
        host.set_plain(&band_id(2, RANGE.id), -9.0);
        settings[2].dynamics.spectral = true;
        settings[2].dynamics.range_db = -9.0;
    }
    // SNAPSHOT_SPECTRAL_FREQ=42 moves that band (the scope where STFT bins are sparse).
    if let Some(f) = std::env::var("SNAPSHOT_SPECTRAL_FREQ").ok().and_then(|v| v.parse::<f64>().ok()) {
        host.set_plain(&band_id(2, FREQ.id), f as f32);
        settings[2].freq = f;
    }
    if relative {
        host.set_plain(&band_id(2, SREL.id), 1.0);
        settings[2].dynamics.relative = true;
    }
    let ring = spectral;
    let mut phase = 0.0_f64;
    // SNAPSHOT_SIDECHAIN: a bass on the sidechain (a low tone and its harmonics, over low noise),
    // and a low tone of the same pitch in the track, so the two collide (the masking view).
    let sidechain = std::env::var_os("SNAPSHOT_SIDECHAIN").is_some();
    let mut bass_phase = 0.0_f64;
    let mut bass_noise = 0.0_f32;
    // SNAPSHOT_MATCH: the reference on the sidechain is the same noise through another EQ
    // (warmer lows, a scooped middle, more air), which EQ Match should find.
    let reference = std::env::var_os("SNAPSHOT_MATCH").is_some();
    let mut reference_bands = [BandSettings::default(); BANDS];
    for (b, &(kind, freq, gain, q)) in [(BandType::LowShelf, 120.0, 4.0, 0.71), (BandType::Bell, 600.0, -4.0, 1.0), (BandType::HighShelf, 7000.0, 5.0, 0.71)].iter().enumerate() {
        reference_bands[b] = BandSettings { enabled: true, kind, freq, gain_db: gain, q, ..Default::default() };
    }
    let mut reference_engine = EqEngine::new(48_000.0, &reference_bands, 0.0);

    // Pink-ish noise (Paul Kellet's filter) through the real engine feeds both analyzer taps.
    let shared = Arc::new(EqShared::new());
    // The masking view shows when asked for (it is off by default).
    let masking = std::env::var_os("SNAPSHOT_SIDECHAIN").is_some() || std::env::var_os("SNAPSHOT_PEER").is_some();
    shared.show_masking.store(masking, std::sync::atomic::Ordering::Relaxed);
    // SNAPSHOT_FOCUS: focus mode (no band panel).
    shared.focus_mode.store(std::env::var_os("SNAPSHOT_FOCUS").is_some(), std::sync::atomic::Ordering::Relaxed);
    shared.sample_rate.store(48_000.0);
    let mut engine = EqEngine::new(48_000.0, &settings, 0.0);
    let mut seed = 0x2545_F491_u32;
    let (mut b0, mut b1, mut b2) = (0.0_f32, 0.0_f32, 0.0_f32);
    let mut block = vec![0.0_f32; 512];
    for _ in 0..if ring { 160 } else { 64 } {
        for s in &mut block {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            let white = (seed as f32 / u32::MAX as f32) * 2.0 - 1.0;
            b0 = 0.99765 * b0 + white * 0.0990460;
            b1 = 0.96300 * b1 + white * 0.2965164;
            b2 = 0.57000 * b2 + white * 1.0526913;
            *s = (b0 + b1 + b2 + white * 0.1848) * 0.06;
            if ring {
                phase += 2.0 * std::f64::consts::PI * 2800.0 / 48_000.0;
                *s += (0.12 * phase.sin()) as f32;
            }
            if sidechain {
                bass_phase += 2.0 * std::f64::consts::PI * 87.0 / 48_000.0;
                *s += (0.05 * bass_phase.sin()) as f32;
            }
        }
        if sidechain {
            let bass: Vec<f32> = (0..block.len())
                .map(|i| {
                    seed ^= seed << 13;
                    seed ^= seed >> 17;
                    seed ^= seed << 5;
                    let white = (seed as f32 / u32::MAX as f32) * 2.0 - 1.0;
                    bass_noise = 0.985 * bass_noise + 0.015 * white;
                    let t = bass_phase + 2.0 * std::f64::consts::PI * 87.0 / 48_000.0 * (i as f64 - block.len() as f64);
                    (0.25 * t.sin() + 0.1 * (2.0 * t).sin() + 0.05 * (3.0 * t).sin()) as f32 + 0.3 * bass_noise
                })
                .collect();
            shared.sidechain.push(bass.into_iter());
        }
        if reference {
            let (mut l, mut r) = (block.clone(), block.clone());
            reference_engine.process(&mut [&mut l, &mut r], &reference_bands, 0.0, None, None);
            shared.sidechain.push(l.iter().zip(&r).map(|(a, b)| 0.5 * (a + b)));
        }
        let mut l = block.clone();
        let mut r = block.clone();
        let (pl, pr) = engine.process(&mut [&mut l, &mut r], &settings, 0.0, Some(&shared.pre), Some(&shared.post));
        shared.meter.store(pl, pr);
        for (slot, db) in shared.dynamic.iter().zip(engine.dynamic_gains()) {
            slot.store(db);
        }
        for (slot, db) in shared.level.iter().zip(engine.detector_levels()) {
            slot.store(db);
        }
        for (i, slot) in shared.spectral.iter().enumerate() {
            slot.store(engine.spectral_gain_db(mite_eq::spectral_point_freq(i)));
        }
        // The editor will select band 3: its per-bin detail.
        for i in 0..mite_eq::SPECTRAL_POINTS {
            let (level, threshold, gain) = engine.spectral_band_db(2, i);
            shared.focus_level[i].store(level);
            shared.focus_threshold[i].store(threshold);
            shared.focus_gain[i].store(gain);
        }
    }

    shared.latency.store(engine.latency() as f32);
    // SNAPSHOT_NP shows Natural Phase.
    if std::env::var_os("SNAPSHOT_NP").is_some() {
        host.set_plain("mode", 1.0);
        shared.latency.store(mite_eq::dsp::natural_phase_latency(48_000.0) as f32);
    }
    // SNAPSHOT_LP=res shows Linear Phase at that resolution (0 = Low … 3 = Max).
    if let Some(res) = std::env::var("SNAPSHOT_LP").ok().and_then(|v| v.parse::<usize>().ok()) {
        host.set_plain("mode", 2.0);
        host.set_plain("lp_res", res as f32);
        shared.latency.store(mite_eq::dsp::linear_phase_latency(res, 48_000.0) as f32);
    }
    // SNAPSHOT_PEER: another Mite EQ in the process plays the bass the sidechain would (the
    // masking view between instances). It stands in as reachable, so both carve rings show.
    let peer = Arc::new(EqShared::new());
    if std::env::var_os("SNAPSHOT_PEER").is_some() {
        mite_eq::INSTANCES.register(&shared);
        mite_eq::INSTANCES.register(&peer);
        let mut bass = vec![0.0f32; mite_eq::editor::ANALYZER_SIZE];
        shared.sidechain.read_latest(&mut bass);
        peer.post.push(bass.into_iter());
        shared.sidechain.push(std::iter::repeat_n(0.0, mite_eq::editor::ANALYZER_SIZE));
        let remote: mite_plugin::SharedParamHost = Arc::new(ReachablePeer);
        *peer.remote.lock().unwrap() = Some(remote);
    }
    // SNAPSHOT_RANGE=6 picks the plot's dB range by hand (Auto otherwise).
    if let Some(r) = std::env::var("SNAPSHOT_RANGE").ok().and_then(|v| v.parse::<f32>().ok()) {
        shared.display_range.store(r);
    }
    let mut view = EqView::new(shared);
    // Let the analyzer's ballistics settle as they would live (~0.5 s of 8 ms ticks).
    for _ in 0..60 {
        view.tick(&host);
        std::thread::sleep(std::time::Duration::from_millis(8));
    }
    if sidechain {
        // The masking averages need a moment of both playing.
        for _ in 0..90 {
            view.tick(&host);
            std::thread::sleep(std::time::Duration::from_millis(16));
        }
    }
    // Select band 3 the way a user would: click its node (mockup: 2.8 kHz, −3 dB).
    let x = 1200.0 * (settings[2].freq / 10.0).ln() / 3000.0_f64.ln();
    let y = 56.0 + 306.0 + 3.0 * 264.0 / 18.0;
    let at = Point::new(x, y);
    let mods = Modifiers::default();
    view.event(&UiEvent::PointerDown { pos: at, button: MouseButton::Left, mods }, &host);
    view.event(&UiEvent::PointerUp { pos: at, button: MouseButton::Left, mods }, &host);
    let _ = host.normalized(OUT_GAIN.id);
    // A tick after selecting, as live: picks up the selected band's per-bin detail.
    view.tick(&host);
    // SNAPSHOT_MATCH_FILE=path: match to that audio file.
    if let Ok(file) = std::env::var("SNAPSHOT_MATCH_FILE") {
        view.load_match_reference(file.into());
        for _ in 0..150 {
            view.tick(&host);
            std::thread::sleep(std::time::Duration::from_millis(16));
        }
    }
    // SNAPSHOT_MATCH: open EQ Match (its header button) and let it learn and fit.
    if std::env::var_os("SNAPSHOT_MATCH").is_some() {
        render_view(&mut view, &host, &Theme::load(theme, mite_eq::editor::ACCENT), 1.0)?;
        let at = Point::new(806.0, 28.0);
        view.event(&UiEvent::PointerDown { pos: at, button: MouseButton::Left, mods }, &host);
        view.event(&UiEvent::PointerUp { pos: at, button: MouseButton::Left, mods }, &host);
        view.event(&UiEvent::PointerLeft, &host);
        for _ in 0..150 {
            view.tick(&host);
            std::thread::sleep(std::time::Duration::from_millis(16));
        }
    }
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
    // Optional press-and-move without release, e.g. SNAPSHOT_DRAG="490,372;540,430" (a drag in
    // progress, such as the ghost Bell of a grabbed peak).
    if let Ok(drag) = std::env::var("SNAPSHOT_DRAG") {
        let points: Vec<Point> = drag
            .split(';')
            .filter_map(|p| p.split_once(',').and_then(|(x, y)| Some(Point::new(x.trim().parse().ok()?, y.trim().parse().ok()?))))
            .collect();
        if let [from, to] = points[..] {
            render_view(&mut view, &host, &Theme::load(theme, mite_eq::editor::ACCENT), 1.0)?;
            for _ in 0..20 {
                std::thread::sleep(std::time::Duration::from_millis(16));
                view.tick(&host);
            }
            view.event(&UiEvent::PointerMove { pos: from, mods }, &host);
            view.event(&UiEvent::PointerDown { pos: from, button: MouseButton::Left, mods }, &host);
            view.event(&UiEvent::PointerMove { pos: to, mods }, &host);
        }
    }
    // Optional pointer position for hover states, e.g. SNAPSHOT_HOVER="700,470".
    if let Ok(hover) = std::env::var("SNAPSHOT_HOVER") {
        if let Some((x, y)) = hover.split_once(',') {
            render_view(&mut view, &host, &Theme::load(theme, mite_eq::editor::ACCENT), 1.0)?;
            view.event(&UiEvent::PointerMove { pos: Point::new(x.trim().parse()?, y.trim().parse()?), mods }, &host);
            // Hover states that fade in (the peak marker, peak hold after 300 ms) need live ticks.
            for _ in 0..45 {
                std::thread::sleep(std::time::Duration::from_millis(16));
                view.tick(&host);
            }
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

/// A peer's parameters that accept writes and read as defaults (the snapshot only draws).
struct ReachablePeer;

impl ParamHost for ReachablePeer {
    fn normalized(&self, _id: &str) -> f32 {
        0.0
    }
    fn plain(&self, _id: &str, normalized: f32) -> f32 {
        normalized
    }
    fn normalize(&self, _id: &str, plain: f32) -> f32 {
        plain
    }
    fn default_normalized(&self, _id: &str) -> f32 {
        0.0
    }
    fn begin_gesture(&self, _id: &str) {}
    fn set_normalized(&self, _id: &str, _normalized: f32) {}
    fn end_gesture(&self, _id: &str) {}
}
