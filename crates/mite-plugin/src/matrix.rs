//! The modulation matrix's host parameters (`mite_core::modmatrix`), shared by Delay, Sat and Mod:
//! the two LFOs, the envelope, the eight slots; their host order and names, and the engine's
//! settings from them.

use mite_core::modmatrix::*;
use mite_core::ChoiceSpec;
use mite_dsp::modmatrix::{EnvSettings, LfoSettings, LfoShape, MatrixSettings, Slot, Source};
use nih_plug::prelude::*;

use crate::{bool_param, choice_param, float_param, float_param_named};

pub struct LfoParams {
    pub rate: FloatParam,
    pub sync: BoolParam,
    pub div: IntParam,
    pub shape: IntParam,
    pub phase: FloatParam,
}

impl LfoParams {
    fn new(lfo: usize) -> Self {
        let name = |what: &str| format!("LFO {} {what}", lfo + 1);
        Self {
            rate: float_param_named(&LFO_RATE, name("Rate")),
            sync: bool_param(&LFO_SYNC, name("Sync")),
            div: choice_param(&LFO_DIV, name("Division")),
            shape: choice_param(&LFO_SHAPE, name("Shape")),
            phase: float_param_named(&LFO_PHASE, name("Phase")),
        }
    }
}

pub struct SlotParams {
    pub src: IntParam,
    pub dst: IntParam,
    pub amt: FloatParam,
}

impl SlotParams {
    fn new(slot: usize, targets: &'static ChoiceSpec) -> Self {
        let name = |what: &str| format!("Mod {} {what}", slot + 1);
        Self {
            src: choice_param(&M_SRC, name("Source")).non_automatable(),
            dst: choice_param(targets, name("Target")).non_automatable(),
            amt: float_param_named(&M_AMT, name("Amount")),
        }
    }
}

pub struct MatrixParams {
    pub lfos: [LfoParams; LFOS],
    pub env_att: FloatParam,
    pub env_rel: FloatParam,
    pub env_gain: FloatParam,
    /// Only where the plugin has a sidechain.
    pub env_src: Option<IntParam>,
    pub slots: [SlotParams; MOD_SLOTS],
}

impl MatrixParams {
    /// `targets`: the plugin's `dst` choice (id "dst"); `sidechain`: whether `env_src` exists.
    pub fn new(targets: &'static ChoiceSpec, sidechain: bool) -> Self {
        Self {
            lfos: std::array::from_fn(LfoParams::new),
            env_att: float_param(&ENV_ATT),
            env_rel: float_param(&ENV_REL),
            env_gain: float_param(&ENV_GAIN),
            env_src: sidechain.then(|| choice_param(&ENV_SRC, ENV_SRC.name.to_owned())),
            slots: std::array::from_fn(|i| SlotParams::new(i, targets)),
        }
    }

    pub fn settings(&self) -> MatrixSettings {
        MatrixSettings {
            lfos: std::array::from_fn(|i| {
                let l = &self.lfos[i];
                LfoSettings { rate_hz: l.rate.value(), sync: l.sync.value(), division: l.div.value().max(0) as usize, shape: LfoShape::from_index(l.shape.value().max(0) as usize), phase: l.phase.value() / 360.0 }
            }),
            env: EnvSettings { attack_ms: self.env_att.value(), release_ms: self.env_rel.value(), gain_db: self.env_gain.value(), sidechain: self.env_src.as_ref().is_some_and(|p| p.value() == 1) },
            slots: std::array::from_fn(|i| {
                let s = &self.slots[i];
                Slot { source: Source::from_index(s.src.value().max(0) as usize), target: s.dst.value().max(0) as usize, amount: s.amt.value() / 100.0 }
            }),
        }
    }

    /// The host map's entries, in manifest order (groups "LFO N", "Envelope", "Modulation").
    pub fn param_map(&self, map: &mut Vec<(String, ParamPtr, String)>) {
        for (i, l) in self.lfos.iter().enumerate() {
            let group = format!("LFO {}", i + 1);
            map.push((lfo_id(i, LFO_RATE.id), l.rate.as_ptr(), group.clone()));
            map.push((lfo_id(i, LFO_SYNC.id), l.sync.as_ptr(), group.clone()));
            map.push((lfo_id(i, LFO_DIV.id), l.div.as_ptr(), group.clone()));
            map.push((lfo_id(i, LFO_SHAPE.id), l.shape.as_ptr(), group.clone()));
            map.push((lfo_id(i, LFO_PHASE.id), l.phase.as_ptr(), group));
        }
        let group = "Envelope".to_owned();
        map.push((ENV_ATT.id.to_owned(), self.env_att.as_ptr(), group.clone()));
        map.push((ENV_REL.id.to_owned(), self.env_rel.as_ptr(), group.clone()));
        map.push((ENV_GAIN.id.to_owned(), self.env_gain.as_ptr(), group.clone()));
        if let Some(src) = &self.env_src {
            map.push((ENV_SRC.id.to_owned(), src.as_ptr(), group));
        }
        for (i, m) in self.slots.iter().enumerate() {
            let group = "Modulation".to_owned();
            map.push((mod_id(i, M_SRC.id), m.src.as_ptr(), group.clone()));
            map.push((mod_id(i, "dst"), m.dst.as_ptr(), group.clone()));
            map.push((mod_id(i, M_AMT.id), m.amt.as_ptr(), group));
        }
    }
}

/// The engine's matrix settings from plain values by id (for stand-in hosts and the editor).
pub fn matrix_settings(p: impl Fn(&str) -> f32) -> MatrixSettings {
    let index = |id: &str| p(id).round().max(0.0) as usize;
    MatrixSettings {
        lfos: std::array::from_fn(|i| LfoSettings {
            rate_hz: p(&lfo_id(i, LFO_RATE.id)),
            sync: p(&lfo_id(i, LFO_SYNC.id)) >= 0.5,
            division: index(&lfo_id(i, LFO_DIV.id)),
            shape: LfoShape::from_index(index(&lfo_id(i, LFO_SHAPE.id))),
            phase: p(&lfo_id(i, LFO_PHASE.id)) / 360.0,
        }),
        env: EnvSettings { attack_ms: p(ENV_ATT.id), release_ms: p(ENV_REL.id), gain_db: p(ENV_GAIN.id), sidechain: index(ENV_SRC.id) == 1 },
        slots: std::array::from_fn(|i| Slot { source: Source::from_index(index(&mod_id(i, M_SRC.id))), target: index(&mod_id(i, "dst")), amount: p(&mod_id(i, M_AMT.id)) / 100.0 }),
    }
}

/// The matrix's per-block plumbing, the same in every plugin: the envelope's input levels (sized
/// in [`MatrixRunner::prepare`]), the control steps over the block's channels, and the readings
/// published for the editor. Never allocates while processing.
pub struct MatrixRunner {
    matrix: mite_dsp::modmatrix::Matrix,
    levels: Vec<f32>,
    offsets: [f32; MAX_TARGETS],
}

impl Default for MatrixRunner {
    fn default() -> Self {
        Self { matrix: mite_dsp::modmatrix::Matrix::new(48_000.0), levels: Vec::new(), offsets: [0.0; MAX_TARGETS] }
    }
}

impl MatrixRunner {
    /// Allocates.
    pub fn prepare(&mut self, sample_rate: f32, max_block: usize) {
        self.matrix = mite_dsp::modmatrix::Matrix::new(sample_rate);
        self.levels = vec![0.0; max_block.max(1)];
    }

    pub fn reset(&mut self) {
        self.matrix.reset();
    }

    /// Runs the block in `channels` (one or two, processed in place): `step` gets each control
    /// step's sub-slices, their range in the block and the targets' offsets (all 0, in one call
    /// over the whole block, when nothing is routed). The envelope follows the input (or `key`
    /// when the settings ask for the sidechain and there is one). `targets`: how many offsets to
    /// publish.
    #[allow(clippy::too_many_arguments)]
    pub fn run(&mut self, settings: &mite_dsp::modmatrix::MatrixSettings, transport: &mite_dsp::modulation::Transport, channels: &mut [&mut [f32]], key: Option<&[&[f32]]>, tap: &MatrixTap, targets: usize, mut step: impl FnMut(&mut [&mut [f32]], std::ops::Range<usize>, &[f32])) {
        let n_ch = channels.len().min(2);
        if n_ch == 0 {
            return;
        }
        let len = channels[..n_ch].iter().map(|c| c.len()).min().unwrap_or(0).min(self.levels.len());
        let source: &[&mut [f32]] = &channels[..n_ch];
        match key.filter(|k| settings.env.sidechain && !k.is_empty()) {
            Some(k) => {
                for (i, e) in self.levels[..len].iter_mut().enumerate() {
                    *e = k.iter().map(|c| c.get(i).map_or(0.0, |v| v.abs())).fold(0.0, f32::max);
                }
            }
            None => {
                for (i, e) in self.levels[..len].iter_mut().enumerate() {
                    *e = source.iter().map(|c| c[i].abs()).fold(0.0, f32::max);
                }
            }
        }
        let levels = &self.levels;
        let readings = self.matrix.steps(settings, transport, len, &mut self.offsets, |i| levels[i], |r, offsets| {
            if n_ch == 2 {
                let (a, b) = channels.split_at_mut(1);
                let mut sub: [&mut [f32]; 2] = [&mut a[0][r.clone()], &mut b[0][r.clone()]];
                step(&mut sub, r, offsets);
            } else {
                let mut sub: [&mut [f32]; 1] = [&mut channels[0][r.clone()]];
                step(&mut sub, r, offsets);
            }
        });
        for (k, v) in readings.sources.iter().enumerate() {
            tap.sources[k].store(*v);
        }
        for l in 0..LFOS {
            tap.cycles[l].store(readings.phases[l].floor() as f32);
            tap.phases[l].store(readings.phases[l].fract() as f32);
            tap.rates[l].store(readings.rates[l] as f32);
        }
        for (k, v) in self.offsets.iter().enumerate().take(targets.min(MAX_TARGETS)) {
            tap.offsets[k].store(*v);
        }
    }
}
