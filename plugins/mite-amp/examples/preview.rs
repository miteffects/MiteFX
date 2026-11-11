//! Opens the Mite Amp editor in a standalone window with the example rig (NAM's example A2 capture,
//! a WaveNet pedal, a cabinet) playing a guitar-like loop through the real engine in real time (it
//! follows the editor's parameters and rig), for interactive testing and frame-time measurement.
//!
//!     MITE_UI_TRACE=1 cargo run --release -p mite_amp --example preview

#[path = "common/rig.rs"]
mod rig;

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use mite_amp::dsp::{AmpEngine, AmpShared};
use mite_amp::editor::{memory_host, settings, AmpView, ACCENT};
use mite_amp::loader::{Loader, LoaderCore};
use mite_amp::params::AmpParams;
use mite_amp::AmpUi;
use mite_ui::{run_standalone, ParamHost, ThemeKind, WindowOptions};

fn main() -> Result<(), String> {
    let host = Arc::new(Mutex::new(memory_host()));
    let params = Arc::new(AmpParams::default());
    let shared = Arc::new(AmpShared::new());
    let fs = 48_000.0f32;
    let core = rig::load(&params, &shared, fs, false);
    drop(core);
    let loader = Loader::new(LoaderCore::new(params.rig.clone(), shared.clone(), params.clone()));
    loader.core.set_rate(fs);
    let ui = Arc::new(AmpUi::with_stores(params.snapshots.clone(), params.preset.clone(), params.rig.clone(), shared.clone()));
    ui.sample_rate.store(fs);
    let audio = shared.clone();
    let audio_ui = ui.clone();
    let knobs = host.clone();
    let _audio = std::thread::Builder::new().name("fake-audio".into()).spawn(move || {
        let x = rig::guitar(8.0, fs);
        let read = || settings(&*knobs.lock().expect("host"));
        let mut engine = AmpEngine::new(fs, &read());
        let start = Instant::now();
        let mut blocks = 0u64;
        let mut pos = 0usize;
        loop {
            let s = read();
            let (mut l, mut r) = (vec![0.0f32; 512], vec![0.0f32; 512]);
            for i in 0..512 {
                l[i] = x[(pos + i) % x.len()];
                r[i] = l[i];
            }
            pos = (pos + 512) % x.len();
            engine.process(&mut [&mut l, &mut r], &s, &audio);
            audio_ui.latency.store(engine.latency() as f32);
            blocks += 1;
            let due = start + Duration::from_secs_f64(blocks as f64 * 512.0 / f64::from(fs));
            if let Some(wait) = due.checked_duration_since(Instant::now()) {
                std::thread::sleep(wait);
            }
        }
    });
    let view = AmpView::new(ui);
    let options = WindowOptions {
        title: "Mite Amp — preview".into(),
        logical_size: (1280.0, 760.0),
        fallback_scale: None,
        theme: ThemeKind::Dark,
        accent: ACCENT,
        on_resize: None,
        host_resize: None,
    };
    let result = run_standalone(options, Box::new(view), Box::new(SharedHost(host))).map_err(|e| format!("{e:?}"));
    drop(loader);
    result
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
