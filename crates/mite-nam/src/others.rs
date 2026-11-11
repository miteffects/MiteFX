//! The other architectures: LSTM (and A1's "lite" recurrent captures), ConvNet, Linear (an
//! impulse response), and the wrappers that hold whole models: SlimmableContainer (A2's sizes)
//! and Sequential.

use serde_json::Value;

use crate::layers::{fast_sigmoid, fast_tanh, sigmoid, Activation, Conv1d, Weights};
use crate::{load_net, Net};

struct Cell {
    input: usize,
    hidden: usize,
    /// 4·hidden × (input + hidden), row-major (gates i, f, g, o).
    w: Vec<f32>,
    b: Vec<f32>,
    xh0: Vec<f32>,
    c0: Vec<f32>,
    xh: Vec<f32>,
    c: Vec<f32>,
    ifgo: Vec<f32>,
}

impl Cell {
    fn read(w: &mut Weights, input: usize, hidden: usize) -> Result<Self, String> {
        let cols = input + hidden;
        let m = (0..4 * hidden * cols).map(|_| w.take()).collect::<Result<Vec<_>, _>>()?;
        let b = (0..4 * hidden).map(|_| w.take()).collect::<Result<Vec<_>, _>>()?;
        let mut xh0 = vec![0.0; cols];
        for v in &mut xh0[input..] {
            *v = w.take()?;
        }
        let c0 = (0..hidden).map(|_| w.take()).collect::<Result<Vec<_>, _>>()?;
        Ok(Self { input, hidden, w: m, b, xh: xh0.clone(), c: c0.clone(), xh0, c0, ifgo: vec![0.0; 4 * hidden] })
    }

    fn step(&mut self, x: &[f32], fast: bool) {
        let (inp, h) = (self.input, self.hidden);
        self.xh[..inp].copy_from_slice(&x[..inp]);
        let cols = inp + h;
        for r in 0..4 * h {
            let row = &self.w[r * cols..(r + 1) * cols];
            let mut acc = 0.0;
            for (a, b) in row.iter().zip(&self.xh) {
                acc += a * b;
            }
            self.ifgo[r] = acc + self.b[r];
        }
        type Curve = fn(f32) -> f32;
        let (sig, th): (Curve, Curve) = if fast { (fast_sigmoid, fast_tanh) } else { (sigmoid, f32::tanh) };
        for i in 0..h {
            self.c[i] = sig(self.ifgo[h + i]) * self.c[i] + sig(self.ifgo[i]) * th(self.ifgo[2 * h + i]);
        }
        for i in 0..h {
            self.xh[inp + i] = sig(self.ifgo[3 * h + i]) * th(self.c[i]);
        }
    }

    fn hidden_state(&self) -> &[f32] {
        &self.xh[self.input..]
    }

    fn reset(&mut self) {
        self.xh.copy_from_slice(&self.xh0);
        self.c.copy_from_slice(&self.c0);
    }
}

pub struct Lstm {
    in_ch: usize,
    out_ch: usize,
    cells: Vec<Cell>,
    head_w: Vec<f32>,
    head_b: Vec<f32>,
    fast: bool,
    prewarm: usize,
    hidden_copy: Vec<f32>,
}

impl Lstm {
    pub fn load(config: &Value, weights: &[f32], fast: bool, sample_rate: f64) -> Result<Self, String> {
        let get = |k: &str, d: Option<usize>| config.get(k).and_then(Value::as_u64).map(|v| v as usize).or(d).ok_or_else(|| format!("an LSTM without {k}"));
        let layers = get("num_layers", None)?;
        let input = get("input_size", None)?;
        let hidden = get("hidden_size", None)?;
        let in_ch = get("in_channels", Some(1))?;
        let out_ch = get("out_channels", Some(1))?;
        let mut w = Weights::new(weights);
        let cells = (0..layers).map(|i| Cell::read(&mut w, if i == 0 { input } else { hidden }, hidden)).collect::<Result<Vec<_>, _>>()?;
        let head_w = (0..out_ch * hidden).map(|_| w.take()).collect::<Result<Vec<_>, _>>()?;
        let head_b = (0..out_ch).map(|_| w.take()).collect::<Result<Vec<_>, _>>()?;
        if w.remaining() != 0 {
            return Err("the LSTM has weights left over".to_owned());
        }
        let prewarm = ((0.5 * sample_rate) as usize).max(1);
        Ok(Self { in_ch, out_ch, cells, head_w, head_b, fast, prewarm, hidden_copy: vec![0.0; hidden] })
    }
}

impl Net for Lstm {
    fn in_channels(&self) -> usize {
        self.in_ch
    }

    fn out_channels(&self) -> usize {
        self.out_ch
    }

    fn prewarm_samples(&self) -> usize {
        self.prewarm
    }

    fn reset(&mut self) {
        for c in &mut self.cells {
            c.reset();
        }
    }

    fn process(&mut self, x: &[f32], y: &mut [f32], n: usize) {
        let (ic, oc) = (self.in_ch, self.out_ch);
        for f in 0..n {
            let frame = &x[f * ic..(f + 1) * ic];
            let out = &mut y[f * oc..(f + 1) * oc];
            if self.cells.is_empty() {
                for (o, v) in out.iter_mut().enumerate() {
                    *v = frame.get(o).copied().unwrap_or(0.0);
                }
                continue;
            }
            self.cells[0].step(frame, self.fast);
            for i in 1..self.cells.len() {
                self.hidden_copy.copy_from_slice(self.cells[i - 1].hidden_state());
                self.cells[i].step(&self.hidden_copy, self.fast);
            }
            let h = self.cells.last().expect("a cell").hidden_state();
            let hn = h.len();
            for (o, v) in out.iter_mut().enumerate() {
                let acc: f32 = self.head_w[o * hn..(o + 1) * hn].iter().zip(h).map(|(a, b)| a * b).sum();
                *v = acc + self.head_b[o];
            }
        }
    }
}

struct ConvBlock {
    conv: Conv1d,
    /// Batch norm folded into a scale and offset per channel.
    norm: Option<(Vec<f32>, Vec<f32>)>,
    act: Activation,
    out: Vec<f32>,
}

pub struct ConvNet {
    in_ch: usize,
    out_ch: usize,
    blocks: Vec<ConvBlock>,
    head_w: Vec<f32>,
    head_b: Vec<f32>,
    prewarm: usize,
}

impl ConvNet {
    pub fn load(config: &Value, weights: &[f32], fast: bool, max: usize) -> Result<Self, String> {
        let channels = config.get("channels").and_then(Value::as_u64).ok_or("a ConvNet without channels")? as usize;
        let dilations: Vec<usize> = config.get("dilations").and_then(Value::as_array).ok_or("a ConvNet without dilations")?.iter().map(|d| d.as_u64().unwrap_or(1) as usize).collect();
        let batchnorm = config.get("batchnorm").and_then(Value::as_bool).unwrap_or(false);
        let act = Activation::from_json(config.get("activation").ok_or("a ConvNet without an activation")?, fast)?;
        let groups = config.get("groups").and_then(Value::as_u64).unwrap_or(1) as usize;
        let in_ch = config.get("in_channels").and_then(Value::as_u64).unwrap_or(1) as usize;
        let out_ch = config.get("out_channels").and_then(Value::as_u64).unwrap_or(1) as usize;
        let mut w = Weights::new(weights);
        let mut blocks = Vec::new();
        for (i, &d) in dilations.iter().enumerate() {
            let ci = if i == 0 { in_ch } else { channels };
            let conv = Conv1d::read(&mut w, ci, channels, 2, d, !batchnorm, groups, max)?;
            let norm = if batchnorm {
                let mut read = || (0..channels).map(|_| w.take()).collect::<Result<Vec<_>, _>>();
                let (mean, var, weight, bias) = (read()?, read()?, read()?, read()?);
                let eps = w.take()?;
                let scale: Vec<f32> = (0..channels).map(|c| weight[c] / (eps + var[c]).sqrt()).collect();
                let loc = (0..channels).map(|c| bias[c] - scale[c] * mean[c]).collect();
                Some((scale, loc))
            } else {
                None
            };
            blocks.push(ConvBlock { conv, norm, act: act.clone(), out: vec![0.0; channels * max] });
        }
        let head_w = (0..out_ch * channels).map(|_| w.take()).collect::<Result<Vec<_>, _>>()?;
        let head_b = (0..out_ch).map(|_| w.take()).collect::<Result<Vec<_>, _>>()?;
        if w.remaining() != 0 {
            return Err("the ConvNet has weights left over".to_owned());
        }
        let prewarm = 1 + dilations.iter().sum::<usize>();
        Ok(Self { in_ch, out_ch, blocks, head_w, head_b, prewarm })
    }
}

impl Net for ConvNet {
    fn in_channels(&self) -> usize {
        self.in_ch
    }

    fn out_channels(&self) -> usize {
        self.out_ch
    }

    fn prewarm_samples(&self) -> usize {
        self.prewarm
    }

    fn reset(&mut self) {
        for b in &mut self.blocks {
            b.conv.reset();
        }
    }

    fn process(&mut self, x: &[f32], y: &mut [f32], n: usize) {
        for i in 0..self.blocks.len() {
            let (done, rest) = self.blocks.split_at_mut(i);
            let block = &mut rest[0];
            let co = block.conv.co;
            match done.last() {
                None => block.conv.process(x, self.in_ch, &mut block.out, n),
                Some(prev) => block.conv.process(&prev.out, prev.conv.co, &mut block.out, n),
            }
            if let Some((scale, loc)) = &block.norm {
                for f in 0..n {
                    for c in 0..co {
                        let v = &mut block.out[f * co + c];
                        *v = *v * scale[c] + loc[c];
                    }
                }
            }
            block.act.apply(&mut block.out[..co * n]);
        }
        let last = self.blocks.last().map(|b| (&b.out, b.conv.co));
        let (src, ch) = match last {
            Some((o, c)) => (o.as_slice(), c),
            None => (x, self.in_ch),
        };
        let oc = self.out_ch;
        for f in 0..n {
            for o in 0..oc {
                let mut acc = 0.0;
                for (a, b) in self.head_w[o * ch..(o + 1) * ch].iter().zip(&src[f * ch..(f + 1) * ch]) {
                    acc += a * b;
                }
                y[f * oc + o] = acc + self.head_b[o];
            }
        }
    }
}

/// An impulse response with an optional bias (mono).
pub struct Linear {
    conv: Conv1d,
}

impl Linear {
    pub fn load(config: &Value, weights: &[f32], max: usize) -> Result<Self, String> {
        let rf = config.get("receptive_field").and_then(Value::as_u64).ok_or("a Linear model without a receptive field")? as usize;
        let bias = config.get("bias").and_then(Value::as_bool).unwrap_or(false);
        let ch = (config.get("in_channels").and_then(Value::as_u64).unwrap_or(1), config.get("out_channels").and_then(Value::as_u64).unwrap_or(1));
        if ch != (1, 1) || rf == 0 || weights.len() != rf + usize::from(bias) {
            return Err("only mono Linear models are supported".to_owned());
        }
        // Tap k (of rf) reads rf−1−k samples back: the response reversed.
        let taps: Vec<f32> = (0..rf).map(|k| weights[rf - 1 - k]).collect();
        let bias = bias.then(|| vec![weights[rf]]);
        Ok(Self { conv: Conv1d::from_taps(1, 1, taps, bias, max) })
    }
}

impl Net for Linear {
    fn in_channels(&self) -> usize {
        1
    }

    fn out_channels(&self) -> usize {
        1
    }

    fn prewarm_samples(&self) -> usize {
        0
    }

    fn reset(&mut self) {
        self.conv.reset();
    }

    fn process(&mut self, x: &[f32], y: &mut [f32], n: usize) {
        self.conv.process(x, 1, y, n);
    }
}

/// Whole models in a row.
pub struct Sequential {
    models: Vec<Box<dyn Net>>,
    bufs: Vec<Vec<f32>>,
    prewarm: usize,
}

impl Sequential {
    pub fn load(config: &Value, fast: bool, max: usize) -> Result<Self, String> {
        let list = config.get("models").and_then(Value::as_array).filter(|m| !m.is_empty()).ok_or("a Sequential model without models")?;
        let models = list.iter().map(|m| load_net(m, fast, max, 1.0).map(|(net, _)| net)).collect::<Result<Vec<_>, _>>()?;
        for pair in models.windows(2) {
            if pair[0].out_channels() != pair[1].in_channels() {
                return Err("a Sequential model's parts do not connect".to_owned());
            }
        }
        let bufs = models.iter().take(models.len() - 1).map(|m| vec![0.0; m.out_channels() * max]).collect();
        let prewarm = models.iter().map(|m| m.prewarm_samples()).sum();
        Ok(Self { models, bufs, prewarm })
    }
}

impl Net for Sequential {
    fn in_channels(&self) -> usize {
        self.models[0].in_channels()
    }

    fn out_channels(&self) -> usize {
        self.models[self.models.len() - 1].out_channels()
    }

    fn prewarm_samples(&self) -> usize {
        self.prewarm
    }

    fn reset(&mut self) {
        for m in &mut self.models {
            m.reset();
        }
    }

    fn process(&mut self, x: &[f32], y: &mut [f32], n: usize) {
        let last = self.models.len() - 1;
        for i in 0..=last {
            let split = i.min(self.bufs.len());
            let (done, rest) = self.bufs.split_at_mut(split);
            let src: &[f32] = if i == 0 { x } else { &done[i - 1] };
            if i == last {
                self.models[i].process(src, y, n);
            } else {
                self.models[i].process(src, &mut rest[0], n);
            }
        }
    }
}
