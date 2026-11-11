//! Cabinet impulse responses: decoded from a file (WAV, AIFF, FLAC; a stereo IR is summed),
//! resampled to the host's rate, cut to [`MAX_SECONDS`], normalised (0 dB on average over
//! 100 Hz…5 kHz, where a guitar lives: swapping cabinets does not jump in level, and the response
//! is the same at every host and file rate) and split for zero-latency convolution: the first
//! [`HEAD`] taps run directly, the rest in an FFT convolver whose block delay is exactly where
//! they belong.

use std::path::Path;

use mite_dsp::conv::{KernelLayout, KernelSet};
use mite_dsp::resample::SincTable;
use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::{DecoderOptions, CODEC_TYPE_NULL};
use symphonia::core::errors::Error;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

/// The file extensions offered for cabinets.
pub const EXTENSIONS: &[&str] = &["wav", "wave", "aif", "aiff", "flac"];
/// Longest response kept (longer ones fade out over the last 10 ms).
pub const MAX_SECONDS: f64 = 0.5;
/// Taps run directly (no latency); also the FFT convolver's block.
pub const HEAD: usize = 64;
/// Points of the drawn magnitude response, log-spaced 20 Hz…20 kHz.
pub const RESPONSE_POINTS: usize = 256;

/// The frequency of response point `i`.
pub fn response_freq(i: usize) -> f64 {
    20.0 * 1000f64.powf(i as f64 / (RESPONSE_POINTS - 1) as f64)
}

/// A response ready for the convolver.
pub struct Cab {
    /// The first [`HEAD`] taps.
    pub head: [f32; HEAD],
    /// The rest (taps HEAD…), partitioned; none when the response is that short.
    pub tail: Option<Box<KernelSet>>,
    /// The tail's length, taps (the convolver must hold it).
    pub tail_len: usize,
}

/// What the editor shows about a loaded response.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct IrInfo {
    pub file_rate: f64,
    pub seconds: f64,
    pub channels: usize,
    pub truncated: bool,
    /// Magnitude at the response points, dB (normalised as it plays).
    pub response_db: Vec<f32>,
}

/// Decodes an audio file to mono samples and its rate.
pub fn decode(path: &Path) -> Result<(Vec<f32>, f64, usize), String> {
    let file = std::fs::File::open(path).map_err(|e| format!("cannot open it ({e})"))?;
    let stream = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }
    let probed = symphonia::default::get_probe().format(&hint, stream, &FormatOptions::default(), &MetadataOptions::default()).map_err(|_| "not an audio file it can read".to_owned())?;
    let mut format = probed.format;
    let track = format.tracks().iter().find(|t| t.codec_params.codec != CODEC_TYPE_NULL).ok_or("no audio in it")?;
    let track_id = track.id;
    let rate = f64::from(track.codec_params.sample_rate.ok_or("no sample rate")?);
    let mut decoder = symphonia::default::get_codecs().make(&track.codec_params, &DecoderOptions::default()).map_err(|_| "its codec is not supported".to_owned())?;
    let mut buffer: Option<SampleBuffer<f32>> = None;
    let mut mono = Vec::new();
    let mut channels = 1;
    // Far more than any cabinet: stop reading a long file early.
    let limit = (rate * MAX_SECONDS * 2.0) as usize + 1024;
    loop {
        let packet = match format.next_packet() {
            Ok(p) => p,
            Err(Error::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(Error::ResetRequired) => break,
            Err(e) => return Err(e.to_string()),
        };
        if packet.track_id() != track_id {
            continue;
        }
        let decoded = match decoder.decode(&packet) {
            Ok(d) => d,
            Err(Error::DecodeError(_)) => continue,
            Err(e) => return Err(e.to_string()),
        };
        channels = decoded.spec().channels.count().max(1);
        let buf = buffer.get_or_insert_with(|| SampleBuffer::new(decoded.capacity() as u64, *decoded.spec()));
        if buf.capacity() < decoded.capacity() * channels {
            *buf = SampleBuffer::new(decoded.capacity() as u64, *decoded.spec());
        }
        buf.copy_interleaved_ref(decoded);
        mono.extend(buf.samples().chunks(channels).map(|f| f.iter().sum::<f32>() / channels as f32));
        if mono.len() > limit {
            break;
        }
    }
    if mono.is_empty() {
        return Err("it holds no sound".to_owned());
    }
    Ok((mono, rate, channels))
}

/// The response at `host_rate`: resampled, cut, normalised. Returns it and whether it was cut.
pub fn prepare(raw: &[f32], file_rate: f64, host_rate: f64) -> Result<(Vec<f64>, bool), String> {
    let mut h: Vec<f64> = if (file_rate - host_rate).abs() < 0.5 {
        raw.iter().map(|&v| f64::from(v)).collect()
    } else {
        let table = SincTable::for_rates(file_rate, host_rate);
        let step = file_rate / host_rate;
        let n = ((raw.len() as f64) / step).ceil() as usize;
        // Scaled by the rate ratio: the same continuous response, sampled anew.
        (0..n).map(|k| f64::from(table.read(raw, k as f64 * step)) * step).collect()
    };
    let max = (MAX_SECONDS * host_rate) as usize;
    let truncated = h.len() > max;
    if truncated {
        h.truncate(max);
        let fade = ((0.01 * host_rate) as usize).min(max);
        for i in 0..fade {
            let t = (i as f64 + 0.5) / fade as f64;
            h[max - fade + i] *= 0.5 + 0.5 * (std::f64::consts::PI * t).cos();
        }
    }
    // Trailing silence costs CPU for nothing.
    let floor = h.iter().fold(0.0f64, |m, v| m.max(v.abs())) * 1e-5;
    while h.len() > 1 && h.last().is_some_and(|v| v.abs() <= floor) {
        h.pop();
    }
    let energy: f64 = h.iter().map(|v| v * v).sum();
    if energy.is_nan() || energy <= 1e-20 || !energy.is_finite() {
        return Err("it is silent".to_owned());
    }
    // The mean power gain over the guitar's band, on a log frequency scale (as an amp's output
    // spreads its energy), brought to 0 dB.
    let band: Vec<f64> = (0..RESPONSE_POINTS).map(response_freq).filter(|f| (100.0..=5000.0).contains(f)).collect();
    let mean: f64 = band.iter().map(|&f| power_at(&h, f, host_rate)).sum::<f64>() / band.len() as f64;
    if mean.is_nan() || mean <= 1e-30 || !mean.is_finite() {
        return Err("it has nothing in the guitar's range".to_owned());
    }
    let g = 1.0 / mean.sqrt();
    h.iter_mut().for_each(|v| *v *= g);
    Ok((h, truncated))
}

/// |H(f)|² of a response at `rate`.
fn power_at(h: &[f64], f: f64, rate: f64) -> f64 {
    let w = std::f64::consts::TAU * f / rate;
    let (mut re, mut im) = (0.0, 0.0);
    let (c, s) = (w.cos(), w.sin());
    // Rotate a phasor instead of a sin/cos per tap.
    let (mut pr, mut pi) = (1.0f64, 0.0f64);
    for &v in h {
        re += v * pr;
        im -= v * pi;
        let nr = pr * c - pi * s;
        pi = pr * s + pi * c;
        pr = nr;
    }
    re * re + im * im
}

/// The magnitude response at the drawn points, dB.
pub fn response_db(h: &[f64], rate: f64) -> Vec<f32> {
    (0..RESPONSE_POINTS).map(|i| (10.0 * power_at(h, response_freq(i), rate).max(1e-20).log10()) as f32).collect()
}

/// Splits a prepared response for the convolver.
pub fn split(h: &[f64]) -> Cab {
    let mut head = [0.0f32; HEAD];
    for (d, s) in head.iter_mut().zip(h) {
        *d = *s as f32;
    }
    if h.len() <= HEAD {
        return Cab { head, tail: None, tail_len: 0 };
    }
    let tail = h[HEAD..].to_vec();
    let tail_len = tail.len();
    Cab { head, tail: Some(Box::new(KernelSet::new(HEAD, KernelLayout::Shared, &[tail], 0))), tail_len }
}

/// Loads a cabinet for `host_rate`. Allocates; worker thread.
pub fn load(path: &Path, host_rate: f64) -> Result<(Cab, IrInfo), String> {
    let (raw, file_rate, channels) = decode(path)?;
    let (h, truncated) = prepare(&raw, file_rate, host_rate)?;
    let info = IrInfo { file_rate, seconds: raw.len() as f64 / file_rate, channels, truncated, response_db: response_db(&h, host_rate) };
    Ok((split(&h), info))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_response_comes_out_at_0_db_in_the_guitar_band_and_holds_across_rates() {
        // A cabinet-like response: decaying, its energy below 8 kHz (a two-pole low pass of a
        // decaying noise), as real cabinets' are.
        let mut seed = 7u32;
        let (mut a, mut b) = (0.0f32, 0.0f32);
        let raw: Vec<f32> = (0..2000)
            .map(|i| {
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                let n = (seed >> 8) as f32 / 16_777_216.0 - 0.5;
                a += 0.35 * (n * (-(i as f32) / 300.0).exp() - a);
                b += 0.35 * (a - b);
                b
            })
            .collect();
        let (h48, _) = prepare(&raw, 48_000.0, 48_000.0).unwrap();
        let r = response_db(&h48, 48_000.0);
        let band: Vec<f64> = (0..RESPONSE_POINTS).filter(|&i| (100.0..=5000.0).contains(&response_freq(i))).map(|i| 10f64.powf(f64::from(r[i]) / 10.0)).collect();
        let mean_db = 10.0 * (band.iter().sum::<f64>() / band.len() as f64).log10();
        assert!(mean_db.abs() < 0.01, "{mean_db}");
        let (h96, _) = prepare(&raw, 48_000.0, 96_000.0).unwrap();
        assert!((h96.len() as f64 / h48.len() as f64 - 2.0).abs() < 0.05);
        let (h44, _) = prepare(&raw, 48_000.0, 44_100.0).unwrap();
        // The same response in the audio band at every rate.
        let (a, b, c) = (response_db(&h48, 48_000.0), response_db(&h96, 96_000.0), response_db(&h44, 44_100.0));
        for i in (0..200).step_by(10) {
            assert!((a[i] - b[i]).abs() < 0.2 && (a[i] - c[i]).abs() < 0.2, "{} Hz: {} {} {}", response_freq(i), a[i], b[i], c[i]);
        }
    }

    #[test]
    fn long_responses_are_cut_and_silent_ones_refused() {
        let raw = vec![0.1f32; 48_000];
        let (h, cut) = prepare(&raw, 48_000.0, 48_000.0).unwrap();
        assert!(cut);
        assert!(h.len() <= 24_000 && h.len() > 23_900, "{}", h.len());
        assert!(prepare(&[0.0; 100], 48_000.0, 48_000.0).is_err());
    }
}
