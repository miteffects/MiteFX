//! Loudness and true peak as broadcast measures them: ITU-R BS.1770-4 and EBU R128 (BLUEPRINT
//! §4.2, "Momentary LUFS only" in Nova → the full set). Shared by Mite Limit, Mite Meter and auto
//! gain everywhere.
//!
//! - **K weighting:** BS.1770's two sections (a +4 dB shelf near 1.7 kHz, a high pass near 38 Hz),
//!   designed from their analog prototypes for any rate; at 48 kHz they are BS.1770's table.
//! - **Momentary** (400 ms), **short-term** (3 s): mean square of the weighted channels, summed
//!   (L and R weight 1), every 100 ms.
//! - **Integrated:** BS.1770-4's gating on 400 ms blocks with 75 % overlap: an absolute gate at
//!   −70 LUFS, then a relative gate 10 LU under the mean of what passed.
//! - **Loudness range** (EBU Tech 3342): short-term values every 100 ms, gated at −70 LUFS and
//!   20 LU under their mean, the spread between the 10th and the 95th percentile.
//! - **True peak** (BS.1770-4 Annex 2): the largest magnitude of the signal interpolated 4×.
//!
//! The gates need every block since the start. They go into histograms with a bin per 0.01 dB
//! that keep each bin's exact energy as well as its count, so the means are exact and only the
//! bin a gate falls in is approximate (0.01 dB); nothing allocates after [`Loudness::new`].

use std::f64::consts::PI;

/// What a level of no signal reads, LUFS.
pub const SILENCE_LUFS: f32 = -120.0;
/// BS.1770's offset: a 1 kHz sine at −23 dBFS in both channels reads −23 LUFS.
const OFFSET: f64 = -0.691;
const ABSOLUTE_GATE: f64 = -70.0;
const RELATIVE_GATE: f64 = -10.0;
const RANGE_GATE: f64 = -20.0;
/// Histogram span (LUFS) and resolution (bins per dB).
const HIST_LOW: f64 = -70.0;
const HIST_HIGH: f64 = 10.0;
const BINS_PER_DB: f64 = 100.0;
const BINS: usize = ((HIST_HIGH - HIST_LOW) * BINS_PER_DB) as usize;
/// 100 ms steps: 4 make the momentary window, 30 the short-term one.
const MOMENTARY_STEPS: usize = 4;
const SHORT_STEPS: usize = 30;

/// One biquad (transposed direct form II, f64: the 38 Hz pole pair sits close to the unit circle).
#[derive(Clone, Copy, Debug, Default)]
struct Biquad {
    b: [f64; 3],
    a: [f64; 2],
    s: [f64; 2],
}

impl Biquad {
    #[inline]
    fn run(&mut self, x: f64) -> f64 {
        let y = self.b[0] * x + self.s[0];
        self.s[0] = self.b[1] * x - self.a[0] * y + self.s[1];
        self.s[1] = self.b[2] * x - self.a[1] * y;
        y
    }
}

/// BS.1770's K weighting for one channel.
#[derive(Clone, Copy, Debug)]
pub struct KWeighting {
    shelf: Biquad,
    high_pass: Biquad,
}

impl KWeighting {
    pub fn new(sample_rate: f64) -> Self {
        // The shelf: BS.1770's first stage from its analog prototype (as libebur128 derives it).
        let (f0, gain_db, q) = (1681.974450955533, 3.999843853973347, 0.7071752369554196);
        let k = (PI * f0 / sample_rate).tan();
        let vh = 10f64.powf(gain_db / 20.0);
        let vb = vh.powf(0.4996667741545416);
        let a0 = 1.0 + k / q + k * k;
        let shelf = Biquad {
            b: [
                (vh + vb * k / q + k * k) / a0,
                2.0 * (k * k - vh) / a0,
                (vh - vb * k / q + k * k) / a0,
            ],
            a: [2.0 * (k * k - 1.0) / a0, (1.0 - k / q + k * k) / a0],
            s: [0.0; 2],
        };
        // The RLB high pass. Its numerator is 1, −2, 1 unscaled, as in BS.1770's table: the
        // standard's −0.691 offset is calibrated with that gain.
        let (f0, q) = (38.13547087602444, 0.5003270373238773);
        let k = (PI * f0 / sample_rate).tan();
        let a0 = 1.0 + k / q + k * k;
        let high_pass = Biquad {
            b: [1.0, -2.0, 1.0],
            a: [2.0 * (k * k - 1.0) / a0, (1.0 - k / q + k * k) / a0],
            s: [0.0; 2],
        };
        Self { shelf, high_pass }
    }

    #[inline]
    pub fn run(&mut self, x: f32) -> f64 {
        self.high_pass.run(self.shelf.run(f64::from(x)))
    }

    pub fn reset(&mut self) {
        self.shelf.s = [0.0; 2];
        self.high_pass.s = [0.0; 2];
    }
}

/// A histogram of block loudness with each bin's exact energy.
#[derive(Clone, Debug)]
struct Gate {
    count: Vec<u64>,
    energy: Vec<f64>,
}

impl Gate {
    fn new() -> Self {
        Self {
            count: vec![0; BINS],
            energy: vec![0.0; BINS],
        }
    }

    fn clear(&mut self) {
        self.count.fill(0);
        self.energy.fill(0.0);
    }

    fn bin(lufs: f64) -> usize {
        (((lufs - HIST_LOW) * BINS_PER_DB) as usize).min(BINS - 1)
    }

    fn lufs_of_bin(bin: usize) -> f64 {
        HIST_LOW + bin as f64 / BINS_PER_DB
    }

    /// Adds a block's energy (mean square, weighted and summed) if it clears the absolute gate.
    fn add(&mut self, energy: f64) {
        let lufs = to_lufs(energy);
        if lufs > ABSOLUTE_GATE {
            let b = Self::bin(lufs);
            self.count[b] += 1;
            self.energy[b] += energy;
        }
    }

    /// The count and energy of the blocks from `from` up.
    fn sum_from(&self, from: usize) -> (u64, f64) {
        (
            self.count[from..].iter().sum(),
            self.energy[from..].iter().sum(),
        )
    }

    /// The first bin at or above `lufs`.
    fn first_bin_at(lufs: f64) -> usize {
        (((lufs - HIST_LOW) * BINS_PER_DB).ceil().max(0.0) as usize).min(BINS)
    }

    /// The bins from the gate `offset` LU under the mean of everything that passed the absolute
    /// gate, or `None` if nothing did.
    fn relative_from(&self, offset: f64) -> Option<usize> {
        let (n, e) = self.sum_from(0);
        (n > 0).then(|| Self::first_bin_at(to_lufs(e / n as f64) + offset))
    }
}

fn to_lufs(energy: f64) -> f64 {
    if energy > 0.0 {
        OFFSET + 10.0 * energy.log10()
    } else {
        f64::from(SILENCE_LUFS)
    }
}

/// Loudness of a mono or stereo stream: momentary, short-term, integrated and range.
#[derive(Clone, Debug)]
pub struct Loudness {
    weighting: [KWeighting; 2],
    step_len: usize,
    step_pos: usize,
    step_sum: f64,
    /// The last 30 steps' summed squares, newest at `ring_pos`.
    ring: [f64; SHORT_STEPS],
    ring_pos: usize,
    steps: usize,
    momentary: f64,
    short_term: f64,
    integrated_gate: Gate,
    range_gate: Gate,
}

impl Loudness {
    /// Allocates (the gates' histograms).
    pub fn new(sample_rate: f64) -> Self {
        let k = KWeighting::new(sample_rate);
        Self {
            weighting: [k, k],
            step_len: (sample_rate / 10.0).round().max(1.0) as usize,
            step_pos: 0,
            step_sum: 0.0,
            ring: [0.0; SHORT_STEPS],
            ring_pos: 0,
            steps: 0,
            momentary: 0.0,
            short_term: 0.0,
            integrated_gate: Gate::new(),
            range_gate: Gate::new(),
        }
    }

    /// Forgets everything: the filters, the windows and the integration.
    pub fn reset(&mut self) {
        for w in &mut self.weighting {
            w.reset();
        }
        self.step_pos = 0;
        self.step_sum = 0.0;
        self.ring = [0.0; SHORT_STEPS];
        self.ring_pos = 0;
        self.steps = 0;
        self.momentary = 0.0;
        self.short_term = 0.0;
        self.integrated_gate.clear();
        self.range_gate.clear();
    }

    /// Restarts the integrated loudness and the range (a click on the meter), keeping the
    /// momentary and short-term windows.
    pub fn reset_integration(&mut self) {
        self.integrated_gate.clear();
        self.range_gate.clear();
    }

    /// One frame: `right` is `None` for mono.
    #[inline]
    pub fn push(&mut self, left: f32, right: Option<f32>) {
        let l = self.weighting[0].run(left);
        let mut sq = l * l;
        if let Some(r) = right {
            let r = self.weighting[1].run(r);
            sq += r * r;
        }
        self.step_sum += sq;
        self.step_pos += 1;
        if self.step_pos == self.step_len {
            self.end_step();
        }
    }

    pub fn process(&mut self, left: &[f32], right: Option<&[f32]>) {
        match right {
            Some(r) => {
                for (&a, &b) in left.iter().zip(r) {
                    self.push(a, Some(b));
                }
            }
            None => {
                for &a in left {
                    self.push(a, None);
                }
            }
        }
    }

    fn end_step(&mut self) {
        self.ring_pos = (self.ring_pos + 1) % SHORT_STEPS;
        self.ring[self.ring_pos] = self.step_sum;
        self.step_sum = 0.0;
        self.step_pos = 0;
        self.steps += 1;
        let window = |n: usize| -> f64 {
            (0..n)
                .map(|i| self.ring[(self.ring_pos + SHORT_STEPS - i) % SHORT_STEPS])
                .sum::<f64>()
                / (n * self.step_len) as f64
        };
        if self.steps >= MOMENTARY_STEPS {
            self.momentary = window(MOMENTARY_STEPS);
            self.integrated_gate.add(self.momentary);
        }
        if self.steps >= SHORT_STEPS {
            self.short_term = window(SHORT_STEPS);
            self.range_gate.add(self.short_term);
        }
    }

    /// Momentary loudness (400 ms), LUFS; [`SILENCE_LUFS`] before the first 400 ms.
    pub fn momentary(&self) -> f32 {
        to_lufs(self.momentary) as f32
    }

    /// Short-term loudness (3 s), LUFS; [`SILENCE_LUFS`] before the first 3 s.
    pub fn short_term(&self) -> f32 {
        to_lufs(self.short_term) as f32
    }

    /// Integrated loudness (gated), LUFS; [`SILENCE_LUFS`] while nothing passed the gates.
    pub fn integrated(&self) -> f32 {
        let gate = &self.integrated_gate;
        let Some(from) = gate.relative_from(RELATIVE_GATE) else {
            return SILENCE_LUFS;
        };
        let (n, e) = gate.sum_from(from);
        if n == 0 {
            return SILENCE_LUFS;
        }
        to_lufs(e / n as f64) as f32
    }

    /// Loudness range (EBU Tech 3342), LU; 0 until there are short-term values.
    pub fn range(&self) -> f32 {
        let gate = &self.range_gate;
        let Some(from) = gate.relative_from(RANGE_GATE) else {
            return 0.0;
        };
        let n: u64 = gate.count[from..].iter().sum();
        if n == 0 {
            return 0.0;
        }
        // Percentiles over the gated values, by rank (nearest rank, as Tech 3342's reference).
        let percentile = |p: f64| -> f64 {
            let rank = ((p * (n - 1) as f64).round() as u64).min(n - 1);
            let mut seen = 0u64;
            for (b, &c) in gate.count.iter().enumerate().skip(from) {
                seen += c;
                if seen > rank {
                    // The bin's mean level: exact when one value lands in it.
                    return to_lufs(gate.energy[b] / c as f64);
                }
            }
            Gate::lufs_of_bin(BINS - 1)
        };
        (percentile(0.95) - percentile(0.10)).max(0.0) as f32
    }
}

/// A polyphase interpolator that reports the largest magnitude between samples: the true peak.
///
/// `phases` sub-samples per sample (phase 0 is the sample itself), each a Kaiser-windowed sinc of
/// `TAPS` taps. A sub-sample between `x[n]` and `x[n + 1]` needs `TAPS / 2` samples after it, so
/// what [`TruePeak::push`] returns at sample `n` is the largest magnitude over the stretch
/// `(x[n − D − 1], x[n − D]]`, `D` = [`TruePeak::DELAY`].
#[derive(Clone, Debug)]
pub struct TruePeak {
    phases: usize,
    /// Phase k (1…phases−1) at `taps[(k − 1) * TAPS ..]`, oldest sample first.
    taps: Vec<f32>,
    /// The last `TAPS` samples, stored twice so a window is one slice; newest at `pos + TAPS − 1`.
    hist: [f32; 2 * TAPS],
    pos: usize,
}

/// Taps per phase.
const TAPS: usize = 48;
/// The window's β: with 48 taps every phase is flat within 0.01 dB to 0.9 × Nyquist.
const TP_BETA: f64 = 6.0;

impl TruePeak {
    /// The delay of the stretch reported, samples (see the type's doc).
    pub const DELAY: usize = TAPS / 2 - 1;

    /// Allocates (the taps).
    pub fn new(phases: usize) -> Self {
        let phases = phases.max(1);
        let half = (TAPS / 2) as f64;
        let i0 = |x: f64| {
            let (mut sum, mut term, mut k) = (1.0, 1.0, 1.0);
            while term > 1e-12 * sum {
                term *= (x / (2.0 * k)) * (x / (2.0 * k));
                sum += term;
                k += 1.0;
            }
            sum
        };
        let mut taps = Vec::with_capacity((phases - 1) * TAPS);
        for k in 1..phases {
            let frac = k as f64 / phases as f64;
            // Sample t (0 = oldest) sits at offset (t − (TAPS/2 − 1)) − frac from the sub-sample.
            let raw: Vec<f64> = (0..TAPS)
                .map(|t| {
                    let d = t as f64 - (half - 1.0) - frac;
                    let sinc = if d.abs() < 1e-12 {
                        1.0
                    } else {
                        (PI * d).sin() / (PI * d)
                    };
                    let w = i0(TP_BETA * (1.0 - (d / half).powi(2)).max(0.0).sqrt()) / i0(TP_BETA);
                    sinc * w
                })
                .collect();
            // Unity at DC, so a constant reads its own value.
            let sum: f64 = raw.iter().sum();
            taps.extend(raw.iter().map(|v| (v / sum) as f32));
        }
        Self {
            phases,
            taps,
            hist: [0.0; 2 * TAPS],
            pos: 0,
        }
    }

    pub fn phases(&self) -> usize {
        self.phases
    }

    pub fn reset(&mut self) {
        self.hist = [0.0; 2 * TAPS];
        self.pos = 0;
    }

    /// Takes a sample; returns the largest magnitude over `(x[n − D − 1], x[n − D]]`.
    #[inline]
    pub fn push(&mut self, x: f32) -> f32 {
        self.hist[self.pos] = x;
        self.hist[self.pos + TAPS] = x;
        self.pos = (self.pos + 1) % TAPS;
        let window = &self.hist[self.pos..self.pos + TAPS];
        // Oldest first: window[TAPS/2 − 1] is x[n − D − 1], the stretch's left end (left out),
        // window[TAPS/2] is x[n − D], its right end; the phases fall between them.
        let mut peak = window[TAPS / 2].abs();
        for k in 0..self.phases - 1 {
            let c = &self.taps[k * TAPS..(k + 1) * TAPS];
            let mut acc = 0.0f32;
            for (a, b) in c.iter().zip(window) {
                acc += a * b;
            }
            peak = peak.max(acc.abs());
        }
        peak
    }
}

/// The true peak of a whole signal at `phases` (tests and offline reports; allocates).
pub fn true_peak_of(signal: &[f32], phases: usize) -> f32 {
    let mut tp = TruePeak::new(phases);
    let mut peak = 0.0f32;
    for &x in signal.iter().chain(std::iter::repeat_n(&0.0, TAPS)) {
        peak = peak.max(tp.push(x));
    }
    peak
}

#[cfg(test)]
mod tests {
    use super::*;

    const FS: f64 = 48000.0;

    fn sine(db: f64, seconds: f64, f: f64, phase: f64) -> Vec<f32> {
        let a = 10f64.powf(db / 20.0);
        (0..(seconds * FS) as usize)
            .map(|i| (a * (std::f64::consts::TAU * f * i as f64 / FS + phase).sin()) as f32)
            .collect()
    }

    fn stereo(parts: &[(f64, f64)]) -> Vec<f32> {
        parts
            .iter()
            .flat_map(|&(db, s)| sine(db, s, 1000.0, 0.0))
            .collect()
    }

    fn measure(signal: &[f32]) -> Loudness {
        let mut m = Loudness::new(FS);
        m.process(signal, Some(signal));
        m
    }

    #[test]
    fn k_weighting_is_bs1770s_table_at_48k() {
        let k = KWeighting::new(48000.0);
        let near = |a: f64, b: f64| (a - b).abs() < 1e-8;
        let s = k.shelf;
        assert!(
            near(s.b[0], 1.53512485958697)
                && near(s.b[1], -2.69169618940638)
                && near(s.b[2], 1.19839281085285),
            "{:?}",
            s.b
        );
        assert!(
            near(s.a[0], -1.69065929318241) && near(s.a[1], 0.73248077421585),
            "{:?}",
            s.a
        );
        let h = k.high_pass;
        assert!(
            near(h.a[0], -1.99004745483398) && near(h.a[1], 0.99007225036621),
            "{:?}",
            h.a
        );
        assert_eq!(h.b, [1.0, -2.0, 1.0]);
    }

    // EBU Tech 3341 (2016), cases 1–5 and 9, generated here from their descriptions; ±0.1 LU.
    #[test]
    fn tech_3341_steady_tones() {
        for db in [-23.0, -33.0] {
            let m = measure(&stereo(&[(db, 20.0)]));
            for (what, v) in [
                ("M", m.momentary()),
                ("S", m.short_term()),
                ("I", m.integrated()),
            ] {
                assert!((f64::from(v) - db).abs() < 0.1, "{db} dBFS: {what} = {v}");
            }
        }
    }

    #[test]
    fn tech_3341_gated_sequences() {
        let cases: [&[(f64, f64)]; 3] = [
            &[(-36.0, 10.0), (-23.0, 60.0), (-36.0, 10.0)],
            &[
                (-72.0, 10.0),
                (-36.0, 10.0),
                (-23.0, 60.0),
                (-36.0, 10.0),
                (-72.0, 10.0),
            ],
            &[(-26.0, 20.0), (-20.0, 20.1), (-26.0, 20.0)],
        ];
        for parts in cases {
            let i = measure(&stereo(parts)).integrated();
            assert!((i + 23.0).abs() < 0.1, "{parts:?}: I = {i}");
        }
    }

    #[test]
    fn tech_3341_short_term_holds_on_a_three_second_cycle() {
        let cycle = stereo(&[(-20.0, 1.34), (-30.0, 1.66)]);
        let mut m = Loudness::new(FS);
        let mut worst = 0.0f32;
        for k in 0..20 {
            for frame in cycle.chunks(4800) {
                m.process(frame, Some(frame));
                if k >= 2 {
                    worst = worst.max((m.short_term() + 23.0).abs());
                }
            }
        }
        assert!(worst < 0.1, "short-term off by {worst} LU");
    }

    // EBU Tech 3342 (2016), cases 1–4; ±1 LU.
    #[test]
    fn tech_3342_loudness_range() {
        let cases: [(&[(f64, f64)], f32); 4] = [
            (&[(-20.0, 20.0), (-30.0, 20.0)], 10.0),
            (&[(-20.0, 20.0), (-15.0, 20.0)], 5.0),
            (&[(-40.0, 20.0), (-20.0, 20.0)], 20.0),
            (
                &[
                    (-50.0, 20.0),
                    (-35.0, 20.0),
                    (-20.0, 20.0),
                    (-35.0, 20.0),
                    (-50.0, 20.0),
                ],
                15.0,
            ),
        ];
        for (parts, lra) in cases {
            let r = measure(&stereo(parts)).range();
            assert!(
                (r - lra).abs() < 1.0,
                "{parts:?}: LRA = {r}, expected {lra}"
            );
        }
    }

    #[test]
    fn silence_and_reset() {
        let m = measure(&vec![0.0; 48000 * 5]);
        assert_eq!(m.integrated(), SILENCE_LUFS);
        assert_eq!(m.range(), 0.0);
        let mut m = measure(&stereo(&[(-23.0, 5.0)]));
        m.reset_integration();
        assert_eq!(m.integrated(), SILENCE_LUFS);
        assert!((m.short_term() + 23.0).abs() < 0.1);
    }

    /// The true peak of a sine's steady middle (its abrupt start and end are steps, whose Gibbs
    /// overshoot is real but not the sine's).
    fn steady_true_peak(x: &[f32]) -> f32 {
        let mut tp = TruePeak::new(4);
        let edge = 2400;
        x.iter()
            .enumerate()
            .map(|(i, &v)| (i, tp.push(v)))
            .filter(|&(i, _)| i >= edge && i + edge < x.len())
            .fold(0.0, |m, (_, p)| m.max(p))
    }

    /// A reference true peak: the sine's own amplitude when the signal is one sine.
    #[test]
    fn true_peak_finds_what_falls_between_the_samples() {
        // fs/4 at 45°: every sample at 0.707 of the peak, the peak exactly between them.
        let x = sine(0.0, 0.5, FS / 4.0, std::f64::consts::FRAC_PI_4);
        let sample_peak = x.iter().fold(0.0f32, |m, v| m.max(v.abs()));
        assert!((crate::gain_to_db(sample_peak) + 3.01).abs() < 0.01);
        let tp = crate::gain_to_db(steady_true_peak(&x));
        assert!(tp.abs() < 0.02, "fs/4 at 45°: {tp} dBTP");
        // Across the band, at 4× the reading is never above the truth and at most BS.1770's
        // ~0.7 dB under it near Nyquist; below 8 kHz it is within 0.25 dB.
        for f in [997.0, 3001.0, 7919.0, 12007.0, 16001.0, 19997.0] {
            for phase in [0.0, 0.3, 1.1] {
                let x = sine(-1.0, 0.25, f, phase);
                let tp = crate::gain_to_db(steady_true_peak(&x)) + 1.0;
                let bound = if f < 8000.0 { 0.25 } else { 0.7 };
                assert!(
                    tp < 0.01 && tp > -bound,
                    "{f} Hz, phase {phase}: {tp} dB from the truth"
                );
            }
        }
    }

    #[test]
    fn true_peak_reports_the_stretch_after_its_delay() {
        let mut tp = TruePeak::new(4);
        let mut out = Vec::new();
        for n in 0..40 {
            out.push(tp.push(if n == 10 { 1.0 } else { 0.0 }));
        }
        // An impulse at 10 is the right end of the stretch reported at 10 + D; the next stretch
        // starts just after it (a quarter sample away: sinc(¼) ≈ 0.9), the others are far.
        let d = TruePeak::DELAY;
        assert!((out[10 + d] - 1.0).abs() < 1e-6);
        assert!(out[11 + d] > 0.85 && out[11 + d] < 0.95, "{}", out[11 + d]);
        assert!(
            out[9 + d] < 0.3 && out[12 + d] < 0.3,
            "{} {}",
            out[9 + d],
            out[12 + d]
        );
    }
}
