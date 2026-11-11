//! The one hot loop: a frame's outputs = bias + Σ over taps and inputs of weight × input, with
//! the outputs kept in registers (specialised for the common output widths) and, where the CPU
//! has them, AVX2 and FMA (chosen at run time; plugins are built for the baseline x86-64).

use std::sync::LazyLock;

static WIDE: LazyLock<bool> = LazyLock::new(|| {
    #[cfg(target_arch = "x86_64")]
    {
        std::arch::is_x86_feature_detected!("avx2") && std::arch::is_x86_feature_detected!("fma")
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        false
    }
});

/// Whether the AVX2/FMA kernels run on this machine.
pub fn wide() -> bool {
    *WIDE
}

/// What one convolution call needs: weights `[tap][in][out]`, an optional bias, the input
/// (frame `f` of tap `k` at `x[(firsts[k] + f)·stride ..][..ci]`), and `n` output frames.
pub(crate) struct Call<'a> {
    pub w: &'a [f32],
    pub bias: Option<&'a [f32]>,
    pub x: &'a [f32],
    pub stride: usize,
    pub ci: usize,
    pub co: usize,
    pub firsts: &'a [usize],
    pub n: usize,
}

#[inline(always)]
fn fixed<const CO: usize, const FMA: bool>(c: &Call, y: &mut [f32]) {
    let ci = c.ci;
    for f in 0..c.n {
        let mut acc = [0.0f32; CO];
        if let Some(b) = c.bias {
            acc.copy_from_slice(&b[..CO]);
        }
        for (k, &first) in c.firsts.iter().enumerate() {
            let at = (first + f) * c.stride;
            let xs = &c.x[at..at + ci];
            let wk = &c.w[k * ci * CO..(k + 1) * ci * CO];
            for (&xi, col) in xs.iter().zip(wk.chunks_exact(CO)) {
                let col: &[f32; CO] = col.try_into().expect("CO wide");
                for o in 0..CO {
                    acc[o] = if FMA { xi.mul_add(col[o], acc[o]) } else { acc[o] + xi * col[o] };
                }
            }
        }
        y[f * CO..(f + 1) * CO].copy_from_slice(&acc);
    }
}

/// Any width: accumulates in the output itself.
#[inline(always)]
fn any<const FMA: bool>(c: &Call, y: &mut [f32]) {
    let (ci, co) = (c.ci, c.co);
    for f in 0..c.n {
        let out = &mut y[f * co..(f + 1) * co];
        match c.bias {
            Some(b) => out.copy_from_slice(&b[..co]),
            None => out.fill(0.0),
        }
        for (k, &first) in c.firsts.iter().enumerate() {
            let at = (first + f) * c.stride;
            let xs = &c.x[at..at + ci];
            let wk = &c.w[k * ci * co..(k + 1) * ci * co];
            for (&xi, col) in xs.iter().zip(wk.chunks_exact(co)) {
                for (o, &wo) in out.iter_mut().zip(col) {
                    *o = if FMA { xi.mul_add(wo, *o) } else { *o + xi * wo };
                }
            }
        }
    }
}

/// A single output: a dot product per frame (the head convolutions).
#[inline(always)]
fn one<const FMA: bool>(c: &Call, y: &mut [f32]) {
    let ci = c.ci;
    for (f, out) in y[..c.n].iter_mut().enumerate() {
        // Eight partial sums, so the reduction vectorises.
        let mut part = [0.0f32; 8];
        let mut tail = c.bias.map_or(0.0, |b| b[0]);
        for (k, &first) in c.firsts.iter().enumerate() {
            let at = (first + f) * c.stride;
            let xs = &c.x[at..at + ci];
            let wk = &c.w[k * ci..(k + 1) * ci];
            let mut xc = xs.chunks_exact(8);
            let mut wc = wk.chunks_exact(8);
            for (xa, wa) in (&mut xc).zip(&mut wc) {
                for o in 0..8 {
                    part[o] = if FMA { xa[o].mul_add(wa[o], part[o]) } else { part[o] + xa[o] * wa[o] };
                }
            }
            for (&a, &b) in xc.remainder().iter().zip(wc.remainder()) {
                tail = if FMA { a.mul_add(b, tail) } else { tail + a * b };
            }
        }
        *out = part.iter().sum::<f32>() + tail;
    }
}

#[inline(always)]
fn run<const FMA: bool>(c: &Call, y: &mut [f32]) {
    match c.co {
        1 => one::<FMA>(c, y),
        2 => fixed::<2, FMA>(c, y),
        3 => fixed::<3, FMA>(c, y),
        4 => fixed::<4, FMA>(c, y),
        5 => fixed::<5, FMA>(c, y),
        6 => fixed::<6, FMA>(c, y),
        8 => fixed::<8, FMA>(c, y),
        10 => fixed::<10, FMA>(c, y),
        12 => fixed::<12, FMA>(c, y),
        16 => fixed::<16, FMA>(c, y),
        20 => fixed::<20, FMA>(c, y),
        24 => fixed::<24, FMA>(c, y),
        32 => fixed::<32, FMA>(c, y),
        _ => any::<FMA>(c, y),
    }
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2,fma")]
unsafe fn run_wide(c: &Call, y: &mut [f32]) {
    run::<true>(c, y);
}

/// Runs the call into `y` (`n` frames of `co`).
pub(crate) fn convolve(c: &Call, y: &mut [f32]) {
    #[cfg(target_arch = "x86_64")]
    if wide() {
        // SAFETY: the CPU was checked for AVX2 and FMA above.
        unsafe { run_wide(c, y) };
        return;
    }
    run::<false>(c, y);
}
