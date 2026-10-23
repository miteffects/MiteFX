//! Mite EQ against FabFilter Pro-Q 4.
//!
//! Accuracy is measured against each EQ itself: a band rendered at 44.1 kHz is compared with the
//! same band rendered at 192 kHz, where a digital EQ is as close to its analog prototype as it gets
//! (within 20 kHz). That way neither EQ needs the other's idea of Q, and both are held to the
//! analog curve they mean to draw.

use std::path::Path;

use super::analyze::{self, Response};
use super::manifest::Manifest;
use super::reaper::{num, text, Case, Rendered, Value};
use super::report::{Row, Table, Verdict};
use super::{signals, wav, Vendor};

const FAB: &str = "VST3:Pro-Q 4 (FabFilter)";
const MITE: &str = "VST3:Mite EQ (MiteFX)";
const LOW_RATE: u32 = 44_100;
const REF_RATE: u32 = 192_000;
const IMPULSE_S: f64 = 4.0;

/// A test band.
#[derive(Debug, Clone, Copy)]
struct Band {
    name: &'static str,
    shape: &'static str,
    freq: f64,
    gain: f64,
    q: f64,
    slope: Option<f64>,
}

const BANDS: &[Band] = &[
    Band { name: "Bell 1 kHz +12 dB", shape: "Bell", freq: 1000.0, gain: 12.0, q: 1.0, slope: None },
    Band { name: "Bell 10 kHz +12 dB Q 2", shape: "Bell", freq: 10_000.0, gain: 12.0, q: 2.0, slope: None },
    Band { name: "Bell 16 kHz −12 dB", shape: "Bell", freq: 16_000.0, gain: -12.0, q: 0.7, slope: None },
    Band { name: "Bell 100 Hz +12 dB Q 8", shape: "Bell", freq: 100.0, gain: 12.0, q: 8.0, slope: None },
    Band { name: "Low shelf 100 Hz +6 dB", shape: "Low Shelf", freq: 100.0, gain: 6.0, q: 0.71, slope: None },
    Band { name: "High shelf 8 kHz +6 dB", shape: "High Shelf", freq: 8000.0, gain: 6.0, q: 0.71, slope: None },
    Band { name: "High shelf 12 kHz +6 dB Q 2", shape: "High Shelf", freq: 12_000.0, gain: 6.0, q: 2.0, slope: None },
    Band { name: "Low cut 80 Hz 24 dB/oct", shape: "Low Cut", freq: 80.0, gain: 0.0, q: 0.71, slope: Some(24.0) },
    Band { name: "High cut 12 kHz 24 dB/oct", shape: "High Cut", freq: 12_000.0, gain: 0.0, q: 0.71, slope: Some(24.0) },
    Band { name: "High cut 15 kHz 96 dB/oct", shape: "High Cut", freq: 15_000.0, gain: 0.0, q: 0.71, slope: Some(96.0) },
    Band { name: "Notch 15 kHz", shape: "Notch", freq: 15_000.0, gain: 0.0, q: 2.0, slope: None },
    Band { name: "Band pass 2 kHz", shape: "Band Pass", freq: 2000.0, gain: 0.0, q: 1.0, slope: None },
    Band { name: "Tilt shelf 1 kHz +6 dB", shape: "Tilt Shelf", freq: 1000.0, gain: 6.0, q: 0.71, slope: None },
    Band { name: "Flat tilt 1 kHz +6 dB", shape: "Flat Tilt", freq: 1000.0, gain: 6.0, q: 0.71, slope: None },
    Band { name: "All pass 1 kHz", shape: "All Pass", freq: 1000.0, gain: 0.0, q: 1.0, slope: None },
];

/// Linear-phase resolutions, per vendor.
const FAB_RES: &[&str] = &["Low", "Medium", "High", "Very High", "Maximum"];
const MITE_RES: &[&str] = &["Low", "Medium", "High", "Max"];
/// The bands the linear-phase test runs: low frequencies are where resolution shows.
const LP_BANDS: &[usize] = &[3, 7];

/// A dynamic band's setting (both EQs take a range and a threshold; Pro-Q 4's attack and
/// release are relative, left at their automatic 50 %, ours at their defaults).
#[derive(Debug, Clone, Copy)]
struct Dyn {
    range: f64,
    threshold: f64,
}

fn globals(v: Vendor, mode: &str, res: Option<&str>, m: &Manifest) -> Vec<(String, Value)> {
    let mut p = Vec::new();
    match v {
        Vendor::Fab => {
            p.push(text("Processing Mode", mode));
            if let Some(r) = res {
                p.push(text("Processing Resolution", r));
            }
            p.extend([text("Character", "Clean"), text("Auto Gain", "Off"), num("Output Level", 0.0), num("Gain Scale", 100.0)]);
        }
        Vendor::Mite => {
            p.push(m.choice("mode", mode));
            if let Some(r) = res {
                p.push(m.choice("lp_res", r));
            }
            p.extend([m.flag("auto_gain", false), m.num("out_gain", 0.0), m.num("mix", 100.0)]);
        }
    }
    p
}

/// Band `n` (from 1) set to `b`, on `placement`, static or dynamic.
fn band(v: Vendor, n: usize, b: &Band, placement: &str, dynamic: Option<Dyn>, m: &Manifest) -> Vec<(String, Value)> {
    let mut p = Vec::new();
    match v {
        Vendor::Fab => {
            let k = |what: &str| format!("Band {n} {what}");
            p.extend([text(&k("Used"), "Used"), text(&k("Enabled"), "Enabled"), text(&k("Shape"), b.shape), num(&k("Frequency"), b.freq), num(&k("Gain"), b.gain), num(&k("Q"), b.q)]);
            p.push(text(&k("Stereo Placement"), placement));
            if let Some(s) = b.slope {
                p.push(num(&k("Slope"), s));
            }
            match dynamic {
                None => p.push(num(&k("Dynamic Range"), 0.0)),
                Some(d) => p.extend([
                    num(&k("Dynamic Range"), d.range),
                    text(&k("Dynamics Enabled"), "Dynamics Enabled"),
                    text(&k("Dynamics Auto"), "Dynamics Manual"),
                    num(&k("Threshold"), d.threshold),
                    num(&k("Attack"), 50.0),
                    num(&k("Release"), 50.0),
                    text(&k("External Side Chain"), "Off"),
                    text(&k("Spectral Enabled"), "Disabled"),
                ]),
            }
        }
        Vendor::Mite => {
            let k = |what: &str| format!("b{n:02}_{what}");
            p.extend([m.flag(&k("on"), true), m.choice(&k("type"), b.shape), m.num(&k("freq"), b.freq), m.num(&k("gain"), b.gain), m.num(&k("q"), b.q), m.choice(&k("stereo"), placement)]);
            if let Some(s) = b.slope {
                p.push(m.choice(&k("slope"), &format!("{s:.0} dB/oct")));
            }
            match dynamic {
                None => p.push(m.flag(&k("dyn"), false)),
                Some(d) => p.extend([
                    m.flag(&k("dyn"), true),
                    m.num(&k("range"), d.range),
                    m.num(&k("thresh"), d.threshold),
                    m.num(&k("attack"), 10.0),
                    m.num(&k("release"), 150.0),
                    m.choice(&k("dmode"), "Compressor"),
                    m.flag(&k("spectral"), false),
                    m.choice(&k("dsrc"), "Own"),
                ]),
            }
        }
    }
    p
}

fn band_params(v: Vendor, b: &Band, mode: &str, res: Option<&str>, m: &Manifest) -> Vec<(String, Value)> {
    let mut p = globals(v, mode, res, m);
    p.extend(band(v, 1, b, "Stereo", None, m));
    p
}

/// A named case: its parameters and its signal.
type Extra = (String, Vec<(String, Value)>, std::path::PathBuf);

/// The extra cases, per vendor.
fn extras(v: Vendor, work: &Path, m: &Manifest) -> Result<Vec<Extra>, String> {
    let sig = |name: &str, make: &dyn Fn() -> Vec<Vec<f32>>| -> Result<std::path::PathBuf, String> {
        let p = work.join(format!("signals/{name}-{LOW_RATE}.wav"));
        if !p.exists() {
            wav::write(&p, LOW_RATE, &make())?;
        }
        Ok(p)
    };
    let tone = signals::tone_hz(LOW_RATE);
    let zl = |extra: Vec<(String, Value)>| {
        let mut p = globals(v, "Zero Latency", None, m);
        p.extend(extra);
        p
    };
    let dynamic = Some(Dyn { range: -12.0, threshold: -30.0 });
    let bell = |freq: f64, gain: f64| Band { name: "", shape: "Bell", freq, gain, q: 1.0, slope: None };
    let mut out = vec![
        ("dynstatic".to_owned(), zl(band(v, 1, &bell(tone, 0.0), "Stereo", dynamic, m)), sig("stepped", &|| signals::stepped_tone(LOW_RATE))?),
        ("dynlow".to_owned(), zl(band(v, 1, &bell(100.0, 0.0), "Stereo", dynamic, m)), sig("sine100", &|| signals::sine(LOW_RATE, 100.0, -6.0, 3.0))?),
        ("left".to_owned(), zl(band(v, 1, &bell(1000.0, 12.0), "Left", None, m)), work.join(format!("signals/impulse-{LOW_RATE}.wav"))),
        ("side".to_owned(), zl(band(v, 1, &bell(1000.0, 12.0), "Side", None, m)), work.join(format!("signals/impulse-{LOW_RATE}.wav"))),
    ];
    let noise = sig("pink", &|| signals::pink_noise(LOW_RATE, 30.0))?;
    let spread = |k: usize| bell(60.0 * 2f64.powf(k as f64 * 0.9), if k % 2 == 0 { 6.0 } else { -6.0 });
    let many = |count: usize, dynamic: Option<Dyn>| -> Vec<(String, Value)> { (0..count).flat_map(|k| band(v, k + 1, &Band { freq: 40.0 * 2f64.powf(k as f64 * 9.0 / count as f64), ..spread(k) }, "Stereo", dynamic, m)).collect() };
    out.push(("cpu8".to_owned(), zl(many(8, None)), noise.clone()));
    out.push(("cpu8dyn".to_owned(), zl(many(8, Some(Dyn { range: -6.0, threshold: -30.0 }))), noise.clone()));
    let mut lp = globals(v, "Linear Phase", Some("High"), m);
    lp.extend(many(24, None));
    out.push(("cpu24lp".to_owned(), lp, noise));
    Ok(out)
}

fn fx(v: Vendor) -> &'static str {
    match v {
        Vendor::Fab => FAB,
        Vendor::Mite => MITE,
    }
}

fn id(v: Vendor, band: usize, mode: &str, rate: u32, res: Option<&str>) -> String {
    let mode = mode.replace(' ', "");
    let res = res.map(|r| format!("-{}", r.replace(' ', ""))).unwrap_or_default();
    format!("eq-{}-b{band}-{mode}{res}-{rate}", v.tag())
}

pub fn cases(work: &Path, m: &Manifest) -> Result<Vec<Case>, String> {
    let mut cases = Vec::new();
    let signal = |rate: u32| -> Result<std::path::PathBuf, String> {
        let p = work.join(format!("signals/impulse-{rate}.wav"));
        if !p.exists() {
            wav::write(&p, rate, &signals::impulse(rate, IMPULSE_S))?;
        }
        Ok(p)
    };
    for rate in [LOW_RATE, REF_RATE] {
        let sig = signal(rate)?;
        for v in [Vendor::Fab, Vendor::Mite] {
            for mode in ["Zero Latency", "Natural Phase"] {
                for (k, b) in BANDS.iter().enumerate() {
                    cases.push(Case { id: id(v, k, mode, rate, None), fx: fx(v).to_owned(), signal: sig.clone(), rate, params: band_params(v, b, mode, None, m) });
                }
            }
        }
    }
    for v in [Vendor::Fab, Vendor::Mite] {
        for (name, params, signal) in extras(v, work, m)? {
            cases.push(Case { id: format!("eq-{}-{name}", v.tag()), fx: fx(v).to_owned(), signal, rate: LOW_RATE, params });
        }
    }
    let sig = signal(LOW_RATE)?;
    for v in [Vendor::Fab, Vendor::Mite] {
        let res = if v == Vendor::Fab { FAB_RES } else { MITE_RES };
        for &k in LP_BANDS {
            for r in res {
                cases.push(Case { id: id(v, k, "Linear Phase", LOW_RATE, Some(r)), fx: fx(v).to_owned(), signal: sig.clone(), rate: LOW_RATE, params: band_params(v, &BANDS[k], "Linear Phase", Some(r), m) });
            }
        }
    }
    Ok(cases)
}

fn load(work: &Path, case: &str) -> Result<(u32, Vec<f32>), String> {
    let (rate, ch) = wav::read(&work.join(format!("renders/{case}.wav")))?;
    Ok((rate, ch.into_iter().next().unwrap_or_default()))
}

fn measure(work: &Path, case: &str, freqs: &[f64]) -> Result<(Response, analyze::Timing), String> {
    let (rate, h) = load(work, case)?;
    let t0 = (signals::IMPULSE_AT_S * f64::from(rate)) as usize;
    Ok((analyze::response(&h, rate, t0, 0.9, 2.5, freqs), analyze::timing(&h, rate, t0)))
}

/// The worst magnitude difference (dB) of `a` from `reference`, where the reference is within
/// 30 dB of its peak (deep stopbands aside).
fn worst_db(a: &Response, reference: &Response) -> f64 {
    let top = reference.mag_db.iter().copied().fold(f64::MIN, f64::max);
    a.mag_db.iter().zip(&reference.mag_db).filter(|(_, r)| **r > top - 30.0).map(|(x, r)| (x - r).abs()).fold(0.0, f64::max)
}

fn worst_phase_deg(a: &Response, reference: &Response) -> f64 {
    a.phase.iter().zip(&reference.phase).map(|(x, r)| analyze::phase_diff(*x, *r).abs().to_degrees()).fold(0.0, f64::max)
}

pub fn tables(work: &Path, rendered: &std::collections::BTreeMap<String, Rendered>) -> Result<Vec<Table>, String> {
    let freqs = analyze::log_freqs(20.0, 20_000.0, 240);
    let mut tables = Vec::new();
    for mode in ["Zero Latency", "Natural Phase"] {
        let mut mag = Table::new(
            &format!("{mode}: magnitude error against the analog curve (dB, worst 20 Hz–20 kHz at 44.1 kHz)"),
            "Each EQ's 44.1 kHz response against its own 192 kHz response (where the digital EQ is as close to analog as it gets), where the curve is within 30 dB of its peak. Lower is better; **close** is within 0.1 dB of Pro-Q 4.",
            &["Band", "Pro-Q 4", "Mite EQ", "Verdict"],
        );
        let mut phase = Table::new(
            &format!("{mode}: phase error against the analog phase (degrees, worst 20 Hz–20 kHz at 44.1 kHz)"),
            "As above, for the phase. Zero Latency is minimum phase by design, so only Natural Phase is meant to follow the analog phase.",
            &["Band", "Pro-Q 4", "Mite EQ", "Verdict"],
        );
        let mut pre = Table::new(
            &format!("{mode}: response before the input impulse (pre-ringing)"),
            "How long before the impulse the output comes within 60 dB of its peak, and its level there. Both are minimum- or near-minimum-phase modes, so less is better.",
            &["Band", "Pro-Q 4", "Mite EQ", "Verdict"],
        );
        for (k, b) in BANDS.iter().enumerate() {
            let mut errs = [0.0; 2];
            let mut perrs = [0.0; 2];
            let mut rings = [(0.0, 0.0); 2];
            for (j, v) in [Vendor::Fab, Vendor::Mite].into_iter().enumerate() {
                let (lo, t) = measure(work, &id(v, k, mode, LOW_RATE, None), &freqs)?;
                let (hi, _) = measure(work, &id(v, k, mode, REF_RATE, None), &freqs)?;
                errs[j] = worst_db(&lo, &hi);
                perrs[j] = worst_phase_deg(&lo, &hi);
                rings[j] = (t.pre_ring_ms, t.pre_ring_db);
            }
            mag.push(Row::lower(b.name, errs[0], errs[1], 0.1, 2));
            phase.push(Row::lower(b.name, perrs[0], perrs[1], 1.0, 1));
            let cell = |r: (f64, f64)| if r.0 == 0.0 { "none".to_owned() } else { format!("{:.2} ms ({:.0} dB)", r.0, r.1) };
            let verdict = Verdict::lower(rings[1].0, rings[0].0, 0.05);
            pre.rows.push(Row { cells: vec![b.name.to_owned(), cell(rings[0]), cell(rings[1])], verdict });
        }
        tables.extend([mag, phase, pre]);
    }

    // Linear phase: latency, low-frequency accuracy (against the EQ's own Natural Phase at
    // 192 kHz) and pre-ringing, at every resolution of each.
    let mut lp = Table::new(
        "Linear Phase: latency, low-frequency accuracy and pre-ringing per resolution (44.1 kHz)",
        "Latency as reported to the host (Reaper compensates it; the residual is the impulse peak's offset after compensation, which must be 0). Accuracy: the worst magnitude difference 20 Hz–20 kHz from the same EQ's Natural Phase response at 192 kHz. Pre-ring: how long before the impulse the output is within 60 dB of its peak. Pro-Q 4's latencies match its manual. Its Very High and Maximum render the Q 8 bell wider than High does, repeatably (also with a second of settling before the render), in Reaper's offline render; not yet checked in a real-time render.",
        &["Band · EQ · resolution", "Latency", "Residual", "Accuracy (dB)", "Pre-ring"],
    );
    let mut lp_best = Table::new(
        "Linear Phase: best low-frequency accuracy at any resolution (dB)",
        "The lowest accuracy figure each EQ reaches in the table below, with the latency it costs. Lower is better.",
        &["Band", "Pro-Q 4", "Mite EQ", "Verdict"],
    );
    for &k in LP_BANDS {
        let mut best = [(f64::MAX, 0i64); 2];
        for (j, v) in [Vendor::Fab, Vendor::Mite].into_iter().enumerate() {
            let (reference, _) = measure(work, &id(v, k, "Natural Phase", REF_RATE, None), &freqs)?;
            for r in if v == Vendor::Fab { FAB_RES } else { MITE_RES } {
                let case = id(v, k, "Linear Phase", LOW_RATE, Some(r));
                let acc = worst_db(&measure(work, &case, &freqs)?.0, &reference);
                if acc < best[j].0 {
                    best[j] = (acc, rendered.get(&case).map_or(0, |x| x.latency));
                }
            }
        }
        let ms = |smp: i64| smp as f64 * 1000.0 / f64::from(LOW_RATE);
        lp_best.rows.push(Row {
            cells: vec![BANDS[k].name.to_owned(), format!("{:.2} ({:.0} ms)", best[0].0, ms(best[0].1)), format!("{:.2} ({:.0} ms)", best[1].0, ms(best[1].1))],
            verdict: Verdict::lower((best[1].0 * 100.0).round() / 100.0, (best[0].0 * 100.0).round() / 100.0, 0.1),
        });
    }
    tables.push(lp_best);
    for &k in LP_BANDS {
        for v in [Vendor::Fab, Vendor::Mite] {
            let (reference, _) = measure(work, &id(v, k, "Natural Phase", REF_RATE, None), &freqs)?;
            let res = if v == Vendor::Fab { FAB_RES } else { MITE_RES };
            for r in res {
                let case = id(v, k, "Linear Phase", LOW_RATE, Some(r));
                let (resp, t) = measure(work, &case, &freqs)?;
                let latency = rendered.get(&case).map_or(0, |x| x.latency);
                lp.rows.push(Row {
                    cells: vec![
                        format!("{} · {} · {r}", BANDS[k].name, v.name("Pro-Q 4", "Mite EQ")),
                        format!("{latency} smp ({:.1} ms)", latency as f64 * 1000.0 / f64::from(LOW_RATE)),
                        format!("{}", t.peak_offset),
                        format!("{:.2}", worst_db(&resp, &reference)),
                        format!("{:.1} ms", t.pre_ring_ms),
                    ],
                    verdict: Verdict::None,
                });
            }
        }
    }
    tables.push(lp);

    // Dynamic EQ.
    let both = [Vendor::Fab, Vendor::Mite];
    let read = |v: Vendor, name: &str| -> Result<Vec<Vec<f32>>, String> { Ok(wav::read(&work.join(format!("renders/eq-{}-{name}.wav", v.tag())))?.1) };
    let (_, stepped) = wav::read(&work.join(format!("signals/stepped-{LOW_RATE}.wav")))?;
    let mut dynamic = Table::new(
        "Dynamic EQ: gain against level (bell at the tone, Q 1, range −12 dB, threshold −30 dB)",
        "A tone at the band's frequency held at each level (dBFS peak). Both use the range as the most the band moves; the verdict row is the depth reached at 0 dBFS against the set −12 dB (lower error is better). Pro-Q 4's attack and release are relative and left at their automatic 50 %; Mite EQ's at 10 / 150 ms.",
        &["Input (dBFS)", "Pro-Q 4", "Mite EQ", "Verdict"],
    );
    let mut curves = Vec::new();
    for v in both {
        let o = read(v, "dynstatic")?;
        curves.push(analyze::step_gains(&stepped[0], &o[0], LOW_RATE, 0.5, signals::STEP_S, signals::STEP_LEVELS_DB.count()));
    }
    for level in [-50, -40, -35, -30, -25, -20, -10, 0] {
        let k = (level - signals::STEP_LEVELS_DB.start()) as usize;
        dynamic.rows.push(Row { cells: vec![format!("{level}"), format!("{:.2}", curves[0][k]), format!("{:.2}", curves[1][k])], verdict: Verdict::None });
    }
    let last = |c: &Vec<f64>| *c.last().unwrap_or(&0.0);
    dynamic.push(Row::lower("depth error at 0 dBFS (dB)", (last(&curves[0]) + 12.0).abs(), (last(&curves[1]) + 12.0).abs(), 0.5, 2));
    tables.push(dynamic);

    let mut steady = Table::new(
        "Dynamic EQ on a steady low tone: 100 Hz at −6 dBFS, bell 100 Hz, range −12 dB, threshold −30 dB",
        "Over the last second: the ripple of the gain (largest minus smallest, per 1 ms) and THD+N of the output. A steady tone must give a steady gain; lower is better for both.",
        &["", "Pro-Q 4", "Mite EQ", "Verdict"],
    );
    let (_, sine) = wav::read(&work.join(format!("signals/sine100-{LOW_RATE}.wav")))?;
    let mut ripple = [0.0; 2];
    let mut thd = [0.0; 2];
    for (j, v) in both.into_iter().enumerate() {
        let o = read(v, "dynlow")?;
        let n = o[0].len();
        let tail = n - LOW_RATE as usize;
        let env = analyze::gain_envelope(&sine[0][tail..], &o[0][tail..], (LOW_RATE / 100) as usize);
        ripple[j] = env.iter().copied().fold(f64::MIN, f64::max) - env.iter().copied().fold(f64::MAX, f64::min);
        thd[j] = analyze::thd_n(&o[0], LOW_RATE, 100.0);
    }
    steady.push(Row::lower("gain ripple (dB)", ripple[0], ripple[1], 0.1, 2));
    steady.push(Row::lower("THD+N (dB)", thd[0], thd[1], 3.0, 1));
    tables.push(steady);

    // Stereo placement.
    let mut place = Table::new(
        "Stereo placement: bell 1 kHz +12 dB on one channel",
        "An impulse on both channels. Left only: the right channel must stay flat. Side only, on a mono signal (no side content): both channels must stay flat. The worst deviation from 0 dB, 20 Hz–20 kHz; lower is better.",
        &["Placement", "Pro-Q 4", "Mite EQ", "Verdict"],
    );
    for (name, label, ch) in [("left", "Left: right channel", 1usize), ("side", "Side on mono: left channel", 0)] {
        let dev = |v: Vendor| -> Result<f64, String> {
            let o = read(v, name)?;
            let t0 = (signals::IMPULSE_AT_S * f64::from(LOW_RATE)) as usize;
            let r = analyze::response(&o[ch], LOW_RATE, t0, 0.9, 2.5, &freqs);
            Ok(r.mag_db.iter().map(|x| x.abs()).fold(0.0, f64::max))
        };
        place.push(Row::lower(label, dev(Vendor::Fab)?, dev(Vendor::Mite)?, 0.01, 3));
    }
    tables.push(place);

    let mut load = Table::new(
        "Render time (rough CPU): 30 s of pink noise",
        "Offline render time in seconds; lower is better. Pro-Q 4 runs through wine and yabridge, so its figure includes the bridge: indicative only.",
        &["Bands", "Pro-Q 4", "Mite EQ", "Verdict"],
    );
    for (name, label) in [("cpu8", "8 bells, Zero Latency"), ("cpu8dyn", "8 dynamic bells, Zero Latency"), ("cpu24lp", "24 bands, Linear Phase High")] {
        let t = |v: Vendor| rendered.get(&format!("eq-{}-{name}", v.tag())).map_or(f64::NAN, |r| r.seconds);
        load.push(Row::lower(label, t(Vendor::Fab), t(Vendor::Mite), 0.0, 3));
    }
    tables.push(load);

    // Render speed: how much faster than real time each EQ rendered the 4 s impulse (both
    // modes, all bands), as a rough CPU figure.
    let mut cpu = Table::new(
        "Render time (rough CPU)",
        "Total offline render time of every Zero Latency and Natural Phase case at 44.1 kHz (one band each, 4 s of audio). Pro-Q 4 runs through wine and yabridge, so its figure includes the bridge: indicative only.",
        &["Mode", "Pro-Q 4", "Mite EQ", "Verdict"],
    );
    for mode in ["Zero Latency", "Natural Phase"] {
        let total = |v: Vendor| (0..BANDS.len()).filter_map(|k| rendered.get(&id(v, k, mode, LOW_RATE, None))).map(|r| r.seconds).sum::<f64>();
        cpu.push(Row::lower(mode, total(Vendor::Fab), total(Vendor::Mite), 0.0, 2));
    }
    tables.push(cpu);
    Ok(tables)
}
