//! Mite Sat against FabFilter Saturn 2: what each style does to a sound, measured the same way on
//! both, so the styles where Saturn 2 is ahead show up as numbers.
//!
//! The two drives do not map onto each other (Saturn's is a percentage, ours dB), so every figure
//! is read at a **matched distortion**: each style is rendered over a drive sweep, a 100 Hz sine's
//! THD+N says how hard it is driven at each step, and the figures are interpolated to the drive
//! where THD+N is −40, −30 or −20 dB (1 %, 3 %, 10 %). Saturn 2 runs one band, Dynamics, Feedback
//! and the tone controls at 0, High Quality "Good"; Mite Sat one band, auto gain on, 4×.
//!
//! What is measured, at matched distortion:
//! - **Harmonics** (sine): the even/odd balance and the share of the high orders (5th up) against
//!   the low (2nd–4th): how warm or how harsh a style's distortion is.
//! - **Tone against drive** (pink noise): the linear part of the response (H1 = Sxy/Sxx) in the
//!   lows and the highs, dB re the mids, at 1 % and at 10 %. A circuit around its nonlinearity
//!   changes its response as it is driven (a tube stage gets duller); a bare curve does not.
//! - **Memory** (a 100 Hz sine alternating −6 and −24 dBFS every 250 ms): the gain at the start
//!   of each loud and each quiet stretch against its end. A curve without state answers 0 dB;
//!   bias shift, supply sag or a hysteresis core answer with a swing that recovers.
//! - **Level** (pink noise): output against input, how well each holds the level as it drives.
//! - **Aliasing** (a 4987 Hz sine) without oversampling and with it, **CPU** and **latency**.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use realfft::RealFftPlanner;

use super::analyze::{self, db};
use super::manifest::Manifest;
use super::reaper::{num, text, Case, Rendered, Value};
use super::report::{Row, Table, Verdict};
use super::{signals, wav, Vendor};

const FAB: &str = "VST3:Saturn 2 (FabFilter)";
const MITE: &str = "VST3:Mite Sat (MiteFX)";
const RATE: u32 = 44_100;
const F0: f64 = 100.0;
const ALIAS_F0: f64 = 4987.0;
/// THD+N targets the figures are read at, dB (1 %, 3 %, 10 %).
const TARGETS: [f64; 3] = [-40.0, -30.0, -20.0];
const CPU_RUNS: usize = 3;

/// Saturn 2's styles in its order, with the Mite Sat model each is paired with (`None`: we have
/// nothing of the kind, measured for reference).
const STYLES: &[(&str, Option<&str>)] = &[
    ("Subtle Tube", Some("Tube")),
    ("Clean Tube", Some("Tube")),
    ("Warm Tube", Some("Tube")),
    ("Broken Tube", Some("Tube")),
    ("Subtle Tape", Some("Tape")),
    ("Clean Tape", Some("Tape")),
    ("Warm Tape", Some("Tape")),
    ("Old Tape", Some("Tape")),
    ("American Tweed Amp", None),
    ("American Plexi Amp", None),
    ("British Rock Amp", None),
    ("British Pop Amp", None),
    ("Smooth Amp", None),
    ("Crunchy Amp", None),
    ("Lead Amp", None),
    ("Screaming Amp", None),
    ("Power Amp", None),
    ("Subtle Saturation", Some("Soft")),
    ("Gentle Saturation", Some("Soft")),
    ("Heavy Saturation", Some("Hard")),
    ("Subtle Transformer", Some("Transformer")),
    ("Gentle Transformer", Some("Transformer")),
    ("Warm Transformer", Some("Transformer")),
    ("Smudge", None),
    ("Breakdown", None),
    ("Foldback", Some("Fold")),
    ("Rectify", Some("Rectify")),
    ("Destroy", None),
];

const MODELS: &[&str] = &["Tape", "Tube", "Transformer", "Diode", "Soft", "Hard", "Fold", "Rectify"];
/// The aliasing pairs: one Saturn style per Mite model that has one.
const ALIAS_PAIRS: &[(&str, &str)] = &[
    ("Warm Tube", "Tube"),
    ("Warm Tape", "Tape"),
    ("Warm Transformer", "Transformer"),
    ("Gentle Saturation", "Soft"),
    ("Heavy Saturation", "Hard"),
    ("Foldback", "Fold"),
    ("Rectify", "Rectify"),
];

/// Drive sweeps: Saturn 2 (%) for the sine, and the coarser one for the other signals; Mite Sat
/// (dB) for every signal.
const FAB_SINE: [f64; 11] = [0.0, 10.0, 20.0, 30.0, 40.0, 50.0, 60.0, 70.0, 80.0, 90.0, 100.0];
const FAB_REST: [f64; 6] = [0.0, 20.0, 40.0, 60.0, 80.0, 100.0];
const MITE_ALL: [f64; 16] = [0.0, 4.0, 8.0, 12.0, 16.0, 20.0, 24.0, 28.0, 32.0, 36.0, 40.0, 44.0, 48.0, 52.0, 56.0, 60.0];
const FAB_ALIAS: [f64; 5] = [0.0, 25.0, 50.0, 75.0, 100.0];
const MITE_ALIAS: [f64; 9] = [0.0, 8.0, 16.0, 24.0, 32.0, 40.0, 44.0, 52.0, 60.0];

/// One setting: a style (Saturn) or model (Mite), a drive in that plugin's units, and whether
/// to oversample (Saturn "Good" / Mite 4×) or not.
#[derive(Debug, Clone)]
struct Setting {
    style: String,
    drive: f64,
    hq: bool,
}

fn params(v: Vendor, s: &Setting, m: &Manifest) -> Vec<(String, Value)> {
    match v {
        Vendor::Fab => vec![
            num("Input Gain", 0.0),
            num("Output Gain", 0.0),
            num("Mix", 100.0),
            num("Band 1 Feedback Amount", 0.0),
            num("Band 1 Dynamics", 0.0),
            text("Band 1 Style", &s.style),
            num("Band 1 Drive", s.drive),
            num("Band 1 Bass", 0.0),
            num("Band 1 Mid", 0.0),
            num("Band 1 Treble", 0.0),
            num("Band 1 Presence", 0.0),
            num("Band 1 Mix", 100.0),
            num("Band 1 Level", 0.0),
            text("High Quality Mode", if s.hq { "Good" } else { "Off" }),
        ],
        Vendor::Mite => vec![
            m.choice("b1_model", &s.style),
            m.num("b1_drive", s.drive),
            m.num("b1_bias", 0.0),
            m.num("b1_tone", 0.0),
            m.num("b1_mix", 100.0),
            m.num("b1_out", 0.0),
            m.num("b1_dyn", 0.0),
            m.flag("auto_gain", true),
            m.choice("oversampling", if s.hq { "4×" } else { "1×" }),
            m.num("mix", 100.0),
            m.num("out_gain", 0.0),
        ],
    }
}

/// A 100 Hz sine alternating −6 and −24 dBFS every 250 ms (six of each), after 0.5 s of silence.
fn bursts(rate: u32) -> Vec<Vec<f32>> {
    let lead = (0.5 * f64::from(rate)) as usize;
    let seg = (0.25 * f64::from(rate)) as usize;
    let n = lead + 12 * seg;
    let x: Vec<f32> = (0..n)
        .map(|i| {
            if i < lead {
                return 0.0;
            }
            let level = if ((i - lead) / seg) % 2 == 0 { -6.0 } else { -24.0 };
            (db(level) * (std::f64::consts::TAU * F0 * (i - lead) as f64 / f64::from(rate)).sin()) as f32
        })
        .collect();
    vec![x.clone(), x]
}

fn signal(work: &Path, name: &str, make: &dyn Fn() -> Vec<Vec<f32>>) -> Result<PathBuf, String> {
    let p = work.join(format!("signals/{name}-{RATE}.wav"));
    if !p.exists() {
        wav::write(&p, RATE, &make())?;
    }
    Ok(p)
}

fn slug(s: &str) -> String {
    s.to_lowercase().replace(' ', "_")
}

fn id(v: Vendor, sig: &str, style: &str, drive: f64, hq: bool) -> String {
    format!("sat-{}-{sig}-{}-{drive:.0}{}", v.tag(), slug(style), if hq { "" } else { "-raw" })
}

/// Every render: (vendor, signal name, setting).
fn plan() -> Vec<(Vendor, &'static str, Setting)> {
    let mut p = Vec::new();
    let set = |style: &str, drive: f64, hq: bool| Setting { style: style.to_owned(), drive, hq };
    for (style, _) in STYLES {
        for d in FAB_SINE {
            p.push((Vendor::Fab, "sine", set(style, d, true)));
        }
        for d in FAB_REST {
            p.push((Vendor::Fab, "pink", set(style, d, true)));
            p.push((Vendor::Fab, "bursts", set(style, d, true)));
        }
    }
    for model in MODELS {
        for d in MITE_ALL {
            for sig in ["sine", "pink", "bursts"] {
                p.push((Vendor::Mite, sig, set(model, d, true)));
            }
        }
    }
    for (style, model) in ALIAS_PAIRS {
        for hq in [false, true] {
            for d in FAB_ALIAS {
                p.push((Vendor::Fab, "alias", set(style, d, hq)));
            }
            for d in MITE_ALIAS {
                p.push((Vendor::Mite, "alias", set(model, d, hq)));
            }
        }
        // The aliasing cases' distortion: the same drive points on the 100 Hz sine, with and
        // without oversampling (a case also in a sweep is rendered once).
        for hq in [false, true] {
            for d in FAB_ALIAS {
                p.push((Vendor::Fab, "sine", set(style, d, hq)));
            }
            for d in MITE_ALIAS {
                p.push((Vendor::Mite, "sine", set(model, d, hq)));
            }
        }
    }
    for k in 0..CPU_RUNS {
        for hq in [false, true] {
            p.push((Vendor::Fab, if k == 0 { "cpu" } else { leak(format!("cpu{k}")) }, set("Warm Tube", 50.0, hq)));
            p.push((Vendor::Mite, if k == 0 { "cpu" } else { leak(format!("cpu{k}")) }, set("Tube", 42.0, hq)));
        }
    }
    p
}

/// A signal name that lives as long as the plan (the CPU runs' names).
fn leak(s: String) -> &'static str {
    Box::leak(s.into_boxed_str())
}

fn signal_file(work: &Path, sig: &str) -> Result<PathBuf, String> {
    match sig.trim_end_matches(char::is_numeric) {
        "sine" => signal(work, "sine100", &|| signals::sine(RATE, F0, -12.0, 1.5)),
        "pink" => signal(work, "pink4", &|| signals::pink_noise(RATE, 4.0)),
        "bursts" => signal(work, "bursts", &|| bursts(RATE)),
        "alias" => signal(work, "sinealias", &|| signals::sine(RATE, ALIAS_F0, -6.0, 1.5)),
        _ => signal(work, "pink20", &|| signals::pink_noise(RATE, 20.0)),
    }
}

fn input_name(sig: &str) -> &'static str {
    match sig.trim_end_matches(char::is_numeric) {
        "sine" => "sine100",
        "pink" => "pink4",
        "bursts" => "bursts",
        "alias" => "sinealias",
        _ => "pink20",
    }
}

pub fn cases(work: &Path, m: &Manifest) -> Result<Vec<Case>, String> {
    let mut cases = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for (v, sig, s) in plan() {
        if !seen.insert(id(v, sig, &s.style, s.drive, s.hq)) {
            continue;
        }
        let fx = match v {
            Vendor::Fab => FAB,
            Vendor::Mite => MITE,
        };
        cases.push(Case { id: id(v, sig, &s.style, s.drive, s.hq), fx: fx.to_owned(), signal: signal_file(work, sig)?, rate: RATE, params: params(v, &s, m) });
    }
    Ok(cases)
}

type Audio = Vec<Vec<f32>>;

fn io(work: &Path, v: Vendor, sig: &str, style: &str, drive: f64, hq: bool) -> Result<(Audio, Audio), String> {
    let (_, input) = wav::read(&work.join(format!("signals/{}-{RATE}.wav", input_name(sig))))?;
    let (_, output) = wav::read(&work.join(format!("renders/{}.wav", id(v, sig, style, drive, hq))))?;
    Ok((input, output))
}

/// The power of each harmonic of `f0` (1 to the last under 20 kHz) in `x`'s last 2¹⁶ samples
/// (Blackman-Harris, ±4 bins).
fn harmonics(x: &[f32], f0: f64) -> Vec<f64> {
    let n = 1usize << 16;
    let x = &x[x.len().saturating_sub(n)..];
    let n = x.len();
    let w = |i: usize| {
        let t = std::f64::consts::TAU * i as f64 / n as f64;
        0.35875 - 0.48829 * t.cos() + 0.14128 * (2.0 * t).cos() - 0.01168 * (3.0 * t).cos()
    };
    let mut buf: Vec<f64> = x.iter().enumerate().map(|(i, v)| f64::from(*v) * w(i)).collect();
    let fft = RealFftPlanner::<f64>::new().plan_fft_forward(n);
    let mut spec = fft.make_output_vec();
    let _ = fft.process(&mut buf, &mut spec);
    let hz = f64::from(RATE) / n as f64;
    let mut out = Vec::new();
    let mut k = 1;
    while k as f64 * f0 < 20_000.0 {
        let c = (k as f64 * f0 / hz).round() as usize;
        out.push(spec[c.saturating_sub(4)..(c + 5).min(spec.len())].iter().map(|z| z.norm_sqr()).sum());
        k += 1;
    }
    out
}

/// Even against odd harmonics (dB; positive: more even), and the 5th and up against the 2nd–4th
/// (dB; higher: harsher).
fn character(h: &[f64]) -> (f64, f64) {
    let (mut even, mut odd, mut low, mut high) = (0.0, 0.0, 0.0, 0.0);
    for (i, p) in h.iter().enumerate().skip(1) {
        let k = i + 1;
        if k % 2 == 0 { even += p } else { odd += p }
        if k <= 4 { low += p } else { high += p }
    }
    (10.0 * (even.max(1e-30) / odd.max(1e-30)).log10(), 10.0 * (high.max(1e-30) / low.max(1e-30)).log10())
}

/// The linear part of the response from `x` to `y` (H1 = Sxy/Sxx, Hann windows of 4096, half
/// overlapping, after the first 0.5 s): its level (dB) over 60–150 Hz and 5–10 kHz against
/// 800–1250 Hz.
fn tilt(x: &[f32], y: &[f32]) -> (f64, f64) {
    let n = 4096;
    let fft = RealFftPlanner::<f64>::new().plan_fft_forward(n);
    let w: Vec<f64> = (0..n).map(|i| 0.5 - 0.5 * (std::f64::consts::TAU * i as f64 / n as f64).cos()).collect();
    let bins = n / 2 + 1;
    let (mut sxy, mut sxx) = (vec![realfft::num_complex::Complex::new(0.0, 0.0); bins], vec![0.0f64; bins]);
    let len = x.len().min(y.len());
    let mut at = (0.5 * f64::from(RATE)) as usize;
    while at + n <= len {
        let mut a: Vec<f64> = (0..n).map(|i| f64::from(x[at + i]) * w[i]).collect();
        let mut b: Vec<f64> = (0..n).map(|i| f64::from(y[at + i]) * w[i]).collect();
        let (mut sa, mut sb) = (fft.make_output_vec(), fft.make_output_vec());
        let _ = fft.process(&mut a, &mut sa);
        let _ = fft.process(&mut b, &mut sb);
        for k in 0..bins {
            sxy[k] += sb[k] * sa[k].conj();
            sxx[k] += sa[k].norm_sqr();
        }
        at += n / 2;
    }
    let hz = f64::from(RATE) / n as f64;
    let band = |lo: f64, hi: f64| {
        let (a, b) = ((lo / hz).ceil() as usize, (hi / hz).floor() as usize);
        let m: f64 = (a..=b).map(|k| sxy[k].norm() / sxx[k].max(1e-30)).sum::<f64>() / (b - a + 1) as f64;
        db(m)
    };
    let mid = band(800.0, 1250.0);
    (band(60.0, 150.0) - mid, band(5000.0, 10_000.0) - mid)
}

/// The bursts' memory: (loud start − loud end, quiet start − quiet end), dB, averaged over the
/// last four of each (periods 1–2 of a stretch against its last five).
fn memory(input: &[f32], output: &[f32]) -> (f64, f64) {
    let lag = analyze::lag(input, output, 2000);
    let shift = |k: usize| ((k as i64) + lag).clamp(0, output.len() as i64 - 1) as usize;
    let period = (f64::from(RATE) / F0).round() as usize;
    let lead = (0.5 * f64::from(RATE)) as usize;
    let seg = (0.25 * f64::from(RATE)) as usize;
    let gain = |a: usize, b: usize| {
        let ri: f64 = (a..b).map(|k| f64::from(input[k]).powi(2)).sum();
        let ro: f64 = (a..b).map(|k| f64::from(output[shift(k)]).powi(2)).sum();
        10.0 * (ro.max(1e-30) / ri.max(1e-30)).log10()
    };
    let swing = |s: usize| {
        let a = lead + s * seg;
        let start = gain(a + period, a + 3 * period);
        let end = gain(a + seg - 5 * period, a + seg);
        start - end
    };
    let avg = |first: usize| (0..4).map(|c| swing(first + 2 * (c + 2))).sum::<f64>() / 4.0;
    (avg(0), avg(1))
}

fn rms_db(x: &[f32]) -> f64 {
    let s: f64 = x.iter().map(|v| f64::from(*v).powi(2)).sum();
    db((s / x.len().max(1) as f64).sqrt())
}

/// `points` (THD+N, figure) along a drive sweep, read at THD+N `target` (linear between the
/// first two neighbours that bracket it); `None` where the sweep never reaches it.
fn at_thd(points: &[(f64, f64)], target: f64) -> Option<f64> {
    points.windows(2).find_map(|w| {
        let ((t0, v0), (t1, v1)) = (w[0], w[1]);
        let (lo, hi) = (t0.min(t1), t0.max(t1));
        (lo <= target && target <= hi && hi > lo).then(|| v0 + (v1 - v0) * (target - t0) / (t1 - t0))
    })
}

fn cell(x: Option<f64>, decimals: usize) -> String {
    x.map_or("–".to_owned(), |v| format!("{v:+.decimals$}"))
}

/// Everything measured for one style or model.
#[derive(Default)]
struct Profile {
    /// (drive, THD+N) on the oversampled sine sweep.
    thd: Vec<(f64, f64)>,
    even_odd: Vec<(f64, f64)>,
    high_order: Vec<(f64, f64)>,
    low: Vec<(f64, f64)>,
    high: Vec<(f64, f64)>,
    attack: Vec<(f64, f64)>,
    recovery: Vec<(f64, f64)>,
    level: Vec<(f64, f64)>,
}

fn profile(work: &Path, v: Vendor, style: &str, sine: &[f64], rest: &[f64]) -> Result<Profile, String> {
    let mut p = Profile::default();
    let mut thd_at = BTreeMap::new();
    for &d in sine {
        let (_, o) = io(work, v, "sine", style, d, true)?;
        let t = analyze::thd_n(&o[0], RATE, F0);
        thd_at.insert((d * 10.0) as i64, t);
        p.thd.push((d, t));
        let (eo, ho) = character(&harmonics(&o[0], F0));
        p.even_odd.push((t, eo));
        p.high_order.push((t, ho));
    }
    for &d in rest {
        let Some(&t) = thd_at.get(&((d * 10.0) as i64)) else { continue };
        let (i, o) = io(work, v, "pink", style, d, true)?;
        let (lo, hi) = tilt(&i[0], &o[0]);
        p.low.push((t, lo));
        p.high.push((t, hi));
        let skip = (0.5 * f64::from(RATE)) as usize;
        p.level.push((t, rms_db(&o[0][skip..]) - rms_db(&i[0][skip..])));
        let (i, o) = io(work, v, "bursts", style, d, true)?;
        let (a, r) = memory(&i[0], &o[0]);
        p.attack.push((t, a));
        p.recovery.push((t, r));
    }
    Ok(p)
}

/// The THD+N of the alias cases' drive points (100 Hz sine without oversampling), then the
/// aliasing at `target` THD+N, oversampled or not.
fn alias_at(work: &Path, v: Vendor, style: &str, drives: &[f64], hq: bool, target: f64) -> Result<Option<f64>, String> {
    let mut points = Vec::new();
    for &d in drives {
        let (_, s) = io(work, v, "sine", style, d, hq)?;
        let (_, a) = io(work, v, "alias", style, d, hq)?;
        points.push((analyze::thd_n(&s[0], RATE, F0), analyze::aliasing(&a[0], RATE, ALIAS_F0)));
    }
    Ok(at_thd(&points, target))
}

pub fn tables(work: &Path, rendered: &BTreeMap<String, Rendered>) -> Result<Vec<Table>, String> {
    let mut fab = BTreeMap::new();
    for (style, _) in STYLES {
        fab.insert(*style, profile(work, Vendor::Fab, style, &FAB_SINE, &FAB_REST)?);
    }
    let mut mite = BTreeMap::new();
    for model in MODELS {
        mite.insert(*model, profile(work, Vendor::Mite, model, &MITE_ALL, &MITE_ALL)?);
    }
    let mut tables = Vec::new();

    // Reach: how much distortion each drive range covers.
    let mut reach = Table::new(
        "Reach: THD+N of a 100 Hz sine at −12 dBFS across the whole drive range",
        "dB re the fundamental at the lowest and highest drive (Saturn 2 0–100 %, Mite Sat 0–60 dB): how clean a style starts and how far it goes. A figure below is read only where both sweeps reach that distortion.",
        &["Saturn 2 style ↔ Mite Sat model", "Saturn min", "Saturn max", "Mite min", "Mite max"],
    );
    for (style, model) in STYLES {
        let f = &fab[style];
        let ends = |p: &Profile| (p.thd.first().map(|x| x.1), p.thd.last().map(|x| x.1));
        let (fa, fb) = ends(f);
        let (ma, mb) = model.map_or((None, None), |m| ends(&mite[m]));
        reach.rows.push(Row { cells: vec![format!("{style} ↔ {}", model.unwrap_or("—")), cell(fa, 1), cell(fb, 1), cell(ma, 1), cell(mb, 1)], verdict: Verdict::None });
    }
    tables.push(reach);

    // Harmonic character.
    let mut harm = Table::new(
        "Harmonics at matched distortion (100 Hz sine, −12 dBFS), at 3 % · 10 % THD+N",
        "**Even/odd**: the even harmonics' power against the odd ones' (dB; positive is even-led, warmer; very negative is a symmetric, odd-led curve). **High**: the 5th harmonic and up against the 2nd–4th (dB; higher is harsher, fizzier distortion). Readings, not a contest; **harsher** marks a Mite Sat model with 10 dB or more high harmonics than the Saturn 2 style at the same distortion.",
        &["Saturn 2 style ↔ Mite Sat", "Saturn even/odd", "Mite even/odd", "Saturn high", "Mite high", ""],
    );
    for (style, model) in STYLES {
        let f = &fab[style];
        let pair = |p: &[(f64, f64)]| format!("{} · {}", cell(at_thd(p, TARGETS[1]), 1), cell(at_thd(p, TARGETS[2]), 1));
        let (me, mh, flag) = match model {
            Some(m) => {
                let p = &mite[m];
                let harsher = [TARGETS[1], TARGETS[2]].iter().any(|&t| matches!((at_thd(&f.high_order, t), at_thd(&p.high_order, t)), (Some(a), Some(b)) if b >= a + 10.0));
                (pair(&p.even_odd), pair(&p.high_order), if harsher { "**harsher**" } else { "" })
            }
            None => ("—".to_owned(), "—".to_owned(), ""),
        };
        harm.rows.push(Row { cells: vec![format!("{style} ↔ {}", model.unwrap_or("—")), pair(&f.even_odd), me, pair(&f.high_order), mh, flag.to_owned()], verdict: Verdict::None });
    }
    tables.push(harm);

    // Tone against drive: where a circuit is ahead of a curve.
    let mut tone = Table::new(
        "Tone against drive (pink noise): the response's lows and highs, dB re 1 kHz, at 3 % → 10 % THD+N",
        "The linear part of each style's response (what passes through, harmonics aside). A curve alone keeps it flat at any drive; a modelled circuit moves it as it is pushed (a tube stage loses highs, tape thickens the lows). **gap** marks a style whose lows or highs move by 1 dB or more between 3 % and 10 % while its Mite Sat pair moves by less than half as much: a behaviour we do not have.",
        &["Saturn 2 style ↔ Mite Sat", "Saturn lows", "Saturn highs", "Mite lows", "Mite highs", ""],
    );
    let moves = |p: &[(f64, f64)]| Some(at_thd(p, TARGETS[2])? - at_thd(p, TARGETS[1])?);
    let span = |p: &[(f64, f64)]| format!("{} → {}", cell(at_thd(p, TARGETS[1]), 1), cell(at_thd(p, TARGETS[2]), 1));
    for (style, model) in STYLES {
        let f = &fab[style];
        let (ml, mh, gap) = match model {
            Some(m) => {
                let p = &mite[m];
                let gap = [(moves(&f.low), moves(&p.low)), (moves(&f.high), moves(&p.high))]
                    .iter()
                    .any(|(a, b)| matches!((a, b), (Some(a), Some(b)) if a.abs() >= 1.0 && b.abs() < 0.5 * a.abs()));
                (span(&p.low), span(&p.high), if gap { "**gap**" } else { "" })
            }
            None => ("—".to_owned(), "—".to_owned(), ""),
        };
        tone.rows.push(Row { cells: vec![format!("{style} ↔ {}", model.unwrap_or("—")), span(&f.low), span(&f.high), ml, mh, gap.to_owned()], verdict: Verdict::None });
    }
    tables.push(tone);

    // Memory.
    let mut mem = Table::new(
        "Memory at 10 % THD+N (100 Hz, −6 / −24 dBFS every 250 ms)",
        "The gain at the start of a loud stretch against its end (**attack**), and of a quiet stretch after a loud one against its end (**recovery**), dB. 0 is a device without state: the same level always gets the same gain. A swing that settles is bias shift, supply sag or a magnetic core remembering. **gap** marks a style with a swing of 0.5 dB or more whose Mite Sat pair swings less than half as much.",
        &["Saturn 2 style ↔ Mite Sat", "Saturn attack", "Saturn recovery", "Mite attack", "Mite recovery", ""],
    );
    for (style, model) in STYLES {
        let f = &fab[style];
        let t = TARGETS[2];
        let (fa, fr) = (at_thd(&f.attack, t), at_thd(&f.recovery, t));
        let (ma, mr, gap) = match model {
            Some(m) => {
                let (a, r) = (at_thd(&mite[m].attack, t), at_thd(&mite[m].recovery, t));
                let gap = [(fa, a), (fr, r)].iter().any(|(s, o)| matches!((s, o), (Some(s), Some(o)) if s.abs() >= 0.5 && o.abs() < 0.5 * s.abs()));
                (cell(a, 2), cell(r, 2), if gap { "**gap**" } else { "" })
            }
            None => ("—".to_owned(), "—".to_owned(), ""),
        };
        mem.rows.push(Row { cells: vec![format!("{style} ↔ {}", model.unwrap_or("—")), cell(fa, 2), cell(fr, 2), ma, mr, gap.to_owned()], verdict: Verdict::None });
    }
    tables.push(mem);

    // Level hold.
    let mut level = Table::new(
        "Level against drive (pink noise, −18 dBFS RMS): output − input, dB",
        "Both make up their level as they drive (Saturn 2 always, Mite Sat with auto gain). The worse of the readings at 1 % and 10 % THD+N, nearer 0 dB is better; **close** is within 1 dB of Saturn 2.",
        &["Saturn 2 style ↔ Mite Sat", "Saturn 3 % · 10 %", "Mite 3 % · 10 %", "Verdict"],
    );
    for (style, model) in STYLES {
        let Some(m) = model else { continue };
        let read = |p: &[(f64, f64)]| (at_thd(p, TARGETS[1]), at_thd(p, TARGETS[2]));
        let (f, o) = (read(&fab[style].level), read(&mite[m].level));
        let worst = |(a, b): (Option<f64>, Option<f64>)| a.into_iter().chain(b).map(f64::abs).fold(None, |m: Option<f64>, x| Some(m.map_or(x, |m| m.max(x))));
        let verdict = match (worst(f), worst(o)) {
            (Some(a), Some(b)) => Verdict::lower((b * 10.0).round() / 10.0, (a * 10.0).round() / 10.0, 1.0),
            _ => Verdict::None,
        };
        level.rows.push(Row { cells: vec![format!("{style} ↔ {m}"), format!("{} · {}", cell(f.0, 1), cell(f.1, 1)), format!("{} · {}", cell(o.0, 1), cell(o.1, 1))], verdict });
    }
    tables.push(level);

    // Aliasing.
    let mut alias = Table::new(
        "Aliasing: a 4987 Hz sine at −6 dBFS, at the drive giving 10 % THD+N on the 100 Hz sine",
        "Energy away from the tone and its harmonics (what folded back from above Nyquist), dB re the fundamental; lower is cleaner. Without oversampling (Saturn 2 High Quality Off, Mite Sat 1×) and with it (Good, 4×). **close** is within 3 dB.",
        &["Saturn 2 style ↔ Mite Sat", "Saturn off", "Mite 1×", "Saturn Good", "Mite 4×", "Verdict (oversampled)"],
    );
    for (style, model) in ALIAS_PAIRS {
        let t = TARGETS[2];
        let (fr, mr) = (alias_at(work, Vendor::Fab, style, &FAB_ALIAS, false, t)?, alias_at(work, Vendor::Mite, model, &MITE_ALIAS, false, t)?);
        let (fh, mh) = (alias_at(work, Vendor::Fab, style, &FAB_ALIAS, true, t)?, alias_at(work, Vendor::Mite, model, &MITE_ALIAS, true, t)?);
        let verdict = match (fh, mh) {
            (Some(a), Some(b)) => Verdict::lower(b.round(), a.round(), 3.0),
            _ => Verdict::None,
        };
        alias.rows.push(Row { cells: vec![format!("{style} ↔ {model}"), cell(fr, 1), cell(mr, 1), cell(fh, 1), cell(mh, 1)], verdict });
    }
    tables.push(alias);

    // CPU and latency.
    let mut cpu = Table::new(
        "Render time (rough CPU) and latency: 20 s of pink noise, Warm Tube ↔ Tube, mid drive",
        "Offline render time, the fastest of three; Saturn 2 runs through wine and yabridge, so its figure includes the bridge: indicative only. **close** is within 10 %. Latency as each reports it.",
        &["", "Saturn 2", "Mite Sat", "Verdict"],
    );
    for hq in [false, true] {
        let s = |v: Vendor, style: &str, d: f64| {
            (0..CPU_RUNS)
                .filter_map(|k| rendered.get(&id(v, &if k == 0 { "cpu".to_owned() } else { format!("cpu{k}") }, style, d, hq)))
                .map(|r| r.seconds)
                .fold(f64::NAN, f64::min)
        };
        let (f, m) = (s(Vendor::Fab, "Warm Tube", 50.0), s(Vendor::Mite, "Tube", 42.0));
        cpu.push(Row::lower(if hq { "seconds, Good ↔ 4×" } else { "seconds, Off ↔ 1×" }, f, m, 0.1 * f, 2));
        let lat = |v: Vendor, style: &str, d: f64| rendered.get(&id(v, "cpu", style, d, hq)).map_or(0, |r| r.latency);
        cpu.rows.push(Row { cells: vec![format!("latency, {}", if hq { "Good ↔ 4×" } else { "Off ↔ 1×" }), format!("{} smp", lat(Vendor::Fab, "Warm Tube", 50.0)), format!("{} smp", lat(Vendor::Mite, "Tube", 42.0))], verdict: Verdict::None });
    }
    tables.push(cpu);
    Ok(tables)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_static_curve_has_no_memory_and_tanh_is_odd() {
        let x = bursts(RATE).remove(0);
        let y: Vec<f32> = x.iter().map(|v| (3.0 * v).tanh()).collect();
        let (a, r) = memory(&x, &y);
        assert!(a.abs() < 0.05 && r.abs() < 0.05, "{a} {r}");
        let s = signals::sine(RATE, F0, -6.0, 1.5).remove(0);
        let (eo, _) = character(&harmonics(&s.iter().map(|v| (3.0 * v).tanh()).collect::<Vec<_>>(), F0));
        assert!(eo < -60.0, "a symmetric curve makes no evens: {eo}");
        let (eo, _) = character(&harmonics(&s.iter().map(|v| (3.0 * v + 0.3).tanh() - 0.3f32.tanh()).collect::<Vec<_>>(), F0));
        assert!(eo > -20.0, "an offset one does: {eo}");
    }

    #[test]
    fn a_gain_has_a_flat_response() {
        let x = signals::pink_noise(RATE, 4.0).remove(0);
        let y: Vec<f32> = x.iter().map(|v| 0.5 * v).collect();
        let (lo, hi) = tilt(&x, &y);
        assert!(lo.abs() < 0.05 && hi.abs() < 0.05, "{lo} {hi}");
    }

    #[test]
    fn reading_at_a_distortion_interpolates_along_the_sweep() {
        let p = [(-50.0, 0.0), (-30.0, 2.0), (-10.0, 4.0)];
        assert_eq!(at_thd(&p, -40.0), Some(1.0));
        assert_eq!(at_thd(&p, -60.0), None);
    }
}
