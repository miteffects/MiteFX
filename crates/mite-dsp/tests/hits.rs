//! Mite Cut's engine against a kit with known hits: every drum bleeds into every close mic (with
//! a mic-to-mic delay, a duller tone and ±1.5 dB from hit to hit), the toms ring undamped into
//! each other's mics, and the softest tom hits are quieter in their own mic than the snare's
//! bleed. Real hits kept, bleed cut, onsets exact; every real hit sounds (a soft one into the
//! same drum's louder ring may have no onset of its own, but sits inside the region kept).
//!
//! Known misses (kit 7, the floor tom's mic): a hit at velocity 0.2 into the rack tom's ring
//! and the snare's bleed in a fill is cut; a kick's sweep through the tom's band reads at the
//! boundary; undamped toms beating swell above the floor for a moment, and are kept.

use mite_dsp::hits::{Curve, Hit, Override, Role, Settings, detect};

const FS: f32 = 48_000.0;

struct Rng(u32);
impl Rng {
    fn next(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 17;
        self.0 ^= self.0 << 5;
        self.0 as f32 / u32::MAX as f32
    }
    fn bi(&mut self) -> f32 {
        self.next() * 2.0 - 1.0
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Drum {
    Kick,
    Snare,
    Tom1,
    Tom2,
    Hat,
}

/// One hit of `d` at velocity `v`, dry.
fn voice(d: Drum, v: f32, r: &mut Rng) -> Vec<f32> {
    let (len, f0, f1, drop, decay, noise, ndecay) = match d {
        Drum::Kick => (0.35, 150.0, 52.0, 0.03, 0.15, 0.25, 0.004),
        Drum::Snare => (0.3, 210.0, 185.0, 0.02, 0.07, 0.8, 0.09),
        Drum::Tom1 => (0.6, 150.0, 118.0, 0.06, 0.3, 0.15, 0.01),
        Drum::Tom2 => (0.8, 100.0, 82.0, 0.08, 0.4, 0.15, 0.01),
        Drum::Hat => (0.08, 0.0, 0.0, 1.0, 1.0, 0.6, 0.02),
    };
    let n = (len * FS) as usize;
    let mut ph = 0.0f32;
    let mut hp = 0.0f32;
    (0..n)
        .map(|i| {
            let t = i as f32 / FS;
            let tone = if f0 > 0.0 {
                ph += std::f32::consts::TAU * (f1 + (f0 - f1) * (-t / drop).exp()) / FS;
                (-t / decay).exp() * ph.sin()
            } else {
                0.0
            };
            let w = r.bi();
            let hpw = w - hp; // a crude high-pass for the hat
            hp = w;
            let nz = if d == Drum::Hat { hpw * 0.5 } else { w };
            // (Faded over its last 50 ms: a voice cut off would click.)
            let tail = ((n - i) as f32 / (0.05 * FS)).min(1.0);
            v * tail * (tone + noise * (-t / ndecay).exp() * nz)
        })
        .collect()
}

/// The pattern: (drum, onset sample, velocity), 40 s at 120 bpm.
fn pattern(r: &mut Rng) -> Vec<(Drum, usize, f32)> {
    let eighth = (0.25 * FS) as usize;
    let mut p = Vec::new();
    for k in 0..160 {
        let at = 2000 + k * eighth;
        p.push((Drum::Hat, at, 0.25 + 0.15 * r.next()));
        if k % 4 == 0 || (k % 8 == 5 && r.next() < 0.5) {
            p.push((Drum::Kick, at, 0.7 + 0.3 * r.next()));
        }
        if k % 4 == 2 {
            p.push((Drum::Snare, at, 0.6 + 0.4 * r.next()));
        }
        // Toms: a fill every other bar (sixteenths on the last beat), and scattered hits,
        // velocity anywhere from 0.15 (−16 dB) to 1.
        let bar = k / 8;
        if bar % 2 == 1 && k % 8 >= 6 {
            for s in 0..2 {
                let d = if (k + s) % 2 == 0 {
                    Drum::Tom1
                } else {
                    Drum::Tom2
                };
                p.push((d, at + s * eighth / 2, 0.15 + 0.85 * r.next()));
            }
        } else if k % 8 == 3 && r.next() < 0.4 {
            p.push((
                if r.next() < 0.5 {
                    Drum::Tom1
                } else {
                    Drum::Tom2
                },
                at + eighth / 2,
                0.15 + 0.85 * r.next(),
            ));
        }
    }
    p
}

/// The true hits: (drum, onset sample, velocity).
type Truth = Vec<(Drum, usize, f32)>;

/// The mics: kick, snare, tom 1, tom 2, hat. `bleed[m][d]`: drum d in mic m (amplitude), the
/// delay in ms.
fn kit(seed: u32) -> (Vec<Vec<f32>>, Truth) {
    let drums = [Drum::Kick, Drum::Snare, Drum::Tom1, Drum::Tom2, Drum::Hat];
    let bleed = [
        [1.0, 0.25, 0.15, 0.2, 0.05],
        [0.2, 1.0, 0.3, 0.25, 0.4],
        [0.2, 0.55, 1.0, 0.4, 0.3],
        [0.25, 0.5, 0.4, 1.0, 0.25],
        [0.05, 0.4, 0.15, 0.1, 1.0],
    ];
    let delay_ms = [
        [0.0, 2.0, 2.5, 2.2, 3.0],
        [2.0, 0.0, 1.4, 2.0, 1.2],
        [2.5, 1.4, 0.0, 1.0, 1.8],
        [2.2, 2.0, 1.0, 0.0, 2.6],
        [3.0, 1.2, 1.8, 2.6, 0.0],
    ];
    let mut r = Rng(seed);
    let p = pattern(&mut r);
    let n = (42.0 * FS) as usize;
    let mut mics = vec![vec![0.0f32; n]; 5];
    for &(d, at, v) in &p {
        let di = drums.iter().position(|&x| x == d).unwrap();
        let dry = voice(d, v, &mut r);
        for m in 0..5 {
            let g = bleed[m][di]
                * if m == di {
                    1.0
                } else {
                    10f32.powf(1.5 * r.bi() / 20.0)
                };
            let off = at + (delay_ms[m][di] * 1e-3 * FS) as usize;
            // Bleed is duller: a one-pole low-pass at 2.5 kHz.
            let k = if m == di {
                1.0
            } else {
                1.0 - (-std::f32::consts::TAU * 2500.0 / FS).exp()
            };
            let mut s = 0.0f32;
            for (i, &x) in dry.iter().enumerate() {
                s += k * (x - s);
                if off + i < n {
                    mics[m][off + i] += g * s;
                }
            }
        }
    }
    for m in &mut mics {
        for v in m.iter_mut() {
            *v += 3e-4 * r.bi();
        }
    }
    (mics, p)
}

struct Score {
    precision: f64,
    /// Hits found and kept, of the true hits.
    recall: f64,
    /// Of the true hits, those that sound: found and kept, or (with no onset of their own, a
    /// soft hit into the same drum's louder ring) inside a region kept for an earlier hit.
    sounding: f64,
    /// The worst onset error of a kept hit, ms.
    worst_ms: f64,
    kept: usize,
    notes: Vec<String>,
}

/// Kept hits against the true onsets of `d`, and what of them the curve lets through.
fn score(hits: &[Hit], curve: &Curve, truth: &[(Drum, usize, f32)], d: Drum) -> Score {
    let truth: Vec<(usize, f32)> = truth
        .iter()
        .filter(|t| t.0 == d)
        .map(|t| (t.1, t.2))
        .collect();
    let kept: Vec<&Hit> = hits.iter().filter(|h| h.kept).collect();
    let tol = (0.005 * FS) as usize;
    let mut worst = 0.0f64;
    let mut notes = Vec::new();
    let (mut found, mut sounding) = (0, 0);
    for &(t, v) in &truth {
        match kept.iter().find(|h| h.onset.abs_diff(t) <= tol) {
            Some(h) => {
                found += 1;
                sounding += 1;
                worst = worst.max((h.onset as f64 - t as f64).abs() / f64::from(FS) * 1e3);
            }
            None => {
                let near = hits.iter().find(|h| h.onset.abs_diff(t) <= tol);
                let open = curve.at(t + (0.0005 * FS) as usize) > 0.99;
                if open {
                    sounding += 1;
                }
                notes.push(format!(
                    "missed {:.3} s (v {v:.2}){}: {near:?}",
                    t as f32 / FS,
                    if open {
                        ", inside a kept region"
                    } else {
                        ", CUT"
                    }
                ));
            }
        }
    }
    let true_pos = kept
        .iter()
        .filter(|h| truth.iter().any(|t| h.onset.abs_diff(t.0) <= tol))
        .count();
    for h in kept
        .iter()
        .filter(|h| !truth.iter().any(|t| h.onset.abs_diff(t.0) <= tol))
    {
        notes.push(format!("false {:.3} s: {h:?}", h.onset as f32 / FS));
    }
    let n = truth.len().max(1) as f64;
    Score {
        precision: true_pos as f64 / kept.len().max(1) as f64,
        recall: f64::from(found) / n,
        sounding: f64::from(sounding) / n,
        worst_ms: worst,
        kept: kept.len(),
        notes,
    }
}

#[test]
fn tom_hits_kept_and_bleed_cut_even_when_the_bleed_is_louder() {
    // Over three kits and their kick, snare and tom mics: what is kept is real, every real hit
    // sounds, nearly all are found as hits, onsets exact; no one mic far off that.
    let (mut kept, mut real, mut hits, mut sounding, mut found) = (0.0, 0.0, 0.0, 0.0, 0.0);
    for seed in [91, 7, 3] {
        println!("== kit {seed}");
        for (track, n, sc) in kit_scores(seed) {
            let label = format!("kit {seed} track {track}");
            assert!(sc.precision >= 0.85 && sc.sounding >= 0.95 && sc.recall >= 0.75, "{label}: precision {:.3} sounding {:.3} recall {:.3}", sc.precision, sc.sounding, sc.recall);
            assert!(sc.worst_ms <= 1.5, "{label}: onset off by {:.2} ms", sc.worst_ms);
            kept += sc.kept as f64;
            real += sc.precision * sc.kept as f64;
            hits += n as f64;
            sounding += sc.sounding * n as f64;
            found += sc.recall * n as f64;
        }
    }
    let (p, snd, r) = (real / kept, sounding / hits, found / hits);
    println!("all: precision {p:.3}, sounding {snd:.3}, recall {r:.3} ({hits} real hits)");
    assert!(p >= 0.97 && snd >= 0.99 && r >= 0.9, "precision {p:.3} sounding {snd:.3} recall {r:.3}");
}

/// Each close mic's score (track, its real hits, the score); the hat's printed too.
fn kit_scores(seed: u32) -> Vec<(usize, usize, Score)> {
    let (mics, truth) = kit(seed);
    let roles = [Role::Kick, Role::Snare, Role::Tom, Role::Tom, Role::Hat];
    let tracks: Vec<(&[f32], Role)> = mics
        .iter()
        .zip(roles)
        .map(|(m, r)| (m.as_slice(), r))
        .collect();
    let t0 = std::time::Instant::now();
    let det = detect(&tracks, FS);
    println!("detect: {:.2} s for 5 × 42 s", t0.elapsed().as_secs_f64());
    let s = Settings::default();
    let t0 = std::time::Instant::now();
    for d in &det {
        let _ = d.hits(&s, &[]);
    }
    println!("hits (a settings change): {:.1} ms for 5 × 42 s", t0.elapsed().as_secs_f64() * 1e3);
    let mut out = Vec::new();
    for (i, d) in [(4, Drum::Hat), (2, Drum::Tom1), (3, Drum::Tom2), (1, Drum::Snare), (0, Drum::Kick)] {
        let hits = det[i].hits(&s, &[]);
        let sc = score(&hits, &det[i].curve(&hits, &s), &truth, d);
        println!("track {i}: {} candidates, precision {:.3} recall {:.3} sounding {:.3}, worst onset {:.2} ms", hits.len(), sc.precision, sc.recall, sc.sounding, sc.worst_ms);
        for n in sc.notes.iter().filter(|n| i != 4 || n.contains("false")).take(12) {
            println!("  {n}");
        }
        if i != 4 {
            // (The hat is scored, not held to it: its mic hears the snare louder than the hats.)
            out.push((i, truth.iter().filter(|t| t.0 == d).count(), sc));
        }
    }
    // The softest tom hits peak lower in their mic than the snare's bleed there (snare hits
    // with no tom near): level alone could not keep them and cut it.
    let peak = |at: usize| {
        mics[2][at..at + (0.02 * FS) as usize]
            .iter()
            .fold(0.0f32, |m, v| m.max(v.abs()))
    };
    let near_tom = |at: usize| {
        truth.iter().any(|t| {
            matches!(t.0, Drum::Tom1 | Drum::Tom2) && t.1.abs_diff(at) < (0.4 * FS) as usize
        })
    };
    let soft_tom = truth
        .iter()
        .filter(|t| t.0 == Drum::Tom1)
        .map(|t| peak(t.1))
        .fold(f32::INFINITY, f32::min);
    let loud_bleed = truth
        .iter()
        .filter(|t| t.0 == Drum::Snare && !near_tom(t.1))
        .map(|t| peak(t.1))
        .fold(0.0, f32::max);
    println!(
        "tom 1 mic: softest tom peaks {:.1} dB, loudest snare bleed {:.1} dB",
        20.0 * soft_tom.log10(),
        20.0 * loud_bleed.log10()
    );
    assert!(
        soft_tom < loud_bleed,
        "the test has soft toms under the bleed"
    );
    out
}

#[test]
fn a_clean_track_alone_keeps_every_hit() {
    let mut r = Rng(5);
    let n = (20.0 * FS) as usize;
    let mut x = vec![0.0f32; n];
    let mut truth = Vec::new();
    let mut at = 3000;
    while at + 40_000 < n {
        let v = 0.1 + 0.9 * r.next();
        for (i, s) in voice(Drum::Tom1, v, &mut r).iter().enumerate() {
            x[at + i] += s;
        }
        truth.push((Drum::Tom1, at, v));
        at += (0.15 * FS) as usize + (r.next() * 0.5 * FS) as usize;
    }
    for v in &mut x {
        *v += 1e-4 * r.bi();
    }
    let det = detect(&[(&x, Role::Tom)], FS);
    let hits = det[0].hits(&Settings::default(), &[]);
    let sc = score(
        &hits,
        &det[0].curve(&hits, &Settings::default()),
        &truth,
        Drum::Tom1,
    );
    println!(
        "alone: precision {:.3} recall {:.3} sounding {:.3}, worst onset {:.2} ms; {:?}",
        sc.precision, sc.recall, sc.sounding, sc.worst_ms, sc.notes
    );
    assert!(sc.precision >= 0.97 && sc.sounding >= 0.99 && sc.recall >= 0.9 && sc.worst_ms <= 1.0);
}

#[test]
fn overrides_win_and_the_curve_is_clean() {
    let (mics, _) = kit(7);
    let roles = [
        Role::Kick,
        Role::Snare,
        Role::Tom,
        Role::Tom,
        Role::Overhead,
    ];
    let tracks: Vec<(&[f32], Role)> = mics
        .iter()
        .zip(roles)
        .map(|(m, r)| (m.as_slice(), r))
        .collect();
    let det = detect(&tracks, FS);
    assert!(
        det[4].hits(&Settings::default(), &[]).is_empty(),
        "overheads are never cut"
    );
    assert!(!det[2].compared.contains(&4), "nor compared");
    let s = Settings::default();
    let hits = det[2].hits(&s, &[]);
    let kept = hits.iter().find(|h| h.kept).unwrap();
    let cut = hits.iter().find(|h| !h.kept).unwrap();
    let ov = [
        Override {
            at: kept.onset + 100,
            keep: Some(false),
            ..Default::default()
        },
        Override {
            at: cut.onset,
            keep: Some(true),
            end: Some(cut.onset + 4800),
            ..Default::default()
        },
        Override {
            at: 30_000,
            keep: Some(true),
            ..Default::default()
        },
    ];
    let h2 = det[2].hits(&s, &ov);
    let find = |o: usize| h2.iter().find(|h| h.onset == o).unwrap();
    assert!(!find(kept.onset).kept && find(kept.onset).forced);
    assert!(find(cut.onset).kept && find(cut.onset).end == cut.onset + 4800);
    assert!(
        find(30_000).kept,
        "a hit the detection never found, added by hand"
    );

    // Floor 0 dB: the curve is 1 everywhere (nulls with the source).
    let flat = Settings { floor_db: 0.0, ..s };
    let c = det[2].curve(&h2, &flat);
    let mut g = vec![0.0f32; mics[2].len()];
    c.fill(0, &mut g);
    assert!(g.iter().all(|&v| (v - 1.0).abs() < 1e-6));
    // Cut: block reads equal sample reads, the gain never steps more than its fades allow, and
    // every region opens before its hit's onset.
    let c: Curve = det[2].curve(&h2, &s);
    let mut blocks = vec![0.0f32; mics[2].len()];
    for (k, b) in blocks.chunks_mut(256).enumerate() {
        c.fill(k * 256, b);
    }
    for (n, &v) in blocks.iter().enumerate().step_by(97) {
        assert_eq!(v, c.at(n));
    }
    let step = blocks
        .windows(2)
        .map(|w| (w[1] - w[0]).abs())
        .fold(0.0, f32::max);
    assert!(
        step < 0.05,
        "a fade of 1 ms (48 samples) steps at most ~3 %: {step}"
    );
    for h in h2.iter().filter(|h| h.kept) {
        assert!(c.at(h.onset) > 0.99, "open at the onset of {h:?}");
    }
}

#[test]
fn a_clip_cut_right_on_a_hit_keeps_it() {
    // (REAPER items are often split on a hit: the clip's first sample is the hit's.)
    let mut r = Rng(11);
    let mut x = vec![0.0f32; (3.0 * FS) as usize];
    for at in [0usize, 48_000, 96_000] {
        for (i, s) in voice(Drum::Tom1, 0.8, &mut r).iter().enumerate() {
            if at + i < x.len() {
                x[at + i] += s;
            }
        }
    }
    let det = detect(&[(&x, Role::Tom)], FS);
    let hits = det[0].hits(&Settings::default(), &[]);
    let first = hits.iter().find(|h| h.kept).expect("a hit");
    assert!(first.onset < 48, "the first hit kept, at its start: {first:?}");
    let curve = det[0].curve(&hits, &Settings::default());
    assert!(curve.at(10) > 0.99, "and open from the clip's start");
}
