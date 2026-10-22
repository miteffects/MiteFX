//! Opens the Mite Comp editor in a standalone window with a drum-like signal running through the
//! real engine in real time (two stages: a leveller and a compressor), for interactive testing
//! and frame-time measurement.
//!
//!     MITE_UI_TRACE=1 cargo run --release -p mite_comp --example preview
//!
//! PREVIEW_CLICK="x,y[;x,y…]" clicks there (design units) once the editor is up, e.g. on a node
//! to open its panel; with MITE_UI_GPU_TIME=1 the traced render time includes the GPU's work.

use std::sync::Arc;
use std::time::{Duration, Instant};

use mite_comp::dsp::{CompEngine, Output};
use mite_comp::editor::{memory_host, CompView, ACCENT};
use mite_comp::params::*;
use mite_comp::CompShared;
use mite_dsp::dynamics::{CompSettings, Detector, Quadrant, Stage};
use mite_ui::kurbo::Point;
use mite_ui::{run_standalone, DrawCx, Modifiers, MouseButton, ParamHost, ThemeKind, UiEvent, View, WindowOptions};

fn main() -> Result<(), String> {
    let host = memory_host();
    let stages = [
        Stage { on: true, quadrant: Quadrant::UpCompress, threshold_db: -40.0, ratio: 2.0, knee_db: 8.0, range_db: 12.0, attack_ms: 30.0, release_ms: 800.0, detector: Detector::Rms },
        Stage { on: true, quadrant: Quadrant::Compress, threshold_db: -20.0, ratio: 4.0, knee_db: 6.0, range_db: 60.0, attack_ms: 10.0, release_ms: 120.0, detector: Detector::Rms },
    ];
    let mut settings = CompSettings { auto_makeup: true, ..CompSettings::default() };
    for (slot, s) in stages.iter().enumerate() {
        settings.stages[slot] = *s;
        let id = |p: &str| stage_id(slot, p);
        host.set_plain(&id(S_ON.id), 1.0);
        host.set_plain(&id(S_QUAD.id), s.quadrant as usize as f32);
        host.set_plain(&id(S_THRESH.id), s.threshold_db);
        host.set_plain(&id(S_RATIO.id), s.ratio);
        host.set_plain(&id(S_KNEE.id), s.knee_db);
        host.set_plain(&id(S_ATTACK.id), s.attack_ms);
        host.set_plain(&id(S_RELEASE.id), s.release_ms);
        host.set_plain(&id(S_RANGE.id), s.range_db);
    }
    host.set_plain(AUTO_MAKEUP.id, 1.0);

    // "Audio thread": 512-sample blocks at 48 kHz, in real time.
    let shared = Arc::new(CompShared::new());
    shared.sample_rate.store(48_000.0);
    let audio = shared.clone();
    let _audio = std::thread::Builder::new().name("fake-audio".into()).spawn(move || {
        let mut engine = CompEngine::new(48_000.0, &settings);
        let mut seed = 0x2545_F491_u32;
        let mut block = vec![0.0f32; 512];
        let start = Instant::now();
        let mut blocks = 0u64;
        loop {
            let t = blocks as f32 * 512.0 / 48_000.0;
            let beat = (t * 4.0).fract();
            let level = -30.0 + 22.0 * (-beat * 8.0).exp() + if (t % 8.0) > 6.0 { -22.0 } else { 0.0 };
            for s in &mut block {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                *s = ((seed as f32 / u32::MAX as f32) * 2.0 - 1.0) * 1.6 * mite_dsp::db_to_gain(level);
            }
            let (mut l, mut r) = (block.clone(), block.clone());
            engine.process(&mut [&mut l, &mut r], None, &settings, &Output::default(), &audio.history);
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
    let view = Clicking { view: CompView::new(shared), clicks, ticks: 0 };
    let (w, h) = view.logical_size();
    let options = WindowOptions {
        title: "Mite Comp — preview".into(),
        logical_size: (w, h),
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
    view: CompView,
    clicks: Vec<Point>,
    ticks: u32,
}

impl View for Clicking {
    fn logical_size(&self) -> (f64, f64) {
        self.view.logical_size()
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
