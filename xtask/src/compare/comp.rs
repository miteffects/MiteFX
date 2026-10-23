//! Mite Comp against FabFilter Pro-C 3. Both are set the same way (threshold, ratio, knee, attack,
//! release in the same units); each Pro-C 3 style is paired with our nearest Character.

use std::path::Path;

use super::analyze::{self, db};
use super::manifest::Manifest;
use super::reaper::{num, text, Case, Rendered, Value};
use super::report::{Row, Table, Verdict};
use super::{signals, wav, Vendor};

const FAB: &str = "VST3:Pro-C 3 (FabFilter)";
const MITE: &str = "VST3:Mite Comp (MiteFX)";
const RATE: u32 = 44_100;
/// Pro-C 3 style ↔ Mite Comp Character.
const PAIRS: &[(&str, &str)] = &[("Clean", "Clean"), ("Punch", "Punch"), ("Opto", "Opto"), ("Bus", "Bus"), ("Vari-Mu", "Vari-Mu")];
/// The timing test: the tone at −40, −10, then −40 dBFS.
const TIMING_STEPS: &[(f64, f64)] = &[(-200.0, 0.5), (-40.0, 1.0), (-10.0, 1.0), (-40.0, 2.0)];

/// One compressor setting, in both plugins' units.
#[derive(Debug, Clone, Copy)]
struct Setting {
    style: usize,
    threshold: f64,
    ratio: f64,
    knee: f64,
    attack: f64,
    release: f64,
    lookahead: f64,
    link: f64,
    oversampling: Option<f64>,
    range: f64,
    auto_makeup: bool,
}

const BASE: Setting = Setting { style: 0, threshold: -20.0, ratio: 4.0, knee: 0.0, attack: 1.0, release: 50.0, lookahead: 0.0, link: 100.0, oversampling: None, range: 60.0, auto_makeup: false };

fn params(v: Vendor, s: &Setting, m: &Manifest) -> Vec<(String, Value)> {
    let (fab_style, mite_char) = PAIRS[s.style];
    match v {
        Vendor::Fab => {
            let mut p = vec![
                text("Style", fab_style),
                num("Threshold", s.threshold),
                text("Auto Threshold", "Off"),
                num("Ratio", s.ratio),
                num("Knee", s.knee),
                num("Range", s.range),
                num("Attack", s.attack),
                num("Release", s.release),
                text("Auto Release", "Off"),
                num("Hold", 0.0),
                text("Character", "Off"),
                num("Wet Gain", 0.0),
                num("Dry Gain", -1e9),
                text("Auto Gain", if s.auto_makeup { "On" } else { "Off" }),
                num("Side Chain Level", 0.0),
                num("Stereo Link", s.link),
                num("Mix", 100.0),
                num("Input Level", 0.0),
                num("Output Level", 0.0),
            ];
            for b in 1..=6 {
                p.push(text(&format!("Side Chain EQ Band {b} Used"), "Unused"));
            }
            match s.oversampling {
                Some(x) => p.push(num("Oversampling", x)),
                None => p.push(text("Oversampling", "Off")),
            }
            if s.lookahead > 0.0 {
                p.push(num("Maximum Lookahead", 20.0));
            } else {
                p.push(text("Maximum Lookahead", "Off"));
            }
            p.push(num("Lookahead", s.lookahead));
            p
        }
        Vendor::Mite => vec![
            m.choice("character", mite_char),
            m.choice("smart", "Off"),
            m.choice("topology", "Feed-forward"),
            m.choice("stereo", "L/R"),
            m.num("link", s.link),
            m.num("lookahead", s.lookahead),
            m.num("hold", 0.0),
            m.num("makeup", 0.0),
            m.flag("auto_makeup", s.auto_makeup),
            m.choice("sc_source", "Internal"),
            m.num("mix", 100.0),
            m.num("out_gain", 0.0),
            m.flag("s1_on", true),
            m.choice("s1_quad", "Compress"),
            m.num("s1_thresh", s.threshold),
            m.num("s1_ratio", s.ratio),
            m.num("s1_knee", s.knee),
            m.num("s1_attack", s.attack),
            m.num("s1_release", s.release),
            m.num("s1_range", s.range),
            m.choice("s1_det", "Peak"),
            m.flag("s2_on", false),
            m.flag("s3_on", false),
            m.flag("s4_on", false),
        ],
    }
}

fn fx(v: Vendor) -> &'static str {
    match v {
        Vendor::Fab => FAB,
        Vendor::Mite => MITE,
    }
}

/// The comparison's cases: (name, setting, signal file).
fn plan(work: &Path) -> Result<Vec<(String, Setting, std::path::PathBuf)>, String> {
    let sig = |name: &str, make: &dyn Fn() -> Vec<Vec<f32>>| -> Result<std::path::PathBuf, String> {
        let p = work.join(format!("signals/{name}-{RATE}.wav"));
        if !p.exists() {
            wav::write(&p, RATE, &make())?;
        }
        Ok(p)
    };
    let stepped = sig("stepped", &|| signals::stepped_tone(RATE))?;
    let timing = sig("timing", &|| signals::tone_steps(RATE, TIMING_STEPS))?;
    let hits = sig("hits", &|| signals::hits(RATE, -3.0, 4.0))?;
    let bass = sig("sine50", &|| signals::sine(RATE, 50.0, -6.0, 3.0))?;
    let split = sig("split", &|| signals::split_tone(RATE, -10.0, -40.0, 2.0))?;
    let noise = sig("pink", &|| signals::pink_noise(RATE, 20.0))?;
    let mut p = vec![
        ("static-hard".to_owned(), BASE, stepped.clone()),
        ("static-knee".to_owned(), Setting { knee: 12.0, ..BASE }, stepped.clone()),
        ("range".to_owned(), Setting { threshold: -40.0, ratio: 20.0, range: 6.0, ..BASE }, stepped.clone()),
        ("automakeup".to_owned(), Setting { auto_makeup: true, ..BASE }, stepped),
    ];
    for k in 0..PAIRS.len() {
        p.push((format!("timing-{k}"), Setting { style: k, threshold: -30.0, ratio: 10.0, attack: 10.0, release: 100.0, ..BASE }, timing.clone()));
    }
    let limit = Setting { threshold: -30.0, ratio: 20.0, attack: 0.05, release: 50.0, ..BASE };
    p.push(("hits-nolook".to_owned(), limit, hits.clone()));
    p.push(("hits-look5".to_owned(), Setting { lookahead: 5.0, ..limit }, hits));
    // Pro-C 3's shortest release is 10 ms.
    let fast = Setting { threshold: -30.0, ratio: 4.0, attack: 0.05, release: 10.0, ..BASE };
    p.push(("thd".to_owned(), fast, bass.clone()));
    p.push(("thd-os4".to_owned(), Setting { oversampling: Some(4.0), ..fast }, bass));
    let high = sig("sine5k", &|| signals::sine(RATE, 5000.0, -6.0, 3.0))?;
    p.push(("alias".to_owned(), fast, high.clone()));
    p.push(("alias-os4".to_owned(), Setting { oversampling: Some(4.0), ..fast }, high));
    p.push(("link100".to_owned(), Setting { threshold: -30.0, ..BASE }, split.clone()));
    p.push(("link0".to_owned(), Setting { threshold: -30.0, link: 0.0, ..BASE }, split));
    p.push(("cpu".to_owned(), Setting { threshold: -30.0, attack: 10.0, release: 100.0, ..BASE }, noise));
    Ok(p)
}

fn id(v: Vendor, name: &str) -> String {
    format!("comp-{}-{name}", v.tag())
}

pub fn cases(work: &Path, m: &Manifest) -> Result<Vec<Case>, String> {
    let mut cases = Vec::new();
    for (name, s, signal) in plan(work)? {
        for v in [Vendor::Fab, Vendor::Mite] {
            // Our oversampling is not built yet: its case runs ours at 1×.
            cases.push(Case { id: id(v, &name), fx: fx(v).to_owned(), signal: signal.clone(), rate: RATE, params: params(v, &s, m) });
        }
    }
    Ok(cases)
}

/// Channels of audio.
type Audio = Vec<Vec<f32>>;

fn io(work: &Path, v: Vendor, name: &str, signal: &str) -> Result<(Audio, Audio), String> {
    let (_, input) = wav::read(&work.join(format!("signals/{signal}-{RATE}.wav")))?;
    let (_, output) = wav::read(&work.join(format!("renders/{}.wav", id(v, name))))?;
    Ok((input, output))
}

/// (threshold, ratio) of a static curve, from its asymptote over the top 12 dB (well past the
/// threshold at −20, past any knee): the line `out = T + (in − T) / R` through it.
fn curve_shape(gains: &[f64]) -> (f64, f64) {
    let levels: Vec<f64> = signals::STEP_LEVELS_DB.map(f64::from).collect();
    let n = levels.len();
    let (a, b) = (n - 13, n - 1);
    let (out_a, out_b) = (levels[a] + gains[a], levels[b] + gains[b]);
    let slope = (out_b - out_a) / (levels[b] - levels[a]);
    if slope >= 1.0 - 1e-6 {
        return (f64::NAN, 1.0);
    }
    // out_b = T + (in_b − T)·slope  →  T = (out_b − in_b·slope) / (1 − slope).
    let threshold = (out_b - levels[b] * slope) / (1.0 - slope);
    (threshold, if slope > 1e-3 { 1.0 / slope } else { f64::INFINITY })
}

/// Attack and release times (ms, to 63 % of the change) and the steady reduction (dB) of a
/// timing render.
fn times(env: &[f64]) -> (f64, f64, f64) {
    let per_s = f64::from(RATE) / 50.0;
    let at = |s: f64| (s * per_s) as usize;
    let mean = |a: f64, b: f64| env[at(a)..at(b)].iter().sum::<f64>() / (at(b) - at(a)) as f64;
    let loud = mean(2.3, 2.5);
    let attack = env[at(1.5)..at(2.5)].iter().position(|g| *g <= 0.63 * loud).map_or(f64::NAN, |i| i as f64 * 1000.0 / per_s);
    let before = env[at(2.5) - 1];
    let quiet = mean(4.2, 4.5);
    let release = env[at(2.5)..].iter().position(|g| *g >= before + 0.63 * (quiet - before)).map_or(f64::NAN, |i| i as f64 * 1000.0 / per_s);
    (attack, release, -loud)
}

pub fn tables(work: &Path, rendered: &std::collections::BTreeMap<String, Rendered>) -> Result<Vec<Table>, String> {
    let both = [Vendor::Fab, Vendor::Mite];
    let mut tables = Vec::new();

    // Static curve.
    let mut stat = Table::new(
        "Static curve: threshold −20 dB, ratio 4:1 (Clean)",
        "A tone held at every level from −60 to 0 dBFS (peak). Threshold and ratio come from the curve's asymptote over the top 12 dB (the line out = T + (in − T)/R), so a soft knee is not counted against it. Errors are against the set values (−20 dB, 4:1); lower is better, and the verdict weighs a ratio error of 0.5 like 1 dB of threshold. Mite Comp uses its Peak detector here; a sine's RMS is 3 dB under its peak, so a detector that reads RMS acts 3 dB later.",
        &["Knee · plugin", "Threshold", "Ratio", "Threshold error", "Ratio error", "Verdict"],
    );
    let mut curve = Table::new("Static curve: gain at each input level (dB)", "The measured gain at a few input levels, hard knee and 12 dB knee.", &["Input (dBFS)", "Pro-C 3 hard", "Mite hard", "Pro-C 3 12 dB knee", "Mite 12 dB knee"]);
    let mut curves = Vec::new();
    for (name, label) in [("static-hard", "hard knee"), ("static-knee", "12 dB knee")] {
        let mut errs = Vec::new();
        for v in both {
            let (i, o) = io(work, v, name, "stepped")?;
            let g = analyze::step_gains(&i[0], &o[0], RATE, 0.5, signals::STEP_S, signals::STEP_LEVELS_DB.count());
            let (k, r) = curve_shape(&g);
            errs.push(((k + 20.0).abs(), (r - 4.0).abs(), k, r));
            curves.push(g);
        }
        for (j, v) in both.into_iter().enumerate() {
            let (te, re, k, r) = errs[j];
            let verdict = if v == Vendor::Mite { Verdict::lower(te.max(re * 2.0), errs[0].0.max(errs[0].1 * 2.0), 0.5) } else { Verdict::None };
            stat.rows.push(Row { cells: vec![format!("{label} · {}", v.name("Pro-C 3", "Mite Comp")), format!("{k:.1} dB"), format!("{r:.2}:1"), format!("{te:.1} dB"), format!("{re:.2}")], verdict });
        }
    }
    for level in [-30, -24, -20, -16, -12, -6, 0] {
        let k = (level - signals::STEP_LEVELS_DB.start()) as usize;
        curve.rows.push(Row { cells: std::iter::once(format!("{level}")).chain(curves.iter().map(|g| format!("{:.2}", g[k]))).collect(), verdict: Verdict::None });
    }
    tables.extend([stat, curve]);

    // Range and auto makeup, on the same stepped tone.
    let gains = |v: Vendor, name: &str| -> Result<Vec<f64>, String> {
        let (i, o) = io(work, v, name, "stepped")?;
        Ok(analyze::step_gains(&i[0], &o[0], RATE, 0.5, signals::STEP_S, signals::STEP_LEVELS_DB.count()))
    };
    let at = |g: &[f64], level: i32| g[(level - signals::STEP_LEVELS_DB.start()) as usize];
    let mut range = Table::new(
        "Range: threshold −40 dB, ratio 20:1, range 6 dB",
        "The most reduction at 0 dBFS input (34 dB past the threshold, where the ratio alone would take 32 dB). It must stop at the range: the error against 6 dB, lower is better.",
        &["", "Pro-C 3", "Mite Comp", "Verdict"],
    );
    let (rf, rm) = (gains(Vendor::Fab, "range")?, gains(Vendor::Mite, "range")?);
    range.rows.push(Row { cells: vec!["reduction at 0 dBFS".to_owned(), format!("{:.2} dB", -at(&rf, 0)), format!("{:.2} dB", -at(&rm, 0))], verdict: Verdict::None });
    range.push(Row::lower("error against 6 dB", (at(&rf, 0) + 6.0).abs(), (at(&rm, 0) + 6.0).abs(), 0.25, 2));
    tables.push(range);
    let mut makeup = Table::new(
        "Auto makeup: threshold −20 dB, ratio 4:1",
        "The level change with auto makeup on, at a few input levels (0 dB: the output matches the input). Both are fixed estimates from the settings, not measured loudness (Pro-C 3's manual calls its own an educated guess); Mite Comp cancels its curve at −12 dBFS. Informational: no verdict.",
        &["Input (dBFS)", "Pro-C 3", "Mite Comp"],
    );
    let (mf, mm) = (gains(Vendor::Fab, "automakeup")?, gains(Vendor::Mite, "automakeup")?);
    for level in [-30, -24, -18, -12, -6, 0] {
        makeup.rows.push(Row { cells: vec![format!("{level}"), format!("{:+.2}", at(&mf, level)), format!("{:+.2}", at(&mm, level))], verdict: Verdict::None });
    }
    tables.push(makeup);

    // Attack and release.
    let mut timing = Table::new(
        "Attack and release: set 10 ms / 100 ms, threshold −30 dB, ratio 10:1",
        "The tone steps from −40 to −10 dBFS and back. Attack: time to 63 % of the reduction; release: time to recover 63 %. Styles and Characters scale the times on purpose, so only Clean has a verdict (error against the set times, lower is better).",
        &["Style ↔ Character", "Pro-C 3 attack / release", "Mite attack / release", "Pro-C 3 GR", "Mite GR", "Verdict"],
    );
    for (k, (fab_style, mite_char)) in PAIRS.iter().enumerate() {
        let mut t = Vec::new();
        for v in both {
            let (i, o) = io(work, v, &format!("timing-{k}"), "timing")?;
            t.push(times(&analyze::gain_envelope(&i[0], &o[0], 50)));
        }
        let err = |x: (f64, f64, f64)| ((x.0 - 10.0).abs() / 10.0).max((x.1 - 100.0).abs() / 100.0);
        let verdict = if k == 0 { Verdict::lower(err(t[1]), err(t[0]), 0.05) } else { Verdict::None };
        timing.rows.push(Row {
            cells: vec![
                format!("{fab_style} ↔ {mite_char}"),
                format!("{:.1} / {:.0} ms", t[0].0, t[0].1),
                format!("{:.1} / {:.0} ms", t[1].0, t[1].1),
                format!("{:.1} dB", t[0].2),
                format!("{:.1} dB", t[1].2),
            ],
            verdict,
        });
    }
    tables.push(timing);

    // Lookahead and peaks.
    let mut look = Table::new(
        "Lookahead: drum hits at −3 dBFS, threshold −30 dB, ratio 20:1, attack 0.05 ms",
        "The highest output peak (lower is better: the hits are caught), with no lookahead and with 5 ms, and the latency each plugin reports with 5 ms.",
        &["Lookahead", "Pro-C 3 peak", "Mite peak", "Verdict"],
    );
    for (name, label) in [("hits-nolook", "off"), ("hits-look5", "5 ms")] {
        let peak = |v: Vendor| -> Result<f64, String> {
            let (_, o) = io(work, v, name, "hits")?;
            Ok(db(f64::from(o[0].iter().fold(0.0f32, |m, x| m.max(x.abs())))))
        };
        look.push(Row::lower(label, peak(Vendor::Fab)?, peak(Vendor::Mite)?, 0.5, 1));
    }
    let lat = |v: Vendor| rendered.get(&id(v, "hits-look5")).map_or(0, |r| r.latency);
    look.rows.push(Row { cells: vec!["reported latency (5 ms)".to_owned(), format!("{} smp", lat(Vendor::Fab)), format!("{} smp", lat(Vendor::Mite))], verdict: Verdict::None });
    // With the reported latency compensated, the output must line up with the input.
    let residual = |v: Vendor| -> Result<i64, String> {
        let (i, o) = io(work, v, "hits-look5", "hits")?;
        Ok(analyze::lag(&i[0], &o[0], 2000))
    };
    let (rf, rm) = (residual(Vendor::Fab)?, residual(Vendor::Mite)?);
    look.rows.push(Row { cells: vec!["output delay after compensation (5 ms)".to_owned(), format!("{rf} smp"), format!("{rm} smp")], verdict: Verdict::lower(rm.unsigned_abs() as f64, rf.unsigned_abs() as f64, 0.0) });
    tables.push(look);

    // Distortion.
    let mut thd = Table::new(
        "Distortion: 50 Hz at −6 dBFS, threshold −30 dB, ratio 4:1, attack 0.05 ms, release 10 ms",
        "THD+N of the output (20 Hz–20 kHz, dB re the fundamental; lower is cleaner). Settings this fast make the gain follow the waveform: these harmonics are the compressor working, not a fault, and oversampling does not remove them. Pro-C 3 without and with 4× oversampling; Mite Comp has no oversampling yet, so it runs at 1× in both rows.",
        &["Oversampling", "Pro-C 3", "Mite Comp", "Verdict"],
    );
    for (name, label) in [("thd", "off"), ("thd-os4", "4× (Pro-C 3 only)")] {
        let m = |v: Vendor| -> Result<f64, String> {
            let (_, o) = io(work, v, name, "sine50")?;
            Ok(analyze::thd_n(&o[0], RATE, 50.0))
        };
        thd.push(Row::lower(label, m(Vendor::Fab)?, m(Vendor::Mite)?, 3.0, 1));
    }
    tables.push(thd);
    let mut alias = Table::new(
        "Aliasing: 5 kHz at −6 dBFS, same settings",
        "Energy away from the fundamental and its harmonics (what folded back from above Nyquist), dB re the fundamental, 20 Hz–20 kHz; lower is better. This is what oversampling is for.",
        &["Oversampling", "Pro-C 3", "Mite Comp", "Verdict"],
    );
    for (name, label) in [("alias", "off"), ("alias-os4", "4× (Pro-C 3 only)")] {
        let m = |v: Vendor| -> Result<f64, String> {
            let (_, o) = io(work, v, name, "sine5k")?;
            Ok(analyze::aliasing(&o[0], RATE, 5000.0))
        };
        alias.push(Row::lower(label, m(Vendor::Fab)?, m(Vendor::Mite)?, 3.0, 1));
    }
    tables.push(alias);

    // Stereo link.
    let mut link = Table::new(
        "Stereo link: left at −10 dBFS, right at −40 dBFS, threshold −30 dB, ratio 4:1",
        "The gains of the left and right channels (the right is under the threshold on its own). Fully linked, the right must take the left's reduction; unlinked it must be untouched. The verdict is on the error: |right − left| linked, |right| unlinked.",
        &["Link", "Pro-C 3 left / right", "Mite left / right", "Verdict"],
    );
    for (name, label, linked) in [("link100", "100 %", true), ("link0", "0 %", false)] {
        let g = |v: Vendor, c: usize| -> Result<f64, String> {
            let (i, o) = io(work, v, name, "split")?;
            let n = o[c].len();
            let env = analyze::gain_envelope(&i[c][n / 2..], &o[c][n / 2..], 50);
            Ok(env.iter().sum::<f64>() / env.len() as f64)
        };
        let (fl, fr, ml, mr) = (g(Vendor::Fab, 0)?, g(Vendor::Fab, 1)?, g(Vendor::Mite, 0)?, g(Vendor::Mite, 1)?);
        let err = |l: f64, r: f64| if linked { (r - l).abs() } else { r.abs() };
        link.rows.push(Row { cells: vec![label.to_owned(), format!("{fl:.1} / {fr:.1} dB"), format!("{ml:.1} / {mr:.1} dB")], verdict: Verdict::lower(err(ml, mr), err(fl, fr), 0.5) });
    }
    tables.push(link);

    // Render speed.
    let mut cpu = Table::new(
        "Render time (rough CPU): 20 s of pink noise, one stage",
        "Offline render time. Pro-C 3 runs through wine and yabridge, so its figure includes the bridge: indicative only.",
        &["", "Pro-C 3", "Mite Comp", "Verdict"],
    );
    let s = |v: Vendor| rendered.get(&id(v, "cpu")).map_or(f64::NAN, |r| r.seconds);
    cpu.push(Row::lower("seconds", s(Vendor::Fab), s(Vendor::Mite), 0.0, 3));
    tables.push(cpu);
    Ok(tables)
}
