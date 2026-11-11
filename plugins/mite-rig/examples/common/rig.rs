//! The examples' board: NAM's example captures (from mite-nam's tests) and a synthetic cabinet,
//! and a guitar-like test sound.

use std::path::{Path, PathBuf};

use mite_rig::board::Board;
use mite_rig::blocks::Kind;
use mite_rig::rig::{SlotFile, Source};

pub fn models_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../crates/mite-nam/tests/models")
}

/// A 4×12-like cabinet (real `mite_dsp::eq` filters on an impulse), written as a float WAV.
pub fn cabinet(path: &Path) {
    use mite_dsp::eq::{design, BandSettings, BandType};
    use mite_dsp::filter::Tdf2;
    let fs = 48_000.0;
    let band = |kind, freq: f64, gain_db: f64, q: f64, slope: usize| BandSettings { enabled: true, kind, freq, gain_db, q, slope, ..BandSettings::default() };
    let bands = [band(BandType::LowCut, 80.0, 0.0, 0.7, 1), band(BandType::Bell, 110.0, 4.0, 1.4, 1), band(BandType::Bell, 800.0, -5.0, 1.0, 1), band(BandType::Bell, 2500.0, 5.0, 1.2, 1), band(BandType::HighCut, 5000.0, 0.0, 0.7, 5)];
    let mut x: Vec<f64> = (0..2400).map(|i| match i { 0 => 1.0, 37 => 0.18, 91 => -0.1, _ => 0.0 }).collect();
    for b in &bands {
        let sections = design(b, fs);
        let mut states = vec![Tdf2::default(); sections.as_slice().len()];
        for v in x.iter_mut() {
            for (c, st) in sections.as_slice().iter().zip(states.iter_mut()) {
                *v = st.process(c, *v);
            }
        }
    }
    let data: Vec<u8> = x.iter().flat_map(|&s| (s as f32).to_le_bytes()).collect();
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

/// Puts the example files in the default board's capture and cabinet blocks.
pub fn fill(board: &Board) {
    let cab = std::env::temp_dir().join("mite-rig-example-cab.wav");
    cabinet(&cab);
    let source = Source { tone_id: 27156, model_id: 1, title: "Example A2 Amp".into(), creator: "Steven Atkinson".into(), license: "mit".into(), url: "https://www.tone3000.com".into(), gear: "amp".into() };
    for b in board.blocks() {
        match b.kind() {
            Kind::Capture => board.set_file(b.id(), SlotFile { path: Some(models_dir().join("A2.nam")), source: Some(source.clone()) }),
            Kind::Cab => board.set_file(b.id(), SlotFile { path: Some(cab.clone()), source: None }),
            _ => {}
        }
    }
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
