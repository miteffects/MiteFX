//! Mite Shape against what theory says it must do. FabFilter has no transient designer to
//! compare with, so every expectation here is worked out from first principles: the LR4
//! crossover's closed-form responses, the detector's definition (a differential envelope follower:
//! attack from the fast envelope's lead over a slow-rising one, sustain from a slow-falling one's
//! lead over the fast one), and the output stage's algebra. The whole engine is driven, as the host drives it.
//!
//! With one band there are no filters, so a steady (DC) input makes the output/input ratio the
//! gain itself, sample by sample: the shaping is read off exactly.

use mite_core::HistoryTap;
use mite_dsp::shape::{soft_clip, ShapeSettings};
use mite_shape::dsp::{Output, ShapeEngine};

const FS: f32 = 48_000.0;

fn db(x: f64) -> f64 {
    20.0 * x.abs().max(1e-12).log10()
}

/// Runs `channels` through a fresh engine in 256-sample blocks.
fn run_at(fs: f32, s: &ShapeSettings, out: &Output, mut channels: Vec<Vec<f32>>) -> (Vec<Vec<f32>>, usize) {
    let mut e = ShapeEngine::new(fs, s);
    e.prepare(fs, s, out);
    let tap = HistoryTap::new(64);
    let n = channels[0].len();
    let mut start = 0;
    while start < n {
        let end = (start + 256).min(n);
        let mut parts: Vec<&mut [f32]> = channels.iter_mut().map(|c| &mut c[start..end]).collect();
        e.process(&mut parts, s, out, &tap, None);
        start = end;
    }
    let latency = e.latency();
    (channels, latency)
}

fn run(s: &ShapeSettings, out: &Output, input: Vec<f32>) -> Vec<f32> {
    run_at(FS, s, out, vec![input]).0.remove(0)
}

/// Silence for `pre` samples, then a steady `level` for `len` samples: one onset, no decay.
fn step(pre: usize, level: f32, len: usize) -> Vec<f32> {
    let mut v = vec![0.0; pre];
    v.extend(std::iter::repeat_n(level, len));
    v
}

/// The gain in dB at `ms` after a step at sample `pre` (one band: output/input).
fn gain_at(y: &[f32], x: &[f32], pre: usize, ms: f64, fs: f32) -> f64 {
    let i = pre + (ms * f64::from(fs) / 1000.0).round() as usize;
    db(f64::from(y[i]) / f64::from(x[i]))
}

fn shaped(attack: f32, sustain: f32) -> ShapeSettings {
    let mut s = ShapeSettings::default();
    s.bands[0].attack_db = attack;
    s.bands[0].sustain_db = sustain;
    s
}

fn noise(n: usize, seed: u32, amp: f32) -> Vec<f32> {
    let mut x = seed;
    (0..n)
        .map(|_| {
            x = x.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            ((x >> 8) as f32 / (1u32 << 24) as f32 - 0.5) * 2.0 * amp
        })
        .collect()
}

fn sine(n: usize, f: f64, amp: f64, fs: f32) -> Vec<f32> {
    (0..n).map(|i| (amp * (std::f64::consts::TAU * f * i as f64 / f64::from(fs)).sin()) as f32).collect()
}

/// The complex response at `f` of an impulse response.
fn response(h: &[f32], f: f64, fs: f32) -> (f64, f64) {
    let w = std::f64::consts::TAU * f / f64::from(fs);
    h.iter().enumerate().fold((0.0, 0.0), |(re, im), (i, &v)| (re + f64::from(v) * (w * i as f64).cos(), im - f64::from(v) * (w * i as f64).sin()))
}

fn impulse(n: usize) -> Vec<f32> {
    let mut v = vec![0.0; n];
    v[0] = 1.0;
    v
}

/// The bilinear transform's frequency warping: what `f` is to a section tuned to `fc`.
fn warped(f: f64, fc: f64, fs: f32) -> f64 {
    let fs = f64::from(fs);
    (std::f64::consts::PI * f / fs).tan() / (std::f64::consts::PI * fc / fs).tan()
}

/// LR4 (two Butterworth sections) magnitudes at warped frequency `o`: low-pass and high-pass.
fn lr4(o: f64) -> (f64, f64) {
    let o4 = o.powi(4);
    (1.0 / (1.0 + o4), o4 / (1.0 + o4))
}

/// The phase of the allpass an LR4 low + high sums to, (s² − √2 s + 1)/(s² + √2 s + 1), at `o`.
fn lr4_allpass_phase(o: f64) -> f64 {
    -2.0 * (std::f64::consts::SQRT_2 * o).atan2(1.0 - o * o)
}

fn wrap(p: f64) -> f64 {
    let t = std::f64::consts::TAU;
    (p + std::f64::consts::PI).rem_euclid(t) - std::f64::consts::PI
}

// ---- nulls and the split -------------------------------------------------------------------

#[test]
fn one_neutral_band_is_bit_exact_at_every_rate_with_or_without_lookahead() {
    for fs in [44_100.0, 48_000.0, 96_000.0, 192_000.0] {
        for lookahead in [0.0, 1.5] {
            let s = ShapeSettings { lookahead_ms: lookahead, ..ShapeSettings::default() };
            let x = noise(8192, 1, 0.5);
            let (y, latency) = run_at(fs, &s, &Output::default(), vec![x.clone(), x.iter().map(|v| -v).collect()]);
            assert_eq!(latency, (lookahead * 0.001 * fs).round() as usize);
            for i in latency..x.len() {
                assert_eq!(y[0][i], x[i - latency], "{fs} Hz, {lookahead} ms, sample {i}");
                assert_eq!(y[1][i], -x[i - latency]);
            }
        }
    }
}

#[test]
fn each_band_has_the_lr4_magnitude_and_they_sum_to_the_analytic_allpass() {
    let (f1, f2) = (200.0, 2000.0);
    let base = ShapeSettings { crossovers: [Some(f1), Some(f2), None, None], ..ShapeSettings::default() };
    let h_all = run(&base, &Output::default(), impulse(32_768));
    let solo = |b: usize| {
        let mut s = base;
        s.bands[b].solo = true;
        run(&s, &Output::default(), impulse(32_768))
    };
    let bands = [solo(0), solo(1), solo(2)];
    for f in [30.0, 100.0, 200.0, 500.0, 1000.0, 2000.0, 5000.0, 12_000.0] {
        let (o1, o2) = (warped(f, f1.into(), FS), warped(f, f2.into(), FS));
        let ((lp1, hp1), (lp2, hp2)) = (lr4(o1), lr4(o2));
        let expect = [lp1, hp1 * lp2, hp1 * hp2];
        for (b, h) in bands.iter().enumerate() {
            let (re, im) = response(h, f, FS);
            let got = re.hypot(im);
            // Within 0.05 dB down to −60 dB; below that, within 10^-4 absolute.
            if expect[b] > 1e-3 {
                assert!((db(got) - db(expect[b])).abs() < 0.05, "band {b} at {f} Hz: {:.3} dB, theory {:.3} dB", db(got), db(expect[b]));
            } else {
                assert!((got - expect[b]).abs() < 1e-4, "band {b} at {f} Hz");
            }
        }
        // The sum: unity magnitude, the phase of the two crossovers' allpasses.
        let (re, im) = response(&h_all, f, FS);
        assert!(db(re.hypot(im)).abs() < 0.01, "sum at {f} Hz: {:.4} dB", db(re.hypot(im)));
        let theory = wrap(lr4_allpass_phase(o1) + lr4_allpass_phase(o2));
        let got = im.atan2(re);
        assert!(wrap(got - theory).abs().to_degrees() < 0.5, "sum phase at {f} Hz: {:.2}°, theory {:.2}°", got.to_degrees(), theory.to_degrees());
    }
    // Right on a crossover each side is half the amplitude (−6.02 dB), in phase.
    let (re0, im0) = response(&bands[0], f1.into(), FS);
    let (re1, im1) = response(&bands[1], f1.into(), FS);
    assert!((db(re0.hypot(im0)) + 6.0206).abs() < 0.02);
    assert!(wrap(im0.atan2(re0) - im1.atan2(re1)).abs().to_degrees() < 0.5, "LR4 halves are in phase");
}

#[test]
fn a_tone_on_a_crossover_hears_both_bands_gains_added_in_phase() {
    // Band 1 at +6 dB, band 2 at −6 dB (their faders, nothing moving): at the crossover each
    // carries half the tone in phase, so the output is (2 + 0.5) / 2 = 1.25 of it (+1.94 dB).
    let mut s = ShapeSettings { crossovers: [Some(1000.0), None, None, None], ..ShapeSettings::default() };
    s.bands[0].out_db = 20.0 * 2f32.log10();
    s.bands[1].out_db = 20.0 * 0.5f32.log10();
    let x = sine(48_000, 1000.0, 0.25, FS);
    let y = run(&s, &Output::default(), x);
    let tail = &y[24_000..];
    let peak = tail.iter().fold(0.0f32, |m, v| m.max(v.abs()));
    assert!((db(f64::from(peak) / 0.25) - db(1.25)).abs() < 0.03, "{:.3} dB", db(f64::from(peak) / 0.25));
}

#[test]
fn muting_a_band_and_soloing_it_add_up_to_the_whole() {
    let mut s = ShapeSettings { crossovers: [Some(300.0), Some(3000.0), None, None], ..ShapeSettings::default() };
    s.bands[1].attack_db = 6.0;
    s.bands[1].sustain_db = -4.0;
    let x = noise(24_000, 2, 0.3);
    let all = run(&s, &Output::default(), x.clone());
    let mut muted = s;
    muted.bands[1].mute = true;
    let mut soloed = s;
    soloed.bands[1].solo = true;
    let (m, o) = (run(&muted, &Output::default(), x.clone()), run(&soloed, &Output::default(), x));
    // After the 10 ms solo/mute fades.
    for i in 1000..all.len() {
        assert!((m[i] + o[i] - all[i]).abs() < 1e-5, "at {i}");
    }
}

// ---- the shaping itself -------------------------------------------------------------------

/// How much of its amount a lead of `d` dB brings when `full` dB is the mark (the detector's curve).
fn weight(d: f64, full: f64) -> f64 {
    (((d * d + 1.0).sqrt() - 1.0) / full).tanh()
}

/// Silence for `pre` samples, then a hit at `level` decaying with time constant `tau_ms`.
fn decaying(pre: usize, level: f32, tau_ms: f64, len: usize) -> Vec<f32> {
    let k = 1000.0 / (tau_ms * f64::from(FS));
    (0..pre + len).map(|i| if i < pre { 0.0 } else { (f64::from(level) * (-((i - pre) as f64) * k).exp()) as f32 }).collect()
}

#[test]
fn a_step_gets_the_attack_curve_theory_gives_and_returns_to_unity() {
    // Out of silence the fast envelope is 1 − e^(−t/a) of the step and the slow-rising one the
    // cascade of that with its own pole r (half the transient time); the attack is `weight` of
    // their ratio in dB. Nothing decays, so sustain adds nothing.
    let a_db = 7.5;
    let s = shaped(a_db, -6.0);
    let pre = 4800;
    let x = step(pre, 0.5, 19_200);
    let y = run(&s, &Output::default(), x.clone());
    let (a, r) = (0.5f64, 12.5f64);
    for ms in [2.0, 5.0, 10.0, 20.0, 30.0] {
        let fast = 1.0 - (-ms / a).exp();
        let rise = 1.0 - (r * (-ms / r).exp() - a * (-ms / a).exp()) / (r - a);
        let theory = f64::from(a_db) * weight(db(fast / rise), 12.0);
        let g = gain_at(&y, &x, pre, ms, FS);
        assert!((g - theory).abs() < 0.15, "at {ms} ms: {g:.3} dB, theory {theory:.3} dB");
    }
    assert!(y[..pre].iter().all(|v| *v == 0.0));
    assert!(gain_at(&y, &x, pre, 300.0, FS).abs() < 1e-3, "a held level is left alone");
}

#[test]
fn a_steady_signal_is_left_alone_whatever_the_amounts() {
    // A tone down to 30 Hz and noise: once settled, no gain at all, so neither distortion nor a
    // change of level. The fast envelope holds through the waveform's ripple.
    let s = shaped(12.0, -12.0);
    for f in [30.0, 60.0, 220.0, 4000.0] {
        let x = sine(48_000, f, 0.5, FS);
        let y = run(&s, &Output::default(), x.clone());
        let worst = (24_000..48_000).map(|i| (y[i] - x[i]).abs()).fold(0.0f32, f32::max);
        assert!(worst < 1e-4, "{f} Hz: off by {worst}");
    }
    let x = noise(96_000, 8, 0.5);
    let y = run(&s, &Output::default(), x.clone());
    let rms = |v: &[f32]| (v.iter().map(|s| f64::from(*s).powi(2)).sum::<f64>() / v.len() as f64).sqrt();
    let change = db(rms(&y[48_000..]) / rms(&x[48_000..]));
    assert!(change.abs() < 0.1, "noise changed by {change:.3} dB");
}

#[test]
fn sustain_grows_as_a_sound_decays_and_stays_within_its_amount() {
    let pre = 4800;
    let x = decaying(pre, 0.5, 80.0, 48_000);
    for amount in [9.0f32, -9.0] {
        let y = run(&shaped(0.0, amount), &Output::default(), x.clone());
        let g = |ms: f64| gain_at(&y, &x, pre, ms, FS);
        assert!(g(15.0).abs() < 0.01, "nothing during the hit: {:.3}", g(15.0));
        let trace: Vec<f64> = (20..300).map(|ms| g(f64::from(ms)).abs()).collect();
        assert!(trace.windows(2).all(|w| w[1] >= w[0] - 1e-3), "grows only");
        assert!(trace[trace.len() - 1] > 8.8 && trace[trace.len() - 1] <= 9.0 + 1e-3, "{:.3}", g(300.0));
        // A slower decay gets there later.
        let slow = decaying(pre, 0.5, 300.0, 48_000);
        let z = run(&shaped(0.0, amount), &Output::default(), slow.clone());
        assert!(gain_at(&z, &slow, pre, 100.0, FS).abs() < g(100.0).abs());
    }
}

#[test]
fn the_transient_time_scales_the_attack_and_the_transition_slows_sustain() {
    let pre = 4800;
    let x = step(pre, 0.5, 48_000);
    for time in [10.0f32, 25.0, 60.0, 200.0] {
        let mut s = shaped(12.0, 0.0);
        s.bands[0].transient_ms = time;
        let y = run(&s, &Output::default(), x.clone());
        let g = gain_at(&y, &x, pre, f64::from(time), FS);
        assert!(g > 0.2 && g < 1.2, "time {time}: {g:.3} dB left at the transient time");
    }
    let tail = decaying(pre, 0.5, 40.0, 19_200);
    let at = |transition: f32| {
        let mut s = shaped(0.0, 9.0);
        s.bands[0].transition = transition;
        let y = run(&s, &Output::default(), tail.clone());
        gain_at(&y, &tail, pre, 50.0, FS)
    };
    assert!(at(0.0) > at(1.0) + 0.5 && at(1.0) > at(2.0) + 0.5, "{:.2} {:.2} {:.2}", at(0.0), at(1.0), at(2.0));
}

#[test]
fn the_attack_scales_with_the_size_of_the_rise() {
    // Steps from a steady level: the lead starts at the step itself. Peak gain ≤ weight(step).
    let a = 12.0;
    let s = shaped(a, 0.0);
    let pre = 48_000;
    let mut last = 0.0;
    for step_db in [2.0f64, 6.0, 9.0, 24.0] {
        let base = 0.03f32;
        let mut x = vec![base; pre];
        x.extend(std::iter::repeat_n(base * 10f32.powf(step_db as f32 / 20.0), 9600));
        let y = run(&s, &Output::default(), x.clone());
        let peak = (pre..pre + 4800).map(|i| db(f64::from(y[i]) / f64::from(x[i]))).fold(0.0, f64::max);
        let share = peak / f64::from(a);
        let theory = weight(step_db, 12.0);
        assert!(share > 0.6 * theory && share < theory + 1e-3, "{step_db} dB step: {share:.3}, at most {theory:.3}");
        assert!(share > last, "bigger rises get more");
        last = share;
    }
}

#[test]
fn a_second_hit_on_a_tail_gets_its_own_attack() {
    // 15 ms after a first hit, a louder one: the gain rises again.
    let s = shaped(12.0, 0.0);
    let pre = 4800;
    let second = pre + 720;
    let x: Vec<f32> = (0..pre + 9600)
        .map(|i| {
            let d = |h: usize| if i >= h { (-((i - h) as f64) / 480.0).exp() } else { 0.0 };
            (0.001 + 0.25 * d(pre) + 0.9 * d(second)) as f32
        })
        .collect();
    let y = run(&s, &Output::default(), x.clone());
    let g = |i: usize| db(f64::from(y[i]) / f64::from(x[i]));
    assert!(g(second + 48) > g(second - 48) + 4.0, "{:.2} then {:.2}", g(second - 48), g(second + 48));
}

#[test]
fn the_shape_is_the_same_at_every_level_above_the_gate() {
    let s = shaped(6.0, -9.0);
    let pre = 4800;
    let reference = {
        let x = step(pre, 0.5, 9600);
        let y = run(&s, &Output::default(), x.clone());
        (pre..pre + 9600).map(|i| db(f64::from(y[i]) / f64::from(x[i]))).collect::<Vec<_>>()
    };
    // −26 and −46 dBFS against −6: 14 dB and more above the gate's top.
    for level in [0.05f32, 0.005] {
        let x = step(pre, level, 9600);
        let y = run(&s, &Output::default(), x.clone());
        // From 2 ms on: until the fast envelope clears the gate's top (−60 dBFS), a quiet hit is
        // still opening it.
        for (k, i) in (pre..pre + 9600).enumerate().skip(96) {
            let g = db(f64::from(y[i]) / f64::from(x[i]));
            assert!((g - reference[k]).abs() < 1e-3, "{level}: {g:.4} against {:.4} at {k}", reference[k]);
        }
    }
}

#[test]
fn below_the_gate_nothing_is_shaped() {
    // −90 dBFS: under the detector's −80 dBFS gate.
    let s = shaped(12.0, -12.0);
    let x = step(4800, 10f32.powf(-90.0 / 20.0), 9600);
    let y = run(&s, &Output::default(), x.clone());
    assert_eq!(y, x);
}

#[test]
fn the_floor_fades_a_lift_out_over_six_db() {
    // Sustain +12 dB, floor −30 dBFS, on a level that dropped 40 dB (the whole amount by now): full
    // lift at or above the floor, half 3 dB under it, none 6 dB under it.
    let mut s = shaped(0.0, 12.0);
    s.floor_db = -30.0;
    for (level_db, lift) in [(-20.0f64, 12.0), (-30.0, 12.0), (-33.0, 6.0), (-36.0, 0.0), (-45.0, 0.0)] {
        let level = 10f32.powf(level_db as f32 / 20.0);
        let x: Vec<f32> = (0..24_000).map(|i| if i < 4800 { level * 100.0 } else { level }).collect();
        let y = run(&s, &Output::default(), x.clone());
        let g = gain_at(&y, &x, 4800, 300.0, FS);
        assert!((g - lift).abs() < 0.05, "{level_db} dBFS: {g:.3} dB, theory {lift}");
    }
}

#[test]
fn the_gain_never_goes_past_either_amount() {
    let s = shaped(6.0, -4.0);
    // Bursts of noise at many levels and speeds.
    let raw = noise(96_000, 3, 0.5);
    let x: Vec<f32> = raw.iter().enumerate().map(|(i, v)| v * (0.02 + (-((i % (2400 + i / 40)) as f32) / 600.0).exp())).collect();
    let y = run(&s, &Output::default(), x.clone());
    for i in 0..x.len() {
        if x[i].abs() > 1e-3 {
            let g = db(f64::from(y[i]) / f64::from(x[i]));
            assert!((-4.0 - 1e-3..=6.0 + 1e-3).contains(&g), "{g:.3} dB at {i}");
        }
    }
}

#[test]
fn a_sustain_length_hands_back_to_unity_along_its_s_curve() {
    // Transient time 25 ms, length 50 ms: sustain until 75 ms, then back to 0 dB over 15 ms
    // (30 % of the length), half of what it would be at 82.5 ms. The endless one is the reference.
    let mut s = shaped(0.0, -9.0);
    let pre = 4800;
    let x = decaying(pre, 0.5, 20.0, 9600);
    let endless = run(&s, &Output::default(), x.clone());
    s.bands[0].length_ms = 50.0;
    let y = run(&s, &Output::default(), x.clone());
    let g = |ms: f64| gain_at(&y, &x, pre, ms, FS);
    let e = |ms: f64| gain_at(&endless, &x, pre, ms, FS);
    assert!(e(60.0) < -3.0 && (g(60.0) - e(60.0)).abs() < 0.05, "{:.3} against {:.3}", g(60.0), e(60.0));
    assert!((g(82.5) - e(82.5) / 2.0).abs() < 0.4, "{:.3} against {:.3}", g(82.5), e(82.5));
    assert!(g(91.0).abs() < 0.05, "{:.3}", g(91.0));
    assert!(g(150.0).abs() < 1e-3);
}

#[test]
fn sample_rate_does_not_change_the_shape_in_time() {
    let s = shaped(6.0, -6.0);
    let at = |fs: f32| {
        let pre = (0.1 * fs) as usize;
        let x = step(pre, 0.5, (0.2 * fs) as usize);
        let y = run_at(fs, &s, &Output::default(), vec![x.clone()]).0.remove(0);
        [3.0, 20.0, 25.0, 30.0, 60.0].map(|ms| gain_at(&y, &x, pre, ms, fs))
    };
    let reference = at(48_000.0);
    for fs in [44_100.0, 96_000.0, 192_000.0] {
        let g = at(fs);
        for k in 0..g.len() {
            assert!((g[k] - reference[k]).abs() < 0.25, "{fs} Hz, point {k}: {:.3} against {:.3}", g[k], reference[k]);
        }
    }
}

// ---- timing, stereo and the output stage -----------------------------------------------------

#[test]
fn lookahead_moves_the_gain_ahead_of_the_audio_by_exactly_its_latency() {
    // The detector hears the hit undelayed and the audio is delayed: so the delayed audio meets
    // exactly the gain the undelayed run applied that many samples later... the same gain curve,
    // on audio L samples late.
    let s0 = shaped(6.0, -6.0);
    let s1 = ShapeSettings { lookahead_ms: 2.0, ..s0 };
    let pre = 4800;
    let x = step(pre, 0.5, 9600);
    let y0 = run(&s0, &Output::default(), x.clone());
    let (y1, latency) = run_at(FS, &s1, &Output::default(), vec![x.clone()]);
    let y1 = &y1[0];
    assert_eq!(latency, 96);
    assert!(y1[..pre + latency].iter().all(|v| *v == 0.0), "nothing before the delayed hit");
    for i in pre + latency..x.len() {
        assert_eq!(y1[i], y0[i], "at {i}: the same gain at the same moment");
    }
}

#[test]
fn a_linked_pair_moves_together_and_an_unlinked_one_does_not() {
    // A hit on the left only, the right steady and quiet all along (linked, the detectors hear
    // the louder side, so the step they hear is the left's).
    let pre = 9600;
    let left = step(pre, 0.5, 9600);
    let right = vec![0.002f32; pre + 9600];
    for (link, same) in [(1.0f32, true), (0.0, false)] {
        let s = ShapeSettings { link, ..shaped(9.0, 0.0) };
        let (y, _) = run_at(FS, &s, &Output::default(), vec![left.clone(), right.clone()]);
        let g_left = gain_at(&y[0], &left, pre, 1.0, FS);
        let g_right = gain_at(&y[1], &right, pre, 1.0, FS);
        assert!(g_left > 8.0, "the hit's side: {g_left:.2}");
        if same {
            assert!((g_right - g_left).abs() < 1e-3, "linked: {g_right:.3} against {g_left:.3}");
        } else {
            assert!(g_right.abs() < 1e-3, "unlinked: the steady side is untouched ({g_right:.3})");
        }
    }
}

#[test]
fn mid_side_on_a_centred_signal_is_the_same_as_left_right() {
    let s = shaped(6.0, -6.0);
    let x = noise(24_000, 4, 0.4).iter().enumerate().map(|(i, v)| v * (-((i % 4800) as f32) / 900.0).exp()).collect::<Vec<_>>();
    let (lr, _) = run_at(FS, &s, &Output::default(), vec![x.clone(), x.clone()]);
    let (ms, _) = run_at(FS, &ShapeSettings { mid_side: true, ..s }, &Output::default(), vec![x.clone(), x.clone()]);
    for i in 0..x.len() {
        assert!((ms[0][i] - lr[0][i]).abs() < 1e-5 && (ms[1][i] - lr[1][i]).abs() < 1e-5, "at {i}");
        assert_eq!(ms[0][i], ms[1][i]);
    }
}

#[test]
fn mix_band_mix_and_band_output_are_exact_blends() {
    let x = noise(24_000, 5, 0.4).iter().enumerate().map(|(i, v)| v * (-((i % 4800) as f32) / 900.0).exp()).collect::<Vec<_>>();
    let s = shaped(6.0, -6.0);
    let wet = run(&s, &Output::default(), x.clone());
    // Global mix 30 %: 0.7 dry + 0.3 wet (one band: dry is the input).
    let mixed = run(&s, &Output { mix: 0.3, ..Output::default() }, x.clone());
    // Band mix 30 %: the same blend, inside the band.
    let band_mixed = run(&ShapeSettings { bands: [mite_dsp::shape::BandShape { mix: 0.3, ..s.bands[0] }; 5], ..s }, &Output::default(), x.clone());
    // Band output −6 dB: the wet signal × 10^(−6/20).
    let quieter = run(&ShapeSettings { bands: [mite_dsp::shape::BandShape { out_db: -6.0, ..s.bands[0] }; 5], ..s }, &Output::default(), x.clone());
    let k = 10f32.powf(-6.0 / 20.0);
    for i in 0..x.len() {
        let blend = 0.7 * x[i] + 0.3 * wet[i];
        assert!((mixed[i] - blend).abs() < 1e-6, "mix at {i}");
        assert!((band_mixed[i] - blend).abs() < 1e-6, "band mix at {i}");
        assert!((quieter[i] - wet[i] * k).abs() < 1e-6, "band output at {i}");
    }
}

// ---- the clip stage -------------------------------------------------------------------------

#[test]
fn the_clip_curve_is_continuous_monotone_odd_and_never_passes_its_ceiling() {
    for softness in [0.0f32, 0.2, 0.4, 1.0] {
        let c = 0.5;
        let knee = c * (1.0 - softness);
        let mut last = -1.0f32;
        for k in 0..=4000 {
            let x = -2.0 + k as f32 * 0.001;
            let y = soft_clip(x, c, softness);
            assert!(y >= last - 1e-7, "monotone at {x}");
            assert!(y.abs() <= c + 1e-6, "under the ceiling at {x}");
            assert!((y + soft_clip(-x, c, softness)).abs() < 1e-7, "odd at {x}");
            if x.abs() <= knee {
                assert_eq!(y, x, "untouched under the knee at {x}");
            }
            last = y;
        }
        if softness > 0.0 {
            // Slope 1 into the knee: no corner where clipping starts.
            let d = 1e-3;
            let slope = (soft_clip(knee + d, c, softness) - soft_clip(knee, c, softness)) / d;
            assert!((slope - 1.0).abs() < 0.01, "softness {softness}: slope {slope}");
        }
    }
}

#[test]
fn the_clip_stage_holds_its_ceiling_and_leaves_quiet_signals_alone() {
    let s = ShapeSettings::default();
    let clip = Output { clip: true, ceiling_db: -6.0, softness: 0.4, ..Output::default() };
    // Loud: a 100 Hz sine at 0 dBFS never comes out over −6 dBFS (the resampler's ripple aside).
    let (y, latency) = run_at(FS, &s, &clip, vec![sine(48_000, 100.0, 1.0, FS)]);
    assert_eq!(latency, mite_dsp::oversample::Oversampler::latency_for(4));
    let peak = y[0][4800..].iter().fold(0.0f32, |m, v| m.max(v.abs()));
    assert!(db(f64::from(peak)) < -6.0 + 0.1, "{:.3} dBFS", db(f64::from(peak)));
    // Quiet: under the knee (−10.4 dBFS here) the stage is a pure delay of its latency.
    let x = sine(48_000, 1000.0, 0.2, FS);
    let (y, latency) = run_at(FS, &s, &clip, vec![x.clone()]);
    let worst = (4800..x.len()).map(|i| (y[0][i] - x[i - latency]).abs()).fold(0.0f32, f32::max);
    assert!(worst < 2e-4, "{worst}");
}

#[test]
fn the_clip_stage_aliases_far_less_than_a_plain_clipper() {
    // A 5 kHz sine hard-clipped at 44.1 kHz: its odd harmonics above Nyquist fold back to
    // frequencies that are not harmonics. Oversampled, they are filtered out before they fold.
    let fs = 44_100.0;
    let n = 44_100;
    let x = sine(n, 5000.0, 1.0, fs);
    let s = ShapeSettings::default();
    let (y, _) = run_at(fs, &s, &Output { clip: true, ceiling_db: -6.0, softness: 0.0, ..Output::default() }, vec![x.clone()]);
    let plain: Vec<f32> = x.iter().map(|v| soft_clip(*v, 0.5, 0.0)).collect();
    let folded = |v: &[f32]| {
        let tail = &v[n / 2..];
        let m = tail.len() as f64;
        let level = |f: f64| {
            let w = std::f64::consts::TAU * f / f64::from(fs);
            let (re, im) = tail.iter().enumerate().fold((0.0, 0.0), |(re, im), (i, &s)| {
                let t = std::f64::consts::TAU * i as f64 / m;
                let win = 0.35875 - 0.48829 * t.cos() + 0.14128 * (2.0 * t).cos() - 0.01168 * (3.0 * t).cos();
                (re + win * f64::from(s) * (w * i as f64).cos(), im + win * f64::from(s) * (w * i as f64).sin())
            });
            re * re + im * im
        };
        let fundamental = level(5000.0);
        let aliases: f64 = (3..=15)
            .step_by(2)
            .map(|k| {
                let f = (5000.0 * f64::from(k)) % f64::from(fs);
                if f > f64::from(fs) / 2.0 { f64::from(fs) - f } else { f }
            })
            .filter(|f| *f > 50.0 && (f / 5000.0 - (f / 5000.0).round()).abs() > 0.01)
            .map(level)
            .sum::<f64>();
        10.0 * (aliases / fundamental).log10()
    };
    let (ours, naive) = (folded(&y[0]), folded(&plain));
    assert!(ours < naive - 20.0, "oversampled {ours:.1} dB against plain {naive:.1} dB");
}
