//! CPU cost of building one editor frame (scene + text, no GPU) in the worst case for dynamic
//! bands: 8 dynamic bells whose live gain changes every frame, one selected (panel open).
//!
//!     cargo run --release -p mite_eq --example bench_draw     (BENCH_TRANSIENT=1 for transient bands,
//!                                                              BENCH_SPECTRAL=1 for spectral ones)

use std::sync::Arc;

use mite_core::{ParamDisplay, ParamScale, ParamSpec};
use mite_eq::editor::{EqView, ACCENT};
use mite_eq::params::{band_id, ATTACK, BANDS, DMODE, DYN, SUSTAIN, TRANSIENT, TRANSITION, TTIME, SPECTRAL, SSMOOTH, SREL, RTHRESH, FREQ, GAIN, ON, OUT_GAIN, Q, RANGE, RATIO, RELEASE, SLOPE, STEREO, THRESH, TYPE};
use mite_eq::EqShared;
use mite_ui::kurbo::Point;
use mite_ui::vello::Scene;
use mite_ui::{DrawCx, MemoryParamHost, Modifiers, MouseButton, Theme, ThemeKind, Typesetter, UiEvent, View};

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

fn main() {
    let mut host = MemoryParamHost::new()
        .with(OUT_GAIN)
        .with_id("mode", index_spec("mode", 3, 0))
        .with_id("lp_res", index_spec("lp_res", 4, 2));
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
            .with_id(leak(band_id(b, TRANSITION.id)), TRANSITION)
            .with_id(leak(band_id(b, SPECTRAL.id)), index_spec("spectral", 2, 0))
            .with_id(leak(band_id(b, SSMOOTH.id)), SSMOOTH)
            .with_id(leak(band_id(b, SREL.id)), index_spec("srel", 2, 0))
            .with_id(leak(band_id(b, RTHRESH.id)), RTHRESH);
    }
    let bell = mite_dsp::eq::BandType::Bell as usize as f32;
    for b in 0..8 {
        host.set_plain(&band_id(b, ON.id), 1.0);
        host.set_plain(&band_id(b, TYPE.id), bell);
        host.set_plain(&band_id(b, FREQ.id), 60.0 * 2f32.powi(b as i32));
        host.set_plain(&band_id(b, GAIN.id), 2.0);
        host.set_plain(&band_id(b, DYN.id), 1.0);
        // BENCH_TRANSIENT=1: transient shapers instead, whose panel simulates a hit every frame.
        if std::env::var_os("BENCH_TRANSIENT").is_some() {
            host.set_plain(&band_id(b, DMODE.id), 1.0);
            host.set_plain(&band_id(b, TRANSIENT.id), 6.0);
            host.set_plain(&band_id(b, SUSTAIN.id), -4.0);
        }
        // BENCH_SPECTRAL=1: spectral compressors (alternately relative), with the
        // per-bin curve moving every frame.
        if std::env::var_os("BENCH_SPECTRAL").is_some() {
            host.set_plain(&band_id(b, SPECTRAL.id), 1.0);
            host.set_plain(&band_id(b, SREL.id), (b % 2) as f32);
            host.set_plain(&band_id(b, RANGE.id), -6.0);
        }
    }
    let shared = Arc::new(EqShared::new());
    shared.sample_rate.store(48_000.0);
    let mut view = EqView::new(shared.clone());
    let theme = Theme::load(ThemeKind::Dark, ACCENT);
    let mut text = Typesetter::new();
    let size = view.logical_size();
    let mut draw = |view: &mut EqView| {
        let mut scene = Scene::new();
        view.draw(&mut DrawCx { scene: &mut scene, theme: &theme, text: &mut text, host: &host, size });
    };
    // Select band 4 (480 Hz, +2 dB) by clicking its node, so the dynamic panel is open.
    draw(&mut view);
    let at = Point::new(1200.0 * (480.0_f64 / 10.0).ln() / 3000.0_f64.ln(), 56.0 + 306.0 - 2.0 * 264.0 / 18.0);
    let mods = Modifiers::default();
    view.event(&UiEvent::PointerDown { pos: at, button: MouseButton::Left, mods }, &host);
    view.event(&UiEvent::PointerUp { pos: at, button: MouseButton::Left, mods }, &host);

    let frames = 600;
    let mut times = Vec::with_capacity(frames);
    for f in 0..frames {
        for (b, slot) in shared.dynamic.iter().enumerate() {
            slot.store(-6.0 * (0.5 + 0.5 * ((f + b * 7) as f32 * 0.1).sin()));
        }
        for (b, slot) in shared.level.iter().enumerate() {
            slot.store(-30.0 + 12.0 * ((f + b * 5) as f32 * 0.13).sin());
        }
        for (i, slot) in shared.spectral.iter().enumerate() {
            slot.store(-4.0 * (0.5 + 0.5 * ((f + i) as f32 * 0.21).sin()));
        }
        view.tick(&host);
        let t = std::time::Instant::now();
        draw(&mut view);
        times.push(t.elapsed().as_secs_f64() * 1e3);
    }
    times.sort_by(f64::total_cmp);
    let mean = times.iter().sum::<f64>() / frames as f64;
    println!("frame build (8 moving bands, panel open): mean {mean:.2} ms, p95 {:.2} ms, max {:.2} ms", times[frames * 95 / 100], times[frames - 1]);
}
