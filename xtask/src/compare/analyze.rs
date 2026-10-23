//! Measurements on rendered audio.

use realfft::num_complex::Complex;
use realfft::RealFftPlanner;

pub fn db(x: f64) -> f64 {
    20.0 * x.max(1e-30).log10()
}

/// Log-spaced frequencies from `lo` to `hi`, `n` of them.
pub fn log_freqs(lo: f64, hi: f64, n: usize) -> Vec<f64> {
    (0..n).map(|i| lo * (hi / lo).powf(i as f64 / (n - 1) as f64)).collect()
}

/// A frequency response at the requested frequencies: magnitude (dB) and phase (radians, with
/// the delay to `t0` removed).
#[derive(Debug, Clone)]
pub struct Response {
    pub mag_db: Vec<f64>,
    pub phase: Vec<f64>,
}

/// The response of impulse response `h` whose input impulse was at sample `t0`, from
/// `pre_s` before it to `post_s` after, at `freqs`: an exact DFT at each frequency (no bin
/// interpolation), over the stretch where the response is within 200 dB of its peak.
pub fn response(h: &[f32], rate: u32, t0: usize, pre_s: f64, post_s: f64, freqs: &[f64]) -> Response {
    let pre = ((pre_s * f64::from(rate)) as usize).min(t0);
    let post = (post_s * f64::from(rate)) as usize;
    let (start, end) = (t0 - pre, (t0 + post).min(h.len()));
    let peak = h[start..end].iter().fold(0.0f32, |m, v| m.max(v.abs()));
    let floor = peak * 1e-10;
    let first = start + h[start..end].iter().position(|v| v.abs() > floor).unwrap_or(0);
    let last = start + h[start..end].iter().rposition(|v| v.abs() > floor).unwrap_or(0) + 1;
    let mut mag_db = Vec::with_capacity(freqs.len());
    let mut phase = Vec::with_capacity(freqs.len());
    for &f in freqs {
        let w = std::f64::consts::TAU * f / f64::from(rate);
        // Rotate a phasor sample by sample, renormalizing now and then (exact to f64 rounding).
        let step = Complex::from_polar(1.0, -w);
        let mut z = Complex::from_polar(1.0, -w * (first as f64 - t0 as f64));
        let mut acc = Complex::new(0.0, 0.0);
        for (k, v) in h[first..last].iter().enumerate() {
            acc += z * f64::from(*v);
            z *= step;
            if k % 4096 == 4095 {
                z /= z.norm();
            }
        }
        mag_db.push(db(acc.norm()));
        phase.push(acc.arg());
    }
    Response { mag_db, phase }
}

/// The difference of two phases, wrapped to ±π.
pub fn phase_diff(a: f64, b: f64) -> f64 {
    let mut d = a - b;
    while d > std::f64::consts::PI {
        d -= std::f64::consts::TAU;
    }
    while d < -std::f64::consts::PI {
        d += std::f64::consts::TAU;
    }
    d
}

/// Timing of impulse response `h` against the input impulse at `t0`: the peak's offset
/// (samples) and the pre-ringing (how long before `t0` the response is within 60 dB of its peak,
/// ms, and its level before `t0`, dB re the peak).
#[derive(Debug, Clone, Copy)]
pub struct Timing {
    pub peak_offset: i64,
    pub pre_ring_ms: f64,
    pub pre_ring_db: f64,
}

pub fn timing(h: &[f32], rate: u32, t0: usize) -> Timing {
    let (peak_i, peak) = h.iter().enumerate().fold((0, 0.0f32), |(bi, bv), (i, v)| if v.abs() > bv { (i, v.abs()) } else { (bi, bv) });
    let floor = peak * 1e-3;
    let first = h.iter().position(|v| v.abs() > floor).unwrap_or(t0);
    let before = h[..t0.saturating_sub(1)].iter().fold(0.0f32, |m, v| m.max(v.abs()));
    Timing {
        peak_offset: peak_i as i64 - t0 as i64,
        pre_ring_ms: if first + 1 < t0 { (t0 - first) as f64 * 1000.0 / f64::from(rate) } else { 0.0 },
        pre_ring_db: db(f64::from(before) / f64::from(peak.max(1e-30))),
    }
}

fn rms(x: &[f32]) -> f64 {
    (x.iter().map(|v| f64::from(*v) * f64::from(*v)).sum::<f64>() / x.len().max(1) as f64).sqrt()
}

/// The gain (dB) of each held level of a stepped tone: output RMS over input RMS over the last
/// 40 % of each step.
pub fn step_gains(input: &[f32], output: &[f32], rate: u32, lead_s: f64, step_s: f64, steps: usize) -> Vec<f64> {
    let lead = (lead_s * f64::from(rate)) as usize;
    let step = (step_s * f64::from(rate)) as usize;
    (0..steps)
        .map(|k| {
            let a = lead + k * step + step * 6 / 10;
            let b = lead + (k + 1) * step;
            db(rms(&output[a..b]) / rms(&input[a..b]).max(1e-30))
        })
        .collect()
}

/// The gain (dB) over every `period` samples (one period of the test tone).
pub fn gain_envelope(input: &[f32], output: &[f32], period: usize) -> Vec<f64> {
    input
        .chunks(period)
        .zip(output.chunks(period))
        .map(|(i, o)| {
            let ri = rms(i);
            if ri < 1e-9 { 0.0 } else { db(rms(o) / ri) }
        })
        .collect()
}

/// THD+N of `x` (a sine at `f0`): everything but the fundamental, dB re the fundamental, over
/// 20 Hz…20 kHz (Blackman-Harris window).
pub fn thd_n(x: &[f32], rate: u32, f0: f64) -> f64 {
    let n = x.len().min(1 << 17).next_power_of_two() / 2;
    let w = |i: usize| {
        let t = std::f64::consts::TAU * i as f64 / n as f64;
        0.35875 - 0.48829 * t.cos() + 0.14128 * (2.0 * t).cos() - 0.01168 * (3.0 * t).cos()
    };
    let start = x.len() - n;
    let mut buf: Vec<f64> = (0..n).map(|i| f64::from(x[start + i]) * w(i)).collect();
    let fft = RealFftPlanner::<f64>::new().plan_fft_forward(n);
    let mut spec = fft.make_output_vec();
    let _ = fft.process(&mut buf, &mut spec);
    let hz = f64::from(rate) / n as f64;
    let k0 = (f0 / hz).round() as usize;
    let (mut fund, mut rest) = (0.0, 0.0);
    for (k, c) in spec.iter().enumerate() {
        let f = k as f64 * hz;
        if !(20.0..=20_000.0).contains(&f) {
            continue;
        }
        let p = c.norm_sqr();
        if k.abs_diff(k0) <= 4 { fund += p } else { rest += p }
    }
    10.0 * (rest / fund.max(1e-60)).log10()
}

/// Aliasing of `x` (a sine at `f0`): the energy away from the fundamental and its harmonics,
/// dB re the fundamental, 20 Hz…20 kHz (Blackman-Harris window). Harmonics are the compressor's
/// own distortion; whatever is left folded back from above Nyquist.
pub fn aliasing(x: &[f32], rate: u32, f0: f64) -> f64 {
    let n = x.len().min(1 << 17).next_power_of_two() / 2;
    let w = |i: usize| {
        let t = std::f64::consts::TAU * i as f64 / n as f64;
        0.35875 - 0.48829 * t.cos() + 0.14128 * (2.0 * t).cos() - 0.01168 * (3.0 * t).cos()
    };
    let start = x.len() - n;
    let mut buf: Vec<f64> = (0..n).map(|i| f64::from(x[start + i]) * w(i)).collect();
    let fft = RealFftPlanner::<f64>::new().plan_fft_forward(n);
    let mut spec = fft.make_output_vec();
    let _ = fft.process(&mut buf, &mut spec);
    let hz = f64::from(rate) / n as f64;
    let (mut fund, mut rest) = (0.0, 0.0);
    for (k, c) in spec.iter().enumerate() {
        let f = k as f64 * hz;
        if !(20.0..=20_000.0).contains(&f) {
            continue;
        }
        let h = (f / f0).round();
        let near_harmonic = h >= 1.0 && (f - h * f0).abs() <= 4.0 * hz;
        if near_harmonic && h == 1.0 {
            fund += c.norm_sqr();
        } else if !near_harmonic {
            rest += c.norm_sqr();
        }
    }
    10.0 * (rest / fund.max(1e-60)).log10()
}

/// The lag (samples) of `y` behind `x` with the highest correlation, within ±`max` samples.
pub fn lag(x: &[f32], y: &[f32], max: usize) -> i64 {
    let n = x.len().min(y.len());
    let mut best = (0i64, f64::MIN);
    for l in -(max as i64)..=(max as i64) {
        let end = n.saturating_sub(max);
        let c: f64 = x[max..end].iter().enumerate().map(|(k, v)| f64::from(*v) * f64::from(y[((max + k) as i64 + l) as usize])).sum();
        if c > best.1 {
            best = (l, c);
        }
    }
    best.0
}
