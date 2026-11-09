//! The Shimmer type through the whole plugin path: the host's parameters, the engine, the mix.

use mite_core::HistoryTap;
use mite_verb::dsp::{record, VerbEngine};
use mite_verb::editor::{memory_host, output, settings};

/// The energy within ±25 Hz of `f` (a few frequencies summed: the detuned voices sit a few Hz off).
fn level(v: &[f32], f: f32) -> f32 {
    let mut e = 0.0f64;
    for df in (-25..=25).step_by(1) {
        let (mut c, mut s) = (0.0f64, 0.0f64);
        for (i, &x) in v.iter().enumerate() {
            let w = std::f64::consts::TAU * f64::from(f + df as f32) * i as f64 / 48_000.0;
            c += f64::from(x) * w.cos();
            s += f64::from(x) * w.sin();
        }
        e += c * c + s * s;
    }
    (e.sqrt() * 2.0 / v.len() as f64) as f32
}

fn render(kind: f32, switch_after: Option<usize>) -> Vec<f32> {
    let host = memory_host();
    host.set_plain("mix", 100.0);
    host.set_plain("reverb_type", if switch_after.is_some() { 0.0 } else { kind });
    let fs = 48_000.0;
    let mut engine = VerbEngine::new(fs, &settings(&host), &output(&host));
    let history: HistoryTap<{ record::WIDTH }> = HistoryTap::new(4096);
    let n = 4 * 48_000;
    let mut out = Vec::with_capacity(n);
    for b in 0..n / 480 {
        if switch_after == Some(b) {
            host.set_plain("reverb_type", kind);
        }
        let mut l: Vec<f32> = (0..480).map(|i| { let k = b * 480 + i; if k < 24_000 { 0.3 * (std::f32::consts::TAU * 440.0 * k as f32 / fs).sin() } else { 0.0 } }).collect();
        let mut r = l.clone();
        engine.process(&mut [&mut l, &mut r], &settings(&host), &output(&host), &history, None);
        out.extend_from_slice(&l);
    }
    out
}

#[test]
fn the_shimmer_type_puts_the_octave_in_the_tail() {
    for switch in [None, Some(10)] {
        let plate = render(2.0, switch);
        let shim = render(5.0, switch);
        let tail = 48_000..96_000;
        let (p, s) = (level(&plate[tail.clone()], 880.0), level(&shim[tail.clone()], 880.0));
        // Measured 2026-11-09: the octave 4 dB under the note with Shimmer, 38 dB under with Plate.
        assert!(s > 4.0 * p, "{switch:?}: the octave {s} against the plate's {p}");
    }
}

/// A short tail (the user's: DECAY 2 s, two decay bells at ×0.11, HIGH 2.2 kHz) still shimmers
/// at once: the input is shifted as it comes in, not only after passes round the tank (with the
/// passes alone, the octave was 21 dB under the note half a second on: "amount does nothing").
#[test]
fn a_short_tail_shimmers_at_once() {
    let host = memory_host();
    for (id, v) in [("mix", 100.0), ("reverb_type", 5.0), ("shimmer", 100.0), ("decay", 2.0), ("size", 30.0), ("mod_depth", 0.0), ("shimmer_high", 2230.0), ("d1_on", 1.0), ("d1_freq", 3184.0), ("d1_mult", 0.113), ("d2_on", 1.0), ("d2_freq", 190.0), ("d2_mult", 0.109)] {
        host.set_plain(id, v);
    }
    let fs = 48_000.0;
    let mut engine = VerbEngine::new(fs, &settings(&host), &output(&host));
    let history: HistoryTap<{ record::WIDTH }> = HistoryTap::new(4096);
    let mut out = Vec::new();
    for b in 0..200 {
        let mut l: Vec<f32> = (0..480).map(|i| { let k = b * 480 + i; if k < 48_000 { 0.3 * (std::f32::consts::TAU * 261.6 * k as f32 / fs).sin() } else { 0.0 } }).collect();
        let mut r = l.clone();
        engine.process(&mut [&mut l, &mut r], &settings(&host), &output(&host), &history, None);
        out.extend_from_slice(&l);
    }
    let tail = &out[48_000..72_000];
    let (note, octave) = (level(tail, 261.6), level(tail, 523.2));
    // Measured 2026-11-09: the octave 0.5 dB under the note (was 21 dB).
    assert!(octave > 0.5 * note, "the octave {octave} against the note {note}");
}
