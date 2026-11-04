//! Mite Delay's engine measured (docs/design/delay-brief.md §4): echoes on time and at their
//! level, the loop's gain against `loop_gain` (what the editor draws), ping-pong, stability at the
//! extremes, ducking, block-size invariance and click-free parameter moves.

use mite_dsp::delay::{echoes, loop_gain, DelayLine, DelaySettings, Mode, TapSettings};
use mite_dsp::eq::{BandSettings, BandType};

const FS: f32 = 48_000.0;

fn render(s: &DelaySettings, input: &[f32], block: usize) -> [Vec<f32>; 2] {
    let mut d = DelayLine::new(FS, s);
    let (mut l, mut r) = (vec![0.0f32; input.len()], vec![0.0f32; input.len()]);
    let mut pos = 0;
    while pos < input.len() {
        let end = (pos + block).min(input.len());
        d.process([&input[pos..end], &input[pos..end]], [&mut l[pos..end], &mut r[pos..end]], true, s);
        pos = end;
    }
    [l, r]
}

fn impulse(n: usize, a: f32) -> Vec<f32> {
    let mut x = vec![0.0f32; n];
    x[0] = a;
    x
}

fn one_tap(t: TapSettings) -> DelaySettings {
    let mut s = DelaySettings::default();
    s.taps[0] = TapSettings { on: true, ..t };
    s
}

/// `x`'s magnitude at `freq` (a DFT bin of the whole slice).
fn magnitude(x: &[f32], freq: f64) -> f64 {
    let w = std::f64::consts::TAU * freq / f64::from(FS);
    let (mut re, mut im) = (0.0, 0.0);
    for (i, &v) in x.iter().enumerate() {
        re += f64::from(v) * (w * i as f64).cos();
        im -= f64::from(v) * (w * i as f64).sin();
    }
    (re * re + im * im).sqrt()
}

#[test]
fn echoes_land_on_time_at_their_level() {
    for (time_ms, fb, level_db) in [(100.0, 0.5, 0.0), (10.0, 0.8, -6.0), (377.3, 0.3, 3.0)] {
        let s = one_tap(TapSettings { time_ms, feedback: fb, level_db, ..TapSettings::default() });
        let n = (time_ms * 0.001 * FS * 3.5) as usize;
        let [l, r] = render(&s, &impulse(n, 1.0), 512);
        let t = time_ms * 0.001 * FS;
        let level = 10f32.powf(level_db / 20.0);
        for k in 1..=3 {
            let want = level * fb.powi(k - 1);
            let at = t * k as f32;
            let i = at.round() as usize;
            // A fractional time spreads the echo over its neighbours (the interpolator's taps
            // sum to one).
            let peak: f32 = l[i - 3..=i + 3].iter().sum();
            assert!((peak - want).abs() < 0.02 * want.max(0.05), "{time_ms} ms echo {k}: {peak} (want {want})");
            assert_eq!(l[i - 2..=i + 2], r[i - 2..=i + 2]);
            // Nothing between the echoes.
            let gap = l[(at - 0.5 * t) as usize..(at - 4.0) as usize].iter().fold(0.0f32, |m, v| m.max(v.abs()));
            assert!(gap < 1e-4, "{time_ms} ms before echo {k}: {gap}");
        }
    }
}

#[test]
fn the_loop_gain_is_what_the_editor_draws() {
    // A quiet impulse (the machines saturate loud ones): echo 1 against the input at a few
    // frequencies, every mode, with a loop EQ band.
    for mode in Mode::ALL {
        let mut s = one_tap(TapSettings { time_ms: 500.0, feedback: 0.0, ..TapSettings::default() });
        s.mode = mode;
        s.eq[0] = BandSettings { enabled: true, kind: BandType::Bell, freq: 1000.0, gain_db: 6.0, q: 1.0, ..BandSettings::default() };
        s.eq[1] = BandSettings { enabled: true, kind: BandType::LowCut, freq: 150.0, slope: 1, ..BandSettings::default() };
        let n = 48_000;
        let [l, _] = render(&s, &impulse(n, 0.01), 512);
        let t = 24_000;
        let echo = &l[t - 48..t + 4800];
        for f in [100.0, 400.0, 1000.0, 3000.0, 8000.0] {
            let got = magnitude(echo, f) / 0.01;
            let want = loop_gain(&s, 0, f, f64::from(FS));
            let db = 20.0 * (got / want).log10();
            assert!(db.abs() < 0.5, "{mode:?} at {f} Hz: {got} against {want} ({db:+.2} dB)");
        }
    }
}

#[test]
fn echoes_follow_the_loop_gain() {
    let mut s = one_tap(TapSettings { time_ms: 50.0, feedback: 0.7, level_db: -3.0, pan: 0.4, ..TapSettings::default() });
    s.mode = Mode::Tape;
    let mut out = [[0.0f32; 2]; 4];
    echoes(&s, 0, 1000.0, f64::from(FS), &mut out);
    let h = loop_gain(&s, 0, 1000.0, f64::from(FS)) as f32;
    let level = 10f32.powf(-3.0 / 20.0);
    // Panned right of centre: the right louder; each echo the feedback × the loop under the last.
    assert!(out[0][1] > out[0][0]);
    for k in 1..4 {
        let ratio = out[k][0] / out[k - 1][0];
        assert!((ratio - 0.7 * h).abs() < 1e-4, "{ratio}");
    }
    assert!(((out[0][0] + out[0][1]) * 0.5 - level * h).abs() < 0.05 * level);
}

#[test]
fn ping_pong_alternates_sides() {
    let s = one_tap(TapSettings { time_ms: 100.0, feedback: 0.6, pingpong: true, ..TapSettings::default() });
    let [l, r] = render(&s, &impulse(48_000, 1.0), 256);
    let t = 4800;
    let e = |v: &[f32], k: usize| v[k * t - 2..=k * t + 2].iter().fold(0.0f32, |m, x| m.max(x.abs()));
    assert!(e(&l, 1) > 0.9 && e(&r, 1) < 1e-5, "first: {} {}", e(&l, 1), e(&r, 1));
    assert!(e(&r, 2) > 0.5 && e(&l, 2) < 1e-5, "second: {} {}", e(&l, 2), e(&r, 2));
    assert!(e(&l, 3) > 0.3 && e(&r, 3) < 1e-5);
    // And `echoes` says the same.
    let mut out = [[0.0f32; 2]; 3];
    echoes(&s, 0, 1000.0, f64::from(FS), &mut out);
    assert!(out[0][1] < 1e-6 && out[1][0] < 1e-6 && out[2][1] < 1e-6);
}

#[test]
fn no_setting_blows_up() {
    // Full feedback through a +24 dB loop boost and full drive, every mode, ping-pong and not:
    // the loop's safety holds it.
    let mut seed = 7u32;
    let noise: Vec<f32> = (0..FS as usize * 6)
        .map(|_| {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            seed as f32 / u32::MAX as f32 - 0.5
        })
        .collect();
    for mode in Mode::ALL {
        for pingpong in [false, true] {
            let mut s = one_tap(TapSettings { time_ms: 10.0, feedback: 1.0, level_db: 6.0, pingpong, ..TapSettings::default() });
            s.taps[1] = TapSettings { on: true, time_ms: 37.0, feedback: 1.0, ..TapSettings::default() };
            s.mode = mode;
            s.diffusion = 1.0;
            s.mod_depth = 1.0;
            s.eq[0] = BandSettings { enabled: true, kind: BandType::Bell, freq: 1000.0, gain_db: 24.0, q: 0.5, ..BandSettings::default() };
            let [l, r] = render(&s, &noise, 512);
            let peak = l.iter().chain(&r).fold(0.0f32, |m, v| m.max(v.abs()));
            assert!(peak.is_finite() && peak < 12.0, "{mode:?} pingpong {pingpong}: {peak}");
        }
    }
}

#[test]
fn ducking_turns_the_echoes_down_while_the_input_plays() {
    let tone: Vec<f32> = (0..96_000).map(|i| 0.5 * (i as f32 * 0.05).sin()).collect();
    let mut s = one_tap(TapSettings { time_ms: 100.0, feedback: 0.5, ..TapSettings::default() });
    let [plain, _] = render(&s, &tone, 512);
    s.duck_db = 12.0;
    s.duck_thresh_db = -40.0;
    let [ducked, _] = render(&s, &tone, 512);
    let rms = |v: &[f32]| (v.iter().map(|x| x * x).sum::<f32>() / v.len() as f32).sqrt();
    let db = 20.0 * (rms(&ducked[48_000..]) / rms(&plain[48_000..])).log10();
    assert!((db + 12.0).abs() < 0.5, "{db}");
}

#[test]
fn the_output_is_the_same_for_any_block_size() {
    let mut s = one_tap(TapSettings { time_ms: 123.4, feedback: 0.6, pan: -0.3, ..TapSettings::default() });
    s.taps[1] = TapSettings { on: true, time_ms: 250.0, feedback: 0.4, pingpong: true, ..TapSettings::default() };
    s.mode = Mode::Tape;
    s.mod_depth = 0.5;
    s.diffusion = 0.4;
    s.drive = 0.3;
    s.eq[0] = BandSettings { enabled: true, kind: BandType::HighCut, freq: 4000.0, ..BandSettings::default() };
    let x: Vec<f32> = (0..30_000).map(|i| 0.3 * (i as f32 * 0.031).sin() * if i < 6000 { 1.0 } else { 0.0 }).collect();
    let a = render(&s, &x, 512);
    for block in [1, 7, 64, 333] {
        assert_eq!(a, render(&s, &x, block), "block {block}");
    }
}

#[test]
fn moving_any_setting_fast_never_clicks() {
    // A low tone in, then the setting jumps: the output's second difference stays near what the
    // echoes of that tone have anyway.
    let n = 96_000;
    let x: Vec<f32> = (0..n).map(|i| 0.4 * (std::f32::consts::TAU * 150.0 * i as f32 / FS).sin()).collect();
    let mut base = one_tap(TapSettings { time_ms: 200.0, feedback: 0.5, ..TapSettings::default() });
    base.taps[1] = TapSettings { on: true, time_ms: 300.0, feedback: 0.3, pan: 0.5, ..TapSettings::default() };
    let with = |f: &dyn Fn(&mut DelaySettings)| {
        let mut s = base;
        f(&mut s);
        s
    };
    let cases: [(&str, DelaySettings); 14] = [
        ("time digital", with(&|s| s.taps[0].time_ms = 330.0)),
        ("time tape", with(&|s| {
            s.mode = Mode::Tape;
            s.taps[0].time_ms = 330.0;
        })),
        ("level", with(&|s| s.taps[0].level_db = -24.0)),
        ("feedback", with(&|s| s.taps[0].feedback = 0.95)),
        ("pan", with(&|s| s.taps[0].pan = -1.0)),
        ("pingpong", with(&|s| s.taps[0].pingpong = true)),
        ("tap on", with(&|s| s.taps[2] = TapSettings { on: true, time_ms: 150.0, ..TapSettings::default() })),
        ("tap off", with(&|s| s.taps[1].on = false)),
        ("mode", with(&|s| s.mode = Mode::Bbd)),
        ("drive", with(&|s| s.drive = 1.0)),
        ("diffusion", with(&|s| s.diffusion = 1.0)),
        ("mod", with(&|s| s.mod_depth = 1.0)),
        ("eq band", with(&|s| s.eq[0] = BandSettings { enabled: true, kind: BandType::LowCut, freq: 400.0, ..BandSettings::default() })),
        ("width", with(&|s| s.width = 0.0)),
    ];
    let d2 = |v: &[f32], i: usize| (v[i + 1] - 2.0 * v[i] + v[i - 1]).abs();
    let render_switch = |switch: bool, after: &DelaySettings| {
        let mut d = DelayLine::new(FS, &base);
        let [mut l, mut r] = [vec![0.0f32; n], vec![0.0f32; n]];
        for (k, (a, b)) in l.chunks_mut(480).zip(r.chunks_mut(480)).enumerate() {
            let s = if !switch || k * 480 >= 48_000 { after } else { &base };
            let p = k * 480;
            d.process([&x[p..p + a.len()], &x[p..p + a.len()]], [a, b], true, s);
        }
        [l, r]
    };
    let at = 48_000;
    let window = |v: &[Vec<f32>; 2]| (at..at + 24_000).map(|i| d2(&v[0], i).max(d2(&v[1], i))).fold(0.0, f32::max);
    let steady = window(&render_switch(false, &base));
    for (name, after) in cases {
        let held = window(&render_switch(false, &after));
        let moving = window(&render_switch(true, &after));
        assert!(moving <= steady.max(held) * 4.0 + 1e-5, "{name}: {moving} after {steady} (held {held})");
    }
}

#[test]
fn tape_glides_to_a_new_time_and_digital_crossfades() {
    let s = one_tap(TapSettings { time_ms: 100.0, ..TapSettings::default() });
    let moved = one_tap(TapSettings { time_ms: 400.0, ..TapSettings::default() });
    let x = vec![0.0f32; 4800];
    for (mode, glides) in [(Mode::Tape, true), (Mode::Digital, false)] {
        let (a, b) = (DelaySettings { mode, ..s }, DelaySettings { mode, ..moved });
        let mut d = DelayLine::new(FS, &a);
        let (mut l, mut r) = (vec![0.0f32; 4800], vec![0.0f32; 4800]);
        d.process([&x, &x], [&mut l, &mut r], true, &a);
        d.process([&x[..480], &x[..480]], [&mut l[..480], &mut r[..480]], true, &b);
        let t = d.time_ms(0);
        if glides {
            assert!(t > 100.0 && t < 400.0, "{t}");
        } else {
            // Mid-fade the first head still reads the old time.
            assert!((t - 100.0).abs() < 1e-3, "{t}");
        }
        for _ in 0..40 {
            d.process([&x, &x], [&mut l, &mut r], true, &b);
        }
        assert!((d.time_ms(0) - 400.0).abs() < 0.01, "{mode:?}: {}", d.time_ms(0));
    }
}
