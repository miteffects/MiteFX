//! Mite Mod's engine measured (docs/design/mod-brief.md §4): smooth shapes, mix 0 is the input,
//! the latency is where the sound is, the flanger passes through zero, the drawn responses are
//! the ones heard, vibrato's cents, block sizes, clicks.

use mite_dsp::modulation::*;

const FS: f32 = 48_000.0;
const NO_TRANSPORT: Transport = Transport { tempo: None, beats: None, playing: false };

fn render(s: &ModSettings, x: &[f32], block: usize) -> ([Vec<f32>; 2], Vec<ModTelemetry>) {
    let mut m = Mod::new(FS, s);
    let (mut l, mut r) = (vec![0.0f32; x.len()], vec![0.0f32; x.len()]);
    let mut tel = Vec::new();
    for ((a, b), c) in l.chunks_mut(block).zip(r.chunks_mut(block)).zip(x.chunks(block)) {
        tel.push(m.process([c, c], [a, b], false, s, &NO_TRANSPORT));
    }
    ([l, r], tel)
}

fn noise(n: usize, seed: u32) -> Vec<f32> {
    let mut s = seed;
    (0..n)
        .map(|_| {
            s ^= s << 13;
            s ^= s >> 17;
            s ^= s << 5;
            (s as f32 / u32::MAX as f32 - 0.5) * 0.5
        })
        .collect()
}

fn modes() -> Vec<(&'static str, ModSettings)> {
    let d = ModSettings::default();
    vec![
        ("chorus", ModSettings { mode: Mode::Chorus, voices: 3, ..d }),
        ("ensemble", ModSettings { mode: Mode::Ensemble, ..d }),
        ("flanger", ModSettings { mode: Mode::Flanger, feedback: 0.6, ..d }),
        ("flanger tz", ModSettings { mode: Mode::Flanger, thru_zero: true, feedback: 0.3, ..d }),
        ("phaser", ModSettings { mode: Mode::Phaser, feedback: 0.5, stages: 8, ..d }),
        ("vibrato", ModSettings { mode: Mode::Vibrato, rate_hz: 5.0, ..d }),
    ]
}

#[test]
fn every_shape_is_continuous_and_in_range() {
    for shape in Shape::ALL {
        let n = 20_000;
        let mut last = shape_at(shape, -0.5);
        for i in 0..=n {
            let p = -0.5 + 3.0 * i as f64 / n as f64;
            let v = shape_at(shape, p);
            assert!((-1.0001..=1.0001).contains(&v), "{shape:?} at {p}: {v}");
            // A step of 1/6667 cycle moves it at most as far as a half-cosine fall over a tenth
            // of a cycle does (the ramps' return, the steepest).
            assert!((v - last).abs() < 0.005, "{shape:?} jumps at {p}: {last} → {v}");
            last = v;
        }
    }
    assert_eq!(shape_at(Shape::Random, 7.3), shape_at(Shape::Random, 7.3));
    assert_ne!(shape_at(Shape::Random, 7.0), shape_at(Shape::Random, 8.0));
}

#[test]
fn mix_zero_is_the_input_at_the_latency() {
    let x = noise(24_000, 3);
    for (name, s) in modes() {
        if s.mode == Mode::Vibrato {
            continue;
        }
        let s = ModSettings { mix: 0.0, ..s };
        let lat = latency_for(&s, FS);
        let ([l, r], _) = render(&s, &x, 256);
        assert_eq!(&l[lat..], &x[..x.len() - lat], "{name}");
        assert_eq!(&r[lat..], &x[..x.len() - lat], "{name}");
    }
}

#[test]
fn the_latency_is_where_the_sound_is() {
    // An impulse: through-zero's dry and vibrato's voice (depth 0) peak at the reported latency.
    let mut x = vec![0.0f32; 4800];
    x[100] = 1.0;
    for s in [
        ModSettings { mode: Mode::Flanger, thru_zero: true, mix: 0.0, ..ModSettings::default() },
        ModSettings { mode: Mode::Vibrato, depth: 0.0, ..ModSettings::default() },
    ] {
        let lat = latency_for(&s, FS);
        assert_eq!(Mod::new(FS, &s).latency(), lat);
        let ([l, _], _) = render(&s, &x, 128);
        let peak = l.iter().enumerate().max_by(|a, b| a.1.abs().total_cmp(&b.1.abs())).map(|(i, _)| i).expect("peak");
        assert_eq!(peak, 100 + lat, "{:?}", s.mode);
    }
    assert_eq!(latency_for(&ModSettings::default(), FS), 0);
}

#[test]
fn through_zero_the_wet_meets_the_dry_and_cancels_or_adds() {
    // At zero delay the wet is the dry: their sum is the input (up to the wet's tone: the low
    // cut's phase at 100 Hz, the high cut's at 9 kHz).
    let s = ModSettings { mode: Mode::Flanger, thru_zero: true, mix: 0.5, feedback: 0.0, ..ModSettings::default() };
    for f in [100.0, 1000.0, 9000.0] {
        let db = flanger_response(&s, 0.0, f, f64::from(FS));
        assert!(db.abs() < 0.3, "{f} Hz: {db:.2} dB");
    }
    // Either side of zero the comb is the same (a delay of −τ notches where +τ does).
    let a = flanger_response(&s, 0.5, 1000.0, f64::from(FS));
    let b = flanger_response(&s, -0.5, 1000.0, f64::from(FS));
    assert!(a < -40.0 && b < -40.0, "the first notch of 0.5 ms is at 1 kHz: {a:.1}, {b:.1}");
    // The sweep crosses zero at depth 50 % and above, never below.
    let deep = ModSettings { depth: 0.6, ..s };
    assert!(flanger_tau_ms(&deep, 1.0) < 0.0 && flanger_tau_ms(&deep, -1.0) > 0.0);
    let shallow = ModSettings { depth: 0.4, ..s };
    assert!(flanger_tau_ms(&shallow, 1.0) > 0.0);
}

/// The output's level at `f` Hz against the input's, dB, over the last part of `x`.
fn gain_at(s: &ModSettings, f: f32) -> (f64, ModTelemetry) {
    let n = 24_000;
    let x: Vec<f32> = (0..n).map(|i| 0.5 * (std::f32::consts::TAU * f * i as f32 / FS).sin()).collect();
    let ([l, _], tel) = render(s, &x, n);
    let tail = &l[n - 4800..];
    let p: f64 = tail.iter().map(|v| f64::from(*v).powi(2)).sum::<f64>() / tail.len() as f64;
    (10.0 * (p / 0.125).log10(), tel[tel.len() - 1])
}

#[test]
fn the_drawn_responses_are_the_ones_heard() {
    // The LFO standing still (depth 0): the flanger's comb and the phaser's notches measured with
    // tones match what the editor draws from the same settings.
    let fsd = f64::from(FS);
    let flanger = ModSettings { mode: Mode::Flanger, depth: 0.0, manual_ms: 1.0, feedback: 0.5, ..ModSettings::default() };
    let phaser = ModSettings { mode: Mode::Phaser, depth: 0.0, centre_hz: 700.0, stages: 6, feedback: 0.4, ..ModSettings::default() };
    for f in [200.0, 700.0, 1300.0, 2500.0, 6000.0] {
        let (heard, tel) = gain_at(&flanger, f);
        let drawn = flanger_response(&flanger, tel.values[0], f64::from(f), fsd);
        assert!((heard - drawn).abs() < 0.3 || (heard < -25.0 && drawn < -25.0), "flanger at {f}: heard {heard:.2}, drawn {drawn:.2}");
        let (heard, tel) = gain_at(&phaser, f);
        let drawn = phaser_response(&phaser, tel.values[0], f64::from(f), fsd);
        assert!((heard - drawn).abs() < 0.3 || (heard < -25.0 && drawn < -25.0), "phaser at {f}: heard {heard:.2}, drawn {drawn:.2}");
    }
}

#[test]
fn vibrato_plays_its_cents() {
    // 50 % at 5 Hz: the pitch swings ±50 cents (the sine's steepest), the readings say so.
    let s = ModSettings { mode: Mode::Vibrato, rate_hz: 5.0, depth: 0.5, ..ModSettings::default() };
    let x = noise(48_000, 9);
    let (_, tel) = render(&s, &x, 16);
    let top = tel[tel.len() / 2..].iter().map(|t| t.values[0].abs()).fold(0.0, f32::max);
    assert!((top - 50.0).abs() < 2.0, "{top} cents");
    assert!((vibrato_amp_ms(&s, 5.0) - 0.9334).abs() < 0.01);
}

#[test]
fn the_output_is_the_same_for_any_block_size() {
    let x = noise(20_000, 5);
    for (name, s) in modes() {
        let (a, _) = render(&s, &x, 512);
        for block in [1, 7, 333] {
            assert_eq!(a, render(&s, &x, block).0, "{name}, block {block}");
        }
    }
}

#[test]
fn moving_any_setting_never_clicks() {
    let n = 96_000;
    let x: Vec<f32> = (0..n).map(|i| 0.4 * (std::f32::consts::TAU * 220.0 * i as f32 / FS).sin()).collect();
    let d2 = |v: &[f32], i: usize| (v[i + 1] - 2.0 * v[i] + v[i - 1]).abs();
    for (name, base) in modes() {
        let with = |f: &dyn Fn(&mut ModSettings)| {
            let mut s = base;
            f(&mut s);
            s
        };
        let cases: Vec<(&str, ModSettings)> = vec![
            ("depth", with(&|s| s.depth = 1.0)),
            ("spread", with(&|s| s.spread = 0.0)),
            ("feedback", with(&|s| s.feedback = -0.8)),
            ("mix", with(&|s| s.mix = 1.0)),
            ("width", with(&|s| s.width = 0.0)),
            ("tone", with(&|s| {
                s.low_cut_hz = 400.0;
                s.high_cut_hz = 3000.0;
            })),
            ("delay", with(&|s| s.delay_ms = 30.0)),
            ("voices", with(&|s| s.voices = 4)),
            ("manual", with(&|s| s.manual_ms = 4.0)),
            ("centre", with(&|s| s.centre_hz = 3000.0)),
            ("stages", with(&|s| s.stages = 24)),
            ("shape", with(&|s| s.shape = Shape::Square)),
            ("rate", with(&|s| s.rate_hz = 3.0)),
            ("mode", with(&|s| s.mode = if s.mode == Mode::Phaser { Mode::Chorus } else { Mode::Phaser })),
            ("through zero", with(&|s| {
                s.mode = Mode::Flanger;
                s.thru_zero = !s.thru_zero;
            })),
        ];
        let run = |switch: bool, after: &ModSettings| {
            let mut m = Mod::new(FS, &base);
            let [mut l, mut r] = [vec![0.0f32; n], vec![0.0f32; n]];
            for (k, (a, b)) in l.chunks_mut(480).zip(r.chunks_mut(480)).enumerate() {
                let s = if !switch || k * 480 >= 48_000 { after } else { &base };
                let p = k * 480;
                m.process([&x[p..p + a.len()], &x[p..p + a.len()]], [a, b], false, s, &NO_TRANSPORT);
            }
            [l, r]
        };
        let window = |v: &[Vec<f32>; 2]| (47_000..60_000).map(|i| d2(&v[0], i).max(d2(&v[1], i))).fold(0.0, f32::max);
        let steady = window(&run(false, &base));
        for (what, after) in cases {
            let held = window(&run(false, &after));
            let moving = window(&run(true, &after));
            assert!(moving <= steady.max(held) * 4.0 + 1e-4, "{name}, {what}: {moving} (steady {steady}, held {held})");
        }
    }
}

#[test]
fn a_synced_lfo_locks_to_the_song_position() {
    // 120 BPM, a cycle per bar: playing from beat 6 the phase settles on beats / 4.
    let s = ModSettings { sync: true, division: 2, ..ModSettings::default() };
    let mut m = Mod::new(FS, &s);
    let x = vec![0.0f32; 512];
    let (mut l, mut r) = (vec![0.0f32; 512], vec![0.0f32; 512]);
    let mut beats = 6.0;
    let mut tel = ModTelemetry::default();
    for _ in 0..400 {
        let t = Transport { tempo: Some(120.0), beats: Some(beats), playing: true };
        tel = m.process([&x, &x], [&mut l, &mut r], false, &s, &t);
        beats += 512.0 / FS as f64 * 2.0;
    }
    assert!((tel.rate - 0.5).abs() < 1e-9);
    let err = ((beats / 4.0) - tel.phase + 0.5).rem_euclid(1.0) - 0.5;
    assert!(err.abs() < 0.01, "{err}");
}
