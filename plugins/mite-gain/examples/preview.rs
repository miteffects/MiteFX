//! Opens the Mite Gain editor in a standalone window with a simulated meter signal.
//!
//!     cargo run -p mite_gain --example preview

use std::sync::Arc;
use std::time::{Duration, Instant};

use mite_core::StereoMeter;
use mite_gain::editor::{GainView, HEIGHT, WIDTH};
use mite_gain::params::GAIN;
use mite_ui::{ThemeKind, run_standalone, MemoryParamHost, WindowOptions};

fn main() -> Result<(), String> {
    let meter = Arc::new(StereoMeter::new(-150.0));
    let feed = meter.clone();
    std::thread::spawn(move || {
        let start = Instant::now();
        loop {
            let t = start.elapsed().as_secs_f32();
            feed.store(-12.0 + 6.0 * (t * 1.9).sin() + 2.0 * (t * 5.3).sin(), -13.0 + 6.5 * (t * 1.7 + 1.0).sin());
            std::thread::sleep(Duration::from_millis(10));
        }
    });
    let options = WindowOptions { title: "Mite Gain — preview".into(), logical_size: (WIDTH, HEIGHT), fallback_scale: None, theme: ThemeKind::Dark, accent: "utility" };
    run_standalone(options, Box::new(GainView::new(meter)), Box::new(MemoryParamHost::new().with(GAIN)))
        .map_err(|e| format!("{e:?}"))
}
