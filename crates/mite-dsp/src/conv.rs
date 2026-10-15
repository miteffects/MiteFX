//! Uniformly partitioned FFT convolution (overlap-save with a frequency-domain delay line), stereo,
//! with a 2×2 kernel matrix so stereo placements (L/R/M/S) survive. 64-bit throughout, so neutral
//! kernels null far below −140 dBFS.
//!
//! Kernels are prepared off the audio thread ([`KernelSet::new`]) and swapped in with a crossfade
//! ([`Convolver::offer`]). [`Convolver::process`] never allocates, locks or frees: the kernel it
//! replaces comes back out through [`Convolver::retired`] for the caller to dispose of elsewhere.
//!
//! Latency is the block size plus the kernel's own delay (see [`Convolver::latency`]).

use std::sync::Arc;

use realfft::num_complex::Complex;
use realfft::{ComplexToReal, RealFftPlanner, RealToComplex};

/// How a kernel set maps input channels to output channels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KernelLayout {
    /// One kernel for both channels.
    Shared,
    /// A kernel per channel (left, right).
    Diagonal,
    /// Four kernels LL, LR, RL, RR: out_L = LL∗in_L + LR∗in_R, out_R = RL∗in_L + RR∗in_R.
    Matrix,
}

impl KernelLayout {
    fn kernels(self) -> usize {
        match self {
            Self::Shared => 1,
            Self::Diagonal => 2,
            Self::Matrix => 4,
        }
    }
}

/// A designed set of time-domain kernels, before partitioning: their layout, the kernels (1, 2
/// or 4, all the same length), and their delay in samples.
pub struct KernelDesign {
    pub layout: KernelLayout,
    pub kernels: Vec<Vec<f64>>,
    pub delay: usize,
}

/// Kernels cut into block-sized partitions and transformed, ready for a [`Convolver`] with the
/// same block size. Build it off the audio thread.
pub struct KernelSet {
    block: usize,
    partitions: usize,
    layout: KernelLayout,
    /// Partition spectra, kernel-major: `[kernel][partition][bin]`, `block + 1` bins each, with the
    /// inverse FFT's 1/(2·block) folded in.
    spectra: Vec<Complex<f64>>,
    /// The kernels' own delay, samples (e.g. half the length for a linear-phase kernel).
    delay: usize,
}

impl KernelSet {
    /// Partitions and transforms `kernels` (all the same length; 1, 2 or 4 of them for
    /// [`KernelLayout::Shared`], `Diagonal` or `Matrix`). Allocates.
    pub fn new(block: usize, layout: KernelLayout, kernels: &[Vec<f64>], delay: usize) -> Self {
        assert_eq!(kernels.len(), layout.kernels(), "kernel count must match the layout");
        let len = kernels[0].len();
        assert!(kernels.iter().all(|k| k.len() == len), "kernels must have equal lengths");
        let partitions = len.div_ceil(block).max(1);
        let bins = block + 1;
        let fft = RealFftPlanner::<f64>::new().plan_fft_forward(2 * block);
        let mut frame = vec![0.0; 2 * block];
        let mut spectrum = fft.make_output_vec();
        let mut scratch = fft.make_scratch_vec();
        let norm = 1.0 / (2 * block) as f64;
        let mut spectra = Vec::with_capacity(kernels.len() * partitions * bins);
        for kernel in kernels {
            for p in 0..partitions {
                frame.fill(0.0);
                let start = p * block;
                let end = (start + block).min(len);
                frame[..end - start].copy_from_slice(&kernel[start..end]);
                fft.process_with_scratch(&mut frame, &mut spectrum, &mut scratch).expect("buffers sized by the planner");
                spectra.extend(spectrum.iter().map(|c| c * norm));
            }
        }
        Self { block, partitions, layout, spectra, delay }
    }

    pub fn block(&self) -> usize {
        self.block
    }

    pub fn layout(&self) -> KernelLayout {
        self.layout
    }

    pub fn partitions(&self) -> usize {
        self.partitions
    }

    pub fn delay(&self) -> usize {
        self.delay
    }

    fn kernel(&self, k: usize, p: usize) -> &[Complex<f64>] {
        let bins = self.block + 1;
        let start = (k * self.partitions + p) * bins;
        &self.spectra[start..start + bins]
    }
}

/// Stereo partitioned convolver. Buffers are sized in [`new`](Self::new) for the longest kernel
/// it will ever run; everything after that is allocation-free.
pub struct Convolver {
    block: usize,
    max_partitions: usize,
    r2c: Arc<dyn RealToComplex<f64>>,
    c2r: Arc<dyn ComplexToReal<f64>>,
    fwd_scratch: Vec<Complex<f64>>,
    inv_scratch: Vec<Complex<f64>>,
    /// FFT input (previous block + current block) and inverse-FFT output.
    frame: Vec<f64>,
    time: Vec<f64>,
    acc: Vec<Complex<f64>>,
    /// Per channel: the previous input block, and the frequency-domain delay line of input-frame
    /// spectra (`max_partitions` slots of `block + 1` bins; `head` is the newest).
    prev: [Vec<f64>; 2],
    fdl: [Vec<Complex<f64>>; 2],
    head: usize,
    /// Per channel: the block being collected, and the last block's output from the current and
    /// the outgoing kernel. `pos` is the position in the block.
    input: [Vec<f64>; 2],
    out_new: [Vec<f64>; 2],
    out_old: [Vec<f64>; 2],
    pos: usize,
    current: Option<Box<KernelSet>>,
    outgoing: Option<Box<KernelSet>>,
    retired: Option<Box<KernelSet>>,
    /// Crossfade progress and length, samples. A crossfade starts at the next block boundary,
    /// the first block both kernels have produced.
    fade: usize,
    fade_len: usize,
    fade_started: bool,
}

impl Convolver {
    /// A convolver with partitions of `block` samples for kernels up to `max_len` samples, whose
    /// kernel swaps crossfade over `fade_len` samples. Allocates.
    pub fn new(block: usize, max_len: usize, fade_len: usize) -> Self {
        let mut planner = RealFftPlanner::<f64>::new();
        let r2c = planner.plan_fft_forward(2 * block);
        let c2r = planner.plan_fft_inverse(2 * block);
        let max_partitions = max_len.div_ceil(block).max(1);
        let bins = block + 1;
        let zeros = || vec![0.0; block];
        Self {
            block,
            max_partitions,
            fwd_scratch: r2c.make_scratch_vec(),
            inv_scratch: c2r.make_scratch_vec(),
            r2c,
            c2r,
            frame: vec![0.0; 2 * block],
            time: vec![0.0; 2 * block],
            acc: vec![Complex::default(); bins],
            prev: [zeros(), zeros()],
            fdl: [vec![Complex::default(); max_partitions * bins], vec![Complex::default(); max_partitions * bins]],
            head: 0,
            input: [zeros(), zeros()],
            out_new: [zeros(), zeros()],
            out_old: [zeros(), zeros()],
            pos: 0,
            current: None,
            outgoing: None,
            retired: None,
            fade: 0,
            fade_len: fade_len.max(1),
            fade_started: false,
        }
    }

    pub fn block(&self) -> usize {
        self.block
    }

    /// Input-to-output delay with the current kernel: one block of buffering plus the kernel's
    /// own delay.
    pub fn latency(&self) -> usize {
        self.block + self.current.as_ref().map_or(0, |k| k.delay)
    }

    /// Whether a kernel swap is still crossfading (a new kernel is refused until it ends).
    pub fn is_fading(&self) -> bool {
        self.outgoing.is_some()
    }

    /// Installs `kernel`, crossfading from the current one. Refused (handed back) while a swap is
    /// still fading or a retired kernel has not been collected. Allocation-free.
    pub fn offer(&mut self, kernel: Box<KernelSet>) -> Result<(), Box<KernelSet>> {
        if self.outgoing.is_some() || self.retired.is_some() || kernel.block != self.block || kernel.partitions > self.max_partitions {
            return Err(kernel);
        }
        match self.current.replace(kernel) {
            Some(old) => {
                // The rest of this block is the old kernel's output; the fade starts after it.
                for c in 0..2 {
                    self.out_old[c].copy_from_slice(&self.out_new[c]);
                }
                self.outgoing = Some(old);
                self.fade = 0;
                self.fade_started = false;
            }
            None => self.fade = self.fade_len,
        }
        Ok(())
    }

    /// Installs `kernel` at once, without a crossfade (for `prepare`, or behind a mute). The kernel
    /// it replaces is returned. Allocation-free.
    pub fn replace(&mut self, kernel: Box<KernelSet>) -> Option<Box<KernelSet>> {
        assert!(kernel.block == self.block && kernel.partitions <= self.max_partitions, "kernel does not fit this convolver");
        if let Some(old) = self.outgoing.take() {
            self.retired.get_or_insert(old);
        }
        self.current.replace(kernel)
    }

    /// The kernel a finished crossfade let go of, for the caller to free off the audio thread.
    pub fn retired(&mut self) -> Option<Box<KernelSet>> {
        self.retired.take()
    }

    /// Clears the signal history (not the kernels). Allocation-free.
    pub fn reset(&mut self) {
        for c in 0..2 {
            self.prev[c].fill(0.0);
            self.fdl[c].fill(Complex::default());
            self.input[c].fill(0.0);
            self.out_new[c].fill(0.0);
            self.out_old[c].fill(0.0);
        }
        self.pos = 0;
    }

    /// Convolves a stereo buffer in place. Allocation- and lock-free.
    pub fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            let i = self.pos;
            // A non-finite input sample would poison the delay line for the kernel's whole length
            // (and make the FFT refuse its input): treat it as silence.
            let finite = |x: f32| if x.is_finite() { f64::from(x) } else { 0.0 };
            self.input[0][i] = finite(*l);
            self.input[1][i] = finite(*r);
            let (yl, yr) = if self.outgoing.is_some() {
                // Raised-cosine crossfade from the outgoing kernel to the new one.
                let t = self.fade as f64 / self.fade_len as f64;
                let w = 0.5 - 0.5 * (std::f64::consts::PI * t).cos();
                if self.fade_started {
                    self.fade += 1;
                }
                let mix = |new: f64, old: f64| old + w * (new - old);
                (mix(self.out_new[0][i], self.out_old[0][i]), mix(self.out_new[1][i], self.out_old[1][i]))
            } else {
                (self.out_new[0][i], self.out_new[1][i])
            };
            *l = yl as f32;
            *r = yr as f32;
            self.pos += 1;
            if self.pos == self.block {
                self.pos = 0;
                if self.fade >= self.fade_len {
                    if let Some(old) = self.outgoing.take() {
                        self.retired = Some(old);
                    }
                }
                self.process_block();
                self.fade_started = self.outgoing.is_some();
            }
        }
    }

    /// One block: transform the new input frame into the delay line, then convolve with the
    /// current (and, while fading, the outgoing) kernel.
    fn process_block(&mut self) {
        let block = self.block;
        let bins = block + 1;
        self.head = (self.head + 1) % self.max_partitions;
        for c in 0..2 {
            self.frame[..block].copy_from_slice(&self.prev[c]);
            self.frame[block..].copy_from_slice(&self.input[c]);
            self.prev[c].copy_from_slice(&self.input[c]);
            let slot = &mut self.fdl[c][self.head * bins..(self.head + 1) * bins];
            // Buffers are sized by the planner, so the only possible error is about input values;
            // the transform still runs, and the audio thread must never panic.
            let _ = self.r2c.process_with_scratch(&mut self.frame, slot, &mut self.fwd_scratch);
        }
        if let Some(k) = self.current.take() {
            self.convolve(&k, true);
            self.current = Some(k);
        }
        if let Some(k) = self.outgoing.take() {
            self.convolve(&k, false);
            self.outgoing = Some(k);
        }
    }

    fn convolve(&mut self, k: &KernelSet, new: bool) {
        let block = self.block;
        let bins = block + 1;
        for out in 0..2 {
            self.acc.fill(Complex::default());
            for p in 0..k.partitions {
                let slot = (self.head + self.max_partitions - p) % self.max_partitions;
                let x = |c: usize| &self.fdl[c][slot * bins..(slot + 1) * bins];
                // (input channel, kernel index) pairs feeding this output.
                let (terms, count) = match (k.layout, out) {
                    (KernelLayout::Shared, o) => ([(o, 0), (0, 0)], 1),
                    (KernelLayout::Diagonal, o) => ([(o, o), (0, 0)], 1),
                    (KernelLayout::Matrix, 0) => ([(0, 0), (1, 1)], 2),
                    (KernelLayout::Matrix, _) => ([(0, 2), (1, 3)], 2),
                };
                for &(c, kernel) in &terms[..count] {
                    for ((a, xv), hv) in self.acc.iter_mut().zip(x(c)).zip(k.kernel(kernel, p)) {
                        *a += xv * hv;
                    }
                }
            }
            // As above: never panic here. (It could only object to non-zero imaginary parts at DC
            // or Nyquist, which it ignores anyway.)
            let _ = self.c2r.process_with_scratch(&mut self.acc, &mut self.time, &mut self.inv_scratch);
            let dest = if new { &mut self.out_new[out] } else { &mut self.out_old[out] };
            dest.copy_from_slice(&self.time[block..]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn noise(n: usize, seed: u32) -> Vec<f32> {
        let mut s = seed;
        (0..n)
            .map(|_| {
                s ^= s << 13;
                s ^= s >> 17;
                s ^= s << 5;
                s as f32 / u32::MAX as f32 - 0.5
            })
            .collect()
    }

    fn direct(x: &[f32], h: &[f64]) -> Vec<f64> {
        (0..x.len()).map(|n| h.iter().enumerate().filter(|(k, _)| *k <= n).map(|(k, &hk)| hk * f64::from(x[n - k])).sum()).collect()
    }

    /// Runs `x` through in host-sized chunks of `chunk` samples.
    fn run(conv: &mut Convolver, l: &[f32], r: &[f32], chunk: usize) -> (Vec<f32>, Vec<f32>) {
        let (mut l, mut r) = (l.to_vec(), r.to_vec());
        for (a, b) in l.chunks_mut(chunk).zip(r.chunks_mut(chunk)) {
            conv.process(a, b);
        }
        (l, r)
    }

    #[test]
    fn matches_direct_convolution_after_its_latency() {
        let block = 64;
        let h: Vec<f64> = noise(300, 7).iter().map(|&v| f64::from(v)).collect();
        let mut conv = Convolver::new(block, 512, 256);
        conv.replace(Box::new(KernelSet::new(block, KernelLayout::Shared, std::slice::from_ref(&h), 0)));
        assert_eq!(conv.latency(), block);
        let (l, r) = (noise(2000, 1), noise(2000, 2));
        // Odd host block sizes must not matter.
        let (yl, yr) = run(&mut conv, &l, &r, 37);
        let (dl, dr) = (direct(&l, &h), direct(&r, &h));
        for n in 0..2000 - block {
            assert!((f64::from(yl[n + block]) - dl[n]).abs() < 1e-5, "left {n}");
            assert!((f64::from(yr[n + block]) - dr[n]).abs() < 1e-5, "right {n}");
        }
    }

    #[test]
    fn matrix_kernels_mix_the_channels() {
        let block = 32;
        // out_L = in_R delayed 3; out_R = 0.5·in_L + in_R.
        let delta = |at: usize, g: f64| {
            let mut k = vec![0.0; 40];
            k[at] = g;
            k
        };
        let kernels = [delta(0, 0.0), delta(3, 1.0), delta(0, 0.5), delta(0, 1.0)];
        let mut conv = Convolver::new(block, 64, 64);
        conv.replace(Box::new(KernelSet::new(block, KernelLayout::Matrix, &kernels, 0)));
        let (l, r) = (noise(400, 3), noise(400, 4));
        let (yl, yr) = run(&mut conv, &l, &r, 50);
        for n in 10..400 - block {
            assert!((yl[n + block] - r[n - 3]).abs() < 1e-6);
            assert!((yr[n + block] - (0.5 * l[n] + r[n])).abs() < 1e-6);
        }
    }

    #[test]
    fn non_finite_input_is_silenced_instead_of_poisoning_the_output() {
        let block = 32;
        let mut conv = Convolver::new(block, 64, 64);
        conv.replace(Box::new(KernelSet::new(block, KernelLayout::Shared, &[vec![1.0; 40]], 0)));
        let mut l = vec![0.25f32; 512];
        l[10] = f32::NAN;
        l[11] = f32::INFINITY;
        let mut r = l.clone();
        conv.process(&mut l, &mut r);
        assert!(l.iter().chain(&r).all(|v| v.is_finite()));
    }

    #[test]
    fn a_kernel_swap_crossfades_and_retires_the_old_kernel() {
        let block = 32;
        let gain = |g: f64| Box::new(KernelSet::new(block, KernelLayout::Shared, &[vec![g]], 0));
        let mut conv = Convolver::new(block, 32, 128);
        conv.replace(gain(1.0));
        let ones = vec![1.0f32; 1024];
        let (y, _) = run(&mut conv, &ones, &ones, 64);
        assert!((y[200] - 1.0).abs() < 1e-6);
        assert!(conv.offer(gain(0.0)).is_ok());
        assert!(conv.offer(gain(0.5)).is_err(), "a second swap must wait for the fade");
        let (y, _) = run(&mut conv, &ones, &ones, 64);
        // Smooth: never jumps, ends at the new gain.
        let jump = y.windows(2).map(|w| (w[1] - w[0]).abs()).fold(0.0f32, f32::max);
        assert!(jump < 0.03, "step {jump}");
        assert!(y[1000].abs() < 1e-6);
        assert!(conv.retired().is_some());
        assert!(conv.offer(gain(0.5)).is_ok());
    }
}
