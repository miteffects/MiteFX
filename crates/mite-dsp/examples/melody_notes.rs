//! Prints a take's notes as the analysis finds them (start s, length ms, centre in cents, the
//! spread of its drift, its level, and how many of its frames jump more than 700 cents from the
//! frame before: an octave error inside the note), for checking detection on real takes.
//!
//!     cargo run --release -p mite-dsp --example melody_notes -- take.wav [seconds]

use mite_dsp::melody::analyse;

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



fn main() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (mut x, fs) = read_wav(args.first().ok_or("a take.wav")?)?;
    if let Some(sec) = args.get(1).and_then(|s| s.parse::<f64>().ok()) {
        x.truncate((sec * fs) as usize);
    }
    let t0 = std::time::Instant::now();
    let a = analyse(&x, fs);
    if std::env::var_os("MELODY_TIME").is_some() {
        let analysed = t0.elapsed();
        let mut b = a.clone();
        let t1 = std::time::Instant::now();
        b.move_reading(&x, a.notes.len() / 2, 1200.0);
        println!("# analysis {analysed:.2?}, a reading moved {:.2?}", t1.elapsed());
    }
    for (i, n) in a.notes.iter().enumerate() {
        let f0 = n.start / a.hop;
        let cents: Vec<Option<f64>> = (f0..n.end / a.hop).map(|f| a.frames.get(f).and_then(|fr| fr.cents)).collect();
        let jumps = cents.windows(2).filter(|w| matches!(w, [Some(p), Some(q)] if (p - q).abs() > 700.0)).count();
        if std::env::var_os("MELODY_JUMPS").is_some() {
            let seq: Vec<String> = cents.iter().map(|c| c.map_or("-".into(), |v| format!("{v:.0}"))).collect();
            if jumps > 0 {
                println!("# note {i} at {:.3}: {}", n.start as f64 / fs, seq.join(" "));
            }
        }
        let spread = (n.drift.iter().map(|d| d * d).sum::<f64>() / n.drift.len().max(1) as f64).sqrt();
        println!("{i} {:.3} {:.0} {:.0} {:.0} {:.1} {jumps}", n.start as f64 / fs, (n.end - n.start) as f64 / fs * 1000.0, n.cents, spread, n.db);
    }
    Ok(())
}
