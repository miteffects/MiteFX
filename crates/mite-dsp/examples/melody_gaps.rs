//! How much of a take's audible sound the notes leave out, and what kind: frames (5 ms) louder than
//! 40 dB under the take's loud level that no note covers, split by the pitch tracker's verdict
//! (unvoiced: consonants, breaths; voiced but cut: pitched sound the notes missed).
//!
//!     cargo run --release -p mite-dsp --example melody_gaps -- take.wav

use mite_dsp::melody::analyse;

fn main() -> Result<(), String> {
    let path = std::env::args().nth(1).ok_or("a take.wav")?;
    let b = std::fs::read(&path).map_err(|e| e.to_string())?;
    // 24-bit or 16-bit PCM, or float, summed to mono (as melody_render reads it).
    let (mut ch, mut bits, mut fmt, mut fs) = (1usize, 16u16, 1u16, 44_100u32);
    let mut pos = 12;
    let mut x: Vec<f32> = Vec::new();
    while pos + 8 <= b.len() {
        let id = &b[pos..pos + 4];
        let len = u32::from_le_bytes([b[pos + 4], b[pos + 5], b[pos + 6], b[pos + 7]]) as usize;
        let body = &b[pos + 8..(pos + 8 + len).min(b.len())];
        if id == b"fmt " {
            fmt = u16::from_le_bytes([body[0], body[1]]);
            ch = u16::from_le_bytes([body[2], body[3]]) as usize;
            fs = u32::from_le_bytes([body[4], body[5], body[6], body[7]]);
            bits = u16::from_le_bytes([body[14], body[15]]);
            if fmt == 0xFFFE {
                fmt = u16::from_le_bytes([body[24], body[25]]);
            }
        } else if id == b"data" {
            let w = (bits / 8) as usize;
            for f in body.chunks_exact(w * ch) {
                let mut s = 0.0;
                for c in 0..ch {
                    let v = &f[c * w..];
                    s += match (fmt, bits) {
                        (1, 16) => f32::from(i16::from_le_bytes([v[0], v[1]])) / 32768.0,
                        (1, 24) => (i32::from_le_bytes([0, v[0], v[1], v[2]]) >> 8) as f32 / 8_388_608.0,
                        (3, 32) => f32::from_le_bytes([v[0], v[1], v[2], v[3]]),
                        _ => 0.0,
                    };
                }
                x.push(s / ch as f32);
            }
        }
        pos += 8 + len + (len & 1);
    }
    let fs = f64::from(fs);
    let a = analyse(&x, fs);
    let mut lv: Vec<f64> = a.frames.iter().map(|f| f.db).collect();
    lv.sort_by(f64::total_cmp);
    let loud = lv[lv.len() * 95 / 100];
    let audible = |db: f64| db > loud - 40.0;
    let covered = |k: usize| a.notes.iter().any(|n| k * a.hop >= n.span().0 && k * a.hop < n.span().1);
    let (mut total, mut out_unvoiced, mut out_voiced, mut e_total, mut e_out) = (0, 0, 0, 0.0, 0.0);
    let mut gaps: Vec<(f64, f64, bool)> = Vec::new();
    for (k, f) in a.frames.iter().enumerate() {
        if !audible(f.db) {
            continue;
        }
        total += 1;
        let e = 10f64.powf(f.db / 10.0);
        e_total += e;
        if !covered(k) {
            e_out += e;
            if f.cents.is_some() { out_voiced += 1 } else { out_unvoiced += 1 }
            gaps.push((k as f64 * a.hop as f64 / fs, f.db, f.cents.is_some()));
        }
    }
    println!("{:.0} s, {} notes; audible frames {total}", x.len() as f64 / fs, a.notes.len());
    println!("outside notes: {} unvoiced (consonants, breaths), {} voiced (pitched, missed): {:.0} % of audible time, {:.1} % of its energy",
        out_unvoiced, out_voiced, 100.0 * (out_unvoiced + out_voiced) as f64 / total as f64, 100.0 * e_out / e_total);
    // The longest stretches outside notes.
    let mut runs: Vec<(f64, f64, usize, f64)> = Vec::new();
    for g in &gaps {
        match runs.last_mut() {
            Some(r) if g.0 - r.1 < 0.011 => { r.1 = g.0; if g.2 { r.2 += 1 } r.3 = r.3.max(g.1) }
            _ => runs.push((g.0, g.0, usize::from(g.2), g.1)),
        }
    }
    runs.sort_by(|a, b| (b.1 - b.0).total_cmp(&(a.1 - a.0)));
    for r in runs.iter().take(8) {
        println!("  {:7.2}–{:7.2} s ({:.0} ms): {} voiced frames, peak {:.0} dB (loud {:.0})", r.0, r.1, (r.1 - r.0) * 1000.0, r.2, r.3, loud);
    }
    // Inside the loudest stretches: what the tracker heard (clarity; 0.8 is the voiced bar).
    let mut det = mite_dsp::pitch::PitchDetector::new(((0.04 * fs) as usize).next_power_of_two());
    let win = det.len();
    for r in runs.iter().filter(|r| r.3 > loud - 6.0).take(3) {
        let line: Vec<String> = (0..10)
            .map(|i| {
                let c = ((r.0 + (r.1 - r.0) * i as f64 / 10.0) * fs) as usize;
                match (c >= win / 2).then(|| det.detect(&x[c - win / 2..(c + win / 2).min(x.len())], fs)).flatten() {
                    Some(p) => format!("{:.0}Hz/{:.2}", p.hz, p.clarity),
                    None => "-".into(),
                }
            })
            .collect();
        println!("  {:.2} s: {}", r.0, line.join(" "));
    }
    Ok(())
}
