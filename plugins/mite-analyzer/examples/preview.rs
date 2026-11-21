//! Opens the Mite Analyzer editor in a standalone window with music (a drum loop, bass and
//! chords) running through the real engine in real time, for interactive testing and frame-time
//! measurement.
//!
//!     MITE_UI_TRACE=1 cargo run --release -p mite_analyzer --example preview
//!
//! PREVIEW_PHASE=1 opens the phase page with a chain to measure: the music through a 60 Hz
//! high-pass and 128 samples of delay, the dry music on the sidechain.

#[path = "common/music.rs"]
#[allow(dead_code)]
mod music;

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use mite_analyzer::dsp::AnalyzerEngine;
use mite_analyzer::editor::{memory_host, AnalyzerView, ACCENT};
use mite_analyzer::AnalyzerShared;
use mite_ui::{run_standalone, ParamHost, ThemeKind, WindowOptions};

fn main() -> Result<(), String> {
    let host = Arc::new(Mutex::new(memory_host()));
    let phase = std::env::var_os("PREVIEW_PHASE").is_some();
    if phase {
        host.lock().expect("host").set_plain(mite_analyzer::params::PAGE.id, 1.0);
    }
    let shared = Arc::new(AnalyzerShared::new());
    shared.sample_rate.store(48_000.0);
    let audio = shared.clone();
    let _audio = std::thread::Builder::new().name("fake-audio".into()).spawn(move || {
        let [xl, xr] = music::music(16.0, 48_000.0);
        let mut engine = AnalyzerEngine::new(48_000.0);
        let start = Instant::now();
        let mut blocks = 0u64;
        let mut pos = 0usize;
        // The chain: an RBJ high-pass (60 Hz, Q 0.707) per channel, then 128 samples of delay.
        let w = std::f64::consts::TAU * 60.0 / 48_000.0;
        let (c, alpha) = (w.cos(), w.sin() / (2.0 * std::f64::consts::FRAC_1_SQRT_2));
        let a0 = 1.0 + alpha;
        let (b0, b1, a1, a2) = ((1.0 + c) / 2.0 / a0, -(1.0 + c) / a0, -2.0 * c / a0, (1.0 - alpha) / a0);
        let mut state = [[0.0f64; 2]; 2];
        let mut line = [vec![0.0f32; 128], vec![0.0f32; 128]];
        let mut at = 0usize;
        loop {
            let (mut l, mut r) = (vec![0.0f32; 512], vec![0.0f32; 512]);
            for i in 0..512 {
                l[i] = xl[(pos + i) % xl.len()];
                r[i] = xr[(pos + i) % xr.len()];
            }
            pos = (pos + 512) % xl.len();
            if phase {
                let dry = [l.clone(), r.clone()];
                for i in 0..512 {
                    for (ch, x) in [&mut l, &mut r].into_iter().enumerate() {
                        let v = f64::from(x[i]);
                        let s = &mut state[ch];
                        let y = b0 * v + s[0];
                        s[0] = b1 * v - a1 * y + s[1];
                        s[1] = b0 * v - a2 * y;
                        x[i] = std::mem::replace(&mut line[ch][at], y as f32);
                    }
                    at = (at + 1) % 128;
                }
                let sc: [&[f32]; 2] = [&dry[0], &dry[1]];
                engine.process_with(&[&mut l, &mut r], Some(&sc), &audio.tel);
            } else {
                engine.process(&[&mut l, &mut r], &audio.tel);
            }
            blocks += 1;
            let due = start + Duration::from_secs_f64(blocks as f64 * 512.0 / 48_000.0);
            if let Some(wait) = due.checked_duration_since(Instant::now()) {
                std::thread::sleep(wait);
            }
        }
    });
    let view = AnalyzerView::new(shared);
    let options = WindowOptions {
        title: "Mite Analyzer — preview".into(),
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
