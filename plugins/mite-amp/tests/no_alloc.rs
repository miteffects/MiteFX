//! The engine's audio path must not allocate or free (BLUEPRINT §5), whatever the settings and
//! the loader do: captures and cabinets swapped mid-stream (with and without a latency change),
//! slots switched, mono blocks, odd sizes, another host rate.

use std::path::Path;

use assert_no_alloc::assert_no_alloc as process_wrapper;
use mite_amp::dsp::{AmpEngine, AmpSettings, AmpShared, CabLoad, Loaded, ModelLoad};
use mite_amp::ir;
use mite_amp::loader::MODEL_BLOCK;
use mite_nam::{LoadOptions, NamModel};

fn model(name: &str, host: f64) -> Box<ModelLoad> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../crates/mite-nam/tests/models").join(name);
    let m = NamModel::from_file(&path, LoadOptions { max_block: MODEL_BLOCK, ..LoadOptions::default() }).unwrap();
    Box::new(ModelLoad { model: Some(Loaded::new(m, host)) })
}

fn cab(host: f64) -> Box<CabLoad> {
    let h: Vec<f64> = (0..3000).map(|i| (-(i as f64) / 400.0).exp() * if i % 3 == 0 { 0.5 } else { -0.2 }).collect();
    let (h, _) = ir::prepare(&h.iter().map(|&v| v as f32).collect::<Vec<_>>(), 48_000.0, host).unwrap();
    Box::new(CabLoad { cab: Some(ir::split(&h)) })
}

#[test]
fn engine_process_never_allocates() {
    for host in [48_000.0f32, 44_100.0] {
        let mut s = AmpSettings::default();
        let shared = AmpShared::new();
        let mut engine = AmpEngine::new(host, &s);
        let (mut l, mut r) = (vec![0.0f32; 1500], vec![0.0f32; 1500]);
        let mut seed = 3u32;
        for i in 0..400 {
            s.pedal = (i / 60) % 2 == 0;
            s.cab = (i / 70) % 3 != 2;
            s.bypass = (i / 90) % 5 == 4;
            s.normalize = (i / 45) % 2 == 0;
            s.calibrate = (i / 55) % 2 == 1;
            s.bass_db = [0.0, 6.0, -9.0][(i / 40) % 3];
            s.low_cut_hz = [20.0, 120.0][(i / 33) % 2];
            match i {
                20 => shared.models[1].send(model("A2.nam", f64::from(host))),
                60 => shared.models[0].send(model("lstm.nam", f64::from(host))),
                100 => shared.cab.send(cab(f64::from(host))),
                150 => shared.models[1].send(model("wavenet_a1_standard.nam", f64::from(host))),
                200 => shared.models[0].send(Box::new(ModelLoad { model: None })),
                250 => shared.cab.send(cab(f64::from(host))),
                300 => shared.cab.send(Box::new(CabLoad { cab: None })),
                _ => {}
            }
            for (a, b) in l.iter_mut().zip(r.iter_mut()) {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                *a = (seed as f32 / u32::MAX as f32 - 0.5) * 0.2;
                *b = *a;
            }
            let block = [1500, 64, 513][i % 3];
            process_wrapper(|| engine.process(&mut [&mut l[..block], &mut r[..block]], &s, &shared));
            if i % 50 == 0 {
                process_wrapper(|| engine.process(&mut [&mut l[..block]], &s, &shared));
            }
            // The loader's side: free what was retired.
            shared.models[0].collect();
            shared.models[1].collect();
            shared.cab.collect();
            assert!(l.iter().chain(&r).all(|x| x.is_finite()));
        }
        process_wrapper(|| engine.reset(&s));
    }
}
