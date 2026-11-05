//! Mite Pitch's engine, measured on a take: every note shifted by a few amounts, the output's
//! periodicity (McLeod clarity) and pitch jitter against the input's, frame by frame through the
//! notes' middles. A broken resynthesis loses clarity and gains jitter.
//!
//!     cargo run --release -p mite-dsp --example melody_quality -- take.wav [seconds]

use mite_dsp::melody::{analyse, hz_to_cents, render, NoteEdit};
use mite_dsp::pitch::PitchDetector;

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


struct Stats {
    clarity: f64,
    clarity_lo: f64,
    error_hi: f64,
    jitter: f64,
    error: f64,
}

fn median(v: Vec<f64>) -> f64 {
    pct(v, 50)
}

fn pct(mut v: Vec<f64>, p: usize) -> f64 {
    if v.is_empty() {
        return f64::NAN;
    }
    v.sort_by(f64::total_cmp);
    v[(v.len() * p / 100).min(v.len() - 1)]
}

/// Frames through the notes' middles (30 ms in from each end): clarity, frame-to-frame pitch
/// change, pitch error against the input shifted by `shift`.
fn measure(x: &[f32], y: &[f32], fs: f64, notes: &[(usize, usize)], shift: f64) -> (Stats, Stats) {
    let win = ((0.04 * fs) as usize).next_power_of_two();
    let hop = (0.005 * fs) as usize;
    let mut det = PitchDetector::new(win);
    let mut track = |s: &[f32], c: usize| {
        if c < win / 2 || c + win / 2 > s.len() {
            return None;
        }
        det.detect(&s[c - win / 2..c + win / 2], fs)
    };
    let (mut ci, mut co, mut ji, mut jo, mut err) = (vec![], vec![], vec![], vec![], vec![]);
    let edge = (0.03 * fs) as usize;
    for &(s, e) in notes {
        let (mut pi, mut po): (Option<f64>, Option<f64>) = (None, None);
        let mut c = s + edge;
        while c + edge < e {
            if let (Some(a), Some(b)) = (track(x, c), track(y, c)) {
                if a.clarity > 0.8 {
                    ci.push(a.clarity);
                    co.push(b.clarity);
                    let (ca, cb) = (hz_to_cents(a.hz), hz_to_cents(b.hz));
                    err.push((cb - ca - shift).abs());
                    if let (Some(p), Some(q)) = (pi, po) {
                        ji.push((ca - p).abs());
                        jo.push((cb - q).abs());
                    }
                    pi = Some(ca);
                    po = Some(cb);
                } else {
                    pi = None;
                    po = None;
                }
            }
            c += hop;
        }
    }
    (Stats { clarity_lo: pct(ci.clone(), 5), clarity: median(ci), jitter: median(ji), error: 0.0, error_hi: 0.0 }, Stats { clarity_lo: pct(co.clone(), 5), clarity: median(co), jitter: median(jo), error_hi: pct(err.clone(), 95), error: median(err) })
}

fn main() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (mut x, fs) = read_wav(args.first().ok_or("a take.wav")?)?;
    if let Some(sec) = args.get(1).and_then(|s| s.parse::<f64>().ok()) {
        x.truncate((sec * fs) as usize);
    }
    let a = analyse(&x, fs);
    let notes: Vec<(usize, usize)> = a.notes.iter().map(|n| (n.start, n.end)).collect();
    println!("{:.1} s, {} notes", x.len() as f64 / fs, notes.len());
    println!("{:>8} {:>10} {:>10} {:>10} {:>10} {:>10} {:>10} {:>10} {:>10}", "shift", "clar in", "clar out", "5% in", "5% out", "jit in", "jit out", "err", "err 95%");
    for shift in [0.0, 50.0, -50.0, 200.0, -200.0, 500.0, -500.0] {
        let edits = vec![NoteEdit { shift_cents: shift, ..NoteEdit::default() }; notes.len()];
        // A tiny edit so even 0 goes through the resynthesis.
        let edits: Vec<NoteEdit> = edits.into_iter().map(|e| NoteEdit { gain_db: 1e-6, ..e }).collect();
        let y = render(&x, &a, &edits);
        let (i, o) = measure(&x, &y, fs, &notes, shift);
        println!("{shift:>8.0} {:>10.4} {:>10.4} {:>10.4} {:>10.4} {:>10.2} {:>10.2} {:>10.2} {:>10.2}", i.clarity, o.clarity, i.clarity_lo, o.clarity_lo, i.jitter, o.jitter, o.error, o.error_hi);
        if shift == 0.0 {
            // At no shift a resynthesis should give back the input: its error, voiced notes only.
            let (mut sig, mut res) = (0.0f64, 0.0f64);
            for &(s, e) in &notes {
                for k in s..e.min(y.len()) {
                    sig += f64::from(x[k]).powi(2);
                    res += f64::from(y[k] - x[k]).powi(2);
                }
            }
            println!("         resynthesis at no shift: {:.1} dB under the take", 10.0 * (sig / res.max(1e-30)).log10());
        }
    }
    // Formants: the spectral centroid (200 Hz–4 kHz) over the notes, moved.
    let centroid = |v: &[f32]| {
        let n = 4096;
        let mut an = mite_dsp::analyzer::Analyzer::new(n);
        let mut p = vec![0.0f32; an.bins()];
        let (mut num, mut den) = (0.0, 0.0);
        for &(s, e) in &notes {
            let mut k = s;
            while k + n < e {
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
    let was = centroid(&x);
    for f in [-6.0, -3.0, 3.0, 6.0] {
        let y = render(&x, &a, &vec![NoteEdit { formant_semitones: f, ..NoteEdit::default() }; notes.len()]);
        println!("formant {f:+}: centroid {was:.0} → {:.0} Hz ({:+.1} %)", centroid(&y), (centroid(&y) / was - 1.0) * 100.0);
    }
    Ok(())
}
