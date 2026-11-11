//! The examples' rig: NAM's example captures (from mite-nam's tests) and a synthetic cabinet,
//! loaded through the real loader, and a guitar-like test sound.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use mite_amp::dsp::AmpShared;
use mite_amp::loader::LoaderCore;
use mite_amp::params::AmpParams;
use mite_amp::rig::{Slot, SlotFile, Source};

pub fn models_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../crates/mite-nam/tests/models")
}

/// A 4×12-like cabinet: low cut under 80 Hz, a resonance at 110 Hz, a dip at 800 Hz, presence at
/// 2.5 kHz and a steep roll-off over 5 kHz (real `mite_dsp::eq` filters on an impulse, plus two
/// faint early reflections), written as a float WAV.
pub fn cabinet(path: &Path) {
    use mite_dsp::eq::{design, BandSettings, BandType};
    use mite_dsp::filter::Tdf2;
    let fs = 48_000.0;
    let band = |kind, freq: f64, gain_db: f64, q: f64, slope: usize| BandSettings { enabled: true, kind, freq, gain_db, q, slope, ..BandSettings::default() };
    let bands = [
        band(BandType::LowCut, 80.0, 0.0, 0.7, 1),
        band(BandType::Bell, 110.0, 4.0, 1.4, 1),
        band(BandType::Bell, 800.0, -5.0, 1.0, 1),
        band(BandType::Bell, 2500.0, 5.0, 1.2, 1),
        band(BandType::HighCut, 5000.0, 0.0, 0.7, 5),
    ];
    let n = 2400;
    let mut x: Vec<f64> = (0..n).map(|i| match i { 0 => 1.0, 37 => 0.18, 91 => -0.1, _ => 0.0 }).collect();
    for b in &bands {
        let sections = design(b, fs);
        let mut states = vec![Tdf2::default(); sections.as_slice().len()];
        for v in x.iter_mut() {
            for (c, st) in sections.as_slice().iter().zip(states.iter_mut()) {
                *v = st.process(c, *v);
            }
        }
    }
    let h: Vec<f32> = x.iter().map(|&v| v as f32).collect();
    let data: Vec<u8> = h.iter().flat_map(|s| s.to_le_bytes()).collect();
    let mut out = Vec::new();
    out.extend(b"RIFF");
    out.extend((36 + data.len() as u32).to_le_bytes());
    out.extend(b"WAVEfmt ");
    out.extend(16u32.to_le_bytes());
    out.extend(3u16.to_le_bytes());
    out.extend(1u16.to_le_bytes());
    out.extend(48_000u32.to_le_bytes());
    out.extend((48_000u32 * 4).to_le_bytes());
    out.extend(4u16.to_le_bytes());
    out.extend(32u16.to_le_bytes());
    out.extend(b"data");
    out.extend((data.len() as u32).to_le_bytes());
    out.extend(data);
    std::fs::write(path, out).expect("cabinet written");
}

/// Loads the rig through the loader (A2 amp, the small WaveNet as a pedal, the cabinet).
pub fn load(params: &Arc<AmpParams>, shared: &Arc<AmpShared>, rate: f32, with_pedal: bool) -> Arc<LoaderCore> {
    let rig = params.rig.clone();
    let cab = std::env::temp_dir().join("mite-amp-example-cab.wav");
    cabinet(&cab);
    if with_pedal {
        rig.set_file(Slot::Pedal, SlotFile { path: Some(models_dir().join("wavenet.nam")), source: None });
    }
    let source = Source { tone_id: 27156, model_id: 1, title: "Example A2 Rig".into(), creator: "Steven Atkinson".into(), license: "mit".into(), url: "https://www.tone3000.com".into(), gear: "amp".into() };
    rig.set_file(Slot::Amp, SlotFile { path: Some(models_dir().join("A2.nam")), source: Some(source) });
    rig.set_file(Slot::Cab, SlotFile { path: Some(cab), source: None });
    let core = Arc::new(LoaderCore::new(rig, shared.clone(), params.clone()));
    core.set_rate(rate);
    core.step();
    core
}

/// Palm-muted chugs and a ringing chord: plucked strings (decaying harmonics).
pub fn guitar(seconds: f32, fs: f32) -> Vec<f32> {
    let n = (seconds * fs) as usize;
    (0..n)
        .map(|i| {
            let t = i as f32 / fs;
            let beat = (t * 4.0).fract() / 4.0;
            let chug: f32 = (1..8).map(|h| (std::f32::consts::TAU * 82.4 * h as f32 * t).sin() / h as f32).sum::<f32>() * (-beat / 0.06).exp();
            let ring = if (t % 2.0) > 1.0 { [110.0f32, 146.8, 196.0].iter().map(|f| (std::f32::consts::TAU * f * t).sin()).sum::<f32>() * (-((t % 1.0) / 0.8)).exp() * 0.3 } else { 0.0 };
            0.25 * (chug * 0.6 + ring)
        })
        .collect()
}
