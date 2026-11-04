//! Mite DeEss's engine measured (docs/design/deess-brief.md §4): untouched while nothing is a
//! sibilant (bit for bit, at the lookahead), a sibilant brought to the threshold, relative
//! detection independent of the level, each way of reducing, the lookahead, block sizes, clicks.

use mite_dsp::deess::{DeEssSettings, DeEsser, Detection, Reduce};

const FS: f32 = 48_000.0;

/// A voice: a 220 Hz vowel (harmonics to 2 kHz) at `db`, with a sibilant (noise band-passed to
/// 5…9 kHz) for 80 ms every 400 ms from 200 ms on, an "air" tone at 15 kHz and a presence tone at
/// 2.5 kHz, both 12 dB down.
fn voice(seconds: f32, db: f32, sib_db: f32) -> (Vec<f32>, Vec<bool>) {
    let n = (seconds * FS) as usize;
    let g = 10f32.powf(db / 20.0);
    let gs = 10f32.powf(sib_db / 20.0);
    let mut seed = 11u32;
    // A simple band-pass for the noise: two RBJ sections at 7 kHz, Q 1.5.
    let w = std::f32::consts::TAU * 7000.0 / FS;
    let alpha = w.sin() / 3.0;
    let a0 = 1.0 + alpha;
    let (b0, b2, a1, a2) = (alpha / a0, -alpha / a0, -2.0 * w.cos() / a0, (1.0 - alpha) / a0);
    let mut st = [[0.0f32; 4]; 2];
    let mut x = vec![0.0f32; n];
    let mut sib = vec![false; n];
    for i in 0..n {
        let t = i as f32 / FS;
        let vowel: f32 = (1..10).map(|k| (std::f32::consts::TAU * 220.0 * k as f32 * t).sin() / k as f32).sum::<f32>() * 0.4;
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        let mut v = seed as f32 / u32::MAX as f32 - 0.5;
        for s in &mut st {
            let y = b0 * v + b2 * s[1] - a1 * s[2] - a2 * s[3];
            s[1] = s[0];
            s[0] = v;
            s[3] = s[2];
            s[2] = y;
            v = y;
        }
        let on = i >= 9600 && (i - 9600) % 19_200 < 3840;
        sib[i] = on;
        x[i] = g * vowel + if on { gs * 8.0 * v } else { 0.0 } + 0.25 * g * (std::f32::consts::TAU * 15_000.0 * t).sin() + 0.25 * g * (std::f32::consts::TAU * 2_500.0 * t).sin();
    }
    (x, sib)
}

fn run(s: &DeEssSettings, x: &[f32], block: usize) -> Vec<f32> {
    let mut d = DeEsser::new(FS, s);
    let mut y = x.to_vec();
    for c in y.chunks_mut(block) {
        d.process(&mut [c], s);
    }
    y
}

/// `x`'s power in a band (two RBJ band-passes at `fc`, Q 2), dB, over `range`.
fn band_db(x: &[f32], fc: f32, range: std::ops::Range<usize>) -> f32 {
    let w = std::f32::consts::TAU * fc / FS;
    let alpha = w.sin() / 4.0;
    let a0 = 1.0 + alpha;
    let (b0, b2, a1, a2) = (alpha / a0, -alpha / a0, -2.0 * w.cos() / a0, (1.0 - alpha) / a0);
    let mut y = x.to_vec();
    for _ in 0..2 {
        let (mut x1, mut x2, mut y1, mut y2) = (0.0f32, 0.0f32, 0.0f32, 0.0f32);
        for v in y.iter_mut() {
            let x0 = *v;
            let o = b0 * x0 + b2 * x2 - a1 * y1 - a2 * y2;
            x2 = x1;
            x1 = x0;
            y2 = y1;
            y1 = o;
            *v = o;
        }
    }
    let p: f32 = y[range.clone()].iter().map(|v| v * v).sum::<f32>() / range.len() as f32;
    10.0 * p.max(1e-20).log10()
}

#[test]
fn nothing_is_touched_without_sibilants() {
    let (x, _) = voice(1.0, -12.0, -200.0);
    for reduce in [Reduce::Shelf, Reduce::Band, Reduce::Wide] {
        let s = DeEssSettings { reduce, ..DeEssSettings::default() };
        let y = run(&s, &x, 333);
        let look = DeEsser::latency_for(&s, FS);
        assert_eq!(look, 144);
        assert_eq!(&y[look..], &x[..x.len() - look], "{reduce:?}");
    }
}

#[test]
fn a_sibilant_is_brought_to_the_threshold() {
    // Relative, shelf: during a sibilant its band sits at about the threshold against the whole.
    let (x, sib) = voice(2.0, -12.0, -12.0);
    let s = DeEssSettings { threshold_db: -15.0, range_db: 24.0, ..DeEssSettings::default() };
    let y = run(&s, &x, 512);
    let look = DeEsser::latency_for(&s, FS);
    // The second sibilant, its middle 40 ms, at the output.
    let start = 9600 + 19_200 + 960;
    let r = start + look..start + look + 1920;
    assert!(sib[start] && sib[start + 1920]);
    let before = band_db(&x, 7000.0, start..start + 1920);
    let after = band_db(&y, 7000.0, r.clone());
    assert!(before - after > 6.0, "reduced {:.1} dB", before - after);
    // The vowel under it untouched.
    let vowel = band_db(&y, 440.0, r) - band_db(&x, 440.0, start..start + 1920);
    assert!(vowel.abs() < 0.3, "{vowel}");
}

#[test]
fn relative_detection_does_not_care_how_loud_the_voice_is() {
    let reduced = |db: f32, detection: Detection| {
        let (x, _) = voice(2.0, db, db);
        let s = DeEssSettings { detection, threshold_db: if detection == Detection::Relative { -15.0 } else { -30.0 }, range_db: 24.0, ..DeEssSettings::default() };
        let y = run(&s, &x, 512);
        let start = 9600 + 19_200 + 960;
        band_db(&x, 7000.0, start..start + 1920) - band_db(&y, 7000.0, start + 144..start + 144 + 1920)
    };
    let (loud, quiet) = (reduced(-6.0, Detection::Relative), reduced(-30.0, Detection::Relative));
    assert!((loud - quiet).abs() < 1.0, "relative: {loud:.1} against {quiet:.1}");
    let (loud, quiet) = (reduced(-6.0, Detection::Absolute), reduced(-30.0, Detection::Absolute));
    assert!(loud - quiet > 6.0, "absolute: {loud:.1} against {quiet:.1}");
}

#[test]
fn each_way_reduces_what_it_says() {
    let (x, _) = voice(2.0, -12.0, -12.0);
    let start = 9600 + 19_200 + 960;
    let r_in = start..start + 1920;
    let r_out = start + 144..start + 144 + 1920;
    let drop = |reduce: Reduce, fc: f32| {
        let s = DeEssSettings { reduce, range_db: 24.0, ..DeEssSettings::default() };
        let y = run(&s, &x, 512);
        band_db(&x, fc, r_in.clone()) - band_db(&y, fc, r_out.clone())
    };
    // The shelf takes the air down with the sibilant; the band leaves it; wide takes everything.
    assert!(drop(Reduce::Shelf, 15_000.0) > 5.0);
    assert!(drop(Reduce::Band, 15_000.0) < 2.0, "{}", drop(Reduce::Band, 15_000.0));
    assert!(drop(Reduce::Band, 2500.0) < 1.0, "{}", drop(Reduce::Band, 2500.0));
    assert!(drop(Reduce::Band, 7000.0) > 5.0);
    assert!(drop(Reduce::Wide, 440.0) > 5.0);
    assert!(drop(Reduce::Shelf, 440.0) < 0.3);
}

#[test]
fn the_lookahead_has_the_reduction_ready_when_the_sibilant_arrives() {
    let (x, _) = voice(1.0, -12.0, -6.0);
    let look = 144;
    let s = DeEssSettings { range_db: 24.0, ..DeEssSettings::default() };
    // The first sibilant's first 1 ms, at the output: already reduced against no lookahead.
    let y = run(&s, &x, 64);
    let y0 = run(&DeEssSettings { lookahead: false, ..s }, &x, 64);
    let ahead = band_db(&y, 7000.0, 9600 + look..9600 + look + 96);
    let late = band_db(&y0, 7000.0, 9600..9600 + 96);
    assert!(late - ahead > 3.0, "lookahead {ahead:.1} dB, none {late:.1} dB");
}

#[test]
fn the_output_is_the_same_for_any_block_size() {
    let (x, _) = voice(1.0, -12.0, -10.0);
    let s = DeEssSettings { reduce: Reduce::Band, ..DeEssSettings::default() };
    let a = run(&s, &x, 512);
    for block in [1, 7, 64, 333] {
        assert_eq!(a, run(&s, &x, block), "block {block}");
    }
}

#[test]
fn moving_any_setting_never_clicks() {
    let (x, _) = voice(2.0, -12.0, -10.0);
    let base = DeEssSettings::default();
    let cases = [
        ("threshold", DeEssSettings { threshold_db: -30.0, ..base }),
        ("range", DeEssSettings { range_db: 0.0, ..base }),
        ("band", DeEssSettings { lo_hz: 3000.0, hi_hz: 16_000.0, ..base }),
        ("reduce", DeEssSettings { reduce: Reduce::Band, ..base }),
        ("lookahead", DeEssSettings { lookahead: false, ..base }),
        ("detection", DeEssSettings { detection: Detection::Absolute, threshold_db: -30.0, ..base }),
    ];
    let d2 = |v: &[f32], i: usize| (v[i + 1] - 2.0 * v[i] + v[i - 1]).abs();
    let worst = |v: &[f32]| (48_000..72_000).map(|i| d2(v, i)).fold(0.0f32, f32::max);
    let steady = worst(&run(&base, &x, 480));
    for (name, after) in cases {
        let mut d = DeEsser::new(FS, &base);
        let mut y = x.clone();
        for (k, c) in y.chunks_mut(480).enumerate() {
            let s = if k * 480 >= 48_000 { after } else { base };
            d.process(&mut [c], &s);
        }
        let held = worst(&run(&after, &x, 480));
        let moving = worst(&y);
        assert!(moving <= steady.max(held) * 2.0 + 1e-4, "{name}: {moving} after {steady} (held {held})");
    }
}

#[test]
fn spectral_is_transparent_at_rest_and_says_its_latency() {
    let (x, _) = voice(1.0, -12.0, -200.0);
    let s = DeEssSettings { reduce: Reduce::Spectral, ..DeEssSettings::default() };
    let y = run(&s, &x, 333);
    let look = DeEsser::latency_for(&s, FS);
    assert_eq!(look, mite_dsp::deess::spectral_size(FS));
    let worst = y[look..].iter().zip(&x).fold(0.0f32, |m, (a, b)| m.max((a - b).abs()));
    assert!(worst < 1e-5, "{worst}");
}

#[test]
fn spectral_takes_a_sibilant_down_and_leaves_the_rest() {
    let (x, _) = voice(2.0, -12.0, -12.0);
    let s = DeEssSettings { reduce: Reduce::Spectral, range_db: 24.0, ..DeEssSettings::default() };
    let y = run(&s, &x, 512);
    let look = DeEsser::latency_for(&s, FS);
    let start = 9600 + 19_200 + 960;
    let drop = |fc: f32| band_db(&x, fc, start..start + 1920) - band_db(&y, fc, start + look..start + look + 1920);
    assert!(drop(7000.0) > 6.0, "{}", drop(7000.0));
    for fc in [440.0, 2500.0, 15_000.0] {
        assert!(drop(fc).abs() < 1.0, "{fc} Hz: {}", drop(fc));
    }
}

#[test]
fn spectral_takes_only_the_peak_of_a_peaky_sibilant() {
    // A whistle at 7 kHz over an even hiss across the band, the voice under it: the whistle is
    // brought down, the hiss beside it (9.5 kHz) barely.
    let n = 48_000;
    let mut seed = 3u32;
    let x: Vec<f32> = (0..n)
        .map(|i| {
            let t = i as f32 / FS;
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            let hiss = 0.02 * (seed as f32 / u32::MAX as f32 - 0.5);
            let vowel: f32 = (1..8).map(|k| (std::f32::consts::TAU * 200.0 * k as f32 * t).sin() / k as f32).sum::<f32>() * 0.2;
            vowel + hiss + 0.15 * (std::f32::consts::TAU * 7000.0 * t).sin()
        })
        .collect();
    let s = DeEssSettings { reduce: Reduce::Spectral, range_db: 24.0, ..DeEssSettings::default() };
    let y = run(&s, &x, 512);
    let look = DeEsser::latency_for(&s, FS);
    // Read on a fine spectrum (3 Hz bins, Hann): the whistle's own leakage stays out of the hiss.
    let drop = |y: &[f32], look: usize, fc: f32, width: f32| fine_db(&x[24_000..40_000], fc, width) - fine_db(&y[24_000 + look..40_000 + look], fc, width);
    assert!(drop(&y, look, 7000.0, 10.0) > 6.0, "whistle {}", drop(&y, look, 7000.0, 10.0));
    assert!(drop(&y, look, 9500.0, 400.0) < 1.5, "hiss beside it {}", drop(&y, look, 9500.0, 400.0));
    // The broadband band mode takes the hiss down with it.
    let band = run(&DeEssSettings { reduce: Reduce::Band, range_db: 24.0, ..DeEssSettings::default() }, &x, 512);
    let b = drop(&band, 144, 9500.0, 400.0);
    assert!(b > 3.0, "band mode on the hiss {b}");
}

#[test]
fn spectral_is_the_same_for_any_block_size_and_switches_without_a_click() {
    let (x, _) = voice(1.0, -12.0, -10.0);
    let s = DeEssSettings { reduce: Reduce::Spectral, ..DeEssSettings::default() };
    let a = run(&s, &x, 512);
    for block in [1, 7, 64, 333] {
        assert_eq!(a, run(&s, &x, block), "block {block}");
    }
    // Into the spectral mode and out: behind the dip, no step.
    let base = DeEssSettings::default();
    let d2 = |v: &[f32], i: usize| (v[i + 1] - 2.0 * v[i] + v[i - 1]).abs();
    let plain = run(&base, &x, 480);
    let steady = (24_000..40_000).map(|i| d2(&plain, i)).fold(0.0f32, f32::max);
    let mut d = DeEsser::new(FS, &base);
    let mut y = x.clone();
    for (k, c) in y.chunks_mut(480).enumerate() {
        let s = if (k * 480) >= 24_000 && (k * 480) < 33_600 { s } else { base };
        d.process(&mut [c], &s);
    }
    let moving = (24_000..40_000).map(|i| d2(&y, i)).fold(0.0f32, f32::max);
    assert!(moving <= steady * 2.0 + 1e-4, "{moving} against {steady}");
}

/// `x`'s power within `width` Hz of `fc`, dB, on its Hann-windowed DFT.
fn fine_db(x: &[f32], fc: f32, width: f32) -> f32 {
    let n = x.len();
    let res = FS / n as f32;
    let (k0, k1) = (((fc - width / 2.0) / res).floor() as usize, ((fc + width / 2.0) / res).ceil() as usize);
    let mut p = 0.0f64;
    for k in k0..=k1 {
        let w = std::f64::consts::TAU * k as f64 / n as f64;
        let (mut re, mut im) = (0.0f64, 0.0f64);
        for (i, &v) in x.iter().enumerate() {
            let h = 0.5 - 0.5 * (std::f64::consts::TAU * i as f64 / n as f64).cos();
            re += f64::from(v) * h * (w * i as f64).cos();
            im -= f64::from(v) * h * (w * i as f64).sin();
        }
        p += re * re + im * im;
    }
    (10.0 * p.max(1e-30).log10()) as f32
}
