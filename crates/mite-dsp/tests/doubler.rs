//! Mite Double's engine measured (docs/design/double-brief.md §4): the source untouched at its
//! place, a new reading at every note within the set spread, each attack arriving whole (the
//! switch done before it), the detune heard as the pitch, block-size invariance, click-free moves.

use mite_dsp::doubler::{auto_gain_db, doubles_trim_db, pan_gains, Doubler, DoublerSettings, VoiceSettings};
use mite_dsp::pitch::{window_len, PitchDetector};

const FS: f32 = 48_000.0;
const NOTE: usize = 24_000;

/// Plucked notes every half second (each a different pitch, harmonics decaying), mono.
fn notes(count: usize) -> (Vec<f32>, Vec<f64>) {
    let hz: Vec<f64> = (0..count).map(|k| 110.0 * 2f64.powf(f64::from([0, 5, 7, 3, 10, 2, 8, 12][k % 8]) / 12.0)).collect();
    let mut x = vec![0.0f32; count * NOTE];
    for (k, &f) in hz.iter().enumerate() {
        for i in 0..NOTE {
            let t = i as f64 / f64::from(FS);
            let env = (1.0 - (-t / 0.001).exp()) * (-t / 0.35).exp();
            let v: f64 = (1..8).map(|h| (std::f64::consts::TAU * f * f64::from(h) * t).sin() / f64::from(h) * (-t * f64::from(h) * 0.8).exp()).sum();
            x[k * NOTE + i] = (0.4 * env * v) as f32;
        }
    }
    (x, hz)
}

fn render(s: &DoublerSettings, x: &[f32], block: usize) -> ([Vec<f32>; 2], Vec<[f32; 3]>) {
    let mut d = Doubler::new(FS);
    let (mut l, mut r) = (vec![0.0f32; x.len()], vec![0.0f32; x.len()]);
    let mut offsets = Vec::new();
    let mut pos = 0;
    while pos < x.len() {
        let end = (pos + block).min(x.len());
        let tel = d.process([&x[pos..end], &x[pos..end]], [&mut l[pos..end], &mut r[pos..end]], false, s);
        if tel.onset {
            offsets.push(d.notes().map(|n| n.offset_ms));
        }
        pos = end;
    }
    ([l, r], offsets)
}

/// The source hard left, one double hard right: the right is the double alone (its lows its own:
/// these tests look at the double's reading).
fn split(s: DoublerSettings) -> DoublerSettings {
    let mut s = s;
    s.source_pan = -1.0;
    s.voice[0].pan = 1.0;
    s.mono_bass = false;
    s
}

#[test]
fn the_source_alone_is_the_input_at_its_place() {
    let (x, _) = notes(2);
    for pan in [-1.0, 0.0, 0.4] {
        let s = DoublerSettings { voices: 0, source_pan: pan, source_db: -6.0, mono_bass: false, auto_gain: false, ..DoublerSettings::default() };
        let ([l, r], _) = render(&s, &x, 512);
        let (gl, gr) = pan_gains(pan);
        let g = mite_dsp::db_to_gain(-6.0);
        for i in 0..x.len() {
            assert!((l[i] - gl * g * x[i]).abs() < 1e-6 && (r[i] - gr * g * x[i]).abs() < 1e-6, "pan {pan} at {i}");
        }
    }
}

#[test]
fn every_note_gets_a_new_reading_within_the_spread() {
    let (x, _) = notes(8);
    let s = split(DoublerSettings { timing_ms: 20.0, ..DoublerSettings::default() });
    let (_, offsets) = render(&s, &x, 480);
    assert_eq!(offsets.len(), 8, "one reading a note");
    let first: Vec<f32> = offsets.iter().map(|o| o[0]).collect();
    assert!(first.iter().all(|o| o.abs() <= 10.0), "{first:?}");
    // They differ, note to note, as a player's do.
    assert!(first.windows(2).all(|w| (w[0] - w[1]).abs() > 1e-3), "{first:?}");
    let spread = first.iter().fold(f32::MIN, |a, &b| a.max(b)) - first.iter().fold(f32::MAX, |a, &b| a.min(b));
    assert!(spread > 4.0, "{first:?}");
}

#[test]
#[allow(clippy::needless_range_loop)] // notes by index, as the input lays them out
fn each_attack_arrives_whole_at_its_note_time() {
    // No detune, chorus or variation: the double is the input, each note at its own lag.
    let (x, _) = notes(6);
    let s = split(DoublerSettings { pitch_cents: 0.0, chorus: 0.0, variation: 0.0, timing_ms: 20.0, delay_ms: 20.0, ..DoublerSettings::default() });
    let ([_, r], offsets) = render(&s, &x, 480);
    let mut lags = Vec::new();
    for k in 1..6 {
        let at = k * NOTE;
        // The lag that best matches the note's first 40 ms.
        let seg = &x[at..at + 1920];
        let (mut best, mut lag) = (f64::MIN, 0);
        for d in (8 * 48)..(31 * 48) {
            let c: f64 = seg.iter().zip(&r[at + d..]).map(|(a, b)| f64::from(*a) * f64::from(*b)).sum();
            if c > best {
                best = c;
                lag = d;
            }
        }
        let e: f64 = seg.iter().map(|a| f64::from(*a).powi(2)).sum();
        let e2: f64 = r[at + lag..at + lag + 1920].iter().map(|a| f64::from(*a).powi(2)).sum();
        let rho = best / (e * e2).sqrt();
        let want = 20.0 + offsets[k][0];
        let got = lag as f32 / 48.0;
        assert!(rho > 0.999, "note {k}: correlation {rho}");
        assert!((got - want).abs() < 0.05, "note {k}: {got} ms, want {want}");
        lags.push(lag);
    }
    lags.dedup();
    assert!(lags.len() >= 4, "{lags:?}");
}

#[test]
fn the_detune_is_heard_as_the_pitch() {
    let (x, hz) = notes(6);
    let s = split(DoublerSettings { pitch_cents: 15.0, chorus: 0.0, variation: 0.0, ..DoublerSettings::default() });
    let mut d = Doubler::new(FS);
    let (mut l, mut r) = (vec![0.0f32; x.len()], vec![0.0f32; x.len()]);
    let mut cents = Vec::new();
    for (k, (a, b)) in l.chunks_mut(480).zip(r.chunks_mut(480)).enumerate() {
        let p = k * 480;
        if d.process([&x[p..p + a.len()], &x[p..p + a.len()]], [a, b], false, &s).onset {
            cents.push(d.notes()[0].cents);
        }
    }
    let mut det = PitchDetector::new(window_len(f64::from(FS)) / 2);
    let mut heard = 0;
    for k in 1..6 {
        // 30…130 ms into the double's note: the detune at work, well before it eases off.
        let at = k * NOTE + 1440 + 4800;
        let p = det.detect(&r[at - det.len()..at], f64::from(FS)).expect("a pitch");
        let got = 1200.0 * (p.hz / hz[k]).log2();
        let want = f64::from(cents[k]);
        assert!(want.abs() <= 15.0);
        assert!((got - want).abs() < 1.5, "note {k}: {got:+.2} cents, want {want:+.2}");
        heard += usize::from(want.abs() > 2.0);
    }
    assert!(heard >= 2);
}

#[test]
fn without_notes_the_readings_move_every_few_seconds() {
    let x: Vec<f32> = (0..FS as usize * 8).map(|i| 0.3 * (i as f32 * 0.02).sin()).collect();
    let s = split(DoublerSettings { follow_notes: false, ..DoublerSettings::default() });
    let mut d = Doubler::new(FS);
    let (mut l, mut r) = (vec![0.0f32; x.len()], vec![0.0f32; x.len()]);
    let mut changes = 0;
    let mut last = None;
    for (k, (a, b)) in l.chunks_mut(480).zip(r.chunks_mut(480)).enumerate() {
        let p = k * 480;
        d.process([&x[p..p + a.len()], &x[p..p + a.len()]], [a, b], false, &s);
        let now = d.notes()[0];
        if last.is_some_and(|n| n != now) {
            changes += 1;
        }
        last = Some(now);
    }
    assert!((3..=6).contains(&changes), "{changes}");
}

#[test]
fn the_output_is_the_same_for_any_block_size() {
    let (x, _) = notes(4);
    let s = DoublerSettings { voices: 3, chorus: 0.5, tone_db: 3.0, ..DoublerSettings::default() };
    let (a, _) = render(&s, &x, 512);
    for block in [1, 7, 64, 333] {
        assert_eq!(a, render(&s, &x, block).0, "block {block}");
    }
}

#[test]
fn moving_any_setting_fast_never_clicks() {
    let n = 96_000;
    let x: Vec<f32> = (0..n).map(|i| 0.4 * (std::f32::consts::TAU * 150.0 * i as f32 / FS).sin()).collect();
    let base = DoublerSettings { voices: 2, follow_notes: false, ..DoublerSettings::default() };
    let with = |f: &dyn Fn(&mut DoublerSettings)| {
        let mut s = base;
        f(&mut s);
        s
    };
    let cases: [(&str, DoublerSettings); 12] = [
        ("mono bass off", with(&|s| s.mono_bass = false)),
        ("bass frequency", with(&|s| s.bass_hz = 300.0)),
        ("auto gain off", with(&|s| s.auto_gain = false)),
        ("source pan", with(&|s| s.source_pan = 1.0)),
        ("source level", with(&|s| s.source_db = -24.0)),
        ("voice pan", with(&|s| s.voice[0].pan = -1.0)),
        ("voice level", with(&|s| s.voice[0].level_db = -24.0)),
        ("voices", with(&|s| s.voices = 3)),
        ("voices off", with(&|s| s.voices = 0)),
        ("tone", with(&|s| s.tone_db = 6.0)),
        ("chorus", with(&|s| s.chorus = 1.0)),
        ("delay", with(&|s| s.delay_ms = 40.0)),
    ];
    let d2 = |v: &[f32], i: usize| (v[i + 1] - 2.0 * v[i] + v[i - 1]).abs();
    let run = |switch: bool, after: &DoublerSettings| {
        let mut d = Doubler::new(FS);
        let [mut l, mut r] = [vec![0.0f32; n], vec![0.0f32; n]];
        for (k, (a, b)) in l.chunks_mut(480).zip(r.chunks_mut(480)).enumerate() {
            let s = if !switch || k * 480 >= 48_000 { after } else { &base };
            let p = k * 480;
            d.process([&x[p..p + a.len()], &x[p..p + a.len()]], [a, b], false, s);
        }
        [l, r]
    };
    let window = |v: &[Vec<f32>; 2]| (48_000..72_000).map(|i| d2(&v[0], i).max(d2(&v[1], i))).fold(0.0, f32::max);
    let steady = window(&run(false, &base));
    for (name, after) in cases {
        let held = window(&run(false, &after));
        let moving = window(&run(true, &after));
        assert!(moving <= steady.max(held) * 4.0 + 1e-5, "{name}: {moving} after {steady} (held {held})");
    }
}

fn noise(seconds: f32) -> Vec<f32> {
    let mut seed = 3u32;
    (0..(FS * seconds) as usize)
        .map(|_| {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            0.3 * (seed as f32 / u32::MAX as f32 - 0.5)
        })
        .collect()
}

fn power(v: &[f32]) -> f64 {
    v.iter().map(|s| f64::from(*s).powi(2)).sum::<f64>() / v.len() as f64
}

#[test]
fn auto_gain_keeps_it_as_loud_as_the_input_and_more_takes_add_no_level() {
    // Noise in (both sides in bypass: twice its power), the output as it comes, many placements
    // and counts: all within 0.6 dB of the input.
    let x = noise(4.0);
    let reference = 2.0 * power(&x[24_000..]);
    let centre = VoiceSettings { pan: 0.0, level_db: 0.0 };
    for s in [
        DoublerSettings::default(),
        DoublerSettings { voices: 2, ..DoublerSettings::default() },
        DoublerSettings { voices: 3, ..DoublerSettings::default() },
        DoublerSettings { voices: 3, voice: [centre, VoiceSettings { pan: -1.0, level_db: 4.0 }, VoiceSettings { pan: 1.0, level_db: 0.0 }], ..DoublerSettings::default() },
        DoublerSettings { voices: 2, source_pan: 0.0, voice: [VoiceSettings { pan: -0.6, level_db: 0.0 }, VoiceSettings { pan: 0.6, level_db: 0.0 }, centre], ..DoublerSettings::default() },
        DoublerSettings { source_db: -6.0, voice: [VoiceSettings { pan: 1.0, level_db: -3.0 }, centre, centre], ..DoublerSettings::default() },
    ] {
        let ([l, r], _) = render(&s, &x, 512);
        let db = 10.0 * ((power(&l[24_000..]) + power(&r[24_000..])) / reference).log10();
        assert!(db.abs() < 0.6, "{} doubles, source at {}: {db:+.2} dB", s.voices, s.source_pan);
    }
    // The source keeps its level whatever the count: the trim is the doubles'.
    assert_eq!(auto_gain_db(&DoublerSettings::default()), auto_gain_db(&DoublerSettings { voices: 3, ..DoublerSettings::default() }));
    assert!((doubles_trim_db(&DoublerSettings { voices: 3, ..DoublerSettings::default() }) + 3.01).abs() < 0.01);
}

#[test]
fn mono_bass_keeps_the_low_end_whole_in_mono() {
    // Plucked notes (110…220 Hz fundamentals), the source hard left and a double hard right, the
    // split at 300 Hz: summed to mono, the octave about 125 Hz never falls under the two sides'
    // sum (the lows are in phase), where the double's own lows cancel by several dB at moments.
    let (x, _) = notes(10);
    let worst = |mono_bass: bool| {
        let s = DoublerSettings { mono_bass, bass_hz: 300.0, ..DoublerSettings::default() };
        let ([l, r], _) = render(&s, &x, 512);
        let (bl, br) = (octave(&l, 125.0), octave(&r, 125.0));
        (24_000..x.len() - 2400)
            .step_by(2400)
            .filter_map(|i| {
                let (mut m, mut st) = (0.0, 0.0);
                for k in i..i + 2400 {
                    m += (bl[k] + br[k]).powi(2);
                    st += bl[k].powi(2) + br[k].powi(2);
                }
                (st > 1e-9).then(|| 10.0 * (m / st).log10())
            })
            .fold(f64::MAX, f64::min)
    };
    let (off, on) = (worst(false), worst(true));
    assert!(off < -2.0, "without it the lows cancel at moments: {off:+.1} dB");
    assert!(on > -0.5, "with it they never do: {on:+.1} dB");
    // And the side the source sits on rebuilds it whole (its highs and the lows: an allpass).
    let n = noise(2.0);
    let s = DoublerSettings { voices: 0, auto_gain: false, ..DoublerSettings::default() };
    let ([l, _], _) = render(&s, &n, 512);
    let db = 10.0 * (power(&l[24_000..]) / power(&n[24_000..])).log10();
    assert!(db.abs() < 0.1, "{db}");
}

/// An octave band (two RBJ band-passes) of `x` about `fc`.
fn octave(x: &[f32], fc: f64) -> Vec<f64> {
    let w = std::f64::consts::TAU * fc / f64::from(FS);
    let alpha = w.sin() / (2.0 * std::f64::consts::SQRT_2);
    let a0 = 1.0 + alpha;
    let (b0, b2, a1, a2) = (alpha / a0, -alpha / a0, -2.0 * w.cos() / a0, (1.0 - alpha) / a0);
    let mut y: Vec<f64> = x.iter().map(|&v| f64::from(v)).collect();
    for _ in 0..2 {
        let (mut x1, mut x2, mut y1, mut y2) = (0.0, 0.0, 0.0, 0.0);
        for v in y.iter_mut() {
            let x0 = *v;
            let out = b0 * x0 + b2 * x2 - a1 * y1 - a2 * y2;
            x2 = x1;
            x1 = x0;
            y2 = y1;
            y1 = out;
            *v = out;
        }
    }
    y
}
