//! A pitch shifter for a reverb's shimmer: a phase-vocoder time stretch (identity phase locking,
//! Laroche & Dolson) followed by resampling. Each frame is analysed every `HOP` samples and laid
//! down again every `ratio × HOP`, its peaks' phases advancing at their own frequencies over the
//! longer hop (the sound stretched, its pitch kept); the stretched stream is then read `ratio`
//! times as fast (Hermite), which puts the pitch up by `ratio` at the input's length.
//!
//! History (2026-11-09): a two-grain time-domain shifter was "grainy" (user); then shifting the
//! bins up in place (k → ratio·k) inside each frame was heard as "metallic": doubling every
//! frequency of a frame squeezes its sound into half the frame, twice over — an echo at half a
//! frame (21 ms, correlation 0.11 on noise against 0.01), a comb. Stretch-and-resample never
//! repeats a frame's sound (tested).
//!
//! Sized in [`SpectralShifter::new`]; `process` never allocates.

use std::sync::Arc;

use realfft::num_complex::Complex;
use realfft::{ComplexToReal, RealFftPlanner, RealToComplex};

/// Analysis frames overlap eightfold.
const OVERLAP: usize = 8;
/// The interval's range, as a speed ratio (±24 semitones).
const RATIO: (f64, f64) = (0.25, 4.0);

/// The frame, about 64 ms (3072 at 48 kHz; a multiple of the overlap): long enough that a chord's
/// partials sit in bins of their own (43 ms let them share and left junk 21 dB down), short
/// enough to stay crisp.
pub fn frame_size(fs: f32) -> usize {
    let n = (0.064 * fs / OVERLAP as f32).round() as usize * OVERLAP;
    n.max(64 * OVERLAP)
}

/// One side's stretcher and resampler.
struct Side {
    /// The last `n` input samples (oldest first) and how many came since the last frame.
    input: Vec<f64>,
    since: usize,
    /// Each bin's last analysis phase and last synthesis phase.
    last: Vec<f64>,
    synth: Vec<f64>,
    /// The overlap-add of the stretched frames and of their windows (to divide by).
    acc: Vec<f64>,
    wsum: Vec<f64>,
    /// The stretched stream (a ring), where it has been written to, where it is read, and the
    /// speed it is read at (the last frame's synthesis hop over the analysis hop).
    fifo: Vec<f64>,
    written: f64,
    read: f64,
    speed: f64,
    /// The synthesis hop's fraction carried to the next frame (the hops average the exact ratio).
    carry: f64,
}

pub struct SpectralShifter {
    n: usize,
    hop: usize,
    r2c: Arc<dyn RealToComplex<f64>>,
    c2r: Arc<dyn ComplexToReal<f64>>,
    fwd_scratch: Vec<Complex<f64>>,
    inv_scratch: Vec<Complex<f64>>,
    /// Square-root Hann (analysis and synthesis).
    window: Vec<f64>,
    frame: Vec<f64>,
    spec: Vec<Complex<f64>>,
    mag: Vec<f64>,
    phase: Vec<f64>,
    sides: [Side; 2],
}

impl SpectralShifter {
    /// Allocates.
    pub fn new(fs: f32) -> Self {
        let n = frame_size(fs);
        let hop = n / OVERLAP;
        let mut planner = RealFftPlanner::<f64>::new();
        let r2c = planner.plan_fft_forward(n);
        let c2r = planner.plan_fft_inverse(n);
        let bins = n / 2 + 1;
        let window = (0..n).map(|i| (0.5 - 0.5 * (std::f64::consts::TAU * i as f64 / n as f64).cos()).sqrt()).collect();
        // The longest synthesis hop is RATIO.1 × hop; the ring holds several frames of it.
        let acc_len = n + (RATIO.1 as usize + 1) * hop;
        let side = || Side {
            input: vec![0.0; n],
            since: 0,
            last: vec![0.0; bins],
            synth: vec![0.0; bins],
            acc: vec![0.0; acc_len],
            wsum: vec![0.0; acc_len],
            fifo: vec![0.0; 8 * n],
            written: 0.0,
            read: 0.0,
            speed: 1.0,
            carry: 0.0,
        };
        let mut s = Self {
            n,
            hop,
            fwd_scratch: r2c.make_scratch_vec(),
            inv_scratch: c2r.make_scratch_vec(),
            spec: r2c.make_output_vec(),
            r2c,
            c2r,
            window,
            frame: vec![0.0; n],
            mag: vec![0.0; bins],
            phase: vec![0.0; bins],
            sides: [side(), side()],
        };
        s.reset();
        s
    }

    /// The delay through it, samples (about: the frame, plus the reader's lag).
    pub fn latency(&self) -> usize {
        self.n + self.n / 2
    }

    pub fn reset(&mut self) {
        for side in &mut self.sides {
            side.input.fill(0.0);
            side.since = 0;
            side.last.fill(0.0);
            side.synth.fill(0.0);
            side.acc.fill(0.0);
            side.wsum.fill(0.0);
            side.fifo.fill(0.0);
            side.written = 0.0;
            // The reader starts half a frame behind what the first frames write.
            side.read = -(self.n as f64) / 2.0;
            side.speed = 1.0;
            side.carry = 0.0;
        }
    }

    /// Shifts `left` and `right` in place by `ratio` (2: an octave up; one per side), delayed by
    /// about [`Self::latency`].
    pub fn process(&mut self, left: &mut [f32], right: &mut [f32], ratio: [f32; 2]) {
        for (c, buf) in [left, right].into_iter().enumerate() {
            let r = f64::from(ratio[c]).clamp(RATIO.0, RATIO.1);
            for x in buf.iter_mut() {
                let side = &mut self.sides[c];
                side.input.copy_within(1.., 0);
                let last = self.n - 1;
                side.input[last] = if x.is_finite() { f64::from(*x) } else { 0.0 };
                side.since += 1;
                if side.since == self.hop {
                    side.since = 0;
                    self.frame(c, r);
                }
                *x = self.sides[c].next() as f32;
            }
        }
    }

    /// One analysis frame of side `c`, laid down stretched by `ratio`.
    fn frame(&mut self, c: usize, ratio: f64) {
        use std::f64::consts::TAU;
        let n = self.n;
        let side = &mut self.sides[c];
        // The synthesis hop, whole samples, its fraction carried on: on average exactly `ratio`
        // times the analysis hop, as fast as the reader reads, so the stream neither starves nor
        // floods and the pitch is exact.
        let want = ratio * self.hop as f64 + side.carry;
        let out_hop = (want.floor() as usize).clamp(1, side.acc.len() - n);
        side.carry = want - out_hop as f64;
        side.speed = ratio;
        for ((f, x), w) in self.frame.iter_mut().zip(&side.input).zip(&self.window) {
            *f = x * w;
        }
        let _ = self.r2c.process_with_scratch(&mut self.frame, &mut self.spec, &mut self.fwd_scratch);
        let bins = self.spec.len();
        for k in 0..bins {
            let (m, ph) = self.spec[k].to_polar();
            self.mag[k] = m;
            self.phase[k] = ph;
        }
        // Each peak: its true frequency from its phase advance over the analysis hop, its
        // synthesis phase advanced at that frequency over the synthesis hop from where it was;
        // the bins around it (to the valleys) keep their phases relative to it.
        let ana = TAU * self.hop as f64 / n as f64;
        let mut start = 0;
        let mut k = 1;
        while start < bins {
            while k < bins && !(self.mag[k] > self.mag[k - 1] && (k + 1 >= bins || self.mag[k] >= self.mag[k + 1])) {
                k += 1;
            }
            let peak = k.min(bins - 1);
            let mut end = peak + 1;
            while end < bins && self.mag[end] < self.mag[end - 1] {
                end += 1;
            }
            let mut d = self.phase[peak] - side.last[peak] - peak as f64 * ana;
            d -= TAU * (d / TAU).round();
            let omega = (peak as f64 * ana + d) / self.hop as f64; // rad per sample
            let syn = side.synth[peak] + omega * out_hop as f64;
            for b in start..end.min(bins) {
                let ph = syn + self.phase[b] - self.phase[peak];
                side.synth[b] = ph % TAU;
                self.spec[b] = Complex::from_polar(self.mag[b], ph);
            }
            start = end;
            k = end.max(k + 1);
        }
        side.last.copy_from_slice(&self.phase);
        let last = bins - 1;
        self.spec[0].im = 0.0;
        self.spec[last].im = 0.0;
        let _ = self.c2r.process_with_scratch(&mut self.spec, &mut self.frame, &mut self.inv_scratch);
        let scale = 1.0 / n as f64;
        for i in 0..n {
            let w = self.window[i];
            side.acc[i] += self.frame[i] * w * scale;
            side.wsum[i] += w * w;
        }
        // The first `out_hop` samples are complete: into the stream, divided by the windows' sum.
        let len = side.fifo.len();
        for i in 0..out_hop {
            let y = side.acc[i] / side.wsum[i].max(0.5);
            let at = side.written as usize % len;
            side.fifo[at] = y;
            side.written += 1.0;
        }
        side.acc.copy_within(out_hop.., 0);
        side.wsum.copy_within(out_hop.., 0);
        let tail = side.acc.len() - out_hop;
        side.acc[tail..].fill(0.0);
        side.wsum[tail..].fill(0.0);
    }
}

impl Side {
    /// The next output sample: the stream read at `speed`, Hermite between samples.
    fn next(&mut self) -> f64 {
        let len = self.fifo.len() as f64;
        // Never read what is not written yet (or what has been overwritten).
        self.read = self.read.clamp(self.written - len + 4.0, self.written - 3.0);
        let y = if self.read < 1.0 {
            0.0
        } else {
            let i = self.read.floor();
            let t = self.read - i;
            let at = |k: f64| self.fifo[((i + k).rem_euclid(len)) as usize];
            let (xm1, x0, x1, x2) = (at(-1.0), at(0.0), at(1.0), at(2.0));
            let c1 = 0.5 * (x1 - xm1);
            let c2 = xm1 - 2.5 * x0 + 2.0 * x1 - 0.5 * x2;
            let c3 = 0.5 * (x2 - xm1) + 1.5 * (x0 - x1);
            ((c3 * t + c2) * t + c1) * t + x0
        };
        self.read += self.speed;
        y
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn level(v: &[f32], f: f32, fs: f32) -> f32 {
        let (mut c, mut s) = (0.0f64, 0.0f64);
        for (i, &x) in v.iter().enumerate() {
            let w = std::f64::consts::TAU * f64::from(f) * i as f64 / f64::from(fs);
            c += f64::from(x) * w.cos();
            s += f64::from(x) * w.sin();
        }
        (2.0 * (c * c + s * s).sqrt() / v.len() as f64) as f32
    }

    #[test]
    fn it_moves_a_tone_by_the_interval_smoothly() {
        let fs = 48_000.0;
        for (semis, want) in [(12.0f32, 880.0f32), (-12.0, 220.0), (7.0, 659.255)] {
            let mut sh = SpectralShifter::new(fs);
            let r = (semis / 12.0).exp2();
            let mut l: Vec<f32> = (0..96_000).map(|i| 0.5 * (std::f32::consts::TAU * 440.0 * i as f32 / fs).sin()).collect();
            let mut rr = l.clone();
            for (a, b) in l.chunks_mut(64).zip(rr.chunks_mut(64)) {
                sh.process(a, b, [r; 2]);
            }
            let tail = &l[24_000..];
            let (hit, orig) = (level(tail, want, fs), level(tail, 440.0, fs));
            assert!(hit > 0.35 && orig < 0.02, "{semis}: {hit} at the interval, {orig} left");
            // Smooth: the envelope (10 ms RMS) varies under ±1 dB on a steady tone (two grains
            // dip several dB at every crossfade).
            let env: Vec<f32> = tail.chunks(480).map(|c| (c.iter().map(|v| v * v).sum::<f32>() / c.len() as f32).sqrt()).collect();
            let (lo, hi) = env.iter().fold((f32::MAX, 0.0f32), |(a, b), &e| (a.min(e), b.max(e)));
            assert!(20.0 * (hi / lo).log10() < 2.0, "{semis}: envelope {lo}…{hi}");
        }
    }
}

#[cfg(test)]
mod purity {
    use super::*;
    use realfft::RealFftPlanner;

    /// The share of the output's energy within ±`width` Hz of the wanted partials.
    fn purity(v: &[f32], fs: f32, want: &[f64], width: f64) -> f64 {
        let n = v.len();
        let mut p = RealFftPlanner::<f64>::new();
        let fft = p.plan_fft_forward(n);
        let mut x: Vec<f64> = v.iter().enumerate().map(|(i, &s)| f64::from(s) * (0.5 - 0.5 * (std::f64::consts::TAU * i as f64 / n as f64).cos())).collect();
        let mut out = fft.make_output_vec();
        fft.process(&mut x, &mut out).unwrap();
        let hz = |k: usize| k as f64 * f64::from(fs) / n as f64;
        let tot: f64 = out.iter().map(|c| c.norm_sqr()).sum();
        let hit: f64 = out.iter().enumerate().filter(|(k, _)| want.iter().any(|w| (hz(*k) - w).abs() < width)).map(|(_, c)| c.norm_sqr()).sum();
        hit / tot
    }

    /// An octave up of a tone, a chord and a drifting tone (as a modulated tail is) comes out
    /// clean (little energy off the moved partials) and steady (the level does not beat).
    #[test]
    fn the_shift_is_clean_and_steady_even_on_a_drifting_tone() {
        let fs = 48_000.0f32;
        let cases: [(&str, Vec<f64>); 3] = [("tone", vec![440.0]), ("chord", vec![261.6, 329.6, 392.0, 523.2]), ("glide", vec![])];
        for (name, freqs) in cases {
            let mut sh = SpectralShifter::new(fs);
            let n = 4 * 48_000;
            let mut ph = 0.0f64;
            let mut l: Vec<f32> = (0..n).map(|i| {
                if freqs.is_empty() {
                    let f = 440.0 + 30.0 * i as f64 / n as f64;
                    ph += std::f64::consts::TAU * f / f64::from(fs);
                    (0.5 * ph.sin()) as f32
                } else {
                    let t = i as f64 / f64::from(fs);
                    freqs.iter().map(|f| 0.2 * (std::f64::consts::TAU * f * t).sin()).sum::<f64>() as f32
                }
            }).collect();
            let mut r = l.clone();
            for (a, b) in l.chunks_mut(1).zip(r.chunks_mut(1)) { sh.process(a, b, [2.0; 2]); }
            let tail = &l[48_000..];
            let want: Vec<f64> = if freqs.is_empty() { vec![] } else { freqs.iter().map(|f| f * 2.0).collect() };
            let p = if want.is_empty() {
                // The glide: 2·(447.5…470) over the tail; count ±40 Hz around the middle.
                purity(tail, fs, &[2.0 * 458.75], 40.0)
            } else { purity(tail, fs, &want, 6.0) };
            let env: Vec<f32> = tail.chunks(480).map(|c| (c.iter().map(|v| v * v).sum::<f32>() / c.len() as f32).sqrt()).collect();
            let (lo, hi) = env.iter().fold((f32::MAX, 0.0f32), |(a, b), &e| (a.min(e), b.max(e)));
            let (junk, swing) = (10.0 * (1.0 - p).max(1e-12).log10(), 10.0 * (hi / lo).log10());
            // Measured 2026-11-09 (stretch and resample, 64 ms): junk −85/−31/−84 dB, swing
            // ±0.07/±1.04/±0.07 dB.
            assert!(junk < -28.0 && swing < 1.5, "{name}: junk {junk:.1} dB, envelope ±{swing:.2} dB");
        }
    }
}

#[cfg(test)]
mod comb {
    use super::*;

    /// Noise shifted an octave up stays noise: no echo of itself at any lag up to 60 ms (shifting
    /// bins in place left one at half a frame, correlation 0.11, heard as metallic).
    #[test]
    fn noise_comes_out_without_an_echo_of_itself() {
        let fs = 48_000.0f32;
        let mut seed = 3u32;
        let mut l: Vec<f32> = (0..4 * 48_000).map(|_| { seed ^= seed << 13; seed ^= seed >> 17; seed ^= seed << 5; (seed as f32 / u32::MAX as f32 - 0.5) * 0.5 }).collect();
        let mut r = l.clone();
        let mut sh = SpectralShifter::new(fs);
        for (a, b) in l.chunks_mut(64).zip(r.chunks_mut(64)) {
            sh.process(a, b, [2.0; 2]);
        }
        let lags = crate::reverb_quality::periodic_lags(&l[48_000..], f64::from(fs), 0.0, 2.5, 1);
        // Measured 2026-11-09: 0.037 at 32 ms (noise itself: 0.01).
        assert!(lags[0].1.abs() < 0.06, "{lags:?}");
    }
}
