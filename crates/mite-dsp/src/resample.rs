//! Reading a buffer between its samples with a windowed sinc (Kaiser, 32 taps): playing audio of
//! one sample rate at another (Mite Pitch's clips at the host's rate). The table is made once
//! (allocates); reads never allocate.

const TAPS: usize = 32;
const PHASES: usize = 512;

/// Windowed-sinc coefficients per fraction of a sample, each set summing to 1.
pub struct SincTable {
    table: Vec<[f32; TAPS]>,
}

impl SincTable {
    /// For reading audio of rate `from` at rate `to`: the pass band ends at 92 % of the lower of
    /// the two Nyquists (no aliasing when going down).
    pub fn for_rates(from: f64, to: f64) -> Self {
        Self::new(0.92 * (to / from).min(1.0))
    }

    /// `cut`: the pass band's end as a fraction of the buffer's Nyquist.
    pub fn new(cut: f64) -> Self {
        let half = (TAPS / 2) as f64;
        let i0 = |x: f64| {
            let (mut sum, mut term) = (1.0, 1.0);
            for k in 1..32 {
                term *= (x / 2.0) / k as f64;
                sum += term * term;
            }
            sum
        };
        let beta = 8.0;
        let table = (0..=PHASES)
            .map(|p| {
                let f = p as f64 / PHASES as f64;
                let mut c = [0.0f64; TAPS];
                for (n, v) in c.iter_mut().enumerate() {
                    // Tap n reads sample floor(pos) + n − (half − 1).
                    let t = n as f64 - (half - 1.0) - f;
                    let x = std::f64::consts::PI * cut * t;
                    let sinc = if x.abs() < 1e-12 { 1.0 } else { x.sin() / x };
                    let r = t / half;
                    let w = if r.abs() <= 1.0 { i0(beta * (1.0 - r * r).sqrt()) / i0(beta) } else { 0.0 };
                    *v = sinc * w;
                }
                let sum: f64 = c.iter().sum();
                c.map(|v| (v / sum) as f32)
            })
            .collect();
        Self { table }
    }

    /// `x` at position `pos` (samples; silence outside the buffer).
    #[inline]
    pub fn read(&self, x: &[f32], pos: f64) -> f32 {
        let whole = pos.floor();
        let frac = (pos - whole) * PHASES as f64;
        let p = (frac as usize).min(PHASES - 1);
        let a = (frac - p as f64) as f32;
        let (c0, c1) = (&self.table[p], &self.table[p + 1]);
        let first = whole as i64 - (TAPS as i64 / 2 - 1);
        let mut y = 0.0;
        if first >= 0 && first + (TAPS as i64) <= x.len() as i64 {
            let s = &x[first as usize..first as usize + TAPS];
            for n in 0..TAPS {
                y += (c0[n] + (c1[n] - c0[n]) * a) * s[n];
            }
        } else {
            for n in 0..TAPS {
                let k = first + n as i64;
                if k >= 0 && (k as usize) < x.len() {
                    y += (c0[n] + (c1[n] - c0[n]) * a) * x[k as usize];
                }
            }
        }
        y
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(fs: f64, hz: f64, n: usize) -> Vec<f32> {
        (0..n).map(|k| (std::f64::consts::TAU * hz * k as f64 / fs).sin() as f32).collect()
    }

    #[test]
    fn a_tone_read_at_another_rate_is_the_same_tone() {
        for (from, to) in [(44_100.0, 48_000.0), (48_000.0, 44_100.0), (96_000.0, 48_000.0)] {
            let t = SincTable::for_rates(from, to);
            for hz in [100.0, 1000.0, 9000.0] {
                let x = tone(from, hz, from as usize);
                let want = tone(to, hz, to as usize);
                let err = (1000..(to as usize - 1000)).map(|k| (t.read(&x, k as f64 * from / to) - want[k]).abs()).fold(0.0f32, f32::max);
                assert!(err < 2e-3, "{from}→{to}, {hz} Hz: {err}");
            }
        }
    }

    #[test]
    fn whole_positions_read_the_samples() {
        let t = SincTable::new(0.92);
        let x = tone(48_000.0, 440.0, 4800);
        let err = (100..4700).map(|k| (t.read(&x, k as f64) - x[k]).abs()).fold(0.0f32, f32::max);
        assert!(err < 2e-3, "{err}");
    }
}
