//! Opens the Mite Limit editor in a standalone window with music running through the real engine
//! in real time, for interactive testing and frame-time measurement. The engine follows the
//! editor's parameters, so what you drag is heard in the pictures.
//!
//!     MITE_UI_TRACE=1 cargo run --release -p mite_limit --example preview
//!
//! PREVIEW_GAIN=12 starts with that much gain (6 dB otherwise).

#[path = "common/music.rs"]
mod music;

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use mite_limit::dsp::LimitEngine;
use mite_limit::editor::{memory_host, settings, LimitView, ACCENT};
use mite_limit::params::*;
use mite_limit::LimitShared;
use mite_ui::{run_standalone, ParamHost, ThemeKind, WindowOptions};

fn main() -> Result<(), String> {
    let host = memory_host();
    host.set_plain(GAIN.id, std::env::var("PREVIEW_GAIN").ok().and_then(|v| v.parse().ok()).unwrap_or(6.0));

    // "Audio thread": 512-sample blocks at 48 kHz, in real time, looping the music.
    let shared = Arc::new(LimitShared::new());
    shared.sample_rate.store(48_000.0);
    let audio = shared.clone();
    let host = Arc::new(Mutex::new(host));
    let params = host.clone();
    let _audio = std::thread::Builder::new().name("fake-audio".into()).spawn(move || {
        let [l0, r0] = music::music(8.0, 48_000.0);
        let read = || {
            let h = params.lock().expect("host");
            settings(&*h)
        };
        let mut engine = LimitEngine::new(48_000.0, &read());
        let start = Instant::now();
        let mut blocks = 0u64;
        let mut pos = 0usize;
        loop {
            let s = read();
            let (mut l, mut r) = (vec![0.0f32; 512], vec![0.0f32; 512]);
            for i in 0..512 {
                l[i] = l0[(pos + i) % l0.len()];
                r[i] = r0[(pos + i) % r0.len()];
            }
            pos = (pos + 512) % l0.len();
            engine.process(&mut [&mut l, &mut r], &s, &audio.history, &audio.loudness);
            audio.latency.store(engine.latency() as f32);
            blocks += 1;
            let due = start + Duration::from_secs_f64(blocks as f64 * 512.0 / 48_000.0);
            if let Some(wait) = due.checked_duration_since(Instant::now()) {
                std::thread::sleep(wait);
            }
        }
    });

    let view = LimitView::new(shared);
    let options = WindowOptions {
        title: "Mite Limit — preview".into(),
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
