//! Opens the Mite Pitch editor in a standalone window over a sung take looping through the real
//! engine in real time (the transport playing): click TRANSFER and the next lap is recorded.
//!
//!     MITE_UI_TRACE=1 cargo run --release -p mite_pitch --example preview

#[path = "common/take.rs"]
mod common;

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use mite_pitch::PitchShared;
use mite_pitch::dsp::{PitchEngine, Transport};
use mite_pitch::editor::{ACCENT, PitchView, memory_host};
use mite_ui::{ParamHost, ThemeKind, WindowOptions, run_standalone};

fn main() -> Result<(), String> {
    let host = Arc::new(Mutex::new(memory_host()));
    let s = common::session("preview");
    let shared = Arc::new(PitchShared {
        session: s.clone(),
        sample_rate: mite_core::AtomicF32::new(48_000.0),
    });
    let params = host.clone();
    let _audio = std::thread::Builder::new()
        .name("fake-audio".into())
        .spawn(move || {
            let mut x = vec![0.0f32; 24_000];
            x.extend(common::take());
            x.extend(vec![0.0f32; 24_000]);
            let mut engine = PitchEngine::new(48_000.0);
            let start = Instant::now();
            let mut blocks = 0u64;
            let mut pos = 0usize;
            loop {
                let bypass = {
                    let h = params.lock().expect("host");
                    h.plain("bypass", h.normalized("bypass")) >= 0.5
                };
                let (mut l, mut r) = (vec![0.0f32; 512], vec![0.0f32; 512]);
                for i in 0..512 {
                    l[i] = x.get(pos + i).copied().unwrap_or(0.0);
                    r[i] = l[i];
                }
                // A lap: play the take, then one stopped block (the loop jumps back).
                let playing = pos < x.len();
                engine.process(
                    &mut [&mut l, &mut r],
                    Transport {
                        playing,
                        pos: Some(pos as i64),
                    },
                    bypass,
                    &s,
                );
                pos = if playing { pos + 512 } else { 0 };
                blocks += 1;
                let due = start + Duration::from_secs_f64(blocks as f64 * 512.0 / 48_000.0);
                if let Some(wait) = due.checked_duration_since(Instant::now()) {
                    std::thread::sleep(wait);
                }
            }
        });

    let view = PitchView::new(shared);
    let options = WindowOptions {
        title: "Mite Pitch — preview".into(),
        logical_size: (1280.0, 760.0),
        fallback_scale: None,
        theme: ThemeKind::Dark,
        accent: ACCENT,
        on_resize: None,
        host_resize: None,
    };
    run_standalone(options, Box::new(view), Box::new(SharedHost(host)))
        .map_err(|e| format!("{e:?}"))
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
