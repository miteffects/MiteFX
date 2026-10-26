//! Opens the Mite Sat editor in a standalone window with music running through the real engine
//! in real time, for interactive testing and frame-time measurement. The engine follows the
//! editor's parameters, so what you drag is heard in the pictures.
//!
//!     MITE_UI_TRACE=1 cargo run --release -p mite_sat --example preview
//!
//! PREVIEW_BANDS=3 or 6 starts with that many bands, each on its own model.

#[path = "common/music.rs"]
mod music;

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use mite_dsp::sat::Model;
use mite_sat::dsp::{Output, SatEngine};
use mite_sat::editor::{memory_host, oversampling, settings, SatView, ACCENT};
use mite_sat::params::*;
use mite_sat::SatShared;
use mite_ui::{run_standalone, ParamHost, ThemeKind, WindowOptions};

fn main() -> Result<(), String> {
    let host = memory_host();
    host.set_plain(&band_id(0, B_MODEL.id), Model::Tube.index() as f32);
    host.set_plain(&band_id(0, B_DRIVE.id), 12.0);
    let bands: usize = std::env::var("PREVIEW_BANDS").ok().and_then(|v| v.parse().ok()).unwrap_or(1);
    let models = [Model::Soft, Model::Hard, Model::Fold, Model::Diode, Model::Rectify];
    for (k, f) in [150.0, 600.0, 2000.0, 5000.0, 11_000.0].into_iter().enumerate().take(bands.clamp(1, 6) - 1) {
        host.set_plain(&cross_id(k, X_ON.id), 1.0);
        host.set_plain(&cross_id(k, X_FREQ.id), f);
        host.set_plain(&band_id(k + 1, B_MODEL.id), models[k].index() as f32);
        host.set_plain(&band_id(k + 1, B_DRIVE.id), 12.0);
    }

    // "Audio thread": 512-sample blocks at 48 kHz, in real time, looping the music.
    let shared = Arc::new(SatShared::new());
    shared.sample_rate.store(48_000.0);
    let audio = shared.clone();
    let host = Arc::new(Mutex::new(host));
    let params = host.clone();
    let _audio = std::thread::Builder::new().name("fake-audio".into()).spawn(move || {
        let [l0, r0] = music::music(8.0, 48_000.0);
        let read = || {
            let h = params.lock().expect("host");
            let p = |id: &str| h.plain(id, h.normalized(id));
            let out = Output { mix: p(MIX.id) / 100.0, delta: p(DELTA.id) >= 0.5, bypass: p(BYPASS.id) >= 0.5, gain_db: p(OUT_GAIN.id), pan: p(OUT_PAN.id) / 100.0 };
            (settings(&*h), oversampling(&*h), out)
        };
        let (s0, f0, _) = read();
        let mut engine = SatEngine::new(48_000.0, f0, &s0);
        let start = Instant::now();
        let mut blocks = 0u64;
        let mut pos = 0usize;
        loop {
            let (s, factor, out) = read();
            let (mut l, mut r) = (vec![0.0f32; 512], vec![0.0f32; 512]);
            for i in 0..512 {
                l[i] = l0[(pos + i) % l0.len()];
                r[i] = r0[(pos + i) % r0.len()];
            }
            pos = (pos + 512) % l0.len();
            engine.process(&mut [&mut l, &mut r], &s, factor, &out, &audio.history, Some([&audio.spectrum, &audio.post]));
            audio.latency.store(engine.latency() as f32);
            blocks += 1;
            let due = start + Duration::from_secs_f64(blocks as f64 * 512.0 / 48_000.0);
            if let Some(wait) = due.checked_duration_since(Instant::now()) {
                std::thread::sleep(wait);
            }
        }
    });

    let view = SatView::new(shared);
    let options = WindowOptions {
        title: "Mite Sat — preview".into(),
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
