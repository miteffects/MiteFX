//! Second- and first-order sections, designed by magnitude matching so the digital response
//! follows the analog prototype up to Nyquist (no bilinear "cramping").
//!
//! Method (after M. Vicanek, "Matched Second Order Digital Filters", 2016):
//! - **Poles** come from impulse invariance of the analog poles.
//! - **Zeros** are chosen so that |H|² equals the analog |H|² at DC, at Nyquist, and at one
//!   more frequency (the band centre). With
//!   φ0 = 1 − sin²(ω/2), φ1 = sin²(ω/2), φ2 = 4·φ0·φ1,
//!   any biquad has |N(e^jω)|² = B0·φ0 + B1·φ1 + B2·φ2,
//!   with B0 = (b0+b1+b2)², B1 = (b0−b1+b2)² and B2 = −4·b0·b2.
//!
//! Everything is `f64` here: design runs at control rate, never per sample.

use std::f64::consts::PI;

/// Normalised analog prototype `(n2·s² + n1·s + n0) / (d2·s² + d1·s + d0)`, where `s` is in
/// units of the band frequency (s = jΩ/Ω0). A first-order section has `n2 = d2 = 0`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Analog {
    pub n: [f64; 3],
    pub d: [f64; 3],
}

impl Analog {
    pub const fn second(n2: f64, n1: f64, n0: f64, d2: f64, d1: f64, d0: f64) -> Self {
        Self { n: [n0, n1, n2], d: [d0, d1, d2] }
    }

    pub const fn first(n1: f64, n0: f64, d1: f64, d0: f64) -> Self {
        Self { n: [n0, n1, 0.0], d: [d0, d1, 0.0] }
    }

    pub fn is_first_order(&self) -> bool {
        self.d[2] == 0.0
    }

    /// |H(jx)|² at normalised frequency `x = Ω/Ω0`.
    pub fn magnitude_sq(&self, x: f64) -> f64 {
        let x2 = x * x;
        let poly = |c: &[f64; 3]| {
            let re = c[0] - c[2] * x2;
            let im = c[1] * x;
            re * re + im * im
        };
        let den = poly(&self.d);
        if den == 0.0 { f64::INFINITY } else { poly(&self.n) / den }
    }
}

/// Direct-form coefficients, `a0` normalised to 1.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Coeffs {
    pub b0: f64,
    pub b1: f64,
    pub b2: f64,
    pub a1: f64,
    pub a2: f64,
}

impl Coeffs {
    pub const IDENTITY: Self = Self { b0: 1.0, b1: 0.0, b2: 0.0, a1: 0.0, a2: 0.0 };

    /// |H(e^jω)|² of the digital section.
    pub fn magnitude_sq(&self, w: f64) -> f64 {
        let s = (w * 0.5).sin();
        let phi1 = s * s;
        let phi0 = 1.0 - phi1;
        let phi2 = 4.0 * phi0 * phi1;
        let num = (self.b0 + self.b1 + self.b2).powi(2) * phi0 + (self.b0 - self.b1 + self.b2).powi(2) * phi1
            - 4.0 * self.b0 * self.b2 * phi2;
        let den = (1.0 + self.a1 + self.a2).powi(2) * phi0 + (1.0 - self.a1 + self.a2).powi(2) * phi1 - 4.0 * self.a2 * phi2;
        num / den
    }
}

/// Highest digital pole frequency used for impulse invariance; above this, poles alias.
const MAX_POLE_W: f64 = 0.98 * PI;
/// The third match point stays clear of Nyquist, where it would coincide with the second.
const MAX_MATCH_W: f64 = 0.9 * PI;

/// Designs the digital section matching `proto` centred at `w0` = 2π·f0/fs (radians/sample).
/// `w0` may exceed π (a band above Nyquist); the section then tracks the in-band skirt.
pub fn matched(proto: &Analog, w0: f64) -> Coeffs {
    if proto.is_first_order() { matched_first(proto, w0) } else { matched_second(proto, w0) }
}

fn matched_first(p: &Analog, w0: f64) -> Coeffs {
    // Three unknowns (b0, b1, a1), three matches: DC, Nyquist and the corner. With
    // X = (b0+b1)², Y = (b0−b1)² the corner condition gives
    // ((1+a1)/(1−a1))² = φ1·(Hm² − Hn²) / (φ0·(Hd² − Hm²)).
    let (hd2, hn2) = (p.magnitude_sq(0.0), p.magnitude_sq(PI / w0));
    let wm = w0.min(MAX_MATCH_W);
    let hm2 = p.magnitude_sq(wm / w0);
    let s = (wm * 0.5).sin();
    let (phi1, phi0) = (s * s, 1.0 - s * s);
    let ratio = phi1 * (hm2 - hn2) / (phi0 * (hd2 - hm2));
    let a1 = if ratio.is_finite() && ratio > 0.0 {
        let k = ratio.sqrt();
        (k - 1.0) / (k + 1.0)
    } else {
        // Degenerate prototype (flat at the corner): fall back to impulse invariance.
        -(-(w0 * p.d[0] / p.d[1]).min(MAX_POLE_W)).exp()
    };
    let dc = hd2.sqrt();
    let ny = hn2.sqrt();
    // |N|² at DC = (b0+b1)² = dc²·(1+a1)²; at Nyquist (b0−b1)² = ny²·(1−a1)².
    let sum = dc * (1.0 + a1);
    let diff = ny * (1.0 - a1);
    Coeffs { b0: 0.5 * (sum + diff), b1: 0.5 * (sum - diff), b2: 0.0, a1, a2: 0.0 }
}

fn matched_second(p: &Analog, w0: f64) -> Coeffs {
    // Monic denominator s² + (ωp/Qp)·s + ωp².
    let wn = (p.d[0] / p.d[2]).sqrt();
    let qp = wn / (p.d[1] / p.d[2]);
    let wp = (w0 * wn).min(MAX_POLE_W);
    let zeta = 1.0 / (2.0 * qp);
    let decay = (-zeta * wp).exp();
    let a1 = if zeta <= 1.0 {
        -2.0 * decay * ((1.0 - zeta * zeta).sqrt() * wp).cos()
    } else {
        -2.0 * decay * ((zeta * zeta - 1.0).sqrt() * wp).cosh()
    };
    let a2 = decay * decay;

    // Pure low- and high-pass prototypes keep their exact zero structure, otherwise the deep
    // stopband loses its slope (a 24 dB/oct cut measured 44 dB over two octaves instead of 48).
    if p.n[1] == 0.0 && p.n[2] == 0.0 {
        return lowpass_zeros(p, w0, a1, a2);
    }
    if p.n[0] == 0.0 && p.n[1] == 0.0 {
        return highpass_zeros(p, w0, a1, a2);
    }
    // Notch: zeros exactly on the unit circle at the centre, so the notch is infinitely deep.
    if p.n[1] == 0.0 && p.n[0] == p.n[2] && w0 < PI {
        let g = p.magnitude_sq(0.0).sqrt() * (1.0 + a1 + a2) / (2.0 - 2.0 * w0.cos());
        return Coeffs { b0: g, b1: -2.0 * w0.cos() * g, b2: g, a1, a2 };
    }
    let with_match = |wm: f64| zeros_for(p, w0, a1, a2, wm);
    let centre = w0.min(MAX_MATCH_W);
    if w0 < HIGH_BAND_W {
        return with_match(centre);
    }
    // High bands: the single match point decides where the unavoidable error lands. Try points
    // on both skirts and keep the most accurate overall.
    let mut best = with_match(centre);
    let mut best_err = audible_error(p, &best, w0);
    for k in [0.85, 0.7, 0.55, 0.4, 1.25, 1.6, 2.0] {
        let candidate = with_match((w0 * k).min(MAX_MATCH_W));
        let err = audible_error(p, &candidate, w0);
        if err < best_err {
            best = candidate;
            best_err = err;
        }
    }
    best
}

/// Bands above this centre frequency (≈ 0.2·π, 4.4 kHz at 44.1 kHz) search their match point.
const HIGH_BAND_W: f64 = 0.2 * PI;

fn den_sq(a1: f64, a2: f64, w: f64) -> (f64, f64, f64) {
    let s = (w * 0.5).sin();
    let phi1 = s * s;
    let phi0 = 1.0 - phi1;
    let d = (1.0 + a1 + a2).powi(2) * phi0 + (1.0 - a1 + a2).powi(2) * phi1 - 4.0 * a2 * 4.0 * phi0 * phi1;
    (d, phi0, phi1)
}

/// Low-pass (Vicanek): `b2 = 0`, matched at DC and at the corner.
fn lowpass_zeros(p: &Analog, w0: f64, a1: f64, a2: f64) -> Coeffs {
    let wm = w0.min(MAX_MATCH_W);
    let (dm, phi0, phi1) = den_sq(a1, a2, wm);
    let b0s = p.magnitude_sq(0.0) * (1.0 + a1 + a2).powi(2);
    let b1s = ((p.magnitude_sq(wm / w0) * dm - b0s * phi0) / phi1).max(0.0);
    let (r0, r1) = (b0s.sqrt(), b1s.sqrt());
    let b0 = 0.5 * (r0 + r1);
    Coeffs { b0, b1: r0 - b0, b2: 0.0, a1, a2 }
}

/// High-pass (Vicanek): double zero at DC, gain matched at the corner.
fn highpass_zeros(p: &Analog, w0: f64, a1: f64, a2: f64) -> Coeffs {
    let wm = w0.min(MAX_MATCH_W);
    let (dm, _, phi1) = den_sq(a1, a2, wm);
    // |1 − z⁻¹|⁴ = (4·φ1)² on the unit circle.
    let g = (p.magnitude_sq(wm / w0) * dm).sqrt() / (4.0 * phi1);
    Coeffs { b0: g, b1: -2.0 * g, b2: g, a1, a2 }
}

/// Numerator matching |H|² at DC, Nyquist and `wm`, for the poles `a1`, `a2`.
fn zeros_for(p: &Analog, w0: f64, a1: f64, a2: f64, wm: f64) -> Coeffs {
    let a0s = (1.0 + a1 + a2).powi(2);
    let a1s = (1.0 - a1 + a2).powi(2);
    let a2s = -4.0 * a2;
    let s = (wm * 0.5).sin();
    let phi1 = s * s;
    let phi0 = 1.0 - phi1;
    let phi2 = 4.0 * phi0 * phi1;
    let dm = a0s * phi0 + a1s * phi1 + a2s * phi2;

    let b0s = p.magnitude_sq(0.0) * a0s;
    let b1s = p.magnitude_sq(PI / w0) * a1s;
    let b2s = (p.magnitude_sq(wm / w0) * dm - b0s * phi0 - b1s * phi1) / phi2;

    let (r0, r1) = (b0s.max(0.0).sqrt(), b1s.max(0.0).sqrt());
    let w = 0.5 * (r0 + r1);
    let b1 = 0.5 * (r0 - r1);
    // b0 and b2 are the roots of x² − W·x − B2/4; a negative discriminant means the match is not
    // exactly realisable, so take the closest real numerator.
    let disc = (w * w + b2s).max(0.0).sqrt();
    let b0 = 0.5 * (w + disc);
    let b2 = 0.5 * (w - disc);
    Coeffs { b0, b1, b2, a1, a2 }
}

/// Worst |dB| deviation from the prototype on a coarse log grid up to 0.95·π, counting only
/// where the prototype is within 24 dB of unity (the audible part of the shape).
fn audible_error(p: &Analog, c: &Coeffs, w0: f64) -> f64 {
    let lo = (w0 / 16.0).max(1e-4);
    let hi = 0.95 * PI;
    let mut worst: f64 = 0.0;
    for i in 0..=24 {
        let w = lo * (hi / lo).powf(f64::from(i) / 24.0);
        let target = p.magnitude_sq(w / w0);
        if target > 0.004 {
            let err = (10.0 * (c.magnitude_sq(w) / target).log10()).abs();
            worst = worst.max(err);
        }
    }
    worst
}

impl Coeffs {
    /// The inverse section 1/H. Exact for minimum-phase sections, which matched boosts are: the
    /// numerator is factored with |b2| ≤ b0, keeping its zeros inside the unit circle.
    pub fn inverse(&self) -> Self {
        let g = self.b0;
        Self { b0: 1.0 / g, b1: self.a1 / g, b2: self.a2 / g, a1: self.b1 / g, a2: self.b2 / g }
    }
}

impl Analog {
    /// 1/H: swaps numerator and denominator.
    pub fn inverse(&self) -> Self {
        Self { n: self.d, d: self.n }
    }
}

/// Designs a section whose gain may be a cut: cuts (|H| < 1 at the match point) are designed as
/// the matched *boost* and inverted, so boost and cut are exact mirror images as in analog EQs
/// and the cut never hits an unrealisable match.
pub fn matched_symmetric(proto: &Analog, w0: f64, is_cut: bool) -> Coeffs {
    if is_cut { matched(&proto.inverse(), w0).inverse() } else { matched(proto, w0) }
}

/// Transposed direct form II state for one channel of one section.
#[derive(Debug, Clone, Copy, Default)]
pub struct Tdf2 {
    s1: f64,
    s2: f64,
}

impl Tdf2 {
    #[inline]
    pub fn process(&mut self, c: &Coeffs, x: f64) -> f64 {
        let y = c.b0 * x + self.s1;
        self.s1 = c.b1 * x - c.a1 * y + self.s2;
        self.s2 = c.b2 * x - c.a2 * y;
        y
    }

    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db(x: f64) -> f64 {
        10.0 * x.log10()
    }

    /// Worst |digital − analog| in dB over 20 Hz … `top` Hz, counted only where the analog
    /// response is within 24 dB of 0 dB (deep stopband dB differences are inaudible).
    fn worst_error(proto: &Analog, f0: f64, fs: f64, top: f64, cut: bool) -> f64 {
        let w0 = 2.0 * PI * f0 / fs;
        let c = matched_symmetric(proto, w0, cut);
        let mut worst: f64 = 0.0;
        let mut f = 20.0;
        while f <= top {
            let w = 2.0 * PI * f / fs;
            let analog = db(proto.magnitude_sq(f / f0));
            if analog > -24.0 {
                worst = worst.max((db(c.magnitude_sq(w)) - analog).abs());
            }
            f *= 1.01;
        }
        worst
    }

    fn bell(gain_db: f64, q: f64) -> Analog {
        let a = 10f64.powf(gain_db / 40.0);
        Analog::second(1.0, a / q, 1.0, 1.0, 1.0 / (a * q), 1.0)
    }

    #[test]
    fn low_and_mid_bells_match_analog_within_a_tenth_of_a_db() {
        for (f0, g, q) in [(60.0, 12.0, 1.0), (1000.0, -12.0, 0.5), (3000.0, 9.0, 4.0), (3000.0, -9.0, 4.0)] {
            let err = worst_error(&bell(g, q), f0, 44_100.0, 20_000.0, g < 0.0);
            assert!(err < 0.1, "bell {f0} Hz {g} dB Q{q}: {err:.3} dB");
        }
    }

    #[test]
    fn high_bells_stay_close_where_bilinear_cramps() {
        // A bilinear (RBJ) bell here loses several dB of width and height. Matched second-order
        // sections stay within ~1.1 dB, the error pushed towards 20 kHz. The 0.1 dB target near
        // Nyquist needs oversampling or a higher-order correction (BLUEPRINT §9, Phase 1 open).
        for (f0, g, q, limit) in [
            (5000.0, -6.0, 0.7, 0.3),
            (8000.0, -9.0, 2.0, 1.2),
            (12000.0, 12.0, 1.0, 1.2),
            (15000.0, 6.0, 0.7, 1.2),
            (18000.0, -12.0, 1.5, 1.2),
        ] {
            let err = worst_error(&bell(g, q), f0, 44_100.0, 20_000.0, g < 0.0);
            assert!(err < limit, "bell {f0} Hz {g} dB Q{q}: {err:.3} dB");
        }
    }

    #[test]
    fn boost_and_cut_are_mirror_images() {
        let fs = 48_000.0;
        let w0 = 2.0 * PI * 2500.0 / fs;
        let boost = matched_symmetric(&bell(8.0, 1.2), w0, false);
        let cut = matched_symmetric(&bell(-8.0, 1.2), w0, true);
        for f in [50.0, 500.0, 2500.0, 9000.0, 20000.0] {
            let w = 2.0 * PI * f / fs;
            let sum = db(boost.magnitude_sq(w)) + db(cut.magnitude_sq(w));
            assert!(sum.abs() < 1e-9, "{f} Hz: {sum}");
        }
    }

    #[test]
    fn bell_peak_gain_is_exact_in_band() {
        let fs = 48_000.0;
        let c = matched(&bell(9.0, 1.0), 2.0 * PI * 1000.0 / fs);
        let peak = db(c.magnitude_sq(2.0 * PI * 1000.0 / fs));
        assert!((peak - 9.0).abs() < 1e-6, "{peak}");
    }

    #[test]
    fn butterworth_lowpass_and_highpass_match() {
        let q = std::f64::consts::FRAC_1_SQRT_2;
        let lp = Analog::second(0.0, 0.0, 1.0, 1.0, 1.0 / q, 1.0);
        let hp = Analog::second(1.0, 0.0, 0.0, 1.0, 1.0 / q, 1.0);
        // Same Nyquist limit as the high bells for corners above ~0.3·π.
        for (f0, limit) in [(100.0, 0.1), (2000.0, 0.1), (12000.0, 1.0)] {
            assert!(worst_error(&lp, f0, 48_000.0, 20_000.0, false) < limit, "lp {f0}");
            assert!(worst_error(&hp, f0, 48_000.0, 20_000.0, false) < limit, "hp {f0}");
        }
    }

    #[test]
    fn first_order_sections_match() {
        let lp1 = Analog::first(0.0, 1.0, 1.0, 1.0);
        let hp1 = Analog::first(1.0, 0.0, 1.0, 1.0);
        // A first-order digital section must reach a finite value at Nyquist while the analog one
        // keeps falling, so high corners deviate where the slope is already ~20 dB down.
        for (f0, limit) in [(50.0, 0.1), (1000.0, 1.0), (10000.0, 1.0)] {
            assert!(worst_error(&lp1, f0, 48_000.0, 20_000.0, false) < limit, "lp1 {f0}");
            assert!(worst_error(&hp1, f0, 48_000.0, 20_000.0, false) < limit, "hp1 {f0}");
        }
    }

    #[test]
    fn tdf2_impulse_matches_coefficients() {
        let c = Coeffs { b0: 0.5, b1: 0.2, b2: 0.1, a1: -0.3, a2: 0.05 };
        let mut s = Tdf2::default();
        let y: Vec<f64> = (0..4).map(|n| s.process(&c, if n == 0 { 1.0 } else { 0.0 })).collect();
        let expected0 = 0.5;
        let expected1 = 0.2 - (-0.3) * expected0;
        let expected2 = 0.1 - (-0.3) * expected1 - 0.05 * expected0;
        assert!((y[0] - expected0).abs() < 1e-12);
        assert!((y[1] - expected1).abs() < 1e-12);
        assert!((y[2] - expected2).abs() < 1e-12);
    }
}


