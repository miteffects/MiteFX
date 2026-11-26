//! Mite Tape's engine: the tape machine (`mite_dsp::tape`, oversampled 4×), mix against the
//! latency-aligned dry signal, output gain and bypass; and the telemetry: peak in and out per 10 ms
//! slice, whatever the host's block size, and the input and output for the spectrum.

use mite_core::{SpectrumTap, StereoMeter};
use mite_dsp::analyzer::{Analyzer, LogSpectrum, SPECTRUM_POINTS};
use mite_dsp::output::{DryDelay, LoudnessMatch};
use mite_dsp::tape::{Machine, MachineSettings, MachineTelemetry, MAX_BLOCK};
use mite_dsp::{db_to_gain, gain_to_db, LinearSmoother, MINUS_INFINITY_DB};

use crate::params::TapeSettings;

/// The machine's oversampling (Sat's default; the core's harmonics fold back far less).
pub const FACTOR: usize = 4;
/// Peak meters: one reading per slice this long.
const METER_SLICE_MS: f32 = 10.0;
/// Auto gain gives back at most this much either way, dB.
pub const AUTO_MAX_DB: f32 = 12.0;

pub struct TapeEngine {
    machine: Machine,
    dry: DryDelay,
    wet: [Vec<f32>; 2],
    mix: LinearSmoother,
    gain: LinearSmoother,
    bypass: LinearSmoother,
    /// Auto gain: the tape's output (before its own gain) against the input, both in time.
    auto: LoudnessMatch,
    fs: f32,
    slice_len: usize,
    slice_pos: usize,
    /// Peak in L, R and out L, R over the slice so far (linear).
    peaks: [f32; 4],
}

impl TapeEngine {
    pub fn new(sample_rate: f32, s: &TapeSettings) -> Self {
        let mut e = Self {
            machine: Machine::new(sample_rate, FACTOR, &s.machine),
            dry: DryDelay::new(1, MAX_BLOCK),
            wet: [vec![0.0; MAX_BLOCK], vec![0.0; MAX_BLOCK]],
            mix: LinearSmoother::new(1.0),
            gain: LinearSmoother::new(1.0),
            bypass: LinearSmoother::new(0.0),
            auto: LoudnessMatch::new(sample_rate, AUTO_MAX_DB),
            fs: sample_rate,
            slice_len: 1,
            slice_pos: 0,
            peaks: [0.0; 4],
        };
        e.prepare(sample_rate, s);
        e
    }

    /// Sizes everything for `sample_rate`. Allocates.
    pub fn prepare(&mut self, sample_rate: f32, s: &TapeSettings) {
        self.fs = sample_rate;
        self.auto = LoudnessMatch::new(sample_rate, AUTO_MAX_DB);
        self.machine.prepare(sample_rate, FACTOR, &s.machine);
        let latency = self.machine.latency();
        self.dry = DryDelay::new(latency, MAX_BLOCK);
        self.dry.set_delay(latency);
        for sm in [&mut self.mix, &mut self.gain, &mut self.bypass] {
            sm.set_ramp(sample_rate, 20.0);
        }
        // Bypass and mix fade as S curves (a straight fade kinks where it starts and ends).
        self.bypass.set_s_curve(true);
        self.mix.set_s_curve(true);
        self.slice_len = ((METER_SLICE_MS * 0.001 * sample_rate).round() as usize).max(1);
        self.reset(s);
    }

    /// Clears the state and snaps every glide to `s`.
    pub fn reset(&mut self, s: &TapeSettings) {
        self.machine.reset(&s.machine);
        self.dry.reset();
        self.mix.reset(s.mix.clamp(0.0, 1.0));
        self.gain.reset(db_to_gain(s.out_db));
        self.bypass.reset(if s.bypass { 1.0 } else { 0.0 });
        self.auto.reset(s.auto_gain);
        self.slice_pos = 0;
        self.peaks = [0.0; 4];
    }

    /// The latency, samples (the oversampler's round trip and the transport's centre; constant).
    pub fn latency(&self) -> usize {
        self.machine.latency()
    }

    /// What auto gain gives back now, dB (0 when off).
    pub fn auto_gain_db(&self) -> f32 {
        self.auto.gain_db()
    }

    pub fn telemetry(&self) -> MachineTelemetry {
        self.machine.telemetry()
    }

    /// One or two channels in place. `meters`: input and output peaks; `spectra`: the input and
    /// the output (mono).
    #[allow(clippy::needless_range_loop)] // channels and their buffers indexed together
    pub fn process(&mut self, channels: &mut [&mut [f32]], s: &TapeSettings, meters: Option<[&StereoMeter; 2]>, spectra: Option<[&SpectrumTap; 2]>) {
        let n_ch = channels.len().min(2);
        if n_ch == 0 {
            return;
        }
        let len = channels.iter().map(|c| c.len()).min().unwrap_or(0);
        self.mix.set_target(s.mix.clamp(0.0, 1.0));
        self.gain.set_target(db_to_gain(s.out_db));
        self.bypass.set_target(if s.bypass { 1.0 } else { 0.0 });
        let mut start = 0;
        while start < len {
            let n = (len - start).min(MAX_BLOCK).min(self.slice_len - self.slice_pos);
            let end = start + n;
            for c in 0..n_ch {
                self.wet[c][..n].copy_from_slice(&channels[c][start..end]);
                let p = channels[c][start..end].iter().fold(0.0f32, |m, v| m.max(v.abs()));
                self.peaks[c] = self.peaks[c].max(p);
            }
            if let Some([pre, _]) = spectra {
                let (l, r) = (&channels[0][start..end], &channels[n_ch - 1][start..end]);
                pre.push(l.iter().zip(r).map(|(a, b)| 0.5 * (a + b)));
            }
            self.dry.write(&channels[0][start..end], (n_ch == 2).then(|| &channels[1][start..end]));
            {
                let [a, b] = &mut self.wet;
                let mut wet: [&mut [f32]; 2] = [&mut a[..n], &mut b[..n]];
                self.machine.process(&mut wet[..n_ch], &s.machine);
            }
            for i in 0..n {
                let (mix, g, bp) = (self.mix.next_value(), self.gain.next_value(), self.bypass.next_value());
                // Auto gain meters the tape before its own gain (no loop) against the input in
                // time with it, and gives back to the tape's side of the mix only.
                let (d2, w2) = ([self.dry.read(0, i), self.dry.read(1, i)], [self.wet[0][i], self.wet[n_ch - 1][i]]);
                let makeup = self.auto.gain();
                self.auto.step(d2, w2, n_ch, s.auto_gain);
                for c in 0..n_ch {
                    let d = self.dry.read(c, i);
                    let w = self.wet[c][i] * makeup;
                    // Mix 0 and bypass are the dry signal bit for bit.
                    let y = if mix == 0.0 { d } else { d + (w - d) * mix } * g;
                    let out = if bp == 1.0 { d } else if bp == 0.0 { y } else { y + (d - y) * bp };
                    channels[c][start + i] = out;
                    self.peaks[2 + c] = self.peaks[2 + c].max(out.abs());
                }
            }
            self.dry.advance(n);
            if let Some([_, post]) = spectra {
                let (l, r) = (&channels[0][start..end], &channels[n_ch - 1][start..end]);
                post.push(l.iter().zip(r).map(|(a, b)| 0.5 * (a + b)));
            }
            if n_ch == 1 {
                self.peaks[1] = self.peaks[0];
                self.peaks[3] = self.peaks[2];
            }
            self.slice_pos += n;
            if self.slice_pos >= self.slice_len {
                if let Some([input, output]) = meters {
                    let db = |v: f32| if v > 0.0 { gain_to_db(v) } else { MINUS_INFINITY_DB };
                    input.store(db(self.peaks[0]), db(self.peaks[1]));
                    output.store(db(self.peaks[2]), db(self.peaks[3]));
                }
                self.peaks = [0.0; 4];
                self.slice_pos = 0;
            }
            start = end;
        }
    }
}

/// The analysis size of [`response`] and the frames it averages.
const RESPONSE_SIZE: usize = 4096;
const RESPONSE_FRAMES: usize = 24;
/// The pink noise [`response`] plays, dBFS RMS: a quiet mix, so it shows the deck's filters and
/// only a little of its saturation (at the record level set; driven harder, the curve sinks as the
/// tape compresses).
pub const RESPONSE_LEVEL_DBFS: f32 = -30.0;

/// The deck's response, dB at each `spectrum_point_freq` (as the spectrum draws): measured, not
/// modelled, by playing pink noise through a machine set as `s` with no wow, flutter or hiss and
/// comparing the spectra in and out. Allocates and takes tens of milliseconds: run it off the
/// audio and UI threads.
pub fn response(s: &MachineSettings, sample_rate: f32) -> Vec<f32> {
    response_at(s, sample_rate, RESPONSE_LEVEL_DBFS)
}

/// [`response`] with the noise at `level_dbfs` (RMS).
pub fn response_at(s: &MachineSettings, sample_rate: f32, level_dbfs: f32) -> Vec<f32> {
    let quiet = MachineSettings { on: true, wow: 0.0, flutter: 0.0, hiss: 0.0, ..*s };
    let mut m = Machine::new(sample_rate, FACTOR, &quiet);
    let latency = m.latency();
    let warm = RESPONSE_SIZE;
    let total = warm + latency + RESPONSE_SIZE * RESPONSE_FRAMES;
    // Pink noise (Kellet's filter, −3 dB/oct, about a mix's balance; white noise is mostly
    // treble, which the slow speeds' record emphasis drives far harder into the core than music),
    // scaled to `level_dbfs` RMS.
    let mut seed = 0x2545_F491u32;
    let mut b = [0.0f32; 7];
    let mut x: Vec<f32> = (0..total)
        .map(|_| {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            let w = seed as f32 / u32::MAX as f32 * 2.0 - 1.0;
            b[0] = 0.99886 * b[0] + w * 0.0555179;
            b[1] = 0.99332 * b[1] + w * 0.0750759;
            b[2] = 0.96900 * b[2] + w * 0.153_852;
            b[3] = 0.86650 * b[3] + w * 0.3104856;
            b[4] = 0.55000 * b[4] + w * 0.5329522;
            b[5] = -0.7616 * b[5] - w * 0.0168980;
            let p = b[..6].iter().sum::<f32>() + b[6] + w * 0.5362;
            b[6] = w * 0.115926;
            p
        })
        .collect();
    let rms = (x[warm..].iter().map(|v| v * v).sum::<f32>() / (total - warm) as f32).sqrt();
    let k = db_to_gain(level_dbfs) / rms.max(1e-9);
    x.iter_mut().for_each(|v| *v *= k);
    let mut y = x.clone();
    for block in y.chunks_mut(MAX_BLOCK) {
        m.process(&mut [block], &quiet);
    }
    let mut analyzer = Analyzer::new(RESPONSE_SIZE);
    let bins = analyzer.bins();
    let (mut px, mut py, mut frame) = (vec![0.0f32; bins], vec![0.0f32; bins], vec![0.0f32; bins]);
    for k in 0..RESPONSE_FRAMES {
        let at = warm + k * RESPONSE_SIZE;
        analyzer.analyze_power(&x[at..at + RESPONSE_SIZE], &mut frame);
        px.iter_mut().zip(&frame).for_each(|(a, b)| *a += b);
        analyzer.analyze_power(&y[at + latency..at + latency + RESPONSE_SIZE], &mut frame);
        py.iter_mut().zip(&frame).for_each(|(a, b)| *a += b);
    }
    let log = LogSpectrum::new(RESPONSE_SIZE, f64::from(sample_rate));
    let (mut dx, mut dy) = (vec![0.0f32; SPECTRUM_POINTS], vec![0.0f32; SPECTRUM_POINTS]);
    log.read(&px, &mut dx);
    log.read(&py, &mut dy);
    dy.iter().zip(&dx).map(|(b, a)| b - a).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use mite_dsp::analyzer::spectrum_point_freq;
    use mite_dsp::tape::Speed;

    fn sine(n: usize) -> Vec<f32> {
        (0..n).map(|i| (i as f32 * 0.031).sin() * 0.3).collect()
    }

    fn run(s: &TapeSettings, x: &[f32]) -> (Vec<f32>, Vec<f32>, usize) {
        let mut e = TapeEngine::new(48_000.0, s);
        let (mut l, mut r) = (x.to_vec(), x.iter().map(|v| -0.5 * v).collect::<Vec<f32>>());
        for (a, b) in l.chunks_mut(333).zip(r.chunks_mut(333)) {
            e.process(&mut [a, b], s, None, None);
        }
        (l, r, e.latency())
    }

    #[test]
    fn bypass_and_mix_zero_are_the_input_at_the_latency() {
        let x = sine(9600);
        for s in [TapeSettings { bypass: true, ..TapeSettings::default() }, TapeSettings { mix: 0.0, ..TapeSettings::default() }] {
            let (l, r, look) = run(&s, &x);
            assert!(look > 0);
            assert_eq!(&l[look..], &x[..x.len() - look], "{s:?}");
            let right: Vec<f32> = x.iter().map(|v| -0.5 * v).collect();
            assert_eq!(&r[look..], &right[..x.len() - look]);
        }
    }

    #[test]
    fn the_tape_changes_the_sound_and_mono_runs() {
        let x = sine(9600);
        let (l, _, look) = run(&TapeSettings { machine: MachineSettings { drive_db: 18.0, ..TapeSettings::default().machine }, ..TapeSettings::default() }, &x);
        let moved = l[look..].iter().zip(&x).fold(0.0f32, |m, (a, b)| m.max((a - b).abs()));
        assert!(moved > 0.01, "the deck left it alone: {moved}");
        let s = TapeSettings::default();
        let mut e = TapeEngine::new(48_000.0, &s);
        let mut m = x.clone();
        e.process(&mut [&mut m], &s, None, None);
        assert!(m.iter().all(|v| v.is_finite()));
    }

    /// K-weighted loudness of both channels, dB (BS.1770 without the gating).
    fn loudness(l: &[f32], r: &[f32]) -> f64 {
        let mut k = [mite_dsp::loudness::KWeighting::new(48_000.0); 2];
        let e: f64 = l.iter().zip(r).map(|(a, b)| k[0].run(*a).powi(2) + k[1].run(*b).powi(2)).sum();
        10.0 * (e / l.len() as f64).log10()
    }

    /// Music-like: a bass and a chord, a hit every half second.
    fn program(n: usize) -> Vec<f32> {
        (0..n)
            .map(|i| {
                let t = i as f32 / 48_000.0;
                let hit = (-((i % 24_000) as f32) / 2400.0).exp();
                (0.25 * (std::f32::consts::TAU * 55.0 * t).sin() + 0.08 * (std::f32::consts::TAU * 440.0 * t).sin() + 0.05 * (std::f32::consts::TAU * 3300.0 * t).sin()) * (0.5 + hit)
            })
            .collect()
    }

    #[test]
    fn auto_gain_brings_a_driven_tape_back_to_the_inputs_loudness() {
        let x = program(48_000 * 12);
        let settle = 48_000 * 8;
        for (level, bias) in [(18.0, 0.0), (24.0, -0.6), (-12.0, 0.6)] {
            let machine = MachineSettings { drive_db: level, bias, ..TapeSettings::default().machine };
            let mut off = 0.0;
            for auto_gain in [false, true] {
                let s = TapeSettings { machine, auto_gain, ..TapeSettings::default() };
                let (l, r, look) = run(&s, &x);
                let right: Vec<f32> = x.iter().map(|v| -0.5 * v).collect();
                let want = loudness(&x[settle - look..x.len() - look], &right[settle - look..x.len() - look]);
                let got = loudness(&l[settle..], &r[settle..]);
                println!("level {level:+} dB, bias {bias:+}: auto gain {}: {:+.2} dB against the input", if auto_gain { "on" } else { "off" }, got - want);
                if auto_gain {
                    assert!((got - want).abs() < 0.5, "level {level}: {got:.2} against the input's {want:.2} (off: {off:.2})");
                } else {
                    off = got - want;
                }
            }
        }
    }

    #[test]
    fn auto_gain_neither_pumps_nor_rides_the_hits() {
        // Once settled, the gain stays within 0.3 dB over two seconds of hits and gaps.
        let x = program(48_000 * 10);
        let s = TapeSettings { machine: MachineSettings { drive_db: 18.0, ..TapeSettings::default().machine }, ..TapeSettings::default() };
        let mut e = TapeEngine::new(48_000.0, &s);
        let (mut lo, mut hi) = (f32::MAX, f32::MIN);
        for (k, block) in x.chunks(480).enumerate() {
            let (mut l, mut r) = (block.to_vec(), block.to_vec());
            e.process(&mut [&mut l, &mut r], &s, None, None);
            if k * 480 > 48_000 * 8 {
                lo = lo.min(e.auto_gain_db());
                hi = hi.max(e.auto_gain_db());
            }
        }
        assert!(hi - lo < 0.3, "the gain moved {lo:.2}…{hi:.2} dB");
    }

    #[test]
    #[ignore]
    fn print_response_at_levels() {
        use mite_dsp::tape::Standard;
        for speed in [Speed::Ips3_75, Speed::Ips15] {
            for standard in [Standard::Nab, Standard::Ccir] {
                for lvl in [-48.0, -30.0, -24.0, -18.0] {
                    let r = response_at(&MachineSettings { speed, standard, ..TapeSettings::default().machine }, 48_000.0, lvl);
                    let row: Vec<String> = [20.0, 30.0, 50.0, 100.0, 300.0, 1000.0, 3000.0, 8000.0, 16000.0].iter().map(|&f| {
                        let i = (0..SPECTRUM_POINTS).min_by(|&a, &b| (spectrum_point_freq(a) / f).ln().abs().total_cmp(&(spectrum_point_freq(b) / f).ln().abs())).unwrap();
                        format!("{:+5.1}", r[i])
                    }).collect();
                    println!("{speed:?} {standard:?} {lvl}: {}", row.join(" "));
                }
            }
        }
    }

    #[test]
    fn the_measured_response_shows_the_head_bump_and_the_top_falling_off_slower_tape() {
        let at = |r: &[f32], f: f64| {
            let i = (0..SPECTRUM_POINTS).min_by(|&a, &b| (spectrum_point_freq(a) / f).ln().abs().total_cmp(&(spectrum_point_freq(b) / f).ln().abs())).unwrap();
            r[i]
        };
        let base = TapeSettings::default().machine;
        let slow = response(&MachineSettings { speed: Speed::Ips7_5, ..base }, 48_000.0);
        let fast = response(&MachineSettings { speed: Speed::Ips30, ..base }, 48_000.0);
        let mid = at(&slow, 1000.0);
        assert!(mid.abs() < 1.0, "flat in the middle: {mid}");
        let (bump, _) = mite_dsp::tape::head_bump(Speed::Ips7_5);
        assert!(at(&slow, bump) > mid + 1.0, "the head bump at {bump} Hz: {}", at(&slow, bump));
        assert!(at(&slow, 16_000.0) < at(&fast, 16_000.0) - 1.0, "slower tape loses more top: {} vs {}", at(&slow, 16_000.0), at(&fast, 16_000.0));
    }
}
