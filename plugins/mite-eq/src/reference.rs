//! EQ Match's reference from a file: decoded (WAV, AIFF, FLAC, MP3, AAC/ALAC in MP4, Ogg Vorbis)
//! and summed to mono as it streams, into a [`RecordingSpectrum`]. Runs on a worker thread.

use std::path::Path;

use mite_dsp::matching::RecordingSpectrum;
use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::{DecoderOptions, CODEC_TYPE_NULL};
use symphonia::core::errors::Error;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

/// The file extensions offered in the dialog.
pub const EXTENSIONS: &[&str] = &["wav", "wave", "aif", "aiff", "flac", "mp3", "m4a", "mp4", "aac", "ogg"];

/// Below this everywhere a frame is silence, dB as drawn.
const FLOOR_DB: f64 = -80.0;

/// The file's long-term spectrum, dB at the analyzer's points.
pub fn analyze(path: &Path) -> Result<Vec<f64>, String> {
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let stream = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }
    let probed = symphonia::default::get_probe()
        .format(&hint, stream, &FormatOptions::default(), &MetadataOptions::default())
        .map_err(|_| "not an audio file it can read".to_owned())?;
    let mut format = probed.format;
    let track = format.tracks().iter().find(|t| t.codec_params.codec != CODEC_TYPE_NULL).ok_or("no audio in it")?;
    let track_id = track.id;
    let rate = track.codec_params.sample_rate.ok_or("no sample rate")?;
    let mut decoder = symphonia::default::get_codecs().make(&track.codec_params, &DecoderOptions::default()).map_err(|_| "its codec is not supported".to_owned())?;
    let mut spectrum = RecordingSpectrum::new(f64::from(rate), FLOOR_DB);
    let mut buffer: Option<SampleBuffer<f32>> = None;
    let mut mono = Vec::new();
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
            // A damaged packet is skipped, as players do.
            Err(Error::DecodeError(_)) => continue,
            Err(e) => return Err(e.to_string()),
        };
        let channels = decoded.spec().channels.count().max(1);
        let buf = buffer.get_or_insert_with(|| SampleBuffer::new(decoded.capacity() as u64, *decoded.spec()));
        if buf.capacity() < decoded.capacity() * channels {
            *buf = SampleBuffer::new(decoded.capacity() as u64, *decoded.spec());
        }
        buf.copy_interleaved_ref(decoded);
        mono.clear();
        mono.extend(buf.samples().chunks(channels).map(|frame| frame.iter().sum::<f32>() / channels as f32));
        spectrum.push(&mono);
    }
    if spectrum.seconds() == 0.0 && mono.is_empty() {
        return Err("it holds no sound".to_owned());
    }
    Ok(spectrum.finish())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 16-bit PCM WAV file.
    pub fn write_wav(path: &Path, rate: u32, channels: u16, samples: &[f32]) {
        let data: Vec<u8> = samples.iter().flat_map(|&s| ((s.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes()).collect();
        let mut out = Vec::new();
        out.extend(b"RIFF");
        out.extend((36 + data.len() as u32).to_le_bytes());
        out.extend(b"WAVEfmt ");
        out.extend(16u32.to_le_bytes());
        out.extend(1u16.to_le_bytes());
        out.extend(channels.to_le_bytes());
        out.extend(rate.to_le_bytes());
        out.extend((rate * u32::from(channels) * 2).to_le_bytes());
        out.extend((channels * 2).to_le_bytes());
        out.extend(16u16.to_le_bytes());
        out.extend(b"data");
        out.extend((data.len() as u32).to_le_bytes());
        out.extend(data);
        std::fs::write(path, out).unwrap();
    }

    #[test]
    fn a_stereo_wav_reads_as_its_tone() {
        let dir = std::env::temp_dir().join(format!("mite-eq-ref-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("tone.wav");
        let rate = 44_100;
        let samples: Vec<f32> = (0..rate * 2).flat_map(|n| {
            let s = (std::f64::consts::TAU * 250.0 * f64::from(n) / f64::from(rate)).sin() as f32 * 0.5;
            [s, s]
        }).collect();
        write_wav(&path, rate, 2, &samples);
        let db = analyze(&path).unwrap();
        let peak = (0..db.len()).max_by(|&a, &b| db[a].total_cmp(&db[b])).unwrap();
        let f = mite_dsp::analyzer::spectrum_point_freq(peak);
        assert!((f / 250.0).log2().abs() < 0.05, "peak at {f} Hz");
        assert!(analyze(&dir.join("missing.wav")).is_err());
        std::fs::write(dir.join("junk.wav"), b"not audio").unwrap();
        assert!(analyze(&dir.join("junk.wav")).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
