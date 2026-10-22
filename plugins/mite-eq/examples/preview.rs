//! Opens the Mite EQ editor in a standalone window with live pink noise running through the real
//! engine (as if audio were playing), for interactive testing and frame-time measurement.
//!
//!     MITE_UI_TRACE=1 cargo run --release -p mite_eq --example preview
//!
//! PREVIEW_SPECTRAL=1 makes band 3 a spectral compressor (PREVIEW_RELATIVE=1: relative) and adds a
//! ringing 2.8 kHz tone to the noise. PREVIEW_CLICK="x,y[;x,y…]" clicks there (design units) once
//! the editor is up, e.g. on a node to open its panel; with MITE_UI_GPU_TIME=1 the traced render
//! time includes the GPU's work (bloom, glass).

use std::sync::Arc;
use std::time::{Duration, Instant};

use mite_dsp::eq::{BandSettings, BandType};
use mite_eq::dsp::EqEngine;
use mite_eq::editor::{EqView, ACCENT};
use mite_eq::params::{band_id, BANDS, DYN, SPECTRAL, SREL, FREQ, GAIN, ON, Q, RANGE, THRESH, TYPE};
use mite_eq::EqShared;
use mite_ui::kurbo::Point;
use mite_ui::{run_standalone, DrawCx, Modifiers, MouseButton, ParamHost, ThemeKind, UiEvent, View, WindowOptions};

fn main() -> Result<(), String> {
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
        settings[b] = BandSettings { enabled: true, kind, freq: f64::from(freq), gain_db: f64::from(gain), q: f64::from(q), slope: 3, ..Default::default() };
    }

    let relative = std::env::var_os("PREVIEW_RELATIVE").is_some();
    let spectral = std::env::var_os("PREVIEW_SPECTRAL").is_some() || relative;
    if spectral {
        host.set_plain(&band_id(2, DYN.id), 1.0);
        host.set_plain(&band_id(2, SPECTRAL.id), 1.0);
        host.set_plain(&band_id(2, RANGE.id), -9.0);
        host.set_plain(&band_id(2, THRESH.id), -40.0);
        host.set_plain(&band_id(2, SREL.id), if relative { 1.0 } else { 0.0 });
        settings[2].dynamics = mite_dsp::eq::Dynamics { enabled: true, spectral: true, relative, range_db: -9.0, threshold_db: -40.0, ..Default::default() };
    }
    let resonance = spectral;

    // "Audio thread": 512-sample blocks of pink noise at 48 kHz, in real time.
    let shared = Arc::new(EqShared::new());
    shared.sample_rate.store(48_000.0);
    let audio = shared.clone();
    let _audio_thread = std::thread::Builder::new().name("fake-audio".into()).spawn(move || {
        let mut engine = EqEngine::new(48_000.0, &settings, 0.0);
        let mut seed = 0x2545_F491_u32;
        let (mut b0, mut b1, mut b2) = (0.0_f32, 0.0_f32, 0.0_f32);
        let mut block = vec![0.0_f32; 512];
        let start = Instant::now();
        let mut blocks = 0u64;
        let mut phase = 0.0_f64;
        loop {
            for s in &mut block {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                let white = (seed as f32 / u32::MAX as f32) * 2.0 - 1.0;
                b0 = 0.99765 * b0 + white * 0.0990460;
                b1 = 0.96300 * b1 + white * 0.2965164;
                b2 = 0.57000 * b2 + white * 1.0526913;
                *s = (b0 + b1 + b2 + white * 0.1848) * 0.03;
                if resonance {
                    // Rings on and off every second, so the cut comes and goes.
                    phase += 2.0 * std::f64::consts::PI * 2800.0 / 48_000.0;
                    let on = (blocks / 94) % 2 == 0;
                    *s += if on { (0.08 * phase.sin()) as f32 } else { 0.0 };
                }
            }
            let (mut l, mut r) = (block.clone(), block.clone());
            let (pl, pr) = engine.process(&mut [&mut l, &mut r], &settings, 0.0, Some(&audio.pre), Some(&audio.post));
            audio.meter.store(pl, pr);
            for (slot, db) in audio.dynamic.iter().zip(engine.dynamic_gains()) {
                slot.store(db);
            }
            for (i, slot) in audio.spectral.iter().enumerate() {
                slot.store(engine.spectral_gain_db(mite_eq::spectral_point_freq(i)));
            }
            audio.latency.store(engine.latency() as f32);
            let focus = audio.focus.load();
            if focus >= 0.0 {
                for i in 0..mite_eq::SPECTRAL_POINTS {
                    let (level, threshold, gain) = engine.spectral_band_db(focus as usize, i);
                    audio.focus_level[i].store(level);
                    audio.focus_threshold[i].store(threshold);
                    audio.focus_gain[i].store(gain);
                }
            }
            blocks += 1;
            let due = start + Duration::from_secs_f64(blocks as f64 * 512.0 / 48_000.0);
            if let Some(wait) = due.checked_duration_since(Instant::now()) {
                std::thread::sleep(wait);
            }
        }
    });

    let clicks = std::env::var("PREVIEW_CLICK")
        .unwrap_or_default()
        .split(';')
        .filter_map(|p| {
            let (x, y) = p.split_once(',')?;
            Some(Point::new(x.trim().parse().ok()?, y.trim().parse().ok()?))
        })
        .collect();
    let view = Clicking { view: EqView::new(shared), clicks, ticks: 0 };
    let (w, h) = view.logical_size();
    let zoom = view.zoom();
    let options = WindowOptions {
        title: "Mite EQ — preview".into(),
        logical_size: (w * zoom, h * zoom),
        fallback_scale: None,
        theme: ThemeKind::Dark,
        accent: ACCENT,
        on_resize: None,
        host_resize: None,
    };
    run_standalone(options, Box::new(view), Box::new(host)).map_err(|e| format!("{e:?}"))
}

/// The editor, plus clicks sent a few ticks after it has drawn (so it knows its layout).
struct Clicking {
    view: EqView,
    clicks: Vec<Point>,
    ticks: u32,
}

impl View for Clicking {
    fn logical_size(&self) -> (f64, f64) {
        self.view.logical_size()
    }

    fn zoom(&self) -> f64 {
        self.view.zoom()
    }

    fn draw(&mut self, cx: &mut DrawCx<'_>) {
        self.view.draw(cx);
    }

    fn event(&mut self, event: &UiEvent, host: &dyn ParamHost) -> bool {
        self.view.event(event, host)
    }

    fn tick(&mut self, host: &dyn ParamHost) -> bool {
        self.ticks += 1;
        if self.ticks == 10 && !self.clicks.is_empty() {
            let mods = Modifiers::default();
            for pos in std::mem::take(&mut self.clicks) {
                self.view.event(&UiEvent::PointerMove { pos, mods }, host);
                self.view.event(&UiEvent::PointerDown { pos, button: MouseButton::Left, mods }, host);
                self.view.event(&UiEvent::PointerUp { pos, button: MouseButton::Left, mods }, host);
            }
        }
        self.view.tick(host) || self.ticks == 10
    }
}
