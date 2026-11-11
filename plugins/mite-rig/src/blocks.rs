//! The blocks a board is made of (docs/design/rig-brief.md §2): what each is called, its
//! parameters (ids frozen once released: they are the board's saved values), and the unit that
//! runs it on the audio thread.
//!
//! Every unit is stereo in and out. Captures and cabinets are mono devices: they sum their input
//! and play the same on both sides (a stereo effect after them makes it wide again).

use mite_core::{ParamDisplay, ParamScale, ParamSpec};
use mite_dsp::eq::{BandProcessor, BandSettings, BandType};
use mite_dsp::{db_to_gain, LinearSmoother};

use crate::board::Live;
use crate::parts::{fade_curve, CabEngine, Gate, Loaded, ModelLoad, CabLoad, Stash, SUB};
use crate::ir::{self, HEAD};

/// What a capture plays at when normalised (NAM's convention), dB.
pub const NORMALIZE_TARGET_DB: f32 = -18.0;
/// The tone stack's points (NAM's): bass shelf, middle bell, treble shelf, Hz.
pub const BASS_HZ: f64 = 150.0;
pub const MIDDLE_HZ: f64 = 425.0;
pub const TREBLE_HZ: f64 = 1800.0;
pub const MIDDLE_Q: f64 = 0.7;
/// The cuts' slope index into `mite_dsp::eq::SLOPES_DB` (12 dB/oct).
pub const CUT_SLOPE: usize = 1;
/// Captures and cabinets crossfade to a new file over this, ms.
const SWAP_MS: f32 = 30.0;

/// The kinds of block, in the order the add menu lists them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    Capture,
    Cab,
    Gate,
    Tone,
    Boost,
    Volume,
}

/// A block's parameter: its spec (the id is frozen), and whether it is a switch.
#[derive(Debug, Clone, Copy)]
pub struct BlockParam {
    pub spec: ParamSpec,
    pub toggle: bool,
}

#[allow(clippy::too_many_arguments)] // one line per parameter reads best
const fn spec(id: &'static str, name: &'static str, unit: &'static str, min: f32, max: f32, default: f32, scale: ParamScale, decimals: u8, wheel: (f32, f32), display: ParamDisplay) -> ParamSpec {
    ParamSpec { id, name, unit, min, max, default, scale, smoothing_ms: 20.0, decimals, wheel_step: wheel.0, wheel_step_fine: wheel.1, display }
}

const fn knob(spec: ParamSpec) -> BlockParam {
    BlockParam { spec, toggle: false }
}

const fn switch(id: &'static str, name: &'static str, default: bool) -> BlockParam {
    BlockParam { spec: spec(id, name, "", 0.0, 1.0, if default { 1.0 } else { 0.0 }, ParamScale::Linear, 0, (1.0, 1.0), ParamDisplay::Number), toggle: true }
}

const DB: (f32, f32) = (0.5, 0.1);
const LIN: ParamScale = ParamScale::Linear;
const NUM: ParamDisplay = ParamDisplay::Number;

const CAPTURE: [BlockParam; 3] = [
    knob(spec("gain", "Gain", "dB", -24.0, 24.0, 0.0, LIN, 1, DB, NUM)),
    knob(spec("level", "Level", "dB", -24.0, 24.0, 0.0, LIN, 1, DB, NUM)),
    switch("normalize", "Normalize", true),
];
const CAB: [BlockParam; 1] = [knob(spec("level", "Level", "dB", -24.0, 24.0, 0.0, LIN, 1, DB, NUM))];
const GATE: [BlockParam; 1] = [knob(spec("threshold", "Threshold", "dB", -100.0, -20.0, -70.0, LIN, 1, (1.0, 0.1), NUM))];
const TONE: [BlockParam; 5] = [
    knob(spec("bass", "Bass", "dB", -12.0, 12.0, 0.0, LIN, 1, DB, NUM)),
    knob(spec("middle", "Middle", "dB", -12.0, 12.0, 0.0, LIN, 1, DB, NUM)),
    knob(spec("treble", "Treble", "dB", -12.0, 12.0, 0.0, LIN, 1, DB, NUM)),
    knob(spec("low_cut", "Low Cut", "Hz", 20.0, 500.0, 20.0, ParamScale::Log, 0, (1.0 / 24.0, 1.0 / 96.0), ParamDisplay::Frequency)),
    knob(spec("high_cut", "High Cut", "Hz", 2000.0, 20_000.0, 20_000.0, ParamScale::Log, 0, (1.0 / 24.0, 1.0 / 96.0), ParamDisplay::Frequency)),
];
const BOOST: [BlockParam; 1] = [knob(spec("gain", "Gain", "dB", 0.0, 24.0, 6.0, LIN, 1, DB, NUM))];
const VOLUME: [BlockParam; 1] = [knob(spec("volume", "Volume", "%", 0.0, 100.0, 100.0, LIN, 0, (1.0, 0.1), NUM))];

impl Kind {
    pub const ALL: [Kind; 6] = [Kind::Capture, Kind::Cab, Kind::Gate, Kind::Tone, Kind::Boost, Kind::Volume];

    /// The frozen id saved with the board.
    pub fn id(self) -> &'static str {
        match self {
            Kind::Capture => "capture",
            Kind::Cab => "cab",
            Kind::Gate => "gate",
            Kind::Tone => "tone",
            Kind::Boost => "boost",
            Kind::Volume => "volume",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.id() == id)
    }

    pub fn name(self) -> &'static str {
        match self {
            Kind::Capture => "Capture",
            Kind::Cab => "Cab",
            Kind::Gate => "Gate",
            Kind::Tone => "Tone",
            Kind::Boost => "Boost",
            Kind::Volume => "Volume",
        }
    }

    /// What the add menu says it is.
    pub fn words(self) -> &'static str {
        match self {
            Kind::Capture => "A NAM capture: a pedal, an amp or a full rig (files or TONE3000)",
            Kind::Cab => "A cabinet impulse response (files or TONE3000)",
            Kind::Gate => "Noise gate: silence between notes",
            Kind::Tone => "Bass, middle, treble and low and high cuts",
            Kind::Boost => "A clean boost",
            Kind::Volume => "A volume pedal",
        }
    }

    pub fn params(self) -> &'static [BlockParam] {
        match self {
            Kind::Capture => &CAPTURE,
            Kind::Cab => &CAB,
            Kind::Gate => &GATE,
            Kind::Tone => &TONE,
            Kind::Boost => &BOOST,
            Kind::Volume => &VOLUME,
        }
    }

    pub fn param_index(self, id: &str) -> Option<usize> {
        self.params().iter().position(|p| p.spec.id == id)
    }

    /// Blocks that play a file.
    pub fn has_file(self) -> bool {
        matches!(self, Kind::Capture | Kind::Cab)
    }
}

/// The tone block's bands from its values (bass, middle, treble, low cut, high cut); also what
/// the editor draws.
pub fn tone_bands(v: &[f32; 5]) -> [BandSettings; 5] {
    let band = |kind, freq: f64, gain: f32, q: f64, enabled: bool| BandSettings { enabled, kind, freq, gain_db: f64::from(gain), q, slope: CUT_SLOPE, ..BandSettings::default() };
    let q = std::f64::consts::FRAC_1_SQRT_2;
    [
        band(BandType::LowShelf, BASS_HZ, v[0], q, v[0].abs() > 1e-3),
        band(BandType::Bell, MIDDLE_HZ, v[1], MIDDLE_Q, v[1].abs() > 1e-3),
        band(BandType::HighShelf, TREBLE_HZ, v[2], q, v[2].abs() > 1e-3),
        band(BandType::LowCut, f64::from(v[3]), 0.0, q, v[3] > 20.5),
        band(BandType::HighCut, f64::from(v[4]), 0.0, q, v[4] < 19_900.0),
    ]
}

/// A block running on the audio thread. Allocation-free after construction.
pub trait Unit: Send {
    /// `l` and `r` (≤ [`SUB`] samples) in place, with the block's current values.
    fn process(&mut self, l: &mut [f32], r: &mut [f32], live: &Live);
    fn latency(&self) -> usize {
        0
    }
    fn reset(&mut self, live: &Live);
    /// A capture's model or a cabinet's response, if it holds one (a graph swap moves it).
    fn holds_file(&self) -> bool {
        false
    }
    /// Whether it plays a full rig (amp and cabinet in one capture).
    fn full_rig(&self) -> bool {
        false
    }
}

/// A new unit of `kind` at `fs`. Allocates; worker thread.
pub fn build(kind: Kind, fs: f32) -> Box<dyn Unit> {
    match kind {
        Kind::Capture => Box::new(CaptureUnit::new(fs)),
        Kind::Cab => Box::new(CabUnit::new(fs)),
        Kind::Gate => Box::new(GateUnit { gate: Gate::new(fs), gain: LinearSmoother::new(1.0) }),
        Kind::Tone => Box::new(ToneUnit::new(fs)),
        Kind::Boost | Kind::Volume => Box::new(GainUnit::new(fs, kind)),
    }
}

fn smoother(fs: f32, ms: f32, v: f32) -> LinearSmoother {
    let mut s = LinearSmoother::new(v);
    s.set_ramp(fs, ms);
    s
}

// ---- Capture ----------------------------------------------------------------------------------

pub struct CaptureUnit {
    cur: Box<ModelLoad>,
    old: Option<Box<ModelLoad>>,
    fade: usize,
    fade_len: usize,
    stash: Stash<ModelLoad>,
    gain: LinearSmoother,
    level: LinearSmoother,
    mono: Vec<f32>,
    out: Vec<f32>,
    old_out: Vec<f32>,
}

impl CaptureUnit {
    fn new(fs: f32) -> Self {
        Self {
            cur: Box::new(ModelLoad { model: None }),
            old: None,
            fade: 0,
            fade_len: ((SWAP_MS * 0.001 * fs) as usize).max(1),
            stash: Stash::new(),
            gain: smoother(fs, 20.0, 1.0),
            level: smoother(fs, 50.0, 1.0),
            mono: vec![0.0; SUB],
            out: vec![0.0; SUB],
            old_out: vec![0.0; SUB],
        }
    }

    fn model(&self) -> Option<&Loaded> {
        self.cur.model.as_ref()
    }
}

impl Unit for CaptureUnit {
    fn process(&mut self, l: &mut [f32], r: &mut [f32], live: &Live) {
        let n = l.len();
        self.stash.flush(&live.model);
        if self.old.is_none() {
            if let Some(new) = live.model.take() {
                let old = std::mem::replace(&mut self.cur, new);
                if old.model.is_some() {
                    // From one capture to another: crossfade.
                    self.old = Some(old);
                    self.fade = 0;
                } else {
                    // Into an empty block: at once (no fade from the dry signal).
                    self.stash.retire(&live.model, old);
                }
            }
        }
        // Normalised: the capture plays at the target loudness when the file says how loud it is.
        let normal = match (live.value(2) >= 0.5, self.model().and_then(|m| m.loudness_db)) {
            (true, Some(lo)) => db_to_gain(NORMALIZE_TARGET_DB - lo),
            _ => 1.0,
        };
        self.gain.set_target(db_to_gain(live.value(0)));
        self.level.set_target(db_to_gain(live.value(1)) * normal);
        for i in 0..n {
            self.mono[i] = 0.5 * (l[i] + r[i]) * self.gain.next_value();
        }
        let (x, y) = (&self.mono[..n], &mut self.out[..n]);
        match &mut self.cur.model {
            Some(m) => m.process(x, y),
            None => y.copy_from_slice(x),
        }
        if let Some(old) = &mut self.old {
            match &mut old.model {
                Some(m) => m.process(x, &mut self.old_out[..n]),
                None => self.old_out[..n].copy_from_slice(x),
            }
            for (v, &o) in y.iter_mut().zip(&self.old_out[..n]) {
                let w = fade_curve(self.fade as f32 / self.fade_len as f32);
                *v = o + w * (*v - o);
                self.fade += 1;
            }
            if self.fade >= self.fade_len {
                if let Some(old) = self.old.take() {
                    self.stash.retire(&live.model, old);
                }
            }
        }
        for i in 0..n {
            let v = y[i] * self.level.next_value();
            l[i] = v;
            r[i] = v;
        }
    }

    fn latency(&self) -> usize {
        self.model().map_or(0, Loaded::latency)
    }

    fn reset(&mut self, live: &Live) {
        if let Some(m) = &mut self.cur.model {
            m.reset();
        }
        if let Some(old) = self.old.take() {
            self.stash.retire(&live.model, old);
        }
        self.gain.reset(db_to_gain(live.value(0)));
    }

    fn holds_file(&self) -> bool {
        self.cur.model.is_some()
    }

    fn full_rig(&self) -> bool {
        self.model().is_some_and(|m| m.full_rig)
    }
}

// ---- Cab --------------------------------------------------------------------------------------

pub struct CabUnit {
    engines: [CabEngine; 2],
    active: usize,
    fade: Option<usize>,
    fade_len: usize,
    stash: Stash<CabLoad>,
    level: LinearSmoother,
    mono: Vec<f32>,
    a: Vec<f32>,
    b: Vec<f32>,
}

impl CabUnit {
    fn new(fs: f32) -> Self {
        let max_len = (ir::MAX_SECONDS * f64::from(fs)) as usize + HEAD;
        Self {
            engines: [CabEngine::new(max_len), CabEngine::new(max_len)],
            active: 0,
            fade: None,
            fade_len: ((SWAP_MS * 0.001 * fs) as usize).max(1),
            stash: Stash::new(),
            level: smoother(fs, 20.0, 1.0),
            mono: vec![0.0; SUB],
            a: vec![0.0; SUB],
            b: vec![0.0; SUB],
        }
    }
}

impl Unit for CabUnit {
    fn process(&mut self, l: &mut [f32], r: &mut [f32], live: &Live) {
        let n = l.len();
        self.stash.flush(&live.cab);
        if self.fade.is_none() {
            if let Some(mut c) = live.cab.take() {
                if self.engines[self.active].loaded {
                    // From one cabinet to another: crossfade.
                    let next = 1 - self.active;
                    self.engines[next].install(&mut c);
                    self.fade = Some(0);
                } else {
                    // Into an empty block: at once.
                    self.engines[self.active].install(&mut c);
                }
                self.stash.retire(&live.cab, c);
            }
        }
        self.level.set_target(db_to_gain(live.value(0)));
        for i in 0..n {
            self.mono[i] = 0.5 * (l[i] + r[i]);
        }
        let (first, second) = self.engines.split_at_mut(1);
        let (cur, other) = if self.active == 0 { (&mut first[0], &mut second[0]) } else { (&mut second[0], &mut first[0]) };
        cur.process(&self.mono[..n], &mut self.a[..n]);
        if let Some(f) = self.fade {
            other.process(&self.mono[..n], &mut self.b[..n]);
            for i in 0..n {
                let w = fade_curve((f + i) as f32 / self.fade_len as f32);
                self.a[i] += w * (self.b[i] - self.a[i]);
            }
            if f + n >= self.fade_len {
                self.fade = None;
                self.active = 1 - self.active;
            } else {
                self.fade = Some(f + n);
            }
        }
        // Empty, a cabinet passes the signal (its level with it).
        let loaded = self.engines[self.active].loaded || self.fade.is_some();
        for i in 0..n {
            let g = self.level.next_value();
            let v = if loaded { self.a[i] * g } else { self.a[i] };
            l[i] = v;
            r[i] = v;
        }
    }

    fn reset(&mut self, _live: &Live) {
        for e in &mut self.engines {
            e.reset();
        }
    }

    fn holds_file(&self) -> bool {
        self.engines[self.active].loaded
    }
}

// ---- Gate, Tone, Boost / Volume -------------------------------------------------------------

struct GateUnit {
    gate: Gate,
    gain: LinearSmoother,
}

impl Unit for GateUnit {
    fn process(&mut self, l: &mut [f32], r: &mut [f32], live: &Live) {
        let threshold = live.value(0);
        for i in 0..l.len() {
            let g = db_to_gain(self.gate.next(0.5 * (l[i] + r[i]), true, threshold));
            l[i] *= g;
            r[i] *= g;
        }
        let _ = &self.gain;
    }

    fn reset(&mut self, _live: &Live) {
        self.gate.reset();
    }
}

struct ToneUnit {
    bands: [BandProcessor; 5],
}

impl ToneUnit {
    fn new(fs: f32) -> Self {
        Self { bands: std::array::from_fn(|_| BandProcessor::new(f64::from(fs), 20.0)) }
    }

    fn values(live: &Live) -> [f32; 5] {
        std::array::from_fn(|i| live.value(i))
    }
}

impl Unit for ToneUnit {
    fn process(&mut self, l: &mut [f32], r: &mut [f32], live: &Live) {
        for (b, s) in self.bands.iter_mut().zip(tone_bands(&Self::values(live))) {
            b.set_target(&s);
            b.process(&mut [&mut *l, &mut *r]);
        }
    }

    fn reset(&mut self, live: &Live) {
        for (b, s) in self.bands.iter_mut().zip(tone_bands(&Self::values(live))) {
            b.reset(&s);
        }
    }
}

struct GainUnit {
    kind: Kind,
    gain: LinearSmoother,
}

impl GainUnit {
    fn new(fs: f32, kind: Kind) -> Self {
        Self { kind, gain: smoother(fs, 20.0, 1.0) }
    }

    /// Boost: dB. Volume: a volume pot's taper (the square of the travel).
    fn target(&self, live: &Live) -> f32 {
        match self.kind {
            Kind::Volume => (live.value(0) / 100.0).clamp(0.0, 1.0).powi(2),
            _ => db_to_gain(live.value(0)),
        }
    }
}

impl Unit for GainUnit {
    fn process(&mut self, l: &mut [f32], r: &mut [f32], live: &Live) {
        self.gain.set_target(self.target(live));
        for i in 0..l.len() {
            let g = self.gain.next_value();
            l[i] *= g;
            r[i] *= g;
        }
    }

    fn reset(&mut self, live: &Live) {
        self.gain.reset(self.target(live));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_kind_has_unique_frozen_ids() {
        let mut seen = std::collections::HashSet::new();
        for k in Kind::ALL {
            assert!(seen.insert(k.id()));
            assert_eq!(Kind::from_id(k.id()), Some(k));
            let mut p = std::collections::HashSet::new();
            assert!(k.params().iter().all(|x| p.insert(x.spec.id)), "{}", k.id());
        }
    }

    #[test]
    fn neutral_blocks_pass_the_signal() {
        let fs = 48_000.0;
        for kind in [Kind::Tone, Kind::Volume, Kind::Capture, Kind::Cab] {
            let live = Live::new(1, kind);
            let mut u = build(kind, fs);
            u.reset(&live);
            if kind == Kind::Capture {
                live.set(2, 0.0);
            }
            let x: Vec<f32> = (0..SUB).map(|i| (i as f32 * 0.1).sin() * 0.3).collect();
            let (mut l, mut r) = (x.clone(), x.clone());
            for _ in 0..10 {
                l.copy_from_slice(&x);
                r.copy_from_slice(&x);
                u.process(&mut l, &mut r, &live);
            }
            let err = l.iter().zip(&x).fold(0.0f32, |m, (a, b)| m.max((a - b).abs()));
            assert!(err < 1e-5, "{}: {err}", kind.id());
        }
    }
}
