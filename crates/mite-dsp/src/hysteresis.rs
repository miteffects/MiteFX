//! Magnetic hysteresis for Mite Sat's Tape and Transformer (sat-brief §1.2): a Jiles–Atherton
//! core in normalised units, `h = H/a` and `m = M/Mₛ`:
//!
//! ```text
//! Q   = h + A·m                 m_an = L(Q) = coth Q − 1/Q   (Langevin)
//! dm/dh = [ (1−c)·δₘ·(m_an − m) / ((1−c)·δ·K − A·(m_an − m)) + c·L'(Q) ] / (1 − c·A·L'(Q))
//! ```
//!
//! `δ` is the sign of `dh`, `δₘ` drops the irreversible term where it would pull the wrong way
//! (non-physical negative susceptibility), `A` couples the domains, `K` pins them (the loop's
//! width) and `c` is the reversible share.
//!
//! The irreversible term relaxes `m` towards `m_an` over a field distance of about `K`: stiff,
//! so an explicit solver blows up once a sample moves the field further than that (a high tone,
//! hard driven). Each substep is instead solved exactly with its coefficients held and the target
//! moving linearly (an exponential integrator: unconditionally stable, it can only relax), once
//! with the coefficients at the start and again with those averaged with the predicted end's
//! (predictor-corrector, second order). Substeps are evenly spaced in `asinh(h / h_s)`: fine where
//! the curve bends, near 0, coarse where it is flat. A quiet or slow signal takes one. Against a
//! finely solved reference, the settled loop is within 0.005 for swings to ±400 at 12 samples a
//! cycle. The model is rate independent: the loop does not depend on
//! the frequency, only on how far the field swings. `dm/dt = dm/dh · dh/dt` is continuous
//! where the field turns (it is 0 there), so the output has no corners and oversampling does the
//! anti-aliasing as for the static curves.
//!
//! A high reversible share keeps quiet signals clean: an unbiased core's gain grows with level at
//! the bottom of the loop (the Rayleigh region), which tape machines hide with AC bias. At
//! `c ≥ 0.82` the gain stays within about 1 dB below the knee and the third harmonic of a
//! −26 dB sine stays under −45 dB, while a driven core still lags by a few degrees.

use std::sync::OnceLock;

/// One core's constants (normalised).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct JaParams {
    /// Domain coupling `A = α·Mₛ/a`.
    pub coupling: f32,
    /// Pinning `K = k/a`: the loop's half width in `h`.
    pub pinning: f32,
    /// The reversible share `c`.
    pub reversible: f32,
    /// The field one unit of the band's driven signal makes.
    pub h_in: f32,
}

/// Tape: a wide loop, soft and loose.
pub const TAPE: JaParams = JaParams { coupling: 0.1, pinning: 0.3, reversible: 0.82, h_in: 2.5 };
/// Transformer: a narrow loop on a stiffer core.
pub const TRANSFORMER: JaParams = JaParams { coupling: 0.1, pinning: 0.12, reversible: 0.9, h_in: 2.0 };

/// Substeps: at most this far apart in `w = asinh(h / H_S)`, and at most this many a sample.
const MAX_DW: f32 = 0.5;
const H_S: f32 = 1.0;
const MAX_SUBSTEPS: usize = 64;

#[inline]
pub fn langevin(x: f32) -> f32 {
    langevin_both(x).0
}

/// `L(x)` and `L'(x)` from one exponential: with `t = e^(−2|x|)`, `coth|x| = (1 + t)/(1 − t)` and
/// `1/sinh²x = 4t/(1 − t)²`.
#[inline]
fn langevin_both(x: f32) -> (f32, f32) {
    let a = x.abs();
    if a < 0.3 {
        // The series (both differences cancel badly in f32 near 0); next terms under 1e-5.
        let x2 = x * x;
        return (x * (1.0 / 3.0 - x2 / 45.0 + 2.0 * x2 * x2 / 945.0), 1.0 / 3.0 - x2 / 15.0 + 2.0 * x2 * x2 / 189.0);
    }
    let t = (-2.0 * a).exp();
    let r = 1.0 / (1.0 - t);
    let inv = 1.0 / a;
    let l = (1.0 + t) * r - inv;
    (l.copysign(x), inv * inv - 4.0 * t * r * r)
}

/// The anhysteretic magnetisation at field `h`: `m = L(h + A·m)`, solved by fixed point (it
/// contracts: `A·L' ≤ A/3`).
pub fn anhysteretic(p: &JaParams, h: f32) -> f32 {
    let mut m = langevin(h);
    for _ in 0..6 {
        m = langevin(h + p.coupling * m);
    }
    m
}

/// A core's state: the field it was last driven to and its magnetisation.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Ja {
    pub h: f32,
    pub m: f32,
}

impl Ja {
    /// At rest at field `h` (on the anhysteretic curve, as after a long fade).
    pub fn rest(p: &JaParams, h: f32) -> Self {
        Self { h, m: anhysteretic(p, h) }
    }

    /// The equation's parts at `(h, m)` moving in direction `d`: the irreversible relaxation rate
    /// `κ` (0 where `δₘ` drops it), the target `m_an`, the reversible term `c·L'` and the
    /// denominator `1 − c·A·L'`.
    #[inline]
    fn parts(p: &JaParams, h: f32, m: f32, d: f32) -> (f32, f32, f32, f32) {
        let (l, lp) = langevin_both(h + p.coupling * m);
        let diff = l - m;
        let c = p.reversible;
        let kappa = if d * diff > 0.0 {
            // The denominator keeps the sign of δ (never through zero).
            let floor = 0.2 * (1.0 - c) * p.pinning;
            let den = (1.0 - c) * d * p.pinning - p.coupling * diff;
            let den = if d * den < floor { d * floor } else { den };
            (1.0 - c) / den
        } else {
            0.0
        };
        (kappa, l, c * lp, 1.0 - c * p.coupling * lp)
    }

    /// `m₁` for `dm/dh = a·(T − m) + b` with `a`, `b` held and `T` moving from `t0` at slope `s`,
    /// solved exactly: `m₁ = m + s·dh + (T₀ − m)(1 − e^(−a·dh)) + (b − s)(1 − e^(−a·dh))/a`.
    #[inline]
    fn relax(m: f32, t0: f32, s: f32, a: f32, b: f32, dh: f32) -> f32 {
        if a == 0.0 {
            return (m + b * dh).clamp(-1.0, 1.0);
        }
        let ad = a * dh;
        let (one, phi) = if ad.abs() < 1e-3 {
            let k = 1.0 - ad / 2.0 + ad * ad / 6.0;
            (ad * k, dh * k)
        } else {
            let one = -(-ad).exp_m1();
            (one, one / a)
        };
        (m + s * dh + (t0 - m) * one + (b - s) * phi).clamp(-1.0, 1.0)
    }

    /// One substep from `(h, m)` by `dh`: predicted with the coefficients at the start, then
    /// corrected with them averaged with the predicted end's.
    #[inline]
    fn substep(p: &JaParams, h: f32, m: f32, dh: f32, d: f32) -> f32 {
        let (k0, t0, r0, den0) = Self::parts(p, h, m, d);
        let (a0, b0) = (k0 / den0, r0 / den0);
        let s0 = (langevin(h + dh + p.coupling * m) - t0) / dh;
        let predicted = Self::relax(m, t0, s0, a0, b0, dh);
        let (k1, t1, r1, den1) = Self::parts(p, h + dh, predicted, d);
        let s1 = (t1 - t0) / dh;
        Self::relax(m, t0, 0.5 * (s0 + s1), 0.5 * (a0 + k1 / den1), 0.5 * (b0 + r1 / den1), dh)
    }

    /// Drives the core to field `h` and returns its magnetisation.
    #[inline]
    pub fn step(&mut self, p: &JaParams, h: f32) -> f32 {
        let total = h - self.h;
        if total == 0.0 {
            return self.m;
        }
        if !total.is_finite() || !self.m.is_finite() {
            *self = Self::rest(p, if h.is_finite() { h } else { 0.0 });
            return self.m;
        }
        let d = if total > 0.0 { 1.0 } else { -1.0 };
        // `|Δw| ≤ |Δh| / H_S`: a short step needs no `asinh` to know it is one substep.
        let m = if total.abs() <= MAX_DW * H_S {
            Self::substep(p, self.h, self.m, total, d)
        } else {
            let (w0, w1) = ((self.h / H_S).asinh(), (h / H_S).asinh());
            let n = (((w1 - w0).abs() / MAX_DW).ceil() as usize).clamp(1, MAX_SUBSTEPS);
            let (mut at, mut m) = (self.h, self.m);
            for j in 1..=n {
                let next = if j == n { h } else { H_S * (w0 + (w1 - w0) * j as f32 / n as f32).sinh() };
                m = Self::substep(p, at, m, next - at, d);
                at = next;
            }
            m
        };
        self.h = h;
        self.m = m;
        m
    }
}

/// The output scale that gives a core slope 1 for a quiet sine (its gain at 2 % of full field
/// swing, measured once per core): what makes `x + a·(F − x)` blend without a level step.
pub fn unity(p: &JaParams) -> f32 {
    static TAPE_UNITY: OnceLock<f32> = OnceLock::new();
    static TRANSFORMER_UNITY: OnceLock<f32> = OnceLock::new();
    let measure = || {
        let (n, amp) = (256usize, 0.02f32);
        let mut core = Ja::rest(p, 0.0);
        let mut c = 0.0f64;
        for i in 0..4 * n {
            let s = (std::f64::consts::TAU * i as f64 / n as f64).sin();
            let m = core.step(p, p.h_in * amp * s as f32);
            if i >= 3 * n {
                c += f64::from(m) * s;
            }
        }
        (1.0 / (c / n as f64 / (f64::from(amp) / 2.0))) as f32
    };
    if *p == TAPE {
        *TAPE_UNITY.get_or_init(measure)
    } else if *p == TRANSFORMER {
        *TRANSFORMER_UNITY.get_or_init(measure)
    } else {
        measure()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A sine of amplitude `amp` (in driven units) through a core, after three settling cycles:
    /// (gain of the fundamental, its phase in degrees, third harmonic re the fundamental in dB).
    fn sine(p: &JaParams, amp: f32) -> (f64, f64, f64) {
        let n = 512usize;
        let mut core = Ja::rest(p, 0.0);
        let (mut re1, mut im1, mut re3, mut im3) = (0.0f64, 0.0f64, 0.0f64, 0.0f64);
        for i in 0..4 * n {
            let th = std::f64::consts::TAU * i as f64 / n as f64;
            let m = f64::from(core.step(p, p.h_in * amp * th.sin() as f32));
            if i >= 3 * n {
                re1 += m * th.sin();
                im1 += m * th.cos();
                re3 += m * (3.0 * th).sin();
                im3 += m * (3.0 * th).cos();
            }
        }
        let k = 2.0 / n as f64 / f64::from(amp) * f64::from(unity(p));
        let (g1, g3) = (re1.hypot(im1) * k, re3.hypot(im3) * k);
        (g1, im1.atan2(re1).to_degrees(), 20.0 * (g3 / g1).log10())
    }

    #[test]
    fn quiet_signals_pass_clean_at_unity() {
        for p in [TAPE, TRANSFORMER] {
            let (g, _, h3) = sine(&p, 0.02);
            assert!((g - 1.0).abs() < 1e-3, "{p:?}: {g}");
            // Below the knee the gain stays within 1.5 dB and the third harmonic low.
            for amp in [0.005f32, 0.05, 0.2] {
                let (g, _, h3) = sine(&p, amp);
                assert!((20.0 * g.log10()).abs() < 1.5, "{p:?} at {amp}: {g}");
                if amp <= 0.05 {
                    assert!(h3 < -40.0, "{p:?} at {amp}: H3 {h3:.1} dB");
                }
            }
            assert!(h3 < -45.0, "{p:?}: H3 {h3:.1} dB");
        }
    }

    #[test]
    fn a_driven_core_lags_and_saturates() {
        for p in [TAPE, TRANSFORMER] {
            let (g_mid, phase, _) = sine(&p, 0.3);
            assert!(phase < -0.4 && phase > -15.0, "{p:?}: phase {phase:.2}°");
            let (g_hot, _, h3) = sine(&p, 3.0);
            assert!(g_hot < 0.7 * g_mid, "{p:?}: {g_mid} → {g_hot}");
            assert!(h3 > -30.0, "{p:?}: H3 {h3:.1} dB");
        }
        // Tape's loop is the wider.
        assert!(sine(&TAPE, 0.3).1 < sine(&TRANSFORMER, 0.3).1);
    }

    #[test]
    fn the_magnetisation_stays_bounded_and_recovers_from_nan() {
        let p = TAPE;
        let mut core = Ja::rest(&p, 0.0);
        // Huge jumps a sample apart (36 dB of drive at 1× on a high tone).
        for i in 0..10_000 {
            let h = if i % 2 == 0 { 400.0 } else { -400.0 };
            let m = core.step(&p, h);
            assert!(m.is_finite() && m.abs() <= 1.0);
        }
        core.step(&p, f32::NAN);
        assert!(core.step(&p, 0.1).is_finite());
    }

    #[test]
    fn langevin_matches_its_closed_form() {
        let mut worst = (0.0f64, 0.0f64);
        let mut x = -40.0f64;
        while x < 40.0 {
            let (l, d) = langevin_both(x as f32);
            let (wl, wd) = if x.abs() < 1e-3 { (x / 3.0, 1.0 / 3.0) } else { (1.0 / x.tanh() - 1.0 / x, 1.0 / (x * x) - 1.0 / x.sinh().powi(2)) };
            worst = (worst.0.max((f64::from(l) - wl).abs()), worst.1.max((f64::from(d) - wd).abs()));
            x += 0.000_731;
        }
        assert!(worst.0 < 1e-5 && worst.1 < 1e-5, "{worst:?}");
    }

    #[test]
    fn big_steps_stay_on_the_loop() {
        // Sines swinging the field to ±4, ±40 and ±400 in 12 and 48 samples a cycle (a high
        // tone, hard driven, without oversampling) against the same swing in 6144 samples: the
        // magnetisation within 0.005 everywhere on the settled cycle.
        for p in [TAPE, TRANSFORMER] {
            for amp in [4.0f64, 40.0, 400.0] {
                let run = |n: usize| -> Vec<f32> {
                    let mut core = Ja::rest(&p, 0.0);
                    (0..3 * n).map(|i| core.step(&p, (amp * (std::f64::consts::TAU * i as f64 / n as f64).sin()) as f32)).skip(2 * n).collect()
                };
                let fine = run(6144);
                for n in [12usize, 48] {
                    let coarse = run(n);
                    let worst = coarse.iter().enumerate().map(|(i, m)| (m - fine[i * (6144 / n)]).abs()).fold(0.0f32, f32::max);
                    assert!(worst < 0.005, "{p:?} amp {amp}, {n} a cycle: {worst}");
                }
            }
        }
    }

    #[test]
    fn the_core_is_rate_independent() {
        // The same swing in 64 or 1024 steps per cycle lands on the same loop.
        let p = TRANSFORMER;
        let at = |n: usize| {
            let mut core = Ja::rest(&p, 0.0);
            let mut peak = 0.0f32;
            for i in 0..3 * n {
                let m = core.step(&p, 2.0 * (std::f64::consts::TAU * i as f64 / n as f64).sin() as f32);
                if i >= 2 * n && i % (n / 4) == 0 {
                    peak = peak.max(m.abs());
                }
            }
            peak
        };
        assert!((at(64) - at(1024)).abs() < 0.01, "{} {}", at(64), at(1024));
    }
}
