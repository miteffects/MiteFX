//! Dither to a word length, with noise shaping: the last stage of a master (Mite Limit, brief
//! §1.7).
//!
//! TPDF dither (two uniform draws, ±1 LSB, triangular), then rounding to the word length, so the
//! float output is already the file's values and the host's conversion is exact. Noise shaping
//! feeds the total error back through a fixed FIR, `y = Q(x − Σ hₖ eₙ₋ₖ + d)`, so the error
//! spectrum is `E(z) (1 − H(z))`: the same noise, moved up where the ear is least sensitive.
//!
//! - **Light:** `1 − z⁻¹`, first order: +6 dB per octave, flat in total power ×2.
//! - **Strong:** Lipshitz, Vanderkooy and Wannamaker's E-weighted 5-tap filter (1991), designed at
//!   44.1 kHz: the error down by 18 dB across 2–5 kHz (light: 6 dB), where hearing is keenest, and up
//!   above 12 kHz. At higher rates the same filter moves the noise further up still.
//!
//! The error and the dither's generator are per channel and deterministic (seeded), so a render
//! repeats bit for bit. Nothing here allocates.

/// The word lengths, in the parameter's frozen order (0: off).
pub const BITS: [u32; 4] = [0, 24, 20, 16];

/// The noise shaping, in the parameter's frozen order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shaping {
    Off,
    Light,
    Strong,
}

impl Shaping {
    pub fn from_index(i: usize) -> Self {
        match i {
            1 => Self::Light,
            2 => Self::Strong,
            _ => Self::Off,
        }
    }

    fn taps(self) -> &'static [f32] {
        match self {
            Self::Off => &[],
            Self::Light => &[1.0],
            Self::Strong => &[2.033, -2.165, 1.959, -1.590, 0.6149],
        }
    }
}

#[derive(Clone, Debug)]
pub struct Dither {
    bits: u32,
    shaping: Shaping,
    /// The last errors, newest first, per channel.
    err: [[f32; 5]; 2],
    rng: [u64; 2],
}

impl Default for Dither {
    fn default() -> Self {
        Self::new()
    }
}

impl Dither {
    pub fn new() -> Self {
        Self { bits: 0, shaping: Shaping::Off, err: [[0.0; 5]; 2], rng: [0x9E37_79B9_7F4A_7C15, 0xD1B5_4A32_D192_ED03] }
    }

    /// The word length (0: off) and the shaping; the error is cleared when either changes.
    pub fn set(&mut self, bits: u32, shaping: Shaping) {
        if (bits, shaping) != (self.bits, self.shaping) {
            self.bits = bits;
            self.shaping = shaping;
            self.err = [[0.0; 5]; 2];
        }
    }

    pub fn is_on(&self) -> bool {
        self.bits > 0
    }

    /// One LSB at the word length (full scale ±1).
    pub fn lsb(bits: u32) -> f32 {
        (0.5f32).powi(bits.max(1) as i32 - 1)
    }

    #[inline]
    fn uniform(&mut self, c: usize) -> f32 {
        let r = &mut self.rng[c];
        *r ^= *r << 13;
        *r ^= *r >> 7;
        *r ^= *r << 17;
        (*r >> 40) as f32 / (1u64 << 24) as f32
    }

    /// One sample of channel `c` (0 or 1).
    #[inline]
    pub fn process(&mut self, c: usize, x: f32) -> f32 {
        if self.bits == 0 {
            return x;
        }
        let q = Self::lsb(self.bits);
        let taps = self.shaping.taps();
        let fb: f32 = taps.iter().zip(&self.err[c]).map(|(h, e)| h * e).sum();
        let v = x - fb;
        let d = (self.uniform(c) - self.uniform(c)) * q;
        let y = ((v + d) / q).round() * q;
        let e = &mut self.err[c];
        e.copy_within(0..4, 1);
        // Held to a few LSB: a clip upstream must not wind the loop up.
        e[0] = (y - v).clamp(-4.0 * q, 4.0 * q);
        y
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn error_of(bits: u32, shaping: Shaping, n: usize) -> Vec<f64> {
        let mut d = Dither::new();
        d.set(bits, shaping);
        (0..n)
            .map(|i| {
                let x = 0.3 * (i as f32 * 0.0123).sin();
                f64::from(d.process(0, x) - x)
            })
            .collect()
    }

    /// Power of `e` in the band `lo…hi` (share of the rate), by a plain DFT over bins.
    fn band_power(e: &[f64], lo: f64, hi: f64) -> f64 {
        let n = 4096;
        let mut total = 0.0;
        let frames = e.len() / n;
        for f in 0..frames {
            let s = &e[f * n..(f + 1) * n];
            for k in (lo * n as f64) as usize..(hi * n as f64) as usize {
                let w = std::f64::consts::TAU * k as f64 / n as f64;
                let (mut re, mut im) = (0.0, 0.0);
                for (i, v) in s.iter().enumerate() {
                    re += v * (w * i as f64).cos();
                    im -= v * (w * i as f64).sin();
                }
                total += (re * re + im * im) / (n * n) as f64;
            }
        }
        total / frames as f64
    }

    #[test]
    fn off_is_the_input_and_on_lands_on_the_word_length() {
        let mut d = Dither::new();
        assert_eq!(d.process(0, 0.123_456_7), 0.123_456_7);
        for bits in [16, 20, 24] {
            d.set(bits, Shaping::Strong);
            let q = Dither::lsb(bits);
            for i in 0..2000 {
                let y = d.process(1, 0.7 * (i as f32 * 0.01).sin());
                assert_eq!((y / q).round() * q, y, "{bits} bit");
            }
        }
    }

    #[test]
    fn tpdf_at_16_bit_adds_the_noise_it_should() {
        // Rounding (q²/12) plus triangular dither (q²/6): q/2 rms in all.
        let e = error_of(16, Shaping::Off, 1 << 16);
        let rms = (e.iter().map(|v| v * v).sum::<f64>() / e.len() as f64).sqrt();
        let q = f64::from(Dither::lsb(16));
        assert!((rms / (q / 2.0) - 1.0).abs() < 0.05, "{} LSB rms", rms / q);
        // Mean zero: no DC.
        assert!((e.iter().sum::<f64>() / e.len() as f64).abs() < 0.02 * q);
    }

    #[test]
    fn shaping_moves_the_noise_out_of_the_ears_band() {
        // 2–5 kHz at 44.1 kHz is 0.045…0.113 of the rate.
        let n = 4096 * 6;
        let flat = band_power(&error_of(16, Shaping::Off, n), 0.045, 0.113);
        let light = band_power(&error_of(16, Shaping::Light, n), 0.045, 0.113);
        let strong = band_power(&error_of(16, Shaping::Strong, n), 0.045, 0.113);
        let db = |p: f64| 10.0 * (p / flat).log10();
        // Measured 2026-11-09: light −6.0 dB there, strong −18.5 dB.
        assert!(db(light) < -5.0, "light {:.1} dB", db(light));
        assert!(db(strong) < -16.0, "strong {:.1} dB", db(strong));
        // And up at the top.
        let top = |s| band_power(&error_of(16, s, n), 0.35, 0.5);
        assert!(top(Shaping::Strong) > 3.0 * top(Shaping::Off));
    }
}
