//! The spectral engine: per-frequency gain inside bands' ranges, on an [`Stft`]'s spectra. Shared
//! by the EQ's spectral compressor and, later, Mite Soothe and Mite DeEss (BLUEPRINT §6).
//!
//! **Detection reads the spectrum exactly as the analyzer shows it:** an [`Analyzer`] FFT of
//! [`ANALYZER_SIZE`] over the stage's input, read at the [`SPECTRUM_POINTS`] by the same
//! [`LogSpectrum`] (1/6-octave smoothing, 4.5 dB/oct tilt, dB rounding). Levels and thresholds are
//! therefore on the analyzer's scale, and what the band hears is what the plot draws.
//!
//! **Spectral compressor** (a dynamic band in compressor mode with spectral on): every point
//! has its own level envelope and runs the band's compressor curve, so only the frequencies that
//! get loud move. Its threshold is either a fixed level or **relative**: a number of dB above
//! the point's surroundings (the envelopes averaged in dB over 0.6 octave around it), which
//! catches what sticks out (a resonance, a ringing note) however loud the whole signal is,
//! soothe-style. Smoothing spreads each point's gain to its neighbours.
//!
//! The per-point gains, weighted by the band's region, are read onto the STFT's bins (averaged
//! where a bin spans several points, interpolated where points are sparser) and combine per bin
//! in the L/R basis (2×2 matrices for Mid and Side), so stereo placements work as everywhere
//! else. Allocation-free after [`SpectralBands::new`].

use realfft::num_complex::Complex;

use crate::analyzer::{spectrum_point_freq, spectrum_point_oct, Analyzer, LogSpectrum, ANALYZER_SIZE, SPECTRUM_POINTS};
use crate::eq::{BandSettings, BandType, Dynamics, Placement, CURVE_SMOOTH_MS, FREQ_MAX, FREQ_MIN, RELATIVE_WIDTH_OCT, SMOOTHING_MAX_OCT};
use crate::filter::Analog;
use crate::stft::Stft;

/// Lowest level considered, dB (silence would otherwise be −∞).
const FLOOR_DB: f64 = -140.0;
/// Region weights below this (40 dB down) are outside the band; the rest are rescaled so the
/// weight still falls continuously to 0 at that edge.
const WEIGHT_MIN: f64 = 1e-2;
/// Gains below this, dB, have settled to nothing.
const GAIN_EPSILON_DB: f64 = 1e-6;
/// Detection runs every this many STFT frames.
const DETECT_EVERY_FRAMES: usize = 2;

/// The detected level each placement listens to, in `SpectralBands::levels` order.
fn level_index(p: Placement) -> usize {
    match p {
        Placement::Stereo => 0,
        Placement::Left => 1,
        Placement::Right => 2,
        Placement::Mid => 3,
        Placement::Side => 4,
    }
}

/// Per band, per spectrum point: the level envelope, the threshold it is compared with, the
/// gain applied (region-weighted), and the region weights; the points the band works on and the
/// STFT bins they reach; and its curve settings, eased.
struct BandState {
    level: Vec<f64>,
    threshold: Vec<f64>,
    gain: Vec<f64>,
    weight: Vec<f64>,
    weight_key: Option<(BandType, f64, f64)>,
    range: (usize, usize),
    bins: (usize, usize),
    /// The curve the band runs, eased towards its settings (threshold, relative threshold,
    /// ratio, range), so moving them glides instead of stepping; `None` until it first runs.
    curve: Option<[f64; 4]>,
}

/// How an STFT bin reads the per-point gains: between two points, or the mean of several.
#[derive(Clone, Copy)]
enum BinRead {
    Lerp(usize, f64),
    Mean(usize, usize),
}

/// Per-frequency processing for up to `bands` bands, on an STFT of `size` at `sample_rate`.
pub struct SpectralBands {
    sample_rate: f64,
    /// Seconds between frames, and between detections.
    frame_s: f64,
    detect_s: f64,
    frames: usize,
    /// The detector: the last `ANALYZER_SIZE` input samples per channel (a ring, newest at
    /// `write - 1`), a linear copy to analyse, power spectra, and the map onto the points.
    history: [Vec<f32>; 2],
    write: usize,
    signal: Vec<f32>,
    power: [Vec<f32>; 2],
    analyzer: Analyzer,
    spectrum: LogSpectrum,
    /// Per point and placement: the detected level (analyzer scale, dB), refreshed every
    /// detection for the placements in use.
    levels: [Vec<f32>; 5],
    states: Vec<BandState>,
    target: Vec<f64>,
    /// Running sums over the points (of envelopes for relative thresholds, then of targets for
    /// smoothing).
    sum: Vec<f64>,
    /// Per STFT bin: its frequency and how it reads the points.
    freqs: Vec<f64>,
    reads: Vec<BinRead>,
    /// Per bin: the Stereo/Left/Right bands' summed gains per channel (dB), and the Mid/Side
    /// bands' combined 2×2 gain matrix (`[ll, lr, rl, rr]`, L/R basis).
    diag_db: [Vec<f64>; 2],
    matrix: Vec<[f64; 4]>,
    /// For displays: each band's strongest gain this frame (dB, signed), and the sum of every
    /// band's gain per bin (dB, each in its own placement).
    band_peak: Vec<f64>,
    total_db: Vec<f32>,
}

/// Fractional point index of `f`.
fn point_pos(f: f64) -> f64 {
    (f / FREQ_MIN).max(1e-9).ln() / (FREQ_MAX / FREQ_MIN).ln() * (SPECTRUM_POINTS - 1) as f64
}

impl SpectralBands {
    /// Allocates for `bands` bands, `size`-sample STFT frames every `size / 4` samples.
    pub fn new(bands: usize, size: usize, sample_rate: f64) -> Self {
        let bins = size / 2 + 1;
        let points = || vec![0.0; SPECTRUM_POINTS];
        let last = (SPECTRUM_POINTS - 1) as f64;
        let bin_hz = sample_rate / size as f64;
        let freqs: Vec<f64> = (0..bins).map(|k| k as f64 * bin_hz).collect();
        let reads = freqs
            .iter()
            .map(|&f| {
                let (a, b) = (point_pos(f - 0.5 * bin_hz).max(0.0).ceil(), point_pos(f + 0.5 * bin_hz).min(last).floor());
                if f > 0.0 && b >= a + 1.0 {
                    BinRead::Mean(a as usize, b as usize)
                } else {
                    let pos = point_pos(f).clamp(0.0, last);
                    let p = (pos.floor() as usize).min(SPECTRUM_POINTS - 2);
                    BinRead::Lerp(p, pos - p as f64)
                }
            })
            .collect();
        let frame_s = (size / 4) as f64 / sample_rate;
        Self {
            sample_rate,
            frame_s,
            detect_s: DETECT_EVERY_FRAMES as f64 * frame_s,
            frames: 0,
            history: [vec![0.0; ANALYZER_SIZE], vec![0.0; ANALYZER_SIZE]],
            write: 0,
            signal: vec![0.0; ANALYZER_SIZE],
            power: [vec![0.0; ANALYZER_SIZE / 2 + 1], vec![0.0; ANALYZER_SIZE / 2 + 1]],
            analyzer: Analyzer::new(ANALYZER_SIZE),
            spectrum: LogSpectrum::new(ANALYZER_SIZE, sample_rate),
            levels: std::array::from_fn(|_| vec![FLOOR_DB as f32; SPECTRUM_POINTS]),
            states: (0..bands)
                .map(|_| BandState {
                    level: vec![FLOOR_DB; SPECTRUM_POINTS],
                    threshold: vec![FLOOR_DB; SPECTRUM_POINTS],
                    gain: points(),
                    weight: points(),
                    weight_key: None,
                    range: (1, 0),
                    bins: (1, 0),
                    curve: None,
                })
                .collect(),
            target: points(),
            sum: vec![0.0; SPECTRUM_POINTS + 1],
            freqs,
            reads,
            diag_db: [vec![0.0; bins], vec![0.0; bins]],
            matrix: vec![[1.0, 0.0, 0.0, 1.0]; bins],
            band_peak: vec![0.0; bands],
            total_db: vec![0.0; bins],
        }
    }

    /// Clears the detector, envelopes and gains. Allocation-free.
    pub fn reset(&mut self) {
        for h in &mut self.history {
            h.fill(0.0);
        }
        for l in &mut self.levels {
            l.fill(FLOOR_DB as f32);
        }
        for s in &mut self.states {
            s.level.fill(FLOOR_DB);
            s.gain.fill(0.0);
            s.curve = None;
        }
        self.frames = 0;
        self.band_peak.fill(0.0);
        self.total_db.fill(0.0);
    }

    /// Runs a stereo buffer through `stft` for `bands`: the detector hears each hop just before
    /// the STFT takes it in. Allocation-free.
    pub fn process(&mut self, stft: &mut Stft, left: &mut [f32], right: &mut [f32], bands: &[BandSettings]) {
        let hop = stft.hop();
        for (l, r) in left.chunks_mut(hop).zip(right.chunks_mut(hop)) {
            self.listen(l, r);
            stft.process(l, r, &mut |x, y| self.process_frame(x, y, bands));
        }
    }

    /// Adds input samples to the detector's history.
    fn listen(&mut self, left: &[f32], right: &[f32]) {
        for (&l, &r) in left.iter().zip(right) {
            self.history[0][self.write] = l;
            self.history[1][self.write] = r;
            self.write = (self.write + 1) % ANALYZER_SIZE;
        }
    }

    /// Band `b`'s per-point level envelope and the threshold it is compared with (both on the
    /// analyzer's scale, dB) and the gain it applies per point (dB), for displays. Points are
    /// [`spectrum_point_freq`].
    pub fn band_points(&self, b: usize) -> (&[f64], &[f64], &[f64]) {
        let s = &self.states[b];
        (&s.level, &s.threshold, &s.gain)
    }

    /// The gain band `b` applies at STFT bin `k`, dB.
    pub fn band_bin_gain(&self, b: usize, k: usize) -> f64 {
        read(&self.states[b].gain, self.reads[k])
    }

    /// The frequency of each STFT bin, Hz.
    pub fn freqs(&self) -> &[f64] {
        &self.freqs
    }

    /// Each band's strongest gain in the last frame, dB (signed; 0 for bands not using it).
    pub fn band_peaks(&self) -> &[f64] {
        &self.band_peak
    }

    /// The summed gain per bin in the last frame, dB, and each bin's frequency.
    pub fn total_db(&self) -> (&[f32], &[f64]) {
        (&self.total_db, &self.freqs)
    }

    /// Processes one frame's spectra in place for `bands` (the settings, indexed like the state).
    /// Allocation-free. Runs through [`Self::process`], which feeds the detector.
    ///
    /// Stereo, Left and Right bands are diagonal in L/R and commute, so they add up in dB; Mid
    /// and Side bands combine as matrices, applied after them.
    fn process_frame(&mut self, left: &mut [Complex<f64>], right: &mut [Complex<f64>], bands: &[BandSettings]) {
        let bands = &bands[..bands.len().min(self.states.len())];
        let detect = self.frames % DETECT_EVERY_FRAMES == 0;
        self.frames = self.frames.wrapping_add(1);
        if detect {
            let mut needed = [false; 5];
            for s in bands.iter().filter(|s| s.uses_spectrum()) {
                needed[level_index(s.placement)] = true;
            }
            self.measure(needed);
        }
        for d in &mut self.diag_db {
            d.fill(0.0);
        }
        self.total_db.fill(0.0);
        let mut matrices = false;
        for (b, s) in bands.iter().enumerate() {
            if !s.uses_spectrum() {
                self.band_peak[b] = 0.0;
                let state = &mut self.states[b];
                if state.curve.is_some() {
                    state.gain.fill(0.0);
                    state.level.fill(FLOOR_DB);
                    state.threshold.fill(FLOOR_DB);
                    state.curve = None;
                }
                continue;
            }
            self.update_weights(b, s);
            let d = self.eased_curve(b, &s.dynamics);
            self.compressor_targets(b, level_index(s.placement), &d, detect);
            let state = &mut self.states[b];
            let mut peak = 0.0f64;
            for (g, (&t, &w)) in state.gain.iter_mut().zip(self.target.iter().zip(&state.weight)) {
                *g = t * w;
                if g.abs() < GAIN_EPSILON_DB {
                    *g = 0.0;
                } else if g.abs() > peak.abs() {
                    peak = *g;
                }
            }
            self.band_peak[b] = peak;
            if peak == 0.0 {
                continue;
            }
            let (lo, hi) = state.bins;
            for k in lo..=hi {
                let g = read(&state.gain, self.reads[k]);
                if g.abs() < GAIN_EPSILON_DB {
                    continue;
                }
                self.total_db[k] += g as f32;
                match s.placement {
                    Placement::Stereo => {
                        self.diag_db[0][k] += g;
                        self.diag_db[1][k] += g;
                    }
                    Placement::Left => self.diag_db[0][k] += g,
                    Placement::Right => self.diag_db[1][k] += g,
                    Placement::Mid | Placement::Side => {
                        if !matrices {
                            matrices = true;
                            self.matrix.fill([1.0, 0.0, 0.0, 1.0]);
                        }
                        let m = band_matrix(s.placement, db_to_lin(g));
                        let acc = self.matrix[k];
                        self.matrix[k] = [
                            m[0] * acc[0] + m[1] * acc[2],
                            m[0] * acc[1] + m[1] * acc[3],
                            m[2] * acc[0] + m[3] * acc[2],
                            m[2] * acc[1] + m[3] * acc[3],
                        ];
                    }
                }
            }
        }
        for (k, (l, r)) in left.iter_mut().zip(right.iter_mut()).enumerate() {
            let (dl, dr) = (self.diag_db[0][k], self.diag_db[1][k]);
            let (gl, gr) = (if dl == 0.0 { 1.0 } else { db_to_lin(dl) }, if dr == 0.0 { 1.0 } else { db_to_lin(dr) });
            if matrices {
                let m = self.matrix[k];
                let (a, c) = (*l * gl, *r * gr);
                *l = a * m[0] + c * m[1];
                *r = a * m[2] + c * m[3];
            } else {
                *l *= gl;
                *r *= gr;
            }
        }
    }

    /// Detected levels per point (the analyzer's reading) for the placements in use: Stereo the
    /// louder of the two channels per bin, Mid and Side their own signals.
    fn measure(&mut self, needed: [bool; 5]) {
        let n = ANALYZER_SIZE;
        let (newer, older) = (n - self.write, self.write);
        let analyse = |signal: &mut Vec<f32>, history: &[Vec<f32>; 2], f: &dyn Fn(f32, f32) -> f32, out: &mut Vec<f32>, analyzer: &mut Analyzer| {
            // Oldest first: the ring from `write` on, then up to it.
            for (i, x) in signal.iter_mut().enumerate() {
                let k = if i < newer { older + i } else { i - newer };
                *x = f(history[0][k], history[1][k]);
            }
            analyzer.analyze_power(signal, out);
        };
        let [pl, pr] = &mut self.power;
        let channels = needed[0] || needed[1] || needed[2];
        if channels {
            analyse(&mut self.signal, &self.history, &|l, _| l, pl, &mut self.analyzer);
            if needed[1] {
                self.spectrum.read(pl, &mut self.levels[1]);
            }
            analyse(&mut self.signal, &self.history, &|_, r| r, pr, &mut self.analyzer);
            if needed[2] {
                self.spectrum.read(pr, &mut self.levels[2]);
            }
            if needed[0] {
                for (a, &b) in pl.iter_mut().zip(pr.iter()) {
                    *a = a.max(b);
                }
                self.spectrum.read(pl, &mut self.levels[0]);
            }
        }
        if needed[3] {
            analyse(&mut self.signal, &self.history, &|l, r| 0.5 * (l + r), pl, &mut self.analyzer);
            self.spectrum.read(pl, &mut self.levels[3]);
        }
        if needed[4] {
            analyse(&mut self.signal, &self.history, &|l, r| 0.5 * (l - r), pl, &mut self.analyzer);
            self.spectrum.read(pl, &mut self.levels[4]);
        }
    }

    /// Recomputes a band's region weights, its points and the bins they reach when its type,
    /// frequency or Q moved.
    fn update_weights(&mut self, b: usize, s: &BandSettings) {
        let key = (s.kind, s.freq, s.q);
        let state = &mut self.states[b];
        if state.weight_key == Some(key) {
            return;
        }
        state.weight_key = Some(key);
        let mut range = (usize::MAX, 0);
        for (i, w) in state.weight.iter_mut().enumerate() {
            *w = weight(s, spectrum_point_freq(i));
            if *w > 0.0 {
                range = (range.0.min(i), range.1.max(i));
            }
        }
        state.range = if range.0 <= range.1 { range } else { (1, 0) };
        let mut bins = (usize::MAX, 0);
        for (k, r) in self.reads.iter().enumerate() {
            let (a, z) = match *r {
                BinRead::Lerp(p, _) => (p, p + 1),
                BinRead::Mean(a, z) => (a, z),
            };
            if z >= range.0 && a <= range.1 {
                bins = (bins.0.min(k), bins.1.max(k));
            }
        }
        state.bins = if bins.0 <= bins.1 { bins } else { (1, 0) };
    }

    /// Band `b`'s dynamics with the curve eased one frame towards `d`'s settings.
    fn eased_curve(&mut self, b: usize, d: &Dynamics) -> Dynamics {
        let goal = [d.threshold_db, d.relative_db, d.ratio, d.range_db];
        let k = 1.0 - (-self.frame_s * 1000.0 / CURVE_SMOOTH_MS).exp();
        let curve = self.states[b].curve.get_or_insert(goal);
        for (c, g) in curve.iter_mut().zip(goal) {
            *c += (g - *c) * k;
            if (g - *c).abs() < 1e-3 {
                *c = g;
            }
        }
        let [threshold_db, relative_db, ratio, range_db] = *curve;
        Dynamics { threshold_db, relative_db, ratio, range_db, ..*d }
    }

    /// Spectral compressor: each point's level envelope (advanced on detection frames) through
    /// the band's compressor curve, its threshold fixed or relative to the point's surroundings,
    /// into `target` (before the region weights).
    fn compressor_targets(&mut self, b: usize, levels: usize, d: &Dynamics, detect: bool) {
        let state = &mut self.states[b];
        if detect {
            let coeff = |ms: f64| 1.0 - (-self.detect_s * 1000.0 / ms.max(0.1)).exp();
            let (attack, release) = (coeff(d.attack_ms), coeff(d.release_ms));
            for (env, &lvl) in state.level.iter_mut().zip(&self.levels[levels]) {
                let lvl = f64::from(lvl).max(FLOOR_DB);
                *env += if lvl > *env { attack } else { release } * (lvl - *env);
            }
        }
        if d.relative {
            // Each point against its surroundings: the envelopes (not the raw levels, so both
            // carry the same attack/release bias) averaged in dB over 0.6 octave around it.
            // The curve then has its threshold at `relative_db` over them.
            prefix(&state.level, &mut self.sum);
            let relative = Dynamics { threshold_db: d.relative_db, ..*d };
            let half = 0.5 * RELATIVE_WIDTH_OCT / spectrum_point_oct();
            for i in 0..SPECTRUM_POINTS {
                let around = window_mean(&self.sum, i, half);
                state.threshold[i] = around + d.relative_db;
                self.target[i] = relative.gain_db(state.level[i] - around);
            }
        } else {
            for i in 0..SPECTRUM_POINTS {
                state.threshold[i] = d.threshold_db;
                self.target[i] = d.gain_db(state.level[i]);
            }
        }
        // Smoothing: each point's gain becomes the average over a window of up to half an
        // octave around it, so neighbours move together.
        let half = 0.5 * SMOOTHING_MAX_OCT * d.smoothing.clamp(0.0, 1.0) / spectrum_point_oct();
        if half > 1e-6 {
            prefix(&self.target, &mut self.sum);
            for i in 0..SPECTRUM_POINTS {
                self.target[i] = window_mean(&self.sum, i, half);
            }
        }
    }

    pub fn sample_rate(&self) -> f64 {
        self.sample_rate
    }
}

/// `sum[i]` = the sum of `v[..i]`.
fn prefix(v: &[f64], sum: &mut [f64]) {
    sum[0] = 0.0;
    for (i, &x) in v.iter().enumerate() {
        sum[i + 1] = sum[i] + x;
    }
}

/// The mean of the values (given as prefix sums) within `half` points of `i`, the window's two
/// edge points weighted by how far it reaches into them (so a widening window grows smoothly),
/// clipped to the ends.
fn window_mean(sum: &[f64], i: usize, half: f64) -> f64 {
    let n = sum.len() - 1;
    let whole = half.floor() as usize;
    let frac = half - whole as f64;
    let (a, z) = (i.saturating_sub(whole), (i + whole).min(n - 1));
    let mut total = sum[z + 1] - sum[a];
    let mut count = (z + 1 - a) as f64;
    if frac > 0.0 {
        if a > 0 {
            total += frac * (sum[a] - sum[a - 1]);
            count += frac;
        }
        if z + 1 < n {
            total += frac * (sum[z + 2] - sum[z + 1]);
            count += frac;
        }
    }
    total / count
}

/// A bin's gain from the per-point gains.
fn read(gain: &[f64], r: BinRead) -> f64 {
    match r {
        BinRead::Lerp(p, t) => gain[p] + (gain[p + 1] - gain[p]) * t,
        BinRead::Mean(a, z) => gain[a..=z].iter().sum::<f64>() / (z + 1 - a) as f64,
    }
}

fn db_to_lin(db: f64) -> f64 {
    (db * (std::f64::consts::LN_10 / 20.0)).exp()
}

/// One band's real gain matrix for a linear gain `g`.
fn band_matrix(placement: Placement, g: f64) -> [f64; 4] {
    let (sum, diff) = (0.5 * (g + 1.0), 0.5 * (g - 1.0));
    match placement {
        Placement::Stereo => [g, 0.0, 0.0, g],
        Placement::Left => [g, 0.0, 0.0, 1.0],
        Placement::Right => [1.0, 0.0, 0.0, g],
        Placement::Mid => [sum, diff, diff, sum],
        Placement::Side => [sum, -diff, -diff, sum],
    }
}

/// How much of a band's per-frequency gain the engine applies at `f` (0…1): its [`region`], cut
/// off at 40 dB down and rescaled so it still falls continuously to 0 there.
pub fn weight(s: &BandSettings, f: f64) -> f64 {
    ((region(s, f) - WEIGHT_MIN) / (1.0 - WEIGHT_MIN)).max(0.0)
}

/// The shape of a band's range at `f` (0…1). Bells, notches and band passes use a band-pass
/// around their frequency at their Q; shelves a low- or high-pass at their frequency; a tilt the
/// whole spectrum. It is the filter's power, not its amplitude: that is how a bell's own gain
/// spreads over frequency, so the band works where its curve does (an amplitude skirt falls only
/// 6 dB/oct and reached frequencies the bell leaves alone).
pub fn region(s: &BandSettings, f: f64) -> f64 {
    let x = f / s.freq.clamp(FREQ_MIN, FREQ_MAX);
    let q = s.q.clamp(0.025, 40.0);
    let proto = match s.kind {
        BandType::Bell | BandType::Notch | BandType::BandPass => Analog::second(0.0, 1.0 / q, 0.0, 1.0, 1.0 / q, 1.0),
        BandType::LowShelf | BandType::LowCut => Analog::second(0.0, 0.0, 1.0, 1.0, std::f64::consts::SQRT_2, 1.0),
        BandType::HighShelf | BandType::HighCut => Analog::second(1.0, 0.0, 0.0, 1.0, std::f64::consts::SQRT_2, 1.0),
        BandType::TiltShelf | BandType::FlatTilt | BandType::AllPass => return 1.0,
    };
    proto.magnitude_sq(x).min(1.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::eq::DynamicsMode;

    const FS: f64 = 48_000.0;
    const SIZE: usize = 1024;

    fn noise(n: usize, seed: u32, amp: f64) -> Vec<f64> {
        let mut s = seed;
        (0..n)
            .map(|_| {
                s ^= s << 13;
                s ^= s >> 17;
                s ^= s << 5;
                (s as f64 / u32::MAX as f64 - 0.5) * 2.0 * amp
            })
            .collect()
    }

    fn tone(n: usize, freq: f64, amp: f64) -> Vec<f64> {
        (0..n).map(|i| amp * (2.0 * std::f64::consts::PI * freq * i as f64 / FS).sin()).collect()
    }

    fn mix(parts: &[&[f64]]) -> Vec<f32> {
        (0..parts[0].len()).map(|i| parts.iter().map(|p| p[i]).sum::<f64>() as f32).collect()
    }

    /// Runs stereo input through an STFT with `bands`.
    fn run(l: &[f32], r: &[f32], bands: &[BandSettings]) -> (Vec<f32>, Vec<f32>) {
        let mut stft = Stft::new(SIZE);
        let mut engine = SpectralBands::new(bands.len(), SIZE, FS);
        let (mut l, mut r) = (l.to_vec(), r.to_vec());
        for (a, b) in l.chunks_mut(512).zip(r.chunks_mut(512)) {
            engine.process(&mut stft, a, b, bands);
        }
        (l, r)
    }

    /// Amplitude of the `freq` component in `x` (the second half, after things settle).
    fn amplitude(x: &[f32], freq: f64) -> f64 {
        let x = &x[x.len() / 2..];
        let (mut re, mut im) = (0.0, 0.0);
        for (i, &v) in x.iter().enumerate() {
            let p = 2.0 * std::f64::consts::PI * freq * i as f64 / FS;
            re += f64::from(v) * p.cos();
            im += f64::from(v) * p.sin();
        }
        2.0 * (re * re + im * im).sqrt() / x.len() as f64
    }

    fn rms(x: &[f32]) -> f64 {
        let x = &x[x.len() / 2..];
        (x.iter().map(|v| f64::from(*v).powi(2)).sum::<f64>() / x.len() as f64).sqrt()
    }

    /// A relative spectral compressor: cuts what sticks out more than 6 dB over its surroundings
    /// back towards them (ratio 10), by up to 12 dB.
    fn relative_band(freq: f64, q: f64) -> BandSettings {
        let mut band = BandSettings { enabled: true, kind: BandType::Bell, freq, q, ..Default::default() };
        band.dynamics = Dynamics { enabled: true, spectral: true, relative: true, relative_db: 6.0, ratio: 10.0, range_db: -12.0, ..Dynamics::default() };
        band
    }

    #[test]
    fn relative_mode_cuts_a_tone_that_sticks_out_and_leaves_the_noise() {
        let n = 96_000;
        let (hiss, ring) = (noise(n, 1, 0.05), tone(n, 2_000.0, 0.3));
        let input = mix(&[&hiss, &ring]);
        let band = relative_band(2_000.0, 0.3);
        let (out, _) = run(&input, &input, &[band]);
        let cut = 20.0 * (amplitude(&out, 2_000.0) / amplitude(&input, 2_000.0)).log10();
        assert!(cut < -6.0, "the ringing tone only moved {cut:.2} dB");
        // The noise around it barely changes: measure without the tone.
        let (quiet, _) = run(&mix(&[&hiss]), &mix(&[&hiss]), &[band]);
        let noise_change = 20.0 * (rms(&quiet) / rms(&mix(&[&hiss]))).log10();
        assert!(noise_change.abs() < 1.0, "plain noise changed by {noise_change:.2} dB");
    }

    #[test]
    fn the_spectral_compressor_moves_only_the_loud_bins() {
        let n = 96_000;
        let (loud, soft) = (tone(n, 1_500.0, 0.5), tone(n, 3_000.0, 0.003));
        let input = mix(&[&loud, &soft]);
        let mut band = BandSettings { enabled: true, kind: BandType::Bell, freq: 2_000.0, q: 0.3, ..Default::default() };
        band.dynamics = Dynamics { enabled: true, mode: DynamicsMode::Compressor, spectral: true, threshold_db: -45.0, ratio: 4.0, range_db: -12.0, ..Dynamics::default() };
        let (out, _) = run(&input, &input, &[band]);
        let loud_cut = 20.0 * (amplitude(&out, 1_500.0) / amplitude(&input, 1_500.0)).log10();
        let soft_cut = 20.0 * (amplitude(&out, 3_000.0) / amplitude(&input, 3_000.0)).log10();
        assert!(loud_cut < -6.0, "loud tone moved {loud_cut:.2} dB");
        assert!(soft_cut.abs() < 0.5, "quiet tone moved {soft_cut:.2} dB");
    }

    #[test]
    fn moving_the_threshold_glides_the_cut_instead_of_stepping() {
        // A loud steady tone (−17.6 dB on the analyzer's scale) under a compressor; once settled
        // the threshold jumps from far below it to above it. The cut follows over several frames
        // (eased like the band's other parameters), not at once.
        let input = mix(&[&tone(48_000, 1_500.0, 0.5)]);
        let mut band = BandSettings { enabled: true, kind: BandType::Bell, freq: 1_500.0, q: 0.5, ..Default::default() };
        band.dynamics = Dynamics { enabled: true, mode: DynamicsMode::Compressor, spectral: true, smoothing: 0.0, threshold_db: -60.0, ratio: 4.0, range_db: -24.0, ..Dynamics::default() };
        let mut stft = Stft::new(SIZE);
        let mut engine = SpectralBands::new(1, SIZE, FS);
        let (mut l, mut r) = (input.clone(), input);
        let mut peaks = Vec::new();
        for (i, (a, b)) in l.chunks_mut(256).zip(r.chunks_mut(256)).enumerate() {
            if i == 120 {
                band.dynamics.threshold_db = -15.0;
            }
            engine.process(&mut stft, a, b, &[band]);
            peaks.push(engine.band_peaks()[0]);
        }
        let (before, after) = (peaks[119], *peaks.last().unwrap());
        assert!(after - before > 10.0, "the cut barely changed: {before:.1} → {after:.1} dB");
        let steps: Vec<f64> = peaks[119..].windows(2).map(|w| (w[1] - w[0]).abs()).collect();
        let largest = steps.iter().cloned().fold(0.0, f64::max);
        assert!(largest < 0.4 * (after - before), "the cut stepped {largest:.1} of {:.1} dB in one frame", after - before);
    }

    #[test]
    fn relative_mode_leaves_loud_even_material_alone_where_a_fixed_threshold_squashes_it() {
        // Loud broadband noise (−10 dBFS-ish bins well over a −40 dB threshold): the fixed
        // threshold pulls it all down, the relative one finds nothing sticking out.
        let n = 96_000;
        let input = mix(&[&noise(n, 6, 0.8)]);
        let change = |relative: bool| {
            let mut band = relative_band(2_000.0, 0.5);
            // A tilt covers every bin.
            band.kind = BandType::TiltShelf;
            band.dynamics.relative = relative;
            band.dynamics.threshold_db = -40.0;
            let (out, _) = run(&input, &input, &[band]);
            20.0 * (rms(&out) / rms(&input)).log10()
        };
        let (fixed, relative) = (change(false), change(true));
        assert!(fixed < -6.0, "fixed threshold only moved {fixed:.2} dB");
        assert!(relative > -1.0, "relative moved even noise by {relative:.2} dB");
    }

    #[test]
    fn spectral_smoothing_spreads_the_cut_to_neighbouring_frequencies() {
        // A loud tone and a quiet one a quarter of an octave above it (clear of the loud tone's
        // reading, ±1/12 octave, and between the STFT's faint frame-rate artefacts around it):
        // with no smoothing only the loud one moves; with full smoothing (half an octave) the
        // quiet one is pulled down too.
        let n = 96_000;
        let input = mix(&[&tone(n, 1_500.0, 0.5), &tone(n, 1_500.0 * 2f64.powf(0.25), 0.0015)]);
        let quiet_cut = |smoothing: f64| {
            let mut band = BandSettings { enabled: true, kind: BandType::Bell, freq: 2_000.0, q: 0.3, ..Default::default() };
            band.dynamics = Dynamics { enabled: true, mode: DynamicsMode::Compressor, spectral: true, smoothing, threshold_db: -50.0, ratio: 4.0, range_db: -12.0, ..Dynamics::default() };
            let (out, _) = run(&input, &input, &[band]);
            let f = 1_500.0 * 2f64.powf(0.25);
            20.0 * (amplitude(&out, f) / amplitude(&input, f)).log10()
        };
        let (alone, spread) = (quiet_cut(0.0), quiet_cut(1.0));
        assert!(alone.abs() < 0.5, "unsmoothed, the quiet tone moved {alone:.2} dB");
        assert!(spread < alone - 1.0, "smoothed, the quiet tone only moved {spread:.2} dB");
    }

    #[test]
    fn a_mid_band_leaves_the_side_alone() {
        let n = 96_000;
        let (hiss, ring) = (noise(n, 2, 0.05), tone(n, 2_000.0, 0.3));
        // The tone is in the mid; an opposite-polarity tone at another frequency is the side.
        let side = tone(n, 5_000.0, 0.3);
        let l = mix(&[&hiss, &ring, &side]);
        let r: Vec<f32> = (0..n).map(|i| (hiss[i] + ring[i] - side[i]) as f32).collect();
        let mut band = relative_band(3_000.0, 0.2);
        band.placement = Placement::Mid;
        let (ol, or) = run(&l, &r, &[band]);
        let s_in: Vec<f32> = l.iter().zip(&r).map(|(a, b)| 0.5 * (a - b)).collect();
        let s_out: Vec<f32> = ol.iter().zip(&or).map(|(a, b)| 0.5 * (a - b)).collect();
        let side_change = 20.0 * (amplitude(&s_out, 5_000.0) / amplitude(&s_in, 5_000.0)).log10();
        assert!(side_change.abs() < 0.1, "side moved {side_change:.3} dB");
        let m_out: Vec<f32> = ol.iter().zip(&or).map(|(a, b)| 0.5 * (a + b)).collect();
        let m_in: Vec<f32> = l.iter().zip(&r).map(|(a, b)| 0.5 * (a + b)).collect();
        assert!(20.0 * (amplitude(&m_out, 2_000.0) / amplitude(&m_in, 2_000.0)).log10() < -3.0);
    }

    #[test]
    fn plain_noise_is_left_nearly_alone() {
        // Noise has no resonances: its bins wobble from frame to frame, but relative mode should
        // not chase them. Measured as the mean cut over the band's bins once settled.
        let n = 96_000;
        let input = mix(&[&noise(n, 4, 0.2)]);
        let band = relative_band(2_000.0, 0.5);
        let mut stft = Stft::new(SIZE);
        let mut engine = SpectralBands::new(1, SIZE, FS);
        let (mut l, mut r) = (input.clone(), input.clone());
        let (mut sum, mut count) = (0.0, 0);
        for (i, (a, b)) in l.chunks_mut(256).zip(r.chunks_mut(256)).enumerate() {
            engine.process(&mut stft, a, b, &[band]);
            if i > 100 {
                let (gains, freqs) = engine.total_db();
                for (g, f) in gains.iter().zip(freqs) {
                    if (1_000.0..4_000.0).contains(f) {
                        sum += f64::from(*g);
                        count += 1;
                    }
                }
            }
        }
        let mean = sum / f64::from(count);
        assert!(mean > -0.25, "mean cut in noise {mean:.2} dB");
    }

    #[test]
    fn bands_that_do_not_use_the_spectrum_change_nothing() {
        let n = 20_000;
        let input = mix(&[&noise(n, 3, 0.2)]);
        let plain = BandSettings { enabled: true, kind: BandType::Bell, freq: 1_000.0, gain_db: 6.0, ..Default::default() };
        let (out, _) = run(&input, &input, &[plain]);
        let worst = (2 * SIZE..n).map(|i| (out[i] - input[i - SIZE]).abs()).fold(0.0f32, f32::max);
        assert!(worst < 1e-6, "{worst}");
    }
}
