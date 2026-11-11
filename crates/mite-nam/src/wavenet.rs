//! WaveNet: the architecture of NAM's A1 captures and, with per-layer kernel sizes, a bottleneck,
//! grouped convolutions, gating or blending, FiLM and a head convolution, of A2.
//!
//! A layer array rechannels its input, runs its layers (each: a dilated convolution of the
//! residual stream plus a 1×1 "mixin" of the condition, an activation, a 1×1 back into the
//! residual stream and a skip output to the head), sums the skips and convolves them into its
//! head output. Arrays chain: each takes the previous array's residual stream and head sum.

use serde_json::Value;

use crate::layers::{Activation, Conv1d, Conv1x1, Film, Weights};
use crate::{Net, load_net};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Gating {
    None,
    Gated,
    Blended,
}

#[derive(Clone, Copy, Default, Debug)]
struct FilmSpec {
    active: bool,
    shift: bool,
    groups: usize,
}

/// The eight places a FiLM can sit in a layer, in the order their weights are stored.
const FILM_KEYS: [&str; 8] = [
    "conv_pre_film",
    "conv_post_film",
    "input_mixin_pre_film",
    "input_mixin_post_film",
    "activation_pre_film",
    "activation_post_film",
    "layer1x1_post_film",
    "head1x1_post_film",
];
const CONV_PRE: usize = 0;
const CONV_POST: usize = 1;
const MIXIN_PRE: usize = 2;
const MIXIN_POST: usize = 3;
const ACT_PRE: usize = 4;
const ACT_POST: usize = 5;
const L1X1_POST: usize = 6;
const H1X1_POST: usize = 7;

/// One layer array's description, from the config.
struct ArraySpec {
    input_size: usize,
    condition_size: usize,
    head_size: usize,
    head_kernel: usize,
    head_dilation: usize,
    head_bias: bool,
    channels: usize,
    bottleneck: usize,
    kernels: Vec<usize>,
    dilations: Vec<usize>,
    activations: Vec<Activation>,
    gating: Vec<Gating>,
    secondary: Vec<Activation>,
    groups_input: usize,
    groups_mixin: usize,
    layer1x1: (bool, usize),
    head1x1: (bool, usize, usize),
    films: [FilmSpec; 8],
}

fn uint(v: &Value, key: &str) -> Result<usize, String> {
    v.get(key).and_then(Value::as_u64).map(|x| x as usize).ok_or_else(|| format!("a layer without {key}"))
}

fn uint_or(v: &Value, key: &str, default: usize) -> usize {
    v.get(key).and_then(Value::as_u64).map_or(default, |x| x as usize)
}

fn flag(v: &Value, key: &str) -> Option<bool> {
    v.get(key).and_then(Value::as_bool)
}

impl ArraySpec {
    fn parse(lc: &Value, fast: bool) -> Result<Self, String> {
        let channels = uint(lc, "channels")?;
        let bottleneck = uint_or(lc, "bottleneck", channels);
        let dilations: Vec<usize> = lc.get("dilations").and_then(Value::as_array).ok_or("a layer array without dilations")?.iter().map(|d| d.as_u64().unwrap_or(1) as usize).collect();
        let layers = dilations.len();
        if layers == 0 {
            return Err("a layer array without layers".to_owned());
        }
        let kernels = match (lc.get("kernel_sizes"), lc.get("kernel_size")) {
            (Some(Value::Array(ks)), None) => ks.iter().map(|k| k.as_u64().unwrap_or(0) as usize).collect(),
            (None, Some(k)) => vec![k.as_u64().unwrap_or(0) as usize; layers],
            _ => return Err("a layer array needs kernel_size or kernel_sizes".to_owned()),
        };
        if kernels.len() != layers {
            return Err("kernel sizes and dilations differ in number".to_owned());
        }
        let activations = match lc.get("activation") {
            Some(Value::Array(a)) => a.iter().map(|x| Activation::from_json(x, fast)).collect::<Result<Vec<_>, _>>()?,
            Some(a) => vec![Activation::from_json(a, fast)?; layers],
            None => return Err("a layer array without an activation".to_owned()),
        };
        if activations.len() != layers {
            return Err("activations and dilations differ in number".to_owned());
        }
        let mode = |s: &str| match s {
            "gated" => Ok(Gating::Gated),
            "blended" => Ok(Gating::Blended),
            "none" => Ok(Gating::None),
            other => Err(format!("unknown gating mode {other}")),
        };
        let secondary_for = |i: usize| -> Result<Activation, String> {
            match lc.get("secondary_activation") {
                Some(Value::Array(a)) => a.get(i).map(|x| Activation::from_json(x, fast)).unwrap_or(Ok(Activation::Sigmoid)),
                Some(v @ (Value::String(_) | Value::Object(_))) => Activation::from_json(v, fast),
                _ => Ok(Activation::Sigmoid),
            }
        };
        let (gating, secondary) = match (lc.get("gating_mode"), flag(lc, "gated")) {
            (Some(Value::Array(a)), _) => {
                let g = a.iter().map(|m| mode(m.as_str().unwrap_or("none"))).collect::<Result<Vec<_>, _>>()?;
                let s = g.iter().enumerate().map(|(i, m)| if *m == Gating::None { Ok(Activation::Identity) } else { secondary_for(i) }).collect::<Result<Vec<_>, _>>()?;
                (g, s)
            }
            (Some(Value::String(m)), _) => {
                let m = mode(m)?;
                let s = if m == Gating::None { Activation::Identity } else { secondary_for(0)? };
                (vec![m; layers], vec![s; layers])
            }
            (_, Some(true)) => (vec![Gating::Gated; layers], vec![Activation::Sigmoid; layers]),
            _ => (vec![Gating::None; layers], vec![Activation::Identity; layers]),
        };
        if gating.len() != layers {
            return Err("gating modes and dilations differ in number".to_owned());
        }
        let layer1x1 = match lc.get("layer1x1") {
            Some(o @ Value::Object(_)) => (flag(o, "active").unwrap_or(true), uint_or(o, "groups", 1)),
            _ => (true, 1),
        };
        let head1x1 = match lc.get("head1x1") {
            Some(o @ Value::Object(_)) => (flag(o, "active").unwrap_or(false), uint_or(o, "out_channels", channels), uint_or(o, "groups", 1)),
            _ => (false, channels, 1),
        };
        let (head_size, head_kernel, head_dilation, head_bias) = match lc.get("head") {
            Some(h @ Value::Object(_)) => (uint(h, "out_channels")?, uint(h, "kernel_size")?, uint_or(h, "head_dilation", 1), flag(h, "bias").unwrap_or(false)),
            _ => (uint(lc, "head_size")?, 1, 1, flag(lc, "head_bias").unwrap_or(false)),
        };
        let mut films = [FilmSpec::default(); 8];
        for (spec, key) in films.iter_mut().zip(FILM_KEYS) {
            if let Some(o @ Value::Object(_)) = lc.get(key) {
                *spec = FilmSpec { active: flag(o, "active").unwrap_or(true), shift: flag(o, "shift").unwrap_or(true), groups: uint_or(o, "groups", 1) };
            }
        }
        if !layer1x1.0 && bottleneck != channels {
            return Err("without layer1x1 the bottleneck must equal the channels".to_owned());
        }
        Ok(Self {
            input_size: uint(lc, "input_size")?,
            condition_size: uint(lc, "condition_size")?,
            head_size,
            head_kernel: head_kernel.max(1),
            head_dilation: head_dilation.max(1),
            head_bias,
            channels,
            bottleneck,
            kernels,
            dilations,
            activations,
            gating,
            secondary,
            groups_input: uint_or(lc, "groups_input", 1),
            groups_mixin: uint_or(lc, "groups_input_mixin", 1),
            layer1x1,
            head1x1,
            films,
        })
    }

    /// Each layer's skip output width.
    fn head_out(&self) -> usize {
        if self.head1x1.0 { self.head1x1.1 } else { self.bottleneck }
    }

    fn receptive_field(&self) -> usize {
        let layers: usize = self.kernels.iter().zip(&self.dilations).map(|(k, d)| d * (k - 1)).sum();
        layers + self.head_dilation * (self.head_kernel - 1)
    }
}

struct Layer {
    channels: usize,
    bottleneck: usize,
    gating: Gating,
    conv: Conv1d,
    mixin: Conv1x1,
    layer1x1: Option<Conv1x1>,
    head1x1: Option<Conv1x1>,
    films: [Option<Film>; 8],
    act: Activation,
    secondary: Activation,
    /// The residual input as the convolution sees it (after a pre-FiLM).
    conv_in: Vec<f32>,
    /// The convolution's output, then the summed pre-activation (2·bottleneck when gated).
    z: Vec<f32>,
    mix: Vec<f32>,
    mix_in: Vec<f32>,
    /// The activated bottleneck (gated and blended layers).
    top: Vec<f32>,
    /// The single-channel scratch a gating activation runs in.
    gate_a: Vec<f32>,
    gate_b: Vec<f32>,
    l1: Vec<f32>,
    h1: Vec<f32>,
}

impl Layer {
    fn read(w: &mut Weights, a: &ArraySpec, i: usize, max: usize) -> Result<Self, String> {
        let (c, b, cond) = (a.channels, a.bottleneck, a.condition_size);
        let gating = a.gating[i];
        let zc = if gating == Gating::None { b } else { 2 * b };
        let conv = Conv1d::read(w, c, zc, a.kernels[i], a.dilations[i], true, a.groups_input, max)?;
        let mixin = Conv1x1::read(w, cond, zc, false, a.groups_mixin)?;
        let layer1x1 = if a.layer1x1.0 { Some(Conv1x1::read(w, b, c, true, a.layer1x1.1)?) } else { None };
        let head1x1 = if a.head1x1.0 { Some(Conv1x1::read(w, b, a.head1x1.1, true, a.head1x1.2)?) } else { None };
        let dims = [c, zc, cond, zc, zc, b, c, a.head1x1.1];
        let mut films: [Option<Film>; 8] = Default::default();
        for k in 0..8 {
            let s = a.films[k];
            let allowed = match k {
                L1X1_POST => a.layer1x1.0,
                H1X1_POST => a.head1x1.0,
                _ => true,
            };
            if s.active && allowed {
                films[k] = Some(Film::read(w, cond, dims[k], s.shift, s.groups, max)?);
            }
        }
        Ok(Self {
            channels: c,
            bottleneck: b,
            gating,
            conv,
            mixin,
            layer1x1,
            head1x1,
            films,
            act: a.activations[i].clone(),
            secondary: a.secondary[i].clone(),
            conv_in: vec![0.0; c * max],
            z: vec![0.0; zc * max],
            mix: vec![0.0; zc * max],
            mix_in: vec![0.0; cond * max],
            top: vec![0.0; b * max],
            gate_a: vec![0.0; b],
            gate_b: vec![0.0; b],
            l1: vec![0.0; c * max],
            h1: vec![0.0; a.head1x1.1 * max],
        })
    }

    /// `x` → `next` (residual), adding the skip output into `head`.
    fn process(&mut self, x: &[f32], cond: &[f32], cc: usize, next: &mut [f32], head: &mut [f32], n: usize) {
        let (c, b) = (self.channels, self.bottleneck);
        let zc = self.mixin.co;
        // The dilated convolution (of the FiLMed input if there is a pre-FiLM).
        if let Some(film) = &mut self.films[CONV_PRE] {
            self.conv_in[..c * n].copy_from_slice(&x[..c * n]);
            film.apply(&mut self.conv_in, c, cond, n);
            self.conv.process(&self.conv_in, c, &mut self.z, n);
        } else {
            self.conv.process(x, c, &mut self.z, n);
        }
        if let Some(film) = &mut self.films[CONV_POST] {
            film.apply(&mut self.z, zc, cond, n);
        }
        // The condition's mixin.
        if let Some(film) = &mut self.films[MIXIN_PRE] {
            self.mix_in[..cc * n].copy_from_slice(&cond[..cc * n]);
            film.apply(&mut self.mix_in, cc, cond, n);
            self.mixin.process(&self.mix_in, cc, &mut self.mix, n);
        } else {
            self.mixin.process(cond, cc, &mut self.mix, n);
        }
        if let Some(film) = &mut self.films[MIXIN_POST] {
            film.apply(&mut self.mix, zc, cond, n);
        }
        for (z, m) in self.z[..zc * n].iter_mut().zip(&self.mix[..zc * n]) {
            *z += *m;
        }
        if let Some(film) = &mut self.films[ACT_PRE] {
            film.apply(&mut self.z, zc, cond, n);
        }
        // The activation: plain, gated (act(a)·σ(b)) or blended (α·act(a) + (1−α)·a, α = σ(b)).
        let out: &mut [f32] = match self.gating {
            Gating::None => {
                self.act.apply(&mut self.z[..b * n]);
                &mut self.z
            }
            Gating::Gated | Gating::Blended => {
                for f in 0..n {
                    let col = &self.z[f * zc..(f + 1) * zc];
                    self.gate_a.copy_from_slice(&col[..b]);
                    self.gate_b.copy_from_slice(&col[b..]);
                    self.act.apply(&mut self.gate_a);
                    self.secondary.apply(&mut self.gate_b);
                    let dst = &mut self.top[f * b..(f + 1) * b];
                    let parts = dst.iter_mut().zip(&self.gate_a).zip(&self.gate_b).zip(&col[..b]);
                    if self.gating == Gating::Gated {
                        parts.for_each(|(((d, a), g), _)| *d = a * g);
                    } else {
                        parts.for_each(|(((d, a), alpha), raw)| *d = alpha * a + (1.0 - alpha) * raw);
                    }
                }
                &mut self.top
            }
        };
        if let Some(film) = &mut self.films[ACT_POST] {
            film.apply(out, b, cond, n);
        }
        let out: &[f32] = out;
        // The skip to the head.
        if let Some(h1) = &self.head1x1 {
            h1.process(out, b, &mut self.h1, n);
            let ho = h1.co;
            if let Some(film) = &mut self.films[H1X1_POST] {
                film.apply(&mut self.h1, ho, cond, n);
            }
            for (h, v) in head[..ho * n].iter_mut().zip(&self.h1[..ho * n]) {
                *h += *v;
            }
        } else {
            for (h, v) in head[..b * n].iter_mut().zip(&out[..b * n]) {
                *h += *v;
            }
        }
        // The residual: input plus the 1×1 back from the bottleneck.
        if let Some(l1) = &self.layer1x1 {
            l1.process(out, b, &mut self.l1, n);
            if let Some(film) = &mut self.films[L1X1_POST] {
                film.apply(&mut self.l1, c, cond, n);
            }
            for i in 0..c * n {
                next[i] = x[i] + self.l1[i];
            }
        } else {
            next[..c * n].copy_from_slice(&x[..c * n]);
        }
    }

    fn reset(&mut self) {
        self.conv.reset();
    }
}

struct LayerArray {
    channels: usize,
    head_out: usize,
    head_size: usize,
    rechannel: Conv1x1,
    layers: Vec<Layer>,
    head_rechannel: Conv1d,
    /// Residual stream ping-pong; `cur` holds the array's output after a block.
    cur: Vec<f32>,
    alt: Vec<f32>,
    head_in: Vec<f32>,
    head: Vec<f32>,
}

impl LayerArray {
    fn read(w: &mut Weights, a: &ArraySpec, max: usize) -> Result<Self, String> {
        let rechannel = Conv1x1::read(w, a.input_size, a.channels, false, 1)?;
        let layers = (0..a.dilations.len()).map(|i| Layer::read(w, a, i, max)).collect::<Result<Vec<_>, _>>()?;
        let head_rechannel = Conv1d::read(w, a.head_out(), a.head_size, a.head_kernel, a.head_dilation, a.head_bias, 1, max)?;
        Ok(Self {
            channels: a.channels,
            head_out: a.head_out(),
            head_size: a.head_size,
            rechannel,
            layers,
            head_rechannel,
            cur: vec![0.0; a.channels * max],
            alt: vec![0.0; a.channels * max],
            head_in: vec![0.0; a.head_out() * max],
            head: vec![0.0; a.head_size * max],
        })
    }

    /// `x` (`xc` channels) and the condition (`cc` channels); `head_init` is the previous
    /// array's head output (the skip sum starts there), or none for the first array.
    fn process(&mut self, x: &[f32], xc: usize, cond: &[f32], cc: usize, head_init: Option<&[f32]>, n: usize) {
        let (c, ho) = (self.channels, self.head_out);
        self.rechannel.process(x, xc, &mut self.cur, n);
        match head_init {
            Some(h) => self.head_in[..ho * n].copy_from_slice(&h[..ho * n]),
            None => self.head_in[..ho * n].fill(0.0),
        }
        for layer in &mut self.layers {
            layer.process(&self.cur, cond, cc, &mut self.alt, &mut self.head_in, n);
            std::mem::swap(&mut self.cur, &mut self.alt);
        }
        let _ = c;
        self.head_rechannel.process(&self.head_in, ho, &mut self.head, n);
    }

    fn reset(&mut self) {
        for l in &mut self.layers {
            l.reset();
        }
        self.head_rechannel.reset();
    }
}

/// The optional head after the last array: activation, then a convolution, per stage.
struct PostHead {
    stages: Vec<(Activation, Conv1d)>,
    bufs: Vec<Vec<f32>>,
}

pub struct WaveNet {
    in_ch: usize,
    out_ch: usize,
    arrays: Vec<LayerArray>,
    head_scale: f32,
    post: Option<PostHead>,
    condition: Option<Box<dyn Net>>,
    cond: Vec<f32>,
    cond_ch: usize,
    scaled: Vec<f32>,
    prewarm: usize,
}

impl WaveNet {
    pub fn load(config: &Value, weights: &[f32], fast: bool, max: usize) -> Result<Self, String> {
        let condition = match config.get("condition_dsp") {
            Some(c @ Value::Object(_)) => Some(load_net(c, fast, max, 1.0)?.0),
            _ => None,
        };
        let specs = config.get("layers").and_then(Value::as_array).ok_or("a WaveNet without layers")?.iter().map(|l| ArraySpec::parse(l, fast)).collect::<Result<Vec<_>, _>>()?;
        let first = specs.first().ok_or("a WaveNet without layer arrays")?;
        let in_ch = uint_or(config, "in_channels", 1);
        let cond_ch = condition.as_ref().map_or(in_ch, |c| c.out_channels());
        for (i, s) in specs.iter().enumerate() {
            if s.condition_size != cond_ch {
                return Err(format!("layer array {i} expects {} condition channels, gets {cond_ch}", s.condition_size));
            }
            if i > 0 && s.input_size != specs[i - 1].channels {
                return Err("a layer array's input does not match the one before".to_owned());
            }
            if i > 0 && s.head_out() != specs[i - 1].head_size {
                return Err("a layer array's head does not match the one before".to_owned());
            }
        }
        if first.input_size != in_ch {
            return Err("the first layer array's input does not match the model's".to_owned());
        }
        let mut w = Weights::new(weights);
        let arrays = specs.iter().map(|s| LayerArray::read(&mut w, s, max)).collect::<Result<Vec<_>, _>>()?;
        let last_head = specs.last().map_or(1, |s| s.head_size);
        let post = match config.get("head") {
            Some(h @ Value::Object(_)) => {
                let channels = uint(h, "channels")?;
                let out = uint(h, "out_channels")?;
                let kernels: Vec<usize> = h.get("kernel_sizes").and_then(Value::as_array).ok_or("a head without kernel sizes")?.iter().map(|k| k.as_u64().unwrap_or(1) as usize).collect();
                let act = Activation::from_json(h.get("activation").ok_or("a head without an activation")?, fast)?;
                let mut stages = Vec::new();
                let mut bufs = Vec::new();
                let mut cin = last_head;
                for (i, &k) in kernels.iter().enumerate() {
                    let cout = if i + 1 == kernels.len() { out } else { channels };
                    stages.push((act.clone(), Conv1d::read(&mut w, cin, cout, k.max(1), 1, true, 1, max)?));
                    bufs.push(vec![0.0; cout * max]);
                    cin = cout;
                }
                Some(PostHead { stages, bufs })
            }
            _ => None,
        };
        let head_scale = w.take()?;
        if w.remaining() != 0 {
            return Err(format!("the file has {} weights more than its layers use", w.remaining()));
        }
        let out_ch = post.as_ref().and_then(|p| p.stages.last()).map_or(last_head, |(_, c)| c.co);
        let mut prewarm = condition.as_ref().map_or(1, |c| c.prewarm_samples());
        prewarm += specs.iter().map(ArraySpec::receptive_field).sum::<usize>();
        if let Some(p) = &post {
            prewarm += p.stages.iter().map(|(_, c)| c.kernel - 1).sum::<usize>();
        }
        Ok(Self { in_ch, out_ch, arrays, head_scale, post, condition, cond: vec![0.0; cond_ch * max], cond_ch, scaled: vec![0.0; last_head * max], prewarm })
    }
}

impl Net for WaveNet {
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
        for a in &mut self.arrays {
            a.reset();
        }
        if let Some(p) = &mut self.post {
            for (_, c) in &mut p.stages {
                c.reset();
            }
        }
        if let Some(c) = &mut self.condition {
            c.reset();
        }
    }

    fn process(&mut self, x: &[f32], y: &mut [f32], n: usize) {
        let in_ch = self.in_ch;
        let cc = self.cond_ch;
        if let Some(c) = &mut self.condition {
            c.process(x, &mut self.cond, n);
        } else {
            self.cond[..cc * n].copy_from_slice(&x[..cc * n]);
        }
        for i in 0..self.arrays.len() {
            let (before, rest) = self.arrays.split_at_mut(i);
            let array = &mut rest[0];
            match before.last() {
                None => array.process(x, in_ch, &self.cond, cc, None, n),
                Some(prev) => array.process(&prev.cur, prev.channels, &self.cond, cc, Some(&prev.head), n),
            }
        }
        let last = self.arrays.last().expect("at least one array");
        let hs = last.head_size;
        let scale = self.head_scale;
        match &mut self.post {
            None => {
                for (o, h) in y[..self.out_ch * n].iter_mut().zip(&last.head) {
                    *o = scale * h;
                }
            }
            Some(p) => {
                for i in 0..hs * n {
                    self.scaled[i] = scale * last.head[i];
                }
                for s in 0..p.stages.len() {
                    let (done, rest) = p.bufs.split_at_mut(s);
                    let (act, conv) = &mut p.stages[s];
                    let (src, ci): (&mut [f32], usize) = if s == 0 { (&mut self.scaled, hs) } else { (&mut done[s - 1], conv.ci) };
                    act.apply(&mut src[..ci * n]);
                    conv.process(src, ci, &mut rest[0], n);
                }
                let out = p.bufs.last().expect("a head stage");
                y[..self.out_ch * n].copy_from_slice(&out[..self.out_ch * n]);
            }
        }
    }
}
