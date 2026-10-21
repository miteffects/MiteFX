//! EQ Match and masking: long-term spectra, fitting bands to a target curve, and finding where
//! two spectra collide. Everything works on curves read at the analyzer's log-spaced points
//! ([`crate::analyzer::spectrum_point_freq`]); the bands are judged by their analog response,
//! which is what the EQ plays and draws. These run on the UI thread or a worker; they allocate.

use crate::eq::{analog_response_db, BandSettings, BandType, GAIN_RANGE_DB};

/// The lowest and highest frequencies a match or a carve listens to, Hz.
pub const MATCH_LO: f64 = 25.0;
pub const MATCH_HI: f64 = 18_000.0;
/// Bands a fit places stay inside these.
const FIT_FREQ: (f64, f64) = (20.0, 20_000.0);
const FIT_Q: (f64, f64) = (0.2, 8.0);
/// Gains a fit places stay within this, dB (less than the full range: a match is a tonal
/// correction, not a filter).
const FIT_GAIN_DB: f64 = 18.0;
/// A fit stops adding bands once the worst residual is below this, dB.
const FIT_DONE_DB: f64 = 0.3;

/// A spectrum averaged over time in power, at the analyzer's points: since it was cleared
/// (`time_constant_s` = ∞, a match's reference) or exponentially (masking).
pub struct LongTermSpectrum {
    power: Vec<f64>,
    weight: f64,
    time_constant_s: f64,
}

impl LongTermSpectrum {
    pub fn new(points: usize, time_constant_s: f64) -> Self {
        Self { power: vec![0.0; points], weight: 0.0, time_constant_s }
    }

    pub fn clear(&mut self) {
        self.power.iter_mut().for_each(|p| *p = 0.0);
        self.weight = 0.0;
    }

    /// Adds one analysis frame (`db` at every point) that stands for `dt` seconds of audio.
    pub fn add(&mut self, db: &[f32], dt: f64) {
        let w = if self.time_constant_s.is_finite() {
            // Exponential: the frame's share of the average.
            self.weight = (self.weight + dt).min(self.time_constant_s);
            dt / self.weight
        } else {
            self.weight += dt;
            dt / self.weight
        };
        for (p, &d) in self.power.iter_mut().zip(db) {
            *p += (10f64.powf(f64::from(d) / 10.0) - *p) * w;
        }
    }

    /// Seconds of audio averaged (capped at the time constant for an exponential average).
    pub fn seconds(&self) -> f64 {
        self.weight
    }

    /// The average in dB, −150 before anything was added.
    pub fn read_db(&self, out: &mut [f64]) {
        for (o, &p) in out.iter_mut().zip(&self.power) {
            *o = 10.0 * p.max(1e-15).log10();
        }
    }
}

/// Smooths `curve` (log-spaced points, `point_oct` octaves apart) with a raised-cosine window
/// `width_oct` octaves wide, renormalized at the ends.
pub fn smooth_curve(curve: &[f64], weight: &[f64], point_oct: f64, width_oct: f64) -> Vec<f64> {
    let half = ((0.5 * width_oct / point_oct).round() as isize).max(0);
    let n = curve.len() as isize;
    (0..n)
        .map(|i| {
            let (mut sum, mut total) = (0.0, 0.0);
            for k in -half..=half {
                let j = i + k;
                if j < 0 || j >= n {
                    continue;
                }
                let w = if half == 0 { 1.0 } else { 0.5 * (1.0 + (std::f64::consts::PI * k as f64 / (half + 1) as f64).cos()) } * weight[j as usize];
                sum += curve[j as usize] * w;
                total += w;
            }
            if total > 0.0 { sum / total } else { 0.0 }
        })
        .collect()
}

/// Where a match listens: points inside [`MATCH_LO`]…[`MATCH_HI`] where both spectra are above
/// `floor_db`, with soft edges so a band at the edge is not pulled by a cliff.
pub fn match_weight(freqs: &[f64], a_db: &[f64], b_db: &[f64], floor_db: f64) -> Vec<f64> {
    freqs
        .iter()
        .zip(a_db.iter().zip(b_db))
        .map(|(&f, (&a, &b))| {
            let edge = |x: f64| x.clamp(0.0, 1.0);
            let band = edge((f / MATCH_LO).log2() * 3.0) * edge((MATCH_HI / f).log2() * 3.0);
            let level = edge((a.min(b) - floor_db) / 6.0);
            band * level
        })
        .collect()
}

/// The match's target: the reference over the source, smoothed `smooth_oct`, without its
/// weighted mean (a match changes tone, not level), times `amount` (0…1).
pub fn match_target(reference_db: &[f64], source_db: &[f64], weight: &[f64], point_oct: f64, smooth_oct: f64, amount: f64) -> Vec<f64> {
    let diff: Vec<f64> = reference_db.iter().zip(source_db).map(|(r, s)| r - s).collect();
    let smooth = smooth_curve(&diff, weight, point_oct, smooth_oct);
    let total: f64 = weight.iter().sum();
    let mean = if total > 0.0 { smooth.iter().zip(weight).map(|(d, w)| d * w).sum::<f64>() / total } else { 0.0 };
    smooth.iter().map(|d| (d - mean) * amount).collect()
}

fn band(kind: BandType, freq: f64, gain_db: f64, q: f64) -> BandSettings {
    BandSettings { enabled: true, kind, freq, gain_db, q, ..BandSettings::default() }
}

fn response(s: &BandSettings, freqs: &[f64]) -> Vec<f64> {
    freqs.iter().map(|&f| analog_response_db(s, f)).collect()
}

fn error(target: &[f64], weight: &[f64], responses: &[Vec<f64>]) -> f64 {
    (0..target.len())
        .map(|i| {
            let r = target[i] - responses.iter().map(|b| b[i]).sum::<f64>();
            weight[i] * r * r
        })
        .sum()
}

/// A bell's Q from its width between the half-gain points, octaves.
fn q_from_width(oct: f64) -> f64 {
    let r = 2f64.powf(oct.max(0.05));
    (r.sqrt() / (r - 1.0)).clamp(FIT_Q.0, FIT_Q.1)
}

/// Fits at most `max_bands` bands (bells, and shelves at the ends) whose summed analog response
/// approaches `target` (dB at `freqs`) where `weight` is above 0. Greedy: each band goes where
/// the residual is worst, then every band placed so far is refined together.
pub fn fit(freqs: &[f64], target: &[f64], weight: &[f64], max_bands: usize) -> Vec<BandSettings> {
    let n = freqs.len();
    let mut bands: Vec<BandSettings> = Vec::new();
    let mut responses: Vec<Vec<f64>> = Vec::new();
    let listened: Vec<usize> = (0..n).filter(|&i| weight[i] > 0.5).collect();
    let (Some(&first), Some(&last)) = (listened.first(), listened.last()) else {
        return bands;
    };
    for _ in 0..max_bands {
        let residual: Vec<f64> = (0..n).map(|i| target[i] - responses.iter().map(|b| b[i]).sum::<f64>()).collect();
        let Some(peak) = (first..=last).max_by(|&a, &b| (residual[a].abs() * weight[a]).total_cmp(&(residual[b].abs() * weight[b]))) else {
            break;
        };
        let g = residual[peak];
        if g.abs() * weight[peak] < FIT_DONE_DB {
            break;
        }
        // Walk out to where the residual has fallen to half (or changes sign).
        let half = |step: isize| {
            let mut j = peak as isize;
            while j + step >= first as isize && j + step <= last as isize && residual[(j + step) as usize] * g.signum() > 0.5 * g.abs() {
                j += step;
            }
            j as usize
        };
        let (lo, hi) = (half(-1), half(1));
        let oct = (freqs[hi] / freqs[lo]).log2();
        let candidate = if lo == first && oct > 1.0 {
            // A step at the bottom of the range: a low shelf through its half-gain point.
            band(BandType::LowShelf, freqs[hi], g, std::f64::consts::FRAC_1_SQRT_2)
        } else if hi == last && oct > 1.0 {
            band(BandType::HighShelf, freqs[lo], g, std::f64::consts::FRAC_1_SQRT_2)
        } else {
            band(BandType::Bell, freqs[peak], g, q_from_width(oct))
        };
        responses.push(response(&candidate, freqs));
        bands.push(candidate);
        refine(freqs, target, weight, &mut bands, &mut responses);
    }
    bands
}

/// Coordinate descent over every band's frequency (octaves), gain (dB) and Q (natural log), with
/// steps halving whenever no move helps.
fn refine(freqs: &[f64], target: &[f64], weight: &[f64], bands: &mut [BandSettings], responses: &mut [Vec<f64>]) {
    let mut best = error(target, weight, responses);
    let mut steps = [0.25, 1.0, 0.25];
    for _ in 0..6 {
        for _ in 0..4 {
            let mut moved = false;
            for b in 0..bands.len() {
                for (param, &step) in steps.iter().enumerate() {
                    for dir in [-1.0, 1.0] {
                        let mut s = bands[b];
                        let d = step * dir;
                        match param {
                            0 => s.freq = (s.freq * 2f64.powf(d)).clamp(FIT_FREQ.0, FIT_FREQ.1),
                            1 => s.gain_db = (s.gain_db + d).clamp(-FIT_GAIN_DB, FIT_GAIN_DB).clamp(-GAIN_RANGE_DB, GAIN_RANGE_DB),
                            _ => s.q = (s.q * d.exp()).clamp(FIT_Q.0, FIT_Q.1),
                        }
                        if s == bands[b] {
                            continue;
                        }
                        let previous = std::mem::replace(&mut responses[b], response(&s, freqs));
                        let e = error(target, weight, responses);
                        if e < best - 1e-9 {
                            best = e;
                            bands[b] = s;
                            moved = true;
                        } else {
                            responses[b] = previous;
                        }
                    }
                }
            }
            if !moved {
                break;
            }
        }
        steps.iter_mut().for_each(|s| *s *= 0.5);
    }
}

/// A range of points where two spectra collide: `start..=end`, the point where they collide
/// most, and how far both stand above their own typical level there, dB.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Collision {
    pub start: usize,
    pub end: usize,
    pub peak: usize,
    pub depth_db: f64,
}

/// Where a region of masking opens and closes, dB of shared prominence (hysteresis).
pub const MASK_OPEN_DB: f64 = 3.0;
pub const MASK_CLOSE_DB: f64 = 1.5;
/// Regions narrower than this are dropped, octaves.
pub const MASK_MIN_OCT: f64 = 1.0 / 6.0;

/// The neighbourhood a peak stands out from, octaves.
const PEAK_NEIGHBOURHOOD_OCT: f64 = 3.0;
/// A peak further than this below the spectrum's median does not count, dB.
const BURIED_DB: f64 = 6.0;

/// How far `db` peaks at every point: above its own neighbourhood ([`PEAK_NEIGHBOURHOOD_OCT`]),
/// and never counted from lower than [`BURIED_DB`] under its median over the points `weight`
/// keeps. A broadly bright range is not a peak; a bump well under everything else is not either.
fn prominence(db: &[f64], weight: &[f64], point_oct: f64) -> Vec<f64> {
    let mut kept: Vec<f64> = db.iter().zip(weight).filter(|&(_, &w)| w > 0.5).map(|(&d, _)| d).collect();
    if kept.is_empty() {
        return vec![f64::NEG_INFINITY; db.len()];
    }
    kept.sort_by(f64::total_cmp);
    let floor = kept[kept.len() / 2] - BURIED_DB;
    let local = smooth_curve(db, weight, point_oct, PEAK_NEIGHBOURHOOD_OCT);
    db.iter().zip(&local).map(|(d, l)| d - l.max(floor)).collect()
}

/// The shared prominence of two (tilted) spectra, `point_oct` octaves apart: at each point, how
/// far the lesser of the two peaks (see [`prominence`]), where both are heard (`weight`), else −∞.
pub fn masking_score(a_db: &[f64], b_db: &[f64], weight: &[f64], point_oct: f64) -> Vec<f64> {
    let (pa, pb) = (prominence(a_db, weight, point_oct), prominence(b_db, weight, point_oct));
    (0..a_db.len()).map(|i| if weight[i] > 0.5 { pa[i].min(pb[i]) } else { f64::NEG_INFINITY }).collect()
}

/// The collisions in `score` (from [`masking_score`]): ranges that rise past [`MASK_OPEN_DB`],
/// extended while they stay above [`MASK_CLOSE_DB`], at least [`MASK_MIN_OCT`] wide.
pub fn collisions(score: &[f64], point_oct: f64) -> Vec<Collision> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < score.len() {
        if score[i] < MASK_OPEN_DB {
            i += 1;
            continue;
        }
        let (mut start, mut end) = (i, i);
        while start > 0 && score[start - 1] >= MASK_CLOSE_DB {
            start -= 1;
        }
        while end + 1 < score.len() && score[end + 1] >= MASK_CLOSE_DB {
            end += 1;
        }
        let peak = (start..=end).max_by(|&a, &b| score[a].total_cmp(&score[b])).unwrap_or(i);
        if (end - start) as f64 * point_oct >= MASK_MIN_OCT {
            out.push(Collision { start, end, peak, depth_db: score[peak] });
        }
        i = end + 1;
    }
    out
}

/// The carve a collision suggests on this track: a bell at the score's centre of mass, cutting
/// half the collision's depth (1.5–6 dB), as wide as the region.
pub fn carve(c: &Collision, freqs: &[f64], score: &[f64]) -> BandSettings {
    let (mut sum, mut total) = (0.0, 0.0);
    for i in c.start..=c.end {
        let w = (score[i] - MASK_CLOSE_DB).max(0.0);
        sum += freqs[i].log2() * w;
        total += w;
    }
    let centre = if total > 0.0 { 2f64.powf(sum / total) } else { freqs[c.peak] };
    let oct = (freqs[c.end] / freqs[c.start]).log2();
    band(BandType::Bell, centre, -(0.5 * c.depth_db).clamp(1.5, 6.0), q_from_width(oct).clamp(0.7, 4.0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analyzer::{spectrum_point_freq, spectrum_point_oct, SPECTRUM_POINTS};

    fn freqs() -> Vec<f64> {
        (0..SPECTRUM_POINTS).map(spectrum_point_freq).collect()
    }

    fn summed(bands: &[BandSettings], f: &[f64]) -> Vec<f64> {
        f.iter().map(|&x| bands.iter().map(|b| analog_response_db(b, x)).sum()).collect()
    }

    fn worst(a: &[f64], b: &[f64], w: &[f64]) -> f64 {
        a.iter().zip(b).zip(w).filter(|&(_, &w)| w >= 1.0).map(|((a, b), _)| (a - b).abs()).fold(0.0, f64::max)
    }

    #[test]
    fn fit_recovers_the_bands_that_made_the_curve() {
        let f = freqs();
        let made = [band(BandType::Bell, 120.0, 4.0, 1.2), band(BandType::Bell, 2500.0, -5.0, 2.0), band(BandType::HighShelf, 9000.0, 3.0, 0.707)];
        let target = summed(&made, &f);
        let weight = match_weight(&f, &vec![0.0; f.len()], &vec![0.0; f.len()], -100.0);
        let fitted = fit(&f, &target, &weight, 6);
        assert!(fitted.len() <= 6);
        let got = summed(&fitted, &f);
        let e = worst(&got, &target, &weight);
        assert!(e < 0.5, "worst {e} dB with {} bands", fitted.len());
    }

    #[test]
    fn fit_respects_the_band_limit_and_a_flat_target_needs_none() {
        let f = freqs();
        let weight = vec![1.0; f.len()];
        assert!(fit(&f, &vec![0.0; f.len()], &weight, 8).is_empty());
        let target: Vec<f64> = f.iter().map(|x| 3.0 * (x.log2() * 1.7).sin()).collect();
        assert_eq!(fit(&f, &target, &weight, 2).len(), 2);
    }

    #[test]
    fn match_target_matches_tone_not_level() {
        let f = freqs();
        let source = vec![-40.0; f.len()];
        // The reference is 10 dB louder and has a 6 dB step above 1 kHz.
        let reference: Vec<f64> = f.iter().map(|&x| -30.0 + if x > 1000.0 { 6.0 } else { 0.0 }).collect();
        let weight = match_weight(&f, &reference, &source, -100.0);
        let t = match_target(&reference, &source, &weight, spectrum_point_oct(), 1.0 / 3.0, 1.0);
        let at = |hz: f64| t[f.iter().position(|&x| x >= hz).unwrap()];
        assert!((at(8000.0) - at(100.0) - 6.0).abs() < 0.1, "{} {}", at(100.0), at(8000.0));
        assert!(at(100.0) < 0.0 && at(8000.0) > 0.0, "the mean is removed");
        let half = match_target(&reference, &source, &weight, spectrum_point_oct(), 1.0 / 3.0, 0.5);
        assert!((half[200] - 0.5 * t[200]).abs() < 1e-9);
    }

    #[test]
    fn long_term_average_is_the_power_mean() {
        let mut s = LongTermSpectrum::new(1, f64::INFINITY);
        s.add(&[0.0], 1.0);
        s.add(&[-150.0], 1.0);
        let mut out = [0.0];
        s.read_db(&mut out);
        assert!((out[0] - (-3.0103)).abs() < 0.01, "{}", out[0]);
        let mut e = LongTermSpectrum::new(1, 1.0);
        for _ in 0..100 {
            e.add(&[-150.0], 0.1);
        }
        e.add(&[0.0], 0.1);
        e.read_db(&mut out);
        assert!((out[0] - 10.0 * 0.1f64.log10()).abs() < 0.05, "{}", out[0]);
    }

    #[test]
    fn collisions_find_where_both_peak_and_carve_there() {
        let f = freqs();
        let bump = |centre: f64, width: f64| -> Vec<f64> { f.iter().map(|&x| -40.0 + 10.0 * (-(x / centre).log2().powi(2) / width).exp()).collect() };
        // Both peak around 85 Hz; the track also peaks at 3 kHz, alone.
        let track: Vec<f64> = bump(85.0, 0.3).iter().zip(bump(3000.0, 0.2)).map(|(a, b)| a.max(b)).collect();
        let sidechain = bump(90.0, 0.4);
        let weight = match_weight(&f, &track, &sidechain, -100.0);
        let score = masking_score(&track, &sidechain, &weight, spectrum_point_oct());
        let found = collisions(&score, spectrum_point_oct());
        assert_eq!(found.len(), 1, "{found:?}");
        let c = found[0];
        assert!(f[c.start] < 85.0 && f[c.end] > 85.0, "{} – {} Hz", f[c.start], f[c.end]);
        let cut = carve(&c, &f, &score);
        assert!((cut.freq / 87.0).log2().abs() < 0.25, "{} Hz", cut.freq);
        assert!((-6.0..=-1.5).contains(&cut.gain_db), "{}", cut.gain_db);
        assert!(collisions(&masking_score(&bump(85.0, 0.3), &bump(3000.0, 0.3), &weight, spectrum_point_oct()), spectrum_point_oct()).is_empty());
        // Two broadly bright spectra (rising 4.5 dB/oct, as white noise reads) do not peak anywhere.
        let bright: Vec<f64> = f.iter().map(|&x| -60.0 + 4.5 * (x / 1000.0).log2()).collect();
        assert!(collisions(&masking_score(&bright, &bright, &weight, spectrum_point_oct()), spectrum_point_oct()).is_empty());
    }
}
