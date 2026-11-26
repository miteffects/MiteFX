//! The phase page: what a chain does to the signal, measured live on whatever plays.
//!
//! Put one Mite Analyzer before the chain and this one after it (or feed the chain's input to
//! the sidechain). The reference is the signal before, the measurement the one after; from the
//! two (`mite_dsp::transfer`):
//!
//! - the chain's **delay** (GCC-PHAT, averaged; fractional) and **polarity**,
//! - its **phase** on top of that delay, wrapped to ±180°,
//! - its **magnitude** and **group delay** (the delay per frequency, the bulk delay included),
//! - its **impulse response** around the delay,
//! - the **coherence**, which every curve wears as its opacity: where the two signals don't
//!   explain each other (noise, modulation, a reverb tail, nothing playing there) the curve fades
//!   instead of showing a number it can't stand behind.
//!
//! Another instance's streams line up with this one's by write count: `Δ` = its count minus ours
//! at the same moment. Read so that ours can't move in between, the difference is Δ or more
//! (an upstream instance has written the block we haven't yet), so the running minimum over the
//! last two seconds is Δ. The sidechain is pushed in the same block, Δ = 0.

use std::collections::VecDeque;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Instant;

use mite_dsp::transfer::{average_weight, blend, DelayFinder, Transfer, TransferPoint};
use mite_ui::kurbo::{Affine, BezPath, Circle, Line, Point, Rect, RoundedRect, Stroke};
use mite_ui::paint::{glow_stroke, panel};
use mite_ui::peniko::{Color, Fill};
use mite_ui::widgets::footer::draw_dropdown_button;
use mite_ui::widgets::history::smooth_path;
use mite_ui::{Align, DrawCx, ParamHost, TextStyle};
use realfft::num_complex::Complex;

use crate::params::{AVERAGE, AVERAGE_S, SMOOTH, SMOOTH_OCT};
use crate::{AnalyzerShared, ANALYZER_SOURCE, INSTANCES, SOURCE_AUTO, SOURCE_SIDECHAIN};

/// The transfer function's frames and hop; the delay search's frame and how often it runs.
pub const FRAME: usize = 16_384;
const HOP: usize = FRAME / 4;
/// The low end's frame (0.73 Hz bins at 48 kHz: a bass note's window lobe would otherwise lend
/// its phase to ±6 Hz around it) and hop; it hands over to the short frame across `LOW_SPLIT` Hz.
const LOW_FRAME: usize = 65_536;
const LOW_HOP: usize = LOW_FRAME / 4;
const LOW_SPLIT: (f64, f64) = (180.0, 360.0);
pub const DELAY_FRAME: usize = 65_536;
const DELAY_EVERY: usize = 12_288;
/// The delay search settles within about a second.
const DELAY_TAU_S: f64 = 1.0;
/// The points across 20 Hz…20 kHz (1/60 octave).
pub const POINTS: usize = 600;
/// The curves ease towards each reading (s).
const EASE_S: f64 = 0.6;
/// Fixed scales (user, 2026-11-26: "it moves too often with audio… centered constantly at 0 ms
/// and no zoom ins and outs"): magnitude ±dB, group delay ms before and after the chain delay,
/// the impulse ms either side of 0. Nothing rescales with the music.
const MAG_RANGE_DB: f64 = 24.0;
const GD_SPAN_MS: (f64, f64) = (10.0, 30.0);
/// How far the impulse view zooms, its whole span in ms.
const IR_ZOOM_MS: (f64, f64) = (0.5, 200.0);
/// How far the group-delay view zooms, its whole span in ms.
const GD_ZOOM_MS: (f64, f64) = (0.5, 500.0);
const IR_SPAN_MS: f64 = 10.0;
/// The impulse's height: its peak, held, letting go this slowly (s per halving), so the trace
/// does not grow and shrink with each reading.
const IR_PEAK_HALF_S: f64 = 4.0;
/// The menu's items: Auto, Sidechain, then Analyzer 1…16 (only those that exist are shown).
pub const SOURCE_ITEMS: &[&str] = &[
    "Auto", "Sidechain", "Analyzer 1", "Analyzer 2", "Analyzer 3", "Analyzer 4", "Analyzer 5", "Analyzer 6", "Analyzer 7", "Analyzer 8", "Analyzer 9", "Analyzer 10", "Analyzer 11", "Analyzer 12", "Analyzer 13",
    "Analyzer 14", "Analyzer 15", "Analyzer 16",
];

// ---- layout (the editor's design units) -------------------------------------------------------

pub const X0: f64 = 64.0;
pub const X1: f64 = 980.0;
pub const MAG: Rect = Rect::new(X0, 72.0, X1, 236.0);
pub const PHASE: Rect = Rect::new(X0, 252.0, X1, 548.0);
pub const GD: Rect = Rect::new(X0, 564.0, X1, 782.0);
const FREQ_LABELS_Y: f64 = 788.0;
pub const CHAIN: Rect = Rect::new(1010.0, 64.0, 1508.0, 420.0);
pub const SOURCE_BUTTON: Rect = Rect::new(CHAIN.x0 + 150.0, CHAIN.y0 + 12.0, CHAIN.x1 - 16.0, CHAIN.y0 + 50.0);
pub const IMPULSE: Rect = Rect::new(1010.0, 428.0, 1508.0, 808.0);
/// The impulse's scale switch: linear, or its envelope in dB (to read how far under the peak
/// anything before 0 is).
pub const IR_LIN_BUTTON: Rect = Rect::new(IMPULSE.x0 + 132.0, IMPULSE.y0 + 8.0, IMPULSE.x0 + 172.0, IMPULSE.y0 + 30.0);
pub const IR_DB_BUTTON: Rect = Rect::new(IMPULSE.x0 + 176.0, IMPULSE.y0 + 8.0, IMPULSE.x0 + 216.0, IMPULSE.y0 + 30.0);
/// The dB scale's floor under the peak.
const IR_FLOOR_DB: f64 = 60.0;
pub const IR_PLOT: Rect = Rect::new(IMPULSE.x0 + 16.0, IMPULSE.y0 + 40.0, IMPULSE.x1 - 16.0, IMPULSE.y1 - 34.0);

const LABEL: TextStyle = TextStyle { size: 12.0, weight: 700.0, tracking: 0.08, tabular: false };
const SMALL: TextStyle = TextStyle { size: 11.0, weight: 700.0, tracking: 0.08, tabular: false };
const AXIS: TextStyle = TextStyle { size: 12.0, weight: 600.0, tracking: 0.0, tabular: true };
const VALUE: TextStyle = TextStyle { size: 24.0, weight: 700.0, tracking: 0.0, tabular: true };
const BIG: TextStyle = TextStyle { size: 60.0, weight: 700.0, tracking: 0.0, tabular: true };
const BODY: TextStyle = TextStyle { size: 15.0, weight: 600.0, tracking: 0.0, tabular: false };

pub fn freq_x(f: f64) -> f64 {
    X0 + (X1 - X0) * (f / 20.0).ln() / 1000f64.ln()
}

pub fn x_freq(x: f64) -> f64 {
    20.0 * 1000f64.powf(((x - X0) / (X1 - X0)).clamp(0.0, 1.0))
}

fn fmt_signed(v: f64, decimals: usize) -> String {
    format!("{:+.*}", decimals, v).replace('-', "−")
}

/// A frequency as the axes write it.
fn fmt_hz(f: f64) -> String {
    if f >= 1000.0 {
        format!("{:.2}k", f / 1000.0)
    } else {
        format!("{f:.0}")
    }
}

/// How much a coherence is believed: nothing under 0.3, all of it from 0.85 (smooth between).
fn trust(coherence: f64) -> f64 {
    let t = ((coherence - 0.3) / 0.55).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Where the reference comes from.
#[derive(Clone)]
enum Reference {
    Sidechain,
    Analyzer(u32, Arc<AnalyzerShared>),
}

impl Reference {
    fn taps<'a>(&'a self, own: &'a AnalyzerShared) -> [&'a mite_core::SpectrumTap; 2] {
        match self {
            Self::Sidechain => [&own.tel.sc_left, &own.tel.sc_right],
            Self::Analyzer(_, other) => [&other.tel.left, &other.tel.right],
        }
    }

    fn number(&self) -> Option<u32> {
        match self {
            Self::Sidechain => None,
            Self::Analyzer(n, _) => Some(*n),
        }
    }
}

/// What the page can say about the measurement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// No reference: nothing on the sidechain, no other instance.
    NoReference,
    /// The other instance runs at another sample rate.
    RateMismatch,
    /// A reference, but nothing lines up yet (silence, or unrelated signals).
    Searching,
    /// This instance is the one before the chain: the other one lags it.
    Upstream(u32),
    Measuring,
}

/// The numbers as shown (held for the editor's readout time).
#[derive(Debug, Clone, Copy, Default)]
struct Shown {
    delay: Option<f64>,
    inverted: bool,
    coherence: f64,
}

pub struct PhaseLab {
    finder: DelayFinder,
    transfer: Transfer,
    transfer_low: Transfer,
    points_high: Vec<TransferPoint>,
    points_low: Vec<TransferPoint>,
    last_low: usize,
    smooth_i: usize,
    points: Vec<TransferPoint>,
    /// As drawn: H (complex, eased), coherence, total group delay (s).
    eased_h: Vec<Complex<f64>>,
    eased_coh: Vec<f64>,
    eased_gd: Vec<f64>,
    ir: Vec<f64>,
    /// The impulse as drawn (eased like the curves) and its held peak (the drawing's scale).
    eased_ir: Vec<f64>,
    ir_peak: f64,
    /// The impulse window shown, ms before and after the delay (eased).
    ir_span: (f64, f64),
    ir_target: (f64, f64),
    /// The plots' ranges (eased): magnitude ±dB, group delay ms.
    mag_range: f64,
    gd_range: (f64, f64),
    /// The group-delay view, ms below and above the chain delay: fixed unless the user zooms
    /// or scrolls it (user, 2026-11-26: "for group delay I should be able to zoom myself").
    gd_view: (f64, f64),
    /// The impulse view, ms before and after 0: fixed unless the user zooms or scrolls it.
    ir_view: (f64, f64),
    /// The impulse drawn as its envelope in dB (else linear).
    ir_db: bool,
    mag_target: f64,
    gd_target: (f64, f64),
    reference: Option<Reference>,
    /// Instances found to lag this one (this one is their reference); Auto skips them.
    downstream: Vec<u32>,
    /// Write-count offset of the reference against ours, and the observations it comes from.
    delta: Option<isize>,
    observed: VecDeque<(Instant, isize)>,
    /// The next frame's end (our write count) and the last delay search.
    next_end: Option<usize>,
    last_search: usize,
    /// The integer delay the reference is aligned by, the delay as found, its trust.
    delay_int: Option<isize>,
    delay: f64,
    inverted: bool,
    confidence: f64,
    lost: usize,
    status: Status,
    /// Mean coherence over the band, weighted by the reference's presence.
    mean_coherence: f64,
    shown: Shown,
    buf: [Vec<f32>; 4],
    dbuf: [Vec<f32>; 4],
    sample_rate: f64,
    fresh: bool,
}

impl PhaseLab {
    pub fn new(fs: f64) -> Self {
        let transfer = Transfer::new(FRAME, fs, POINTS, SMOOTH_OCT[1]);
        Self {
            finder: DelayFinder::new(DELAY_FRAME, fs),
            ir: transfer.make_impulse_vec(),
            transfer,
            transfer_low: Transfer::new(LOW_FRAME, fs, POINTS, SMOOTH_OCT[1]),
            points_high: vec![TransferPoint::default(); POINTS],
            points_low: vec![TransferPoint::default(); POINTS],
            last_low: 0,
            smooth_i: 1,
            points: vec![TransferPoint::default(); POINTS],
            eased_h: vec![Complex::new(0.0, 0.0); POINTS],
            eased_coh: vec![0.0; POINTS],
            eased_gd: vec![0.0; POINTS],
            ir_span: (IR_SPAN_MS, IR_SPAN_MS),
            ir_target: (IR_SPAN_MS, IR_SPAN_MS),
            mag_range: MAG_RANGE_DB,
            gd_range: (-GD_SPAN_MS.0, GD_SPAN_MS.1),
            gd_view: GD_SPAN_MS,
            ir_view: (IR_SPAN_MS, IR_SPAN_MS),
            ir_db: false,
            mag_target: MAG_RANGE_DB,
            gd_target: (-GD_SPAN_MS.0, GD_SPAN_MS.1),
            eased_ir: Vec::new(),
            ir_peak: 0.0,
            reference: None,
            downstream: Vec::new(),
            delta: None,
            observed: VecDeque::with_capacity(512),
            next_end: None,
            last_search: 0,
            delay_int: None,
            delay: 0.0,
            inverted: false,
            confidence: 0.0,
            lost: 0,
            status: Status::NoReference,
            mean_coherence: 0.0,
            shown: Shown::default(),
            buf: std::array::from_fn(|_| vec![0.0; FRAME]),
            dbuf: std::array::from_fn(|_| vec![0.0; DELAY_FRAME]),
            sample_rate: fs,
            fresh: false,
        }
    }

    // ---- for tests and tools ----------------------------------------------------------------

    pub fn status(&self) -> Status {
        self.status
    }

    /// The delay as found, samples (positive: the chain delays).
    pub fn delay(&self) -> Option<f64> {
        (self.status == Status::Measuring).then_some(self.delay)
    }

    pub fn inverted(&self) -> bool {
        self.inverted
    }

    /// The reading at `hz` (nearest point): magnitude dB, phase degrees, total group delay ms,
    /// coherence.
    pub fn at(&self, hz: f64) -> (f64, f64, f64, f64) {
        let i = self.index(hz);
        let p = self.points[i];
        let fs = self.sample_rate;
        (p.magnitude_db, p.phase.to_degrees(), (p.group_delay + self.delay / fs) * 1000.0, p.coherence)
    }

    fn index(&self, hz: f64) -> usize {
        let t = (hz / 20.0).ln() / 1000f64.ln();
        ((t * (POINTS - 1) as f64).round().max(0.0) as usize).min(POINTS - 1)
    }

    /// Starts the measurement again (the averages, the delay), and Auto looks at every instance
    /// again.
    pub fn restart(&mut self) {
        self.downstream.clear();
        self.reset_measurement();
    }

    fn reset_measurement(&mut self) {
        self.finder.reset();
        self.transfer.reset();
        self.transfer_low.reset();
        self.delay_int = None;
        self.lost = 0;
        self.fresh = false;
        self.eased_coh.fill(0.0);
        if !matches!(self.status, Status::NoReference | Status::RateMismatch) {
            self.status = Status::Searching;
        }
    }

    // ---- choosing the reference ---------------------------------------------------------------

    fn carries_signal(tap: &mite_core::SpectrumTap, scratch: &mut [f32]) -> bool {
        tap.read_latest(&mut scratch[..4096]);
        scratch[..4096].iter().map(|&v| f64::from(v) * f64::from(v)).sum::<f64>() / 4096.0 > 1e-9
    }

    /// The reference the source setting asks for, now.
    fn resolve(&mut self, me: &Arc<AnalyzerShared>) -> Option<Reference> {
        let source = me.phase_source.load(Ordering::Relaxed);
        let others = INSTANCES.others(me);
        let pick = |n: u32| others.iter().find(|(m, _)| *m == n).map(|(m, a)| Reference::Analyzer(*m, a.clone()));
        match source {
            SOURCE_SIDECHAIN => Some(Reference::Sidechain),
            SOURCE_AUTO => {
                if Self::carries_signal(&me.tel.sc_left, &mut self.buf[0]) || Self::carries_signal(&me.tel.sc_right, &mut self.buf[0]) {
                    return Some(Reference::Sidechain);
                }
                others.iter().find(|(n, _)| !self.downstream.contains(n)).map(|(n, a)| Reference::Analyzer(*n, a.clone())).or_else(|| {
                    // Every other one lags this one: this is their reference; keep showing that.
                    self.downstream.first().and_then(|&n| pick(n))
                })
            }
            n => pick(n - ANALYZER_SOURCE),
        }
    }

    /// The instances the source menu can offer (indices into [`SOURCE_ITEMS`]).
    pub fn source_choices(me: &Arc<AnalyzerShared>) -> Vec<usize> {
        let mut v = vec![0, 1];
        v.extend(INSTANCES.others(me).into_iter().map(|(n, _)| (n + ANALYZER_SOURCE) as usize).filter(|&i| i < SOURCE_ITEMS.len()));
        v
    }

    /// What the source button says: the choice, and under Auto what it found.
    pub fn source_label(&self, me: &AnalyzerShared) -> String {
        let source = me.phase_source.load(Ordering::Relaxed) as usize;
        let name = SOURCE_ITEMS.get(source).copied().unwrap_or("Auto");
        if source == SOURCE_AUTO as usize {
            match &self.reference {
                Some(Reference::Sidechain) => "Auto · Sidechain".into(),
                Some(Reference::Analyzer(n, _)) => format!("Auto · Analyzer {n}"),
                None => "Auto".into(),
            }
        } else {
            name.into()
        }
    }

    // ---- measuring ----------------------------------------------------------------------------

    /// Follows the streams: aligns, searches the delay, feeds the transfer function. At most
    /// `max_frames` frames (unbounded when settling).
    fn run(&mut self, me: &Arc<AnalyzerShared>, host: &dyn ParamHost, max_frames: usize) {
        let fs = f64::from(me.sample_rate.load()).max(8000.0);
        if (fs - self.sample_rate).abs() > 0.5 {
            *self = Self::new(fs);
        }
        let smooth_i = (host.plain(SMOOTH.id, host.normalized(SMOOTH.id)).round().max(0.0) as usize).min(SMOOTH_OCT.len() - 1);
        if smooth_i != self.smooth_i {
            self.smooth_i = smooth_i;
            self.transfer.set_smoothing(SMOOTH_OCT[smooth_i]);
            self.transfer_low.set_smoothing(SMOOTH_OCT[smooth_i]);
            self.fresh = self.transfer.has_data();
        }
        let avg_i = (host.plain(AVERAGE.id, host.normalized(AVERAGE.id)).round().max(0.0) as usize).min(AVERAGE_S.len() - 1);
        let reference = self.resolve(me);
        let changed = match (&reference, &self.reference) {
            (Some(Reference::Sidechain), Some(Reference::Sidechain)) | (None, None) => false,
            (Some(Reference::Analyzer(a, _)), Some(Reference::Analyzer(b, _))) => a != b,
            _ => true,
        };
        if changed {
            self.reference = reference;
            self.delta = None;
            self.observed.clear();
            self.next_end = None;
            self.reset_measurement();
        }
        let Some(reference) = self.reference.clone() else {
            self.status = Status::NoReference;
            return;
        };
        if let Reference::Analyzer(_, other) = &reference {
            if (f64::from(other.sample_rate.load()) - fs).abs() > 0.5 {
                self.status = Status::RateMismatch;
                return;
            }
        }
        if matches!(self.status, Status::NoReference | Status::RateMismatch) {
            self.status = Status::Searching;
        }
        let own = [&me.tel.left, &me.tel.right];
        let refs = reference.taps(me);
        // Δ: the reference's count minus ours, ours read twice around it so it can't have moved.
        let delta = match &reference {
            Reference::Sidechain => 0,
            Reference::Analyzer(..) => {
                let now = Instant::now();
                for _ in 0..4 {
                    let a = own[0].written();
                    let r = refs[0].written();
                    if own[0].written() == a {
                        self.observed.push_back((now, r.wrapping_sub(a) as isize));
                        break;
                    }
                }
                while self.observed.front().is_some_and(|(t, _)| now.duration_since(*t).as_secs_f64() > 2.0) {
                    self.observed.pop_front();
                }
                let Some(d) = self.observed.iter().map(|o| o.1).min() else { return };
                d
            }
        };
        if self.delta != Some(delta) {
            if self.delta.is_some() {
                self.reset_measurement();
            }
            self.delta = Some(delta);
            self.next_end = None;
        }
        let now = own[0].written();
        let start_from = |n: usize| n.wrapping_sub(if max_frames == usize::MAX { own[0].capacity() * 7 / 16 } else { 0 });
        let mut end = match self.next_end {
            Some(e) if (now.wrapping_sub(e) as isize) < 8 * HOP as isize => e,
            _ => {
                let e = start_from(now);
                self.last_search = e.wrapping_sub(DELAY_EVERY);
                self.last_low = e.wrapping_sub(LOW_HOP);
                e
            }
        };
        let a = average_weight(HOP as f64 / fs, AVERAGE_S[avg_i]);
        let mut frames = 0;
        while (now.wrapping_sub(end) as isize) >= 0 && frames < max_frames {
            // The delay search, every DELAY_EVERY samples.
            if end.wrapping_sub(self.last_search) >= DELAY_EVERY {
                self.last_search = end;
                self.search(&own, &refs, end, delta, fs);
            }
            if let Some(d) = self.delay_int.filter(|_| self.status == Status::Measuring) {
                let re = end.wrapping_add_signed(delta - d);
                let [b0, b1, b2, b3] = &mut self.buf;
                let ok = own[0].read_ending(end, b0) && own[1].read_ending(end, b1) && refs[0].read_ending(re, b2) && refs[1].read_ending(re, b3);
                if ok && self.transfer.feed(&[b2, b3], &[b0, b1], a) {
                    self.fresh = true;
                }
                if end.wrapping_sub(self.last_low) >= LOW_HOP {
                    self.last_low = end;
                    let a_low = average_weight(LOW_HOP as f64 / fs, AVERAGE_S[avg_i]);
                    let [b0, b1, b2, b3] = &mut self.dbuf;
                    let ok = own[0].read_ending(end, b0) && own[1].read_ending(end, b1) && refs[0].read_ending(re, b2) && refs[1].read_ending(re, b3);
                    if ok && self.transfer_low.feed(&[b2, b3], &[b0, b1], a_low) {
                        self.fresh = true;
                    }
                }
            }
            end = end.wrapping_add(HOP);
            frames += 1;
        }
        self.next_end = Some(end);
        if self.fresh {
            self.fresh = false;
            let frac = self.delay - self.delay_int.unwrap_or(0) as f64;
            self.transfer.read(frac, &mut self.points_high);
            if self.transfer_low.has_data() {
                self.transfer_low.read(frac, &mut self.points_low);
                blend(&self.points_low, &self.points_high, LOW_SPLIT.0, LOW_SPLIT.1, &mut self.points);
            } else {
                self.points.copy_from_slice(&self.points_high);
            }
            self.transfer.impulse(&mut self.ir);
            self.measure_ranges(fs);
        }
    }

    /// One delay search, with the frame ending at our count `end`.
    fn search(&mut self, own: &[&mite_core::SpectrumTap; 2], refs: &[&mite_core::SpectrumTap; 2], end: usize, delta: isize, fs: f64) {
        let re = end.wrapping_add_signed(delta);
        let [b0, b1, b2, b3] = &mut self.dbuf;
        if !(own[0].read_ending(end, b0) && own[1].read_ending(end, b1) && refs[0].read_ending(re, b2) && refs[1].read_ending(re, b3)) {
            return;
        }
        let a = average_weight(DELAY_EVERY as f64 / fs, DELAY_TAU_S);
        match self.finder.update(&[b2, b3], &[b0, b1], a) {
            Some(e) => {
                self.lost = 0;
                self.confidence = e.confidence;
                self.inverted = e.inverted;
                // Negative from another instance: it lags this one, so this one is its reference.
                if let (Some(n), true) = (self.reference.as_ref().and_then(Reference::number), e.samples < -0.5) {
                    if !self.downstream.contains(&n) {
                        self.downstream.push(n);
                    }
                    self.status = Status::Upstream(n);
                    return;
                }
                self.delay = e.samples;
                match self.delay_int {
                    Some(d) if (e.samples - d as f64).abs() <= 0.75 => {}
                    _ => {
                        // A new integer alignment: what was averaged at the old one no longer fits.
                        self.delay_int = Some(e.samples.round() as isize);
                        self.transfer.reset();
                        self.transfer_low.reset();
                    }
                }
                self.status = Status::Measuring;
            }
            None => {
                self.lost += 1;
                // About three seconds without a peak: nothing lines up any more.
                if self.lost > (3.0 * fs / DELAY_EVERY as f64) as usize && self.status == Status::Measuring {
                    self.status = Status::Searching;
                }
            }
        }
    }

    /// The mean coherence, and the plots' fixed ranges: magnitude ±24 dB, the impulse ±10 ms
    /// around 0, group delay a fixed span around the chain delay (moving only when the delay
    /// itself moves by a whole ms).
    fn measure_ranges(&mut self, fs: f64) {
        let n = self.points.len().max(1) as f64;
        self.mean_coherence = self.points.iter().map(|p| p.coherence).sum::<f64>() / n;
        self.mag_target = MAG_RANGE_DB;
        let d = (self.delay / fs * 1000.0).round();
        self.gd_target = (d - self.gd_view.0, d + self.gd_view.1);
        self.ir_target = self.ir_view;
    }

    /// Each frame: measure, then ease what is drawn.
    pub fn tick(&mut self, me: &Arc<AnalyzerShared>, host: &dyn ParamHost, dt: f64) {
        self.run(me, host, 3);
        self.ease(1.0 - (-dt / EASE_S).exp());
        self.ir_peak *= 0.5f64.powf(dt / IR_PEAK_HALF_S);
        self.ir_peak = self.ir_peak.max(self.eased_ir.iter().fold(0.0f64, |m, v| m.max(v.abs())));
    }

    /// Everything the streams hold, measured at once and landed (tools render one frame).
    pub fn settle(&mut self, me: &Arc<AnalyzerShared>, host: &dyn ParamHost) {
        self.next_end = None;
        self.run(me, host, usize::MAX);
        self.ease(1.0);
        self.mag_range = self.mag_target;
        self.gd_range = self.gd_target;
        self.ir_span = self.ir_target;
        self.ir_peak = self.eased_ir.iter().fold(0.0f64, |m, v| m.max(v.abs()));
        self.take_readings();
    }

    fn ease(&mut self, k: f64) {
        for (i, p) in self.points.iter().enumerate() {
            let target = Complex::from_polar(10f64.powf(p.magnitude_db / 20.0), p.phase);
            let coh = if self.status == Status::Measuring { p.coherence } else { 0.0 };
            if self.eased_coh[i] <= 0.0 && coh > 0.0 {
                // Nothing was drawn here: start at the reading rather than glide in from zero.
                self.eased_h[i] = target;
                self.eased_gd[i] = p.group_delay + self.delay / self.sample_rate;
            }
            let d = target - self.eased_h[i];
            self.eased_h[i] += d * k;
            self.eased_coh[i] += (coh - self.eased_coh[i]) * k;
            self.eased_gd[i] += (p.group_delay + self.delay / self.sample_rate - self.eased_gd[i]) * k;
        }
        // The impulse eases as the curves do (its fractional alignment, drawn later, is the
        // delay's, which is steady).
        if self.eased_ir.len() != self.ir.len() {
            self.eased_ir = self.ir.clone();
        }
        for (e, v) in self.eased_ir.iter_mut().zip(&self.ir) {
            *e += (v - *e) * k;
        }
        self.mag_range += (self.mag_target - self.mag_range) * k * 0.5;
        self.gd_range.0 += (self.gd_target.0 - self.gd_range.0) * k * 0.5;
        self.gd_range.1 += (self.gd_target.1 - self.gd_range.1) * k * 0.5;
        self.ir_span.0 += (self.ir_target.0 - self.ir_span.0) * k * 0.5;
        self.ir_span.1 += (self.ir_target.1 - self.ir_span.1) * k * 0.5;
    }

    /// The chain delay the group-delay view is around, ms (whole ms, as the view follows it).
    fn gd_centre(&self) -> f64 {
        (self.delay / self.sample_rate * 1000.0).round()
    }

    /// The group-delay view set to `view` (ms below, above the chain delay): at once, no glide.
    fn set_gd_view(&mut self, view: (f64, f64)) {
        let span = (view.0 + view.1).clamp(GD_ZOOM_MS.0, GD_ZOOM_MS.1);
        let k = span / (view.0 + view.1).max(1e-9);
        self.gd_view = (view.0 * k, view.1 * k);
        let d = self.gd_centre();
        self.gd_target = (d - self.gd_view.0, d + self.gd_view.1);
        self.gd_range = self.gd_target;
    }

    /// The wheel over the group-delay plot at `y`: zooms around the pointer (as the editors'
    /// wheel does), or with `scroll` moves the view up or down.
    pub fn gd_wheel(&mut self, y: f64, lines: f64, scroll: bool) {
        let (below, above) = self.gd_view;
        let span = below + above;
        if scroll {
            let d = lines * 0.1 * span;
            self.set_gd_view((below - d, above + d));
            return;
        }
        let d = self.gd_centre();
        let at = self.gd_ms_at(y) - d;
        let k = 0.85f64.powf(lines);
        self.set_gd_view(((below + at) * k - at, (above - at) * k + at));
    }

    /// A vertical drag on the group-delay plot by `dy` px: the view follows the pointer.
    pub fn gd_drag(&mut self, dy: f64) {
        let ms = dy / (GD.height() - 16.0) * (self.gd_view.0 + self.gd_view.1);
        self.set_gd_view((self.gd_view.0 - ms, self.gd_view.1 + ms));
    }

    /// The impulse view set to `view` (ms before, after 0): at once, no glide.
    fn set_ir_view(&mut self, view: (f64, f64)) {
        let span = (view.0 + view.1).clamp(IR_ZOOM_MS.0, IR_ZOOM_MS.1);
        let k = span / (view.0 + view.1).max(1e-9);
        self.ir_view = (view.0 * k, view.1 * k);
        self.ir_target = self.ir_view;
        self.ir_span = self.ir_view;
    }

    /// The wheel over the impulse at `x`: zooms in time around the pointer, or with `scroll`
    /// moves the view earlier or later.
    pub fn ir_wheel(&mut self, x: f64, lines: f64, scroll: bool) {
        let (pre, post) = self.ir_view;
        let span = pre + post;
        if scroll {
            let d = lines * 0.1 * span;
            self.set_ir_view((pre + d, post - d));
            return;
        }
        let at = -pre + (x - IR_PLOT.x0) / IR_PLOT.width() * span;
        let k = 0.85f64.powf(lines);
        self.set_ir_view(((pre + at) * k - at, (post - at) * k + at));
    }

    /// A sideways drag on the impulse by `dx` px: the view follows the pointer.
    pub fn ir_drag(&mut self, dx: f64) {
        let ms = dx / IR_PLOT.width() * (self.ir_view.0 + self.ir_view.1);
        self.set_ir_view((self.ir_view.0 + ms, self.ir_view.1 - ms));
    }

    /// The impulse view back to ±10 ms around 0.
    pub fn ir_reset(&mut self) {
        self.set_ir_view((IR_SPAN_MS, IR_SPAN_MS));
    }

    /// The impulse as its envelope in dB (`true`) or linear.
    pub fn set_ir_db(&mut self, db: bool) {
        self.ir_db = db;
    }

    pub fn ir_db(&self) -> bool {
        self.ir_db
    }

    /// The impulse window now, ms before and after 0 (tests and tools).
    pub fn ir_window(&self) -> (f64, f64) {
        self.ir_span
    }

    /// The group-delay view back to its default.
    pub fn gd_reset(&mut self) {
        self.set_gd_view(GD_SPAN_MS);
    }

    /// The ms at plot height `y` on the group-delay plot.
    fn gd_ms_at(&self, y: f64) -> f64 {
        let (lo, hi) = self.gd_range;
        lo + (GD.y1 - 8.0 - y) / (GD.height() - 16.0) * (hi - lo)
    }

    /// The numbers, now (they then hold).
    pub fn take_readings(&mut self) {
        self.shown = Shown { delay: self.delay(), inverted: self.inverted, coherence: self.mean_coherence };
    }

    // ---- drawing ------------------------------------------------------------------------------

    pub fn draw(&self, cx: &mut DrawCx<'_>, me: &AnalyzerShared, pointer: Option<Point>, source_open: bool, source_hot: bool) {
        self.draw_plots(cx, pointer);
        self.draw_chain(cx, me, source_open, source_hot);
        self.draw_impulse(cx);
    }

    fn grid_x(cx: &mut DrawCx<'_>, r: Rect) {
        let theme = cx.theme;
        for f in [20.0, 50.0, 100.0, 200.0, 500.0, 1000.0, 2000.0, 5000.0, 10_000.0, 20_000.0] {
            let x = freq_x(f);
            cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, theme.grid_minor, None, &Line::new((x, r.y0), (x, r.y1)));
        }
    }

    fn mag_y(&self, db: f64) -> f64 {
        let r = self.mag_range.max(1.0);
        MAG.center().y - (db.clamp(-r * 1.05, r * 1.05) / r) * (MAG.height() / 2.0 - 8.0)
    }

    fn phase_y(deg: f64) -> f64 {
        PHASE.center().y - deg / 180.0 * (PHASE.height() / 2.0 - 10.0)
    }

    fn gd_y(&self, ms: f64) -> f64 {
        let (lo, hi) = self.gd_range;
        let span = (hi - lo).max(1e-3);
        GD.y1 - 8.0 - ((ms - lo) / span).clamp(-0.05, 1.05) * (GD.height() - 16.0)
    }

    /// A curve whose opacity follows the coherence: runs of points of one opacity step, each a
    /// smooth path (overlapping one point so they join), broken where `split` says (a phase
    /// wrap).
    fn faded_curve(cx: &mut DrawCx<'_>, pts: &[(Point, f64)], color: Color, width: f64, glow: bool, split: impl Fn(usize) -> bool) {
        const STEPS: f64 = 6.0;
        let level = |t: f64| (t * STEPS).round() / STEPS;
        let mut i = 0;
        while i < pts.len() {
            let l = level(pts[i].1);
            let mut j = i + 1;
            while j < pts.len() && level(pts[j].1) == l && !split(j) {
                j += 1;
            }
            let joined = j < pts.len() && !split(j);
            if l > 0.0 {
                let run: Vec<Point> = pts[i..if joined { j + 1 } else { j }].iter().map(|p| p.0).collect();
                if run.len() >= 2 {
                    let path = smooth_path(&run);
                    let c = color.multiply_alpha(l as f32);
                    if glow && l >= 0.99 {
                        glow_stroke(cx, &path, width, c, c, 2.2);
                    } else {
                        cx.scene.stroke(&Stroke::new(width), Affine::IDENTITY, c, None, &path);
                    }
                }
            }
            i = j;
        }
    }

    #[allow(clippy::needless_range_loop)] // neighbours by index
    fn draw_plots(&self, cx: &mut DrawCx<'_>, pointer: Option<Point>) {
        let theme = cx.theme;
        let fs = self.sample_rate;
        for (r, name) in [(MAG, "MAGNITUDE · dB"), (PHASE, "PHASE"), (GD, "GROUP DELAY · ms")] {
            cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.well, None, &RoundedRect::from_rect(r, 6.0));
            let _ = name;
            Self::grid_x(cx, r);
        }
        for f in [20.0, 50.0, 100.0, 200.0, 500.0, 1000.0, 2000.0, 5000.0, 10_000.0, 20_000.0] {
            let t = if f >= 1000.0 { format!("{}k", f / 1000.0) } else { format!("{f}") };
            let align = if f >= 20_000.0 { Align::Right } else if f <= 20.0 { Align::Left } else { Align::Center };
            cx.text.draw(cx.scene, &t, freq_x(f), FREQ_LABELS_Y, AXIS, theme.text_lo, align);
        }
        // Grids: magnitude every 6 dB (or 12), phase every 45°, group delay at round ms.
        let r = self.mag_range;
        let step = if r > 18.0 { 12.0 } else if r > 9.0 { 6.0 } else { 3.0 };
        let mut db = -(r / step).floor() * step;
        while db <= r + 1e-6 {
            let y = self.mag_y(db);
            let color = if db == 0.0 { theme.grid_zero } else { theme.grid_minor };
            cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, color, None, &Line::new((MAG.x0, y), (MAG.x1, y)));
            cx.text.draw(cx.scene, &format!("{}", db as i32).replace('-', "−"), MAG.x0 - 8.0, y - 8.0, AXIS, theme.text_lo, Align::Right);
            db += step;
        }
        for deg in [-180, -135, -90, -45, 0, 45, 90, 135, 180] {
            let y = Self::phase_y(f64::from(deg));
            let color = if deg == 0 { theme.grid_zero } else if deg % 90 == 0 { theme.grid_major } else { theme.grid_minor };
            cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, color, None, &Line::new((PHASE.x0, y), (PHASE.x1, y)));
            if deg % 90 == 0 {
                cx.text.draw(cx.scene, &format!("{deg}°").replace('-', "−"), PHASE.x0 - 8.0, y - 8.0, AXIS, theme.text_lo, Align::Right);
            }
        }
        let (lo, hi) = self.gd_range;
        let span = hi - lo;
        let gstep = [0.1, 0.2, 0.5, 1.0, 2.0, 5.0, 10.0, 20.0, 50.0, 100.0].into_iter().find(|s| span / s <= 6.0).unwrap_or(200.0);
        let mut ms = (lo / gstep).ceil() * gstep;
        while ms <= hi {
            let y = self.gd_y(ms);
            cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, theme.grid_minor, None, &Line::new((GD.x0, y), (GD.x1, y)));
            let shown = if ms.abs() < 1e-9 { 0.0 } else { ms };
            let t = if gstep < 1.0 { format!("{shown:.1}") } else { format!("{shown:.0}") };
            cx.text.draw(cx.scene, &t.replace('-', "−"), GD.x0 - 8.0, y - 8.0, AXIS, theme.text_lo, Align::Right);
            ms += gstep;
        }
        // The titles over their grids, on a patch of the well so no line runs through them.
        for (r, name) in [(MAG, "MAGNITUDE · dB"), (PHASE, "PHASE"), (GD, "GROUP DELAY · ms")] {
            let w = f64::from(cx.text.measure(name, LABEL)) + 16.0;
            cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.well, None, &Rect::new(r.x1 - 6.0 - w, r.y0 + 4.0, r.x1 - 4.0, r.y0 + 26.0));
            cx.text.draw(cx.scene, name, r.x1 - 10.0, r.y0 + 8.0, LABEL, theme.text_lo, Align::Right);
        }
        // The measured delay on the group-delay plot: where a pure delay would lie.
        if self.status == Status::Measuring {
            let y = self.gd_y(self.delay / fs * 1000.0);
            cx.scene.stroke(&Stroke::new(1.0).with_dashes(0.0, [5.0, 4.0]), Affine::IDENTITY, theme.text_mid.multiply_alpha(0.6), None, &Line::new((GD.x0, y), (GD.x1, y)));
        }
        // The curves.
        let mut mag = Vec::with_capacity(POINTS);
        let mut ph = Vec::with_capacity(POINTS);
        let mut gd = Vec::with_capacity(POINTS);
        let mut wrapped = vec![false; POINTS];
        let mut last_deg: Option<f64> = None;
        for i in 0..POINTS {
            let f = self.points[i].freq.max(20.0);
            let x = freq_x(f);
            let h = self.eased_h[i];
            let t = trust(self.eased_coh[i]);
            let deg = h.arg().to_degrees();
            if let Some(l) = last_deg {
                wrapped[i] = (deg - l).abs() > 180.0;
            }
            last_deg = Some(deg);
            mag.push((Point::new(x, self.mag_y(20.0 * h.norm().max(1e-6).log10())), t));
            ph.push((Point::new(x, Self::phase_y(deg)), t));
            gd.push((Point::new(x, self.gd_y(self.eased_gd[i] * 1000.0)), t));
        }
        let ph_color = theme.accent;
        let mag_color = theme.curve;
        let gd_color = theme.sidechain;
        cx.begin_clip(&MAG);
        Self::faded_curve(cx, &mag, mag_color, 1.8, false, |_| false);
        cx.end_clip();
        cx.begin_clip(&PHASE);
        Self::faded_curve(cx, &ph, ph_color, 2.2, true, |i| wrapped[i]);
        cx.end_clip();
        cx.begin_clip(&GD);
        Self::faded_curve(cx, &gd, gd_color, 1.8, false, |_| false);
        cx.end_clip();
        // Nothing to show yet: say why, in the phase plot's middle.
        let words = match self.status {
            Status::Measuring => None,
            Status::NoReference => Some("Put a Mite Analyzer before the chain, or feed the chain's input to the sidechain"),
            Status::RateMismatch => Some("The reference runs at another sample rate"),
            Status::Searching => Some("Listening for the reference in this signal…"),
            Status::Upstream(_) => None,
        };
        if let Some(w) = words {
            cx.text.draw(cx.scene, w, PHASE.center().x, PHASE.center().y - 30.0, BODY, theme.text_mid, Align::Center);
        }
        if let Status::Upstream(n) = self.status {
            cx.text.draw(cx.scene, &format!("This is the reference: Analyzer {n} measures the chain after it"), PHASE.center().x, PHASE.center().y - 30.0, BODY, theme.text_mid, Align::Center);
        }
        // Hover: a line through all three plots, a dot on each curve.
        if let Some(p) = pointer.filter(|p| (X0..=X1).contains(&p.x) && (MAG.y0..=GD.y1).contains(&p.y)) {
            for r in [MAG, PHASE, GD] {
                cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, theme.text_mid.multiply_alpha(0.5), None, &Line::new((p.x, r.y0 + 4.0), (p.x, r.y1 - 4.0)));
            }
            if self.status == Status::Measuring {
                let i = self.index(x_freq(p.x));
                for (pts, r) in [(&mag, MAG), (&ph, PHASE), (&gd, GD)] {
                    let (q, t) = pts[i];
                    if t > 0.05 && r.contains(q) {
                        cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.text_strong.multiply_alpha(t as f32), None, &Circle::new(q, 3.5));
                    }
                }
            }
        }
    }

    fn draw_chain(&self, cx: &mut DrawCx<'_>, me: &AnalyzerShared, source_open: bool, source_hot: bool) {
        let theme = cx.theme;
        panel(cx.scene, RoundedRect::from_rect(CHAIN, 8.0), theme.display, theme.line);
        cx.text.draw(cx.scene, "REFERENCE", CHAIN.x0 + 16.0, SOURCE_BUTTON.center().y - 7.0, LABEL, theme.text_lo, Align::Left);
        draw_dropdown_button(cx, SOURCE_BUTTON, &self.source_label(me), source_open, source_hot, |_, _| {});
        let n = &self.shown;
        let fs = self.sample_rate;
        let (ms, samples) = match n.delay {
            Some(d) => (format!("{:.2}", d / fs * 1000.0).replace('-', "−"), format!("{:.1}", d).replace('-', "−")),
            None => ("—".to_owned(), "—".to_owned()),
        };
        let y = CHAIN.y0 + 76.0;
        cx.text.draw(cx.scene, &ms, CHAIN.x0 + 300.0, y, BIG, theme.text_strong, Align::Right);
        cx.text.draw(cx.scene, "ms", CHAIN.x0 + 308.0, y + 34.0, VALUE.sized(20.0), theme.text_mid, Align::Left);
        cx.text.draw(cx.scene, "CHAIN DELAY", CHAIN.x0 + 20.0, y + 76.0, LABEL, theme.text_lo, Align::Left);
        cx.text.draw(cx.scene, &samples, CHAIN.x1 - 18.0, y + 14.0, VALUE, theme.text_strong, Align::Right);
        cx.text.draw(cx.scene, "SAMPLES", CHAIN.x1 - 18.0, y + 48.0, SMALL, theme.text_lo, Align::Right);
        // Polarity and coherence.
        let row = CHAIN.y0 + 196.0;
        let measuring = n.delay.is_some();
        let (pol, pol_color) = match (measuring, n.inverted) {
            (false, _) => ("—", theme.text_strong),
            (true, false) => ("Normal", theme.text_strong),
            (true, true) => ("Inverted", theme.danger),
        };
        let w = (CHAIN.width() - 32.0) / 2.0;
        cx.text.draw(cx.scene, pol, CHAIN.x0 + 16.0 + w - 16.0, row, VALUE, pol_color, Align::Right);
        cx.text.draw(cx.scene, "POLARITY", CHAIN.x0 + 16.0 + w - 16.0, row + 34.0, SMALL, theme.text_lo, Align::Right);
        let coh = if measuring { format!("{:.2}", n.coherence) } else { "—".to_owned() };
        cx.text.draw(cx.scene, &coh, CHAIN.x1 - 18.0, row, VALUE, theme.text_strong, Align::Right);
        cx.text.draw(cx.scene, "COHERENCE", CHAIN.x1 - 18.0, row + 34.0, SMALL, theme.text_lo, Align::Right);
        // The coherence across the band, as a filled curve: high where the curves can be
        // believed.
        let strip = Rect::new(CHAIN.x0 + 16.0, CHAIN.y1 - 82.0, CHAIN.x1 - 16.0, CHAIN.y1 - 36.0);
        cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.well, None, &RoundedRect::from_rect(strip, 6.0));
        let sx = |f: f64| strip.x0 + 4.0 + (strip.width() - 8.0) * (f / 20.0f64).ln() / 1000f64.ln();
        let sy = |c: f64| strip.y1 - 4.0 - c.clamp(0.0, 1.0) * (strip.height() - 8.0);
        let line: Vec<Point> = (0..POINTS).step_by(3).map(|i| Point::new(sx(self.points[i].freq.max(20.0)), sy(self.eased_coh[i]))).collect();
        let path = smooth_path(&line);
        let mut fill = path.clone();
        if let (Some(a), Some(b)) = (line.first(), line.last()) {
            fill.line_to((b.x, strip.y1 - 4.0));
            fill.line_to((a.x, strip.y1 - 4.0));
            fill.close_path();
        }
        cx.begin_clip(&strip);
        cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.accent.multiply_alpha(0.18), None, &fill);
        cx.scene.stroke(&Stroke::new(1.4), Affine::IDENTITY, theme.accent, None, &path);
        cx.end_clip();
        for (f, align) in [(20.0, Align::Left), (200.0, Align::Center), (2000.0, Align::Center), (20_000.0, Align::Right)] {
            let t = if f >= 1000.0 { format!("{}k", f / 1000.0) } else { format!("{f}") };
            cx.text.draw(cx.scene, &t, sx(f), strip.y1 + 6.0, AXIS, theme.text_lo, align);
        }
        cx.text.draw(cx.scene, "COHERENCE ACROSS THE BAND", strip.x0, strip.y0 - 20.0, SMALL, theme.text_lo, Align::Left);
    }

    fn draw_impulse(&self, cx: &mut DrawCx<'_>) {
        let theme = cx.theme;
        panel(cx.scene, RoundedRect::from_rect(IMPULSE, 8.0), theme.display, theme.line);
        cx.text.draw(cx.scene, "IMPULSE · ms", IMPULSE.x0 + 16.0, IMPULSE.y0 + 12.0, LABEL, theme.text_lo, Align::Left);
        cx.text.draw(cx.scene, "0 = THE CHAIN DELAY", IMPULSE.x1 - 16.0, IMPULSE.y0 + 12.0, SMALL, theme.text_lo, Align::Right);
        for (b, name, on) in [(IR_LIN_BUTTON, "LIN", !self.ir_db), (IR_DB_BUTTON, "dB", self.ir_db)] {
            let rr = RoundedRect::from_rect(b, b.height() / 2.0);
            cx.scene.fill(Fill::NonZero, Affine::IDENTITY, if on { theme.accent.multiply_alpha(0.18) } else { theme.well }, None, &rr);
            cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, if on { theme.accent } else { theme.well_border }, None, &rr);
            cx.text.draw(cx.scene, name, b.center().x, b.center().y - 6.5, SMALL, if on { theme.accent } else { theme.text_mid }, Align::Center);
        }
        let r = IR_PLOT;
        cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.well, None, &RoundedRect::from_rect(r, 6.0));
        let (pre, post) = self.ir_span;
        let fs = self.sample_rate;
        let x_of = |ms: f64| r.x0 + (ms + pre) / (pre + post) * r.width();
        let mid = r.center().y;
        let db_y = |db: f64| r.y0 + 8.0 + (-db).clamp(0.0, IR_FLOOR_DB) / IR_FLOOR_DB * (r.height() - 16.0);
        if self.ir_db {
            for db in [0.0, -20.0, -40.0, -60.0] {
                let y = db_y(db);
                cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, if db == 0.0 { theme.grid_zero } else { theme.grid_minor }, None, &Line::new((r.x0, y), (r.x1, y)));
                cx.text.draw(cx.scene, &format!("{}", db as i32).replace('-', "−"), r.x0 + 6.0, y + 2.0, AXIS, theme.text_lo, Align::Left);
            }
        } else {
            cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, theme.grid_zero, None, &Line::new((r.x0, mid), (r.x1, mid)));
        }
        // Time ticks at a round step.
        let span = pre + post;
        let step = [0.5, 1.0, 2.0, 5.0, 10.0, 20.0, 50.0].into_iter().find(|s| span / s <= 7.0).unwrap_or(100.0);
        let mut t = -(pre / step).floor() * step;
        while t <= post {
            let x = x_of(t);
            let color = if t.abs() < 1e-9 { theme.grid_major } else { theme.grid_minor };
            cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, color, None, &Line::new((x, r.y0), (x, r.y1)));
            let label = if t.abs() < 1e-9 { "0".to_owned() } else { fmt_signed(t, if step < 1.0 { 1 } else { 0 }) };
            cx.text.draw(cx.scene, &label, x, r.y1 + 6.0, AXIS, theme.text_lo, Align::Center);
            t += step;
        }
        if self.status != Status::Measuring {
            return;
        }
        // Per pixel column, the lowest and highest sample there (a long window has many a
        // column), filled between; normalised to the peak.
        let ir = &self.eased_ir;
        let n = ir.len();
        if n == 0 {
            return;
        }
        let peak = self.ir_peak.max(1e-12);
        let frac = self.delay - self.delay_int.unwrap_or(0) as f64;
        let cols = r.width().round() as usize;
        let sample = |lag: f64| -> f64 {
            let i = lag.floor();
            let u = lag - i;
            let at = |k: f64| ir[(k as isize).rem_euclid(n as isize) as usize];
            at(i) + (at(i + 1.0) - at(i)) * u
        };
        let mut top = Vec::with_capacity(cols + 1);
        let mut bottom = Vec::with_capacity(cols + 1);
        let amp = r.height() / 2.0 - 8.0;
        for c in 0..=cols {
            let ms0 = -pre + span * c as f64 / cols as f64;
            let ms1 = -pre + span * (c + 1) as f64 / cols as f64;
            let (l0, l1) = (ms0 / 1000.0 * fs + frac, ms1 / 1000.0 * fs + frac);
            let (mut lo, mut hi) = (sample(l0), sample(l0));
            let mut k = l0.ceil();
            while k < l1 {
                let v = sample(k);
                lo = lo.min(v);
                hi = hi.max(v);
                k += 1.0;
            }
            let x = r.x0 + r.width() * c as f64 / cols as f64;
            if self.ir_db {
                // The envelope: the column's largest swing, dB under the peak, filled up from
                // the floor.
                let a = lo.abs().max(hi.abs()) / peak;
                let db = 20.0 * a.max(1e-9).log10();
                top.push(Point::new(x, db_y(db)));
                bottom.push(Point::new(x, r.y1));
            } else {
                top.push(Point::new(x, mid - hi / peak * amp));
                bottom.push(Point::new(x, mid - lo / peak * amp));
            }
        }
        let mut shape = BezPath::new();
        shape.move_to(top[0]);
        for p in &top[1..] {
            shape.line_to(*p);
        }
        for p in bottom.iter().rev() {
            shape.line_to(*p);
        }
        shape.close_path();
        cx.begin_clip(&r);
        cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.accent.multiply_alpha(0.35), None, &shape);
        cx.scene.stroke(&Stroke::new(1.2), Affine::IDENTITY, theme.accent, None, &shape);
        cx.end_clip();
    }

    /// The help strip's reading where the pointer is.
    pub fn help(&self, p: Point) -> Option<String> {
        if IR_PLOT.contains(p) {
            let (pre, post) = self.ir_span;
            let ms = -pre + (p.x - IR_PLOT.x0) / IR_PLOT.width() * (pre + post);
            return Some(format!("{} ms after the chain delay — wheel: zoom, Alt+wheel or drag: scroll, double-click: ±10 ms", fmt_signed(ms, 2)));
        }
        if !((X0..=X1).contains(&p.x) && (MAG.y0..=GD.y1).contains(&p.y)) {
            return None;
        }
        let f = x_freq(p.x);
        if self.status != Status::Measuring {
            return Some(format!("{} Hz", fmt_hz(f)));
        }
        let i = self.index(f);
        let h = self.eased_h[i];
        let coh = self.points[i].coherence;
        let db = 20.0 * h.norm().max(1e-6).log10();
        let hint = if GD.contains(p) { " — wheel: zoom, Alt+wheel or drag: scroll, double-click: as it was" } else { "" };
        Some(format!("{} Hz · phase {}° · group delay {} ms · {} dB · coherence {:.2}{hint}", fmt_hz(f), fmt_signed(h.arg().to_degrees(), 1), format!("{:.2}", self.eased_gd[i] * 1000.0).replace('-', "−"), fmt_signed(db, 1), coh))
    }

    /// The group-delay plot's range now, ms (tests and tools).
    pub fn gd_range(&self) -> (f64, f64) {
        self.gd_range
    }
}
