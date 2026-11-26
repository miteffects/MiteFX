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
    /// Parameters the step test sets first (by name, CLAP units), unless it steps them.
    hold: Vec<(String, f64)>,
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

/// A stereo test signal: a steady 220 Hz tone at −12 dBFS with a little noise (so detectors
/// have something to follow), `n` samples at `rate`. Steady, so the step test's windows before,
/// during and after a change hear the same level (a swell biased them by up to 4 dB, and far
/// more through a saturator, whose harmonics grow steeply with level).
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
        let tone = 0.25 * (std::f64::consts::TAU * 220.0 * t).sin();
        l.push((tone + noise) as f32);
        r.push((0.9 * tone - noise) as f32);
    }
    [l, r]
}

impl Gates {
    pub fn new(entry: &'static clap_plugin_entry, id: &str) -> Self {
        Self { entry, id: id.to_owned(), state: None, hold: Vec::new(), sample_rate: 48_000.0 }
    }

    /// Loads the newest golden state `<dir>/<prefix>.state.v<N>.json` (a state that switches the
    /// plugin's features on) into every instance, if there is one.
    pub fn with_golden_state(self, dir: &std::path::Path, prefix: &str) -> Self {
        let newest = std::fs::read_dir(dir)
            .into_iter()
            .flatten()
            .filter_map(|e| e.ok()?.file_name().into_string().ok())
            .filter_map(|n| n.strip_prefix(&format!("{prefix}.state.v"))?.strip_suffix(".json")?.parse::<u32>().ok())
            .max();
        match newest {
            Some(v) => self.with_state(std::fs::read(dir.join(format!("{prefix}.state.v{v}.json"))).expect("golden state")),
            None => self,
        }
    }

    /// Loads `state` (a saved state's JSON) into every instance first.
    pub fn with_state(mut self, state: Vec<u8>) -> Self {
        self.state = Some(state);
        self
    }

    /// The step test sets parameter `name` to `value` (CLAP units) first, unless it is the one
    /// stepped: a free-running modulator (an LFO) moves the sound by itself, and the test, which
    /// compares the sound before, during and after a step, would hear that as a click.
    pub fn holding(mut self, name: &str, value: f64) -> Self {
        self.hold.push((name.to_owned(), value));
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

    /// Creates an instance (golden state loaded), activates it and plays a few blocks: what a
    /// host's main and audio threads do first. Run on a small stack by the gate below.
    pub fn smoke(&self) {
        let input = test_signal(self.sample_rate, 4 * 512);
        self.run(self.sample_rate, &input, &mut || 512);
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
        // Where the outputs first part (> 1e-6), for the report: a time that moves between runs
        // points at something arriving from another thread.
        let first = |o: &[Vec<f32>]| o.iter().zip(&reference).filter_map(|(a, b)| a.iter().zip(b).position(|(x, y)| (x - y).abs() > 1e-6)).min();
        let named = |b: String, o: &[Vec<f32>]| match first(o) {
            Some(at) => (format!("{b} (from sample {at})"), diff(o)),
            None => (b, diff(o)),
        };
        let mut out: Vec<(String, f32)> = BLOCK_SIZES.iter().map(|&b| named(format!("{b}"), &self.run(self.sample_rate, &input, &mut || b))).collect();
        let mut seed = 0x9e37_79b9u32;
        let random = self.run(self.sample_rate, &input, &mut || {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            1 + (seed % 3000) as usize
        });
        out.push(named("random".to_owned(), &random));
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
        let block = 256;
        // A plugin without sample-accurate automation applies the change where its host block
        // starts, up to a block before `jump`: the "during" window opens there, or a click on the
        // change itself would count as "before" and pass.
        let start = jump - jump % block;
        let input = test_signal(rate, n);
        let params = self.instance(rate).params();
        let held: Vec<ParamChange> = self
            .hold
            .iter()
            .filter(|(name, _)| *name != param.name)
            .map(|(name, value)| {
                let id = params.iter().find(|i| i.name == *name).unwrap_or_else(|| panic!("no parameter {name:?} to hold")).id;
                ParamChange { offset: 0, id, value: *value }
            })
            .collect();
        let measure = |from: f64, to: f64| -> f64 {
            let mut p = self.instance(rate);
            let mut out: [Vec<f32>; 2] = [Vec::with_capacity(n), Vec::with_capacity(n)];
            let mut at = 0;
            let mut first = true;
            while at < n {
                let len = block.min(n - at);
                let mut changes = Vec::new();
                if first {
                    changes.extend_from_slice(&held);
                    changes.push(ParamChange { offset: 0, id: param.id, value: from });
                    first = false;
                }
                if (at..at + len).contains(&jump) {
                    changes.push(ParamChange { offset: (jump - at) as u32, id: param.id, value: to });
                }
                let mut b = vec![input[0][at..at + len].to_vec(), input[1][at..at + len].to_vec()];
                p.process(&mut b, &changes);
                out[0].extend_from_slice(&b[0]);
                out[1].extend_from_slice(&b[1]);
                at += len;
            }
            // MITEFX_GATES_DUMP=<dir>: the output of every step, as raw f32 (`<param>-<up|down>.f32`).
            if let Some(dir) = std::env::var_os("MITEFX_GATES_DUMP") {
                let name = format!("{}-{}.f32", param.name.replace(' ', "_"), if to > from { "up" } else { "down" });
                let bytes: Vec<u8> = out[0].iter().flat_map(|v| v.to_le_bytes()).collect();
                let _ = std::fs::write(std::path::Path::new(&dir).join(name), bytes);
            }
            // Both channels: a pan or balance silences one of them.
            let d2 = |range: std::ops::Range<usize>| out.iter().flat_map(|o| range.clone().map(move |i| (o[i] - 2.0 * o[i - 1] + o[i - 2]).abs())).fold(0.0f32, f32::max);
            let before = d2(start - window..start);
            let during = d2(start..start + window);
            let after = d2(n - window..n);
            let floor = before.max(after).max(1e-6);
            20.0 * f64::from(during / floor).log10()
        };
        Step { param: param.name.clone(), up_db: measure(param.min, param.max), down_db: measure(param.max, param.min) }
    }
}

/// The four gate tests for a plugin crate's `tests/gates.rs`:
///
/// ```ignore
/// mite_plugin::gate_tests!(mite_comp, MiteComp, "mite_comp", block_tolerance = 0.0, skip = []);
/// ```
///
/// `block_tolerance` is the largest difference allowed between block sizes (0: bit-identical);
/// `skip` names parameters left out of the ramp test (each with its reason beside it); the
/// optional `hold = [("LFO 1 Rate", 0.02)]` sets parameters first in it (see [`Gates::holding`]).
#[macro_export]
macro_rules! gate_tests {
    ($krate:ident, $plugin:ident, $prefix:literal, block_tolerance = $tol:expr, skip = [$($skip:literal),* $(,)?]) => {
        $crate::gate_tests!($krate, $plugin, $prefix, block_tolerance = $tol, skip = [$($skip),*], hold = []);
    };
    ($krate:ident, $plugin:ident, $prefix:literal, block_tolerance = $tol:expr, skip = [$($skip:literal),* $(,)?], hold = [$(($hold:literal, $value:expr)),* $(,)?]) => {
        fn gates() -> $crate::gates::Gates {
            use nih_plug::prelude::ClapPlugin;
            let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/golden");
            let g = $crate::gates::Gates::new(&$krate::clap_entry, $krate::$plugin::CLAP_ID).with_golden_state(&dir, $prefix);
            $(let g = g.holding($hold, $value);)*
            g
        }

        /// Windows gives a thread 1 MB of stack (Linux 8 MB), and a host creates and runs a plugin
        /// on such threads: nothing may need more (Sat once overflowed it on Windows only).
        #[test]
        fn it_runs_on_a_one_megabyte_stack() {
            let g = gates();
            std::thread::Builder::new()
                .stack_size(1 << 20)
                .spawn(move || g.smoke())
                .expect("a thread")
                .join()
                .expect("the plugin panicked on a 1 MB stack");
        }

        #[test]
        fn the_output_is_the_same_at_every_block_size() {
            let tol: f32 = $tol;
            let all = gates().block_sizes(1.0);
            for (blocks, diff) in &all {
                println!("blocks {blocks}: max difference {diff:e}");
            }
            let bad: Vec<String> = all.iter().filter(|(_, d)| *d > tol).map(|(b, d)| format!("{b}: {d:e}")).collect();
            assert!(bad.is_empty(), "allowed {tol}; over it: {}", bad.join(", "));
        }

        #[test]
        fn every_sample_rate_from_44_1_to_384_khz_works() {
            for (rate, r) in gates().sample_rates() {
                let (peak, latency) = r.unwrap_or_else(|e| panic!("{rate} Hz: {e}"));
                println!("{rate} Hz: peak {peak:.3}, latency {latency}");
                assert!(peak < 8.0, "{rate} Hz: peak {peak}");
                assert!(f64::from(latency) < rate, "{rate} Hz: latency {latency}");
            }
        }

        #[test]
        fn nan_and_inf_recover_through_reset() {
            let (finite, peak) = gates().nan_recovery();
            println!("after reset: peak {peak}");
            assert!(finite, "non-finite output after reset");
        }

        #[test]
        fn no_continuous_parameter_clicks_when_it_jumps_end_to_end() {
            let g = gates();
            let mut steps: Vec<_> = g.continuous(&[$($skip),*]).iter().map(|p| g.step(p)).collect();
            steps.sort_by(|a, b| b.worst_db().total_cmp(&a.worst_db()));
            for s in steps.iter().take(12) {
                println!("{:<32} up {:+6.1} dB  down {:+6.1} dB", s.param, s.up_db, s.down_db);
            }
            let clicks: Vec<_> = steps.iter().filter(|s| s.worst_db() > 6.0).map(|s| format!("{} ({:+.1} dB)", s.param, s.worst_db())).collect();
            assert!(clicks.is_empty(), "{} of {} click: {}", clicks.len(), steps.len(), clicks.join(", "));
        }
    };
}
