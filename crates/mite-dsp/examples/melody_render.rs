//! Mite Pitch's engine on a take, for listening (docs/design/pitch-brief.md, slice 1): analyses
//! it into notes and writes one WAV per kind of edit next to `out_dir`.
//!
//!     cargo run --release -p mite-dsp --example melody_render -- [take.wav] [out_dir]
//!
//! Without a take, a synthetic sung line. A take is read as 16/24-bit PCM or 32-bit float WAV,
//! summed to mono. Written: original, corrected (every note centred on its semitone), up3 (all
//! notes three semitones up), flat (no vibrato), wide (vibrato doubled), formant+3, formant-3,
//! slower (every note 25 % longer), quieter_odd (every other note −6 dB).

use mite_dsp::melody::{NoteEdit, analyse, render, voice};

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

fn write_wav(path: &std::path::Path, x: &[f32], fs: f64) -> std::io::Result<()> {
    let data = (x.len() * 4) as u32;
    let mut b = Vec::with_capacity(44 + data as usize);
    b.extend_from_slice(b"RIFF");
    b.extend_from_slice(&(36 + data).to_le_bytes());
    b.extend_from_slice(b"WAVEfmt ");
    b.extend_from_slice(&16u32.to_le_bytes());
    b.extend_from_slice(&3u16.to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes());
    b.extend_from_slice(&(fs as u32).to_le_bytes());
    b.extend_from_slice(&((fs as u32) * 4).to_le_bytes());
    b.extend_from_slice(&4u16.to_le_bytes());
    b.extend_from_slice(&32u16.to_le_bytes());
    b.extend_from_slice(b"data");
    b.extend_from_slice(&data.to_le_bytes());
    for v in x {
        b.extend_from_slice(&v.to_le_bytes());
    }
    std::fs::write(path, b)
}

fn main() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (x, fs) = match args.first().filter(|a| a.ends_with(".wav")) {
        Some(p) => read_wav(p)?,
        None => {
            let n = |cents: f64, seconds: f64| voice::Sung {
                cents,
                seconds,
                vibrato_cents: 35.0,
                vibrato_hz: 5.5,
                db: -12.0,
            };
            // A little out of tune on purpose (so "corrected" has work to do).
            let (x, _) = voice::sing(
                &[
                    n(5710.0, 0.6),
                    n(5925.0, 0.5),
                    n(6215.0, 0.7),
                    n(5885.0, 0.55),
                    n(5730.0, 0.9),
                ],
                0.08,
                48_000.0,
            );
            (x, 48_000.0)
        }
    };
    let out_dir = std::path::PathBuf::from(
        args.iter()
            .find(|a| !a.ends_with(".wav"))
            .cloned()
            .unwrap_or_else(|| ".".into()),
    );
    std::fs::create_dir_all(&out_dir).map_err(|e| e.to_string())?;
    let t0 = std::time::Instant::now();
    let a = analyse(&x, fs);
    let analysed = t0.elapsed();
    println!(
        "{:.1} s of audio at {fs} Hz: {} notes, analysed in {:.0} ms",
        x.len() as f64 / fs,
        a.notes.len(),
        analysed.as_secs_f64() * 1000.0
    );
    for (i, n) in a.notes.iter().enumerate() {
        let name = [
            "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
        ][((n.cents / 100.0).round() as i64).rem_euclid(12) as usize];
        println!(
            "  note {i:2}: {:7.3}–{:7.3} s  {name}{}  {:+5.1} cents off  {:6.1} dBFS",
            n.start as f64 / fs,
            n.end as f64 / fs,
            (n.cents / 100.0).round() as i64 / 12 - 1,
            n.cents - (n.cents / 100.0).round() * 100.0,
            n.db
        );
    }
    let each = |f: &dyn Fn(usize, &mite_dsp::melody::Note) -> NoteEdit| {
        a.notes
            .iter()
            .enumerate()
            .map(|(i, n)| f(i, n))
            .collect::<Vec<_>>()
    };
    let versions: Vec<(&str, Vec<NoteEdit>)> = vec![
        ("original", each(&|_, _| NoteEdit::default())),
        (
            "corrected",
            each(&|_, n| NoteEdit {
                shift_cents: (n.cents / 100.0).round() * 100.0 - n.cents,
                ..NoteEdit::default()
            }),
        ),
        (
            "up3",
            each(&|_, _| NoteEdit {
                shift_cents: 300.0,
                ..NoteEdit::default()
            }),
        ),
        (
            "flat",
            each(&|_, _| NoteEdit {
                drift: 0.0,
                ..NoteEdit::default()
            }),
        ),
        (
            "wide",
            each(&|_, _| NoteEdit {
                drift: 2.0,
                ..NoteEdit::default()
            }),
        ),
        (
            "formant+3",
            each(&|_, _| NoteEdit {
                formant_semitones: 3.0,
                ..NoteEdit::default()
            }),
        ),
        (
            "formant-3",
            each(&|_, _| NoteEdit {
                formant_semitones: -3.0,
                ..NoteEdit::default()
            }),
        ),
        (
            "slower",
            each(&|_, n| NoteEdit {
                stretch: 1.25,
                offset: (n.start as f64) * 0.25,
                ..NoteEdit::default()
            }),
        ),
        (
            "quieter_odd",
            each(&|i, _| NoteEdit {
                gain_db: if i % 2 == 1 { -6.0 } else { 0.0 },
                ..NoteEdit::default()
            }),
        ),
    ];
    for (name, edits) in versions {
        let t0 = std::time::Instant::now();
        let y = render(&x, &a, &edits);
        let path = out_dir.join(format!("{name}.wav"));
        write_wav(&path, &y, fs).map_err(|e| e.to_string())?;
        println!(
            "wrote {} ({:.0} ms)",
            path.display(),
            t0.elapsed().as_secs_f64() * 1000.0
        );
    }
    Ok(())
}
