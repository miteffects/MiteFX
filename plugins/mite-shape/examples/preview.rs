//! Opens the Mite Shape editor in a standalone window with a drum loop running through the real
//! engine in real time, for interactive testing and frame-time measurement. The engine follows
//! the editor's parameters, so splitting bands and shaping them is heard in the pictures.
//!
//!     MITE_UI_TRACE=1 cargo run --release -p mite_shape --example preview
//!
//! PREVIEW_BANDS=3 starts with three bands (the design's kick, snare body and cymbals).

#[path = "common/drums.rs"]
mod drums;

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use mite_shape::dsp::{Output, ShapeEngine};
use mite_shape::editor::{memory_host, settings, ShapeView, ACCENT};
use mite_shape::params::*;
use mite_shape::ShapeShared;
use mite_ui::{run_standalone, ParamHost, ThemeKind, WindowOptions};

fn main() -> Result<(), String> {
    let host = memory_host();
    host.set_plain(&band_id(0, B_ATTACK.id), 6.0);
    host.set_plain(&band_id(0, B_SUSTAIN.id), -4.0);
    if std::env::var("PREVIEW_BANDS").is_ok_and(|v| v == "3") {
        for (k, f) in [(0, 150.0), (1, 3500.0)] {
            host.set_plain(&cross_id(k, X_ON.id), 1.0);
            host.set_plain(&cross_id(k, X_FREQ.id), f);
        }
        for (b, (a, s, t)) in [(5.0, -3.0, 30.0), (3.0, -6.0, 40.0), (-3.0, -2.0, 15.0)].into_iter().enumerate() {
            host.set_plain(&band_id(b, B_ATTACK.id), a);
            host.set_plain(&band_id(b, B_SUSTAIN.id), s);
            host.set_plain(&band_id(b, B_TIME.id), t);
        }
    }

    // "Audio thread": 512-sample blocks at 48 kHz, in real time, looping the drums.
    let shared = Arc::new(ShapeShared::new());
    shared.sample_rate.store(48_000.0);
    let audio = shared.clone();
    let host = Arc::new(Mutex::new(host));
    let params = host.clone();
    let _audio = std::thread::Builder::new().name("fake-audio".into()).spawn(move || {
        let [l0, r0] = drums::drum_loop(8.0, 48_000.0);
        let read = || {
            let h = params.lock().expect("host");
            (settings(&*h), h.plain(DELTA.id, h.normalized(DELTA.id)) >= 0.5)
        };
        let mut engine = ShapeEngine::new(48_000.0, &read().0);
        let start = Instant::now();
        let mut blocks = 0u64;
        let mut pos = 0usize;
        loop {
            let (s, delta) = read();
            let (mut l, mut r) = (vec![0.0f32; 512], vec![0.0f32; 512]);
            for i in 0..512 {
                l[i] = l0[(pos + i) % l0.len()];
                r[i] = r0[(pos + i) % r0.len()];
            }
            pos = (pos + 512) % l0.len();
            let out = Output { delta, ..Output::default() };
            engine.process(&mut [&mut l, &mut r], &s, &out, &audio.history, Some([&audio.spectrum, &audio.post]));
            audio.latency.store(engine.latency() as f32);
            blocks += 1;
            let due = start + Duration::from_secs_f64(blocks as f64 * 512.0 / 48_000.0);
            if let Some(wait) = due.checked_duration_since(Instant::now()) {
                std::thread::sleep(wait);
            }
        }
    });

    let view = ShapeView::new(shared);
    let options = WindowOptions {
        title: "Mite Shape — preview".into(),
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
