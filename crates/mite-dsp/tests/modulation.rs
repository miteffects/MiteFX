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
        ("rotary", ModSettings { mode: Mode::Rotary, rotor: Rotor::Fast, depth: 0.8, ..d }),
        ("micro", ModSettings { mode: Mode::Micro, detune_cents: 12.0, ..d }),
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
        ModSettings { mode: Mode::Rotary, depth: 0.0, mix: 1.0, ..ModSettings::default() },
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
            ("rotor fast", with(&|s| s.rotor = Rotor::Fast)),
            ("rotor brake", with(&|s| s.rotor = Rotor::Brake)),
            ("balance", with(&|s| s.balance = 0.9)),
            ("inertia", with(&|s| s.inertia = 0.0)),
            ("detune", with(&|s| s.detune_cents = 40.0)),
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

/// The rotary on `x` (mono), switching to `later` after `switch_at` samples: the left side and
/// the telemetry per 10 ms block.
fn rotary(s: &ModSettings, later: &ModSettings, switch_at: usize, x: &[f32]) -> (Vec<f32>, Vec<ModTelemetry>) {
    let mut m = Mod::new(FS, s);
    let (mut l, mut r) = (vec![0.0f32; x.len()], vec![0.0f32; x.len()]);
    let mut tel = Vec::new();
    for (k, ((a, b), c)) in l.chunks_mut(480).zip(r.chunks_mut(480)).zip(x.chunks(480)).enumerate() {
        let now = if k * 480 >= switch_at { later } else { s };
        tel.push(m.process([c, c], [a, b], false, now, &NO_TRANSPORT));
    }
    (l, tel)
}

#[test]
fn the_rotors_change_speed_each_with_its_own_inertia() {
    // From slow to fast: the light horn is nearly there in 2 s, the heavy drum still on its way;
    // braking, both coast to a stop.
    let slow = ModSettings { mode: Mode::Rotary, rotor: Rotor::Slow, ..ModSettings::default() };
    let fast = ModSettings { rotor: Rotor::Fast, ..slow };
    let x = vec![0.0f32; 48_000 * 2];
    let (_, tel) = rotary(&slow, &fast, 0, &x);
    let [horn, drum] = [tel[199].values[2], tel[199].values[3]];
    assert!(tel[0].values[2] < 1.5, "starts slow: {}", tel[0].values[2]);
    assert!(horn > 0.9 * ROTOR_FAST_HZ[0], "horn at 2 s: {horn}");
    assert!(drum < 0.6 * ROTOR_FAST_HZ[1] && drum > ROTOR_SLOW_HZ[1] * 2.0, "drum at 2 s: {drum}");
    // More inertia: slower still.
    let (_, heavy) = rotary(&ModSettings { inertia: 1.0, ..slow }, &ModSettings { inertia: 1.0, ..fast }, 0, &x);
    assert!(heavy[199].values[2] < 0.7 * horn, "{} against {horn}", heavy[199].values[2]);
    let brake = ModSettings { rotor: Rotor::Brake, ..slow };
    let (_, tel) = rotary(&fast, &brake, 0, &vec![0.0f32; 48_000 * 30]);
    let last = tel.last().expect("tel");
    assert!(last.values[2] < 0.02 && last.values[3] < 0.05, "{:?}", &last.values[..4]);
    // The places turn with the speeds, in 0…1.
    assert!(tel.iter().all(|t| (0.0..1.0).contains(&t.values[0]) && (0.0..1.0).contains(&t.values[1])));
}

#[test]
fn the_horn_sweeps_its_level_and_pitch_past_the_mics_and_depth_zero_stands_still() {
    // A 3 kHz tone (the horn's) through a fast rotary: each 5 ms slice's level swings with the
    // horn's turn at DEPTH 100 %, and stays put at 0 (only the rotors' sound, nothing moving).
    let n = 48_000 * 3;
    let x: Vec<f32> = (0..n).map(|i| 0.3 * (std::f32::consts::TAU * 3000.0 * i as f32 / FS).sin()).collect();
    let swing = |depth: f32| {
        let s = ModSettings { mode: Mode::Rotary, rotor: Rotor::Fast, depth, mix: 1.0, ..ModSettings::default() };
        let (l, _) = rotary(&s, &s, 0, &x);
        let db: Vec<f32> = l[48_000..].chunks(240).map(|c| 10.0 * (c.iter().map(|v| v * v).sum::<f32>() / c.len() as f32).max(1e-12).log10()).collect();
        db.iter().cloned().fold(f32::MIN, f32::max) - db.iter().cloned().fold(f32::MAX, f32::min)
    };
    let (still, moving) = (swing(0.0), swing(1.0));
    assert!(still < 0.3, "depth 0 swings {still} dB");
    assert!(moving > 6.0, "depth 100 % swings only {moving} dB");
}

#[test]
fn rotary_balance_tilts_and_then_takes_the_other_rotor_away() {
    assert_eq!(rotary_balance(0.0), [1.0, 1.0]);
    let [h, d] = rotary_balance(0.5);
    // Half way: the ±3 dB tilt and the drum at half: 12 dB between them.
    assert!((20.0 * (h / d).log10() - 12.04).abs() < 0.05, "{h} {d}");
    assert_eq!(rotary_balance(1.0)[1], 0.0);
    assert_eq!(rotary_balance(-1.0)[0], 0.0);
}

/// The frequency of `y` over `range`, Hz, from its upward zero crossings (interpolated).
fn frequency(y: &[f32], range: std::ops::Range<usize>) -> f64 {
    let ups: Vec<f64> = range.clone().skip(1).filter(|&i| y[i - 1] < 0.0 && y[i] >= 0.0).map(|i| i as f64 - 1.0 + f64::from(y[i - 1] / (y[i - 1] - y[i]))).collect();
    (ups.len() - 1) as f64 / ((ups[ups.len() - 1] - ups[0]) / f64::from(FS))
}

#[test]
fn micro_shifts_the_left_down_and_the_right_up_by_its_cents() {
    // A tone, all wet, steady (DEPTH 0): each side plays it × 2^(∓cents/1200), and the
    // readings say so; with DEPTH the LFO sways the shift around it.
    let n = 48_000 * 3;
    // 1030 Hz: not a whole number of cycles in half a window, so the two taps meet out of phase.
    let x: Vec<f32> = (0..n).map(|i| 0.5 * (std::f32::consts::TAU * 1030.0 * i as f32 / FS).sin()).collect();
    for cents in [9.0f32, 30.0] {
        let s = ModSettings { mode: Mode::Micro, detune_cents: cents, depth: 0.0, mix: 1.0, ..ModSettings::default() };
        let ([l, r], tel) = render(&s, &x, 480);
        let want = |c: f32| 1030.0 * 2f64.powf(f64::from(c) / 1200.0);
        let (fl, fr) = (frequency(&l, 24_000..n), frequency(&r, 24_000..n));
        assert!((fl - want(-cents)).abs() < 0.3, "{cents} cents: left {fl:.2} Hz, want {:.2}", want(-cents));
        assert!((fr - want(cents)).abs() < 0.3, "{cents} cents: right {fr:.2} Hz, want {:.2}", want(cents));
        let last = tel.last().expect("tel");
        assert_eq!((last.values[0], last.values[1]), (-cents, cents));
        // The level holds through the splices (the new tap matched to the old: within 0.5 dB; two
        // taps always crossfading dipped 2.9 dB here).
        let db: Vec<f32> = l[24_000..].chunks(480).map(|c| 10.0 * (c.iter().map(|v| v * v).sum::<f32>() / c.len() as f32).log10()).collect();
        let swing = db.iter().cloned().fold(f32::MIN, f32::max) - db.iter().cloned().fold(f32::MAX, f32::min);
        assert!(swing < 0.5, "{cents} cents: the level swings {swing:.2} dB");
    }
    let s = ModSettings { mode: Mode::Micro, detune_cents: 10.0, depth: 1.0, ..ModSettings::default() };
    assert_eq!((micro_cents(&s, 0, 1.0), micro_cents(&s, 1, -1.0)), (-15.0, 5.0));
}
