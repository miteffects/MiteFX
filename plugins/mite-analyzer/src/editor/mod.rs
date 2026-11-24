//! The Mite Analyzer editor (docs/design/analyzer-brief.md §2), in 1280×760 design units.
//!
//! Left, on one frequency axis: the spectrum (eased, with a peak-hold line; the sum, mid and side
//! or left and right) and under it the spectrogram (newest at the top). Right: LOUDNESS (the
//! integrated reading against the target, momentary, short-term, range, true peak, PLR, and the
//! last minute of short-term loudness) and STEREO (the vectorscope, correlation, balance, and
//! correlation per band). The footer picks the target, the view, the tilt and the speed; HOLD
//! keeps the peak line; RESET restarts the integration (or click the integrated reading).
//!
//! The footer's first control switches to the PHASE page ([`phase`]): a chain's magnitude,
//! phase and group delay, its delay, polarity and impulse response, measured against a reference
//! (another Mite Analyzer before the chain, or the sidechain).

pub mod phase;

use std::collections::VecDeque;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Instant;

use mite_core::{ParamDisplay, ParamScale, ParamSpec};
use mite_dsp::analyzer::{spectrum_point_freq, tilt_db, Analyzer, LogSpectrum, SPECTRUM_POINTS};
use mite_dsp::loudness::SILENCE_LUFS;
use mite_ui::kurbo::{Affine, BezPath, Circle, Line, Point, Rect, RoundedRect, Stroke};
use mite_ui::paint::{glow_stroke, panel, raised};
use mite_ui::peniko::{Blob, Color, Fill, ImageAlphaType, ImageBrush, ImageData, ImageFormat, ImageQuality};
use mite_ui::session::{Session, SessionConfig};
use mite_ui::widgets::brand::{draw_mark, draw_title};
use mite_ui::widgets::footer::draw_dropdown_button;
use mite_ui::widgets::history::smooth_path;
use mite_ui::widgets::{Dropdown, DropdownEvent, Footer, Help};
use mite_ui::{Align, DrawCx, MouseButton, ParamHost, RecordingHost, TextStyle, UiEvent, View};
use realfft::num_complex::Complex;
use realfft::{RealFftPlanner, RealToComplex};

use crate::dsp::{record, HISTORY_SLICE_MS};
use crate::params::*;
use crate::presets::{preset_ids, FACTORY, PLUGIN};
use crate::AnalyzerShared;
use phase::{PhaseLab, SOURCE_ITEMS};

/// Mite Analyzer wears its accent (tokens.json `color.accent.analyzer`, pewter).
pub const ACCENT: &str = "analyzer";

const VIEW_W: f64 = 1520.0;
const VIEW_H: f64 = 900.0;
const HEADER: f64 = 56.0;
const DISPLAY_TOP: f64 = HEADER;
const FOOTER_TOP: f64 = VIEW_H - 56.0;
const HELP_TOP: f64 = FOOTER_TOP - 28.0;

/// The spectrum and the spectrogram share the frequency axis, 20 Hz…20 kHz.
const SPEC: Rect = Rect::new(0.0, DISPLAY_TOP, 1000.0, DISPLAY_TOP + 430.0);
const X0: f64 = 56.0;
const X1: f64 = 980.0;
const GRAM: Rect = Rect::new(X0, SPEC.y1 + 8.0, X1, HELP_TOP - 8.0);
const TOP_DB: f64 = -6.0;
const BOTTOM_DB: f64 = -96.0;
/// The spectrogram: columns across, rows (one every `GRAM_ROW_S`) down.
const GRAM_COLS: usize = 462;
const GRAM_ROWS: usize = 240;
const GRAM_ROW_S: f64 = 1.0 / 30.0;
/// Silence (replaced by the theme's well), then a magma-like heat scale.
const GRAM_PALETTE: [[u8; 4]; 6] = [[20, 22, 26, 255], [52, 16, 92, 255], [140, 34, 116, 255], [222, 73, 82, 255], [252, 160, 70, 255], [252, 246, 190, 255]];

/// The right column's panels.
const RIGHT: (f64, f64) = (1010.0, VIEW_W - 12.0);
const LOUD: Rect = Rect::new(RIGHT.0, DISPLAY_TOP + 8.0, RIGHT.1, DISPLAY_TOP + 414.0);
const INTEGRATED: Rect = Rect::new(LOUD.x0 + 16.0, LOUD.y0 + 30.0, LOUD.x0 + 270.0, LOUD.y0 + 126.0);
/// The integrated reading's right edge (its digits stay put as it changes).
const INTEGRATED_X: f64 = LOUD.x0 + 250.0;
const LOUD_GRAPH: Rect = Rect::new(LOUD.x0 + 16.0, LOUD.y0 + 204.0, LOUD.x1 - 16.0, LOUD.y1 - 16.0);
const LOUD_TOP: f64 = -3.0;
const LOUD_BOTTOM: f64 = -45.0;
const LOUD_SECONDS: f64 = 60.0;
const STEREO: Rect = Rect::new(RIGHT.0, LOUD.y1 + 8.0, RIGHT.1, HELP_TOP - 8.0);
const SCOPE: Rect = Rect::new(STEREO.x0 + 16.0, STEREO.y0 + 36.0, STEREO.x0 + 256.0, STEREO.y0 + 276.0);
const CORR: Rect = Rect::new(SCOPE.x0, SCOPE.y1 + 12.0, SCOPE.x1, SCOPE.y1 + 26.0);
const BANDS: Rect = Rect::new(SCOPE.x1 + 24.0, SCOPE.y0, STEREO.x1 - 16.0, CORR.y1);
/// The correlation bands, Hz.
const BAND_EDGES: [f64; 6] = [20.0, 120.0, 500.0, 2000.0, 6000.0, 20_000.0];
const BAND_NAMES: [&str; 5] = ["LOW", "LOW MID", "MID", "HIGH MID", "HIGH"];

/// The footer's controls.
/// The page switch: SPECTRUM | PHASE.
const PAGE_BUTTON: Rect = Rect::new(16.0, FOOTER_TOP + 9.0, 236.0, FOOTER_TOP + 47.0);
const TARGET_BUTTON: Rect = Rect::new(256.0, FOOTER_TOP + 9.0, 476.0, FOOTER_TOP + 47.0);
const VIEW_BUTTON: Rect = Rect::new(486.0, FOOTER_TOP + 9.0, 656.0, FOOTER_TOP + 47.0);
const TILT_BUTTON: Rect = Rect::new(666.0, FOOTER_TOP + 9.0, 816.0, FOOTER_TOP + 47.0);
const SPEED_BUTTON: Rect = Rect::new(826.0, FOOTER_TOP + 9.0, 966.0, FOOTER_TOP + 47.0);
const HOLD_BUTTON: Rect = Rect::new(976.0, FOOTER_TOP + 9.0, 1076.0, FOOTER_TOP + 47.0);
const SMOOTH_BUTTON: Rect = Rect::new(256.0, FOOTER_TOP + 9.0, 416.0, FOOTER_TOP + 47.0);
const AVERAGE_BUTTON: Rect = Rect::new(426.0, FOOTER_TOP + 9.0, 566.0, FOOTER_TOP + 47.0);
const RESET_BUTTON: Rect = Rect::new(VIEW_W - 116.0, FOOTER_TOP + 9.0, VIEW_W - 16.0, FOOTER_TOP + 47.0);

/// The readings change at most this often, s: a number that changes every frame cannot be read.
const READOUT_S: f64 = 0.25;
/// Text sizes: labels, small values, the readings, the integrated reading.
const LABEL: TextStyle = TextStyle { size: 12.0, weight: 700.0, tracking: 0.08, tabular: false };
const SMALL: TextStyle = TextStyle { size: 11.0, weight: 700.0, tracking: 0.08, tabular: false };
const AXIS: TextStyle = TextStyle { size: 12.0, weight: 600.0, tracking: 0.0, tabular: true };
const VALUE: TextStyle = TextStyle { size: 24.0, weight: 700.0, tracking: 0.0, tabular: true };
const ROW_VALUE: TextStyle = TextStyle { size: 15.0, weight: 700.0, tracking: 0.0, tabular: true };
const BIG: TextStyle = TextStyle { size: 64.0, weight: 700.0, tracking: 0.0, tabular: true };

/// The FFTs: the spectrum's, and the bands' (left and right, complex).
const FFT: usize = 8192;
const BAND_FFT: usize = 4096;
/// The scope's samples (the newest).
const SCOPE_SAMPLES: usize = 2048;

fn freq_x(f: f64) -> f64 {
    X0 + (X1 - X0) * (f / 20.0).ln() / 1000f64.ln()
}

fn x_freq(x: f64) -> f64 {
    20.0 * 1000f64.powf((x - X0) / (X1 - X0))
}

fn db_y(db: f64) -> f64 {
    SPEC.y0 + 16.0 + (TOP_DB - db.clamp(BOTTOM_DB, TOP_DB)) / (TOP_DB - BOTTOM_DB) * (SPEC.height() - 40.0)
}

fn lufs_y(l: f64) -> f64 {
    LOUD_GRAPH.y0 + 6.0 + (LOUD_TOP - l.clamp(LOUD_BOTTOM, LOUD_TOP)) / (LOUD_TOP - LOUD_BOTTOM) * (LOUD_GRAPH.height() - 12.0)
}

/// A frequency's note, e.g. "A4 +3".
fn note_name(hz: f64) -> String {
    const NAMES: [&str; 12] = ["C", "C♯", "D", "D♯", "E", "F", "F♯", "G", "G♯", "A", "A♯", "B"];
    let midi = 69.0 + 12.0 * (hz / 440.0).log2();
    let n = midi.round();
    let cents = ((midi - n) * 100.0).round() as i32;
    let ni = n as i32;
    format!("{}{} {:+}", NAMES[ni.rem_euclid(12) as usize], ni.div_euclid(12) - 1, cents)
}

fn fmt_lufs(v: f32) -> String {
    if v <= SILENCE_LUFS + 1.0 {
        "—".to_owned()
    } else {
        format!("{:.1}", v).replace('-', "−")
    }
}

/// A stand-in host holding every Mite Analyzer parameter at its default (tests, preview, tools).
pub fn memory_host() -> mite_ui::MemoryParamHost {
    fn leak(s: String) -> &'static str {
        Box::leak(s.into_boxed_str())
    }
    let index = |id: &'static str, count: usize, default: usize| ParamSpec {
        id,
        name: id,
        unit: "",
        min: 0.0,
        max: (count - 1) as f32,
        default: default as f32,
        scale: ParamScale::Linear,
        smoothing_ms: 0.0,
        decimals: 0,
        wheel_step: 1.0,
        wheel_step_fine: 1.0,
        display: ParamDisplay::Number,
    };
    let mut host = mite_ui::MemoryParamHost::new();
    for line in manifest().lines() {
        let f: Vec<&str> = line.split(" | ").collect();
        let id = leak(f[0].to_owned());
        let spec = match f[1] {
            "bool" => index(id, 2, usize::from(f[2] == "true")),
            _ => index(id, f[2].split(", ").count(), f[3].parse().unwrap_or(0)),
        };
        host = host.with(spec);
    }
    host
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Menu {
    Target,
    View,
    Tilt,
    Speed,
    Smooth,
    Average,
    /// The phase page's reference (not a parameter: instances come and go).
    Source,
}

impl Menu {
    fn spec(self) -> ChoiceSpecRef {
        match self {
            Self::Target => &TARGET,
            Self::View => &VIEW,
            Self::Tilt => &TILT,
            Self::Speed => &SPEED,
            Self::Smooth => &SMOOTH,
            Self::Average | Self::Source => &AVERAGE,
        }
    }

    fn items(self) -> &'static [&'static str] {
        match self {
            Self::Source => SOURCE_ITEMS,
            m => m.spec().choices,
        }
    }

    fn button(self) -> Rect {
        match self {
            Self::Target => TARGET_BUTTON,
            Self::View => VIEW_BUTTON,
            Self::Tilt => TILT_BUTTON,
            Self::Speed => SPEED_BUTTON,
            Self::Smooth => SMOOTH_BUTTON,
            Self::Average => AVERAGE_BUTTON,
            Self::Source => phase::SOURCE_BUTTON,
        }
    }

    const SPECTRUM: &'static [Menu] = &[Menu::Target, Menu::View, Menu::Tilt, Menu::Speed];
    /// The footer's menus on the phase page (the source sits in the chain panel).
    const PHASE: &'static [Menu] = &[Menu::Smooth, Menu::Average, Menu::Source];
}

type ChoiceSpecRef = &'static mite_core::ChoiceSpec;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Hot {
    Menu(Menu),
    /// The page switch's half: 0 spectrum, 1 phase.
    Page(usize),
    PhasePlots,
    Chain,
    Impulse,
    Hold,
    Reset,
    Integrated,
    Spectrum,
    Gram,
    Loudness,
    Scope,
    Bands,
}

/// The per-band correlation's FFT (left and right).
struct BandFft {
    fft: Arc<dyn RealToComplex<f64>>,
    window: Vec<f64>,
    buf: Vec<f64>,
    spec: [Vec<Complex<f64>>; 2],
    scratch: Vec<Complex<f64>>,
}

impl BandFft {
    fn new() -> Self {
        let fft = RealFftPlanner::<f64>::new().plan_fft_forward(BAND_FFT);
        let window = (0..BAND_FFT).map(|i| 0.5 - 0.5 * (std::f64::consts::TAU * i as f64 / BAND_FFT as f64).cos()).collect();
        Self { spec: [fft.make_output_vec(), fft.make_output_vec()], scratch: fft.make_scratch_vec(), buf: vec![0.0; BAND_FFT], window, fft }
    }

    /// Each band's correlation (Σ Re L·R̄ / √(Σ|L|² Σ|R|²)) and power, from the newest samples.
    fn bands(&mut self, l: &[f32], r: &[f32], fs: f64) -> [(f64, f64); 5] {
        for (c, x) in [l, r].into_iter().enumerate() {
            let x = &x[x.len() - BAND_FFT..];
            for ((b, &v), w) in self.buf.iter_mut().zip(x).zip(&self.window) {
                *b = f64::from(v) * w;
            }
            let _ = self.fft.process_with_scratch(&mut self.buf, &mut self.spec[c], &mut self.scratch);
        }
        let hz = fs / BAND_FFT as f64;
        std::array::from_fn(|b| {
            let (lo, hi) = (((BAND_EDGES[b] / hz).ceil() as usize).max(1), ((BAND_EDGES[b + 1] / hz).floor() as usize).min(self.spec[0].len() - 1));
            let (mut ll, mut rr, mut lr) = (0.0, 0.0, 0.0);
            for k in lo..=hi.max(lo) {
                let (a, c) = (self.spec[0][k], self.spec[1][k]);
                ll += a.norm_sqr();
                rr += c.norm_sqr();
                lr += (a * c.conj()).re;
            }
            (if ll * rr > 1e-18 { lr / (ll * rr).sqrt() } else { 0.0 }, ll + rr)
        })
    }
}

/// The numbers as shown.
#[derive(Debug, Clone, Copy)]
struct Shown {
    momentary: f32,
    short_term: f32,
    integrated: f32,
    range: f32,
    true_peak: f32,
    correlation: f64,
    balance: f64,
    bands: [(f64, f64); 5],
}

impl Default for Shown {
    fn default() -> Self {
        Self { momentary: SILENCE_LUFS, short_term: SILENCE_LUFS, integrated: SILENCE_LUFS, range: 0.0, true_peak: -150.0, correlation: 0.0, balance: 0.0, bands: [(0.0, 0.0); 5] }
    }
}

pub struct AnalyzerView {
    shared: Arc<AnalyzerShared>,
    session: Session,
    footer: Footer,
    analyzer: Analyzer,
    log: Option<LogSpectrum>,
    bands_fft: BandFft,
    /// The newest samples, left and right; the spectrum's input and its power.
    l: Vec<f32>,
    r: Vec<f32>,
    work: Vec<f32>,
    power: Vec<f32>,
    raw: [Vec<f32>; 2],
    /// The curves as drawn (eased), the peak hold, and the spectrogram's last row.
    curves: [Vec<f32>; 2],
    hold: Vec<f32>,
    gram: Vec<u8>,
    gram_rows: usize,
    gram_due: f64,
    /// The spectrogram's palette, silence to the loudest (dark violet, magenta, orange, yellow,
    /// white: a heat scale reads levels where one hue does not).
    gram_colors: [[u8; 4]; 6],
    /// Loudness: the last minute of records, the history's read position.
    history: VecDeque<[f32; record::WIDTH]>,
    seen: usize,
    /// Stereo: the scope's points and gain, correlation, balance, the bands.
    scope: Vec<(f32, f32)>,
    scope_gain: f64,
    correlation: f64,
    balance: f64,
    bands: [(f64, f64); 5],
    /// The numbers as shown (sampled every `READOUT_S`) and when they were last taken.
    shown: Shown,
    shown_at: Instant,
    menu: Option<(Menu, Dropdown)>,
    /// The phase page is open (from `page`, as of the last tick or draw).
    phase_open: bool,
    /// The phase page's measurement.
    phase: PhaseLab,
    pointer: Option<Point>,
    hot: Option<Hot>,
    last_tick: Instant,
}

impl AnalyzerView {
    pub fn new(shared: Arc<AnalyzerShared>) -> Self {
        Self {
            session: Session::new(SessionConfig { plugin: PLUGIN, factory: FACTORY, sound_ids: preset_ids(), snapshot_ids: preset_ids() }, shared.snapshots.clone(), shared.preset.clone()),
            footer: Footer::new(FOOTER_TOP, (VIEW_W, VIEW_H), vec![], vec![]),
            analyzer: Analyzer::new(FFT),
            log: None,
            bands_fft: BandFft::new(),
            l: vec![0.0; FFT],
            r: vec![0.0; FFT],
            work: vec![0.0; FFT],
            power: vec![0.0; FFT / 2 + 1],
            raw: [vec![BOTTOM_DB as f32; SPECTRUM_POINTS], vec![BOTTOM_DB as f32; SPECTRUM_POINTS]],
            curves: [vec![BOTTOM_DB as f32; SPECTRUM_POINTS], vec![BOTTOM_DB as f32; SPECTRUM_POINTS]],
            hold: vec![BOTTOM_DB as f32; SPECTRUM_POINTS],
            gram: vec![0; GRAM_COLS * GRAM_ROWS * 4],
            gram_rows: 0,
            gram_due: 0.0,
            gram_colors: GRAM_PALETTE,
            history: VecDeque::with_capacity(1024),
            // Everything the history still holds: an editor opened late shows the last minute.
            seen: 0,
            scope: Vec::with_capacity(SCOPE_SAMPLES),
            scope_gain: 1.0,
            correlation: 0.0,
            balance: 0.0,
            bands: [(0.0, 0.0); 5],
            shown: Shown::default(),
            shown_at: Instant::now(),
            menu: None,
            phase_open: false,
            phase: PhaseLab::new(f64::from(shared.sample_rate.load())),
            pointer: None,
            hot: None,
            last_tick: Instant::now(),
            shared,
        }
    }

    /// Keeps the user's presets in `dir` instead of the settings folder (tests).
    pub fn with_preset_dir(mut self, dir: std::path::PathBuf) -> Self {
        self.session = self.session.with_preset_dir(dir);
        self
    }

    /// Lets everything land at once (tools render a single frame): the curves at their targets,
    /// the spectrogram filled with rows of now.
    pub fn settle(&mut self, host: &dyn ParamHost) {
        if Self::phase_page(host) {
            self.phase.settle(&self.shared, host);
        }
        self.analyze(host, 1.0);
        for c in 0..2 {
            self.curves[c].copy_from_slice(&self.raw[c]);
        }
        self.hold.copy_from_slice(&self.raw[0]);
        for _ in 0..GRAM_ROWS {
            self.push_gram_row(host);
        }
        self.take_readings();
    }

    /// The numbers, now (they then hold for `READOUT_S`).
    fn take_readings(&mut self) {
        let tel = &self.shared.tel;
        self.shown = Shown {
            momentary: tel.momentary.load(),
            short_term: tel.short_term.load(),
            integrated: tel.integrated.load(),
            range: tel.range.load(),
            true_peak: tel.true_peak.load(),
            correlation: self.correlation,
            balance: self.balance,
            bands: self.bands,
        };
        self.phase.take_readings();
        self.shown_at = Instant::now();
    }

    // ---- for tests and tools ----------------------------------------------------------------

    pub fn button(&self, name: &str) -> Option<Rect> {
        Some(match name {
            "spectrum_page" => Rect::new(PAGE_BUTTON.x0, PAGE_BUTTON.y0, PAGE_BUTTON.center().x, PAGE_BUTTON.y1),
            "phase_page" => Rect::new(PAGE_BUTTON.center().x, PAGE_BUTTON.y0, PAGE_BUTTON.x1, PAGE_BUTTON.y1),
            "smooth" => SMOOTH_BUTTON,
            "average" => AVERAGE_BUTTON,
            "source" => phase::SOURCE_BUTTON,
            "target" => TARGET_BUTTON,
            "view" => VIEW_BUTTON,
            "tilt" => TILT_BUTTON,
            "speed" => SPEED_BUTTON,
            "hold" => HOLD_BUTTON,
            "reset" => RESET_BUTTON,
            "integrated" => INTEGRATED,
            _ => return None,
        })
    }

    pub fn menu_rows(&self) -> Vec<Rect> {
        self.menu.as_ref().map(|(_, m)| m.item_rects()).unwrap_or_default()
    }

    /// The correlation per band as read (low to high).
    pub fn band_correlation(&self) -> [f64; 5] {
        self.bands.map(|b| b.0)
    }

    pub fn correlation(&self) -> f64 {
        self.correlation
    }

    /// The phase page's measurement.
    pub fn phase(&self) -> &PhaseLab {
        &self.phase
    }

    fn phase_page(host: &dyn ParamHost) -> bool {
        Self::index(host, PAGE.id) == 1
    }

    fn menus(host: &dyn ParamHost) -> &'static [Menu] {
        if Self::phase_page(host) {
            Menu::PHASE
        } else {
            Menu::SPECTRUM
        }
    }

    // ---- reading ----------------------------------------------------------------------------

    fn plain(host: &dyn ParamHost, id: &str) -> f32 {
        host.plain(id, host.normalized(id))
    }

    fn index(host: &dyn ParamHost, id: &str) -> usize {
        Self::plain(host, id).round().max(0.0) as usize
    }

    fn fs(&self) -> f64 {
        f64::from(self.shared.sample_rate.load()).max(8000.0)
    }

    /// The newest samples through the FFT: both curves (by the view) at the spectrum points, and
    /// the stereo readings.
    fn analyze(&mut self, host: &dyn ParamHost, ease: f64) {
        let fs = self.fs();
        if self.log.as_ref().is_none_or(|l| l.sample_rate() != fs) {
            self.log = Some(LogSpectrum::new(FFT, fs));
        }
        self.shared.tel.left.read_latest(&mut self.l);
        self.shared.tel.right.read_latest(&mut self.r);
        let view = Self::index(host, VIEW.id);
        let slope = TILT_DB_OCT[Self::index(host, TILT.id).min(2)];
        for c in 0..2 {
            for ((w, &a), &b) in self.work.iter_mut().zip(&self.l).zip(&self.r) {
                *w = match (view, c) {
                    (1, 0) | (0, _) => 0.5 * (a + b),
                    (1, _) => 0.5 * (a - b),
                    (_, 0) => a,
                    _ => b,
                };
            }
            if view == 0 && c == 1 {
                break;
            }
            self.analyzer.analyze_power(&self.work, &mut self.power);
            if let Some(log) = &self.log {
                log.read(&self.power, &mut self.raw[c]);
            }
            // The analyzer's own 4.5 dB/oct out, the chosen tilt in.
            for (i, v) in self.raw[c].iter_mut().enumerate() {
                let f = spectrum_point_freq(i);
                *v = (f64::from(*v) - tilt_db(f) + slope * (f / 1000.0).log2()) as f32;
            }
        }
        // Stereo: the scope's points, correlation and balance over the newest samples.
        let n = SCOPE_SAMPLES;
        let (l, r) = (&self.l[FFT - n..], &self.r[FFT - n..]);
        let (mut ll, mut rr, mut lr, mut peak) = (0.0f64, 0.0f64, 0.0f64, 0.0f32);
        self.scope.clear();
        for (&a, &b) in l.iter().zip(r) {
            ll += f64::from(a) * f64::from(a);
            rr += f64::from(b) * f64::from(b);
            lr += f64::from(a) * f64::from(b);
            peak = peak.max(a.abs()).max(b.abs());
            self.scope.push(((b - a) * std::f32::consts::FRAC_1_SQRT_2, (a + b) * std::f32::consts::FRAC_1_SQRT_2));
        }
        let corr = if ll * rr > 1e-14 { lr / (ll * rr).sqrt() } else { 0.0 };
        let bal = if ll + rr > 1e-14 { 10.0 * ((rr + 1e-20) / (ll + 1e-20)).log10() } else { 0.0 };
        let target_gain = if peak > 1e-5 { (0.9 / f64::from(peak)).min(30.0) } else { self.scope_gain };
        let bands = self.bands_fft.bands(&self.l, &self.r, fs);
        // Eased (about 300 ms): readable, not jumpy.
        let k = ease;
        self.correlation += (corr - self.correlation) * k;
        self.balance += (bal.clamp(-24.0, 24.0) - self.balance) * k;
        self.scope_gain += (target_gain - self.scope_gain) * if target_gain < self.scope_gain { 0.5_f64.max(ease) } else { 0.05_f64.max(ease) };
        for (b, n) in self.bands.iter_mut().zip(bands) {
            b.0 += (n.0 - b.0) * k;
            b.1 += (n.1 - b.1) * k;
        }
    }

    /// One spectrogram row: the newest spectrum (the first curve, not eased) in colour, pushed in
    /// at the top.
    fn push_gram_row(&mut self, host: &dyn ParamHost) {
        let _ = host;
        let row = GRAM_COLS * 4;
        self.gram.copy_within(0..(GRAM_ROWS - 1) * row, row);
        let stops = self.gram_colors;
        for c in 0..GRAM_COLS {
            let x = GRAM.x0 + (c as f64 + 0.5) / GRAM_COLS as f64 * GRAM.width();
            let db = sample_curve(&self.raw[0], x_freq(x));
            // The lower 6 dB of the scale is silence; the rest runs through the palette.
            let t = ((f64::from(db) - BOTTOM_DB - 6.0) / (TOP_DB - BOTTOM_DB - 6.0)).clamp(0.0, 1.0) as f32 * (stops.len() - 1) as f32;
            let i = (t.floor() as usize).min(stops.len() - 2);
            let u = t - i as f32;
            let (a, b) = (stops[i], stops[i + 1]);
            for k in 0..4 {
                self.gram[c * 4 + k] = (f32::from(a[k]) + (f32::from(b[k]) - f32::from(a[k])) * u).round() as u8;
            }
        }
        self.gram_rows = (self.gram_rows + 1).min(GRAM_ROWS);
    }

    // ---- drawing ----------------------------------------------------------------------------

    fn draw_header(&mut self, cx: &mut DrawCx<'_>) {
        let theme = cx.theme;
        cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.window, None, &Rect::new(0.0, 0.0, VIEW_W, HEADER));
        cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, theme.line, None, &Line::new((0.0, HEADER - 0.5), (VIEW_W, HEADER - 0.5)));
        draw_mark(cx, 16.0, 15.0, 26.0);
        draw_title(cx, 52.0, 18.0, "Analyzer");
        // No preset header: a measuring tool has nothing to recall (user, 2026-11-24).
    }

    fn draw_spectrum(&self, cx: &mut DrawCx<'_>) {
        let theme = cx.theme;
        let label = AXIS;
        for f in [20.0, 50.0, 100.0, 200.0, 500.0, 1000.0, 2000.0, 5000.0, 10_000.0] {
            let x = freq_x(f);
            cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, theme.grid_minor, None, &Line::new((x, SPEC.y0 + 8.0), (x, SPEC.y1 - 22.0)));
            let t = if f >= 1000.0 { format!("{}k", f / 1000.0) } else { format!("{f}") };
            cx.text.draw(cx.scene, &t, x, SPEC.y1 - 20.0, label, theme.text_lo, Align::Center);
        }
        for db in [-12.0, -24.0, -36.0, -48.0, -60.0, -72.0, -84.0] {
            let y = db_y(db);
            cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, theme.grid_minor, None, &Line::new((X0, y), (X1, y)));
            cx.text.draw(cx.scene, &format!("{}", db as i32).replace('-', "−"), X0 - 8.0, y - 8.0, label, theme.text_lo, Align::Right);
        }
        let view = Self::index(cx.host, VIEW.id);
        let pts = |v: &[f32]| -> Vec<Point> {
            (0..SPECTRUM_POINTS)
                .filter_map(|i| {
                    let f = spectrum_point_freq(i);
                    (16.0..=22_000.0).contains(&f).then(|| Point::new(freq_x(f), db_y(f64::from(v[i]))))
                })
                .collect()
        };
        cx.begin_clip(&Rect::new(X0, SPEC.y0 + 8.0, X1, db_y(BOTTOM_DB)));
        // The second curve (side, or right) dim, under.
        if view > 0 {
            let p = pts(&self.curves[1]);
            let line = smooth_path(&p);
            let color = if view == 1 { theme.spectrum_pre } else { theme.curve };
            cx.scene.stroke(&Stroke::new(1.4), Affine::IDENTITY, color.multiply_alpha(0.8), None, &line);
        }
        let p = pts(&self.curves[0]);
        let line = smooth_path(&p);
        let mut fill = line.clone();
        if let (Some(first), Some(last)) = (p.first(), p.last()) {
            fill.line_to((last.x, db_y(BOTTOM_DB)));
            fill.line_to((first.x, db_y(BOTTOM_DB)));
            fill.close_path();
        }
        cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.accent.multiply_alpha(0.12), None, &fill);
        glow_stroke(cx, &line, 1.8, theme.accent, theme.accent, 2.4);
        if Self::plain(cx.host, HOLD.id) >= 0.5 {
            let h = smooth_path(&pts(&self.hold));
            cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, theme.text_mid.multiply_alpha(0.7), None, &h);
        }
        cx.end_clip();
        // Hover: where the pointer is, the frequency, its note and the level there.
        if let Some(p) = self.pointer.filter(|p| SPEC.contains(*p) && (X0..=X1).contains(&p.x)) {
            let f = x_freq(p.x);
            let db = sample_curve(&self.curves[0], f);
            cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, theme.text_mid.multiply_alpha(0.5), None, &Line::new((p.x, SPEC.y0 + 8.0), (p.x, db_y(BOTTOM_DB))));
            let y = db_y(f64::from(db));
            cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.text_strong, None, &Circle::new((p.x, y), 3.0));
        }
        let names: [&str; 3] = ["L + R", "MID (lit) · SIDE", "LEFT (lit) · RIGHT"];
        cx.text.draw(cx.scene, names[view.min(2)], X1, SPEC.y0 + 12.0, LABEL, theme.text_lo, Align::Right);
    }

    fn draw_gram(&self, cx: &mut DrawCx<'_>) {
        let theme = cx.theme;
        cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.well, None, &GRAM);
        if self.gram_rows > 0 {
            let image = ImageData { data: Blob::new(Arc::new(self.gram.clone())), format: ImageFormat::Rgba8, alpha_type: ImageAlphaType::Alpha, width: GRAM_COLS as u32, height: GRAM_ROWS as u32 };
            let brush = ImageBrush::new(image).with_quality(ImageQuality::Medium);
            let k = (GRAM.width() / GRAM_COLS as f64, GRAM.height() / GRAM_ROWS as f64);
            let transform = Affine::translate((GRAM.x0, GRAM.y0)) * Affine::scale_non_uniform(k.0, k.1);
            cx.scene.fill(Fill::NonZero, Affine::IDENTITY, &brush, Some(transform), &GRAM);
        }
        let label = AXIS;
        let secs = GRAM_ROWS as f64 * GRAM_ROW_S;
        cx.text.draw(cx.scene, "now", GRAM.x0 - 6.0, GRAM.y0, label, theme.text_lo, Align::Right);
        cx.text.draw(cx.scene, &format!("−{secs:.0} s"), GRAM.x0 - 6.0, GRAM.y1 - 12.0, label, theme.text_lo, Align::Right);
    }

    fn draw_loudness(&self, cx: &mut DrawCx<'_>) {
        let theme = cx.theme;
        let n = &self.shown;
        panel(cx.scene, RoundedRect::from_rect(LOUD, 8.0), theme.display, theme.line);
        cx.text.draw(cx.scene, "LOUDNESS", LOUD.x0 + 16.0, LOUD.y0 + 12.0, LABEL, theme.text_lo, Align::Left);
        let target_i = Self::index(cx.host, TARGET.id).min(TARGET_LUFS.len() - 1);
        let target = TARGET_LUFS[target_i];
        // The integrated reading, large, right-aligned so its digits stay put.
        let hot = self.hot == Some(Hot::Integrated);
        cx.text.draw(cx.scene, &fmt_lufs(n.integrated), INTEGRATED_X, INTEGRATED.y0, BIG, if hot { theme.accent } else { theme.text_strong }, Align::Right);
        cx.text.draw(cx.scene, "LUFS INTEGRATED", INTEGRATED.x0 + 4.0, INTEGRATED.y1 - 18.0, LABEL, theme.text_lo, Align::Left);
        if n.integrated > SILENCE_LUFS + 1.0 {
            let d = n.integrated - target;
            let color = if d.abs() <= 1.0 { theme.gain_up } else if d > 0.0 { theme.warn } else { theme.text_mid };
            cx.text.draw(cx.scene, &format!("{:+.1} LU", d).replace('-', "−"), LOUD.x1 - 18.0, INTEGRATED.y0 + 14.0, VALUE.sized(30.0), color, Align::Right);
        }
        cx.text.draw(cx.scene, TARGET.choices[target_i], LOUD.x1 - 18.0, INTEGRATED.y0 + 58.0, LABEL, theme.text_lo, Align::Right);
        // The row of readings, each right-aligned in its cell.
        let tp = n.true_peak;
        let plr = if n.integrated > SILENCE_LUFS + 1.0 && tp > -140.0 { format!("{:.1}", tp - n.integrated) } else { "—".to_owned() };
        let tp_text = if tp > -140.0 { format!("{:.1}", tp).replace('-', "−") } else { "—".to_owned() };
        let cells = [("MOMENTARY", fmt_lufs(n.momentary), theme.text_strong), ("SHORT-TERM", fmt_lufs(n.short_term), theme.text_strong), ("RANGE LU", format!("{:.1}", n.range), theme.text_strong), ("TRUE PEAK", tp_text, if tp > -1.0 { theme.danger } else { theme.text_strong }), ("PLR", plr, theme.text_strong)];
        let w = (LOUD.width() - 32.0) / cells.len() as f64;
        for (i, (name, v, color)) in cells.into_iter().enumerate() {
            let x0 = LOUD.x0 + 16.0 + i as f64 * w;
            cx.text.draw(cx.scene, &v, x0 + w - 16.0, LOUD.y0 + 140.0, VALUE, color, Align::Right);
            cx.text.draw(cx.scene, name, x0 + w - 16.0, LOUD.y0 + 174.0, SMALL, theme.text_lo, Align::Right);
        }
        // The last minute: short-term (lit), momentary (dim), the target (dashed).
        cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.well, None, &LOUD_GRAPH);
        for l in [-6.0, -12.0, -18.0, -24.0, -30.0, -36.0, -42.0] {
            let y = lufs_y(l);
            cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, theme.grid_minor, None, &Line::new((LOUD_GRAPH.x0, y), (LOUD_GRAPH.x1, y)));
            cx.text.draw(cx.scene, &format!("{}", l as i32).replace('-', "−"), LOUD_GRAPH.x0 + 6.0, y - 15.0, AXIS, theme.text_lo, Align::Left);
        }
        let ty = lufs_y(f64::from(target));
        cx.scene.stroke(&Stroke::new(1.4).with_dashes(0.0, [6.0, 4.0]), Affine::IDENTITY, theme.gain_up.multiply_alpha(0.8), None, &Line::new((LOUD_GRAPH.x0, ty), (LOUD_GRAPH.x1, ty)));
        let per = (LOUD_SECONDS * 1000.0 / f64::from(HISTORY_SLICE_MS)) as usize;
        let count = self.history.len().min(per);
        if count >= 2 {
            let x = |i: usize| LOUD_GRAPH.x1 - (count - 1 - i) as f64 / per as f64 * LOUD_GRAPH.width();
            let recs: Vec<&[f32; record::WIDTH]> = self.history.iter().skip(self.history.len() - count).collect();
            cx.begin_clip(&LOUD_GRAPH);
            for (field, color, width) in [(record::MOMENTARY, theme.text_mid.multiply_alpha(0.5), 1.0), (record::SHORT_TERM, theme.accent, 2.0)] {
                let mut path = BezPath::new();
                let mut open = false;
                for (i, r) in recs.iter().enumerate() {
                    let v = r[field];
                    if v <= SILENCE_LUFS + 1.0 {
                        open = false;
                        continue;
                    }
                    let p = (x(i), lufs_y(f64::from(v)));
                    if open {
                        path.line_to(p);
                    } else {
                        path.move_to(p);
                        open = true;
                    }
                }
                if field == record::SHORT_TERM {
                    glow_stroke(cx, &path, width, color, color, 2.0);
                } else {
                    cx.scene.stroke(&Stroke::new(width), Affine::IDENTITY, color, None, &path);
                }
            }
            cx.end_clip();
        }
        cx.text.draw(cx.scene, "LAST MINUTE · SHORT-TERM", LOUD_GRAPH.x1 - 6.0, LOUD_GRAPH.y0 + 6.0, SMALL, theme.text_lo, Align::Right);
    }

    fn draw_stereo(&self, cx: &mut DrawCx<'_>) {
        let theme = cx.theme;
        let n = &self.shown;
        panel(cx.scene, RoundedRect::from_rect(STEREO, 8.0), theme.display, theme.line);
        cx.text.draw(cx.scene, "STEREO", STEREO.x0 + 16.0, STEREO.y0 + 12.0, LABEL, theme.text_lo, Align::Left);
        // The vectorscope: mono straight up, left and right on the diagonals.
        cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.well, None, &SCOPE);
        let c = SCOPE.center();
        let r = SCOPE.width() / 2.0 - 8.0;
        for (a, b) in [((c.x, SCOPE.y0 + 4.0), (c.x, SCOPE.y1 - 4.0)), ((SCOPE.x0 + 4.0, c.y), (SCOPE.x1 - 4.0, c.y)), ((c.x - r * 0.707, c.y - r * 0.707), (c.x + r * 0.707, c.y + r * 0.707)), ((c.x + r * 0.707, c.y - r * 0.707), (c.x - r * 0.707, c.y + r * 0.707))] {
            cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, theme.grid_minor, None, &Line::new(a, b));
        }
        cx.text.draw(cx.scene, "L", c.x - r * 0.707 - 4.0, c.y - r * 0.707 - 4.0, SMALL, theme.text_lo, Align::Right);
        cx.text.draw(cx.scene, "R", c.x + r * 0.707 + 4.0, c.y - r * 0.707 - 4.0, SMALL, theme.text_lo, Align::Left);
        cx.text.draw(cx.scene, "M", c.x + 6.0, SCOPE.y0 + 4.0, SMALL, theme.text_lo, Align::Left);
        if self.scope.len() > 1 {
            let g = self.scope_gain * r;
            let mut path = BezPath::new();
            for (i, &(x, y)) in self.scope.iter().enumerate() {
                let p = (c.x + f64::from(x) * g, c.y - f64::from(y) * g);
                if i == 0 {
                    path.move_to(p);
                } else {
                    path.line_to(p);
                }
            }
            cx.begin_clip(&SCOPE);
            cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, theme.accent.multiply_alpha(0.45), None, &path);
            cx.end_clip();
        }
        // Correlation: −1 (out of phase) … +1 (mono). The bar moves live, the number holds.
        bar(cx, CORR, self.correlation, if self.correlation < 0.0 { theme.danger } else { theme.accent });
        cx.text.draw(cx.scene, "CORRELATION", CORR.x0, CORR.y1 + 6.0, SMALL, theme.text_lo, Align::Left);
        cx.text.draw(cx.scene, &format!("{:+.2}", n.correlation).replace('-', "−"), CORR.x1, CORR.y1 + 3.0, ROW_VALUE, if n.correlation < 0.0 { theme.danger } else { theme.text_strong }, Align::Right);
        // Balance and the bands' correlation.
        let rows = BAND_NAMES.len() + 1;
        let h = BANDS.height() / rows as f64;
        for i in 0..rows {
            let y = BANDS.y0 + i as f64 * h;
            let track = Rect::new(BANDS.x0, y + h - 18.0, BANDS.x1, y + h - 6.0);
            if i == 0 {
                cx.text.draw(cx.scene, "BALANCE", BANDS.x0, y + 4.0, SMALL, theme.text_lo, Align::Left);
                let side = if n.balance.abs() < 0.05 { "C".to_owned() } else if n.balance > 0.0 { format!("R {:.1}", n.balance) } else { format!("L {:.1}", -n.balance) };
                cx.text.draw(cx.scene, &side, BANDS.x1, y, ROW_VALUE, theme.text_strong, Align::Right);
                bar(cx, track, (self.balance / 12.0).clamp(-1.0, 1.0), theme.text_mid);
            } else {
                let (live, power_live) = self.bands[i - 1];
                let (corr, power) = n.bands[i - 1];
                cx.text.draw(cx.scene, BAND_NAMES[i - 1], BANDS.x0, y + 4.0, SMALL, theme.text_lo, Align::Left);
                let words = if power < 1e-6 { "—".to_owned() } else { format!("{:+.2}", corr).replace('-', "−") };
                cx.text.draw(cx.scene, &words, BANDS.x1, y, ROW_VALUE, if power >= 1e-6 && corr < 0.0 { theme.danger } else { theme.text_strong }, Align::Right);
                bar(cx, track, if power_live < 1e-6 { 0.0 } else { live }, if live < 0.0 { theme.danger } else { theme.accent });
            }
        }
    }

    fn draw_footer(&self, cx: &mut DrawCx<'_>) {
        let theme = cx.theme;
        // The page switch: two halves, the open page lit.
        let page = Self::index(cx.host, PAGE.id).min(1);
        raised(cx.scene, RoundedRect::from_rect(PAGE_BUTTON, 6.0), theme.panel_control, theme.panel_control_border, theme.shadows);
        for (i, name) in ["SPECTRUM", "PHASE"].into_iter().enumerate() {
            let w = PAGE_BUTTON.width() / 2.0;
            let half = Rect::new(PAGE_BUTTON.x0 + i as f64 * w, PAGE_BUTTON.y0, PAGE_BUTTON.x0 + (i + 1) as f64 * w, PAGE_BUTTON.y1);
            let on = page == i;
            if on {
                cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.accent.multiply_alpha(0.16), None, &RoundedRect::from_rect(half.inset(-3.0), 4.0));
                cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, theme.accent, None, &RoundedRect::from_rect(half.inset(-3.0), 4.0));
            }
            let hot = self.hot == Some(Hot::Page(i));
            let color = if on { theme.accent } else if hot { theme.text_hi } else { theme.text_mid };
            cx.text.draw(cx.scene, name, half.center().x, half.center().y - 7.0, LABEL, color, Align::Center);
        }
        for &m in Self::menus(cx.host).iter().filter(|m| **m != Menu::Source) {
            let i = Self::index(cx.host, m.spec().id).min(m.spec().choices.len() - 1);
            let open = matches!(self.menu, Some((o, _)) if o == m);
            draw_dropdown_button(cx, m.button(), m.spec().choices[i], open, self.hot == Some(Hot::Menu(m)), |_, _| {});
        }
        let spectrum = page == 0;
        let hold: &[(Rect, &str, bool, Hot)] = &[(HOLD_BUTTON, "HOLD", Self::plain(cx.host, HOLD.id) >= 0.5, Hot::Hold)];
        let reset = [(RESET_BUTTON, "RESET", false, Hot::Reset)];
        for &(r, text, on, hot) in (if spectrum { hold } else { &[] }).iter().chain(&reset) {
            let is_hot = self.hot == Some(hot);
            let border = if on || is_hot { theme.accent } else { theme.panel_control_border };
            raised(cx.scene, RoundedRect::from_rect(r, 6.0), theme.panel_control, border, theme.shadows);
            cx.text.draw(cx.scene, text, r.center().x, r.center().y - 7.0, LABEL, if on { theme.accent } else { theme.text_mid }, Align::Center);
        }
    }

    fn help_text(&self) -> Help {
        if let Some((m, list)) = &self.menu {
            let i = self.pointer.and_then(|p| list.item_rects().iter().position(|r| r.contains(p))).map(|k| list.shown[k]);
            return match (m, i) {
                (Menu::Target, Some(i)) => Help::default().text(format!("{} — the integrated loudness is read against {} LUFS", list.items[i], TARGET_LUFS[i.min(TARGET_LUFS.len() - 1)])),
                _ => Help::default().text("Pick one; click elsewhere to close."),
            };
        }
        match self.hot {
            Some(Hot::Page(0)) => Help::default().text("Spectrum — the spectrum, the spectrogram, loudness and the stereo field."),
            Some(Hot::Page(_)) => Help::default().text("Phase — what a chain does: put a Mite Analyzer before it (or feed its input to the sidechain), this one after it."),
            Some(Hot::PhasePlots) => Help::default().text(self.pointer.and_then(|p| self.phase.help(p)).unwrap_or_default()),
            Some(Hot::Chain) => Help::default().text("The chain's delay (its latency, as heard), its polarity, and how well the two signals explain each other across the band; curves fade where they don't."),
            Some(Hot::Impulse) => Help::default().text("The chain's impulse response around its delay: ringing before 0 is a linear-phase filter's pre-ring."),
            Some(Hot::Menu(Menu::Smooth)) => Help::default().text("Smoothing — across frequency: finer shows more detail, wider settles noisy material."),
            Some(Hot::Menu(Menu::Average)) => Help::default().text("Average — how long the measurement remembers: Fast follows changes, Long steadies music."),
            Some(Hot::Menu(Menu::Source)) => Help::default().text("Reference — the signal before the chain: Auto takes the sidechain if it plays, else another Mite Analyzer."),
            Some(Hot::Reset) if self.phase_open => Help::default().text("Reset — start the measurement again."),
            Some(Hot::Spectrum) => {
                if let Some(p) = self.pointer {
                    let f = x_freq(p.x);
                    let db = sample_curve(&self.curves[0], f);
                    return Help::default().text(format!("{} Hz · {} · {:.1} dB", if f >= 1000.0 { format!("{:.2}k", f / 1000.0) } else { format!("{f:.0}") }, note_name(f), db).replace('-', "−"));
                }
                Help::default()
            }
            Some(Hot::Gram) => Help::default().text("The spectrogram: the last eight seconds, newest at the top, the louder the brighter."),
            Some(Hot::Integrated) => Help::default().text("Integrated loudness since the last reset (gated, BS.1770 / EBU R128) — click to start again."),
            Some(Hot::Loudness) => Help::default().text("Momentary (400 ms), short-term (3 s), loudness range, the highest true peak (4× oversampled) and the peak-to-loudness ratio; below, the last minute against the target (dashed)."),
            Some(Hot::Scope) => Help::default().text("The vectorscope: mono stands straight up, wide spreads sideways, out of phase lies flat. Under it, the correlation: +1 mono, 0 wide, below 0 cancels in mono."),
            Some(Hot::Bands) => Help::default().text("Balance, and the correlation per band: the lows should stay near +1 (mono-safe); a band below 0 cancels when summed to mono."),
            Some(Hot::Hold) => Help::default().text("Hold — keep the spectrum's peaks as a thin line (falling slowly)."),
            Some(Hot::Reset) => Help::default().text("Reset — start the integrated loudness, the range and the highest true peak again."),
            Some(Hot::Menu(Menu::Target)) => Help::default().text("Target — the loudness the integrated reading is held against."),
            Some(Hot::Menu(Menu::View)) => Help::default().text("View — the spectrum of the sum, of mid and side, or of left and right."),
            Some(Hot::Menu(Menu::Tilt)) => Help::default().text("Tilt — 4.5 dB/oct reads a balanced mix (and pink noise) level; Flat is the raw spectrum."),
            Some(Hot::Menu(Menu::Speed)) => Help::default().text("Speed — how fast the spectrum follows the music."),
            None if self.phase_open => Help::default().text("A chain's magnitude, phase and group delay, against the reference before it; curves fade where the two signals don't explain each other."),
            None => Help::default().text("Spectrum and spectrogram on the left; loudness and the stereo field on the right."),
        }
    }

    // ---- events -----------------------------------------------------------------------------

    fn busy(&self) -> bool {
        self.menu.is_some() || self.footer.busy()
    }

    fn hot_at(&self, p: Point) -> Option<Hot> {
        if PAGE_BUTTON.contains(p) {
            return Some(Hot::Page(usize::from(p.x >= PAGE_BUTTON.center().x)));
        }
        let menus = if self.phase_open { Menu::PHASE } else { Menu::SPECTRUM };
        for &m in menus {
            if m.button().contains(p) {
                return Some(Hot::Menu(m));
            }
        }
        if RESET_BUTTON.contains(p) {
            return Some(Hot::Reset);
        }
        if self.phase_open {
            if (phase::X0..=phase::X1).contains(&p.x) && (phase::MAG.y0..=phase::GD.y1).contains(&p.y) {
                return Some(Hot::PhasePlots);
            }
            if phase::CHAIN.contains(p) {
                return Some(Hot::Chain);
            }
            if phase::IMPULSE.contains(p) {
                return Some(Hot::Impulse);
            }
            return None;
        }
        if HOLD_BUTTON.contains(p) {
            return Some(Hot::Hold);
        }
        if RESET_BUTTON.contains(p) {
            return Some(Hot::Reset);
        }
        if INTEGRATED.contains(p) {
            return Some(Hot::Integrated);
        }
        if SPEC.contains(p) && (X0..=X1).contains(&p.x) {
            return Some(Hot::Spectrum);
        }
        if GRAM.contains(p) {
            return Some(Hot::Gram);
        }
        if LOUD.contains(p) {
            return Some(Hot::Loudness);
        }
        if SCOPE.contains(p) || CORR.inflate(0.0, 12.0).contains(p) {
            return Some(Hot::Scope);
        }
        if STEREO.contains(p) {
            return Some(Hot::Bands);
        }
        None
    }

    fn gesture(host: &dyn ParamHost, id: &str, v: f32) {
        host.begin_gesture(id);
        host.set_normalized(id, host.normalize(id, v));
        host.end_gesture(id);
    }

    fn handle_event(&mut self, event: &UiEvent, host: &dyn ParamHost) -> bool {
        if let UiEvent::KeyDown { .. } = *event {
            return false;
        }
        if let Some((m, list)) = &mut self.menu {
            if let UiEvent::PointerMove { pos, .. } = *event {
                self.pointer = Some(pos);
            }
            let m = *m;
            match list.event(event) {
                DropdownEvent::Open => {}
                DropdownEvent::Picked(i) if m == Menu::Source => {
                    self.menu = None;
                    self.shared.phase_source.store(i as u32, Ordering::Relaxed);
                }
                DropdownEvent::Picked(i) => {
                    self.menu = None;
                    Self::gesture(host, m.spec().id, i as f32);
                }
                DropdownEvent::Closed => self.menu = None,
            }
            return true;
        }
        if self.footer.event(event, host) {
            return true;
        }
        match *event {
            UiEvent::PointerMove { pos, .. } => {
                self.pointer = Some(pos);
                self.hot = self.hot_at(pos);
                true
            }
            UiEvent::PointerLeft => {
                self.pointer = None;
                self.hot = None;
                true
            }
            UiEvent::PointerDown { pos, button: MouseButton::Left, .. } => match self.hot_at(pos) {
                Some(Hot::Page(i)) => {
                    Self::gesture(host, PAGE.id, i as f32);
                    self.phase_open = i == 1;
                    true
                }
                Some(Hot::Menu(Menu::Source)) => {
                    let i = self.shared.phase_source.load(Ordering::Relaxed) as usize;
                    let shown = PhaseLab::source_choices(&self.shared);
                    let list = Dropdown::new(Menu::Source.button(), SOURCE_ITEMS, i, (DISPLAY_TOP, FOOTER_TOP)).with_shown(shown).with_min_width(Menu::Source.button().width());
                    self.menu = Some((Menu::Source, list));
                    true
                }
                Some(Hot::Menu(m)) => {
                    let i = Self::index(host, m.spec().id);
                    self.menu = Some((m, Dropdown::new(m.button(), m.items(), i, (DISPLAY_TOP, FOOTER_TOP)).with_min_width(m.button().width().max(150.0))));
                    true
                }
                Some(Hot::Reset) if self.phase_open => {
                    self.phase.restart();
                    true
                }
                Some(Hot::Hold) => {
                    Self::gesture(host, HOLD.id, if Self::plain(host, HOLD.id) >= 0.5 { 0.0 } else { 1.0 });
                    true
                }
                Some(Hot::Reset) | Some(Hot::Integrated) => {
                    self.shared.tel.reset.store(true, Ordering::Release);
                    self.hold.fill(BOTTOM_DB as f32);
                    true
                }
                _ => false,
            },
            _ => false,
        }
    }
}

/// A bar from the middle of `r` to `v` (−1…1) of its half width.
fn bar(cx: &mut DrawCx<'_>, r: Rect, v: f64, color: Color) {
    let theme = cx.theme;
    cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.well, None, &RoundedRect::from_rect(r, 3.0));
    let mid = r.center().x;
    cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, theme.grid_major, None, &Line::new((mid, r.y0 - 2.0), (mid, r.y1 + 2.0)));
    let end = mid + v.clamp(-1.0, 1.0) * r.width() / 2.0;
    let fill = Rect::new(mid.min(end), r.y0 + 2.0, mid.max(end), r.y1 - 2.0);
    if fill.width() > 0.5 {
        cx.scene.fill(Fill::NonZero, Affine::IDENTITY, color, None, &RoundedRect::from_rect(fill, 2.0));
    }
}

/// A curve (at the spectrum points) read at `hz`.
fn sample_curve(v: &[f32], hz: f64) -> f32 {
    let f0 = spectrum_point_freq(0);
    let f1 = spectrum_point_freq(SPECTRUM_POINTS - 1);
    let x = (hz / f0).ln() / (f1 / f0).ln() * (SPECTRUM_POINTS - 1) as f64;
    let i = (x.floor().max(0.0) as usize).min(SPECTRUM_POINTS - 2);
    let t = (x - i as f64).clamp(0.0, 1.0) as f32;
    v[i] + (v[i + 1] - v[i]) * t
}

impl AnalyzerView {
    /// The spectrogram's palette, starting from the theme's well (its silence).
    fn set_gram_colors(&mut self, theme: &mite_ui::Theme) {
        self.gram_colors[0] = theme.well.to_rgba8().to_u8_array();
    }
}

impl View for AnalyzerView {
    fn logical_size(&self) -> (f64, f64) {
        (VIEW_W, VIEW_H)
    }

    fn draw(&mut self, cx: &mut DrawCx<'_>) {
        let theme = cx.theme;
        self.set_gram_colors(theme);
        self.draw_header(cx);
        self.phase_open = Self::phase_page(cx.host);
        cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.display, None, &Rect::new(0.0, DISPLAY_TOP, VIEW_W, HELP_TOP));
        if self.phase_open {
            let source_open = matches!(self.menu, Some((Menu::Source, _)));
            self.phase.draw(cx, &self.shared, self.pointer, source_open, self.hot == Some(Hot::Menu(Menu::Source)));
        } else {
            self.draw_spectrum(cx);
            self.draw_gram(cx);
            self.draw_loudness(cx);
            self.draw_stereo(cx);
        }
        self.help_text().draw(cx, Rect::new(0.0, HELP_TOP, VIEW_W, FOOTER_TOP));
        self.footer.draw_frame(cx);
        self.draw_footer(cx);
        if let Some((_, list)) = &self.menu {
            list.draw(cx, cx.theme.accent, |_, _, _, _| {});
        }
    }

    fn event(&mut self, event: &UiEvent, host: &dyn ParamHost) -> bool {
        self.session.begin(host);
        let recording = RecordingHost::new(host);
        let changed = self.handle_event(event, &recording);
        let busy = self.busy();
        self.session.end(host, recording.wrote(), busy);
        self.session.take_replaced();
        changed
    }

    fn tick(&mut self, host: &dyn ParamHost) -> bool {
        let busy = self.busy();
        self.session.follow(host, busy);
        let dt = self.last_tick.elapsed().as_secs_f64().min(0.1);
        self.last_tick = Instant::now();
        self.phase_open = Self::phase_page(host);
        if self.phase_open {
            self.phase.tick(&self.shared, host, dt);
            if self.shown_at.elapsed().as_secs_f64() >= READOUT_S {
                self.take_readings();
            }
            let history = &mut self.history;
            self.shared.tel.history.read_since(&mut self.seen, |r| {
                if history.len() == 1024 {
                    history.pop_front();
                }
                history.push_back(*r);
            });
            return true;
        }
        self.analyze(host, 0.12);
        // Ballistics by SPEED: rise and fall time constants, s.
        let (rise, fall) = [(0.02, 0.15), (0.06, 0.4), (0.2, 1.2)][Self::index(host, SPEED.id).min(2)];
        let (up, down) = ((1.0 - (-dt / rise).exp()) as f32, (1.0 - (-dt / fall).exp()) as f32);
        for c in 0..2 {
            for (v, &t) in self.curves[c].iter_mut().zip(&self.raw[c]) {
                *v += (t - *v) * if t > *v { up } else { down };
            }
        }
        // The peak hold falls 3 dB a second.
        let sink = (3.0 * dt) as f32;
        for (h, &v) in self.hold.iter_mut().zip(&self.curves[0]) {
            *h = (*h - sink).max(v).max(BOTTOM_DB as f32);
        }
        self.gram_due += dt;
        while self.gram_due >= GRAM_ROW_S {
            self.gram_due -= GRAM_ROW_S;
            self.push_gram_row(host);
        }
        if self.shown_at.elapsed().as_secs_f64() >= READOUT_S {
            self.take_readings();
        }
        let history = &mut self.history;
        self.shared.tel.history.read_since(&mut self.seen, |r| {
            if history.len() == 1024 {
                history.pop_front();
            }
            history.push_back(*r);
        });
        // An analyzer moves all the time.
        true
    }
}
