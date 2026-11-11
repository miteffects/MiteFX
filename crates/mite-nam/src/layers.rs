//! The building blocks every architecture is made of: activations, 1×1 and dilated causal
//! convolutions, FiLM. Audio is frame-major (`[frame][channel]`), so one frame's channels are
//! contiguous and a matrix-vector product per frame runs over contiguous memory.
//!
//! Weights are read in the order the trainer flattens them (NeuralAmpModelerCore's loaders are
//! the reference for that order; the tests compare against its output sample by sample).

use std::slice::Iter;

use crate::kernel::{convolve, Call};

/// The flat weight list of a model, read front to back.
pub struct Weights<'a> {
    it: Iter<'a, f32>,
    pub(crate) taken: usize,
}

impl<'a> Weights<'a> {
    pub fn new(w: &'a [f32]) -> Self {
        Self { it: w.iter(), taken: 0 }
    }

    pub fn take(&mut self) -> Result<f32, String> {
        self.taken += 1;
        self.it.next().copied().ok_or_else(|| "the file has fewer weights than its layers need".to_owned())
    }

    pub fn remaining(&self) -> usize {
        self.it.len()
    }
}

/// An activation, applied in place.
#[derive(Debug, Clone, PartialEq)]
pub enum Activation {
    Identity,
    Tanh,
    /// The rational approximation NAM's plugins run instead of `tanh` (`fast_tanh`).
    FastTanh,
    HardTanh,
    ReLU,
    LeakyReLU(f32),
    /// Per channel slopes (one slope for all when a single one is given).
    PReLU(Vec<f32>),
    Sigmoid,
    SiLU,
    HardSwish,
    LeakyHardTanh { min_val: f32, max_val: f32, min_slope: f32, max_slope: f32 },
    Softsign,
}

/// The constants are the reference's, digit for digit (their `f32` roundings are what it uses).
#[inline]
#[allow(clippy::excessive_precision)]
pub fn fast_tanh(x: f32) -> f32 {
    let ax = x.abs();
    let x2 = x * x;
    (x * (2.45550750702956 + 2.45550750702956 * ax + (0.893229853513558 + 0.821226666969744 * ax) * x2))
        / (2.44506634652299 + (2.44506634652299 + x2) * (x + 0.814642734961073 * x * ax).abs())
}

#[inline]
pub fn sigmoid(x: f32) -> f32 {
    1.0 / (1.0 + (-x).exp())
}

#[inline]
pub fn fast_sigmoid(x: f32) -> f32 {
    0.5 * (fast_tanh(x * 0.5) + 1.0)
}

impl Activation {
    /// Applies the activation to `data`. PReLU's slope for element `pos` is `slopes[pos % n]`,
    /// as in the reference (frame-major data with as many slopes as channels: per channel).
    pub fn apply(&self, data: &mut [f32]) {
        match self {
            Self::Identity => {}
            Self::Tanh => data.iter_mut().for_each(|x| *x = x.tanh()),
            Self::FastTanh => data.iter_mut().for_each(|x| *x = fast_tanh(*x)),
            Self::HardTanh => data.iter_mut().for_each(|x| *x = x.clamp(-1.0, 1.0)),
            Self::ReLU => data.iter_mut().for_each(|x| *x = if *x > 0.0 { *x } else { 0.0 }),
            Self::LeakyReLU(s) => data.iter_mut().for_each(|x| *x = if *x > 0.0 { *x } else { s * *x }),
            Self::PReLU(slopes) => {
                let n = slopes.len().max(1);
                for (pos, x) in data.iter_mut().enumerate() {
                    let s = slopes.get(pos % n).copied().unwrap_or(0.01);
                    *x = if *x > 0.0 { *x } else { s * *x };
                }
            }
            Self::Sigmoid => data.iter_mut().for_each(|x| *x = sigmoid(*x)),
            Self::SiLU => data.iter_mut().for_each(|x| *x *= sigmoid(*x)),
            Self::HardSwish => data.iter_mut().for_each(|x| {
                let t = (*x + 3.0).clamp(0.0, 6.0);
                *x = *x * t * (1.0 / 6.0);
            }),
            Self::LeakyHardTanh { min_val, max_val, min_slope, max_slope } => data.iter_mut().for_each(|x| {
                *x = if *x < *min_val {
                    (*x - min_val) * min_slope + min_val
                } else if *x > *max_val {
                    (*x - max_val) * max_slope + max_val
                } else {
                    *x
                };
            }),
            Self::Softsign => data.iter_mut().for_each(|x| *x /= 1.0 + x.abs()),
        }
    }

    /// From a config value: a name, or an object with `type` and parameters. `fast` turns `Tanh`
    /// into [`FastTanh`](Self::FastTanh), as NAM's plugins do.
    pub fn from_json(v: &serde_json::Value, fast: bool) -> Result<Self, String> {
        let (name, obj) = match v {
            serde_json::Value::String(s) => (s.as_str(), None),
            serde_json::Value::Object(o) => (o.get("type").and_then(|t| t.as_str()).ok_or("an activation without a type")?, Some(o)),
            _ => return Err("an activation that is neither a name nor an object".to_owned()),
        };
        let num = |k: &str, d: f32| obj.and_then(|o| o.get(k)).and_then(|x| x.as_f64()).map_or(d, |x| x as f32);
        Ok(match name {
            "Tanh" if fast => Self::FastTanh,
            "Tanh" => Self::Tanh,
            "Fasttanh" => Self::FastTanh,
            "Hardtanh" => Self::HardTanh,
            "ReLU" => Self::ReLU,
            "LeakyReLU" => Self::LeakyReLU(num("negative_slope", 0.01)),
            "PReLU" => {
                if let Some(list) = obj.and_then(|o| o.get("negative_slopes")).and_then(|x| x.as_array()) {
                    Self::PReLU(list.iter().map(|x| x.as_f64().unwrap_or(0.01) as f32).collect())
                } else {
                    Self::PReLU(vec![num("negative_slope", 0.01)])
                }
            }
            "Sigmoid" => Self::Sigmoid,
            "SiLU" => Self::SiLU,
            "Hardswish" => Self::HardSwish,
            "LeakyHardtanh" | "LeakyHardTanh" => Self::LeakyHardTanh {
                min_val: num("min_val", -1.0),
                max_val: num("max_val", 1.0),
                min_slope: num("min_slope", 0.01),
                max_slope: num("max_slope", 0.01),
            },
            "Softsign" => Self::Softsign,
            other => return Err(format!("unknown activation {other}")),
        })
    }
}

/// Reads a grouped weight matrix in the trainer's order (per group, output, input, then tap)
/// into column-major taps `[k][in][out]`. Block-diagonal: off-group entries stay zero.
fn read_grouped(w: &mut Weights, ci: usize, co: usize, k: usize, groups: usize) -> Result<Vec<f32>, String> {
    if groups == 0 || ci % groups != 0 || co % groups != 0 {
        return Err(format!("{ci}→{co} channels do not split into {groups} groups"));
    }
    let (ipg, opg) = (ci / groups, co / groups);
    let mut m = vec![0.0; k * ci * co];
    for g in 0..groups {
        for i in 0..opg {
            for j in 0..ipg {
                for tap in 0..k {
                    let (o, inp) = (g * opg + i, g * ipg + j);
                    m[tap * ci * co + inp * co + o] = w.take()?;
                }
            }
        }
    }
    Ok(m)
}

/// A 1×1 convolution: a matrix per frame, optional bias.
pub struct Conv1x1 {
    pub ci: usize,
    pub co: usize,
    w: Vec<f32>,
    bias: Option<Vec<f32>>,
}

impl Conv1x1 {
    pub fn read(w: &mut Weights, ci: usize, co: usize, bias: bool, groups: usize) -> Result<Self, String> {
        let m = read_grouped(w, ci, co, 1, groups)?;
        let bias = if bias { Some((0..co).map(|_| w.take()).collect::<Result<Vec<_>, _>>()?) } else { None };
        Ok(Self { ci, co, w: m, bias })
    }

    /// `x`: `n` frames of `ci` channels at stride `stride`; `y`: `n` frames of `co`.
    pub fn process(&self, x: &[f32], stride: usize, y: &mut [f32], n: usize) {
        let call = Call { w: &self.w, bias: self.bias.as_deref(), x, stride, ci: self.ci, co: self.co, firsts: &[0], n };
        convolve(&call, y);
    }
}

/// A dilated causal convolution with its input history: tap `k` of `K` reads the input
/// `dilation·(K−1−k)` frames back.
pub struct Conv1d {
    pub ci: usize,
    pub co: usize,
    pub kernel: usize,
    pub dilation: usize,
    w: Vec<f32>,
    bias: Option<Vec<f32>>,
    /// History ring of `ring` frames, stored twice over so any window is contiguous.
    hist: Vec<f32>,
    ring: usize,
    pos: usize,
    /// Each tap's first frame in `hist` for the current block.
    firsts: Vec<usize>,
}

impl Conv1d {
    #[allow(clippy::too_many_arguments)] // mirrors the layer's description
    pub fn read(w: &mut Weights, ci: usize, co: usize, kernel: usize, dilation: usize, bias: bool, groups: usize, max_block: usize) -> Result<Self, String> {
        if kernel == 0 {
            return Err("a convolution with no taps".to_owned());
        }
        let m = read_grouped(w, ci, co, kernel, groups)?;
        let bias = if bias { Some((0..co).map(|_| w.take()).collect::<Result<Vec<_>, _>>()?) } else { None };
        let ring = (kernel - 1) * dilation + max_block;
        Ok(Self { ci, co, kernel, dilation, w: m, bias, hist: vec![0.0; 2 * ring * ci], ring, pos: 0, firsts: vec![0; kernel] })
    }

    /// From taps already in `[k][in][out]` order (Linear's impulse response).
    pub fn from_taps(ci: usize, co: usize, taps: Vec<f32>, bias: Option<Vec<f32>>, max_block: usize) -> Self {
        let kernel = taps.len() / (ci * co);
        let ring = kernel.saturating_sub(1) + max_block;
        Self { ci, co, kernel, dilation: 1, w: taps, bias, hist: vec![0.0; 2 * ring * ci], ring, pos: 0, firsts: vec![0; kernel] }
    }

    /// How far back the oldest tap reads, frames.
    pub fn lookback(&self) -> usize {
        (self.kernel - 1) * self.dilation
    }

    pub fn reset(&mut self) {
        self.hist.fill(0.0);
        self.pos = 0;
    }

    /// `x`: `n` frames of `ci` at stride `stride`; `y`: `n` frames of `co`. `n` ≤ max block.
    pub fn process(&mut self, x: &[f32], stride: usize, y: &mut [f32], n: usize) {
        let (ci, co, ring) = (self.ci, self.co, self.ring);
        let start = self.pos;
        for f in 0..n {
            let src = &x[f * stride..f * stride + ci];
            let p = (start + f) % ring;
            self.hist[p * ci..(p + 1) * ci].copy_from_slice(src);
            self.hist[(p + ring) * ci..(p + ring + 1) * ci].copy_from_slice(src);
        }
        self.pos = (start + n) % ring;
        for k in 0..self.kernel {
            let back = self.dilation * (self.kernel - 1 - k);
            self.firsts[k] = (start + ring - back % ring) % ring;
        }
        let call = Call { w: &self.w, bias: self.bias.as_deref(), x: &self.hist, stride: ci, ci, co, firsts: &self.firsts, n };
        convolve(&call, y);
    }
}

/// Feature-wise linear modulation: a 1×1 convolution of the condition gives a scale (and a
/// shift) per channel.
pub struct Film {
    conv: Conv1x1,
    shift: bool,
    dim: usize,
    ss: Vec<f32>,
}

impl Film {
    pub fn read(w: &mut Weights, cond: usize, dim: usize, shift: bool, groups: usize, max_block: usize) -> Result<Self, String> {
        let out = if shift { 2 * dim } else { dim };
        let conv = Conv1x1::read(w, cond, out, true, groups)?;
        Ok(Self { conv, shift, dim, ss: vec![0.0; out * max_block] })
    }

    /// `x` (`n` frames of `dim`, stride `stride`) ← x·scale + shift.
    pub fn apply(&mut self, x: &mut [f32], stride: usize, cond: &[f32], n: usize) {
        let s = self.conv.co;
        self.conv.process(cond, self.conv.ci, &mut self.ss, n);
        let d = self.dim;
        for f in 0..n {
            let col = &self.ss[f * s..(f + 1) * s];
            let v = &mut x[f * stride..f * stride + d];
            if self.shift {
                for c in 0..d {
                    v[c] = v[c] * col[c] + col[d + c];
                }
            } else {
                for c in 0..d {
                    v[c] *= col[c];
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_dilated_convolution_reads_its_taps_back_in_time() {
        // One channel, three taps [a, b, c] at dilation 2: y[t] = a·x[t−4] + b·x[t−2] + c·x[t].
        let w = [0.5f32, 0.25, 1.0];
        let mut weights = Weights::new(&w);
        let mut conv = Conv1d::read(&mut weights, 1, 1, 3, 2, false, 1, 8).unwrap();
        let x: Vec<f32> = (1..=12).map(|v| v as f32).collect();
        let mut y = [0.0; 12];
        // In uneven blocks: the history carries across.
        let (mut at, sizes) = (0, [5usize, 3, 4]);
        for n in sizes {
            conv.process(&x[at..], 1, &mut y[at..], n);
            at += n;
        }
        for t in 0..12 {
            let past = |d: usize| if t >= d { x[t - d] } else { 0.0 };
            let want = 0.5 * past(4) + 0.25 * past(2) + past(0);
            assert_eq!(y[t], want, "t {t}");
        }
    }

    #[test]
    fn grouped_weights_stay_inside_their_groups() {
        // 4 → 4 in 2 groups: 2·2 weights per group, no bias.
        let w: Vec<f32> = (1..=8).map(|v| v as f32).collect();
        let conv = Conv1x1::read(&mut Weights::new(&w), 4, 4, false, 2).unwrap();
        let mut y = [0.0; 4];
        conv.process(&[1.0, 0.0, 0.0, 0.0], 4, &mut y, 1);
        // Input 0 feeds outputs 0 and 1 of group 0 only (weights (0,0)=1, (1,0)=3).
        assert_eq!(y, [1.0, 3.0, 0.0, 0.0]);
    }

    #[test]
    fn fast_tanh_is_close_to_tanh() {
        for i in -400..=400 {
            let x = i as f32 / 50.0;
            assert!((fast_tanh(x) - x.tanh()).abs() < 2e-3, "{x}");
        }
    }
}
