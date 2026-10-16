//! Zero Latency correction: what to add after the matched IIR bands so their magnitude follows
//! the analog prototypes up to Nyquist, without adding latency. Runs off the audio thread
//! (allocates).
//!
//! One matched biquad per band cannot follow the analog curve everywhere near Nyquist (it matches
//! three frequencies exactly and parts ways between them: by a dB or more for bells and shelves
//! in the top octave). The residual analog / digital, in magnitude only, becomes a
//! **minimum-phase** FIR (real cepstrum, folded): causal, front-loaded, no delay. It corrects the
//! magnitude only: the phase stays the digital minimum phase of the band-limited curve, which
//! differs from the analog prototype's (that depends on the curve above Nyquist too) by a few
//! degrees in the mids and about 15° near Nyquist, as without the correction. Matching the phase
//! as well is Natural Phase's job.
//!
//! Bands on the left, right or both channels are corrected per channel; mid and side bands in the
//! mid/side domain, after them (the two corrections are exact for each group and, together,
//! for every chain that does not mix mid/side bands with one-sided ones).

use realfft::num_complex::Complex;
use realfft::RealFftPlanner;

use crate::eq::{analog_response_db, design as design_band, BandSettings, Placement};

/// The residual is never corrected by more than this, dB (the gain range; a resonant shelf near
/// Nyquist can be that far off). Stopbands are handled separately below.
const MAX_CORRECTION_DB: f64 = 30.0;
/// The kernel lengths tried, shortest first, at 48 kHz (scaled with the sample rate): the
/// correction uses the shortest that brings the response within [`TARGET_DB`] of the analog
/// one where it is audible (within 24 dB of unity), or the longest.
pub const TAPS_48K: [usize; 4] = [64, 128, 256, 512];
const TARGET_DB: f64 = 0.05;
const AUDIBLE_DB: f64 = -24.0;
/// Where the analog response falls from −40 to −60 dB (a cut's stopband), the correction eases
/// back to none.
const STOPBAND_FULL_DB: f64 = -40.0;
const STOPBAND_NONE_DB: f64 = -60.0;
/// A correction whose residual stays inside this everywhere is not worth running, dB.
const NEGLIGIBLE_DB: f64 = 0.005;

/// The minimum-phase FIR (`taps` long) whose magnitude is `exp(log_mag)`, `log_mag` holding the
/// natural log of the magnitude at the `n/2 + 1` bins of an `n`-point FFT. Its tail is tapered
/// over the last quarter so the truncation does not ring.
pub fn min_phase_fir(log_mag: &[f64], taps: usize) -> Vec<f64> {
    let n = 2 * (log_mag.len() - 1);
    let mut planner = RealFftPlanner::<f64>::new();
    let (forward, inverse) = (planner.plan_fft_forward(n), planner.plan_fft_inverse(n));
    // Real cepstrum of the (real, even) log magnitude.
    let mut spectrum: Vec<Complex<f64>> = log_mag.iter().map(|&v| Complex::new(v, 0.0)).collect();
    let mut cepstrum = vec![0.0; n];
    inverse.process(&mut spectrum, &mut cepstrum).expect("sizes match");
    let scale = 1.0 / n as f64;
    // Fold onto positive quefrencies: the minimum-phase spectrum's log.
    for (i, c) in cepstrum.iter_mut().enumerate() {
        *c *= scale
            * match i {
                0 => 1.0,
                i if i < n / 2 => 2.0,
                i if i == n / 2 => 1.0,
                _ => 0.0,
            };
    }
    let mut log_min = vec![Complex::new(0.0, 0.0); n / 2 + 1];
    forward.process(&mut cepstrum, &mut log_min).expect("sizes match");
    let mut min: Vec<Complex<f64>> = log_min.iter().map(|z| z.exp()).collect();
    // The first and last bins of a real signal's spectrum are real.
    min[0].im = 0.0;
    min[n / 2].im = 0.0;
    let mut h = vec![0.0; n];
    inverse.process(&mut min, &mut h).expect("sizes match");
    let taper = taps / 4;
    (0..taps.min(n))
        .map(|i| {
            let from_end = taps - i;
            let w = if from_end <= taper { 0.5 - 0.5 * (std::f64::consts::PI * from_end as f64 / taper as f64).cos() } else { 1.0 };
            h[i] * scale * w
        })
        .collect()
}

/// The correction kernels: per channel (left, right; mono uses the left), and, when mid or side
/// bands are present, per mid/side after them.
#[derive(Debug, Clone, PartialEq)]
pub struct Correction {
    pub lr: [Vec<f32>; 2],
    pub ms: Option<[Vec<f32>; 2]>,
}

/// The correction for `bands` at `sample_rate`: minimum-phase FIRs of one of `lengths` (the
/// shortest that meets the target, per group), designed on an `fft`-point grid. `mono`: every
/// band processes the one channel. `None` when the bands already match their prototypes (no
/// bands, or all far from Nyquist) and nothing needs to run.
pub fn zero_latency_correction(bands: &[BandSettings], sample_rate: f64, lengths: &[usize], fft: usize, mono: bool) -> Option<Correction> {
    let bins = fft / 2 + 1;
    let active: Vec<_> = bands.iter().filter(|s| s.enabled).map(|s| (s, design_band(s, sample_rate))).collect();
    if active.is_empty() {
        return None;
    }
    // Residual in dB per group (left, right, mid, side), and the group's analog response.
    let mut residual = vec![[0.0f64; 4]; bins];
    let mut analog_total = vec![[0.0f64; 4]; bins];
    let ln_db = std::f64::consts::LN_10 / 20.0;
    for (k, (r, total)) in residual.iter_mut().zip(analog_total.iter_mut()).enumerate() {
        // Bin 0 is DC, where analog and digital agree exactly; evaluate just above it.
        let f = (k as f64 * sample_rate / fft as f64).max(1.0);
        let w = 2.0 * std::f64::consts::PI * f / sample_rate;
        for (s, sections) in &active {
            let analog = analog_response_db(s, f);
            let digital = sections.response_db(w.min(std::f64::consts::PI));
            let ease = ((analog - STOPBAND_NONE_DB) / (STOPBAND_FULL_DB - STOPBAND_NONE_DB)).clamp(0.0, 1.0);
            let d = if analog.is_finite() && digital.is_finite() { (analog - digital) * ease } else { 0.0 };
            let groups: &[usize] = match (mono, s.placement) {
                (true, _) | (_, Placement::Stereo) => &[0, 1],
                (_, Placement::Left) => &[0],
                (_, Placement::Right) => &[1],
                (_, Placement::Mid) => &[2],
                (_, Placement::Side) => &[3],
            };
            for &g in groups {
                r[g] += d;
                total[g] += if analog.is_finite() { analog } else { -300.0 };
            }
        }
    }
    let mut planner = RealFftPlanner::<f64>::new();
    let forward = planner.plan_fft_forward(fft);
    // A kernel's worst error against the residual where the group is audible, dB.
    let error = |h: &[f64], g: usize| {
        let mut padded = vec![0.0; fft];
        padded[..h.len()].copy_from_slice(h);
        let mut spectrum = vec![Complex::new(0.0, 0.0); bins];
        forward.process(&mut padded, &mut spectrum).expect("sizes match");
        spectrum
            .iter()
            .zip(&residual)
            .zip(&analog_total)
            .filter(|(_, total)| total[g] > AUDIBLE_DB)
            .map(|((z, r), _)| (20.0 * z.norm().max(1e-12).log10() - r[g].clamp(-MAX_CORRECTION_DB, MAX_CORRECTION_DB)).abs())
            .fold(0.0, f64::max)
    };
    let kernel = |g: usize| -> Option<Vec<f32>> {
        let worst = residual.iter().map(|r| r[g].abs()).fold(0.0, f64::max);
        (worst > NEGLIGIBLE_DB).then(|| {
            let log_mag: Vec<f64> = residual.iter().map(|r| r[g].clamp(-MAX_CORRECTION_DB, MAX_CORRECTION_DB) * ln_db).collect();
            let mut best = Vec::new();
            for &taps in lengths {
                best = min_phase_fir(&log_mag, taps);
                if error(&best, g) <= TARGET_DB {
                    break;
                }
            }
            best.into_iter().map(|v| v as f32).collect()
        })
    };
    let identity = || vec![1.0f32];
    let (l, r, m, sd) = (kernel(0), kernel(1), kernel(2), kernel(3));
    if l.is_none() && r.is_none() && m.is_none() && sd.is_none() {
        return None;
    }
    let ms = (m.is_some() || sd.is_some()).then(|| [m.unwrap_or_else(identity), sd.unwrap_or_else(identity)]);
    Some(Correction { lr: [l.unwrap_or_else(identity), r.unwrap_or_else(identity)], ms })
}

/// Kernel lengths the audio-side filter is sized for at `rate_ratio` × 48 kHz.
pub fn max_taps(rate_ratio: usize) -> usize {
    TAPS_48K[TAPS_48K.len() - 1] * rate_ratio.max(1)
}

impl Correction {
    /// Passes the signal unchanged (a single unit tap everywhere, no mid/side stage).
    pub fn identity() -> Self {
        Self { lr: [vec![1.0], vec![1.0]], ms: None }
    }

    fn is_identity(&self) -> bool {
        self.ms.is_none() && self.lr.iter().all(|k| k.len() == 1 && k[0] == 1.0)
    }
}

/// `h · x` over the kernel, in eight lanes so it vectorises.
#[inline]
fn dot(h: &[f32], x: &[f32]) -> f32 {
    let n = h.len().min(x.len());
    let (h, x) = (&h[..n], &x[..n]);
    let mut acc = [0.0f32; 8];
    let chunks = n / 8;
    for (hc, xc) in h.chunks_exact(8).zip(x.chunks_exact(8)) {
        for i in 0..8 {
            acc[i] += hc[i] * xc[i];
        }
    }
    let mut sum = acc.iter().sum::<f32>();
    for i in chunks * 8..n {
        sum += h[i] * x[i];
    }
    sum
}

/// The audio side of the correction: direct-form minimum-phase FIRs (no latency), per channel and
/// then per mid/side. A new kernel crossfades in over `fade` samples. Kernels arrive and leave as
/// boxes so the audio thread never allocates or frees. Allocation-free after construction.
pub struct CorrectionFilter {
    kernel: Option<Box<Correction>>,
    previous: Option<Box<Correction>>,
    fade_len: usize,
    faded: usize,
    /// Input history per channel (left/right, then mid/side), newest first, stored twice so any
    /// kernel-long window is one contiguous slice.
    history: [Vec<f32>; 4],
    capacity: usize,
    pos: usize,
}

impl CorrectionFilter {
    /// Sized for kernels up to `max_taps`, crossfading over `fade` samples. Allocates.
    pub fn new(max_taps: usize, fade: usize) -> Self {
        let capacity = max_taps.max(1);
        Self {
            kernel: None,
            previous: None,
            fade_len: fade.max(1),
            faded: 0,
            history: std::array::from_fn(|_| vec![0.0; 2 * capacity]),
            capacity,
            pos: 0,
        }
    }

    pub fn reset(&mut self) {
        for h in &mut self.history {
            h.fill(0.0);
        }
        self.pos = 0;
    }

    /// Whether a crossfade is under way (a new kernel cannot be taken until it ends).
    pub fn is_fading(&self) -> bool {
        self.previous.is_some()
    }

    /// Takes `kernel` (crossfading from the current one). Hands back the kernel it no longer
    /// needs (the one faded out), or `kernel` itself while a crossfade is still running.
    pub fn offer(&mut self, kernel: Box<Correction>) -> Option<Box<Correction>> {
        if self.previous.is_some() {
            return Some(kernel);
        }
        if kernel.lr.iter().chain(kernel.ms.iter().flatten()).any(|k| k.len() > self.capacity) {
            return Some(kernel);
        }
        self.previous = self.kernel.replace(kernel);
        self.faded = 0;
        if self.previous.is_none() {
            // From nothing: fade in from the plain signal.
            self.previous = Some(Box::new(Correction::identity()));
        }
        None
    }

    /// The kernel that finished fading out, for the caller to give back.
    pub fn retired(&mut self) -> Option<Box<Correction>> {
        if self.previous.is_some() && self.faded >= self.fade_len {
            return self.previous.take();
        }
        None
    }

    /// Drops the current kernel at once (a mode switch, under the engine's dip) and hands it back.
    pub fn clear(&mut self) -> [Option<Box<Correction>>; 2] {
        self.faded = 0;
        [self.kernel.take(), self.previous.take()]
    }

    fn push(&mut self, ch: usize, x: f32) {
        let (p, cap) = (self.pos, self.capacity);
        self.history[ch][p] = x;
        self.history[ch][p + cap] = x;
    }

    /// One kernel's output for the current sample (history already holds it).
    fn run(&self, k: &Correction, channels: usize, out: &mut [f32; 2]) {
        let p = self.pos;
        for (c, o) in out.iter_mut().enumerate().take(channels) {
            *o = dot(&k.lr[c], &self.history[c][p..]);
        }
        if let (Some(ms), true) = (&k.ms, channels == 2) {
            let (m, s) = (dot(&ms[0], &self.history[2][p..]), dot(&ms[1], &self.history[3][p..]));
            *out = [m + s, m - s];
        }
    }

    /// Processes one or two channels in place (sample by sample across the channels: the
    /// kernels read every channel's history at each sample).
    #[allow(clippy::needless_range_loop)]
    pub fn process(&mut self, channels: &mut [&mut [f32]]) {
        let Some(kernel) = self.kernel.as_deref() else { return };
        if self.previous.is_none() && kernel.is_identity() {
            // Nothing to correct; the history still follows, so a kernel can start cleanly.
            let frames = channels.first().map_or(0, |c| c.len());
            for i in 0..frames {
                self.pos = if self.pos == 0 { self.capacity - 1 } else { self.pos - 1 };
                for c in 0..channels.len().min(2) {
                    let x = channels[c][i];
                    self.push(c, x);
                }
                if channels.len() >= 2 {
                    let (l, r) = (channels[0][i], channels[1][i]);
                    self.push(2, 0.5 * (l + r));
                    self.push(3, 0.5 * (l - r));
                }
            }
            return;
        }
        let n = channels.len().min(2);
        let frames = channels.first().map_or(0, |c| c.len());
        // Whether any kernel in play has a mid/side stage (its history is only fed then; a
        // kernel that brings one in starts from silence there, under the crossfade).
        let ms = self.kernel.as_ref().is_some_and(|k| k.ms.is_some()) || self.previous.as_ref().is_some_and(|k| k.ms.is_some());
        for i in 0..frames {
            self.pos = if self.pos == 0 { self.capacity - 1 } else { self.pos - 1 };
            for c in 0..n {
                let x = channels[c][i];
                self.push(c, x);
            }
            // The mid/side stage hears the current kernel's per-channel output.
            let mut lr = [0.0f32; 2];
            if let Some(kernel) = self.kernel.as_deref() {
                for (c, o) in lr.iter_mut().enumerate().take(n) {
                    *o = dot(&kernel.lr[c], &self.history[c][self.pos..]);
                }
            }
            if n == 2 && ms {
                self.push(2, 0.5 * (lr[0] + lr[1]));
                self.push(3, 0.5 * (lr[0] - lr[1]));
            }
            // Without a mid/side stage the output is the per-channel stage, already computed.
            let mut y = lr;
            if ms || self.previous.is_some() {
                if let Some(kernel) = self.kernel.as_deref() {
                    self.run(kernel, n, &mut y);
                }
            }
            if let Some(old) = self.previous.as_deref() {
                let mut z = [0.0f32; 2];
                self.run(old, n, &mut z);
                let t = (self.faded as f32 / self.fade_len as f32).min(1.0);
                for c in 0..n {
                    y[c] = z[c] + t * (y[c] - z[c]);
                }
                self.faded += 1;
            }
            for c in 0..n {
                channels[c][i] = y[c];
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::eq::BandType;

    fn magnitude_db(h: &[f64], w: f64) -> f64 {
        let z: Complex<f64> = h.iter().enumerate().map(|(n, &v)| Complex::from_polar(v, -w * n as f64)).sum();
        20.0 * z.norm().log10()
    }

    #[test]
    fn the_fir_has_the_magnitude_asked_for_and_no_delay() {
        // A gentle bump near the top: +3 dB around 0.8π.
        let fft = 4096;
        let target = |w: f64| 3.0 * (-((w - 0.8 * std::f64::consts::PI) / 0.15).powi(2)).exp();
        let ln_db = std::f64::consts::LN_10 / 20.0;
        let log_mag: Vec<f64> = (0..=fft / 2).map(|k| target(std::f64::consts::PI * k as f64 / (fft / 2) as f64) * ln_db).collect();
        let h = min_phase_fir(&log_mag, 128);
        for i in 1..100 {
            let w = std::f64::consts::PI * i as f64 / 100.0;
            assert!((magnitude_db(&h, w) - target(w)).abs() < 0.02, "at {w:.2}: {} vs {}", magnitude_db(&h, w), target(w));
        }
        // Minimum phase: the energy is at the front (no pre-delay).
        assert!(h[0].abs() > 0.8, "{}", h[0]);
        let energy: f64 = h.iter().map(|v| v * v).sum();
        let front: f64 = h[..8].iter().map(|v| v * v).sum();
        assert!(front / energy > 0.99, "{}", front / energy);
    }

    #[test]
    fn the_filter_runs_a_kernel_with_no_delay_and_crossfades_to_the_next() {
        let mut f = CorrectionFilter::new(64, 32);
        // A kernel that halves the left and passes the right.
        let half = Correction { lr: [vec![0.5], vec![1.0]], ms: None };
        assert!(f.offer(Box::new(half)).is_none());
        let (mut l, mut r) = (vec![1.0f32; 64], vec![1.0f32; 64]);
        f.process(&mut [&mut l, &mut r]);
        // From the plain signal into the kernel over the fade, then exactly the kernel.
        assert_eq!(l[0], 1.0);
        assert!((l[63] - 0.5).abs() < 1e-6 && r.iter().all(|&v| (v - 1.0).abs() < 1e-6));
        assert!(l.windows(2).all(|w| w[1] <= w[0] + 1e-6), "fades smoothly");
        assert!(f.retired().is_some_and(|k| k.is_identity()));
        // A one-sample delay kernel shows the history works: the output lags by one.
        let delay = Correction { lr: [vec![0.0, 1.0], vec![0.0, 1.0]], ms: None };
        assert!(f.offer(Box::new(delay)).is_none());
        let mut warm = (vec![0.0f32; 64], vec![0.0f32; 64]);
        f.process(&mut [&mut warm.0, &mut warm.1]);
        let _ = f.retired();
        let ramp: Vec<f32> = (0..16).map(|i| i as f32).collect();
        let (mut a, mut b) = (ramp.clone(), ramp.clone());
        f.process(&mut [&mut a, &mut b]);
        assert_eq!(&a[1..], &ramp[..15]);
    }

    #[test]
    fn no_bands_need_no_correction() {
        assert!(zero_latency_correction(&[BandSettings::default(); 4], 48_000.0, &TAPS_48K, 4096, false).is_none());
        // A low, narrow bell already matches: nothing to run.
        let low = BandSettings { enabled: true, kind: BandType::Bell, freq: 200.0, gain_db: 6.0, q: 4.0, ..Default::default() };
        assert!(zero_latency_correction(&[low], 48_000.0, &TAPS_48K, 4096, false).is_none());
    }
}
