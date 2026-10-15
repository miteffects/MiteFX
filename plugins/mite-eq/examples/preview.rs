//! Opens the Mite EQ editor in a standalone window with live pink noise running through the real
//! engine (as if audio were playing), for interactive testing and frame-time measurement.
//!
//!     MITE_UI_TRACE=1 cargo run --release -p mite_eq --example preview

use std::sync::Arc;
use std::time::{Duration, Instant};

use mite_core::{ParamDisplay, ParamScale, ParamSpec};
use mite_dsp::eq::{BandSettings, BandType};
use mite_eq::dsp::EqEngine;
use mite_eq::editor::{EqView, ACCENT};
use mite_eq::params::{band_id, ATTACK, BANDS, DMODE, DYN, SUSTAIN, TRANSIENT, TRANSITION, TTIME, FREQ, GAIN, ON, OUT_GAIN, Q, RANGE, RATIO, RELEASE, SLOPE, STEREO, THRESH, TYPE};
use mite_eq::EqShared;
use mite_ui::{run_standalone, MemoryParamHost, ThemeKind, View, WindowOptions};

fn index_spec(id: &'static str, count: usize, default: usize) -> ParamSpec {
    ParamSpec {
        id,
        name: id,
        unit: "",
        min: 0.0,
        max: (count - 1) as f32,
        default: default as f32,
        scale: ParamScale::Linear,
        smoothing_ms: 0.0,
        decimals: 0,
        wheel_step: 1.0,
        wheel_step_fine: 1.0,
        display: ParamDisplay::Number,
    }
}

fn leak(s: String) -> &'static str {
    Box::leak(s.into_boxed_str())
}

fn main() -> Result<(), String> {
    let mut host = MemoryParamHost::new().with(OUT_GAIN);
    for b in 0..BANDS {
        host = host
            .with_id(leak(band_id(b, ON.id)), index_spec("on", 2, 0))
            .with_id(leak(band_id(b, TYPE.id)), index_spec("type", TYPE.choices.len(), 0))
            .with_id(leak(band_id(b, FREQ.id)), FREQ)
            .with_id(leak(band_id(b, GAIN.id)), GAIN)
            .with_id(leak(band_id(b, Q.id)), Q)
            .with_id(leak(band_id(b, SLOPE.id)), index_spec("slope", SLOPE.choices.len(), SLOPE.default))
            .with_id(leak(band_id(b, STEREO.id)), index_spec("stereo", STEREO.choices.len(), 0))
            .with_id(leak(band_id(b, DYN.id)), index_spec("dyn", 2, 0))
            .with_id(leak(band_id(b, RANGE.id)), RANGE)
            .with_id(leak(band_id(b, THRESH.id)), THRESH)
            .with_id(leak(band_id(b, ATTACK.id)), ATTACK)
            .with_id(leak(band_id(b, RELEASE.id)), RELEASE)
            .with_id(leak(band_id(b, RATIO.id)), RATIO)
            .with_id(leak(band_id(b, DMODE.id)), index_spec("dmode", 2, 0))
            .with_id(leak(band_id(b, SUSTAIN.id)), SUSTAIN)
            .with_id(leak(band_id(b, TRANSIENT.id)), TRANSIENT)
            .with_id(leak(band_id(b, TTIME.id)), TTIME)
            .with_id(leak(band_id(b, TRANSITION.id)), TRANSITION);
    }
    let mock = [
        (BandType::LowCut, 32.0, 0.0, 0.71),
        (BandType::Bell, 160.0, 3.5, 0.9),
        (BandType::Bell, 2800.0, -3.0, 2.6),
        (BandType::HighShelf, 9500.0, 2.5, 0.71),
    ];
    let mut settings = [BandSettings::default(); BANDS];
    for (b, &(kind, freq, gain, q)) in mock.iter().enumerate() {
        host.set_plain(&band_id(b, ON.id), 1.0);
        host.set_plain(&band_id(b, TYPE.id), kind as usize as f32);
        host.set_plain(&band_id(b, FREQ.id), freq);
        host.set_plain(&band_id(b, GAIN.id), gain);
        host.set_plain(&band_id(b, Q.id), q);
        settings[b] = BandSettings { enabled: true, kind, freq: f64::from(freq), gain_db: f64::from(gain), q: f64::from(q), slope: 3, ..Default::default() };
    }

    // "Audio thread": 512-sample blocks of pink noise at 48 kHz, in real time.
    let shared = Arc::new(EqShared::new());
    shared.sample_rate.store(48_000.0);
    let audio = shared.clone();
    let _audio_thread = std::thread::Builder::new().name("fake-audio".into()).spawn(move || {
        let mut engine = EqEngine::new(48_000.0, &settings, 0.0);
        let mut seed = 0x2545_F491_u32;
        let (mut b0, mut b1, mut b2) = (0.0_f32, 0.0_f32, 0.0_f32);
        let mut block = vec![0.0_f32; 512];
        let start = Instant::now();
        let mut blocks = 0u64;
        loop {
            for s in &mut block {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                let white = (seed as f32 / u32::MAX as f32) * 2.0 - 1.0;
                b0 = 0.99765 * b0 + white * 0.0990460;
                b1 = 0.96300 * b1 + white * 0.2965164;
                b2 = 0.57000 * b2 + white * 1.0526913;
                *s = (b0 + b1 + b2 + white * 0.1848) * 0.03;
            }
            let (mut l, mut r) = (block.clone(), block.clone());
            let (pl, pr) = engine.process(&mut [&mut l, &mut r], &settings, 0.0, Some(&audio.pre), Some(&audio.post));
            audio.meter.store(pl, pr);
            blocks += 1;
            let due = start + Duration::from_secs_f64(blocks as f64 * 512.0 / 48_000.0);
            if let Some(wait) = due.checked_duration_since(Instant::now()) {
                std::thread::sleep(wait);
            }
        }
    });

    let view = EqView::new(shared);
    let (w, h) = view.logical_size();
    let zoom = view.zoom();
    let options = WindowOptions {
        title: "Mite EQ — preview".into(),
        logical_size: (w * zoom, h * zoom),
        fallback_scale: None,
        theme: ThemeKind::Dark,
        accent: ACCENT,
        on_resize: None,
        host_resize: None,
    };
    run_standalone(options, Box::new(view), Box::new(host)).map_err(|e| format!("{e:?}"))
}
