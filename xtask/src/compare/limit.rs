//! Mite Limit against FabFilter Pro-L 2. Both are set the same way (gain, ceiling = Pro-L 2's
//! output level, lookahead, attack, release, links at 100 %, true peak, oversampling); each
//! Pro-L 2 style is paired with our nearest style.
//!
//! Pro-L 2 limits at 0 dB and then turns the result down to its output level (measured: with
//! light limiting its output sits exactly the output level under ours), so for the same drive
//! into the ceiling Mite Limit's gain is Pro-L 2's plus the (negative) ceiling.

use std::path::Path;

use realfft::RealFftPlanner;

use super::analyze::{self, db};
use super::manifest::Manifest;
use super::reaper::{num, text, Case, Rendered, Value};
use super::report::{Row, Table, Verdict};
use super::{signals, wav, Vendor};

const FAB: &str = "VST3:Pro-L 2 (FabFilter)";
const MITE: &str = "VST3:Mite Limit (MiteFX)";
const RATE: u32 = 44_100;
/// Pro-L 2 style ↔ Mite Limit style.
const PAIRS: &[(&str, &str)] = &[("Transparent", "Transparent"), ("Punchy", "Punch"), ("Aggressive", "Dense"), ("Bus", "Bus"), ("Safe", "Safe")];
const CEILING: f64 = -1.0;
/// Renders of the CPU case (the fastest counts).
const CPU_RUNS: usize = 3;
/// The true-peak signals (name in the plan, label in the table).
const TP_SIGNALS: &[(&str, &str)] = &[("sinenyq", "sine at 0.45 fs"), ("clipnoise", "clipped noise"), ("hits", "drum hits"), ("pink6", "pink noise")];

/// One limiter setting, in both plugins' units.
#[derive(Debug, Clone, Copy)]
struct Setting {
    style: usize,
    gain: f64,
    lookahead: f64,
    attack: f64,
    release: f64,
    /// 1 = off.
    oversampling: u32,
    true_peak: bool,
}

const BASE: Setting = Setting { style: 0, gain: 20.0, lookahead: 2.0, attack: 30.0, release: 100.0, oversampling: 4, true_peak: true };

fn params(v: Vendor, s: &Setting, m: &Manifest) -> Vec<(String, Value)> {
    let (fab_style, mite_style) = PAIRS[s.style];
    match v {
        Vendor::Fab => {
            let mut p = vec![
                num("Gain", s.gain),
                text("Style", fab_style),
                num("Lookahead", s.lookahead),
                num("Attack", s.attack),
                num("Release", s.release),
                num("Channel Link Transients", 100.0),
                num("Channel Link Release", 100.0),
                text("True Peak Limiting", if s.true_peak { "On" } else { "Off" }),
                text("Dithering", "Off"),
                text("Filter DC Offset", "Off"),
                text("Unity Gain", "Off"),
                text("Audition Limiting", "Off"),
                num("Output Level", CEILING),
            ];
            if s.oversampling > 1 {
                p.push(num("Oversampling", f64::from(s.oversampling)));
            } else {
                p.push(text("Oversampling", "Off"));
            }
            p
        }
        Vendor::Mite => vec![
            m.num("gain", s.gain + CEILING),
            m.num("ceiling", CEILING),
            m.choice("style", mite_style),
            m.num("lookahead", s.lookahead),
            m.num("attack", s.attack),
            m.num("release", s.release),
            m.num("link_t", 100.0),
            m.num("link_r", 100.0),
            m.flag("true_peak", s.true_peak),
            m.choice("oversampling", &format!("{}×", s.oversampling)),
            m.flag("clip_on", false),
            m.num("protect", 0.0),
            m.flag("unity", false),
            m.flag("delta", false),
            m.flag("bypass", false),
            m.choice("dither", "Off"),
            m.flag("dc_filter", false),
            m.flag("preview", false),
        ],
    }
}

fn fx(v: Vendor) -> &'static str {
    match v {
        Vendor::Fab => FAB,
        Vendor::Mite => MITE,
    }
}

/// White noise clipped at half scale: flat tops with sharp corners, the hardest true peaks.
fn clipped_noise(rate: u32, seconds: f64) -> Vec<Vec<f32>> {
    let n = (seconds * f64::from(rate)) as usize;
    let mut seed = 0x5eed_1234u32;
    let x: Vec<f32> = (0..n)
        .map(|_| {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let white = f64::from(seed >> 8) / f64::from(1u32 << 24) * 2.0 - 1.0;
            (0.5 * (2.0 * white).clamp(-1.0, 1.0)) as f32
        })
        .collect();
    vec![x.clone(), x]
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
    let files = [
        sig("sinenyq", &|| signals::sine(RATE, 0.45 * f64::from(RATE), -6.0, 3.0))?,
        sig("clipnoise", &|| clipped_noise(RATE, 3.0))?,
        sig("hits", &|| signals::hits(RATE, -3.0, 4.0))?,
        sig("pink6", &|| signals::pink_noise(RATE, 6.0))?,
    ];
    let quiet = sig("hitsquiet", &|| signals::hits(RATE, -20.0, 4.0))?;
    let bass = sig("sine50", &|| signals::sine(RATE, 50.0, -6.0, 3.0))?;
    let pink = sig("pink", &|| signals::pink_noise(RATE, 20.0))?;
    let mut p = Vec::new();
    for ((name, _), file) in TP_SIGNALS.iter().zip(&files) {
        for os in [1, 4] {
            p.push((format!("tp-{name}-os{os}"), Setting { oversampling: os, ..BASE }, file.clone()));
        }
    }
    p.push(("tp-pink6-off".to_owned(), Setting { true_peak: false, ..BASE }, files[3].clone()));
    for k in 0..PAIRS.len() {
        p.push((format!("loud-{k}"), Setting { style: k, gain: 20.0, ..BASE }, files[3].clone()));
    }
    for k in [0, 4] {
        // −6 dBFS + 13 dB into −1 dBTP: 8 dB of steady limiting.
        p.push((format!("thd-{k}"), Setting { style: k, gain: 13.0, release: 100.0, ..BASE }, bass.clone()));
    }
    // Under the ceiling: nothing to limit, so the output is the input, delayed and compensated.
    p.push(("null".to_owned(), Setting { gain: 0.0, oversampling: 1, ..BASE }, quiet.clone()));
    p.push(("align".to_owned(), Setting { gain: 0.0, ..BASE }, quiet));
    for k in 0..CPU_RUNS {
        p.push((format!("cpu{k}"), Setting { gain: 12.0, ..BASE }, pink.clone()));
    }
    Ok(p)
}

fn id(v: Vendor, name: &str) -> String {
    format!("limit-{}-{name}", v.tag())
}

pub fn cases(work: &Path, m: &Manifest) -> Result<Vec<Case>, String> {
    let mut cases = Vec::new();
    for (name, s, signal) in plan(work)? {
        for v in [Vendor::Fab, Vendor::Mite] {
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

/// The true peak of `x` (dBFS) on a 32× band-limited reconstruction: the spectrum zero-padded
/// (the signal padded with silence first, so the circular transform has no seam), read away
/// from its first and last 2048 samples.
pub fn true_peak_32x(x: &[f32]) -> f64 {
    let pad = 8192;
    let n = (x.len() + 2 * pad).next_power_of_two();
    let mut planner = RealFftPlanner::<f64>::new();
    let fwd = planner.plan_fft_forward(n);
    let mut buf = vec![0.0f64; n];
    for (i, v) in x.iter().enumerate() {
        buf[pad + i] = f64::from(*v);
    }
    let mut spec = fwd.make_output_vec();
    let _ = fwd.process(&mut buf, &mut spec);
    let up = 32;
    let inv = planner.plan_fft_inverse(n * up);
    let mut big = inv.make_input_vec();
    big[..spec.len()].copy_from_slice(&spec);
    // The Nyquist bin is shared between its two halves in the longer transform.
    big[spec.len() - 1] *= 0.5;
    let mut out = inv.make_output_vec();
    let _ = inv.process(&mut big, &mut out);
    // Away from the ends: a signal that starts or stops abruptly rings there (Gibbs), a peak the
    // render does not have.
    let edge = 2048.min(x.len() / 4);
    let (a, b) = ((pad + edge) * up, (pad + x.len() - edge) * up);
    let peak = out[a..b].iter().fold(0.0f64, |m, v| m.max(v.abs())) / n as f64;
    db(peak)
}

fn rms_db(x: &[f32]) -> f64 {
    let s: f64 = x.iter().map(|v| f64::from(*v).powi(2)).sum();
    db((s / x.len().max(1) as f64).sqrt())
}

pub fn tables(work: &Path, rendered: &std::collections::BTreeMap<String, Rendered>) -> Result<Vec<Table>, String> {
    let mut tables = Vec::new();

    // True peak.
    let mut tp = Table::new(
        "True peak: 20 dB of gain into a −1 dBTP ceiling (Transparent, lookahead 2 ms)",
        "How far the output's true peak goes over the ceiling (dB; 0 holds it; lower is better), without oversampling and with 4×. **BS.1770** is the standard's 4× meter (`mite_dsp::loudness::true_peak_of`), what a delivery spec checks; the verdict is on it. **32×** is an ideal band-limited reconstruction (the zero-padded spectrum): it counts content up to Nyquist that the standard's filter and a real DAC attenuate, so it reads higher for both. The last row has true-peak limiting off, for scale.",
        &["Signal · oversampling", "Pro-L 2 BS.1770", "Mite BS.1770", "Pro-L 2 32×", "Mite 32×", "Verdict"],
    );
    let meters = |v: Vendor, case: &str, signal: &str| -> Result<(f64, f64), String> {
        let (_, o) = io(work, v, case, signal)?;
        let bs = o.iter().map(|c| f64::from(mite_dsp::gain_to_db(mite_dsp::loudness::true_peak_of(c, 4)))).fold(f64::MIN, f64::max);
        let ideal = o.iter().map(|c| true_peak_32x(c)).fold(f64::MIN, f64::max);
        Ok(((bs - CEILING).max(0.0), (ideal - CEILING).max(0.0)))
    };
    let row = |label: String, f: (f64, f64), m: (f64, f64), verdict: Verdict| Row { cells: vec![label, format!("{:.3}", f.0), format!("{:.3}", m.0), format!("{:.3}", f.1), format!("{:.3}", m.1)], verdict };
    for (name, label) in TP_SIGNALS {
        for os in [1, 4] {
            let case = format!("tp-{name}-os{os}");
            let (f, m) = (meters(Vendor::Fab, &case, name)?, meters(Vendor::Mite, &case, name)?);
            let os_label = if os == 1 { "off".to_owned() } else { format!("{os}×") };
            let round = |x: f64| (x * 1000.0).round() / 1000.0;
            tp.rows.push(row(format!("{label} · {os_label}"), f, m, Verdict::lower(round(m.0), round(f.0), 0.05)));
        }
    }
    let (f, m) = (meters(Vendor::Fab, "tp-pink6-off", "pink6")?, meters(Vendor::Mite, "tp-pink6-off", "pink6")?);
    tp.rows.push(row("pink noise · 4×, true peak off".to_owned(), f, m, Verdict::None));
    tables.push(tp);

    // Loudness for the same gain.
    let mut loud = Table::new(
        "Level at the same settings: pink noise, 20 dB of gain, −1 dBTP, 4×",
        "The output's RMS level (dBFS) with each style pair: how much level a style keeps for the same push into the same ceiling. Styles trade level for transparency on purpose, so these are readings, not a contest.",
        &["Style ↔ style", "Pro-L 2", "Mite Limit"],
    );
    for (k, (f, m)) in PAIRS.iter().enumerate() {
        let r = |v: Vendor| -> Result<f64, String> {
            let (_, o) = io(work, v, &format!("loud-{k}"), "pink6")?;
            Ok(rms_db(&o[0][o[0].len() / 3..]))
        };
        loud.rows.push(Row { cells: vec![format!("{f} ↔ {m}"), format!("{:.2}", r(Vendor::Fab)?), format!("{:.2}", r(Vendor::Mite)?)], verdict: Verdict::None });
    }
    tables.push(loud);

    // Distortion.
    let mut thd = Table::new(
        "Distortion: 50 Hz at −6 dBFS, 13 dB of gain into −1 dBTP, release 100 ms, 4×",
        "THD+N of the output (20 Hz–20 kHz, dB re the fundamental; lower is cleaner): 8 dB of steady limiting on a bass tone, where a limiter's gain riding the waveform shows as distortion.",
        &["Style ↔ style", "Pro-L 2", "Mite Limit", "Verdict"],
    );
    for k in [0, 4] {
        let m = |v: Vendor| -> Result<f64, String> {
            let (_, o) = io(work, v, &format!("thd-{k}"), "sine50")?;
            Ok(analyze::thd_n(&o[0], RATE, 50.0))
        };
        thd.push(Row::lower(&format!("{} ↔ {}", PAIRS[k].0, PAIRS[k].1), m(Vendor::Fab)?, m(Vendor::Mite)?, 3.0, 1));
    }
    tables.push(thd);

    // Transparency under the ceiling, and latency.
    let mut clean = Table::new(
        "Under the ceiling: drum hits peaking at −20 dBFS, gain 0",
        "Nothing reaches the ceiling, so a limiter should leave the signal alone. Residual: the output minus the input after delay compensation, dB re the input's RMS (lower is better; −∞ is a bit-exact null), without oversampling. Then the latency each reports and what is left after REAPER compensates it (4×, true peak on).",
        &["", "Pro-L 2", "Mite Limit", "Verdict"],
    );
    let residual = |v: Vendor| -> Result<f64, String> {
        let (i, o) = io(work, v, "null", "hitsquiet")?;
        let lag = analyze::lag(&i[0], &o[0], 2000);
        let n = i[0].len().min(o[0].len());
        let (a, b) = (2000usize, n.saturating_sub(2000));
        let diff: Vec<f32> = (a..b).map(|k| o[0][((k as i64) + lag) as usize] - i[0][k]).collect();
        Ok((rms_db(&diff) - rms_db(&i[0][a..b])).max(-200.0))
    };
    let fmt = |x: f64| if x <= -199.0 { "−∞".to_owned() } else { format!("{x:.1} dB") };
    let (rf, rm) = (residual(Vendor::Fab)?, residual(Vendor::Mite)?);
    clean.rows.push(Row { cells: vec!["residual, no oversampling".to_owned(), fmt(rf), fmt(rm)], verdict: Verdict::lower(rm, rf, 6.0) });
    let lat = |v: Vendor| rendered.get(&id(v, "align")).map_or(0, |r| r.latency);
    clean.rows.push(Row { cells: vec!["reported latency".to_owned(), format!("{} smp", lat(Vendor::Fab)), format!("{} smp", lat(Vendor::Mite))], verdict: Verdict::None });
    let after = |v: Vendor| -> Result<i64, String> {
        let (i, o) = io(work, v, "align", "hitsquiet")?;
        Ok(analyze::lag(&i[0], &o[0], 2000))
    };
    let (af, am) = (after(Vendor::Fab)?, after(Vendor::Mite)?);
    clean.rows.push(Row { cells: vec!["output delay after compensation".to_owned(), format!("{af} smp"), format!("{am} smp")], verdict: Verdict::lower(am.unsigned_abs() as f64, af.unsigned_abs() as f64, 0.0) });
    tables.push(clean);

    // Render speed.
    let mut cpu = Table::new(
        "Render time (rough CPU): 20 s of pink noise, 12 dB of gain, true peak, 4×",
        "Offline render time, the fastest of three renders. Pro-L 2 runs through wine and yabridge, so its figure includes the bridge: indicative only. **close** is within 5 % of FabFilter.",
        &["", "Pro-L 2", "Mite Limit", "Verdict"],
    );
    let s = |v: Vendor| (0..CPU_RUNS).filter_map(|k| rendered.get(&id(v, &format!("cpu{k}")))).map(|r| r.seconds).fold(f64::NAN, f64::min);
    cpu.push(Row::lower("seconds", s(Vendor::Fab), s(Vendor::Mite), 0.05 * s(Vendor::Fab), 3));
    tables.push(cpu);
    Ok(tables)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A sine at fs/4 sampled at ±45° from its crests reads 3 dB low on its samples; the 32×
    /// reconstruction finds the true crest.
    #[test]
    fn the_reference_finds_the_peak_between_samples() {
        let x: Vec<f32> = (0..44_100).map(|i| (std::f64::consts::FRAC_PI_2 * i as f64 + std::f64::consts::FRAC_PI_4).sin() as f32 * 0.5).collect();
        let sample_peak = db(f64::from(x.iter().fold(0.0f32, |m, v| m.max(v.abs()))));
        let tp = true_peak_32x(&x);
        assert!((sample_peak - (db(0.5) - 3.01)).abs() < 0.02, "{sample_peak}");
        assert!((tp - db(0.5)).abs() < 0.02, "{tp}");
    }
}
