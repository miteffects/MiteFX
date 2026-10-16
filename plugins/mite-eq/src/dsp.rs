//! The Mite EQ engine: 24 bands in series, output gain, meters and analyzer taps, in Zero Latency
//! (IIR), Natural Phase (IIR plus a short correction FIR that brings magnitude and phase to the
//! analog prototypes) or Linear Phase (partitioned convolution) mode. Free of the plugin framework so it can be
//! tested directly.
//!
//! Natural and Linear Phase: a worker thread designs kernels from the latest band settings (sent through a
//! triple buffer) and hands them over; each new kernel crossfades in. Dynamic bands keep their
//! static gain in the kernel and move through a gain-only IIR filter after it. Changing mode or
//! resolution changes the latency, so the output fades out, switches, and fades back in once the
//! new kernel is in place.
//!
//! Zero Latency correction: the same worker designs a minimum-phase FIR (no delay) that brings the
//! matched IIR bands' magnitude to the analog prototypes up to Nyquist (magnitude only; the phase
//! stays minimum phase); it crossfades in after the bands whenever their static settings change.
//!
//! Spectral stage: while any band works per frequency bin (a spectral compressor), an STFT runs after the EQ chain and adds its frame length to the latency. Turning it on
//! or off goes through the same dip as a mode change.
//!
//! Output stage ([`Output`]): auto gain, mix and delta against the dry signal delayed by the
//! latency, band solo, output gain and balance, then bypass (also latency-aligned). Blocks run
//! in chunks of at most [`CHUNK`] samples.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use mite_core::{triple_buffer, Handover, SpectrumTap, TripleReader, TripleWriter};
use mite_dsp::conv::{Convolver, KernelSet};
use mite_dsp::eq::{BandProcessor, BandSettings, DetectSource, Placement};
use mite_dsp::min_phase::{max_taps, zero_latency_correction, Correction, CorrectionFilter, TAPS_48K};
use mite_dsp::output::{balance, isolate, loudness_change_db, solo_filter, DryDelay};
use mite_dsp::analyzer::{spectrum_point_freq, SPECTRUM_POINTS};
use mite_dsp::spectral::SpectralBands;
use mite_dsp::stft::Stft;
use mite_dsp::{db_to_gain, linear_phase, natural_phase, LinearSmoother, PeakMeter};

use crate::params::{BANDS, FREQ, OUT_GAIN};

/// Processing modes, in the order of the `mode` parameter's choices.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PhaseMode {
    ZeroLatency,
    NaturalPhase,
    LinearPhase,
}

impl PhaseMode {
    pub const NAMES: [&'static str; 3] = ["Zero Latency", "Natural Phase", "Linear Phase"];

    pub fn from_index(i: usize) -> Self {
        match i {
            1 => Self::NaturalPhase,
            2 => Self::LinearPhase,
            _ => Self::ZeroLatency,
        }
    }

    /// Whether this mode runs a convolution kernel (and so has latency).
    fn convolves(self) -> bool {
        self != Self::ZeroLatency
    }
}

/// Linear-phase resolutions (the `lp_res` choices): kernel lengths at 48 kHz.
pub const LP_RESOLUTIONS: [&str; 4] = ["Low", "Medium", "High", "Max"];
const LP_LENGTHS_48K: [usize; 4] = [1024, 2048, 4096, 8192];
/// Convolution partition sizes at 48 kHz, samples (part of the latency). Natural Phase's kernel
/// is short, so it uses smaller partitions for less latency.
const LP_BLOCK_48K: usize = 128;
const NP_BLOCK_48K: usize = 64;
/// Kernel swaps and the mode-switch dip, ms.
const KERNEL_FADE_MS: f64 = 20.0;
const SWITCH_FADE_MS: f32 = 10.0;
/// Chunk size for mono input through the stereo convolver.
const MONO_CHUNK: usize = 256;
/// Spectral stage frame length at 48 kHz, samples (also its latency).
const STFT_48K: usize = 1024;
/// The engine processes long host blocks in chunks of this many samples (the dry path and the
/// solo scratch are sized for it).
pub const CHUNK: usize = 512;
/// Fades of bypass, delta and solo, ms; auto gain's glide to a new compensation, ms, and the
/// least time between two of its recalculations while the curve moves, samples at 48 kHz.
const OUTPUT_FADE_MS: f32 = 10.0;
const AUTO_GAIN_GLIDE_MS: f32 = 50.0;
const AUTO_GAIN_INTERVAL_48K: usize = 1024;
/// Auto gain never compensates more than this, dB.
const AUTO_GAIN_LIMIT_DB: f64 = 24.0;

/// The global output settings (BLUEPRINT §8.4), as the host parameters describe them.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Output {
    /// Dry/wet, 0…1.
    pub mix: f32,
    /// Play what the EQ changes (the wet signal minus the dry) instead of the result.
    pub delta: bool,
    /// The whole plugin bypassed (latency kept).
    pub bypass: bool,
    /// Compensate the static curve's loudness change.
    pub auto_gain: bool,
    /// Output balance, −1…1.
    pub pan: f32,
    /// The band being listened to alone.
    pub solo: Option<usize>,
}

impl Default for Output {
    fn default() -> Self {
        Self { mix: 1.0, delta: false, bypass: false, auto_gain: false, pan: 0.0, solo: None }
    }
}

/// The power-of-two multiple of 48 kHz nearest `sample_rate` (1 at 44.1/48, 2 at 88.2/96 …).
fn rate_ratio(sample_rate: f64) -> usize {
    ((sample_rate / 48_000.0).round().max(1.0) as usize).next_power_of_two()
}

/// Kernel length for resolution `lp_res` at `sample_rate`.
pub fn kernel_len(lp_res: usize, sample_rate: f64) -> usize {
    LP_LENGTHS_48K[lp_res.min(LP_LENGTHS_48K.len() - 1)] * rate_ratio(sample_rate)
}

/// Linear Phase latency at `sample_rate`, samples: one convolution block plus half the kernel.
pub fn linear_phase_latency(lp_res: usize, sample_rate: f64) -> usize {
    LP_BLOCK_48K * rate_ratio(sample_rate) + kernel_len(lp_res, sample_rate) / 2
}

/// Natural Phase latency at `sample_rate`, samples: one (small) block plus the correction's delay.
pub fn natural_phase_latency(sample_rate: f64) -> usize {
    (NP_BLOCK_48K + natural_phase::DELAY_48K) * rate_ratio(sample_rate)
}

/// The latency the spectral stage adds at `sample_rate`, samples.
pub fn spectral_latency(sample_rate: f64) -> usize {
    STFT_48K * rate_ratio(sample_rate)
}

/// Whether any band needs the spectral stage.
fn needs_spectrum(settings: &[BandSettings; BANDS]) -> bool {
    settings.iter().any(BandSettings::uses_spectrum)
}

/// What runs: the phase mode, the linear-phase resolution, and whether the spectral stage is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Config {
    mode: PhaseMode,
    lp_res: usize,
    spectral: bool,
}

impl Config {
    /// Whether going from `self` to `other` changes the latency (and so needs the dip).
    fn differs(&self, other: &Self) -> bool {
        self.mode != other.mode || self.spectral != other.spectral || (self.mode == PhaseMode::LinearPhase && self.lp_res != other.lp_res)
    }
}

/// What the worker designs a kernel for: Linear Phase at `len` taps, or the Natural Phase
/// correction (`len` is then its delay).
#[derive(Debug, Clone, Copy, PartialEq)]
struct DesignRequest {
    mode: PhaseMode,
    bands: [BandSettings; BANDS],
    sample_rate: f64,
    len: usize,
    block: usize,
}

fn design_kernel(req: &DesignRequest) -> KernelSet {
    let k = if req.mode == PhaseMode::NaturalPhase {
        natural_phase::design(&req.bands, req.sample_rate, req.len)
    } else {
        linear_phase::design(&req.bands, req.sample_rate, req.len)
    };
    KernelSet::new(req.block, k.layout, &k.kernels, k.delay)
}

/// What the worker designs the Zero Latency correction for: the bands' static settings.
#[derive(Debug, Clone, Copy, PartialEq)]
struct CorrectionRequest {
    bands: [BandSettings; BANDS],
    sample_rate: f64,
    mono: bool,
}

/// Design grid for the correction at 48 kHz, points (scaled with the sample rate).
const CORRECTION_FFT_48K: usize = 8192;

fn design_correction(req: &CorrectionRequest) -> Correction {
    let ratio = rate_ratio(req.sample_rate);
    let lengths = TAPS_48K.map(|t| t * ratio);
    zero_latency_correction(&req.bands, req.sample_rate, &lengths, CORRECTION_FFT_48K * ratio, req.mono).unwrap_or_else(Correction::identity)
}

/// The kernel-design thread: designs whatever was requested last (convolution kernels and the
/// Zero Latency correction), hands it over, and frees what the audio thread retired.
fn run_designer(
    mut requests: TripleReader<DesignRequest>,
    kernels: Arc<Handover<KernelSet>>,
    mut corrections_in: TripleReader<CorrectionRequest>,
    corrections: Arc<Handover<Correction>>,
    stop: Arc<AtomicBool>,
) {
    while !stop.load(Ordering::Relaxed) {
        if let Some(req) = requests.read() {
            kernels.send(Box::new(design_kernel(&req)));
        }
        if let Some(req) = corrections_in.read() {
            corrections.send(Box::new(design_correction(&req)));
        }
        kernels.collect();
        corrections.collect();
        std::thread::sleep(Duration::from_millis(2));
    }
}

/// Where a mode or resolution change is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Switch {
    Steady,
    /// Fading out before the change.
    FadingOut,
    /// Muted, waiting for the new resolution's first kernel.
    Waiting,
    /// Muted while the new kernel's delay line (and the STFT's) fills, so the sound comes back
    /// with the fade-in instead of starting abruptly once the latency has passed.
    Filling,
}

pub struct EqEngine {
    bands: Vec<BandProcessor>,
    out_gain: LinearSmoother,
    meters: [PeakMeter; 2],
    sample_rate: f64,
    /// The configuration asked for, and the one running.
    wanted: Config,
    active: Config,
    switch: Switch,
    /// The mode-switch dip (1 = audible).
    fader: LinearSmoother,
    /// Linear Phase's convolver, and Natural Phase's (smaller blocks, short kernels).
    conv: Convolver,
    np_conv: Convolver,
    kernels: Arc<Handover<KernelSet>>,
    requests: TripleWriter<DesignRequest>,
    last_request: Option<DesignRequest>,
    /// Kernels the handover had no room to take back yet (retried every block).
    pending: [Option<Box<KernelSet>>; 4],
    stop: Arc<AtomicBool>,
    designer: Option<JoinHandle<()>>,
    /// The spectral stage.
    stft: Stft,
    spectral: SpectralBands,
    /// Right-channel stand-in for mono input through the stereo convolver and the STFT.
    mono_scratch: Vec<f32>,
    /// Samples processed since the last switch, and samples spent waiting muted for a kernel.
    filled: usize,
    waited: usize,
    /// The output stage: its settings, the latency-aligned dry signal and the fades.
    output: Output,
    dry: DryDelay,
    mix: LinearSmoother,
    delta: LinearSmoother,
    bypass: LinearSmoother,
    pan: [LinearSmoother; 2],
    /// Auto gain: the compensation (linear), the curve it was worked out for, and samples until
    /// it may be worked out again.
    auto_gain: LinearSmoother,
    loudness_key: Option<([BandSettings; BANDS], bool)>,
    loudness_wait: usize,
    /// Solo: the filter over the soloed band's region, the part of the stereo image the band
    /// hears, the fade into the solo, and the soloed signal's scratch.
    solo_filter: BandProcessor,
    solo_placement: Placement,
    solo: LinearSmoother,
    solo_scratch: [Vec<f32>; 2],
    /// Zero Latency correction: the filter, where its kernels come from and go back to, the
    /// request last sent, kernels waiting to be given back, and whether the input is mono.
    correction: CorrectionFilter,
    corrections: Arc<Handover<Correction>>,
    correction_requests: TripleWriter<CorrectionRequest>,
    last_correction: Option<CorrectionRequest>,
    correction_pending: [Option<Box<Correction>>; 4],
    mono: bool,
    /// The sidechain, delayed to line up with what the bands' detectors hear (Linear Phase runs
    /// the bands after its kernel), and the delayed chunk.
    sc_delay: DryDelay,
    sc_scratch: [Vec<f32>; 2],
}

impl EqEngine {
    /// Allocates and starts the kernel-design thread; call off the audio thread.
    pub fn new(sample_rate: f32, settings: &[BandSettings; BANDS], out_db: f32) -> Self {
        let sr = f64::from(sample_rate);
        let kernels = Arc::new(Handover::new());
        let stop = Arc::new(AtomicBool::new(false));
        let initial = DesignRequest { mode: PhaseMode::LinearPhase, bands: *settings, sample_rate: sr, len: kernel_len(2, sr), block: LP_BLOCK_48K };
        let (requests, reader) = triple_buffer(initial);
        let corrections = Arc::new(Handover::new());
        let (correction_requests, correction_reader) = triple_buffer(CorrectionRequest { bands: *settings, sample_rate: sr, mono: false });
        let designer = {
            let (kernels, corrections, stop) = (kernels.clone(), corrections.clone(), stop.clone());
            std::thread::Builder::new()
                .name("mite-eq-kernels".into())
                .spawn(move || run_designer(reader, kernels, correction_reader, corrections, stop))
                .ok()
        };
        let mut engine = Self {
            bands: (0..BANDS).map(|_| BandProcessor::new(sr, FREQ.smoothing_ms)).collect(),
            out_gain: LinearSmoother::new(1.0),
            meters: [PeakMeter::new(sample_rate), PeakMeter::new(sample_rate)],
            sample_rate: sr,
            wanted: Config { mode: PhaseMode::ZeroLatency, lp_res: 2, spectral: false },
            active: Config { mode: PhaseMode::ZeroLatency, lp_res: 2, spectral: false },
            switch: Switch::Steady,
            fader: LinearSmoother::new(1.0),
            conv: Convolver::new(LP_BLOCK_48K, 1, 1),
            np_conv: Convolver::new(NP_BLOCK_48K, 1, 1),
            kernels,
            requests,
            last_request: None,
            pending: Default::default(),
            stop,
            designer,
            stft: Stft::new(16),
            spectral: SpectralBands::new(BANDS, 16, sr),
            mono_scratch: vec![0.0; MONO_CHUNK],
            filled: 0,
            waited: 0,
            output: Output::default(),
            dry: DryDelay::new(0, CHUNK),
            mix: LinearSmoother::new(1.0),
            delta: LinearSmoother::new(0.0),
            bypass: LinearSmoother::new(0.0),
            pan: [LinearSmoother::new(1.0), LinearSmoother::new(1.0)],
            auto_gain: LinearSmoother::new(1.0),
            loudness_key: None,
            loudness_wait: 0,
            solo_filter: BandProcessor::new(sr, FREQ.smoothing_ms),
            solo_placement: Placement::Stereo,
            solo: LinearSmoother::new(0.0),
            solo_scratch: [vec![0.0; CHUNK], vec![0.0; CHUNK]],
            correction: CorrectionFilter::new(1, 1),
            corrections,
            correction_requests,
            last_correction: None,
            correction_pending: Default::default(),
            mono: false,
            sc_delay: DryDelay::new(0, CHUNK),
            sc_scratch: [vec![0.0; CHUNK], vec![0.0; CHUNK]],
        };
        engine.prepare(sample_rate, settings, out_db);
        engine
    }

    /// Asks for a processing mode and linear-phase resolution. A change takes effect through a
    /// short dip (the latency changes with it). Allocation-free.
    pub fn set_mode(&mut self, mode: PhaseMode, lp_res: usize) {
        self.wanted.mode = mode;
        self.wanted.lp_res = lp_res.min(LP_LENGTHS_48K.len() - 1);
    }

    /// Sets the output stage (mix, delta, bypass, auto gain, balance, solo). Changes
    /// fade in over the next blocks. Allocation-free.
    pub fn set_output(&mut self, output: Output) {
        self.output = output;
    }

    /// Off the audio thread: sets the sample rate, sizes the convolver, and jumps to the given
    /// settings and the mode asked for (designing its kernel here, so it starts at once).
    pub fn prepare(&mut self, sample_rate: f32, settings: &[BandSettings; BANDS], out_db: f32) {
        let sr = f64::from(sample_rate);
        self.sample_rate = sr;
        for (band, s) in self.bands.iter_mut().zip(settings) {
            band.prepare(sr, FREQ.smoothing_ms, s);
        }
        self.out_gain.set_ramp(sample_rate, OUT_GAIN.smoothing_ms);
        self.fader.set_ramp(sample_rate, SWITCH_FADE_MS);
        self.meters = [PeakMeter::new(sample_rate), PeakMeter::new(sample_rate)];
        let ratio = rate_ratio(sr);
        let fade = (KERNEL_FADE_MS * sr / 1000.0) as usize;
        self.conv = Convolver::new(LP_BLOCK_48K * ratio, kernel_len(LP_LENGTHS_48K.len() - 1, sr), fade);
        self.np_conv = Convolver::new(NP_BLOCK_48K * ratio, 2 * natural_phase::DELAY_48K * ratio, fade);
        self.stft = Stft::new(spectral_latency(sr));
        self.spectral = SpectralBands::new(BANDS, self.stft.size(), sr);
        let max_latency = linear_phase_latency(LP_LENGTHS_48K.len() - 1, sr).max(natural_phase_latency(sr)) + spectral_latency(sr);
        self.dry = DryDelay::new(max_latency, CHUNK);
        self.sc_delay = DryDelay::new(linear_phase_latency(LP_LENGTHS_48K.len() - 1, sr), CHUNK);
        // The correction is designed off the audio thread here, so Zero Latency starts corrected.
        self.correction = CorrectionFilter::new(max_taps(ratio), fade);
        self.last_correction = Some(CorrectionRequest { bands: *settings, sample_rate: sr, mono: self.mono });
        let _ = self.correction.offer(Box::new(design_correction(&CorrectionRequest { bands: *settings, sample_rate: sr, mono: self.mono })));
        let [pan_l, pan_r] = &mut self.pan;
        for s in [&mut self.mix, pan_l, pan_r] {
            s.set_ramp(sample_rate, crate::params::MIX.smoothing_ms);
        }
        for s in [&mut self.delta, &mut self.bypass, &mut self.solo] {
            s.set_ramp(sample_rate, OUTPUT_FADE_MS);
        }
        self.auto_gain.set_ramp(sample_rate, AUTO_GAIN_GLIDE_MS);
        self.solo_filter.prepare(sr, FREQ.smoothing_ms, &BandSettings::default());
        self.wanted.spectral = needs_spectrum(settings);
        self.active = self.wanted;
        self.switch = Switch::Steady;
        self.last_request = None;
        if self.active.mode.convolves() {
            let req = self.request(settings);
            let kernel = Box::new(design_kernel(&req));
            self.convolver(self.active.mode).replace(kernel);
        }
        self.reset(settings, out_db);
    }

    /// Clears all state. Allocation-free.
    pub fn reset(&mut self, settings: &[BandSettings; BANDS], out_db: f32) {
        for (band, s) in self.bands.iter_mut().zip(settings) {
            band.reset(s);
        }
        self.out_gain.reset(db_to_gain(out_db));
        self.fader.reset(if self.switch == Switch::Steady { 1.0 } else { 0.0 });
        self.conv.reset();
        self.np_conv.reset();
        self.stft.reset();
        self.spectral.reset();
        for m in &mut self.meters {
            m.reset();
        }
        self.dry.reset();
        self.sc_delay.reset();
        self.correction.reset();
        let o = self.output;
        let (pl, pr) = balance(o.pan);
        self.mix.reset(o.mix.clamp(0.0, 1.0));
        self.delta.reset(if o.delta { 1.0 } else { 0.0 });
        self.bypass.reset(if o.bypass { 1.0 } else { 0.0 });
        self.pan[0].reset(pl);
        self.pan[1].reset(pr);
        self.solo.reset(0.0);
        self.solo_filter.reset(&BandSettings::default());
        self.auto_gain.reset(if o.auto_gain { self.auto_gain.target() } else { 1.0 });
    }

    /// The convolver a convolving mode runs.
    fn convolver(&mut self, mode: PhaseMode) -> &mut Convolver {
        if mode == PhaseMode::NaturalPhase { &mut self.np_conv } else { &mut self.conv }
    }

    /// The block size and kernel delay a mode's kernels have, at `lp_res` for Linear Phase.
    fn kernel_shape(&self, mode: PhaseMode, lp_res: usize) -> (usize, usize) {
        let ratio = rate_ratio(self.sample_rate);
        match mode {
            PhaseMode::NaturalPhase => (NP_BLOCK_48K * ratio, natural_phase::DELAY_48K * ratio),
            _ => (LP_BLOCK_48K * ratio, kernel_len(lp_res, self.sample_rate) / 2),
        }
    }

    fn request(&self, settings: &[BandSettings; BANDS]) -> DesignRequest {
        let (block, delay) = self.kernel_shape(self.wanted.mode, self.wanted.lp_res);
        let len = if self.wanted.mode == PhaseMode::NaturalPhase { delay } else { 2 * delay };
        DesignRequest { mode: self.wanted.mode, bands: *settings, sample_rate: self.sample_rate, len, block }
    }

    /// The latency the engine runs with now, samples (report it to the host).
    pub fn latency(&self) -> usize {
        let phase = match self.active.mode {
            PhaseMode::LinearPhase => self.conv.latency(),
            PhaseMode::NaturalPhase => self.np_conv.latency(),
            PhaseMode::ZeroLatency => 0,
        };
        phase + if self.active.spectral { self.stft.latency() } else { 0 }
    }

    /// Once per block, Zero Latency: asks for the correction of the bands' static settings when
    /// they change, takes a finished one in (crossfading), and gives retired ones back.
    fn update_correction(&mut self, settings: &[BandSettings; BANDS]) {
        for i in 0..self.correction_pending.len() {
            if let Some(k) = self.correction_pending[i].take() {
                if let Err(k) = self.corrections.retire(k) {
                    self.correction_pending[i] = Some(k);
                }
            }
        }
        if let Some(old) = self.correction.retired() {
            self.retire_correction(old);
        }
        if self.wanted.mode != PhaseMode::ZeroLatency {
            return;
        }
        let req = CorrectionRequest { bands: *settings, sample_rate: self.sample_rate, mono: self.mono };
        if self.last_correction != Some(req) {
            self.correction_requests.write(req);
            self.last_correction = Some(req);
        }
        if !self.correction.is_fading() {
            if let Some(k) = self.corrections.take() {
                if let Some(back) = self.correction.offer(k) {
                    self.retire_correction(back);
                }
            }
        }
    }

    fn retire_correction(&mut self, kernel: Box<Correction>) {
        if let Err(kernel) = self.corrections.retire(kernel) {
            match self.correction_pending.iter_mut().find(|slot| slot.is_none()) {
                Some(slot) => *slot = Some(kernel),
                // Every slot full: leaking is the only option that keeps the audio thread clean.
                None => std::mem::forget(kernel),
            }
        }
    }

    /// Gives a kernel back to the designer to free; never frees on this thread.
    fn retire(&mut self, kernel: Box<KernelSet>) {
        if let Err(kernel) = self.kernels.retire(kernel) {
            match self.pending.iter_mut().find(|slot| slot.is_none()) {
                Some(slot) => *slot = Some(kernel),
                // Every slot full: leaking is the only option that keeps the audio thread clean.
                None => std::mem::forget(kernel),
            }
        }
    }

    /// Once per block: publish design requests, move kernels in and out, and step a mode switch.
    fn update_mode(&mut self, settings: &[BandSettings; BANDS]) {
        for i in 0..self.pending.len() {
            if let Some(k) = self.pending[i].take() {
                if let Err(k) = self.kernels.retire(k) {
                    self.pending[i] = Some(k);
                }
            }
        }
        for mode in [PhaseMode::LinearPhase, PhaseMode::NaturalPhase] {
            if let Some(old) = self.convolver(mode).retired() {
                self.retire(old);
            }
        }
        self.wanted.spectral = needs_spectrum(settings);
        self.update_correction(settings);
        if self.wanted.mode.convolves() {
            let req = self.request(settings);
            if self.last_request != Some(req) {
                self.requests.write(req);
                self.last_request = Some(req);
            }
        }
        let mode = self.active.mode;
        // A kernel fits the running mode if it has its block size and delay (a kernel left from
        // another mode, resolution or sample rate does not).
        let (block, delay) = self.kernel_shape(mode, self.active.lp_res);
        let fits = |k: &KernelSet| k.block() == block && k.delay() == delay;
        match self.switch {
            Switch::Steady => {
                if self.wanted.differs(&self.active) {
                    self.fader.set_target(0.0);
                    self.switch = Switch::FadingOut;
                } else if mode.convolves() && !self.convolver(mode).is_fading() {
                    if let Some(k) = self.kernels.take() {
                        if fits(&k) {
                            if let Err(k) = self.convolver(mode).offer(k) {
                                self.retire(k);
                            }
                        } else {
                            self.retire(k);
                        }
                    }
                } else if !mode.convolves() {
                    if let Some(k) = self.kernels.take() {
                        self.retire(k);
                    }
                }
            }
            Switch::FadingOut => {
                if !self.fader.is_smoothing() {
                    self.active = self.wanted;
                    for (band, s) in self.bands.iter_mut().zip(settings) {
                        band.reset(s);
                    }
                    self.conv.reset();
                    self.np_conv.reset();
                    self.correction.reset();
                    self.stft.reset();
                    self.spectral.reset();
                    self.filled = 0;
                    self.waited = 0;
                    // Always ask for the new mode's kernel: one designed earlier for the same
                    // bands may have been thrown away while it was not needed.
                    self.last_request = None;
                    self.switch = if self.active.mode.convolves() {
                        Switch::Waiting
                    } else if self.active.spectral {
                        Switch::Filling
                    } else {
                        Switch::Steady
                    };
                    if self.switch == Switch::Steady {
                        self.fader.set_target(1.0);
                    }
                }
            }
            Switch::Waiting => {
                // Never stay muted for good: if no fitting kernel came, ask again.
                if self.waited > (0.25 * self.sample_rate) as usize {
                    self.waited = 0;
                    self.last_request = None;
                }
                if let Some(k) = self.kernels.take() {
                    if fits(&k) {
                        if let Some(old) = self.convolver(mode).replace(k) {
                            self.retire(old);
                        }
                        if let Some(old) = self.convolver(mode).retired() {
                            self.retire(old);
                        }
                        self.switch = Switch::Filling;
                    } else {
                        self.retire(k);
                    }
                }
            }
            Switch::Filling => {
                if self.filled >= self.latency() {
                    self.switch = Switch::Steady;
                    self.fader.set_target(1.0);
                }
            }
        }
    }

    /// Processes in place and returns output peak levels (left, right) in dBFS. `pre`/`post`
    /// receive the mono sum before and after the EQ for the analyzer, at the same moment: the
    /// pre tap hears the input delayed by the latency, as the output is.
    pub fn process(
        &mut self,
        channels: &mut [&mut [f32]],
        settings: &[BandSettings; BANDS],
        out_db: f32,
        pre: Option<&SpectrumTap>,
        post: Option<&SpectrumTap>,
    ) -> (f32, f32) {
        self.process_sidechain(channels, None, settings, out_db, pre, post)
    }

    /// [`Self::process`] with an external sidechain (one or two channels, as long as the main
    /// ones), which bands detecting from the sidechain listen to. Without one they hear silence.
    pub fn process_sidechain(
        &mut self,
        channels: &mut [&mut [f32]],
        sidechain: Option<&[&[f32]]>,
        settings: &[BandSettings; BANDS],
        out_db: f32,
        pre: Option<&SpectrumTap>,
        post: Option<&SpectrumTap>,
    ) -> (f32, f32) {
        self.mono = channels.len() == 1;
        self.update_mode(settings);
        self.update_output(settings, channels.len() == 1, out_db);

        let frames = channels.first().map_or(0, |c| c.len());
        self.loudness_wait = self.loudness_wait.saturating_sub(frames);
        let mut start = 0;
        while start < frames {
            let end = (start + CHUNK).min(frames);
            let sc = sidechain.map(|sc| {
                let at = |c: usize| sc.get(c.min(sc.len().saturating_sub(1))).map_or(&[][..], |ch| &ch[start.min(ch.len())..end.min(ch.len())]);
                [at(0), at(1)]
            });
            match channels {
                [l, r, ..] => self.process_chunk(&mut [&mut l[start..end], &mut r[start..end]], settings, pre, sc),
                [m] => self.process_chunk(&mut [&mut m[start..end]], settings, pre, sc),
                [] => {}
            }
            start = end;
        }

        if let Some(tap) = post {
            push_mono(tap, channels);
        }
        let left = channels.first().map_or(mite_dsp::MINUS_INFINITY_DB, |c| self.meters[0].process(c));
        let right = channels.get(1).map_or(left, |c| self.meters[1].process(c));
        (left, right)
    }

    /// Once per block: the output stage's targets.
    fn update_output(&mut self, settings: &[BandSettings; BANDS], mono: bool, out_db: f32) {
        let o = self.output;
        self.out_gain.set_target(db_to_gain(out_db));
        self.mix.set_target(o.mix.clamp(0.0, 1.0));
        self.delta.set_target(if o.delta { 1.0 } else { 0.0 });
        self.bypass.set_target(if o.bypass { 1.0 } else { 0.0 });
        let (pl, pr) = if mono { (1.0, 1.0) } else { balance(o.pan) };
        self.pan[0].set_target(pl);
        self.pan[1].set_target(pr);

        // Auto gain: worked out from the curve, at most every few milliseconds while it moves.
        if o.auto_gain {
            let key = Some((*settings, mono));
            if self.loudness_key != key && self.loudness_wait == 0 {
                let db = (-loudness_change_db(settings, mono)).clamp(-AUTO_GAIN_LIMIT_DB, AUTO_GAIN_LIMIT_DB);
                self.auto_gain.set_target(db_to_gain(db as f32));
                self.loudness_key = key;
                self.loudness_wait = AUTO_GAIN_INTERVAL_48K * rate_ratio(self.sample_rate);
            }
        } else {
            self.auto_gain.set_target(1.0);
            self.loudness_key = None;
        }

        // Solo: the soloed band's region, faded in; a band without one (tilt) cannot solo.
        let filter = o.solo.filter(|&b| b < BANDS && settings[b].enabled).and_then(|b| solo_filter(&settings[b]).map(|f| (f, settings[b].placement)));
        match filter {
            Some((f, placement)) => {
                if self.solo.current() == 0.0 && !self.solo.is_smoothing() {
                    // Starting from silence: jump straight to the band.
                    self.solo_filter.reset(&f);
                } else {
                    self.solo_filter.set_target(&f);
                }
                self.solo_placement = placement;
                self.solo.set_target(1.0);
            }
            None => self.solo.set_target(0.0),
        }
    }

    /// One chunk of at most [`CHUNK`] samples.
    fn process_chunk(&mut self, channels: &mut [&mut [f32]], settings: &[BandSettings; BANDS], pre: Option<&SpectrumTap>, sidechain: Option<[&[f32]; 2]>) {
        let frames = channels.first().map_or(0, |c| c.len());
        // The sidechain chunk as the bands' detectors need it: delayed by the kernel in Linear
        // Phase (the bands run after it), silence where none is connected or it is short.
        let wants_sidechain = settings.iter().any(|s| s.enabled && s.is_dynamic() && !s.is_spectral() && s.dynamics.source == DetectSource::Sidechain);
        if wants_sidechain {
            let delay = if self.active.mode == PhaseMode::LinearPhase { self.conv.latency() } else { 0 };
            self.sc_delay.set_delay(delay);
            let [l, r] = sidechain.unwrap_or([&[], &[]]);
            let fill = |src: &[f32], dst: &mut [f32]| {
                let n = src.len().min(dst.len());
                dst[..n].copy_from_slice(&src[..n]);
                dst[n..].fill(0.0);
            };
            let [a, b] = &mut self.sc_scratch;
            fill(l, &mut a[..frames]);
            fill(r, &mut b[..frames]);
            self.sc_delay.write(&a[..frames], Some(&b[..frames]));
            for i in 0..frames {
                a[i] = self.sc_delay.read(0, i);
                b[i] = self.sc_delay.read(1, i);
            }
            self.sc_delay.advance(frames);
        }
        self.dry.set_delay(self.latency());
        match &*channels {
            [l, r, ..] => self.dry.write(l, Some(r)),
            [m] => self.dry.write(m, None),
            [] => {}
        }
        if let Some(tap) = pre {
            // The input as delayed as the output will be (mono input is stored on both sides).
            let dry = &self.dry;
            tap.push((0..frames).map(|i| 0.5 * (dry.read(0, i) + dry.read(1, i))));
        }
        self.filled = self.filled.saturating_add(frames);
        if self.switch == Switch::Waiting {
            self.waited = self.waited.saturating_add(frames);
        }
        if self.active.mode == PhaseMode::LinearPhase {
            self.convolve(PhaseMode::LinearPhase, channels);
            // The kernel holds every band's static curve; dynamic bands add their movement here,
            // through a filter with no static gain of its own (spectral ones move in the STFT).
            let detect = [&self.sc_scratch[0][..frames], &self.sc_scratch[1][..frames]];
            for (band, s) in self.bands.iter_mut().zip(settings) {
                if (s.is_dynamic() && !s.is_spectral()) || band.dynamic_gain_db() != 0.0 {
                    band.set_target(&BandSettings { gain_db: 0.0, ..*s });
                    route(band, s, settings, channels, &detect);
                }
            }
        } else {
            let detect = [&self.sc_scratch[0][..frames], &self.sc_scratch[1][..frames]];
            for (band, s) in self.bands.iter_mut().zip(settings) {
                band.set_target(s);
                route(band, s, settings, channels, &detect);
            }
            // Natural Phase: the correction after the IIR sections (designed for their static
            // settings; dynamic movement passes through the IIR sections as in Zero Latency).
            // Zero Latency: the minimum-phase magnitude correction (no delay).
            match self.active.mode {
                PhaseMode::NaturalPhase => self.convolve(PhaseMode::NaturalPhase, channels),
                PhaseMode::ZeroLatency => self.correction.process(channels),
                PhaseMode::LinearPhase => {}
            }
        }
        if self.active.spectral {
            self.run_spectral(channels, settings);
        }
        self.finish(channels);
        self.dry.advance(frames);
    }

    /// Whether the output stage leaves the wet signal as it is (only output gain and the switch
    /// dip apply).
    fn output_is_plain(&self) -> bool {
        let still = |s: &LinearSmoother, v: f32| !s.is_smoothing() && s.current() == v;
        still(&self.mix, 1.0)
            && still(&self.delta, 0.0)
            && still(&self.bypass, 0.0)
            && still(&self.solo, 0.0)
            && still(&self.auto_gain, 1.0)
            && still(&self.pan[0], 1.0)
            && still(&self.pan[1], 1.0)
    }

    /// The output stage over one chunk.
    fn finish(&mut self, channels: &mut [&mut [f32]]) {
        let frames = channels.first().map_or(0, |c| c.len());
        if self.output_is_plain() {
            if self.out_gain.is_smoothing() || self.fader.is_smoothing() || self.fader.current() != 1.0 {
                for i in 0..frames {
                    let g = self.out_gain.next_value() * self.fader.next_value();
                    for ch in channels.iter_mut() {
                        ch[i] *= g;
                    }
                }
            } else if self.out_gain.current() != 1.0 {
                let g = self.out_gain.current();
                for ch in channels.iter_mut() {
                    for s in ch.iter_mut() {
                        *s *= g;
                    }
                }
            }
            return;
        }
        // Auto gain, then mix and delta against the dry signal.
        for i in 0..frames {
            let (ag, mix, delta) = (self.auto_gain.next_value(), self.mix.next_value(), self.delta.next_value());
            for (c, ch) in channels.iter_mut().enumerate() {
                let (wet, dry) = (ch[i] * ag, self.dry.read(c, i));
                let x = if mix <= 0.0 { dry } else { dry + mix * (wet - dry) };
                ch[i] = x - delta * dry;
            }
        }
        // Solo: crossfade into the soloed band's region (of what the band hears).
        if self.solo.is_smoothing() || self.solo.current() > 0.0 {
            let [sl, sr] = &mut self.solo_scratch;
            let (sl, sr) = (&mut sl[..frames], &mut sr[..frames]);
            match &*channels {
                [l, r, ..] => {
                    for i in 0..frames {
                        (sl[i], sr[i]) = isolate(self.solo_placement, l[i], r[i]);
                    }
                    self.solo_filter.process(&mut [sl, sr]);
                }
                [m] => {
                    sl.copy_from_slice(m);
                    self.solo_filter.process(&mut [sl]);
                }
                [] => {}
            }
            for i in 0..frames {
                let t = self.solo.next_value();
                for (c, ch) in channels.iter_mut().enumerate() {
                    let soloed = self.solo_scratch[c][i];
                    ch[i] += t * (soloed - ch[i]);
                }
            }
        }
        // Output gain and balance, then bypass to the dry signal, then the switch dip.
        for i in 0..frames {
            let g = self.out_gain.next_value();
            let pan = [self.pan[0].next_value(), self.pan[1].next_value()];
            let (bypass, fader) = (self.bypass.next_value(), self.fader.next_value());
            for (c, ch) in channels.iter_mut().enumerate() {
                let y = ch[i] * g * pan[c];
                let dry = self.dry.read(c, i);
                let y = if bypass >= 1.0 { dry } else { y + bypass * (dry - y) };
                ch[i] = y * fader;
            }
        }
    }

    /// Runs a convolving mode's convolver over the channels (mono through a scratch right channel).
    fn convolve(&mut self, mode: PhaseMode, channels: &mut [&mut [f32]]) {
        let conv = if mode == PhaseMode::NaturalPhase { &mut self.np_conv } else { &mut self.conv };
        match channels {
            [l, r, ..] => conv.process(l, r),
            [m] => {
                for chunk in m.chunks_mut(MONO_CHUNK) {
                    let scratch = &mut self.mono_scratch[..chunk.len()];
                    scratch.copy_from_slice(chunk);
                    conv.process(chunk, scratch);
                }
            }
            [] => {}
        }
    }

    /// Runs the spectral stage over the channels (mono through a scratch right channel).
    fn run_spectral(&mut self, channels: &mut [&mut [f32]], settings: &[BandSettings; BANDS]) {
        match channels {
            [l, r, ..] => self.spectral.process(&mut self.stft, l, r, settings),
            [m] => {
                for chunk in m.chunks_mut(MONO_CHUNK) {
                    let scratch = &mut self.mono_scratch[..chunk.len()];
                    scratch.copy_from_slice(chunk);
                    self.spectral.process(&mut self.stft, chunk, scratch, settings);
                }
            }
            [] => {}
        }
    }

    /// The spectral stage's total gain at `freq` right now, dB (0 while it is off), for the plot.
    pub fn spectral_gain_db(&self, freq: f64) -> f32 {
        if !self.active.spectral {
            return 0.0;
        }
        // Catmull-Rom between bins, so a curve drawn through these values has no corners where
        // the bins are sparse (low frequencies), held between the two bins so it never shows a
        // boost or cut beyond them.
        let (gains, _) = self.spectral.total_db();
        let last = gains.len() - 1;
        let pos = (freq * self.stft.size() as f64 / self.sample_rate).clamp(0.0, last as f64);
        let k = (pos as usize).min(last - 1);
        let t = (pos - k as f64) as f32;
        let (p0, p1, p2, p3) = (gains[k.saturating_sub(1)], gains[k], gains[k + 1], gains[(k + 2).min(last)]);
        let (t2, t3) = (t * t, t * t * t);
        let v = 0.5 * ((2.0 * p1) + (p2 - p0) * t + (2.0 * p0 - 5.0 * p1 + 4.0 * p2 - p3) * t2 + (3.0 * p1 - p0 - 3.0 * p2 + p3) * t3);
        v.clamp(p1.min(p2), p1.max(p2))
    }

    /// Band `band`'s detected level and the threshold it is compared with (both on the
    /// analyzer's scale, dB, −150 where the stage is off) and the gain it applies (dB) at
    /// spectrum point `point` ([`mite_dsp::analyzer::spectrum_point_freq`]), for the display.
    /// The levels are the detector's own points, read as the analyzer reads; the gain is what
    /// the STFT bins actually apply there (a monotone cubic through them, so where bins are
    /// sparse the display shows the cut as wide as it really is).
    pub fn spectral_band_db(&self, band: usize, point: usize) -> (f32, f32, f32) {
        if !self.active.spectral || band >= BANDS || point >= SPECTRUM_POINTS {
            return (-150.0, -150.0, 0.0);
        }
        let (levels, thresholds, _) = self.spectral.band_points(band);
        let last = self.spectral.freqs().len() - 1;
        let pos = (spectrum_point_freq(point) * self.stft.size() as f64 / self.sample_rate).min(last as f64);
        let gain = monotone_at(|k| self.spectral.band_bin_gain(band, k), 0, last, pos);
        (levels[point].max(-150.0) as f32, thresholds[point].max(-150.0) as f32, gain as f32)
    }

    /// What each band is producing right now (smoothed values), for the display.
    pub fn band_state(&self, band: usize) -> BandSettings {
        self.bands[band].current()
    }

    /// Each band's dynamic gain right now, dB, for the display (for a spectral band, its
    /// strongest per-bin gain).
    pub fn dynamic_gains(&self) -> impl Iterator<Item = f32> + '_ {
        self.bands.iter().zip(self.spectral.band_peaks()).map(|(b, peak)| (b.dynamic_gain_db() + peak) as f32)
    }

    /// Each band's detected level right now, dBFS, for the display.
    pub fn detector_levels(&self) -> impl Iterator<Item = f32> + '_ {
        self.bands.iter().map(|b| b.detector_level_db() as f32)
    }
}

/// Runs `band` (settings `s`) over `channels` with its detector on the right source: its own
/// region, another band's region (`settings[n]`), or the sidechain chunk `sidechain`.
fn route(band: &mut BandProcessor, s: &BandSettings, settings: &[BandSettings; BANDS], channels: &mut [&mut [f32]], sidechain: &[&[f32]; 2]) {
    let source = if s.is_dynamic() && !s.is_spectral() { s.dynamics.source } else { DetectSource::Own };
    match source {
        DetectSource::Band(n) if n < BANDS => band.set_detect_region(Some(settings[n])),
        _ => band.set_detect_region(None),
    }
    if source == DetectSource::Sidechain {
        band.process_detecting(channels, Some(&sidechain[..channels.len().clamp(1, 2)]));
    } else {
        band.process(channels);
    }
}

/// A monotone cubic (Fritsch–Butland tangents) through `v(lo..=hi)` at fractional index `pos`:
/// smooth across the samples, and between each pair it never leaves their span.
fn monotone_at(v: impl Fn(usize) -> f64, lo: usize, hi: usize, pos: f64) -> f64 {
    if hi == lo {
        return v(lo);
    }
    let k = (pos.floor() as usize).clamp(lo, hi - 1);
    let t = pos - k as f64;
    let d = |i: usize| v(i + 1) - v(i);
    let tangent = |i: usize| {
        if i == lo {
            d(lo)
        } else if i == hi {
            d(hi - 1)
        } else {
            let (a, b) = (d(i - 1), d(i));
            if a * b > 0.0 { 2.0 * a * b / (a + b) } else { 0.0 }
        }
    };
    let (p0, p1, m0, m1) = (v(k), v(k + 1), tangent(k), tangent(k + 1));
    let (t2, t3) = (t * t, t * t * t);
    (2.0 * t3 - 3.0 * t2 + 1.0) * p0 + (t3 - 2.0 * t2 + t) * m0 + (-2.0 * t3 + 3.0 * t2) * p1 + (t3 - t2) * m1
}

impl Drop for EqEngine {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.designer.take() {
            let _ = t.join();
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

#[cfg(test)]
mod tests {
    use super::*;
    use mite_dsp::eq::BandType;
    use std::f64::consts::PI;

    #[test]
    fn the_scope_runs_smoothly_through_sparse_bins_without_overshoot() {
        let v = [-60.0, -60.0, -20.0, -30.0, -32.0, -50.0];
        let at = |p: f64| monotone_at(|k| v[k], 0, v.len() - 1, p);
        let mut slope = (at(0.01) - at(0.0)) / 0.01;
        for i in 1..500 {
            let p = i as f64 * 0.01;
            let k = (p.floor() as usize).min(v.len() - 2);
            let y = at(p);
            assert!(y >= v[k].min(v[k + 1]) - 1e-9 && y <= v[k].max(v[k + 1]) + 1e-9, "overshoot at {p}: {y}");
            // No corner: the slope changes gradually, also across the bins themselves.
            let next = (at(p + 0.01) - y) / 0.01;
            assert!((next - slope).abs() < 6.0, "corner at {p}: {slope} → {next}");
            slope = next;
        }
    }

    const SR: f32 = 48_000.0;

    fn sine(freq: f64, len: usize) -> Vec<f32> {
        (0..len).map(|n| (2.0 * PI * freq * n as f64 / f64::from(SR)).sin() as f32 * 0.5).collect()
    }

    fn flat() -> [BandSettings; BANDS] {
        [BandSettings::default(); BANDS]
    }

    #[test]
    fn empty_eq_passes_audio_bit_exactly() {
        // The design's empty state promises "passing audio unchanged".
        let mut eq = EqEngine::new(SR, &flat(), 0.0);
        let input = sine(997.0, 4096);
        let (mut l, mut r) = (input.clone(), input.clone());
        eq.process(&mut [&mut l, &mut r], &flat(), 0.0, None, None);
        assert_eq!(l, input);
        assert_eq!(r, input);
    }

    #[test]
    fn bands_stack_in_series() {
        let mut settings = flat();
        for (i, s) in settings.iter_mut().take(2).enumerate() {
            *s = BandSettings { enabled: true, kind: BandType::Bell, freq: 1000.0, gain_db: 3.0, q: 1.0, slope: 3, ..Default::default() };
            let _ = i;
        }
        let mut eq = EqEngine::new(SR, &settings, 0.0);
        let input = sine(1000.0, 48_000);
        let mut buf = input.clone();
        eq.process(&mut [&mut buf], &settings, 0.0, None, None);
        let rms = |x: &[f32]| (x.iter().map(|v| f64::from(*v).powi(2)).sum::<f64>() / x.len() as f64).sqrt();
        let gain = 20.0 * (rms(&buf[24_000..]) / rms(&input[24_000..])).log10();
        assert!((gain - 6.0).abs() < 0.05, "{gain}");
    }

    fn noise(n: usize, seed: u32) -> Vec<f32> {
        let mut s = seed;
        (0..n)
            .map(|_| {
                s ^= s << 13;
                s ^= s >> 17;
                s ^= s << 5;
                (s as f32 / u32::MAX as f32 - 0.5) * 0.5
            })
            .collect()
    }

    fn bell(gain_db: f64) -> [BandSettings; BANDS] {
        let mut s = flat();
        s[0] = BandSettings { enabled: true, kind: BandType::Bell, freq: 1000.0, gain_db, q: 1.0, ..Default::default() };
        s
    }

    /// An engine already running Linear Phase at `lp_res`.
    fn linear(settings: &[BandSettings; BANDS], lp_res: usize) -> EqEngine {
        let mut eq = EqEngine::new(SR, settings, 0.0);
        eq.set_mode(PhaseMode::LinearPhase, lp_res);
        eq.prepare(SR, settings, 0.0);
        eq
    }

    /// Runs `l`/`r` through in 128-sample blocks, giving the designer thread time between blocks.
    fn run(eq: &mut EqEngine, settings: &[BandSettings; BANDS], l: &mut [f32], r: &mut [f32], pause: bool) {
        for (a, b) in l.chunks_mut(128).zip(r.chunks_mut(128)) {
            eq.process(&mut [a, b], settings, 0.0, None, None);
            if pause {
                std::thread::sleep(Duration::from_micros(300));
            }
        }
    }

    #[test]
    fn linear_phase_with_no_bands_is_a_pure_delay() {
        let mut eq = linear(&flat(), 0);
        let latency = eq.latency();
        assert_eq!(latency, linear_phase_latency(0, f64::from(SR)));
        let input = noise(8192, 3);
        let (mut l, mut r) = (input.clone(), input.clone());
        run(&mut eq, &flat(), &mut l, &mut r, false);
        // Nulls with the delayed input far below −140 dBFS (f32 output resolution aside).
        let worst = (0..8192 - latency).map(|n| (l[n + latency] - input[n]).abs()).fold(0.0f32, f32::max);
        assert!(worst < 1e-7, "residual {worst}");
    }

    #[test]
    fn linear_phase_reports_its_true_delay_and_is_symmetric() {
        let settings = bell(6.0);
        let mut eq = linear(&settings, 2);
        let latency = eq.latency();
        let len = latency * 2 + 512;
        let (mut l, mut r) = (vec![0.0f32; len], vec![0.0f32; len]);
        l[0] = 1.0;
        r[0] = 1.0;
        run(&mut eq, &settings, &mut l, &mut r, false);
        // The impulse response is mirror-symmetric about the reported latency: linear phase, and
        // a group delay that is exactly the latency.
        let peak = l.iter().map(|v| v.abs()).fold(0.0f32, f32::max);
        for i in 1..1500 {
            assert!((l[latency - i] - l[latency + i]).abs() < 1e-6 * peak, "asymmetric at ±{i}");
        }
        assert!(l[latency].abs() > 0.5 * peak);
    }

    #[test]
    fn linear_phase_matches_zero_latency_in_magnitude() {
        let settings = bell(6.0);
        let rms = |x: &[f32]| (x.iter().map(|v| f64::from(*v).powi(2)).sum::<f64>() / x.len() as f64).sqrt();
        for freq in [300.0, 1000.0, 3000.0] {
            let input = sine(freq, 48_000);
            let mut zl = EqEngine::new(SR, &settings, 0.0);
            let (mut a, mut b) = (input.clone(), input.clone());
            run(&mut zl, &settings, &mut a, &mut b, false);
            let mut lp = linear(&settings, 2);
            let (mut c, mut d) = (input.clone(), input.clone());
            run(&mut lp, &settings, &mut c, &mut d, false);
            let diff = 20.0 * (rms(&c[24_000..]) / rms(&a[24_000..])).log10();
            assert!(diff.abs() < 0.05, "{freq} Hz: linear phase differs by {diff:.3} dB");
        }
    }

    #[test]
    fn switching_to_linear_phase_is_click_free_and_changes_the_latency() {
        let settings = bell(6.0);
        let mut eq = EqEngine::new(SR, &settings, 0.0);
        assert_eq!(eq.latency(), 0);
        let input = sine(200.0, 48_000);
        let (mut l, mut r) = (input.clone(), input.clone());
        eq.set_mode(PhaseMode::LinearPhase, 1);
        run(&mut eq, &settings, &mut l, &mut r, true);
        assert_eq!(eq.latency(), linear_phase_latency(1, f64::from(SR)));
        // A 200 Hz sine at 0.5 (+6 dB at most) moves at most ~0.03 per sample; a click would not.
        let jump = l.windows(2).map(|w| (w[1] - w[0]).abs()).fold(0.0f32, f32::max);
        assert!(jump < 0.04, "step {jump}");
        // It ends up playing again, not stuck in the dip.
        assert!(l[40_000..].iter().any(|v| v.abs() > 0.3));
        // And back to Zero Latency, just as smoothly.
        let (mut l, mut r) = (input.clone(), input.clone());
        eq.set_mode(PhaseMode::ZeroLatency, 1);
        run(&mut eq, &settings, &mut l, &mut r, true);
        assert_eq!(eq.latency(), 0);
        let jump = l.windows(2).map(|w| (w[1] - w[0]).abs()).fold(0.0f32, f32::max);
        assert!(jump < 0.04, "step back {jump}");
    }

    #[test]
    fn linear_phase_kernel_updates_crossfade() {
        let quiet = bell(-12.0);
        let mut eq = linear(&quiet, 0);
        let input = sine(1000.0, 48_000);
        let (mut l, mut r) = (input.clone(), input.clone());
        // Half a second at −12 dB, then the band drops to 0 dB while playing.
        let (la, lb) = l.split_at_mut(24_000);
        let (ra, rb) = r.split_at_mut(24_000);
        run(&mut eq, &quiet, la, ra, true);
        run(&mut eq, &bell(0.0), lb, rb, true);
        let jump = l.windows(2).map(|w| (w[1] - w[0]).abs()).fold(0.0f32, f32::max);
        assert!(jump < 0.07, "step {jump}");
        let peak = |x: &[f32]| x.iter().map(|v| v.abs()).fold(0.0f32, f32::max);
        assert!(peak(&l[20_000..23_000]) < 0.15 && (peak(&l[44_000..47_000]) - 0.5).abs() < 0.01);
    }

    /// An engine already running `mode` at `sr`.
    fn engine_in(mode: PhaseMode, sr: f32, settings: &[BandSettings; BANDS]) -> EqEngine {
        let mut eq = EqEngine::new(sr, settings, 0.0);
        eq.set_mode(mode, 2);
        eq.prepare(sr, settings, 0.0);
        eq
    }

    #[test]
    fn natural_phase_with_no_bands_is_a_pure_delay() {
        let mut eq = engine_in(PhaseMode::NaturalPhase, SR, &flat());
        let latency = eq.latency();
        assert_eq!(latency, natural_phase_latency(f64::from(SR)));
        assert!((latency as f64 / f64::from(SR) * 1000.0) < 5.0, "Natural Phase should stay under 5 ms");
        let input = noise(8192, 5);
        let (mut l, mut r) = (input.clone(), input.clone());
        run(&mut eq, &flat(), &mut l, &mut r, false);
        let worst = (0..8192 - latency).map(|n| (l[n + latency] - input[n]).abs()).fold(0.0f32, f32::max);
        assert!(worst < 1e-7, "residual {worst}");
    }

    #[test]
    fn both_iir_modes_reach_the_analog_curve_near_nyquist() {
        // At 44.1 kHz a 12 kHz bell's matched IIR drifts from the analog curve near Nyquist.
        // Natural Phase's correction (complex, 4 ms) and Zero Latency's (minimum phase, no delay)
        // both bring it back within 0.1 dB.
        let sr = 44_100.0_f32;
        let mut settings = flat();
        settings[0] = BandSettings { enabled: true, kind: BandType::Bell, freq: 12_000.0, gain_db: 9.0, q: 1.5, ..Default::default() };
        let rms = |x: &[f32]| (x.iter().map(|v| f64::from(*v).powi(2)).sum::<f64>() / x.len() as f64).sqrt();
        let sections = mite_dsp::eq::design(&settings[0], f64::from(sr));
        let mut worst_iir: f64 = 0.0;
        for freq in [8_000.0, 12_000.0, 16_000.0, 19_000.0] {
            let input: Vec<f32> = (0..44_100).map(|n| (0.25 * (2.0 * PI * freq * n as f64 / f64::from(sr)).sin()) as f32).collect();
            let analog = mite_dsp::eq::analog_response_db(&settings[0], freq);
            let gain = |mode| {
                let mut eq = engine_in(mode, sr, &settings);
                let (mut l, mut r) = (input.clone(), input.clone());
                run(&mut eq, &settings, &mut l, &mut r, false);
                20.0 * (rms(&l[22_050..]) / rms(&input[22_050..])).log10()
            };
            for mode in [PhaseMode::NaturalPhase, PhaseMode::ZeroLatency] {
                let off = gain(mode) - analog;
                assert!(off.abs() < 0.1, "{freq} Hz: {mode:?} {off:+.3} dB off the analog curve");
            }
            let iir = sections.response_db(2.0 * PI * freq / f64::from(sr));
            worst_iir = worst_iir.max((iir - analog).abs());
        }
        assert!(worst_iir > 0.2, "the test should cover a region where the matched IIR drifts ({worst_iir:.3} dB)");
    }

    #[test]
    fn switching_through_every_mode_is_click_free() {
        let settings = bell(6.0);
        let mut eq = EqEngine::new(SR, &settings, 0.0);
        for (mode, latency) in [
            (PhaseMode::NaturalPhase, natural_phase_latency(f64::from(SR))),
            (PhaseMode::LinearPhase, linear_phase_latency(2, f64::from(SR))),
            (PhaseMode::NaturalPhase, natural_phase_latency(f64::from(SR))),
            (PhaseMode::ZeroLatency, 0),
        ] {
            let input = sine(200.0, 24_000);
            let (mut l, mut r) = (input.clone(), input.clone());
            eq.set_mode(mode, 2);
            run(&mut eq, &settings, &mut l, &mut r, true);
            assert_eq!(eq.latency(), latency, "{mode:?}");
            let jump = l.windows(2).map(|w| (w[1] - w[0]).abs()).fold(0.0f32, f32::max);
            assert!(jump < 0.04, "{mode:?}: step {jump}");
            assert!(l[18_000..].iter().any(|v| v.abs() > 0.3), "{mode:?}: still muted");
        }
    }

    #[test]
    fn going_back_to_a_mode_with_unchanged_bands_plays_again() {
        // Linear Phase → Zero Latency → Linear Phase without touching any band: the second switch
        // must still get a kernel and come back out of the dip.
        let settings = bell(6.0);
        let mut eq = EqEngine::new(SR, &settings, 0.0);
        for mode in [PhaseMode::LinearPhase, PhaseMode::ZeroLatency, PhaseMode::LinearPhase, PhaseMode::NaturalPhase, PhaseMode::ZeroLatency, PhaseMode::NaturalPhase] {
            eq.set_mode(mode, 2);
            let input = sine(200.0, 24_000);
            let (mut l, mut r) = (input.clone(), input.clone());
            run(&mut eq, &settings, &mut l, &mut r, true);
            assert!(l[18_000..].iter().any(|v| v.abs() > 0.3), "{mode:?}: stuck muted");
        }
    }

    /// A relative spectral compressor at 2 kHz that cuts up to `range_db` (0: changes nothing).
    fn relative(range_db: f64) -> [BandSettings; BANDS] {
        let mut s = flat();
        s[0] = BandSettings { enabled: true, kind: BandType::Bell, freq: 2000.0, q: 0.3, ..Default::default() };
        s[0].dynamics = mite_dsp::eq::Dynamics { enabled: true, spectral: true, relative: true, ratio: 10.0, range_db: -range_db, ..Default::default() };
        s
    }

    #[test]
    fn the_spectral_stage_adds_its_latency_in_every_mode_and_nulls_when_neutral() {
        // A spectral band with no range changes nothing but the delay.
        let settings = relative(0.0);
        let stft = spectral_latency(f64::from(SR));
        for (mode, phase) in [
            (PhaseMode::ZeroLatency, 0),
            (PhaseMode::NaturalPhase, natural_phase_latency(f64::from(SR))),
            (PhaseMode::LinearPhase, linear_phase_latency(2, f64::from(SR))),
        ] {
            let mut eq = engine_in(mode, SR, &settings);
            let latency = eq.latency();
            assert_eq!(latency, phase + stft, "{mode:?}");
            let input = noise(16_384, 7);
            let (mut l, mut r) = (input.clone(), input.clone());
            run(&mut eq, &settings, &mut l, &mut r, false);
            let worst = (2 * stft..16_384 - latency).map(|n| (l[n + latency] - input[n]).abs()).fold(0.0f32, f32::max);
            assert!(worst < 1e-6, "{mode:?}: residual {worst}");
        }
    }

    #[test]
    fn relative_spectral_mode_cuts_a_ringing_tone_through_the_engine() {
        let settings = relative(12.0);
        let mut eq = EqEngine::new(SR, &settings, 0.0);
        let n = 96_000;
        let input: Vec<f32> = noise(n, 9).iter().zip(sine(2000.0, n)).map(|(a, b)| 0.1 * a + 0.6 * b).collect();
        let (mut l, mut r) = (input.clone(), input.clone());
        run(&mut eq, &settings, &mut l, &mut r, false);
        let tone = |x: &[f32]| {
            let x = &x[n / 2..];
            let (mut re, mut im) = (0.0, 0.0);
            for (i, &v) in x.iter().enumerate() {
                let p = 2.0 * PI * 2000.0 * i as f64 / f64::from(SR);
                re += f64::from(v) * p.cos();
                im += f64::from(v) * p.sin();
            }
            2.0 * (re * re + im * im).sqrt() / x.len() as f64
        };
        let cut = 20.0 * (tone(&l) / tone(&input)).log10();
        assert!(cut < -6.0, "only {cut:.2} dB");
        // The band shows its movement, and the plot curve dips at the tone.
        assert!(eq.dynamic_gains().next().unwrap() < -6.0);
        assert!(eq.spectral_gain_db(2000.0) < -6.0);
        assert!(eq.spectral_gain_db(300.0).abs() < 1.0);
    }

    #[test]
    fn adding_and_removing_a_spectral_band_is_click_free_and_signals_latency() {
        let plain = bell(6.0);
        let mut with = plain;
        with[1] = relative(6.0)[0];
        let mut eq = EqEngine::new(SR, &plain, 0.0);
        for (settings, latency) in [(&with, spectral_latency(f64::from(SR))), (&plain, 0), (&with, spectral_latency(f64::from(SR)))] {
            let input = sine(200.0, 24_000);
            let (mut l, mut r) = (input.clone(), input.clone());
            run(&mut eq, settings, &mut l, &mut r, false);
            assert_eq!(eq.latency(), latency);
            let jump = l.windows(2).map(|w| (w[1] - w[0]).abs()).fold(0.0f32, f32::max);
            assert!(jump < 0.04, "step {jump}");
            assert!(l[18_000..].iter().any(|v| v.abs() > 0.3), "still muted");
        }
        // Mono goes through the same stage.
        let mut m = sine(200.0, 24_000);
        eq.process(&mut [&mut m], &with, 0.0, None, None);
        assert!(m[18_000..].iter().any(|v| v.abs() > 0.3));
    }

    #[test]
    fn the_spectral_compressor_hears_exactly_what_the_analyzer_shows() {
        // A steady chord from the lows to the highs under a wide spectral band that never cuts:
        // its detected level at every point matches the analyzer's own reading of the signal
        // (same FFT, smoothing and tilt), the low notes included. Deep dips between beating
        // notes are left out: there the band's envelope (rising faster than it falls) rides a
        // little above any single analysis.
        let mut settings = flat();
        settings[0] = BandSettings { enabled: true, kind: BandType::TiltShelf, freq: 1000.0, q: 0.7, ..Default::default() };
        settings[0].dynamics = mite_dsp::eq::Dynamics { enabled: true, spectral: true, threshold_db: 20.0, ratio: 4.0, range_db: -12.0, ..Default::default() };
        let mut eq = EqEngine::new(SR, &settings, 0.0);
        let n = 96_000;
        let notes = [42.0, 63.0, 110.0, 440.0, 2500.0, 9000.0];
        let input: Vec<f32> = (0..n).map(|i| notes.iter().map(|&f| (0.1 * (2.0 * PI * f * i as f64 / f64::from(SR)).sin()) as f32).sum()).collect();
        let (mut l, mut r) = (input.clone(), input.clone());
        run(&mut eq, &settings, &mut l, &mut r, false);
        let mut analyzer = mite_dsp::analyzer::Analyzer::new(mite_dsp::analyzer::ANALYZER_SIZE);
        let mut power = vec![0.0; analyzer.bins()];
        analyzer.analyze_power(&input[n - analyzer.size()..], &mut power);
        let mut shown = vec![0.0; SPECTRUM_POINTS];
        mite_dsp::analyzer::LogSpectrum::new(analyzer.size(), f64::from(SR)).read(&power, &mut shown);
        let mut compared = 0;
        for (i, &want) in shown.iter().enumerate().filter(|&(_, &v)| v > -50.0) {
            let (heard, _, _) = eq.spectral_band_db(0, i);
            assert!((heard - want).abs() < 0.5, "at {:.0} Hz the band hears {heard:.1} dB, the analyzer shows {want:.1} dB", spectrum_point_freq(i));
            compared += 1;
        }
        assert!(compared > 30, "only {compared} points compared");
    }

    #[test]
    fn a_spectral_compressor_moves_only_its_loud_bins_through_the_engine() {
        // Thresholds are on the analyzer's scale (a sine reads about 12 dB under its dBFS level).
        let mut settings = flat();
        settings[0] = BandSettings { enabled: true, kind: BandType::Bell, freq: 2000.0, q: 0.3, ..Default::default() };
        settings[0].dynamics = mite_dsp::eq::Dynamics { enabled: true, spectral: true, threshold_db: -45.0, ratio: 4.0, range_db: -12.0, ..Default::default() };
        let mut eq = EqEngine::new(SR, &settings, 0.0);
        assert_eq!(eq.latency(), spectral_latency(f64::from(SR)));
        let n = 96_000;
        let input: Vec<f32> = sine(1500.0, n).iter().zip(sine(3000.0, n)).map(|(a, b)| a + 0.006 * b).collect();
        let (mut l, mut r) = (input.clone(), input.clone());
        run(&mut eq, &settings, &mut l, &mut r, false);
        assert!(eq.spectral_gain_db(1500.0) < -6.0, "{}", eq.spectral_gain_db(1500.0));
        assert!(eq.spectral_gain_db(3000.0).abs() < 0.5, "{}", eq.spectral_gain_db(3000.0));
        // The whole-band IIR detector stays out of it.
        assert_eq!(eq.bands[0].dynamic_gain_db(), 0.0);
    }

    #[test]
    fn the_analyzer_compares_the_same_moment_before_and_after() {
        // A spectral bell high up (8 kHz) adds the STFT's latency. Below it nothing is touched,
        // so the pre and post spectra must agree there, on changing material too: the pre tap
        // has to hear the input as delayed as the output is.
        use mite_dsp::analyzer::{Analyzer, LogSpectrum, ANALYZER_SIZE, SPECTRUM_POINTS, spectrum_point_freq};
        let mut settings = flat();
        settings[0] = BandSettings { enabled: true, kind: BandType::Bell, freq: 8000.0, q: 2.0, ..Default::default() };
        settings[0].dynamics = mite_dsp::eq::Dynamics { enabled: true, spectral: true, threshold_db: -60.0, range_db: -12.0, ..Default::default() };
        for mode in [PhaseMode::ZeroLatency, PhaseMode::LinearPhase] {
            let mut eq = engine_in(mode, SR, &settings);
            assert!(eq.latency() > 0);
            let (pre, post) = (SpectrumTap::new(ANALYZER_SIZE), SpectrumTap::new(ANALYZER_SIZE));
            // Noise in bursts: the spectrum changes from one moment to the next.
            let mut input = noise(48_000, 13);
            for (i, v) in input.iter_mut().enumerate() {
                *v *= if (i / 1500) % 2 == 0 { 1.0 } else { 0.05 };
            }
            let (mut l, mut r) = (input.clone(), input);
            for (a, b) in l.chunks_mut(128).zip(r.chunks_mut(128)) {
                eq.process(&mut [a, b], &settings, 0.0, Some(&pre), Some(&post));
            }
            let mut analyzer = Analyzer::new(ANALYZER_SIZE);
            let spectrum = LogSpectrum::new(ANALYZER_SIZE, f64::from(SR));
            let read = |tap: &SpectrumTap, analyzer: &mut Analyzer| {
                let mut samples = vec![0.0; ANALYZER_SIZE];
                tap.read_latest(&mut samples);
                let mut power = vec![0.0; analyzer.bins()];
                analyzer.analyze_power(&samples, &mut power);
                let mut db = vec![0.0; SPECTRUM_POINTS];
                spectrum.read(&power, &mut db);
                db
            };
            let (a, b) = (read(&pre, &mut analyzer), read(&post, &mut analyzer));
            let worst = (0..SPECTRUM_POINTS)
                .filter(|&i| (40.0..2000.0).contains(&spectrum_point_freq(i)))
                .map(|i| (a[i] - b[i]).abs())
                .fold(0.0f32, f32::max);
            assert!(worst < 0.2, "{mode:?}: pre and post differ by {worst:.2} dB below the band");
        }
    }

    /// A 1 kHz compressor band (−12 dB, fast) listening to `source`.
    fn sidechain_band(source: DetectSource) -> [BandSettings; BANDS] {
        let mut s = flat();
        s[0] = BandSettings { enabled: true, kind: BandType::Bell, freq: 1000.0, q: 1.0, ..Default::default() };
        s[0].dynamics = mite_dsp::eq::Dynamics { enabled: true, threshold_db: -20.0, ratio: 10.0, range_db: -12.0, attack_ms: 0.5, release_ms: 50.0, source, ..Default::default() };
        s
    }

    #[test]
    fn the_sidechain_ducks_a_band_in_time_in_every_mode() {
        // A steady tone; the sidechain is silent, then loud from sample `at`. The band must duck
        // where the output shows that moment: `at` plus the latency (Linear Phase delays the
        // signal, so the sidechain is delayed with it).
        let settings = sidechain_band(DetectSource::Sidechain);
        let at = 24_000;
        for mode in [PhaseMode::ZeroLatency, PhaseMode::LinearPhase] {
            let mut eq = engine_in(mode, SR, &settings);
            let latency = eq.latency();
            let tone = sine(1000.0, 48_000);
            let key: Vec<f32> = (0..48_000).map(|n| if n >= at { tone[n] } else { 0.0 }).collect();
            let (mut l, mut r) = (tone.clone(), tone.clone());
            for (i, (a, b)) in l.chunks_mut(128).zip(r.chunks_mut(128)).enumerate() {
                let k = &key[i * 128..i * 128 + a.len()];
                eq.process_sidechain(&mut [a, b], Some(&[k, k]), &settings, 0.0, None, None);
            }
            // Peak over each 1 ms: full before the key, ducked after it.
            let peak = |from: usize| l[from..from + 48].iter().fold(0.0f32, |m, v| m.max(v.abs()));
            let steady = peak(at + latency - 2_000);
            assert!(steady > 0.45, "{mode:?}: {steady}");
            let first = (at + latency - 2_000..48_000 - 48).find(|&n| peak(n) < 0.8 * steady).expect("it ducks");
            let lag = first as i64 - (at + latency) as i64;
            assert!((-48..=480).contains(&lag), "{mode:?}: ducks {lag} samples off the key");
            assert!(peak(47_000) < 0.35, "{mode:?}: ducked to {}", peak(47_000));
        }
    }

    #[test]
    fn a_band_listening_to_another_bands_region_ducks_through_the_engine() {
        // Band 1 works at 1 kHz but listens to band 2's region (8 kHz, a flat bell).
        let mut settings = sidechain_band(DetectSource::Band(1));
        settings[1] = BandSettings { enabled: true, kind: BandType::Bell, freq: 8000.0, q: 2.0, ..Default::default() };
        let tone = sine(1000.0, 48_000);
        let level = |input: &[f32]| {
            let mut eq = EqEngine::new(SR, &settings, 0.0);
            let (mut l, mut r) = (input.to_vec(), input.to_vec());
            run(&mut eq, &settings, &mut l, &mut r, false);
            20.0 * (rms(&l[24_000..]) / rms(&input[24_000..])).log10()
        };
        assert!(level(&tone).abs() < 0.3, "{}", level(&tone));
        let both: Vec<f32> = tone.iter().zip(sine(8000.0, 48_000)).map(|(a, b)| a + 0.5 * b).collect();
        assert!(level(&both) < -3.0, "{}", level(&both));
    }

    #[test]
    fn zero_latency_stays_zero_latency_and_matches_the_analog_magnitude() {
        // A 14 kHz bell at 44.1 kHz: corrected, it is still causal from the first sample (an
        // impulse comes out where it went in, nothing before) and its magnitude is the analog
        // prototype's. The phase stays minimum phase: within a few degrees of the prototype's in
        // the mids and about 15° near Nyquist (as the IIR alone; Natural Phase matches it).
        let sr = 44_100.0_f32;
        let mut settings = flat();
        settings[0] = BandSettings { enabled: true, kind: BandType::Bell, freq: 14_000.0, gain_db: 8.0, q: 1.2, ..Default::default() };
        let mut eq = engine_in(PhaseMode::ZeroLatency, sr, &settings);
        assert_eq!(eq.latency(), 0);
        let n = 8192;
        let at = 1000;
        let mut l = vec![0.0f32; n];
        l[at] = 1.0;
        let mut r = l.clone();
        run(&mut eq, &settings, &mut l, &mut r, false);
        assert!(l[..at].iter().all(|&v| v == 0.0), "output before the impulse");
        let h = &l[at..];
        for f in [2_000.0, 8_000.0, 12_000.0, 16_000.0, 19_000.0] {
            let w = 2.0 * PI * f / f64::from(sr);
            let (re, im) = h.iter().enumerate().fold((0.0f64, 0.0f64), |(re, im), (k, &v)| {
                let (sin, cos) = (w * k as f64).sin_cos();
                (re + f64::from(v) * cos, im - f64::from(v) * sin)
            });
            let analog = mite_dsp::eq::analog_response(&settings[0], f);
            let db: f64 = 20.0 * (re.hypot(im) / analog.norm()).log10();
            let mut deg: f64 = (im.atan2(re) - analog.im.atan2(analog.re)).to_degrees();
            deg = (deg + 180.0).rem_euclid(360.0) - 180.0;
            assert!(db.abs() < 0.1, "{f} Hz: {db:+.3} dB");
            assert!(deg.abs() < 20.0, "{f} Hz: phase {deg:+.1}° off the analog prototype");
        }
    }

    #[test]
    fn zero_latency_corrections_crossfade_without_clicks() {
        // A high bell whose gain is automated: its correction is redesigned and crossfaded in
        // again and again, under a tone near it.
        let sr = 44_100.0_f32;
        let mut settings = flat();
        settings[0] = BandSettings { enabled: true, kind: BandType::Bell, freq: 15_000.0, gain_db: 0.0, q: 1.0, ..Default::default() };
        let mut eq = EqEngine::new(sr, &settings, 0.0);
        let input: Vec<f32> = (0..88_200).map(|n| (0.25 * (2.0 * PI * 3_000.0 * n as f64 / f64::from(sr)).sin()) as f32).collect();
        let (mut l, mut r) = (input.clone(), input.clone());
        for (i, (a, b)) in l.chunks_mut(128).zip(r.chunks_mut(128)).enumerate() {
            settings[0].gain_db = 12.0 * ((i / 30) % 2) as f64;
            eq.process(&mut [a, b], &settings, 0.0, None, None);
            std::thread::sleep(Duration::from_micros(300));
        }
        // A 3 kHz sine at 0.25 moves at most about 0.11 per sample; the bell barely touches it.
        let step = largest_step(&l);
        assert!(step < 0.13, "step {step}");
    }

    #[test]
    fn taps_receive_mono_sum() {
        let tap = SpectrumTap::new(16);
        let mut eq = EqEngine::new(SR, &flat(), 0.0);
        let (mut l, mut r) = (vec![1.0_f32; 4], vec![0.0_f32; 4]);
        eq.process(&mut [&mut l, &mut r], &flat(), 0.0, Some(&tap), None);
        let mut out = [0.0; 4];
        tap.read_latest(&mut out);
        assert_eq!(out, [0.5; 4]);
    }

    fn rms(x: &[f32]) -> f64 {
        (x.iter().map(|v| f64::from(*v).powi(2)).sum::<f64>() / x.len() as f64).sqrt()
    }

    fn largest_step(x: &[f32]) -> f32 {
        x.windows(2).map(|w| (w[1] - w[0]).abs()).fold(0.0f32, f32::max)
    }

    #[test]
    fn mix_zero_and_bypass_null_against_the_delayed_input_in_every_mode() {
        let settings = bell(9.0);
        for mode in [PhaseMode::ZeroLatency, PhaseMode::NaturalPhase, PhaseMode::LinearPhase] {
            for output in [Output { mix: 0.0, ..Default::default() }, Output { bypass: true, ..Default::default() }] {
                let mut eq = engine_in(mode, SR, &settings);
                eq.set_output(output);
                eq.reset(&settings, 0.0);
                let latency = eq.latency();
                let input = noise(16_384, 11);
                let (mut l, mut r) = (input.clone(), input.clone());
                run(&mut eq, &settings, &mut l, &mut r, false);
                let worst = (0..16_384 - latency).map(|n| (l[n + latency] - input[n]).abs()).fold(0.0f32, f32::max);
                assert!(worst == 0.0, "{mode:?} {output:?}: residual {worst}");
            }
        }
    }

    #[test]
    fn delta_plays_exactly_what_the_eq_changes() {
        let settings = bell(9.0);
        for mode in [PhaseMode::ZeroLatency, PhaseMode::LinearPhase] {
            let input = noise(16_384, 5);
            let mut wet = engine_in(mode, SR, &settings);
            let (mut wl, mut wr) = (input.clone(), input.clone());
            run(&mut wet, &settings, &mut wl, &mut wr, false);
            let mut delta = engine_in(mode, SR, &settings);
            delta.set_output(Output { delta: true, ..Default::default() });
            delta.reset(&settings, 0.0);
            let latency = delta.latency();
            let (mut dl, mut dr) = (input.clone(), input.clone());
            run(&mut delta, &settings, &mut dl, &mut dr, false);
            let worst = (0..16_384 - latency).map(|n| (dl[n + latency] + input[n] - wl[n + latency]).abs()).fold(0.0f32, f32::max);
            assert!(worst < 1e-6, "{mode:?}: residual {worst}");
            // And a neutral EQ's delta is silence.
            let mut empty = engine_in(mode, SR, &flat());
            empty.set_output(Output { delta: true, ..Default::default() });
            empty.reset(&flat(), 0.0);
            let (mut el, mut er) = (input.clone(), input.clone());
            run(&mut empty, &flat(), &mut el, &mut er, false);
            assert!(el.iter().all(|v| v.abs() < 1e-6), "{mode:?}: neutral delta not silent");
        }
    }

    #[test]
    fn output_switches_are_click_free() {
        let settings = bell(9.0);
        let mut eq = EqEngine::new(SR, &settings, 0.0);
        let steps = [
            Output { bypass: true, ..Default::default() },
            Output::default(),
            Output { delta: true, ..Default::default() },
            Output::default(),
            Output { mix: 0.0, ..Default::default() },
            Output { pan: 1.0, ..Default::default() },
            Output { solo: Some(0), ..Default::default() },
            Output::default(),
            Output { auto_gain: true, ..Default::default() },
            Output::default(),
        ];
        let input = sine(1000.0, 4800 * steps.len());
        let (mut l, mut r) = (input.clone(), input.clone());
        for (i, (a, b)) in l.chunks_mut(4800).zip(r.chunks_mut(4800)).enumerate() {
            eq.set_output(steps[i]);
            for (x, y) in a.chunks_mut(128).zip(b.chunks_mut(128)) {
                eq.process(&mut [x, y], &settings, 0.0, None, None);
            }
        }
        // A 1 kHz sine at 0.5 (+9 dB at most) moves at most about 0.37 per sample.
        let step = largest_step(&l);
        assert!(step < 0.45, "step {step}");
    }

    #[test]
    fn auto_gain_keeps_the_loudness_of_a_broad_boost() {
        let mut settings = flat();
        settings[0] = BandSettings { enabled: true, kind: BandType::Bell, freq: 2000.0, gain_db: 6.0, q: 0.3, ..Default::default() };
        // Pink noise (Paul Kellet's filter on white), the spectrum auto gain is weighted for.
        let mut b = [0.0f32; 7];
        let input: Vec<f32> = noise(96_000, 9)
            .into_iter()
            .map(|w| {
                b[0] = 0.99886 * b[0] + w * 0.0555179;
                b[1] = 0.99332 * b[1] + w * 0.0750759;
                b[2] = 0.96900 * b[2] + w * 0.153852;
                b[3] = 0.86650 * b[3] + w * 0.3104856;
                b[4] = 0.55000 * b[4] + w * 0.5329522;
                b[5] = -0.7616 * b[5] - w * 0.0168980;
                let pink = b[0] + b[1] + b[2] + b[3] + b[4] + b[5] + b[6] + w * 0.5362;
                b[6] = w * 0.115926;
                pink * 0.2
            })
            .collect();
        // Loudness as auto gain weighs it (K weighting: a high pass near 38 Hz, +4 dB above 1.5 kHz).
        let loudness = |x: &[f32]| {
            let k = [
                BandSettings { enabled: true, kind: BandType::LowCut, freq: 38.0, slope: 1, ..Default::default() },
                BandSettings { enabled: true, kind: BandType::HighShelf, freq: 1500.0, gain_db: 4.0, q: 0.7, ..Default::default() },
            ];
            let mut y = x.to_vec();
            for s in &k {
                let mut band = BandProcessor::new(f64::from(SR), 20.0);
                band.reset(s);
                band.process(&mut [&mut y]);
            }
            rms(&y)
        };
        let change = |auto_gain: bool| {
            let mut eq = EqEngine::new(SR, &settings, 0.0);
            eq.set_output(Output { auto_gain, ..Default::default() });
            eq.reset(&settings, 0.0);
            let (mut l, mut r) = (input.clone(), input.clone());
            run(&mut eq, &settings, &mut l, &mut r, false);
            20.0 * (loudness(&l[48_000..]) / loudness(&input[48_000..])).log10()
        };
        let (plain, compensated) = (change(false), change(true));
        assert!(plain > 2.0, "{plain}");
        assert!(compensated.abs() < 0.5, "{plain} → {compensated}");
    }

    #[test]
    fn a_soloed_band_plays_only_its_region() {
        let mut settings = flat();
        settings[0] = BandSettings { enabled: true, kind: BandType::Bell, freq: 500.0, gain_db: -6.0, q: 4.0, ..Default::default() };
        let level_with = |settings: &[BandSettings; BANDS], freq: f64, solo: Option<usize>| {
            let mut eq = EqEngine::new(SR, settings, 0.0);
            eq.set_output(Output { solo, ..Default::default() });
            let input = sine(freq, 24_000);
            let (mut l, mut r) = (input.clone(), input.clone());
            run(&mut eq, settings, &mut l, &mut r, false);
            20.0 * (rms(&l[12_000..]) / rms(&input[12_000..])).log10()
        };
        let level = |freq, solo| level_with(&settings, freq, solo);
        assert!(level(5000.0, None).abs() < 0.1);
        assert!(level(5000.0, Some(0)) < -30.0, "{}", level(5000.0, Some(0)));
        // At the centre the band pass is unity, so the solo plays the band's own −6 dB.
        assert!((level(500.0, Some(0)) + 6.0).abs() < 0.2, "{}", level(500.0, Some(0)));
        // A disabled band cannot solo.
        settings[0].enabled = false;
        assert!(level_with(&settings, 5000.0, Some(0)).abs() < 0.1);
    }

    #[test]
    fn a_soloed_side_band_plays_only_the_side() {
        let mut settings = flat();
        settings[0] = BandSettings { enabled: true, kind: BandType::Bell, freq: 1000.0, q: 1.0, placement: Placement::Side, ..Default::default() };
        let mut eq = EqEngine::new(SR, &settings, 0.0);
        eq.set_output(Output { solo: Some(0), ..Default::default() });
        // Identical channels: no side at all.
        let input = sine(1000.0, 24_000);
        let (mut l, mut r) = (input.clone(), input.clone());
        run(&mut eq, &settings, &mut l, &mut r, false);
        assert!(rms(&l[12_000..]) < 1e-4, "{}", rms(&l[12_000..]));
    }

    #[test]
    fn balance_closes_the_other_side() {
        let mut eq = EqEngine::new(SR, &flat(), 0.0);
        eq.set_output(Output { pan: 1.0, ..Default::default() });
        eq.reset(&flat(), 0.0);
        let input = sine(1000.0, 4800);
        let (mut l, mut r) = (input.clone(), input.clone());
        run(&mut eq, &flat(), &mut l, &mut r, false);
        assert!(rms(&l) < 1e-6);
        assert!((rms(&r) - rms(&input)).abs() < 1e-6);
    }
}
