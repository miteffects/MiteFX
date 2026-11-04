//! Opens the Mite DeEss editor in a standalone window with a voice (vowels and sibilants, loud
//! and quiet phrases) running through the real engine in real time (it follows the editor's
//! parameters), for interactive testing and frame-time measurement.
//!
//!     MITE_UI_TRACE=1 cargo run --release -p mite_deess --example preview

#[path = "common/voice.rs"]
mod voice;

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use mite_deess::dsp::DeEssEngine;
use mite_deess::editor::{memory_host, out_gain, settings, DeEssView, ACCENT};
use mite_deess::DeEssShared;
use mite_ui::{run_standalone, ParamHost, ThemeKind, WindowOptions};

fn main() -> Result<(), String> {
    let host = Arc::new(Mutex::new(memory_host()));
    let shared = Arc::new(DeEssShared::new());
    shared.sample_rate.store(48_000.0);
    let audio = shared.clone();
    let params = host.clone();
    let _audio = std::thread::Builder::new().name("fake-audio".into()).spawn(move || {
        let x = voice::voice(6.4, 48_000.0);
        let read = || {
            let h = params.lock().expect("host");
            (settings(&*h), out_gain(&*h))
        };
        let (s0, g0) = read();
        let mut engine = DeEssEngine::new(48_000.0, &s0, g0);
        let start = Instant::now();
        let mut blocks = 0u64;
        let mut pos = 0usize;
        loop {
            let (s, g) = read();
            let (mut l, mut r) = (vec![0.0f32; 512], vec![0.0f32; 512]);
            for i in 0..512 {
                l[i] = x[(pos + i) % x.len()];
                r[i] = l[i];
            }
            pos = (pos + 512) % x.len();
            engine.process(&mut [&mut l, &mut r], &s, g, &audio.history, Some([&audio.spectrum, &audio.output]));
            blocks += 1;
            let due = start + Duration::from_secs_f64(blocks as f64 * 512.0 / 48_000.0);
            if let Some(wait) = due.checked_duration_since(Instant::now()) {
                std::thread::sleep(wait);
            }
        }
    });
    let view = DeEssView::new(shared);
    let options = WindowOptions {
        title: "Mite DeEss — preview".into(),
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
