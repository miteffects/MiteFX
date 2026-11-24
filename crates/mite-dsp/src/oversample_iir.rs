//! Minimum-phase oversampling: polyphase half-bands made of two first-order allpass chains
//! (Laurent de Soras' HIIR design: an elliptic half-band split into its two polyphase allpass
//! paths). Far less delay than the linear-phase FIR cascade in [`crate::oversample`]
//! (a few samples at the base rate against 69–79), at the cost of phase shift near the top of
//! the band. Used where latency matters more than phase (Mite Sat's low-latency mode).
//!
//! A 2× half-band is `H(z) = ½·(A₀(z²) + z⁻¹·A₁(z²))`; decimating, path 0 takes the newer sample
//! of each pair and path 1 the older; interpolating, path 0 gives the even output and path 1 the
//! odd. Each path is a cascade of `(c + z⁻¹)/(1 + c·z⁻¹)` at the lower rate. Allocation-free.

/// The coefficients of a half-band with `n` allpass sections and normalized transition band
/// `transition` (0…½ of the high rate), alternating between the two paths (index 0 to path 0).
pub fn design(n: usize, transition: f64) -> Vec<f64> {
    let pi = std::f64::consts::PI;
    let mut k = ((1.0 - transition * 2.0) * pi / 4.0).tan();
    k *= k;
    let kk = (1.0 - k * k).powf(0.25);
    let e = 0.5 * (1.0 - kk) / (1.0 + kk);
    let e4 = e.powi(4);
    let q = e * (1.0 + e4 * (2.0 + e4 * (15.0 + 150.0 * e4)));
    let order = (2 * n + 1) as f64;
    (0..n)
        .map(|index| {
            let c = (index + 1) as f64;
            let (mut num, mut i, mut sign) = (0.0, 0i32, 1.0);
            loop {
                let t = q.powi(i * (i + 1)) * ((2 * i + 1) as f64 * c * pi / order).sin() * sign;
                num += t;
                sign = -sign;
                i += 1;
                if t.abs() < 1e-100 || i > 64 {
                    break;
                }
            }
            let (mut den, mut i, mut sign) = (0.0, 1i32, -1.0);
            loop {
                let t = q.powi(i * i) * ((2 * i) as f64 * c * pi / order).cos() * sign;
                den += t;
                sign = -sign;
                i += 1;
                if t.abs() < 1e-100 || i > 64 {
                    break;
                }
            }
            let ww = num * q.powf(0.25) / (den + 0.5);
            let w2 = ww * ww;
            let x = ((1.0 - w2 * k) * (1.0 - w2 / k)).sqrt() / (1.0 + w2);
            (1.0 - x) / (1.0 + x)
        })
        .collect()
}

/// The most allpass sections a stage has.
pub const MAX_SECTIONS: usize = 12;

/// One allpass path: its coefficients and each section's last input and output.
#[derive(Clone, Copy, Debug, Default)]
struct Path {
    n: usize,
    c: [f32; MAX_SECTIONS / 2],
    x1: [f32; MAX_SECTIONS / 2],
    y1: [f32; MAX_SECTIONS / 2],
}

impl Path {
    #[inline]
    fn tick(&mut self, mut x: f32) -> f32 {
        for s in 0..self.n {
            let y = self.c[s] * (x - self.y1[s]) + self.x1[s];
            self.x1[s] = x;
            self.y1[s] = y;
            x = y;
        }
        x
    }

    fn clear(&mut self) {
        self.x1 = [0.0; MAX_SECTIONS / 2];
        self.y1 = [0.0; MAX_SECTIONS / 2];
    }
}

/// One 2× stage for one channel: an interpolator and a decimator with the same coefficients.
#[derive(Clone, Copy, Debug, Default)]
pub struct IirStage {
    up: [Path; 2],
    down: [Path; 2],
}

impl IirStage {
    pub fn new(coefs: &[f64]) -> Self {
        let mut p = [Path::default(); 2];
        for (i, &c) in coefs.iter().enumerate().take(MAX_SECTIONS) {
            let path = &mut p[i % 2];
            path.c[path.n] = c as f32;
            path.n += 1;
        }
        Self { up: p, down: p }
    }

    /// One low-rate sample in, two high-rate samples out (even, odd).
    #[inline]
    pub fn interpolate(&mut self, x: f32) -> [f32; 2] {
        [self.up[0].tick(x), self.up[1].tick(x)]
    }

    /// Two high-rate samples in (older, newer), one low-rate sample out.
    #[inline]
    pub fn decimate(&mut self, x: [f32; 2]) -> f32 {
        0.5 * (self.down[0].tick(x[1]) + self.down[1].tick(x[0]))
    }

    pub fn clear(&mut self) {
        for p in self.up.iter_mut().chain(self.down.iter_mut()) {
            p.clear();
        }
    }
}

/// Each stage's sections and transition band, every one rejecting what would fold back by 100 dB
/// or more and flat over the band: the first carries the steep transition (it passes up to
/// 0.9 × the base Nyquist, −110 dB from its mirror); the later ones only clear the images of the
/// stage before (the band is under 0.1125, 0.056, 0.028 of their rates), so they are short.
pub const STAGES: [(usize, f64); 4] = [(10, 0.0275), (6, 0.13), (5, 0.19), (4, 0.215)];

#[cfg(test)]
mod tests {
    use super::*;

    /// A stage's decimator response at `f` (fraction of the high rate), dB, from its impulse
    /// response.
    fn response_db(st: &mut IirStage, f: f64) -> f64 {
        let n = 1 << 14;
        st.clear();
        let mut h = Vec::with_capacity(n);
        for i in 0..n {
            // Feed (x0, x1) pairs of an impulse at high-rate sample 1, read the output stream.
            let pair = [if 2 * i == 1 { 1.0 } else { 0.0 }, if 2 * i + 1 == 1 { 1.0 } else { 0.0 }];
            h.push(f64::from(st.decimate(pair)));
        }
        // The decimator's output is H's even samples after the pair: its spectrum at the high
        // rate is read on the full impulse response instead, from the interpolator.
        let _ = h;
        st.clear();
        let mut full = Vec::with_capacity(2 * n);
        for i in 0..n {
            let [a, b] = st.interpolate(if i == 0 { 1.0 } else { 0.0 });
            full.push(f64::from(a));
            full.push(f64::from(b));
        }
        let w = std::f64::consts::TAU * f;
        let (re, im) = full.iter().enumerate().fold((0.0, 0.0), |(re, im), (k, v)| (re + v * (w * k as f64).cos(), im - v * (w * k as f64).sin()));
        // The interpolator's gain is 2 (it fills the zeros); halve to the half-band's.
        20.0 * (0.5 * (re * re + im * im).sqrt()).log10()
    }

    #[test]
    fn the_first_stage_passes_the_band_and_rejects_the_images() {
        let c = design(STAGES[0].0, STAGES[0].1);
        let mut st = IirStage::new(&c);
        // At the 2× rate: the base band reaches 0.25; 0.9 of it passes, its mirror is rejected.
        let mut worst_pass: f64 = 0.0;
        let mut f = 0.001;
        while f < 0.225 {
            worst_pass = worst_pass.max(response_db(&mut st, f).abs());
            f += 0.004;
        }
        let mut worst_stop = f64::MIN;
        let mut f = 0.275;
        while f < 0.5 {
            worst_stop = worst_stop.max(response_db(&mut st, f));
            f += 0.004;
        }
        println!("first stage: passband within {worst_pass:.4} dB, stopband {worst_stop:.1} dB");
        assert!(worst_pass < 0.01, "{worst_pass}");
        assert!(worst_stop < -100.0, "{worst_stop}");
    }
}
