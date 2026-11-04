//! Opens the Mite Tuner editor in a standalone window, a guitar's six strings plucked in turn
//! (each a little off) through the real engine in real time, for interactive testing and
//! frame-time measurement.
//!
//!     MITE_UI_TRACE=1 cargo run --release -p mite_tuner --example preview

#[path = "common/pluck.rs"]
mod common;

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use mite_tuner::dsp::TunerEngine;
use mite_tuner::editor::{memory_host, TunerView, ACCENT};
use mite_tuner::TunerShared;
use mite_ui::{run_standalone, ParamHost, ThemeKind, WindowOptions};

fn main() -> Result<(), String> {
    let host = memory_host();
    let shared = Arc::new(TunerShared::new());
    shared.sample_rate.store(48_000.0);
    let audio = shared.clone();
    let host = Arc::new(Mutex::new(host));
    let params = host.clone();
    // "Audio thread": the six strings of a guitar in turn, each a little off, in real time.
    let _audio = std::thread::Builder::new().name("fake-audio".into()).spawn(move || {
        let mut x = Vec::new();
        for (hz, cents) in [(82.41, -14.0), (110.0, 6.0), (146.83, -3.0), (196.0, 0.5), (246.94, 22.0), (329.63, -1.0)] {
            x.extend(common::pluck(hz * 2f64.powf(cents / 1200.0), 10.0, 3.0, 3.0, 48_000.0));
        }
        let mut engine = TunerEngine::new(48_000.0, false);
        let start = Instant::now();
        let mut blocks = 0u64;
        let mut pos = 0usize;
        loop {
            let mute = {
                let h = params.lock().expect("host");
                h.plain("mute", h.normalized("mute")) >= 0.5
            };
            let (mut l, mut r) = (vec![0.0f32; 512], vec![0.0f32; 512]);
            for i in 0..512 {
                l[i] = x[(pos + i) % x.len()];
                r[i] = l[i];
            }
            pos = (pos + 512) % x.len();
            engine.process(&mut [&mut l, &mut r], mute, &audio.history, Some(&audio.input));
            blocks += 1;
            let due = start + Duration::from_secs_f64(blocks as f64 * 512.0 / 48_000.0);
            if let Some(wait) = due.checked_duration_since(Instant::now()) {
                std::thread::sleep(wait);
            }
        }
    });

    let view = TunerView::new(shared);
    let options = WindowOptions {
        title: "Mite Tuner — preview".into(),
        logical_size: (1280.0, 760.0),
        fallback_scale: None,
        theme: ThemeKind::Dark,
        accent: ACCENT,
        on_resize: None,
        host_resize: None,
    };
    run_standalone(options, Box::new(view), Box::new(SharedHost(host))).map_err(|e| format!("{e:?}"))
}

/// The stand-in host, shared with the fake audio thread.
struct SharedHost(Arc<Mutex<mite_ui::MemoryParamHost>>);

impl SharedHost {
    fn host(&self) -> std::sync::MutexGuard<'_, mite_ui::MemoryParamHost> {
        self.0.lock().expect("host")
    }
}

impl ParamHost for SharedHost {
    fn normalized(&self, id: &str) -> f32 {
        self.host().normalized(id)
    }
    fn plain(&self, id: &str, normalized: f32) -> f32 {
        self.host().plain(id, normalized)
    }
    fn normalize(&self, id: &str, plain: f32) -> f32 {
        self.host().normalize(id, plain)
    }
    fn default_normalized(&self, id: &str) -> f32 {
        self.host().default_normalized(id)
    }
    fn begin_gesture(&self, id: &str) {
        self.host().begin_gesture(id);
    }
    fn set_normalized(&self, id: &str, value: f32) {
        self.host().set_normalized(id, value);
    }
    fn end_gesture(&self, id: &str) {
        self.host().end_gesture(id);
    }
}
