//! Neural Amp Modeler captures in pure Rust: reads `.nam` files (format 0.5 to 0.7: A1's WaveNet
//! and LSTM, A2's WaveNet with its new layer features, ConvNet, Linear, SlimmableContainer,
//! Sequential) and runs them.
//!
//! Loading parses, allocates and pre-warms (runs silence through the network until its state
//! settles), so it belongs on a worker thread. [`NamModel::process`] never allocates, locks or
//! panics. The arithmetic follows NeuralAmpModelerCore (MIT, Steven Atkinson), whose output the
//! tests match sample by sample on its example models.

use std::path::Path;

use serde_json::Value;

pub mod kernel;
pub mod layers;
mod others;
mod wavenet;

/// The largest block a model is built for; longer calls are cut into pieces of this size.
pub const DEFAULT_MAX_BLOCK: usize = 512;

/// Oldest and newest file versions it reads (minor versions beyond the newest are refused, as
/// the reference does: they may change what the weights mean).
const EARLIEST: (u64, u64, u64) = (0, 5, 0);
const LATEST_MINOR: (u64, u64) = (0, 7);

/// A network: frame-major multichannel in and out, `n` ≤ its max block.
pub(crate) trait Net: Send {
    fn in_channels(&self) -> usize;
    fn out_channels(&self) -> usize;
    /// Samples of silence to run after a reset for the state to settle.
    fn prewarm_samples(&self) -> usize;
    /// Clears all state (to the network's initial state).
    fn reset(&mut self);
    fn process(&mut self, x: &[f32], y: &mut [f32], n: usize);
}

/// What the file says about the capture.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ModelInfo {
    /// `"WaveNet"`, `"LSTM"`, …: the architecture run (inside a container, the chosen one's).
    pub architecture: String,
    /// The file's format version.
    pub version: String,
    /// A2 (the 0.6+ WaveNet family and its containers) or A1.
    pub generation: Generation,
    /// The rate it was trained at, Hz (none: unknown, assume the host's).
    pub sample_rate: Option<f64>,
    /// Its output loudness for the trainer's test signal, dB (for normalising captures).
    pub loudness: Option<f64>,
    /// The input level that hits 0 dBFS in the capture, dBu (for calibrating the input).
    pub input_level_dbu: Option<f64>,
    pub output_level_dbu: Option<f64>,
    pub name: Option<String>,
    pub modeled_by: Option<String>,
    pub gear_type: Option<String>,
    pub gear_make: Option<String>,
    pub gear_model: Option<String>,
    pub tone_type: Option<String>,
    /// The sizes a container offers (their upper bounds on 0…1, ascending; empty: one size).
    pub sizes: Vec<f64>,
    /// The size index chosen (into `sizes`).
    pub size_index: usize,
    /// Pre-warm length, samples at the model's rate.
    pub prewarm: usize,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Generation {
    #[default]
    A1,
    A2,
}

/// How to build a model.
#[derive(Debug, Clone, Copy)]
pub struct LoadOptions {
    /// Run `Tanh` as NAM's fast approximation, as its plugins do.
    pub fast_tanh: bool,
    /// For containers with several sizes: 0…1, 1 the full model.
    pub size: f64,
    pub max_block: usize,
}

impl Default for LoadOptions {
    fn default() -> Self {
        Self { fast_tanh: true, size: 1.0, max_block: DEFAULT_MAX_BLOCK }
    }
}

fn parse_version(v: &str) -> Option<(u64, u64, u64)> {
    let mut parts = v.trim().split('.').map(|p| p.parse::<u64>().ok());
    let r = (parts.next()??, parts.next()??, parts.next()??);
    parts.next().is_none().then_some(r)
}

fn check_version(v: &str) -> Result<(), String> {
    let p = parse_version(v).ok_or_else(|| format!("an unreadable format version \"{v}\""))?;
    if p < EARLIEST {
        return Err(format!("format {v} is older than NAM 0.5"));
    }
    if (p.0, p.1) > LATEST_MINOR {
        return Err(format!("format {v} is newer than this version understands"));
    }
    Ok(())
}

fn weights_of(j: &Value) -> Result<Vec<f32>, String> {
    match j.get("weights") {
        Some(Value::Array(a)) => Ok(a.iter().map(|w| w.as_f64().unwrap_or(0.0) as f32).collect()),
        _ => Err("the file has no weights".to_owned()),
    }
}

fn text(m: &Value, k: &str) -> Option<String> {
    m.get(k).and_then(Value::as_str).filter(|s| !s.is_empty()).map(str::to_owned)
}

fn number(m: &Value, k: &str) -> Option<f64> {
    m.get(k).and_then(Value::as_f64)
}

/// Builds the network a model document describes. `size` picks a container's member. Returns
/// the network and what the document says about it.
pub(crate) fn load_net(j: &Value, fast: bool, max: usize, size: f64) -> Result<(Box<dyn Net>, ModelInfo), String> {
    let version = j.get("version").and_then(Value::as_str).ok_or("not a NAM model (no version)")?;
    check_version(version)?;
    let arch = j.get("architecture").and_then(Value::as_str).ok_or("not a NAM model (no architecture)")?;
    let config = j.get("config").ok_or("not a NAM model (no config)")?;
    let sample_rate = number(j, "sample_rate").filter(|r| *r > 0.0);
    let meta = j.get("metadata").cloned().unwrap_or(Value::Null);
    let mut info = ModelInfo {
        architecture: arch.to_owned(),
        version: version.to_owned(),
        generation: if parse_version(version).is_some_and(|v| v >= (0, 6, 0)) { Generation::A2 } else { Generation::A1 },
        sample_rate,
        loudness: number(&meta, "loudness"),
        input_level_dbu: number(&meta, "input_level_dbu"),
        output_level_dbu: number(&meta, "output_level_dbu"),
        name: text(&meta, "name"),
        modeled_by: text(&meta, "modeled_by"),
        gear_type: text(&meta, "gear_type"),
        gear_make: text(&meta, "gear_make"),
        gear_model: text(&meta, "gear_model"),
        tone_type: text(&meta, "tone_type"),
        ..ModelInfo::default()
    };
    let rate = sample_rate.unwrap_or(48_000.0);
    let net: Box<dyn Net> = match arch {
        "WaveNet" => Box::new(wavenet::WaveNet::load(config, &weights_of(j)?, fast, max)?),
        "LSTM" => Box::new(others::Lstm::load(config, &weights_of(j)?, fast, rate)?),
        "ConvNet" => Box::new(others::ConvNet::load(config, &weights_of(j)?, fast, max)?),
        "Linear" => Box::new(others::Linear::load(config, &weights_of(j)?, max)?),
        "Sequential" => Box::new(others::Sequential::load(config, fast, max)?),
        "SlimmableContainer" => {
            let subs = config.get("submodels").and_then(Value::as_array).filter(|s| !s.is_empty()).ok_or("a container without models")?;
            let bounds: Vec<f64> = subs.iter().map(|s| s.get("max_value").and_then(Value::as_f64).unwrap_or(1.0)).collect();
            if bounds.windows(2).any(|w| w[1] <= w[0]) || bounds.last().is_some_and(|b| *b < 1.0) {
                return Err("a container whose sizes are out of order".to_owned());
            }
            let index = bounds.iter().position(|b| size < *b).unwrap_or(bounds.len() - 1);
            let (net, inner) = load_net(subs[index].get("model").ok_or("a container member without a model")?, fast, max, 1.0)?;
            // The container's own metadata wins; what it lacks comes from the member.
            info.architecture = inner.architecture.clone();
            info.generation = Generation::A2;
            info.sample_rate = info.sample_rate.or(inner.sample_rate);
            info.loudness = info.loudness.or(inner.loudness);
            info.input_level_dbu = info.input_level_dbu.or(inner.input_level_dbu);
            info.output_level_dbu = info.output_level_dbu.or(inner.output_level_dbu);
            info.name = info.name.or(inner.name);
            info.modeled_by = info.modeled_by.or(inner.modeled_by);
            info.gear_type = info.gear_type.or(inner.gear_type);
            info.gear_make = info.gear_make.or(inner.gear_make);
            info.gear_model = info.gear_model.or(inner.gear_model);
            info.tone_type = info.tone_type.or(inner.tone_type);
            info.sizes = bounds;
            info.size_index = index;
            net
        }
        other => return Err(format!("the {other} architecture is not supported")),
    };
    info.prewarm = net.prewarm_samples();
    Ok((net, info))
}

/// A capture, ready to run: mono in, mono out.
pub struct NamModel {
    net: Box<dyn Net>,
    info: ModelInfo,
    max_block: usize,
    zeros: Vec<f32>,
    scratch: Vec<f32>,
}

impl NamModel {
    /// Parses a model document, builds it and pre-warms it. Allocates.
    pub fn from_json(text: &str, options: LoadOptions) -> Result<Self, String> {
        let j: Value = serde_json::from_str(text).map_err(|e| format!("not a NAM model ({e})"))?;
        let max = options.max_block.max(1);
        let (net, info) = load_net(&j, options.fast_tanh, max, options.size.clamp(0.0, 1.0))?;
        if net.in_channels() != 1 || net.out_channels() != 1 {
            return Err(format!("a {}-in {}-out model (only mono captures play)", net.in_channels(), net.out_channels()));
        }
        let mut model = Self { net, info, max_block: max, zeros: vec![0.0; max], scratch: vec![0.0; max] };
        model.reset();
        Ok(model)
    }

    /// Reads and builds a `.nam` file. Allocates.
    pub fn from_file(path: &Path, options: LoadOptions) -> Result<Self, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("cannot read it ({e})"))?;
        Self::from_json(&text, options)
    }

    pub fn info(&self) -> &ModelInfo {
        &self.info
    }

    pub fn max_block(&self) -> usize {
        self.max_block
    }

    /// Back to the settled silent state: cleared, then pre-warmed (CPU proportional to the
    /// pre-warm length; no allocation).
    pub fn reset(&mut self) {
        self.net.reset();
        let mut done = 0;
        let target = self.net.prewarm_samples();
        // In whole max-size blocks, as the reference does (the settled state then matches it).
        while done < target {
            self.net.process(&self.zeros, &mut self.scratch, self.max_block);
            done += self.max_block;
        }
    }

    /// Mono in, mono out, any length. Allocation-free.
    pub fn process(&mut self, input: &[f32], output: &mut [f32]) {
        let len = input.len().min(output.len());
        let mut at = 0;
        while at < len {
            let n = (len - at).min(self.max_block);
            self.net.process(&input[at..at + n], &mut output[at..at + n], n);
            at += n;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_outside_the_supported_range_are_refused() {
        assert!(check_version("0.5.4").is_ok());
        assert!(check_version("0.7.0").is_ok());
        assert!(check_version("0.7.9").is_ok());
        assert!(check_version("0.8.0").is_err());
        assert!(check_version("0.4.0").is_err());
        assert!(check_version("x").is_err());
    }
}
