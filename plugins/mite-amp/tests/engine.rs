//! The engine against what it should be: a capture through the plugin is the capture; a capture
//! at another rate comes out exactly `latency()` late; a cabinet convolves exactly, with no
//! latency; swaps do not click; the loader's files reach the audio thread.

use std::path::{Path, PathBuf};

use mite_amp::dsp::{AmpEngine, AmpSettings, AmpShared, CabLoad, Loaded, ModelLoad};
use mite_amp::ir;
use mite_amp::loader::{LoaderCore, MODEL_BLOCK};
use mite_amp::params::AmpParams;
use mite_amp::rig::{Slot, SlotFile, Status};
use mite_nam::{LoadOptions, NamModel};
use std::sync::Arc;

fn models_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../crates/mite-nam/tests/models")
}

fn tmp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("mite-amp-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(name)
}

/// A 32-bit float mono WAV.
fn write_wav(path: &Path, rate: u32, samples: &[f32]) {
    let data: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
    let mut out = Vec::new();
    out.extend(b"RIFF");
    out.extend((36 + data.len() as u32).to_le_bytes());
    out.extend(b"WAVEfmt ");
    out.extend(16u32.to_le_bytes());
    out.extend(3u16.to_le_bytes());
    out.extend(1u16.to_le_bytes());
    out.extend(rate.to_le_bytes());
    out.extend((rate * 4).to_le_bytes());
    out.extend(4u16.to_le_bytes());
    out.extend(32u16.to_le_bytes());
    out.extend(b"data");
    out.extend((data.len() as u32).to_le_bytes());
    out.extend(data);
    std::fs::write(path, out).unwrap();
}

/// A model that is the identity (a one-tap Linear model) at `rate`.
fn identity_model(rate: u32) -> PathBuf {
    let path = tmp(&format!("identity-{rate}.nam"));
    std::fs::write(&path, format!(r#"{{"version":"0.5.4","architecture":"Linear","config":{{"receptive_field":1,"bias":false}},"weights":[1.0],"sample_rate":{rate}}}"#)).unwrap();
    path
}

fn quiet() -> AmpSettings {
    AmpSettings { gate: false, normalize: false, ..AmpSettings::default() }
}

fn model(path: &Path, host: f64) -> Box<ModelLoad> {
    let m = NamModel::from_file(path, LoadOptions { max_block: MODEL_BLOCK, ..LoadOptions::default() }).unwrap();
    Box::new(ModelLoad { model: Some(Loaded::new(m, host)) })
}

fn run(e: &mut AmpEngine, s: &AmpSettings, shared: &AmpShared, x: &[f32], block: usize) -> Vec<f32> {
    let (mut l, mut r) = (x.to_vec(), x.to_vec());
    for (a, b) in l.chunks_mut(block).zip(r.chunks_mut(block)) {
        e.process(&mut [a, b], s, shared);
    }
    assert_eq!(l, r, "the output is the same on both sides");
    l
}

fn noise(n: usize, level: f32) -> Vec<f32> {
    let mut seed = 0x2468_ace1u32;
    (0..n)
        .map(|_| {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            level * (seed as f32 / u32::MAX as f32 * 2.0 - 1.0)
        })
        .collect()
}

#[test]
fn a_capture_through_the_plugin_is_the_capture() {
    let path = models_dir().join("A2.nam");
    let s = quiet();
    let shared = AmpShared::new();
    let mut e = AmpEngine::new(48_000.0, &s);
    drop(e.install_now([None, Some(model(&path, 48_000.0))], None));
    e.reset(&s);
    assert_eq!(e.latency(), 0);
    let x = noise(9600, 0.2);
    let y = run(&mut e, &s, &shared, &x, 333);
    let mut reference = NamModel::from_file(&path, LoadOptions { max_block: MODEL_BLOCK, ..LoadOptions::default() }).unwrap();
    let mut want = vec![0.0; x.len()];
    reference.process(&x, &mut want);
    let err = y.iter().zip(&want).fold(0.0f32, |m, (a, b)| m.max((a - b).abs()));
    assert!(err < 1e-5, "{err}");
}

#[test]
fn a_capture_at_another_rate_comes_out_exactly_its_latency_late() {
    let path = identity_model(48_000);
    for host in [44_100.0f32, 96_000.0] {
        let s = quiet();
        let shared = AmpShared::new();
        let mut e = AmpEngine::new(host, &s);
        drop(e.install_now([None, Some(model(&path, f64::from(host)))], None));
        e.reset(&s);
        let l = e.latency();
        assert!(l > 0);
        // A band-limited tone (well inside both rates' pass bands).
        let x: Vec<f32> = (0..(host as usize / 4)).map(|i| 0.5 * (std::f32::consts::TAU * 997.0 * i as f32 / host).sin()).collect();
        let y = run(&mut e, &s, &shared, &x, 128);
        let err = (2000..x.len()).fold(0.0f32, |m, i| m.max((y[i] - x[i - l]).abs()));
        assert!(err < 2e-3, "{host}: latency {l}, error {err}");
    }
}

#[test]
fn a_cabinet_convolves_exactly_and_without_latency() {
    // Taps at 0, 10 and 100 (past the direct head): the output is those echoes of the input.
    let mut h = vec![0.0f32; 160];
    h[0] = 0.8;
    h[10] = -0.4;
    h[100] = 0.2;
    let path = tmp("cab.wav");
    write_wav(&path, 48_000, &h);
    let (cab, info) = ir::load(&path, 48_000.0).unwrap();
    assert_eq!(info.channels, 1);
    // The response is normalised to 0 dB over 100 Hz…5 kHz: find the gain it got from its head.
    let g = 0.8 / cab.head[0];
    let s = quiet();
    let shared = AmpShared::new();
    let mut e = AmpEngine::new(48_000.0, &s);
    drop(e.install_now([None, None], Some(Box::new(CabLoad { cab: Some(cab) }))));
    e.reset(&s);
    assert_eq!(e.latency(), 0);
    let x = noise(4800, 0.3);
    let y = run(&mut e, &s, &shared, &x, 77);
    let at = |i: usize, d: usize| if i >= d { x[i - d] } else { 0.0 };
    let err = (0..x.len()).fold(0.0f32, |m, i| m.max((y[i] - (0.8 * at(i, 0) - 0.4 * at(i, 10) + 0.2 * at(i, 100)) / g).abs()));
    assert!(err < 1e-5, "{err}");
}

#[test]
fn swapping_captures_and_cabinets_does_not_click() {
    // A steady tone; captures swap from identity to A2 and back, a cabinet comes and goes: the
    // largest sample-to-sample step stays near the music's own.
    let host = 48_000.0;
    let s = quiet();
    let shared = AmpShared::new();
    let mut e = AmpEngine::new(host, &s);
    let id = identity_model(48_000);
    drop(e.install_now([None, Some(model(&id, f64::from(host)))], None));
    e.reset(&s);
    let x: Vec<f32> = (0..48_000).map(|i| 0.3 * (std::f32::consts::TAU * 110.0 * i as f32 / host).sin()).collect();
    let mut cab = vec![0.0f32; 300];
    cab[0] = 1.0;
    cab[200] = 0.5;
    let cab_path = tmp("cab-swap.wav");
    write_wav(&cab_path, 48_000, &cab);
    let (mut l, mut r) = (x.clone(), x.clone());
    for (k, (a, b)) in l.chunks_mut(256).zip(r.chunks_mut(256)).enumerate() {
        match k {
            40 => shared.models[1].send(model(&models_dir().join("wavenet.nam"), f64::from(host))),
            80 => shared.cab.send(Box::new(CabLoad { cab: Some(ir::load(&cab_path, f64::from(host)).unwrap().0) })),
            120 => shared.models[1].send(model(&id, f64::from(host))),
            150 => shared.cab.send(Box::new(CabLoad { cab: None })),
            _ => {}
        }
        e.process(&mut [a, b], &s, &shared);
        shared.models[1].collect();
        shared.cab.collect();
    }
    let step = |v: &[f32]| v.windows(2).map(|w| (w[1] - w[0]).abs()).fold(0.0f32, f32::max);
    let tone_step = step(&x);
    // The wavenet capture changes the waveform, so judge against its own steady output too.
    let worst = step(&l[1000..]);
    assert!(worst < 4.0 * tone_step.max(step(&l[12_000..20_000])), "step {worst} vs tone {tone_step}");
}

#[test]
fn the_loader_builds_what_the_rig_asks_for_and_the_engine_takes_it() {
    let params = Arc::new(AmpParams::default());
    let rig = params.rig.clone();
    let shared = Arc::new(AmpShared::new());
    let core = LoaderCore::new(rig.clone(), shared.clone(), params.clone());
    core.set_rate(44_100.0);
    rig.set_file(Slot::Amp, SlotFile { path: Some(models_dir().join("A2.nam")), source: None });
    rig.set_file(Slot::Pedal, SlotFile { path: Some(tmp("missing.nam")), source: None });
    assert!(core.step());
    assert!(matches!(rig.status(Slot::Amp), Status::Model(ref i) if i.sizes.len() == 2));
    assert!(matches!(rig.status(Slot::Pedal), Status::Failed(_)));
    assert!(!core.step(), "nothing changed, nothing rebuilt");
    let s = quiet();
    let mut e = AmpEngine::new(44_100.0, &s);
    let x = noise(4410, 0.2);
    let _ = run(&mut e, &s, &shared, &x, 256);
    // A 48 kHz capture at 44.1 kHz: the latency changed behind the dip.
    assert_eq!(e.latency(), 32);
    assert!(shared.installed.load(std::sync::atomic::Ordering::Acquire) >= 1);
    // Emptying the slot sends the empty slot.
    rig.set_file(Slot::Amp, SlotFile::default());
    assert!(core.step());
    assert_eq!(rig.status(Slot::Amp), Status::Empty);
    let _ = run(&mut e, &s, &shared, &x, 256);
    assert_eq!(e.latency(), 0);
}
