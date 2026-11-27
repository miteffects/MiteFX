//! Mite Shape's engine: the shaper (`mite_dsp::shape`) and its output stage (mix and delta
//! against the band sum, bypass against the latency-aligned dry signal, auto gain, output gain and
//! balance), plus the history telemetry: one record per 5 ms slice, whatever the host's block size.
//!
//! Linear Phase (2026-11-26, user: the minimum-phase crossovers turned the kick's phase at each
//! crossover) splits the bands with FIR kernels instead (`mite_dsp::crossover::linear_phase_kernels`:
//! each band the LR4 magnitude with zero phase, their sum a pure delay), one convolver per band
//! slot, as Mite Sat does. A designer thread makes them whenever the crossovers move; the audio
//! thread swaps them in with a crossfade and hands the old ones back to be freed there. Switching
//! it moves the latency: behind the dip, as the lookahead and the clip stage.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use mite_core::{triple_buffer, Handover, HistoryTap, SpectrumTap, TripleReader, TripleWriter};
use mite_dsp::conv::{Convolver, KernelLayout, KernelSet};
use mite_dsp::output::{balance, DryDelay};
use mite_dsp::oversample::Oversampler;
use mite_dsp::shape::{soft_clip, Layout, ShapeSettings, ShapeTelemetry, Shaper, BANDS, CROSSOVERS, MAX_LOOKAHEAD_MS};
use mite_dsp::{db_to_gain, LinearSmoother, MINUS_INFINITY_DB};

/// Fields of a history record (`ShapeShared::history`).
pub mod record {
    use mite_dsp::shape::BANDS;

    /// Peak in and out, L and R, dBFS.
    pub const IN: usize = 0;
    pub const OUT: usize = 2;
    /// Per band slot: the gain with the largest size, per processing channel (L/R or M/S), dB.
    pub const GAIN_A: usize = 4;
    pub const GAIN_B: usize = GAIN_A + BANDS;
    /// Per band slot: the attack weight at its highest (0…1).
    pub const WEIGHT: usize = GAIN_B + BANDS;
    /// Per band slot: a hit's sharpness (0…1), in the slice it is reported in.
    pub const HIT: usize = WEIGHT + BANDS;
    /// Per band slot: its peak level as the detector hears it, dBFS.
    pub const LEVEL: usize = HIT + BANDS;
    /// The peak going into the clip stage, L and R, dBFS (while it is on; else silence).
    pub const CLIP: usize = LEVEL + BANDS;
    pub const WIDTH: usize = 32;
}

/// One history record per this much audio.
pub const HISTORY_SLICE_MS: f32 = 5.0;
/// Records kept: 30 s (what pausing keeps, per the design).
pub const HISTORY_RECORDS: usize = 6000;
/// Longest chunk the engine processes at once (scratch buffers are this long).
const CHUNK: usize = 512;
/// Switch fades (delta, bypass, the latency dip) and output glides (mix, gain, balance), ms.
const SWITCH_MS: f32 = 10.0;
const GLIDE_MS: f32 = 20.0;
/// Auto gain: loudness in and out, measured over this long (s), and how far it may correct, dB.
const AUTO_SECONDS: f32 = 1.5;
const AUTO_LIMIT_DB: f32 = 12.0;
/// Below this mean square (−70 dBFS) the loudness is not measured (silence keeps the gain).
const AUTO_GATE: f32 = 1e-7;
/// Auto gain retargets every this many samples (on the clock, not per host block).
const AUTO_EVERY: usize = 64;
/// The clip stage runs this many times oversampled (it is the one nonlinear step).
const CLIP_OVERSAMPLING: usize = 4;

/// Linear Phase's kernel length and convolution block at 48 kHz (both scale with the sample
/// rate), as Mite Sat's: 8192 taps hold an LR4 band down to the lowest crossovers within 0.3 dB;
/// the latency is a block plus half the kernel, 90.7 ms.
const LP_LEN_48K: usize = 8192;
const LP_BLOCK_48K: usize = 256;
/// A kernel swap's crossfade, ms.
const KERNEL_FADE_MS: f64 = 20.0;

fn rate_ratio(sample_rate: f64) -> usize {
    ((sample_rate / 48_000.0).round().max(1.0) as usize).next_power_of_two()
}

/// Linear Phase's added latency at `sample_rate`, samples.
pub fn linear_phase_latency(sample_rate: f32) -> usize {
    let r = rate_ratio(f64::from(sample_rate));
    LP_BLOCK_48K * r + LP_LEN_48K * r / 2
}

/// What the designer makes kernels for.
#[derive(Debug, Clone, Copy, PartialEq)]
struct SplitRequest {
    crossovers: [Option<f32>; CROSSOVERS],
    sample_rate: f64,
    len: usize,
    block: usize,
}

impl SplitRequest {
    fn new(crossovers: [Option<f32>; CROSSOVERS], sample_rate: f32) -> Self {
        let r = rate_ratio(f64::from(sample_rate));
        Self { crossovers, sample_rate: f64::from(sample_rate), len: LP_LEN_48K * r, block: LP_BLOCK_48K * r }
    }
}

/// A band slot's kernel each (zero for slots the split does not use), and what they were made for.
pub struct SplitKernels {
    request: SplitRequest,
    sets: [Option<Box<KernelSet>>; BANDS],
}

fn design_split(req: &SplitRequest) -> SplitKernels {
    let layout = Layout::new(&req.crossovers);
    let kernels = mite_dsp::crossover::linear_phase_kernels(&layout, &req.crossovers, req.sample_rate, req.len);
    SplitKernels { request: *req, sets: std::array::from_fn(|b| Some(Box::new(KernelSet::new(req.block, KernelLayout::Shared, std::slice::from_ref(&kernels[b]), req.len / 2)))) }
}

/// The designer thread: designs for the latest request, hands it over, frees what came back.
fn run_designer(mut requests: TripleReader<SplitRequest>, kernels: Arc<Handover<SplitKernels>>, stop: Arc<AtomicBool>) {
    while !stop.load(Ordering::Relaxed) {
        if let Some(req) = requests.read() {
            kernels.send(Box::new(design_split(&req)));
        }
        kernels.collect();
        std::thread::sleep(Duration::from_millis(2));
    }
}

/// Where a switch that moves the latency stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Switch {
    Steady,
    /// Muted, waiting (for Linear Phase's first kernels).
    Waiting,
    /// Muted while the new path fills (its latency), so the sound comes back with the fade.
    Filling(usize),
}

/// The output stage as the parameters set it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Output {
    /// 0…1.
    pub mix: f32,
    pub delta: bool,
    pub bypass: bool,
    pub auto_gain: bool,
    pub gain_db: f32,
    /// −1 … 1.
    pub pan: f32,
    /// The clip stage: on, its ceiling (dBFS) and softness (0…1).
    pub clip: bool,
    pub ceiling_db: f32,
    pub softness: f32,
    /// The bands split by linear-phase kernels (no phase turn at the crossovers, 90.7 ms of
    /// latency) rather than LR4 crossovers.
    pub linear: bool,
}

impl Default for Output {
    fn default() -> Self {
        Self { mix: 1.0, delta: false, bypass: false, auto_gain: false, gain_db: 0.0, pan: 0.0, clip: false, ceiling_db: -1.0, softness: 0.4, linear: false }
    }
}

pub struct ShapeEngine {
    sample_rate: f32,
    shaper: Shaper,
    dry: DryDelay,
    wet: [Vec<f32>; 2],
    band_sum: [Vec<f32>; 2],
    mix: LinearSmoother,
    delta: LinearSmoother,
    bypass: LinearSmoother,
    gain: LinearSmoother,
    pan: LinearSmoother,
    /// The latency switch: fades out, applies the new lookahead, fades back in.
    dip: LinearSmoother,
    lookahead_ms: f32,
    /// The clip stage in force (a change waits for the dip, as it changes the latency), its
    /// resamplers and high-rate buffer, and its glides.
    clip_on: bool,
    os: Oversampler,
    hi: Vec<f32>,
    ceiling: LinearSmoother,
    softness: LinearSmoother,
    /// Per sample of a chunk: the bypass blend, and the ceiling, softness and delta blend, for
    /// after the clip stage.
    bypass_now: Vec<f32>,
    clip_now: Vec<(f32, f32, f32)>,
    /// Auto gain: mean squares in and out, the correction, and their per-sample coefficient.
    auto_in: f32,
    auto_out: f32,
    auto: LinearSmoother,
    auto_k: f32,
    auto_count: usize,
    slice_len: usize,
    slice_pos: usize,
    acc: [f32; record::WIDTH],
    switch: Switch,
    /// Linear Phase: running, its convolvers (one per band slot) and their outputs, the kernels'
    /// way in and out, the request last sent, what the convolvers' kernels were made for, a set
    /// whose old kernels are still fading out, and the designer thread.
    linear: bool,
    convs: Vec<Convolver>,
    lp_bands: Vec<[Vec<f32>; 2]>,
    kernels: Arc<Handover<SplitKernels>>,
    requests: TripleWriter<SplitRequest>,
    last_request: Option<SplitRequest>,
    installed: Option<SplitRequest>,
    spare: Option<Box<SplitKernels>>,
    stop: Arc<AtomicBool>,
    designer: Option<JoinHandle<()>>,
}

impl Drop for ShapeEngine {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.designer.take() {
            let _ = t.join();
        }
    }
}

impl ShapeEngine {
    /// Allocates and starts the kernel designer; call off the audio thread.
    pub fn new(sample_rate: f32, s: &ShapeSettings) -> Self {
        let kernels = Arc::new(Handover::new());
        let stop = Arc::new(AtomicBool::new(false));
        let (requests, reader) = triple_buffer(SplitRequest::new(s.crossovers, sample_rate));
        let designer = {
            let (kernels, stop) = (kernels.clone(), stop.clone());
            std::thread::Builder::new().name("mite-shape-split".into()).spawn(move || run_designer(reader, kernels, stop)).ok()
        };
        let mut e = Self {
            sample_rate,
            shaper: Shaper::new(sample_rate, s),
            dry: DryDelay::new(1, 1),
            wet: [Vec::new(), Vec::new()],
            band_sum: [Vec::new(), Vec::new()],
            mix: LinearSmoother::new(1.0),
            delta: LinearSmoother::new(0.0),
            bypass: LinearSmoother::new(0.0),
            gain: LinearSmoother::new_s_curve(1.0),
            pan: LinearSmoother::new(0.0),
            dip: LinearSmoother::new_s_curve(1.0),
            lookahead_ms: s.lookahead_ms,
            clip_on: false,
            os: Oversampler::new(1),
            hi: Vec::new(),
            ceiling: LinearSmoother::new(1.0),
            softness: LinearSmoother::new(0.4),
            bypass_now: Vec::new(),
            clip_now: Vec::new(),
            auto_in: 0.0,
            auto_out: 0.0,
            auto: LinearSmoother::new(1.0),
            auto_k: 0.0,
            auto_count: 0,
            slice_len: 1,
            slice_pos: 0,
            acc: empty_record(),
            switch: Switch::Steady,
            linear: false,
            convs: Vec::new(),
            lp_bands: Vec::new(),
            kernels,
            requests,
            last_request: None,
            installed: None,
            spare: None,
            stop,
            designer,
        };
        e.prepare(sample_rate, s, &Output::default());
        e
    }

    /// Sizes everything for `sample_rate`. Allocates.
    pub fn prepare(&mut self, sample_rate: f32, s: &ShapeSettings, out: &Output) {
        self.sample_rate = sample_rate;
        let max_latency = (MAX_LOOKAHEAD_MS * 0.001 * sample_rate).ceil() as usize + 1 + Oversampler::latency_for(CLIP_OVERSAMPLING) + linear_phase_latency(sample_rate);
        self.dry = DryDelay::new(max_latency, CHUNK);
        let req = SplitRequest::new(s.crossovers, sample_rate);
        let fade = (KERNEL_FADE_MS * 0.001 * f64::from(sample_rate)) as usize;
        self.convs = (0..BANDS).map(|_| Convolver::new(req.block, req.len, fade)).collect();
        self.lp_bands = (0..BANDS).map(|_| [vec![0.0; CHUNK], vec![0.0; CHUNK]]).collect();
        self.spare = None;
        self.last_request = None;
        self.installed = None;
        self.linear = out.linear;
        if out.linear {
            // Designed here, so a render starts with them.
            let mut set = design_split(&req);
            for (conv, k) in self.convs.iter_mut().zip(set.sets.iter_mut()) {
                if let Some(k) = k.take() {
                    conv.replace(k);
                }
            }
            self.last_request = Some(req);
            self.installed = Some(req);
            self.requests.write(req);
        }
        self.wet = [vec![0.0; CHUNK], vec![0.0; CHUNK]];
        self.band_sum = [vec![0.0; CHUNK], vec![0.0; CHUNK]];
        self.hi = vec![0.0; CHUNK * CLIP_OVERSAMPLING];
        self.bypass_now = vec![0.0; CHUNK];
        self.clip_now = vec![(1.0, 0.0, 0.0); CHUNK];
        self.slice_len = ((HISTORY_SLICE_MS * 0.001 * sample_rate).round() as usize).max(1);
        for sm in [&mut self.delta, &mut self.bypass, &mut self.dip] {
            sm.set_ramp(sample_rate, SWITCH_MS);
        }
        for sm in [&mut self.mix, &mut self.gain, &mut self.pan, &mut self.auto, &mut self.ceiling, &mut self.softness] {
            sm.set_ramp(sample_rate, GLIDE_MS);
        }
        self.auto_k = 1.0 - (-1.0 / (AUTO_SECONDS * sample_rate)).exp();
        self.shaper.prepare(sample_rate, &self.active(s));
        self.reset(s, out);
    }

    /// Clears the state and snaps every glide to `s` and `out`.
    pub fn reset(&mut self, s: &ShapeSettings, out: &Output) {
        self.lookahead_ms = s.lookahead_ms;
        self.clip_on = out.clip;
        self.os.set_factor(if out.clip { CLIP_OVERSAMPLING } else { 1 });
        self.os.reset();
        self.ceiling.reset(db_to_gain(out.ceiling_db));
        self.softness.reset(out.softness);
        self.shaper.reset(s);
        self.switch = Switch::Steady;
        for conv in &mut self.convs {
            conv.reset();
        }
        self.dry.reset();
        self.dry.set_delay(self.latency());
        self.mix.reset(out.mix);
        self.delta.reset(if out.delta { 1.0 } else { 0.0 });
        self.bypass.reset(if out.bypass { 1.0 } else { 0.0 });
        self.gain.reset(db_to_gain(out.gain_db));
        self.pan.reset(out.pan);
        self.dip.reset(1.0);
        self.auto_in = 0.0;
        self.auto_out = 0.0;
        self.auto.reset(1.0);
        self.auto_count = 0;
        self.slice_pos = 0;
        self.acc = empty_record();
    }

    /// The latency in force, samples: the lookahead, plus the clip stage's oversampling while it
    /// is on, plus Linear Phase's split while it runs (each follows a change once the dip has
    /// faded out).
    pub fn latency(&self) -> usize {
        self.active(&ShapeSettings::default()).latency(self.sample_rate) + self.os.latency() + if self.linear { linear_phase_latency(self.sample_rate) } else { 0 }
    }

    /// Whether Linear Phase is running.
    pub fn linear(&self) -> bool {
        self.linear
    }

    /// Gives a set back to the designer to free; never frees on this thread.
    fn give_back(&mut self, set: Box<SplitKernels>) {
        if let Err(set) = self.kernels.retire(set) {
            if self.spare.is_none() {
                self.spare = Some(set);
            } else {
                // Nowhere to keep it: leaking is the only way that keeps the audio thread clean.
                std::mem::forget(set);
            }
        }
    }

    /// Once per stretch, while Linear Phase runs: asks for kernels when the crossovers move, takes
    /// a new set into every convolver at once (they fade together), and returns the old ones.
    fn update_kernels(&mut self, s: &ShapeSettings) {
        let req = SplitRequest::new(s.crossovers, self.sample_rate);
        if self.last_request != Some(req) {
            self.requests.write(req);
            self.last_request = Some(req);
        }
        let fading = self.convs.iter().any(Convolver::is_fading);
        if let Some(mut spare) = self.spare.take() {
            for (conv, slot) in self.convs.iter_mut().zip(spare.sets.iter_mut()) {
                if let Some(old) = conv.retired() {
                    *slot = Some(old);
                }
            }
            if fading {
                self.spare = Some(spare);
            } else {
                self.give_back(spare);
            }
            return;
        }
        // Only while it runs: switching into it, the kernels wait in the handover for
        // `install_kernels`.
        if fading || self.switch != Switch::Steady || !self.linear {
            return;
        }
        if let Some(mut set) = self.kernels.take() {
            if set.request.block != req.block || set.request.len != req.len || set.request.sample_rate != req.sample_rate {
                self.give_back(set);
                return;
            }
            for (conv, slot) in self.convs.iter_mut().zip(set.sets.iter_mut()) {
                if let Some(k) = slot.take() {
                    if let Err(k) = conv.offer(k) {
                        *slot = Some(k);
                    }
                }
            }
            self.installed = Some(set.request);
            self.spare = Some(set);
        }
    }

    /// Switching into Linear Phase behind the dip: the convolvers start empty, with the kernels
    /// they kept if those still fit the crossovers, else the newest designed (no fade). Whether
    /// they have kernels to run.
    fn install_kernels(&mut self, s: &ShapeSettings) -> bool {
        let req = SplitRequest::new(s.crossovers, self.sample_rate);
        if self.installed == Some(req) {
            for conv in &mut self.convs {
                conv.reset();
            }
            return true;
        }
        let Some(mut set) = self.kernels.take() else {
            if self.last_request != Some(req) || self.installed.is_some() {
                self.requests.write(req);
                self.last_request = Some(req);
            }
            return false;
        };
        if set.request.sample_rate != f64::from(self.sample_rate) {
            self.give_back(set);
            return false;
        }
        for (conv, slot) in self.convs.iter_mut().zip(set.sets.iter_mut()) {
            if let Some(k) = slot.take() {
                *slot = conv.replace(k);
            }
            conv.reset();
        }
        self.installed = Some(set.request);
        self.give_back(set);
        true
    }

    /// The band layout playing now (for tests and tools).
    pub fn shaper(&self) -> &Shaper {
        &self.shaper
    }

    /// `s` with the lookahead in force (a change waits for the dip).
    fn active(&self, s: &ShapeSettings) -> ShapeSettings {
        ShapeSettings { lookahead_ms: self.lookahead_ms, ..*s }
    }

    /// Processes one host block in place, pushes the history records it completes, and the input
    /// and the output to `spectra` (mono each).
    #[allow(clippy::needless_range_loop)] // channels and scratch buffers indexed together
    pub fn process(&mut self, channels: &mut [&mut [f32]], s: &ShapeSettings, out: &Output, history: &HistoryTap<{ record::WIDTH }>, spectra: Option<[&SpectrumTap; 2]>) {
        let n_ch = channels.len().min(2);
        if n_ch == 0 {
            return;
        }
        let len = channels.iter().map(|c| c.len()).min().unwrap_or(0);
        if let Some([input, _]) = spectra {
            push_mono(input, channels);
        }
        // Listen replaces the output with what the detector hears: no mix, delta, bypass or auto
        // gain then.
        let (mix, delta, bypass) = if s.listen { (1.0, false, false) } else { (out.mix, out.delta, out.bypass) };
        self.mix.set_target(mix);
        self.delta.set_target(if delta { 1.0 } else { 0.0 });
        self.bypass.set_target(if bypass { 1.0 } else { 0.0 });
        self.gain.set_target(db_to_gain(out.gain_db));
        self.pan.set_target(if n_ch == 2 { out.pan } else { 0.0 });
        let auto_on = out.auto_gain && !s.listen;
        self.ceiling.set_target(db_to_gain(out.ceiling_db));
        self.softness.set_target(out.softness.clamp(0.0, 1.0));
        // Listen hears the detector as it is: no clip then.
        let clip = out.clip && !s.listen;

        let mut start = 0;
        while start < len {
            // A lookahead, clip or phase change (each moves the latency): fade out, switch once
            // silent (into Linear Phase once its kernels are in), let a new split fill, fade in.
            if out.linear || self.linear {
                self.update_kernels(s);
            }
            match self.switch {
                Switch::Steady if s.lookahead_ms != self.lookahead_ms || clip != self.clip_on || out.linear != self.linear => {
                    self.dip.set_target(0.0);
                    if self.dip.current() == 0.0 {
                        self.switch = Switch::Waiting;
                    }
                }
                Switch::Waiting => {
                    if out.linear && !self.linear && !self.install_kernels(s) {
                        // Still designing: stay muted.
                    } else {
                        self.lookahead_ms = s.lookahead_ms;
                        if clip != self.clip_on {
                            self.clip_on = clip;
                            self.os.set_factor(if clip { CLIP_OVERSAMPLING } else { 1 });
                            self.os.reset();
                        }
                        let refill = out.linear != self.linear;
                        if refill {
                            self.linear = out.linear;
                            self.shaper.reset(&self.active(s));
                        }
                        self.dry.set_delay(self.latency());
                        self.switch = Switch::Filling(if refill { self.latency() } else { 0 });
                    }
                }
                Switch::Filling(0) => {
                    self.switch = Switch::Steady;
                    self.dip.set_target(1.0);
                }
                _ => {}
            }
            let n = (len - start).min(CHUNK).min(self.slice_len - self.slice_pos);
            let n = match self.switch {
                Switch::Filling(left) if left > 0 => n.min(left),
                _ => n,
            };
            let end = start + n;
            for c in 0..n_ch {
                self.wet[c][..n].copy_from_slice(&channels[c][start..end]);
            }
            self.dry.write(&channels[0][start..end], (n_ch == 2).then(|| &channels[1][start..end]));
            let mut in_peak = [0.0f32; 2];
            for c in 0..n_ch {
                in_peak[c] = channels[c][start..end].iter().fold(0.0f32, |m, v| m.max(v.abs()));
            }
            let active = self.active(s);
            let telemetry = {
                let [a, b] = &mut self.wet;
                let [da, db] = &mut self.band_sum;
                let mut wet: [&mut [f32]; 2] = [&mut a[..n], &mut b[..n]];
                let mut dry: [&mut [f32]; 2] = [&mut da[..n], &mut db[..n]];
                if self.linear {
                    // Each band slot's own split of the input (mono runs as two equal sides).
                    for (conv, buf) in self.convs.iter_mut().zip(self.lp_bands.iter_mut()) {
                        let [l, r] = buf;
                        l[..n].copy_from_slice(&channels[0][start..end]);
                        r[..n].copy_from_slice(&channels[n_ch - 1][start..end]);
                        conv.process(&mut l[..n], &mut r[..n]);
                    }
                    let bands: [[&[f32]; 2]; BANDS] = std::array::from_fn(|b| [&self.lp_bands[b][0][..n], &self.lp_bands[b][1][..n]]);
                    self.shaper.process_bands(&mut wet[..n_ch], &mut dry[..n_ch], &active, &bands)
                } else {
                    self.shaper.process(&mut wet[..n_ch], &mut dry[..n_ch], &active)
                }
            };
            if let Switch::Filling(left) = self.switch {
                self.switch = Switch::Filling(left.saturating_sub(n));
            }
            let mut out_peak = [0.0f32; 2];
            for i in 0..n {
                // Auto gain: the band sum's loudness against the result's, over seconds.
                let (mut sum_in, mut sum_out) = (0.0f32, 0.0f32);
                for c in 0..n_ch {
                    sum_in += self.band_sum[c][i] * self.band_sum[c][i];
                    sum_out += self.wet[c][i] * self.wet[c][i];
                }
                self.auto_in += self.auto_k * (sum_in - self.auto_in);
                self.auto_out += self.auto_k * (sum_out - self.auto_out);
                if self.auto_count == 0 {
                    let target = if auto_on && self.auto_in > AUTO_GATE && self.auto_out > AUTO_GATE {
                        (self.auto_in / self.auto_out).sqrt().clamp(db_to_gain(-AUTO_LIMIT_DB), db_to_gain(AUTO_LIMIT_DB))
                    } else if auto_on {
                        self.auto.target()
                    } else {
                        1.0
                    };
                    self.auto.set_target(target);
                }
                self.auto_count = (self.auto_count + 1) % AUTO_EVERY;
                let auto = self.auto.next_value();
                let (mix, delta, bypass) = (self.mix.next_value(), self.delta.next_value(), self.bypass.next_value());
                let gain = self.gain.next_value() * self.dip.next_value();
                let (pl, pr) = balance(self.pan.next_value());
                self.bypass_now[i] = bypass;
                self.clip_now[i] = (self.ceiling.next_value(), self.softness.next_value(), delta);
                for c in 0..n_ch {
                    let wet = self.wet[c][i] * auto;
                    let bands = self.band_sum[c][i];
                    let dry = self.dry.read(c, i);
                    // Each blend as a·(1 − t) + b·t, so its ends are exact (mix 0 and bypass null).
                    let mixed = bands * (1.0 - mix) + wet * mix;
                    let y = mixed * (1.0 - delta) + (wet - bands) * delta;
                    let side = if n_ch == 2 { if c == 0 { pl } else { pr } } else { 1.0 };
                    // The result before the clip stage, and the dry path as bypass hears it.
                    self.wet[c][i] = y * gain * side;
                    self.band_sum[c][i] = dry * gain * side;
                }
            }
            // The clip stage, at the high rate.
            let mut clip_peak = [0.0f32; 2];
            if self.clip_on {
                let m = n * CLIP_OVERSAMPLING;
                for c in 0..n_ch {
                    clip_peak[c] = self.wet[c][..n].iter().fold(0.0f32, |p, v| p.max(v.abs()));
                    self.os.up(c, &self.wet[c][..n], &mut self.hi[..m]);
                    for (k, v) in self.hi[..m].iter_mut().enumerate() {
                        // Delta is heard unclipped: while it fades in, the clip fades out.
                        let (ceiling, soft, delta) = self.clip_now[k / CLIP_OVERSAMPLING];
                        let clipped = soft_clip(*v, ceiling, soft);
                        *v += (clipped - *v) * (1.0 - delta);
                    }
                    self.os.down(c, &self.hi[..m], &mut self.wet[c][..n]);
                }
            }
            for i in 0..n {
                let bypass = self.bypass_now[i];
                for c in 0..n_ch {
                    let v = self.wet[c][i] * (1.0 - bypass) + self.band_sum[c][i] * bypass;
                    out_peak[c] = out_peak[c].max(v.abs());
                    channels[c][start + i] = v;
                }
            }
            self.dry.advance(n);
            if n_ch == 1 {
                in_peak[1] = in_peak[0];
                out_peak[1] = out_peak[0];
            }
            if n_ch == 1 {
                clip_peak[1] = clip_peak[0];
            }
            self.accumulate(&telemetry, in_peak, out_peak, n_ch);
            if self.clip_on {
                for c in 0..2 {
                    let db = if clip_peak[c] > 0.0 { 20.0 * clip_peak[c].log10() } else { MINUS_INFINITY_DB };
                    self.acc[record::CLIP + c] = self.acc[record::CLIP + c].max(db);
                }
            }
            self.slice_pos += n;
            if self.slice_pos >= self.slice_len {
                history.push(&self.acc);
                self.acc = empty_record();
                self.slice_pos = 0;
            }
            start = end;
        }
        if let Some([_, output]) = spectra {
            push_mono(output, channels);
        }
    }

    fn accumulate(&mut self, t: &ShapeTelemetry, in_peak: [f32; 2], out_peak: [f32; 2], n_ch: usize) {
        let a = &mut self.acc;
        let db = |x: f32| if x > 0.0 { 20.0 * x.log10() } else { MINUS_INFINITY_DB };
        for c in 0..2 {
            a[record::IN + c] = a[record::IN + c].max(db(in_peak[c]));
            a[record::OUT + c] = a[record::OUT + c].max(db(out_peak[c]));
        }
        for b in 0..BANDS {
            let gains = if n_ch == 1 { [t.gain_db[b][0]; 2] } else { t.gain_db[b] };
            for (k, g) in [record::GAIN_A, record::GAIN_B].into_iter().zip(gains) {
                if g.abs() > a[k + b].abs() {
                    a[k + b] = g;
                }
            }
            a[record::WEIGHT + b] = a[record::WEIGHT + b].max(t.weight[b]);
            a[record::HIT + b] = a[record::HIT + b].max(t.hit[b]);
            a[record::LEVEL + b] = a[record::LEVEL + b].max(t.level_db[b]);
        }
    }
}

fn push_mono(tap: &SpectrumTap, channels: &[&mut [f32]]) {
    match channels {
        [l, r, ..] => tap.push(l.iter().zip(r.iter()).map(|(a, b)| 0.5 * (a + b))),
        [m] => tap.push(m.iter().copied()),
        [] => {}
    }
}

fn empty_record() -> [f32; record::WIDTH] {
    let mut r = [0.0; record::WIDTH];
    for i in [record::IN, record::IN + 1, record::OUT, record::OUT + 1, record::CLIP, record::CLIP + 1] {
        r[i] = MINUS_INFINITY_DB;
    }
    for b in 0..BANDS {
        r[record::LEVEL + b] = MINUS_INFINITY_DB;
    }
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    const FS: f32 = 48_000.0;

    fn shaping() -> ShapeSettings {
        let mut s = ShapeSettings::default();
        s.bands[0].attack_db = 6.0;
        s.bands[0].sustain_db = -6.0;
        s
    }

    fn noise(n: usize, seed: u32) -> Vec<f32> {
        let mut x = seed;
        (0..n)
            .map(|i| {
                x = x.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                ((x >> 8) as f32 / (1u32 << 24) as f32 - 0.5) * 0.5 * (-((i % 4800) as f32) / 700.0).exp()
            })
            .collect()
    }

    fn run(e: &mut ShapeEngine, input: &[f32], s: &ShapeSettings, out: &Output, block: usize) -> Vec<f32> {
        let tap = HistoryTap::new(1024);
        let mut y = input.to_vec();
        for chunk in y.chunks_mut(block) {
            e.process(&mut [chunk], s, out, &tap, None);
        }
        y
    }

    /// Linear Phase: with three bands and no shaping, the output is the input delayed by the
    /// latency (no phase turn at the crossovers); shaping still shapes; Minimum Phase turns the
    /// phase at a crossover where Linear Phase does not.
    #[test]
    fn linear_phase_splits_without_turning_the_phase() {
        let mut s = ShapeSettings::default();
        s.crossovers[0] = Some(120.0);
        s.crossovers[1] = Some(2000.0);
        let lp = Output { linear: true, ..Output::default() };
        let n = (FS * 1.5) as usize;
        let tone = |f: f32| -> Vec<f32> { (0..n).map(|i| 0.3 * (std::f32::consts::TAU * f * i as f32 / FS).sin()).collect() };
        let err = |y: &[f32], x: &[f32], d: usize| -> f32 {
            let from = d + (FS * 0.5) as usize;
            (from..n).map(|i| (y[i] - x[i - d]).abs()).fold(0.0, f32::max)
        };
        let mut e = ShapeEngine::new(FS, &s);
        e.prepare(FS, &s, &lp);
        assert!(e.linear());
        let latency = e.latency();
        assert_eq!(latency, linear_phase_latency(FS));
        // On a crossover (a tone right at 120 Hz) and between them.
        for f in [120.0, 500.0, 2000.0, 60.0] {
            let x = tone(f);
            e.reset(&s, &lp);
            let y = run(&mut e, &x, &s, &lp, 333);
            assert!(err(&y, &x, latency) < 0.003, "{f} Hz: off by {}", err(&y, &x, latency));
        }
        // Minimum Phase at the crossover: flat in level but turned (180° at 120 Hz), far from the
        // input delayed.
        let mut m = ShapeEngine::new(FS, &s);
        let x = tone(120.0);
        let y = run(&mut m, &x, &s, &Output::default(), 333);
        assert!(err(&y, &x, m.latency()) > 0.2, "minimum phase turns it");
        // Shaping still shapes in Linear Phase.
        let mut sh = s;
        sh.bands[0].attack_db = 9.0;
        e.reset(&sh, &lp);
        let x = noise(n, 3);
        let y = run(&mut e, &x, &sh, &lp, 333);
        assert!(err(&y, &x, latency) > 0.01, "the low band's attacks moved");
    }

    #[test]
    fn switching_into_linear_phase_moves_the_latency_after_a_dip() {
        let mut s = ShapeSettings::default();
        s.crossovers[0] = Some(200.0);
        let mut e = ShapeEngine::new(FS, &s);
        let before = e.latency();
        let x = noise((FS * 2.0) as usize, 5);
        let lp = Output { linear: true, ..Output::default() };
        let tap = HistoryTap::new(1024);
        let mut y = x.clone();
        for chunk in y.chunks_mut(256) {
            e.process(&mut [chunk], &s, &lp, &tap, None);
        }
        assert!(e.linear(), "it switched (once its kernels were designed)");
        assert_eq!(e.latency(), before + linear_phase_latency(FS));
        assert!(y.iter().all(|v| v.is_finite()));
    }

    #[test]
    fn bypass_and_mix_zero_null_against_the_delayed_input() {
        let s = ShapeSettings { lookahead_ms: 3.0, ..shaping() };
        let input = noise(9600, 1);
        let latency = s.latency(FS);
        for out in [Output { bypass: true, ..Output::default() }, Output { mix: 0.0, ..Output::default() }] {
            let mut e = ShapeEngine::new(FS, &s);
            e.prepare(FS, &s, &out);
            let y = run(&mut e, &input, &s, &out, 256);
            assert_eq!(e.latency(), latency);
            for i in latency..input.len() {
                assert_eq!(y[i], input[i - latency], "{out:?} at {i}");
            }
        }
    }

    #[test]
    fn delta_plus_dry_is_the_wet_signal() {
        let s = shaping();
        let input = noise(9600, 2);
        let mut wet_engine = ShapeEngine::new(FS, &s);
        let wet = run(&mut wet_engine, &input, &s, &Output::default(), 480);
        let delta_out = Output { delta: true, ..Output::default() };
        let mut delta_engine = ShapeEngine::new(FS, &s);
        delta_engine.prepare(FS, &s, &delta_out);
        let delta = run(&mut delta_engine, &input, &s, &delta_out, 480);
        for i in 0..input.len() {
            assert!((delta[i] + input[i] - wet[i]).abs() < 1e-6, "at {i}");
        }
    }

    #[test]
    fn mix_zero_with_bands_is_the_band_sum_so_it_never_combs() {
        // Several bands at mix 0: the result is the band sum (flat), not dry against wet.
        let mut s = shaping();
        s.crossovers = [Some(200.0), Some(2000.0), None, None];
        let out = Output { mix: 0.0, ..Output::default() };
        let mut e = ShapeEngine::new(FS, &s);
        e.prepare(FS, &s, &out);
        let mut impulse = vec![0.0f32; 16_384];
        impulse[0] = 1.0;
        let y = run(&mut e, &impulse, &s, &out, 512);
        for f in [100.0f64, 200.0, 1000.0, 2000.0, 9000.0] {
            let w = std::f64::consts::TAU * f / f64::from(FS);
            let (re, im) = y.iter().enumerate().fold((0.0, 0.0), |(re, im), (i, &v)| (re + f64::from(v) * (w * i as f64).cos(), im - f64::from(v) * (w * i as f64).sin()));
            let db = 10.0 * (re * re + im * im).log10();
            assert!(db.abs() < 0.01, "{f} Hz: {db}");
        }
    }

    #[test]
    fn block_size_does_not_change_the_output() {
        let mut s = ShapeSettings { lookahead_ms: 2.0, ..shaping() };
        s.crossovers[0] = Some(300.0);
        s.bands[1].sustain_db = 4.0;
        let input = noise(12_000, 3);
        let mut reference = None;
        for block in [1, 7, 64, 128, 4096] {
            let mut e = ShapeEngine::new(FS, &s);
            let y = run(&mut e, &input, &s, &Output { auto_gain: true, ..Output::default() }, block);
            match &reference {
                None => reference = Some(y),
                Some(r) => assert_eq!(&y, r, "block {block}"),
            }
        }
    }

    #[test]
    fn history_gets_one_record_per_slice() {
        let s = shaping();
        let mut e = ShapeEngine::new(FS, &s);
        let tap = HistoryTap::new(1024);
        let mut y = noise(48_000, 4);
        for chunk in y.chunks_mut(333) {
            e.process(&mut [chunk], &s, &Output::default(), &tap, None);
        }
        // 1 s of 5 ms slices.
        assert_eq!(tap.count(), 200);
        let (mut seen, mut up, mut down, mut hits) = (0, 0.0f32, 0.0f32, 0);
        tap.read_since(&mut seen, |r| {
            up = up.max(r[record::GAIN_A]);
            down = down.min(r[record::GAIN_A]);
            hits += usize::from(r[record::HIT] > 0.3);
        });
        assert!(up > 3.0 && down < -3.0, "{up} {down}");
        // Ten bursts, a tick each.
        assert!((9..=11).contains(&hits), "{hits}");
    }

    #[test]
    fn a_lookahead_change_moves_the_latency_after_a_dip() {
        let mut s = shaping();
        let mut e = ShapeEngine::new(FS, &s);
        let input = noise(4800, 5);
        run(&mut e, &input, &s, &Output::default(), 480);
        assert_eq!(e.latency(), 0);
        s.lookahead_ms = 5.0;
        run(&mut e, &input, &s, &Output::default(), 480);
        assert_eq!(e.latency(), 240);
    }

    #[test]
    fn auto_gain_brings_the_loudness_back() {
        let mut s = ShapeSettings::default();
        s.bands[0].attack_db = -9.0;
        s.bands[0].sustain_db = -9.0;
        // Bursts of noise, 8 per second: all onset and decay.
        let mut x = 6u32;
        let input: Vec<f32> = (0..96_000)
            .map(|i| {
                x = x.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                ((x >> 8) as f32 / (1u32 << 24) as f32 - 0.5) * 0.5 * (-((i % 6000) as f32) / 1800.0).exp()
            })
            .collect();
        let rms = |v: &[f32]| (v.iter().map(|x| x * x).sum::<f32>() / v.len() as f32).sqrt();
        let mut plain = ShapeEngine::new(FS, &s);
        let quiet = run(&mut plain, &input, &s, &Output::default(), 512);
        let mut auto = ShapeEngine::new(FS, &s);
        let level = run(&mut auto, &input, &s, &Output { auto_gain: true, ..Output::default() }, 512);
        let tail = 48_000..;
        let (a, b, c) = (rms(&input[tail.clone()]), rms(&quiet[tail.clone()]), rms(&level[tail]));
        assert!(20.0 * (b / a).log10() < -2.0, "the cut is heard without it");
        assert!((20.0 * (c / a).log10()).abs() < 1.0, "{} dB", 20.0 * (c / a).log10());
    }
}
