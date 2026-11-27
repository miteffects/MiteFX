//! Mite Cut's engine on a real track (mono f32 file): every candidate with its level, own share,
//! score and verdict, as `TrackDetection::hits` reads it at the given sensitivities.
//!
//! `cargo run --profile fasttest -p mite-dsp --example hits_probe -- <fs> <audio.f32> <role> [sens...]`

use mite_dsp::hits::{detect, Role, Settings};

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let fs: f32 = a[0].parse().expect("fs");
    let x: Vec<f32> = std::fs::read(&a[1]).expect("audio").chunks_exact(4).map(|b| f32::from_le_bytes(b.try_into().unwrap())).collect();
    let role = mite_dsp::hits::Role::ALL.into_iter().find(|r| r.name().eq_ignore_ascii_case(&a[2])).unwrap_or(Role::Kick);
    let det = detect(&[(&x, role)], fs);
    let sens: Vec<f32> = if a.len() > 3 { a[3..].iter().map(|v| v.parse().unwrap()).collect() } else { vec![0.5] };
    for s in sens {
        let hits = det[0].hits(&Settings { sensitivity: s, ..Settings::default() }, &[]);
        println!("== sensitivity {s}: {} candidates, {} kept", hits.len(), hits.iter().filter(|h| h.kept).count());
        for h in &hits {
            let peak = x[h.onset..(h.onset + (0.02 * fs) as usize).min(x.len())].iter().fold(0.0f32, |m, v| m.max(v.abs()));
            println!("{:8.3} s  peak {:6.1} dBFS  level {:6.1}  own {:6.1}  score {:6.1}  {}{}", h.onset as f32 / fs, 20.0 * peak.max(1e-9).log10(), h.level_db, h.dominance_db, h.score_db, if h.kept { "KEPT" } else if h.inside { "inside" } else { "cut" }, if (0.25..0.75).contains(&h.confidence) { "  (near)" } else { "" });
        }
    }
}
