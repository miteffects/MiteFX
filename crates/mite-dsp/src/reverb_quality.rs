//! Offline measurements of a reverb's impulse response: what makes a reverb sound smooth or
//! cheap, in numbers (docs/design/verb-brief.md §4). Allocates; never for the audio thread.
//!
//! - [`echo_density`]: Abel & Huang's normalised echo density (1: Gaussian, as a diffuse tail;
//!   well under 1: separate echoes heard as flutter or grain).
//! - [`periodicity`]: the tail's strongest normalised autocorrelation at lags of 0.5…60 ms (a
//!   repeating pattern is heard as flutter or a metallic pitch; noise gives about 1/√N).
//! - [`peakiness`]: how many of the tail's spectral bins stand over 10 dB above their
//!   neighbourhood's mean power (ringing modes; a Gaussian tail gives e⁻¹⁰ ≈ 0.005 %).

use realfft::RealFftPlanner;

/// The IR divided by its own envelope (RMS over `win` samples), so a decay does not weigh the
/// statistics towards its start.
pub fn flatten(ir: &[f32], win: usize) -> Vec<f64> {
    let n = ir.len();
    let sq: Vec<f64> = ir.iter().map(|&v| f64::from(v) * f64::from(v)).collect();
    let mut prefix = vec![0.0; n + 1];
    for i in 0..n {
        prefix[i + 1] = prefix[i] + sq[i];
    }
    (0..n)
        .map(|i| {
            let (a, b) = (i.saturating_sub(win / 2), (i + win / 2).min(n));
            let rms = ((prefix[b] - prefix[a]) / (b - a) as f64).sqrt();
            if rms > 1e-12 { f64::from(ir[i]) / rms } else { 0.0 }
        })
        .collect()
}

/// Normalised echo density per `step` samples over a 20 ms window: (fraction of samples whose
/// magnitude exceeds the window's standard deviation) / erfc(1/√2).
pub fn echo_density(ir: &[f32], fs: f64, step: usize) -> Vec<f64> {
    let win = (0.02 * fs) as usize;
    let mut out = Vec::new();
    let mut start = 0;
    while start + win <= ir.len() {
        let w = &ir[start..start + win];
        let sd = (w.iter().map(|&v| f64::from(v) * f64::from(v)).sum::<f64>() / win as f64).sqrt();
        let over = w.iter().filter(|&&v| f64::from(v).abs() > sd).count() as f64 / win as f64;
        out.push(over / 0.317_310_507_862_914);
        start += step;
    }
    out
}

/// The time (s) the echo density first holds ≥ 0.9 (its 5 ms steps averaged in threes).
pub fn dense_after(ir: &[f32], fs: f64) -> f64 {
    let step = (0.005 * fs) as usize;
    let d = echo_density(ir, fs, step);
    for i in 0..d.len().saturating_sub(2) {
        if (d[i] + d[i + 1] + d[i + 2]) / 3.0 >= 0.9 {
            return i as f64 * 0.005;
        }
    }
    f64::INFINITY
}

/// The flattened tail's (from `from` to `to` s) strongest |normalised autocorrelation| at lags
/// of 0.5…60 ms.
pub fn periodicity(ir: &[f32], fs: f64, from: f64, to: f64) -> f64 {
    let flat = flatten(ir, (0.01 * fs) as usize);
    let (a, b) = ((from * fs) as usize, ((to * fs) as usize).min(flat.len()));
    let x = &flat[a..b];
    let n = x.len();
    let len = (2 * n).next_power_of_two();
    let mut planner = RealFftPlanner::<f64>::new();
    let fwd = planner.plan_fft_forward(len);
    let inv = planner.plan_fft_inverse(len);
    let mut buf = vec![0.0; len];
    buf[..n].copy_from_slice(x);
    let mut spec = fwd.make_output_vec();
    fwd.process(&mut buf, &mut spec).expect("fft");
    for c in spec.iter_mut() {
        *c = realfft::num_complex::Complex::new(c.norm_sqr(), 0.0);
    }
    let mut ac = vec![0.0; len];
    inv.process(&mut spec, &mut ac).expect("ifft");
    let zero = ac[0];
    let (lo, hi) = ((0.0005 * fs) as usize, (0.06 * fs) as usize);
    (lo..hi.min(n)).map(|k| (ac[k] / zero).abs() * n as f64 / (n - k) as f64).fold(0.0, f64::max)
}

/// [`periodicity`]'s strongest lags (ms) and their correlations, the strongest first.
pub fn periodic_lags(ir: &[f32], fs: f64, from: f64, to: f64, count: usize) -> Vec<(f64, f64)> {
    let flat = flatten(ir, (0.01 * fs) as usize);
    let (a, b) = ((from * fs) as usize, ((to * fs) as usize).min(flat.len()));
    let x = &flat[a..b];
    let n = x.len();
    let (lo, hi) = ((0.0005 * fs) as usize, (0.06 * fs) as usize);
    let zero: f64 = x.iter().map(|v| v * v).sum();
    let mut r: Vec<(f64, f64)> = (lo..hi.min(n))
        .map(|k| {
            let c: f64 = x[..n - k].iter().zip(&x[k..]).map(|(p, q)| p * q).sum();
            (k as f64 / fs * 1000.0, c / zero * n as f64 / (n - k) as f64)
        })
        .collect();
    r.sort_by(|p, q| q.1.abs().total_cmp(&p.1.abs()));
    r.truncate(count);
    r
}

/// The fraction of the flattened tail's (from `from` to `to` s) spectral bins, 100 Hz…10 kHz,
/// standing over 10 dB above the mean power within a third of an octave around them.
pub fn peakiness(ir: &[f32], fs: f64, from: f64, to: f64) -> f64 {
    let flat = flatten(ir, (0.01 * fs) as usize);
    let (a, b) = ((from * fs) as usize, ((to * fs) as usize).min(flat.len()));
    let n = (b - a).next_power_of_two() / 2;
    let mut buf: Vec<f64> = (0..n).map(|i| flat[a + i] * (0.5 - 0.5 * (std::f64::consts::TAU * i as f64 / n as f64).cos())).collect();
    let mut planner = RealFftPlanner::<f64>::new();
    let fwd = planner.plan_fft_forward(n);
    let mut spec = fwd.make_output_vec();
    fwd.process(&mut buf, &mut spec).expect("fft");
    let p: Vec<f64> = spec.iter().map(|c| c.norm_sqr()).collect();
    let mut prefix = vec![0.0; p.len() + 1];
    for i in 0..p.len() {
        prefix[i + 1] = prefix[i] + p[i];
    }
    let hz = fs / n as f64;
    let (k0, k1) = ((100.0 / hz) as usize, ((10_000.0 / hz) as usize).min(p.len() - 1));
    let mut over = 0;
    for k in k0..k1 {
        let half = ((k as f64 * (2f64.powf(1.0 / 6.0) - 1.0)) as usize).max(4);
        let (lo, hi) = (k.saturating_sub(half), (k + half + 1).min(p.len()));
        let mean = (prefix[hi] - prefix[lo]) / (hi - lo) as f64;
        if p[k] > 10.0 * mean {
            over += 1;
        }
    }
    over as f64 / (k1 - k0) as f64
}

/// [`peakiness`]'s peaks: (Hz, dB over the neighbourhood's mean), the highest first.
pub fn peaks(ir: &[f32], fs: f64, from: f64, to: f64, count: usize) -> Vec<(f64, f64)> {
    let flat = flatten(ir, (0.01 * fs) as usize);
    let (a, b) = ((from * fs) as usize, ((to * fs) as usize).min(flat.len()));
    let n = (b - a).next_power_of_two() / 2;
    let mut buf: Vec<f64> = (0..n).map(|i| flat[a + i] * (0.5 - 0.5 * (std::f64::consts::TAU * i as f64 / n as f64).cos())).collect();
    let mut planner = RealFftPlanner::<f64>::new();
    let fwd = planner.plan_fft_forward(n);
    let mut spec = fwd.make_output_vec();
    fwd.process(&mut buf, &mut spec).expect("fft");
    let p: Vec<f64> = spec.iter().map(|c| c.norm_sqr()).collect();
    let mut prefix = vec![0.0; p.len() + 1];
    for i in 0..p.len() {
        prefix[i + 1] = prefix[i] + p[i];
    }
    let hz = fs / n as f64;
    let (k0, k1) = ((100.0 / hz) as usize, ((10_000.0 / hz) as usize).min(p.len() - 1));
    let mut out: Vec<(f64, f64)> = (k0..k1)
        .map(|k| {
            let half = ((k as f64 * (2f64.powf(1.0 / 6.0) - 1.0)) as usize).max(4);
            let (lo, hi) = (k.saturating_sub(half), (k + half + 1).min(p.len()));
            let mean = (prefix[hi] - prefix[lo]) / (hi - lo) as f64;
            (k as f64 * hz, 10.0 * (p[k] / mean).log10())
        })
        .collect();
    out.sort_by(|x, y| y.1.total_cmp(&x.1));
    out.truncate(count);
    out
}

/// A second-order high-pass at `fc` (the metrics judge what rings over the bass: a room's few low
/// modes are its nature; a metallic ring is not).
pub fn highpass(x: &[f32], fc: f64, fs: f64) -> Vec<f32> {
    let w = std::f64::consts::TAU * fc / fs;
    let (sin, cos) = w.sin_cos();
    let alpha = sin * std::f64::consts::FRAC_1_SQRT_2;
    let a0 = 1.0 + alpha;
    let (b0, b1, b2, a1, a2) = ((1.0 + cos) / 2.0 / a0, -(1.0 + cos) / a0, (1.0 + cos) / 2.0 / a0, -2.0 * cos / a0, (1.0 - alpha) / a0);
    let (mut x1, mut x2, mut y1, mut y2) = (0.0, 0.0, 0.0, 0.0);
    x.iter()
        .map(|&v| {
            let v = f64::from(v);
            let y = b0 * v + b1 * x1 + b2 * x2 - a1 * y1 - a2 * y2;
            x2 = x1;
            x1 = v;
            y2 = y1;
            y1 = y;
            y as f32
        })
        .collect()
}
