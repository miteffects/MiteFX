//! Mite Pitch's engine, measured where its artifacts live (melody_quality measures the notes'
//! steady middles): every note shifted by ±300 and ±700 cents, then
//!
//! * **envelope**: the spectral envelope (⅓-octave-smoothed log spectrum, 200 Hz–6 kHz) over the
//!   notes' middles against the input's, RMS dB — formants kept means near 0;
//! * **air**: 4–12 kHz energy inside the notes, out against in, dB (breath, the voice's top);
//! * **consonants**: 3–14 kHz energy in the notes' leading and trailing unpitched sound, dB;
//! * **seams**: the largest 2nd difference of the 3 kHz+ band within 10 ms of each note's ends,
//!   out against in, dB (a click is a spike there), median and worst.
//!
//! With `out_dir`, writes the take and each shift as 32-bit float WAVs to listen to.
//!
//!     cargo run --release -p mite-dsp --example melody_artifacts -- take.wav [seconds] [out_dir]

use mite_dsp::analyzer::Analyzer;
use mite_dsp::melody::{analyse, render, NoteEdit};

fn read_wav(path: &str) -> Result<(Vec<f32>, f64), String> {
    let b = std::fs::read(path).map_err(|e| e.to_string())?;
    if b.len() < 44 || &b[0..4] != b"RIFF" || &b[8..12] != b"WAVE" {
        return Err("not a WAV file".into());
    }
    let (mut channels, mut fs, mut bits, mut format) = (1usize, 48_000u32, 16u16, 1u16);
    let mut pos = 12;
    while pos + 8 <= b.len() {
        let id = &b[pos..pos + 4];
        let len = u32::from_le_bytes([b[pos + 4], b[pos + 5], b[pos + 6], b[pos + 7]]) as usize;
        let body = &b[pos + 8..(pos + 8 + len).min(b.len())];
        if id == b"fmt " {
            format = u16::from_le_bytes([body[0], body[1]]);
            channels = u16::from_le_bytes([body[2], body[3]]) as usize;
            fs = u32::from_le_bytes([body[4], body[5], body[6], body[7]]);
            bits = u16::from_le_bytes([body[14], body[15]]);
            if format == 0xFFFE && body.len() >= 26 {
                format = u16::from_le_bytes([body[24], body[25]]);
            }
        } else if id == b"data" {
            let step = (bits / 8) as usize * channels;
            let mut out = Vec::with_capacity(body.len() / step.max(1));
            for frame in body.chunks_exact(step) {
                let mut sum = 0.0f32;
                for c in 0..channels {
                    let s = &frame[c * (bits / 8) as usize..];
                    sum += match (format, bits) {
                        (1, 16) => f32::from(i16::from_le_bytes([s[0], s[1]])) / 32768.0,
                        (1, 24) => {
                            (i32::from_le_bytes([0, s[0], s[1], s[2]]) >> 8) as f32 / 8_388_608.0
                        }
                        (3, 32) => f32::from_le_bytes([s[0], s[1], s[2], s[3]]),
                        _ => return Err(format!("unsupported WAV: format {format}, {bits} bits")),
                    };
                }
                out.push(sum / channels as f32);
            }
            return Ok((out, f64::from(fs)));
        }
        pos += 8 + len + (len & 1);
    }
    Err("no data chunk".into())
}



fn write_wav(path: &str, x: &[f32], fs: f64) {
    let mut b = Vec::with_capacity(44 + x.len() * 4);
    let data = (x.len() * 4) as u32;
    b.extend_from_slice(b"RIFF");
    b.extend_from_slice(&(36 + data).to_le_bytes());
    b.extend_from_slice(b"WAVEfmt ");
    b.extend_from_slice(&16u32.to_le_bytes());
    b.extend_from_slice(&3u16.to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes());
    b.extend_from_slice(&(fs as u32).to_le_bytes());
    b.extend_from_slice(&(fs as u32 * 4).to_le_bytes());
    b.extend_from_slice(&4u16.to_le_bytes());
    b.extend_from_slice(&32u16.to_le_bytes());
    b.extend_from_slice(b"data");
    b.extend_from_slice(&data.to_le_bytes());
    for v in x {
        b.extend_from_slice(&v.to_le_bytes());
    }
    let _ = std::fs::write(path, b);
}

/// A one-pole-pair high pass at `f` (12 dB/oct), for the seam and band measures.
fn high_pass(x: &[f32], f: f64, fs: f64) -> Vec<f32> {
    let a = (-std::f64::consts::TAU * f / fs).exp();
    let mut out = vec![0.0f32; x.len()];
    let (mut x1, mut y1, mut y2, mut z1) = (0.0f64, 0.0f64, 0.0f64, 0.0f64);
    for (o, &v) in out.iter_mut().zip(x) {
        let v = f64::from(v);
        let y = a * (y1 + v - x1);
        x1 = v;
        y1 = y;
        let z = a * (y2 + y - z1);
        z1 = y;
        y2 = z;
        *o = z as f32;
    }
    out
}

fn db(a: f64, b: f64) -> f64 {
    10.0 * (a.max(1e-30) / b.max(1e-30)).log10()
}

fn pct(mut v: Vec<f64>, p: usize) -> f64 {
    if v.is_empty() {
        return f64::NAN;
    }
    v.sort_by(f64::total_cmp);
    v[(v.len() * p / 100).min(v.len() - 1)]
}

fn main() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (mut x, fs) = read_wav(args.first().ok_or("a take.wav")?)?;
    if let Some(sec) = args.get(1).and_then(|s| s.parse::<f64>().ok()) {
        x.truncate((sec * fs) as usize);
    }
    let out_dir = args.get(2);
    let a = analyse(&x, fs);
    println!("{:.1} s, {} notes", x.len() as f64 / fs, a.notes.len());
    if let Some(d) = out_dir {
        write_wav(&format!("{d}/take.wav"), &x, fs);
    }
    let n = 4096;
    let mut an = Analyzer::new(n);
    let bins = an.bins();
    // Power spectrum summed over the notes' middles.
    let mut spectrum = |v: &[f32]| -> Vec<f64> {
        let mut acc = vec![0.0f64; bins];
        let mut p = vec![0.0f32; bins];
        for note in &a.notes {
            let (s, e) = (note.start + (0.03 * fs) as usize, note.end.saturating_sub((0.03 * fs) as usize));
            let mut k = s;
            while k + n <= e {
                an.analyze_power(&v[k..k + n], &mut p);
                acc.iter_mut().zip(&p).for_each(|(a, b)| *a += f64::from(*b));
                k += n / 2;
            }
        }
        acc
    };
    let envelope = |p: &[f64]| -> Vec<f64> {
        // ⅓-octave smoothing of the log spectrum at log-spaced points, 200 Hz–6 kHz.
        (0..40)
            .map(|i| {
                let f = 200.0 * 30f64.powf(i as f64 / 39.0);
                let (lo, hi) = (f / 2f64.powf(1.0 / 6.0), f * 2f64.powf(1.0 / 6.0));
                let (b0, b1) = ((lo * n as f64 / fs) as usize, ((hi * n as f64 / fs) as usize).max((lo * n as f64 / fs) as usize + 1));
                let e: f64 = p[b0..b1.min(p.len())].iter().sum::<f64>() / (b1 - b0) as f64;
                10.0 * e.max(1e-30).log10()
            })
            .collect()
    };
    let band = |v: &[f32], lo: f64, hi: f64, ranges: &[(usize, usize)]| -> f64 {
        let hp = high_pass(v, lo, fs);
        let lp_a = (-std::f64::consts::TAU * hi / fs).exp();
        let mut s = 0.0f64;
        for &(r0, r1) in ranges {
            let mut y = 0.0f64;
            for &v in &hp[r0.min(hp.len())..r1.min(hp.len())] {
                y = lp_a * y + (1.0 - lp_a) * f64::from(v);
                s += y * y;
            }
        }
        s
    };
    let inside: Vec<(usize, usize)> = a.notes.iter().map(|n| (n.start, n.end)).collect();
    let consonants: Vec<(usize, usize)> = a.notes.iter().flat_map(|n| [(n.start - n.lead, n.start), (n.end, n.end + n.tail)]).filter(|r| r.1 > r.0).collect();
    let seams = |v: &[f32]| -> Vec<f64> {
        let hp = high_pass(v, 3000.0, fs);
        let w = (0.01 * fs) as usize;
        a.notes
            .iter()
            .flat_map(|n| [n.start, n.end])
            .map(|c| {
                let (r0, r1) = (c.saturating_sub(w).max(2), (c + w).min(hp.len()));
                (r0..r1).map(|k| f64::from((hp[k] - 2.0 * hp[k - 1] + hp[k - 2]).abs())).fold(1e-9, f64::max)
            })
            .collect()
    };
    let env_in = envelope(&spectrum(&x));
    let (air_in, cons_in, seams_in) = (band(&x, 4000.0, 12_000.0, &inside), band(&x, 3000.0, 14_000.0, &consonants), seams(&x));
    let centroid = |v: &[f32]| -> f64 {
        let mut an = Analyzer::new(n);
        let mut p = vec![0.0f32; an.bins()];
        let (mut num, mut den) = (0.0, 0.0);
        for note in &a.notes {
            let mut k = note.start;
            while k + n < note.end {
                an.analyze_power(&v[k..k + n], &mut p);
                for (b, pw) in p.iter().enumerate() {
                    let f = b as f64 * fs / n as f64;
                    if (200.0..4000.0).contains(&f) {
                        num += f * f64::from(*pw);
                        den += f64::from(*pw);
                    }
                }
                k += n;
            }
        }
        num / den
    };
    let c_in = centroid(&x);
    println!("{:>8} {:>12} {:>10} {:>12} {:>12} {:>12} {:>10}", "shift", "envelope dB", "air dB", "consonant dB", "seam med dB", "seam max dB", "centroid");
    for shift in [300.0, -300.0, 700.0, -700.0] {
        let edits = vec![NoteEdit { shift_cents: shift, ..NoteEdit::default() }; a.notes.len()];
        let y = render(&x, &a, &edits);
        if let Some(d) = out_dir {
            write_wav(&format!("{d}/shift{shift:+.0}.wav"), &y, fs);
        }
        let env_out = envelope(&spectrum(&y));
        let env_err = (env_out.iter().zip(&env_in).map(|(o, i)| (o - i).powi(2)).sum::<f64>() / env_in.len() as f64).sqrt();
        if std::env::var_os("MELODY_ENVELOPE").is_some() {
            let row: Vec<String> = env_out.iter().zip(&env_in).enumerate().filter(|(i, _)| i % 3 == 0).map(|(i, (o, x))| format!("{:.0}:{:+.1}", 200.0 * 30f64.powf(i as f64 / 39.0), o - x)).collect();
            println!("  {shift:+.0}: {}", row.join(" "));
        }
        let seams_out = seams(&y);
        let ratio: Vec<f64> = seams_out.iter().zip(&seams_in).map(|(o, i)| 20.0 * (o / i).log10()).collect();
        if std::env::var_os("MELODY_SEAMS").is_some() {
            let mut worst: Vec<(f64, usize)> = ratio.iter().copied().zip(0..).collect();
            worst.sort_by(|p, q| q.0.total_cmp(&p.0));
            for &(r, k) in worst.iter().take(5) {
                let note = &a.notes[k / 2];
                let at = if k % 2 == 0 { note.start } else { note.end };
                let legato_next = k % 2 == 1 && a.notes.get(k / 2 + 1).is_some_and(|m| (m.start as f64 - note.end as f64) < 0.03 * fs);
                println!("  {shift:+.0}: {r:+.1} dB at {:.3} s, note {} {} (lead {} tail {} ms){}", at as f64 / fs, k / 2, if k % 2 == 0 { "start" } else { "end" }, note.lead * 1000 / fs as usize, note.tail * 1000 / fs as usize, if legato_next { ", legato" } else { "" });
            }
        }
        println!(
            "{shift:>8.0} {env_err:>12.2} {:>10.2} {:>12.2} {:>12.2} {:>12.2} {:>+9.1}%",
            db(band(&y, 4000.0, 12_000.0, &inside), air_in),
            db(band(&y, 3000.0, 14_000.0, &consonants), cons_in),
            pct(ratio.clone(), 50),
            pct(ratio, 100),
            100.0 * (centroid(&y) / c_in - 1.0)
        );
    }
    Ok(())
}
