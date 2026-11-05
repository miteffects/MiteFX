//! A sung take through the engine as a host would play it.
#![allow(dead_code)]

use std::sync::Arc;
use std::time::Duration;

use mite_dsp::melody::voice::{Sung, sing};
use mite_pitch::dsp::{PitchEngine, Transport};
use mite_pitch::session::{Session, Status};

pub const FS: f64 = 48_000.0;

/// Four notes, a little out of tune.
pub fn take() -> Vec<f32> {
    let n = |cents: f64, seconds: f64| Sung {
        cents,
        seconds,
        vibrato_cents: 30.0,
        vibrato_hz: 5.5,
        db: -12.0,
    };
    sing(
        &[
            n(5710.0, 0.5),
            n(5925.0, 0.4),
            n(6215.0, 0.6),
            n(5885.0, 0.45),
        ],
        0.08,
        FS,
    )
    .0
}

/// A session keeping its takes in a fresh temporary folder.
pub fn session(tag: &str) -> Arc<Session> {
    let s = Session::new();
    let dir = std::env::temp_dir().join(format!("mitefx-pitch-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    s.set_dir(dir);
    s.set_sample_rate(FS as f32);
    s
}

/// Plays `x` (mono, both channels) from timeline sample `pos`, in blocks of `block`, and returns
/// what came out (left).
pub fn play(engine: &mut PitchEngine, s: &Session, x: &[f32], pos: i64, block: usize) -> Vec<f32> {
    let mut out = Vec::with_capacity(x.len());
    for (k, chunk) in x.chunks(block).enumerate() {
        let (mut l, mut r) = (chunk.to_vec(), chunk.to_vec());
        engine.process(
            &mut [&mut l, &mut r],
            Transport {
                playing: true,
                pos: Some(pos + (k * block) as i64),
            },
            false,
            s,
        );
        out.extend_from_slice(&l);
    }
    out
}

/// One stopped block (the transport stops).
pub fn stop(engine: &mut PitchEngine, s: &Session) {
    let (mut l, mut r) = (vec![0.0f32; 64], vec![0.0f32; 64]);
    engine.process(
        &mut [&mut l, &mut r],
        Transport {
            playing: false,
            pos: Some(0),
        },
        false,
        s,
    );
}

/// Arms, plays `x` from `pos`, stops, and waits for the analysis and the first render.
pub fn transfer(engine: &mut PitchEngine, s: &Session, x: &[f32], pos: i64) {
    s.set_armed(true);
    let heard = play(engine, s, x, pos, 512);
    assert_eq!(heard, x, "the input is heard while recording");
    stop(engine, s);
    assert!(s.poll(), "the recording ended");
    assert!(s.wait_idle(Duration::from_secs(30)));
    assert_eq!(s.status(), Status::Ready);
}

/// Waits for the worker, then lets the engine pick up the newest render.
pub fn settle(engine: &mut PitchEngine, s: &Session) {
    assert!(s.wait_idle(Duration::from_secs(30)));
    stop(engine, s);
}
