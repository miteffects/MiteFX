//! Minimal WAV I/O for the comparison: 32-bit float out, and float or integer PCM in (what Reaper
//! renders, with its extra chunks).

use std::path::Path;

/// Writes `channels` (each the same length) as a 32-bit float WAV.
pub fn write(path: &Path, rate: u32, channels: &[Vec<f32>]) -> Result<(), String> {
    let n = channels.first().map_or(0, Vec::len);
    let ch = channels.len() as u16;
    let data_len = (n * channels.len() * 4) as u32;
    let mut out = Vec::with_capacity(44 + data_len as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&3u16.to_le_bytes());
    out.extend_from_slice(&ch.to_le_bytes());
    out.extend_from_slice(&rate.to_le_bytes());
    out.extend_from_slice(&(rate * u32::from(ch) * 4).to_le_bytes());
    out.extend_from_slice(&(ch * 4).to_le_bytes());
    out.extend_from_slice(&32u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for i in 0..n {
        for c in channels {
            out.extend_from_slice(&c[i].to_le_bytes());
        }
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    std::fs::write(path, out).map_err(|e| format!("{}: {e}", path.display()))
}

/// Reads a WAV: (rate, one vector per channel).
pub fn read(path: &Path) -> Result<(u32, Vec<Vec<f32>>), String> {
    let d = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if d.len() < 12 || &d[0..4] != b"RIFF" || &d[8..12] != b"WAVE" {
        return Err(format!("{}: not a WAV file", path.display()));
    }
    let u16_at = |i: usize| u16::from_le_bytes([d[i], d[i + 1]]);
    let u32_at = |i: usize| u32::from_le_bytes([d[i], d[i + 1], d[i + 2], d[i + 3]]);
    let (mut format, mut channels, mut rate, mut bits) = (0u16, 0usize, 0u32, 0u16);
    let mut i = 12;
    while i + 8 <= d.len() {
        let id = &d[i..i + 4];
        let size = u32_at(i + 4) as usize;
        let body = i + 8;
        if id == b"fmt " {
            format = u16_at(body);
            channels = usize::from(u16_at(body + 2));
            rate = u32_at(body + 4);
            bits = u16_at(body + 14);
            // WAVE_FORMAT_EXTENSIBLE: the real format is the sub-format GUID's first two bytes.
            if format == 0xFFFE && size >= 26 {
                format = u16_at(body + 24);
            }
        } else if id == b"data" {
            let end = (body + size).min(d.len());
            let bytes = usize::from(bits / 8);
            if channels == 0 || bytes == 0 {
                return Err(format!("{}: data before fmt", path.display()));
            }
            let frames = (end - body) / (bytes * channels);
            let mut out = vec![Vec::with_capacity(frames); channels];
            for f in 0..frames {
                for (c, ch) in out.iter_mut().enumerate() {
                    let at = body + (f * channels + c) * bytes;
                    let v = match (format, bits) {
                        (3, 32) => f32::from_le_bytes([d[at], d[at + 1], d[at + 2], d[at + 3]]),
                        (3, 64) => f64::from_le_bytes(d[at..at + 8].try_into().map_err(|_| "short sample")?) as f32,
                        (1, 16) => f32::from(i16::from_le_bytes([d[at], d[at + 1]])) / 32768.0,
                        (1, 24) => (i32::from_le_bytes([0, d[at], d[at + 1], d[at + 2]]) >> 8) as f32 / 8_388_608.0,
                        (1, 32) => i32::from_le_bytes([d[at], d[at + 1], d[at + 2], d[at + 3]]) as f32 / 2_147_483_648.0,
                        _ => return Err(format!("{}: unsupported format {format}/{bits}", path.display())),
                    };
                    ch.push(v);
                }
            }
            return Ok((rate, out));
        }
        i = body + size + (size & 1);
    }
    Err(format!("{}: no data chunk", path.display()))
}
