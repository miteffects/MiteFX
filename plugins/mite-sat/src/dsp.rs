//! Mite Sat's engine: the saturator (`mite_dsp::sat`) and its output stage (mix and delta against
//! the band sum, bypass against the latency-aligned dry signal, output gain and balance), the
//! oversampling and phase switches behind a dip (they move the latency), and the history
//! telemetry: one record per 5 ms slice, whatever the host's block size.
//!
//! Linear Phase splits the bands with FIR kernels (`mite_dsp::crossover::linear_phase_kernels`,
//! each band the minimum-phase split's LR4 magnitude with zero phase, the sum a pure delay) run by
//! one convolver per band slot. A designer thread makes them whenever the crossovers move; the
//! audio thread swaps them in with a crossfade and hands the old ones back to be freed there.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use mite_core::{triple_buffer, Handover, HistoryTap, SpectrumTap, TripleReader, TripleWriter};
use mite_dsp::conv::{Convolver, KernelLayout, KernelSet};
use mite_dsp::output::{balance, DryDelay};
use mite_dsp::oversample::Oversampler;
use mite_dsp::sat::{Layout, SatSettings, SatTelemetry, Saturator, BANDS, CROSSOVERS};
use mite_dsp::{db_to_gain, LinearSmoother, MINUS_INFINITY_DB};

/// Linear Phase's kernel length and convolution block at 48 kHz (both scale with the sample
/// rate): 8192 taps hold an LR4 band down to the lowest crossovers within 0.3 dB (tested in
/// `mite_dsp::crossover`); the latency is a block plus half the kernel, 90.7 ms.
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
    /// Muted, waiting for Linear Phase's first kernels.
    Waiting,
    /// Muted while the new path fills (its latency), so the sound comes back with the fade.
    Filling(usize),
}

/// Fields of a history record (`SatShared::history`).
pub mod record {
    use mite_dsp::sat::BANDS;

    /// Peak in and out, L and R, dBFS.
    pub const IN: usize = 0;
    pub const OUT: usize = 2;
    /// Per band slot: its peak going in (linear: the transfer curve's x) and coming out, dBFS.
    pub const BAND_IN: usize = 4;
    pub const BAND_OUT: usize = BAND_IN + BANDS;
    /// Per band slot: the operating point the curve sits at (Tube's drifts).
    pub const POINT: usize = BAND_OUT + BANDS;
    /// Per band slot: auto gain's correction, linear.
    pub const AUTO: usize = POINT + BANDS;
    /// Per band slot: the dynamics' gain, linear, and their automatic threshold (linear, 0 before
    /// the band has been heard).
    pub const DYN: usize = AUTO + BANDS;
    pub const DYN_THR: usize = DYN + BANDS;
    pub const WIDTH: usize = DYN_THR + BANDS;
}

/// One history record per this much audio.
pub const HISTORY_SLICE_MS: f32 = 5.0;
/// Records kept: 30 s.
pub const HISTORY_RECORDS: usize = 6000;
/// Longest chunk the engine processes at once (scratch buffers are this long).
const CHUNK: usize = 512;
/// Switch fades (delta, bypass, the oversampling dip) and output glides (mix, gain, balance), ms.
const SWITCH_MS: f32 = 10.0;
const GLIDE_MS: f32 = 20.0;

/// The output stage as the parameters set it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Output {
    /// 0…1.
    pub mix: f32,
    pub delta: bool,
    pub bypass: bool,
    pub gain_db: f32,
    /// −1 … 1.
    pub pan: f32,
}

impl Default for Output {
    fn default() -> Self {
        Self { mix: 1.0, delta: false, bypass: false, gain_db: 0.0, pan: 0.0 }
    }
}

pub struct SatEngine {
    sample_rate: f32,
    sat: Saturator,
    dry: DryDelay,
    wet: [Vec<f32>; 2],
    band_sum: [Vec<f32>; 2],
    mix: LinearSmoother,
    delta: LinearSmoother,
    bypass: LinearSmoother,
    gain: LinearSmoother,
    pan: LinearSmoother,
    /// The oversampling and phase switch: fades out, changes, fades back in.
    dip: LinearSmoother,
    switch: Switch,
    slice_len: usize,
    slice_pos: usize,
    acc: [f32; record::WIDTH],
    /// Linear Phase: running, its convolvers (one per band slot) and their outputs, the kernels'
    /// way in and out, the request last sent, a set whose old kernels are still fading out (and
    /// coming back into it), and the designer thread.
    linear: bool,
    convs: Vec<Convolver>,
    lp_bands: Vec<[Vec<f32>; 2]>,
    kernels: Arc<Handover<SplitKernels>>,
    requests: TripleWriter<SplitRequest>,
    last_request: Option<SplitRequest>,
    spare: Option<Box<SplitKernels>>,
    stop: Arc<AtomicBool>,
    designer: Option<JoinHandle<()>>,
}

impl Drop for SatEngine {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.designer.take() {
            let _ = t.join();
        }
    }
}

impl SatEngine {
    /// Allocates and starts the kernel designer; call off the audio thread.
    pub fn new(sample_rate: f32, factor: usize, s: &SatSettings) -> Self {
        let kernels = Arc::new(Handover::new());
        let stop = Arc::new(AtomicBool::new(false));
        let (requests, reader) = triple_buffer(SplitRequest::new(s.crossovers, sample_rate));
        let designer = {
            let (kernels, stop) = (kernels.clone(), stop.clone());
            std::thread::Builder::new().name("mite-sat-split".into()).spawn(move || run_designer(reader, kernels, stop)).ok()
        };
        let mut e = Self {
            sample_rate,
            sat: Saturator::new(sample_rate, factor, s),
            dry: DryDelay::new(1, 1),
            wet: [Vec::new(), Vec::new()],
            band_sum: [Vec::new(), Vec::new()],
            mix: LinearSmoother::new(1.0),
            delta: LinearSmoother::new(0.0),
            bypass: LinearSmoother::new(0.0),
            gain: LinearSmoother::new(1.0),
            pan: LinearSmoother::new(0.0),
            dip: LinearSmoother::new(1.0),
            switch: Switch::Steady,
            slice_len: 1,
            slice_pos: 0,
            acc: empty_record(),
            linear: false,
            convs: Vec::new(),
            lp_bands: Vec::new(),
            kernels,
            requests,
            last_request: None,
            spare: None,
            stop,
            designer,
        };
        e.prepare(sample_rate, factor, s, &Output::default(), false);
        e
    }

    /// Sizes everything for `sample_rate`; with `linear`, designs Linear Phase's kernels here (so a
    /// render starts with them). Allocates.
    pub fn prepare(&mut self, sample_rate: f32, factor: usize, s: &SatSettings, out: &Output, linear: bool) {
        self.sample_rate = sample_rate;
        self.dry = DryDelay::new(Oversampler::latency_for(8) + linear_phase_latency(sample_rate) + 1, CHUNK);
        let req = SplitRequest::new(s.crossovers, sample_rate);
        let fade = (KERNEL_FADE_MS * 0.001 * f64::from(sample_rate)) as usize;
        self.convs = (0..BANDS).map(|_| Convolver::new(req.block, req.len, fade)).collect();
        self.lp_bands = (0..BANDS).map(|_| [vec![0.0; CHUNK], vec![0.0; CHUNK]]).collect();
        self.spare = None;
        self.last_request = None;
        self.linear = linear;
        if linear {
            let mut set = design_split(&req);
            for (conv, k) in self.convs.iter_mut().zip(set.sets.iter_mut()) {
                if let Some(k) = k.take() {
                    conv.replace(k);
                }
            }
            self.last_request = Some(req);
            self.requests.write(req);
        }
        self.wet = [vec![0.0; CHUNK], vec![0.0; CHUNK]];
        self.band_sum = [vec![0.0; CHUNK], vec![0.0; CHUNK]];
        self.slice_len = ((HISTORY_SLICE_MS * 0.001 * sample_rate).round() as usize).max(1);
        for sm in [&mut self.delta, &mut self.bypass, &mut self.dip] {
            sm.set_ramp(sample_rate, SWITCH_MS);
        }
        for sm in [&mut self.mix, &mut self.gain, &mut self.pan] {
            sm.set_ramp(sample_rate, GLIDE_MS);
        }
        self.sat.prepare(sample_rate, factor, s);
        self.reset(s, out);
    }

    /// Clears the state and snaps every glide to `s` and `out`.
    pub fn reset(&mut self, s: &SatSettings, out: &Output) {
        self.sat.reset(s);
        self.dry.reset();
        self.dry.set_delay(self.latency());
        self.mix.reset(out.mix);
        self.delta.reset(if out.delta { 1.0 } else { 0.0 });
        self.bypass.reset(if out.bypass { 1.0 } else { 0.0 });
        self.gain.reset(db_to_gain(out.gain_db));
        self.pan.reset(out.pan);
        self.dip.reset(1.0);
        self.switch = Switch::Steady;
        for conv in &mut self.convs {
            conv.reset();
        }
        self.slice_pos = 0;
        self.acc = empty_record();
    }

    /// The latency in force, samples: the oversampling's round trip, plus Linear Phase's split
    /// (each follows a change once the dip has faded out).
    pub fn latency(&self) -> usize {
        self.sat.latency() + if self.linear { linear_phase_latency(self.sample_rate) } else { 0 }
    }

    /// Whether Linear Phase is running.
    pub fn linear(&self) -> bool {
        self.linear
    }

    /// Gives a set back to the designer to free; never frees on this thread (kept for the next
    /// try when the way back is full).
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
    fn update_kernels(&mut self, s: &SatSettings) {
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
        // Only while Linear Phase runs: switching into it, the kernels must wait in the handover
        // for `install_kernels` (taken here, they went into idle convolvers and the switch waited,
        // muted, for kernels that never came — 2026-11-10).
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
            self.spare = Some(set);
        }
    }

    /// Switching into Linear Phase behind the dip: the newest kernels go in at once (no fade; the
    /// convolvers start empty). Whether there were any.
    fn install_kernels(&mut self) -> bool {
        let Some(mut set) = self.kernels.take() else { return false };
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
        self.give_back(set);
        true
    }

    /// The oversampling in force.
    pub fn oversampling(&self) -> usize {
        self.sat.oversampling()
    }

    /// The saturator (for tests and tools).
    pub fn saturator(&self) -> &Saturator {
        &self.sat
    }

    /// Processes one host block in place at oversampling `factor`, pushes the history records it
    /// completes, and the input and the output to `spectra` (mono each).
    #[allow(clippy::needless_range_loop)] // channels and scratch buffers indexed together
    #[allow(clippy::too_many_arguments)] // the block, the settings the plugin reads, and the taps
    pub fn process(&mut self, channels: &mut [&mut [f32]], s: &SatSettings, factor: usize, linear: bool, out: &Output, history: &HistoryTap<{ record::WIDTH }>, spectra: Option<[&SpectrumTap; 2]>) {
        let n_ch = channels.len().min(2);
        if n_ch == 0 {
            return;
        }
        let len = channels.iter().map(|c| c.len()).min().unwrap_or(0);
        if let Some([input, _]) = spectra {
            push_mono(input, channels);
        }
        self.mix.set_target(out.mix);
        self.delta.set_target(if out.delta { 1.0 } else { 0.0 });
        self.bypass.set_target(if out.bypass { 1.0 } else { 0.0 });
        self.gain.set_target(db_to_gain(out.gain_db));
        self.pan.set_target(if n_ch == 2 { out.pan } else { 0.0 });

        let mut start = 0;
        while start < len {
            // An oversampling or phase change moves the latency: fade out, switch once silent (into
            // Linear Phase once its kernels are in), let the new path fill, fade in.
            if linear || self.linear {
                self.update_kernels(s);
            }
            match self.switch {
                Switch::Steady if factor != self.sat.oversampling() || linear != self.linear => {
                    self.dip.set_target(0.0);
                    if self.dip.current() == 0.0 {
                        self.switch = Switch::Waiting;
                    }
                }
                Switch::Waiting => {
                    if linear && !self.linear && !self.install_kernels() {
                        // Still designing: stay muted.
                    } else {
                        self.linear = linear;
                        if factor != self.sat.oversampling() {
                            self.sat.set_oversampling(factor, s);
                        } else {
                            self.sat.reset(s);
                        }
                        self.dry.set_delay(self.latency());
                        self.switch = Switch::Filling(self.latency());
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
                Switch::Filling(left) => n.min(left.max(1)),
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
                    self.sat.process_bands(&mut wet[..n_ch], &mut dry[..n_ch], s, &bands)
                } else {
                    self.sat.process(&mut wet[..n_ch], &mut dry[..n_ch], s)
                }
            };
            if let Switch::Filling(left) = self.switch {
                self.switch = Switch::Filling(left.saturating_sub(n));
            }
            let mut out_peak = [0.0f32; 2];
            for i in 0..n {
                let (mix, delta, bypass) = (self.mix.next_value(), self.delta.next_value(), self.bypass.next_value());
                let gain = self.gain.next_value() * self.dip.next_value();
                let (pl, pr) = balance(self.pan.next_value());
                for c in 0..n_ch {
                    let wet = self.wet[c][i];
                    let bands = self.band_sum[c][i];
                    let dry = self.dry.read(c, i);
                    // Each blend as a·(1 − t) + b·t, so its ends are exact (mix 0 and bypass null).
                    let mixed = bands * (1.0 - mix) + wet * mix;
                    let y = mixed * (1.0 - delta) + (wet - bands) * delta;
                    let side = if n_ch == 2 { if c == 0 { pl } else { pr } } else { 1.0 };
                    let v = (y * (1.0 - bypass) + dry * bypass) * gain * side;
                    out_peak[c] = out_peak[c].max(v.abs());
                    channels[c][start + i] = v;
                }
            }
            self.dry.advance(n);
            if n_ch == 1 {
                in_peak[1] = in_peak[0];
                out_peak[1] = out_peak[0];
            }
            self.accumulate(&telemetry, in_peak, out_peak);
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

    fn accumulate(&mut self, t: &SatTelemetry, in_peak: [f32; 2], out_peak: [f32; 2]) {
        let a = &mut self.acc;
        let db = |x: f32| if x > 0.0 { 20.0 * x.log10() } else { MINUS_INFINITY_DB };
        for c in 0..2 {
            a[record::IN + c] = a[record::IN + c].max(db(in_peak[c]));
            a[record::OUT + c] = a[record::OUT + c].max(db(out_peak[c]));
        }
        for b in 0..BANDS {
            a[record::BAND_IN + b] = a[record::BAND_IN + b].max(db(t.in_peak[b]));
            a[record::BAND_OUT + b] = a[record::BAND_OUT + b].max(db(t.out_peak[b]));
            a[record::POINT + b] = t.point[b];
            a[record::AUTO + b] = t.auto[b];
            a[record::DYN + b] = t.dynamics[b];
            a[record::DYN_THR + b] = t.threshold[b];
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
    for i in [record::IN, record::IN + 1, record::OUT, record::OUT + 1] {
        r[i] = MINUS_INFINITY_DB;
    }
    for b in 0..BANDS {
        r[record::BAND_IN + b] = MINUS_INFINITY_DB;
        r[record::BAND_OUT + b] = MINUS_INFINITY_DB;
        r[record::AUTO + b] = 1.0;
        r[record::DYN + b] = 1.0;
    }
    r
}

#[cfg(test)]
mod tests {
    use super::*;
    use mite_dsp::sat::Model;

    const FS: f32 = 48_000.0;

    fn driven() -> SatSettings {
        let mut s = SatSettings::default();
        s.bands[0].model = Model::Hard;
        s.bands[0].drive_db = 18.0;
        s
    }

    fn noise(n: usize, seed: u32) -> Vec<f32> {
        let mut x = seed;
        (0..n)
            .map(|_| {
                x = x.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                ((x >> 8) as f32 / (1u32 << 24) as f32 - 0.5) * 0.8
            })
            .collect()
    }

    fn run(e: &mut SatEngine, input: &[f32], s: &SatSettings, factor: usize, out: &Output, block: usize) -> Vec<f32> {
        run_phase(e, input, s, factor, false, out, block)
    }

    fn run_phase(e: &mut SatEngine, input: &[f32], s: &SatSettings, factor: usize, linear: bool, out: &Output, block: usize) -> Vec<f32> {
        let tap = HistoryTap::new(1024);
        let mut y = input.to_vec();
        for chunk in y.chunks_mut(block) {
            e.process(&mut [chunk], s, factor, linear, out, &tap, None);
        }
        y
    }

    #[test]
    fn linear_phase_bands_add_back_to_the_input_delayed() {
        // Four bands split in linear phase, mix 0: the band sum is the input, delayed by exactly
        // the latency reported (the minimum-phase split gives an allpass of it instead).
        let mut s = driven();
        s.crossovers = [Some(120.0), Some(1000.0), Some(6000.0), None, None];
        let input = noise(48_000, 11);
        let out = Output { mix: 0.0, ..Output::default() };
        let mut e = SatEngine::new(FS, 2, &s);
        e.prepare(FS, 2, &s, &out, true);
        let y = run_phase(&mut e, &input, &s, 2, true, &out, 300);
        let latency = e.latency();
        assert_eq!(latency, Oversampler::latency_for(2) + linear_phase_latency(FS));
        let worst = (latency..input.len()).map(|i| (y[i] - input[i - latency]).abs()).fold(0.0f32, f32::max);
        assert!(worst < 1e-5, "worst {worst}");
    }

    #[test]
    fn switching_to_linear_phase_in_small_blocks_comes_back_with_sound() {
        // As in a host: small blocks, the designer's kernels arriving while the dip still fades.
        let s = driven();
        let mut e = SatEngine::new(FS, 2, &s);
        let input = noise(64, 3);
        let tap = HistoryTap::new(1024);
        let mut heard = 0.0f32;
        for k in 0..3000 {
            let mut y = input.clone();
            e.process(&mut [&mut y[..]], &s, 2, true, &Output::default(), &tap, None);
            if k == 0 {
                // The dip has begun (64 of its 480 samples); the kernels arrive before it ends.
                std::thread::sleep(std::time::Duration::from_millis(300));
            }
            if k > 2500 {
                heard = heard.max(y.iter().fold(0.0f32, |m, v| m.max(v.abs())));
            }
        }
        assert!(e.linear(), "never switched");
        assert!(heard > 0.1, "silent: {heard}");
    }

    #[test]
    fn a_phase_change_moves_the_latency_after_a_dip() {
        let s = driven();
        let mut e = SatEngine::new(FS, 2, &s);
        let input = noise(4800, 5);
        run(&mut e, &input, &s, 2, &Output::default(), 480);
        assert_eq!(e.latency(), 69);
        // Into Linear Phase once the designer has handed its kernels over.
        for _ in 0..200 {
            run_phase(&mut e, &input, &s, 2, true, &Output::default(), 480);
            if e.linear() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(e.linear());
        assert_eq!(e.latency(), 69 + linear_phase_latency(FS));
        run_phase(&mut e, &input, &s, 2, false, &Output::default(), 480);
        assert_eq!(e.latency(), 69);
    }

    #[test]
    fn bypass_and_mix_zero_null_against_the_delayed_input() {
        let s = driven();
        let input = noise(9600, 1);
        for factor in [1, 2, 4, 8] {
            for out in [Output { bypass: true, ..Output::default() }, Output { mix: 0.0, ..Output::default() }] {
                let mut e = SatEngine::new(FS, factor, &s);
                e.prepare(FS, factor, &s, &out, false);
                let y = run(&mut e, &input, &s, factor, &out, 256);
                let latency = e.latency();
                assert_eq!(latency, Oversampler::latency_for(factor));
                for i in latency..input.len() {
                    assert_eq!(y[i], input[i - latency], "{factor}× {out:?} at {i}");
                }
            }
        }
    }

    #[test]
    fn delta_plus_dry_is_the_wet_signal() {
        let s = driven();
        let input = noise(9600, 2);
        let mut wet_engine = SatEngine::new(FS, 2, &s);
        let wet = run(&mut wet_engine, &input, &s, 2, &Output::default(), 480);
        let delta_out = Output { delta: true, ..Output::default() };
        let mut delta_engine = SatEngine::new(FS, 2, &s);
        delta_engine.prepare(FS, 2, &s, &delta_out, false);
        let delta = run(&mut delta_engine, &input, &s, 2, &delta_out, 480);
        let lat = wet_engine.latency();
        for i in lat..input.len() {
            assert!((delta[i] + input[i - lat] - wet[i]).abs() < 1e-6, "at {i}");
        }
    }

    #[test]
    fn block_size_does_not_change_the_output() {
        let mut s = driven();
        s.crossovers[0] = Some(300.0);
        s.bands[1].drive_db = 9.0;
        let input = noise(12_000, 3);
        let mut reference = None;
        for block in [1, 7, 64, 128, 4096] {
            let mut e = SatEngine::new(FS, 4, &s);
            let y = run(&mut e, &input, &s, 4, &Output { mix: 0.7, ..Output::default() }, block);
            match &reference {
                None => reference = Some(y),
                Some(r) => assert_eq!(&y, r, "block {block}"),
            }
        }
    }

    #[test]
    fn an_oversampling_change_moves_the_latency_after_a_dip() {
        let s = driven();
        let mut e = SatEngine::new(FS, 2, &s);
        let input = noise(4800, 5);
        run(&mut e, &input, &s, 2, &Output::default(), 480);
        assert_eq!(e.latency(), 69);
        run(&mut e, &input, &s, 8, &Output::default(), 480);
        assert_eq!((e.latency(), e.oversampling()), (78, 8));
    }

    #[test]
    fn history_gets_one_record_per_slice() {
        let s = driven();
        let mut e = SatEngine::new(FS, 2, &s);
        let tap = HistoryTap::new(1024);
        let mut y = noise(48_000, 4);
        for chunk in y.chunks_mut(333) {
            e.process(&mut [chunk], &s, 2, false, &Output::default(), &tap, None);
        }
        assert_eq!(tap.count(), 200);
        let (mut seen, mut band_in) = (0, f32::MIN);
        tap.read_since(&mut seen, |r| band_in = band_in.max(r[record::BAND_IN]));
        // The noise peaks at 0.4 (−8 dBFS).
        assert!((band_in + 7.96).abs() < 0.5, "{band_in}");
    }
}
