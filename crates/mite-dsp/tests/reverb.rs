//! Mite Verb's algorithm measured (docs/design/verb-brief.md §4): decay times per octave against
//! the settings and against `decay_at`, stability at the extremes, freeze, echo density, stereo,
//! block-size invariance and click-free parameter moves.

use mite_dsp::reverb::{decay_at, design_absorption, line_len, Reverb, ReverbSettings, LINES};

fn render(fs: f32, s: &ReverbSettings, input: &[f32], block: usize) -> [Vec<f32>; 2] {
    let mut r = Reverb::new(fs, s);
    let (mut l, mut rr) = (vec![0.0f32; input.len()], vec![0.0f32; input.len()]);
    let mut pos = 0;
    while pos < input.len() {
        let end = (pos + block).min(input.len());
        r.process([&input[pos..end], &input[pos..end]], [&mut l[pos..end], &mut rr[pos..end]], true, s);
        pos = end;
    }
    [l, rr]
}

fn impulse(n: usize) -> Vec<f32> {
    let mut x = vec![0.0f32; n];
    x[0] = 1.0;
    x
}

/// A plain (not modulated, no reflections) tank, so the decay is the absorption's alone.
fn plain(s: ReverbSettings) -> ReverbSettings {
    ReverbSettings { mod_depth: 0.0, early: 0.0, ..s }
}

/// An octave band (two second-order band-passes, Q √2) of `x` at `fc`.
fn octave(x: &[f32], fc: f64, fs: f64) -> Vec<f64> {
    let w = std::f64::consts::TAU * fc / fs;
    let (sin, cos) = w.sin_cos();
    let alpha = sin / (2.0 * std::f64::consts::SQRT_2);
    let a0 = 1.0 + alpha;
    let (b0, b2, a1, a2) = (alpha / a0, -alpha / a0, -2.0 * cos / a0, (1.0 - alpha) / a0);
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

/// T60 from Schroeder's backward integration, the line fitted from −5 to −25 dB (T20 × 3).
fn t60(band: &[f64], fs: f64) -> f64 {
    let mut edc = vec![0.0; band.len()];
    let mut acc = 0.0;
    for i in (0..band.len()).rev() {
        acc += band[i] * band[i];
        edc[i] = acc;
    }
    let total = edc[0];
    let db: Vec<f64> = edc.iter().map(|&e| 10.0 * (e / total).max(1e-30).log10()).collect();
    let (mut sx, mut sy, mut sxx, mut sxy, mut n) = (0.0, 0.0, 0.0, 0.0, 0.0);
    for (i, &d) in db.iter().enumerate() {
        if (-25.0..=-5.0).contains(&d) {
            let t = i as f64 / fs;
            sx += t;
            sy += d;
            sxx += t * t;
            sxy += t * d;
            n += 1.0;
        }
    }
    let slope = (n * sxy - sx * sy) / (n * sxx - sx * sx);
    -60.0 / slope
}

#[test]
fn measured_decay_times_follow_the_settings_and_decay_at() {
    let cases = [
        (48_000.0, ReverbSettings { decay_s: 1.5, lo_mult: 2.0, lo_freq: 300.0, hi_mult: 0.4, hi_freq: 3000.0, size: 0.5, ..ReverbSettings::default() }),
        (44_100.0, ReverbSettings { decay_s: 0.8, lo_mult: 1.0, hi_mult: 1.0, size: 0.2, ..ReverbSettings::default() }),
        (96_000.0, ReverbSettings { decay_s: 2.5, lo_mult: 0.5, lo_freq: 400.0, hi_mult: 0.6, hi_freq: 4000.0, size: 0.8, ..ReverbSettings::default() }),
    ];
    for (fs, s) in cases {
        let s = plain(s);
        let n = (fs * (s.decay_s * s.lo_mult.max(1.0) * 1.2 + 0.3)) as usize;
        let [l, r] = render(fs, &s, &impulse(n), 512);
        let mono: Vec<f32> = l.iter().zip(&r).map(|(a, b)| a + b).collect();
        for fc in [125.0, 500.0, 1000.0, 2000.0, 8000.0] {
            let measured = t60(&octave(&mono, fc, f64::from(fs)), f64::from(fs));
            let predicted = decay_at(&s, fc, f64::from(fs));
            assert!((measured / predicted - 1.0).abs() < 0.1, "{fs} Hz, {fc} Hz octave: measured {measured:.3} s, decay_at {predicted:.3} s");
        }
        // And decay_at says what the settings say where the shelves have settled.
        let mid = decay_at(&s, (f64::from(s.lo_freq) * f64::from(s.hi_freq)).sqrt(), f64::from(fs));
        assert!((mid / f64::from(s.decay_s) - 1.0).abs() < 0.12, "mid {mid}");
    }
}

#[test]
fn the_loop_never_gains_at_any_extreme() {
    let fs = 48_000.0;
    for decay_s in [0.1, 5.0, 30.0] {
        for (lo_mult, hi_mult) in [(4.0, 4.0), (0.25, 0.1), (4.0, 0.1), (0.25, 4.0)] {
            for (lo_freq, hi_freq) in [(50.0, 16_000.0), (2000.0, 1000.0)] {
                for size in [0.0, 1.0] {
                    let s = ReverbSettings { decay_s, lo_mult, hi_mult, lo_freq, hi_freq, size, ..ReverbSettings::default() };
                    for i in 0..LINES {
                        let a = design_absorption(f64::from(line_len(i, size, fs)), &s, f64::from(fs), 0.0);
                        for k in 0..=2000 {
                            let w = std::f64::consts::PI * k as f64 / 2000.0;
                            assert!(a.magnitude(w) < 1.0, "{s:?} line {i} at {w}: {}", a.magnitude(w));
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn a_minute_of_noise_at_the_longest_settings_never_grows() {
    let fs = 48_000.0;
    let s = ReverbSettings { decay_s: 30.0, lo_mult: 4.0, hi_mult: 4.0, lo_freq: 2000.0, hi_freq: 1000.0, size: 1.0, mod_depth: 1.0, mod_rate: 5.0, diffusion: 1.0, ..ReverbSettings::default() };
    let mut seed = 1u32;
    let mut noise = || {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        seed as f32 / u32::MAX as f32 - 0.5
    };
    // 20 s of noise, then 40 s of silence: the tail falls (30 s T60 at its fastest band).
    let n = (fs * 60.0) as usize;
    let x: Vec<f32> = (0..n).map(|i| if i < n / 3 { noise() } else { 0.0 }).collect();
    let [l, _] = render(fs, &s, &x, 1024);
    let rms = |a: usize, b: usize| (l[a..b].iter().map(|v| f64::from(*v).powi(2)).sum::<f64>() / (b - a) as f64).sqrt();
    let sec = fs as usize;
    let mut last = rms(20 * sec, 21 * sec);
    for k in 22..60 {
        let now = rms(k * sec, (k + 1) * sec);
        assert!(now <= last * 1.05, "second {k}: {now} after {last}");
        last = last.min(now).max(now);
    }
    assert!(l.iter().all(|v| v.is_finite() && v.abs() < 50.0));
}

#[test]
fn freeze_holds_the_tail_and_lets_go_to_the_set_decay() {
    let fs = 48_000.0;
    let s = ReverbSettings { decay_s: 1.0, size: 0.4, ..ReverbSettings::default() };
    let frozen = ReverbSettings { freeze: true, ..s };
    let mut r = Reverb::new(fs, &s);
    let sec = fs as usize;
    let mut seed = 7u32;
    let mut noise = || {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        seed as f32 / u32::MAX as f32 - 0.5
    };
    let mut energy = Vec::new();
    let (mut l, mut rr) = (vec![0.0f32; 480], vec![0.0f32; 480]);
    for block in 0..(14 * sec / 480) {
        let t = block * 480;
        // 0.5 s of noise, freeze from 0.5 s, more noise (ignored while frozen), release at 11 s.
        let x: Vec<f32> = (0..480).map(|_| if t < sec / 2 || (2 * sec..3 * sec).contains(&t) { noise() } else { 0.0 }).collect();
        let settings = if (sec / 2..11 * sec).contains(&t) { &frozen } else { &s };
        r.process([&x, &x], [&mut l, &mut rr], true, settings);
        energy.push(l.iter().chain(&rr).map(|v| f64::from(*v).powi(2)).sum::<f64>());
    }
    let per_sec = |k: usize| -> f64 { energy[k * 100..(k + 1) * 100].iter().sum::<f64>() };
    let db = |e: f64| 10.0 * e.log10();
    let held = per_sec(1);
    for k in 2..11 {
        assert!((db(per_sec(k)) - db(held)).abs() < 1.0, "second {k}: {:.2} dB against {:.2} dB", db(per_sec(k)), db(held));
    }
    // Released: falls 60 dB per second (decay 1 s), give or take the bands.
    let fall = db(per_sec(12)) - db(per_sec(13));
    assert!((45.0..80.0).contains(&fall), "{fall} dB in the second second after the release");
}

#[test]
fn the_tail_is_dense_after_80_ms() {
    let fs = 48_000.0;
    let s = ReverbSettings { size: 0.5, early: 0.0, ..ReverbSettings::default() };
    let [l, _] = render(fs, &s, &impulse(fs as usize / 2), 512);
    // Kurtosis in 10 ms windows, each normalised by its own energy (the decay removed).
    let w = (fs * 0.01) as usize;
    let mut ks = Vec::new();
    for k in 8..30 {
        let win = &l[k * w..(k + 1) * w];
        let m2 = win.iter().map(|v| f64::from(*v).powi(2)).sum::<f64>() / w as f64;
        let m4 = win.iter().map(|v| f64::from(*v).powi(4)).sum::<f64>() / w as f64;
        ks.push(m4 / (m2 * m2));
    }
    let mean = ks.iter().sum::<f64>() / ks.len() as f64;
    assert!(mean < 4.0, "kurtosis {mean}: {ks:?}");
}

#[test]
fn width_decorrelates_the_sides_and_zero_width_is_mono() {
    let fs = 48_000.0;
    let corr = |s: &ReverbSettings| {
        let [l, r] = render(fs, s, &impulse(fs as usize), 512);
        let from = (fs * 0.1) as usize;
        let (a, b) = (&l[from..], &r[from..]);
        let dot: f64 = a.iter().zip(b).map(|(x, y)| f64::from(*x) * f64::from(*y)).sum();
        let ea: f64 = a.iter().map(|x| f64::from(*x).powi(2)).sum();
        let eb: f64 = b.iter().map(|x| f64::from(*x).powi(2)).sum();
        dot / (ea * eb).sqrt()
    };
    let wide = corr(&ReverbSettings::default());
    assert!(wide.abs() < 0.3, "{wide}");
    let mono = corr(&ReverbSettings { width: 0.0, ..ReverbSettings::default() });
    assert!(mono > 0.9999, "{mono}");
}

#[test]
fn the_output_is_the_same_for_every_block_size() {
    let fs = 48_000.0;
    let s = ReverbSettings::default();
    let mut x: Vec<f32> = (0..24_000).map(|i| ((i as f32) * 0.03).sin() * if i < 6000 { 0.5 } else { 0.0 }).collect();
    x[100] = 1.0;
    let a = render(fs, &s, &x, 512);
    for block in [1, 7, 64, 333] {
        let b = render(fs, &s, &x, block);
        assert_eq!(a, b, "block {block}");
    }
}

#[test]
fn moving_any_setting_fast_never_clicks() {
    // A low tone through the reverb: its second difference is tiny; a discontinuity is not.
    let fs = 48_000.0;
    let n = 96_000;
    let x: Vec<f32> = (0..n).map(|i| 0.5 * (std::f32::consts::TAU * 150.0 * i as f32 / fs).sin()).collect();
    let base = ReverbSettings { hi_cut: 4000.0, ..ReverbSettings::default() };
    let with = |f: &dyn Fn(&mut ReverbSettings)| {
        let mut s = base;
        f(&mut s);
        s
    };
    let cases: [(&str, ReverbSettings); 12] = [
        ("predelay", with(&|s| s.predelay_ms = 500.0)),
        ("size", with(&|s| s.size = 0.8)),
        ("decay", with(&|s| s.decay_s = 20.0)),
        ("lo_mult", with(&|s| s.lo_mult = 4.0)),
        ("hi_mult", with(&|s| s.hi_mult = 0.1)),
        ("lo_freq", with(&|s| s.lo_freq = 2000.0)),
        ("diffusion", with(&|s| s.diffusion = 0.0)),
        ("early", with(&|s| s.early = 1.0)),
        ("width", with(&|s| s.width = 0.0)),
        ("lo_cut", with(&|s| s.lo_cut = 1000.0)),
        ("freeze", with(&|s| s.freeze = true)),
        ("duck", with(&|s| s.duck_db = 24.0)),
    ];
    let d2 = |v: &[f32], i: usize| (v[i + 1] - 2.0 * v[i] + v[i - 1]).abs();
    for (name, after) in cases {
        let mut r = Reverb::new(fs, &base);
        let [mut l, mut rr] = [vec![0.0f32; n], vec![0.0f32; n]];
        let at = 48_000;
        for (k, (a, b)) in l.chunks_mut(480).zip(rr.chunks_mut(480)).enumerate() {
            let s = if k * 480 >= at { &after } else { &base };
            let p = k * 480;
            r.process([&x[p..p + a.len()], &x[p..p + a.len()]], [a, b], true, s);
        }
        let steady = (at - 24_000..at).map(|i| d2(&l, i).max(d2(&rr, i))).fold(0.0, f32::max);
        let moving = (at..at + 24_000).map(|i| d2(&l, i).max(d2(&rr, i))).fold(0.0, f32::max);
        assert!(moving <= steady * 4.0 + 1e-5, "{name}: {moving} after {steady}");
    }
}
