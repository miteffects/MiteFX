//! The quality gates every plugin owes (BLUEPRINT §10), run through the plugin's real CLAP entry
//! by [`crate::testing::ClapInstance`]: block-size invariance, sample rates 44.1–384 kHz, NaN/Inf
//! recovery through `reset()`, and a step on every continuous parameter (the ramp test).
//!
//! A plugin's test builds a [`Gates`] (optionally with a state that switches its features on, so
//! the parameters have something to act on) and asserts on what each check returns, naming the
//! exceptions it allows and why.

use clap_sys::entry::clap_plugin_entry;

use crate::testing::{ClapInstance, ParamChange, ParamInfo};

/// The block sizes §10 names, then a run of variable ones.
pub const BLOCK_SIZES: [usize; 6] = [1, 7, 64, 128, 4096, 8192];
pub const SAMPLE_RATES: [f64; 8] = [44_100.0, 48_000.0, 88_200.0, 96_000.0, 176_400.0, 192_000.0, 352_800.0, 384_000.0];
const MAX_BLOCK: u32 = 8192;

pub struct Gates {
    entry: &'static clap_plugin_entry,
    id: String,
    state: Option<Vec<u8>>,
    pub sample_rate: f64,
}

/// What a parameter step did: the largest second difference of the output while the parameter
/// moves, against the larger of the settled levels before and after (dB; 0 means no worse than
/// the sound on either side, a click shows as a positive figure).
#[derive(Debug, Clone)]
pub struct Step {
    pub param: String,
    pub up_db: f64,
    pub down_db: f64,
}

impl Step {
    pub fn worst_db(&self) -> f64 {
        self.up_db.max(self.down_db)
    }
}

/// A stereo test signal: a 220 Hz tone at −12 dBFS with a little noise (so dynamics and
/// detectors have something to follow) and a slow swell, `n` samples at `rate`.
pub fn test_signal(rate: f64, n: usize) -> [Vec<f32>; 2] {
    let mut seed = 0x2468_ace1u32;
    let mut l = Vec::with_capacity(n);
    let mut r = Vec::with_capacity(n);
    for i in 0..n {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        let noise = (seed as f64 / u32::MAX as f64 * 2.0 - 1.0) * 0.002;
        let t = i as f64 / rate;
        let swell = 0.75 + 0.25 * (std::f64::consts::TAU * 0.7 * t).sin();
        let tone = 0.25 * swell * (std::f64::consts::TAU * 220.0 * t).sin();
        l.push((tone + noise) as f32);
        r.push((0.9 * tone - noise) as f32);
    }
    [l, r]
}

impl Gates {
    pub fn new(entry: &'static clap_plugin_entry, id: &str) -> Self {
        Self { entry, id: id.to_owned(), state: None, sample_rate: 48_000.0 }
    }

    /// Loads `state` (a saved state's JSON) into every instance first.
    pub fn with_state(mut self, state: Vec<u8>) -> Self {
        self.state = Some(state);
        self
    }

    fn instance(&self, rate: f64) -> ClapInstance {
        let mut p = ClapInstance::new(self.entry, &self.id);
        if let Some(s) = &self.state {
            assert!(p.load(s), "the state did not load");
        }
        p.activate(rate, MAX_BLOCK);
        p
    }

    /// Runs `input` through a fresh instance in blocks of the sizes `blocks` yields (cycled).
    fn run(&self, rate: f64, input: &[Vec<f32>; 2], blocks: &mut dyn FnMut() -> usize) -> Vec<Vec<f32>> {
        let mut p = self.instance(rate);
        let n = input[0].len();
        let mut out = vec![Vec::with_capacity(n), Vec::with_capacity(n)];
        let mut at = 0;
        while at < n {
            let len = blocks().clamp(1, MAX_BLOCK as usize).min(n - at);
            let mut block = vec![input[0][at..at + len].to_vec(), input[1][at..at + len].to_vec()];
            p.process(&mut block, &[]);
            out[0].extend_from_slice(&block[0]);
            out[1].extend_from_slice(&block[1]);
            at += len;
        }
        out
    }

    /// The largest difference (absolute) between the output at 512-sample blocks and at each of
    /// [`BLOCK_SIZES`] and a random run of sizes, over `seconds` of the test signal.
    pub fn block_sizes(&self, seconds: f64) -> Vec<(String, f32)> {
        let input = test_signal(self.sample_rate, (seconds * self.sample_rate) as usize);
        let reference = self.run(self.sample_rate, &input, &mut || 512);
        let diff = |o: &[Vec<f32>]| o.iter().zip(&reference).flat_map(|(a, b)| a.iter().zip(b).map(|(x, y)| (x - y).abs())).fold(0.0f32, f32::max);
        let mut out: Vec<(String, f32)> = BLOCK_SIZES.iter().map(|&b| (format!("{b}"), diff(&self.run(self.sample_rate, &input, &mut || b)))).collect();
        let mut seed = 0x9e37_79b9u32;
        let random = self.run(self.sample_rate, &input, &mut || {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            1 + (seed % 3000) as usize
        });
        out.push(("random".to_owned(), diff(&random)));
        out
    }

    /// At each of [`SAMPLE_RATES`]: half a second of the test signal; the output's peak (finite,
    /// or the check failed) and the latency reported.
    pub fn sample_rates(&self) -> Vec<(f64, Result<(f32, u32), String>)> {
        SAMPLE_RATES
            .iter()
            .map(|&rate| {
                let input = test_signal(rate, (0.5 * rate) as usize);
                let p = self.instance(rate);
                let latency = p.latency();
                drop(p);
                let out = self.run(rate, &input, &mut || 1024);
                let bad = out.iter().flatten().any(|v| !v.is_finite());
                let peak = out.iter().flatten().fold(0.0f32, |m, v| m.max(v.abs()));
                (rate, if bad { Err("a non-finite sample".to_owned()) } else { Ok((peak, latency)) })
            })
            .collect()
    }

    /// NaN and ±Inf in the input for a block, then `reset()`, then a second of the test signal:
    /// whether every output sample after the reset is finite, and the peak of that second.
    pub fn nan_recovery(&self) -> (bool, f32) {
        let rate = self.sample_rate;
        let mut p = self.instance(rate);
        let warm = test_signal(rate, 4096);
        let mut block = vec![warm[0].clone(), warm[1].clone()];
        p.process(&mut block, &[]);
        let mut poison = vec![vec![0.25f32; 512], vec![0.25f32; 512]];
        for (i, v) in poison[0].iter_mut().enumerate() {
            *v = match i % 3 {
                0 => f32::NAN,
                1 => f32::INFINITY,
                _ => f32::NEG_INFINITY,
            };
        }
        poison[1][100] = f32::NAN;
        p.process(&mut poison, &[]);
        p.reset();
        let input = test_signal(rate, rate as usize);
        let mut finite = true;
        let mut peak = 0.0f32;
        for k in 0..input[0].len() / 512 {
            let mut b = vec![input[0][k * 512..(k + 1) * 512].to_vec(), input[1][k * 512..(k + 1) * 512].to_vec()];
            p.process(&mut b, &[]);
            finite &= b.iter().flatten().all(|v| v.is_finite());
            peak = b.iter().flatten().filter(|v| v.is_finite()).fold(peak, |m, v| m.max(v.abs()));
        }
        (finite, peak)
    }

    /// The continuous parameters a host would automate (not stepped, hidden, read-only or the
    /// bypass), minus `skip`.
    pub fn continuous(&self, skip: &[&str]) -> Vec<ParamInfo> {
        let p = ClapInstance::new(self.entry, &self.id);
        p.params().into_iter().filter(|i| !i.stepped && !i.internal && i.max > i.min && !skip.contains(&i.name.as_str())).collect()
    }

    /// Steps `param` from its minimum to its maximum (and back, on a second instance) in one
    /// sample, with the test signal playing, and measures the click (see [`Step`]).
    pub fn step(&self, param: &ParamInfo) -> Step {
        let rate = self.sample_rate;
        let n = (0.9 * rate) as usize;
        let jump = (0.4 * rate) as usize + 37;
        let window = (0.1 * rate) as usize;
        let input = test_signal(rate, n);
        let measure = |from: f64, to: f64| -> f64 {
            let mut p = self.instance(rate);
            let mut out: Vec<f32> = Vec::with_capacity(n);
            let mut at = 0;
            let block = 256;
            let mut first = true;
            while at < n {
                let len = block.min(n - at);
                let mut changes = Vec::new();
                if first {
                    changes.push(ParamChange { offset: 0, id: param.id, value: from });
                    first = false;
                }
                if (at..at + len).contains(&jump) {
                    changes.push(ParamChange { offset: (jump - at) as u32, id: param.id, value: to });
                }
                let mut b = vec![input[0][at..at + len].to_vec(), input[1][at..at + len].to_vec()];
                p.process(&mut b, &changes);
                out.extend_from_slice(&b[0]);
                at += len;
            }
            let d2 = |range: std::ops::Range<usize>| range.map(|i| (out[i] - 2.0 * out[i - 1] + out[i - 2]).abs()).fold(0.0f32, f32::max);
            let before = d2(jump - window..jump);
            let during = d2(jump..jump + window);
            let after = d2(n - window..n);
            let floor = before.max(after).max(1e-6);
            20.0 * f64::from(during / floor).log10()
        };
        Step { param: param.name.clone(), up_db: measure(param.min, param.max), down_db: measure(param.max, param.min) }
    }
}
