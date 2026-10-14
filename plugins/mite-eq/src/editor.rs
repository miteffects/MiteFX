//! The Mite EQ editor (docs/design/mockups/eq-main.dc.html), laid out in the mockup's 1280×760
//! design units and shown at [`ZOOM`].
//!
//! Gestures follow docs/design/interaction.md, "Main display gestures".

use std::f64::consts::LN_2;
use std::sync::Arc;
use std::time::Instant;

use mite_dsp::analyzer::Analyzer;
use mite_dsp::eq::{analog_response_db, design, BandSettings, BandType, Placement, Sections, FREQ_MAX, FREQ_MIN};
use mite_ui::kurbo::{Affine, BezPath, Circle, Line, Point, Rect, RoundedRect, Stroke};
use mite_ui::paint::{glow_stroke, panel, raised};
use mite_ui::peniko::{Color, Fill, Gradient};
use mite_ui::widgets::brand::{draw_mark, draw_title};
use mite_ui::widgets::{Knob, KnobKind, LevelMeter};
use mite_ui::{Align, DrawCx, MouseButton, ParamHost, TextStyle, UiEvent, View};

use crate::params::{band_id, BANDS, FREQ, GAIN, ON, OUT_GAIN, Q, SLOPE, STEREO, TYPE};
use crate::EqShared;

/// Mite EQ wears the EQ accent (tokens.json `color.accent.eq`).
pub const ACCENT: &str = "eq";
/// Analyzer FFT size: the design's "8192 · High" resolution.
pub const ANALYZER_SIZE: usize = 8192;
/// Default UI scale. The mockup is 1280×760; 75 % suits most screens.
pub const ZOOM: f64 = 1.0;

const WIDTH: f64 = 1280.0;
const HEIGHT: f64 = 760.0;
const HEADER: f64 = 56.0;
const DISPLAY_TOP: f64 = HEADER;
const DISPLAY_H: f64 = 620.0;
const HELP_TOP: f64 = DISPLAY_TOP + DISPLAY_H;
const FOOTER_TOP: f64 = HELP_TOP + 28.0;

/// Plot geometry inside the display (mockup: W 1200, TOP 24, BOT 588, MID 306).
const PLOT_W: f64 = 1200.0;
const PLOT_TOP: f64 = DISPLAY_TOP + 24.0;
const PLOT_BOT: f64 = DISPLAY_TOP + 588.0;
const MID: f64 = DISPLAY_TOP + 306.0;
const PX_PER_DB: f64 = 264.0 / 18.0;
/// Curve and spectrum resolution: one point every 4 design px.
const POINTS: usize = 301;

const NODE_HIT: f64 = 16.0;
const PANEL_W: f64 = 620.0;
const PANEL_H: f64 = 104.0;
/// Panel dials: diameter, and the x of their group's centre relative to the panel.
const PANEL_DIAL: f64 = 56.0;
const PANEL_DIALS_X: f64 = 450.0;
const MENU_ITEM_H: f64 = 28.0;
/// The band panel is docked at the bottom centre of the plot, like FabFilter's, so it never
/// covers the area around the node being edited.
const PANEL_ORIGIN: Point = Point::new((PLOT_W - PANEL_W) / 2.0, HELP_TOP - PANEL_H - 10.0);

fn fx(f: f64) -> f64 {
    PLOT_W * (f / FREQ_MIN).ln() / (FREQ_MAX / FREQ_MIN).ln()
}

fn xf(x: f64) -> f64 {
    FREQ_MIN * ((x / PLOT_W) * (FREQ_MAX / FREQ_MIN).ln()).exp()
}

fn gy(db: f64) -> f64 {
    MID - db * PX_PER_DB
}

fn yg(y: f64) -> f64 {
    (MID - y) / PX_PER_DB
}

/// Spectrum: 0 dBFS at the top of the plot, −90 at the bottom.
fn sy(db: f64) -> f64 {
    PLOT_TOP + (-db / 90.0) * (PLOT_BOT - PLOT_TOP)
}

fn note_name(freq: f64) -> String {
    const NAMES: [&str; 12] = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];
    let midi = 69.0 + 12.0 * (freq / 440.0).log2();
    let n = midi.round();
    let cents = ((midi - n) * 100.0).round() as i32;
    let idx = (n as i32).rem_euclid(12) as usize;
    let octave = (n as i32).div_euclid(12) - 1;
    format!("{}{} {}{} ct", NAMES[idx], octave, if cents < 0 { "−" } else { "+" }, cents.abs())
}

/// Host ids for one band, built once.
struct BandIds {
    on: String,
    kind: String,
    freq: String,
    gain: String,
    q: String,
    slope: String,
    stereo: String,
}

impl BandIds {
    fn new(b: usize) -> Self {
        Self {
            on: band_id(b, ON.id),
            kind: band_id(b, TYPE.id),
            freq: band_id(b, FREQ.id),
            gain: band_id(b, GAIN.id),
            q: band_id(b, Q.id),
            slope: band_id(b, SLOPE.id),
            stereo: band_id(b, STEREO.id),
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum DragMode {
    Move,
    Width,
}

struct NodeDrag {
    band: usize,
    mode: DragMode,
    last: Point,
    log_freq: f64,
    gain: f64,
    log_q: f64,
}

/// How one display point reads the FFT: average bins `first..=last`, or interpolate between
/// `first` and `last` by `frac`.
struct SpectrumPoint {
    first: usize,
    last: usize,
    frac: f32,
    tilt: f32,
    average: bool,
}

struct BandResponse {
    settings: BandSettings,
    fs: f64,
    sections: Sections,
    grid: Vec<f64>,
}

/// x of display grid point `i`.
fn grid_x(i: usize) -> f64 {
    i as f64 * PLOT_W / (POINTS - 1) as f64
}

/// A band's response in dB at `f`, as drawn. Up to half Nyquist it is the digital filter's.
/// From there it blends smoothly into the analog shape, which continues above Nyquist (the plot
/// runs to 30 kHz), as FabFilter draws it. A digital response always flattens at Nyquist, so
/// splicing the analog curve on there kinked the line; the matched filters track the analog shape
/// closely, so the blend moves the line only slightly.
fn band_db(sections: &Sections, s: &BandSettings, f: f64, fs: f64) -> f64 {
    let floor = |db: f64| if db.is_nan() { -300.0 } else { db.max(-300.0) };
    let nyquist = 0.5 * fs;
    let start = 0.5 * nyquist;
    if f <= start {
        return sections.response_db(2.0 * std::f64::consts::PI * f / fs);
    }
    let analog = floor(analog_response_db(s, f));
    if f >= nyquist {
        return analog;
    }
    let digital = floor(sections.response_db(2.0 * std::f64::consts::PI * f / fs));
    // Smoothstep over the octave in log frequency: no step and no kink at either end.
    let t = (f / start).log2();
    let t = t * t * (3.0 - 2.0 * t);
    digital + t * (analog - digital)
}

/// Draws a band type's response shape as a 22 × 12 icon with its top-left at `at`.
fn draw_type_icon(scene: &mut mite_ui::vello::Scene, kind: BandType, at: Point, color: Color) {
    let mut p = BezPath::new();
    let pt = |x: f64, y: f64| (at.x + x, at.y + y);
    match kind {
        BandType::Bell => {
            p.move_to(pt(0.0, 9.0));
            p.line_to(pt(5.0, 9.0));
            p.curve_to(pt(8.0, 9.0), pt(9.0, 2.0), pt(11.0, 2.0));
            p.curve_to(pt(13.0, 2.0), pt(14.0, 9.0), pt(17.0, 9.0));
            p.line_to(pt(22.0, 9.0));
        }
        BandType::LowShelf => {
            p.move_to(pt(0.0, 3.0));
            p.line_to(pt(6.0, 3.0));
            p.curve_to(pt(10.0, 3.0), pt(12.0, 9.0), pt(16.0, 9.0));
            p.line_to(pt(22.0, 9.0));
        }
        BandType::HighShelf => {
            p.move_to(pt(0.0, 9.0));
            p.line_to(pt(6.0, 9.0));
            p.curve_to(pt(10.0, 9.0), pt(12.0, 3.0), pt(16.0, 3.0));
            p.line_to(pt(22.0, 3.0));
        }
        BandType::LowCut => {
            p.move_to(pt(2.0, 12.0));
            p.curve_to(pt(5.0, 4.0), pt(7.0, 3.0), pt(12.0, 3.0));
            p.line_to(pt(22.0, 3.0));
        }
        BandType::HighCut => {
            p.move_to(pt(0.0, 3.0));
            p.line_to(pt(10.0, 3.0));
            p.curve_to(pt(15.0, 3.0), pt(17.0, 4.0), pt(20.0, 12.0));
        }
        BandType::Notch => {
            p.move_to(pt(0.0, 3.0));
            p.line_to(pt(7.0, 3.0));
            p.curve_to(pt(10.0, 3.0), pt(10.5, 12.0), pt(11.0, 12.0));
            p.curve_to(pt(11.5, 12.0), pt(12.0, 3.0), pt(15.0, 3.0));
            p.line_to(pt(22.0, 3.0));
        }
        BandType::BandPass => {
            p.move_to(pt(1.0, 12.0));
            p.curve_to(pt(6.0, 12.0), pt(8.0, 2.0), pt(11.0, 2.0));
            p.curve_to(pt(14.0, 2.0), pt(16.0, 12.0), pt(21.0, 12.0));
        }
        BandType::TiltShelf => {
            p.move_to(pt(0.0, 10.0));
            p.curve_to(pt(9.0, 10.0), pt(13.0, 2.0), pt(22.0, 2.0));
        }
    }
    let stroke = Stroke::new(1.6).with_caps(mite_ui::kurbo::Cap::Round).with_join(mite_ui::kurbo::Join::Round);
    scene.stroke(&stroke, Affine::IDENTITY, color, None, &p);
}

/// Which band setting an open dropdown chooses.
#[derive(Clone, Copy, PartialEq)]
enum MenuKind {
    Type,
    Slope,
    Stereo,
}

/// An open dropdown: its button, its items and the hovered item.
struct Menu {
    kind: MenuKind,
    band: usize,
    anchor: Rect,
    items: &'static [&'static str],
    current: usize,
    hover: Option<usize>,
}

impl Menu {
    /// Item rectangles: below the button when they fit inside the display, otherwise above.
    fn item_rects(&self) -> Vec<Rect> {
        let n = self.items.len() as f64;
        let w = self.anchor.width().max(if self.kind == MenuKind::Type { 180.0 } else { 150.0 });
        let h = n * MENU_ITEM_H + 8.0;
        let top = if self.anchor.y1 + 4.0 + h <= HELP_TOP { self.anchor.y1 + 4.0 } else { self.anchor.y0 - 4.0 - h };
        (0..self.items.len())
            .map(|i| {
                let y = top + 4.0 + i as f64 * MENU_ITEM_H;
                Rect::new(self.anchor.x0, y, self.anchor.x0 + w, y + MENU_ITEM_H)
            })
            .collect()
    }

    fn bounds(&self) -> Rect {
        let items = self.item_rects();
        let (first, last) = (items[0], items[items.len() - 1]);
        Rect::new(first.x0, first.y0 - 4.0, first.x1, last.y1 + 4.0)
    }
}

/// Clickable rectangles of the open band panel, set while drawing.
#[derive(Default, Clone, Copy)]
struct PanelHits {
    panel: Rect,
    kind: Rect,
    slope: Option<Rect>,
    stereo: Rect,
    bypass: Rect,
    close: Rect,
}

pub struct EqView {
    shared: Arc<EqShared>,
    ids: Vec<BandIds>,
    selected: Option<usize>,
    hover: Option<usize>,
    pointer: Option<Point>,
    drag: Option<NodeDrag>,
    panel_knobs: Vec<Knob>,
    panel_hits: Option<PanelHits>,
    menu: Option<Menu>,
    out_knob: Knob,
    meter: LevelMeter,
    analyzer: Analyzer,
    samples: Vec<f32>,
    power: Vec<f32>,
    spectrum_map: Vec<SpectrumPoint>,
    spectrum_rate: f64,
    pre: Vec<f64>,
    post: Vec<f64>,
    last_spectrum: Instant,
    last_analysis: Instant,
    pre_target: Vec<f32>,
    post_target: Vec<f32>,
    responses: Vec<Option<BandResponse>>,
    signature: Vec<f32>,
    last_signature_check: Instant,
    /// Wheel movement not yet turned into whole slope steps (smooth-scrolling devices send
    /// fractions of a line).
    slope_wheel: f32,
    ticks: u32,
}

impl EqView {
    pub fn new(shared: Arc<EqShared>) -> Self {
        let analyzer = Analyzer::new(ANALYZER_SIZE);
        let bins = analyzer.bins();
        let mut out_knob = Knob::new(OUT_GAIN, "", KnobKind::Bipolar, Point::new(1196.0, FOOTER_TOP + 28.0), 34.0);
        out_knob.value_inside = false;
        Self {
            shared,
            ids: (0..BANDS).map(BandIds::new).collect(),
            selected: None,
            hover: None,
            pointer: None,
            drag: None,
            panel_knobs: Vec::new(),
            panel_hits: None,
            menu: None,
            out_knob,
            meter: LevelMeter::new(Rect::new(1233.5, DISPLAY_TOP + 19.5, 1266.5, DISPLAY_TOP + 592.5)),
            analyzer,
            samples: vec![0.0; ANALYZER_SIZE],
            power: vec![0.0; bins],
            spectrum_map: Vec::new(),
            spectrum_rate: 0.0,
            pre: vec![-150.0; POINTS],
            post: vec![-150.0; POINTS],
            last_spectrum: Instant::now(),
            last_analysis: Instant::now(),
            pre_target: vec![-150.0; POINTS],
            post_target: vec![-150.0; POINTS],
            responses: (0..BANDS).map(|_| None).collect(),
            signature: Vec::new(),
            last_signature_check: Instant::now(),
            slope_wheel: 0.0,
            ticks: 0,
        }
    }

    fn plain(host: &dyn ParamHost, id: &str) -> f32 {
        host.plain(id, host.normalized(id))
    }

    fn band(&self, host: &dyn ParamHost, b: usize) -> BandSettings {
        let ids = &self.ids[b];
        BandSettings {
            enabled: Self::plain(host, &ids.on) >= 0.5,
            kind: BandType::from_index(Self::plain(host, &ids.kind).round().max(0.0) as usize),
            freq: f64::from(Self::plain(host, &ids.freq)),
            gain_db: f64::from(Self::plain(host, &ids.gain)),
            q: f64::from(Self::plain(host, &ids.q)),
            slope: Self::plain(host, &ids.slope).round().max(0.0) as usize,
            placement: Placement::from_index(Self::plain(host, &ids.stereo).round().max(0.0) as usize),
        }
    }

    fn set(host: &dyn ParamHost, id: &str, plain: f32) {
        host.begin_gesture(id);
        host.set_normalized(id, host.normalize(id, plain));
        host.end_gesture(id);
    }

    fn node_pos(s: &BandSettings) -> Point {
        let y = if s.kind.has_gain() { gy(s.gain_db.clamp(-18.0, 18.0)) } else { MID };
        Point::new(fx(s.freq), y)
    }

    /// One colour per stereo placement: all stereo bands share a colour, all left bands
    /// another, and so on, so the colour says what the band processes.
    fn band_color(theme: &mite_ui::Theme, s: &BandSettings) -> Color {
        theme.band_ramp[s.placement as usize % theme.band_ramp.len()]
    }

    fn node_at(&self, host: &dyn ParamHost, p: Point) -> Option<usize> {
        // Topmost first: the selected band, then the highest band number.
        let mut order: Vec<usize> = (0..BANDS).rev().collect();
        if let Some(sel) = self.selected {
            order.retain(|&b| b != sel);
            order.insert(0, sel);
        }
        order.into_iter().find(|&b| {
            let s = self.band(host, b);
            s.enabled && Self::node_pos(&s).distance(p) <= NODE_HIT
        })
    }

    fn select(&mut self, band: Option<usize>) {
        self.selected = band;
        self.slope_wheel = 0.0;
        self.menu = None;
        self.panel_knobs.clear();
        self.panel_hits = None;
    }

    fn sample_rate(&self) -> f64 {
        f64::from(self.shared.sample_rate.load()).max(8000.0)
    }

    /// Band `b`'s digital sections and its response in dB at every grid point, cached until its
    /// settings or the sample rate change.
    fn response(&mut self, b: usize, s: &BandSettings) -> &BandResponse {
        let fs = self.sample_rate();
        let stale = !matches!(&self.responses[b], Some(r) if r.settings == *s && r.fs == fs);
        if stale {
            let sections = design(s, fs);
            let grid = (0..POINTS).map(|i| band_db(&sections, s, xf(grid_x(i)), fs)).collect();
            self.responses[b] = Some(BandResponse { settings: *s, fs, sections, grid });
        }
        self.responses[b].as_ref().expect("just filled")
    }

    /// Per display point: the FFT bins it averages (or the bin pair it interpolates) and its
    /// tilt gain. Depends only on the sample rate, so it is rebuilt only when that changes.
    fn rebuild_spectrum_map(&mut self, fs: f64) {
        let size = self.analyzer.size() as f64;
        let last = self.power.len() - 1;
        self.spectrum_map = (0..POINTS)
            .map(|i| {
                let f = xf(i as f64 * PLOT_W / (POINTS - 1) as f64);
                // 1/12-octave smoothing: ±1/24 octave around the point.
                let (lo, hi) = (f * 2f64.powf(-1.0 / 24.0) * size / fs, f * 2f64.powf(1.0 / 24.0) * size / fs);
                // 4.5 dB/oct tilt around 1 kHz (settings default), as a power factor.
                let tilt = 10f64.powf(0.45 * (f / 1000.0).log2()) as f32;
                if hi.floor() - lo.ceil() < 1.0 {
                    let k = f * size / fs;
                    let k0 = (k.floor() as usize).min(last - 1);
                    SpectrumPoint { first: k0, last: k0 + 1, frac: (k - k0 as f64) as f32, tilt, average: false }
                } else {
                    let (a, b) = (lo.ceil() as usize, (hi.floor() as usize).min(last));
                    SpectrumPoint { first: a.min(b), last: b, frac: 0.0, tilt, average: true }
                }
            })
            .collect();
        self.spectrum_rate = fs;
    }

    /// Called on every animation frame (~2× the display refresh). The FFT runs at most once
    /// per display refresh; every call advances the ballistics towards the latest analysis, so
    /// in-between frames still move smoothly.
    fn update_spectrum(&mut self) -> bool {
        let dt = self.last_spectrum.elapsed().as_secs_f64().min(0.1);
        self.last_spectrum = Instant::now();
        let fs = self.sample_rate();
        if fs != self.spectrum_rate {
            self.rebuild_spectrum_map(fs);
        }
        if self.last_analysis.elapsed().as_secs_f64() >= 0.0155 {
            self.last_analysis = Instant::now();
            for which in 0..2 {
                let tap = if which == 0 { &self.shared.pre } else { &self.shared.post };
                tap.read_latest(&mut self.samples);
                self.analyzer.analyze_power(&self.samples, &mut self.power);
                let target = if which == 0 { &mut self.pre_target } else { &mut self.post_target };
                for (t, m) in target.iter_mut().zip(&self.spectrum_map) {
                    let power = if m.average {
                        let span = &self.power[m.first..=m.last];
                        span.iter().sum::<f32>() / span.len() as f32
                    } else {
                        self.power[m.first] * (1.0 - m.frac) + self.power[m.last] * m.frac
                    };
                    *t = 10.0 * (power * m.tilt).max(1e-15).log10();
                }
            }
        }
        let rise = (1.0 - (-dt / 0.03).exp()) as f32;
        let fall = (60.0 * dt) as f32;
        let mut changed = false;
        for (curve, target) in [(&mut self.pre, &self.pre_target), (&mut self.post, &self.post_target)] {
            for (value, &db) in curve.iter_mut().zip(target.iter()) {
                // Ballistics: ~30 ms rise, 60 dB/s fall.
                let current = *value as f32;
                let next = if db > current { current + (db - current) * rise } else { db.max(current - fall) };
                if (next - current).abs() > 0.02 {
                    changed = true;
                }
                *value = f64::from(next);
            }
        }
        changed
    }

    fn help_text(&self, host: &dyn ParamHost) -> String {
        let band_line = |b: usize| {
            let s = self.band(host, b);
            let mut t = format!("Band {} · {} · {}", b + 1, BandType::NAMES[s.kind as usize], FREQ.format(s.freq as f32));
            if s.kind.has_gain() {
                t += &format!(" · {}", GAIN.format(s.gain_db as f32));
            }
            let wheel = if s.kind.has_slope() { "wheel for slope" } else { "Alt-drag or wheel for Q" };
            t + " — drag to move, Shift for fine, " + wheel + ", double-click resets, right-click deletes"
        };
        if let Some(d) = &self.drag {
            return band_line(d.band);
        }
        if let Some(b) = self.hover {
            return band_line(b);
        }
        if let Some(p) = self.pointer.filter(|p| p.x < PLOT_W && p.y > PLOT_TOP && p.y < PLOT_BOT) {
            let f = xf(p.x);
            return format!("{} · {} · {:+.1} dB — double-click to add a band", FREQ.format(f as f32), note_name(f), yg(p.y));
        }
        if let Some(b) = self.selected {
            return band_line(b);
        }
        if (0..BANDS).any(|b| self.band(host, b).enabled) {
            "Click a band to edit it. Double-click empty space to add one.".into()
        } else {
            "No bands yet — the plugin is passing audio unchanged. Double-click anywhere to add a band.".into()
        }
    }

    fn add_band(&mut self, host: &dyn ParamHost, p: Point) {
        let Some(b) = (0..BANDS).find(|&b| !self.band(host, b).enabled) else { return };
        let ids = &self.ids[b];
        Self::set(host, &ids.kind, 0.0);
        Self::set(host, &ids.freq, xf(p.x).clamp(FREQ_MIN, FREQ_MAX) as f32);
        Self::set(host, &ids.gain, yg(p.y).clamp(-18.0, 18.0) as f32);
        Self::set(host, &ids.q, 1.0);
        Self::set(host, &ids.on, 1.0);
        self.select(Some(b));
    }

    fn build_panel_knobs(&mut self, host: &dyn ParamHost, origin: Point) {
        let Some(b) = self.selected else { return };
        let s = self.band(host, b);
        let ids = &self.ids[b];
        let mut col = 0.0;
        // Dials in a row on the right of the panel, beside the buttons.
        let count = if s.kind.has_gain() { 3.0 } else { 2.0 };
        let first = origin.x + PANEL_DIALS_X - (count - 1.0) * 40.0;
        let mut knob = |spec, label, kind, id: &str| {
            let k = Knob::new(spec, label, kind, Point::new(first + col * 80.0, origin.y + 44.0), PANEL_DIAL)
                .with_id(id);
            col += 1.0;
            k
        };
        let mut knobs = vec![knob(FREQ, "FREQ", KnobKind::Unipolar, &ids.freq)];
        if s.kind.has_gain() {
            knobs.push(knob(GAIN, "GAIN", KnobKind::Bipolar, &ids.gain));
        }
        knobs.push(knob(Q, "Q", KnobKind::Unipolar, &ids.q));
        self.panel_knobs = knobs;
    }

    // ---- drawing ------------------------------------------------------------------------

    fn draw_grid(&self, cx: &mut DrawCx<'_>) {
        let theme = cx.theme;
        let (mut minor, mut major) = (BezPath::new(), BezPath::new());
        for dec in [10.0, 100.0, 1000.0, 10000.0] {
            for k in 1..=9 {
                let f = dec * f64::from(k);
                if f > FREQ_MAX {
                    break;
                }
                let x = fx(f).round() + 0.5;
                let path = if k == 1 { &mut major } else { &mut minor };
                path.move_to((x, DISPLAY_TOP));
                path.line_to((x, HELP_TOP));
            }
        }
        for d in [-15.0, -9.0, -3.0, 3.0, 9.0, 15.0] {
            minor.move_to((0.0, gy(d).round() + 0.5));
            minor.line_to((PLOT_W, gy(d).round() + 0.5));
        }
        for d in [-12.0, -6.0, 6.0, 12.0] {
            major.move_to((0.0, gy(d).round() + 0.5));
            major.line_to((PLOT_W, gy(d).round() + 0.5));
        }
        let hair = Stroke::new(1.0);
        cx.scene.stroke(&hair, Affine::IDENTITY, theme.grid_minor, None, &minor);
        cx.scene.stroke(&hair, Affine::IDENTITY, theme.grid_major, None, &major);
        cx.scene.stroke(&hair, Affine::IDENTITY, theme.grid_zero, None, &Line::new((0.0, MID + 0.5), (PLOT_W, MID + 0.5)));
        cx.scene.stroke(&hair, Affine::IDENTITY, theme.line, None, &Line::new((PLOT_W + 0.5, DISPLAY_TOP), (PLOT_W + 0.5, HELP_TOP)));

        let label = TextStyle::READOUT.sized(10.0);
        for (f, t) in [(20.0, "20"), (50.0, "50"), (100.0, "100"), (200.0, "200"), (500.0, "500"), (1000.0, "1k"), (2000.0, "2k"), (5000.0, "5k"), (10000.0, "10k"), (20000.0, "20k")] {
            cx.text.draw(cx.scene, t, fx(f), DISPLAY_TOP + 596.0, label, theme.text_lo, Align::Center);
        }
        for d in [12, 6, 0, -6, -12] {
            let t = match d.cmp(&0) {
                std::cmp::Ordering::Greater => format!("+{d}"),
                std::cmp::Ordering::Less => format!("−{}", -d),
                std::cmp::Ordering::Equal => "0".into(),
            };
            let color = if d == 0 { theme.text_mid } else { theme.text_lo };
            cx.text.draw(cx.scene, &t, 1206.0, gy(f64::from(d)) - 7.0, label, color, Align::Left);
        }
    }

    fn curve_path(values: &[f64], map: impl Fn(f64) -> f64) -> BezPath {
        let mut path = BezPath::new();
        let mut started = false;
        for (i, &v) in values.iter().enumerate() {
            if !v.is_finite() {
                continue;
            }
            let p = (i as f64 * PLOT_W / (POINTS - 1) as f64, map(v));
            if started {
                path.line_to(p);
            } else {
                path.move_to(p);
                started = true;
            }
        }
        path
    }

    fn xy_path(xs: &[(f64, Option<usize>)], values: &[f64], map: impl Fn(f64) -> f64) -> BezPath {
        let mut path = BezPath::new();
        for (i, (&(x, _), &v)) in xs.iter().zip(values).enumerate() {
            if i == 0 {
                path.move_to((x, map(v)));
            } else {
                path.line_to((x, map(v)));
            }
        }
        path
    }

    fn draw_spectrum(&self, cx: &mut DrawCx<'_>) {
        let theme = cx.theme;
        // Silence (or no audio yet): draw nothing rather than a line along the floor.
        if self.pre.iter().chain(&self.post).all(|&v| v < -88.0) {
            return;
        }
        let clamp = |db: f64| sy(db.clamp(-90.0, 6.0));
        let pre = Self::curve_path(&self.pre, clamp);
        cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, theme.spectrum_pre.multiply_alpha(0.5), None, &pre);

        let post = Self::curve_path(&self.post, clamp);
        let mut fill = post.clone();
        fill.line_to((PLOT_W, HELP_TOP));
        fill.line_to((0.0, HELP_TOP));
        fill.close_path();
        let gradient = Gradient::new_linear((0.0, PLOT_TOP), (0.0, HELP_TOP)).with_stops([
            (0.0, theme.accent.multiply_alpha(0.34)),
            (0.55, theme.spectrum_post_fill.multiply_alpha(0.12)),
            (1.0, theme.spectrum_post_fill.multiply_alpha(0.0)),
        ]);
        cx.scene.fill(Fill::NonZero, Affine::IDENTITY, &gradient, None, &fill);
        cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, theme.accent.multiply_alpha(0.55), None, &post);
    }

    fn draw_curves(&mut self, cx: &mut DrawCx<'_>, bands: &[BandSettings]) {
        let theme = cx.theme;
        let fs = self.sample_rate();
        // The fixed grid plus every band's own frequency: a notch's zero (or a narrow bell's peak)
        // falls between grid points, so without it the drawn depth changed as the band moved.
        let mut xs: Vec<(f64, Option<usize>)> = (0..POINTS).map(|i| (grid_x(i), Some(i))).collect();
        for s in bands.iter().filter(|s| s.enabled) {
            let x = fx(s.freq);
            if x > 0.0 && x < PLOT_W {
                xs.push((x, None));
            }
        }
        xs.sort_by(|a, b| a.0.total_cmp(&b.0));
        let mut total = vec![0.0_f64; xs.len()];
        let mut any = false;
        // Curves may leave the plot (the display is clipped); clamping only keeps paths finite.
        let clamp = |db: f64| gy(if db.is_nan() { -40.0 } else { db.clamp(-40.0, 40.0) });
        let mut values = vec![0.0_f64; xs.len()];
        for (b, s) in bands.iter().enumerate() {
            if !s.enabled {
                continue;
            }
            any = true;
            let color = Self::band_color(theme, s);
            let selected = self.selected == Some(b);
            let r = self.response(b, s);
            for (v, &(x, grid)) in values.iter_mut().zip(&xs) {
                *v = match grid {
                    Some(i) => r.grid[i],
                    None => band_db(&r.sections, s, xf(x), fs),
                };
            }
            for (t, v) in total.iter_mut().zip(&values) {
                *t += v;
            }
            let mut fill = Self::xy_path(&xs, &values, clamp);
            fill.line_to((PLOT_W, MID));
            fill.line_to((0.0, MID));
            fill.close_path();
            cx.scene.fill(Fill::NonZero, Affine::IDENTITY, color.multiply_alpha(if selected { 0.22 } else { 0.08 }), None, &fill);
        }
        let curve = if any { Self::xy_path(&xs, &total, clamp) } else {
            let mut flat = BezPath::new();
            flat.move_to((0.0, MID));
            flat.line_to((PLOT_W, MID));
            flat
        };
        glow_stroke(cx.scene, &curve, 2.2, theme.curve, theme.curve, 5.0);
    }

    fn draw_nodes(&self, cx: &mut DrawCx<'_>, bands: &[BandSettings]) {
        let theme = cx.theme;
        let mut order: Vec<usize> = (0..BANDS).collect();
        if let Some(sel) = self.selected {
            order.retain(|&b| b != sel);
            order.push(sel);
        }
        for b in order {
            let s = &bands[b];
            if !s.enabled {
                continue;
            }
            let color = Self::band_color(theme, s);
            let p = Self::node_pos(s);
            let selected = self.selected == Some(b);
            let hovered = self.hover == Some(b);
            let r = if selected { 9.0 } else if hovered { 8.0 } else { 7.0 };
            if selected {
                cx.scene.fill(Fill::NonZero, Affine::IDENTITY, color.multiply_alpha(0.2), None, &Circle::new(p, r + 5.0));
                glow_stroke(cx.scene, &Circle::new(p, r - 1.0), 2.0, theme.needle, color, 6.0);
                cx.scene.fill(Fill::NonZero, Affine::IDENTITY, color, None, &Circle::new(p, r - 2.0));
                let num = format!("{}", b + 1);
                cx.text.draw(cx.scene, &num, p.x, p.y - 6.5, TextStyle::LABEL.sized(9.0).weighted(800.0), Color::from_rgba8(14, 17, 22, 255), Align::Center);
            } else {
                if hovered {
                    cx.scene.fill(Fill::NonZero, Affine::IDENTITY, color.multiply_alpha(0.13), None, &Circle::new(p, r + 6.0));
                }
                cx.scene.fill(Fill::NonZero, Affine::IDENTITY, color.multiply_alpha(if hovered { 0.33 } else { 0.2 }), None, &Circle::new(p, r - 1.0));
                glow_stroke(cx.scene, &Circle::new(p, r - 1.0), 2.0, color, color, 3.0);
            }
        }
    }

    fn draw_panel(&mut self, cx: &mut DrawCx<'_>, bands: &[BandSettings]) {
        let Some(b) = self.selected else { return };
        let s = bands[b];
        if !s.enabled {
            return;
        }
        let theme = cx.theme;
        let origin = PANEL_ORIGIN;
        if self.panel_knobs.len() != if s.kind.has_gain() { 3 } else { 2 } {
            self.build_panel_knobs(cx.host, origin);
        }
        let rect = Rect::new(origin.x, origin.y, origin.x + PANEL_W, origin.y + PANEL_H);
        let shape = RoundedRect::from_rect(rect, 14.0);
        if theme.shadows {
            cx.scene.draw_blurred_rounded_rect(Affine::translate((0.0, 14.0)), rect, Color::from_rgba8(0, 0, 0, 140), 14.0, 20.0);
        }
        // Backdrop blur is a later shader pass; the glass tint is drawn nearly opaque until then.
        panel(cx.scene, shape, theme.glass.with_alpha(0.94), theme.glass_border);

        // Left: band title over a row of dropdown buttons.
        let color = Self::band_color(theme, &s);
        let dot = Point::new(origin.x + 22.0, origin.y + 24.0);
        glow_stroke(cx.scene, &Circle::new(dot, 4.0), 2.0, color, color, 4.0);
        cx.scene.fill(Fill::NonZero, Affine::IDENTITY, color, None, &Circle::new(dot, 5.0));
        let title = format!("Band {}", b + 1);
        cx.text.draw(cx.scene, &title, origin.x + 34.0, origin.y + 16.0, TextStyle::TITLE.sized(13.0), theme.text_hi, Align::Left);

        let row_y = origin.y + 54.0;
        let open = self.menu.as_ref().map(|m| m.kind);
        let dropdown = |cx: &mut DrawCx<'_>, x: f64, w: f64, text: &str, icon: Option<BandType>, active: bool| -> Rect {
            let r = Rect::new(x, row_y, x + w, row_y + 32.0);
            let border = if active { color } else { cx.theme.panel_control_border };
            raised(cx.scene, RoundedRect::from_rect(r, 6.0), cx.theme.panel_control, border, cx.theme.shadows);
            let text_x = match icon {
                Some(kind) => {
                    draw_type_icon(cx.scene, kind, Point::new(x + 10.0, row_y + 10.0), color);
                    x + 40.0
                }
                None => x + 12.0,
            };
            cx.text.draw(cx.scene, text, text_x, row_y + 9.0, TextStyle::BODY.weighted(600.0), cx.theme.text_hi, Align::Left);
            let mut chevron = BezPath::new();
            let (cxr, cy) = (r.x1 - 14.0, r.center().y);
            chevron.move_to((cxr - 4.0, cy - 2.0));
            chevron.line_to((cxr, cy + 2.0));
            chevron.line_to((cxr + 4.0, cy - 2.0));
            let round = Stroke::new(1.5).with_caps(mite_ui::kurbo::Cap::Round).with_join(mite_ui::kurbo::Join::Round);
            cx.scene.stroke(&round, Affine::IDENTITY, cx.theme.text_mid, None, &chevron);
            r
        };
        let mut x = origin.x + 14.0;
        let kind = dropdown(cx, x, 140.0, BandType::NAMES[s.kind as usize], Some(s.kind), open == Some(MenuKind::Type));
        x += 148.0;
        let slope = s.kind.has_slope().then(|| {
            let r = dropdown(cx, x, 108.0, SLOPE.choices[s.slope.min(SLOPE.choices.len() - 1)], None, open == Some(MenuKind::Slope));
            x += 116.0;
            r
        });
        let stereo = dropdown(cx, x, 90.0, Placement::NAMES[s.placement as usize], None, open == Some(MenuKind::Stereo));

        // Right: bypass over close.
        let round = Stroke::new(1.5).with_caps(mite_ui::kurbo::Cap::Round);
        let close = Rect::new(origin.x + PANEL_W - 44.0, origin.y + 12.0, origin.x + PANEL_W - 14.0, origin.y + 42.0);
        let bypass = Rect::new(close.x0, origin.y + 58.0, close.x1, origin.y + 88.0);
        for r in [close, bypass] {
            raised(cx.scene, RoundedRect::from_rect(r, 6.0), theme.panel_control, theme.panel_control_border, theme.shadows);
        }
        let c = bypass.center();
        let mut glyph = BezPath::new();
        glyph.move_to((c.x, c.y - 5.5));
        glyph.line_to((c.x, c.y - 0.5));
        let arc = mite_ui::kurbo::Arc::new(c + (0.0, 1.0), (4.5, 4.5), -1.0, 5.3, 0.0);
        cx.scene.stroke(&round, Affine::IDENTITY, theme.text_mid, None, &glyph);
        cx.scene.stroke(&round, Affine::IDENTITY, theme.text_mid, None, &arc);
        let mut cross = BezPath::new();
        let k = close.center();
        cross.move_to((k.x - 3.0, k.y - 3.0));
        cross.line_to((k.x + 3.0, k.y + 3.0));
        cross.move_to((k.x + 3.0, k.y - 3.0));
        cross.line_to((k.x - 3.0, k.y + 3.0));
        cx.scene.stroke(&round, Affine::IDENTITY, theme.text_mid, None, &cross);

        for knob in &mut self.panel_knobs {
            knob.color = Some(color);
            knob.draw(cx);
        }
        self.panel_hits = Some(PanelHits { panel: rect, kind, slope, stereo, bypass, close });
    }

    /// The open dropdown, drawn over everything.
    fn draw_menu(&self, cx: &mut DrawCx<'_>) {
        let Some(menu) = &self.menu else { return };
        let theme = cx.theme;
        let bounds = menu.bounds();
        if theme.shadows {
            cx.scene.draw_blurred_rounded_rect(Affine::translate((0.0, 8.0)), bounds, Color::from_rgba8(0, 0, 0, 150), 8.0, 14.0);
        }
        panel(cx.scene, RoundedRect::from_rect(bounds, 8.0), theme.glass.with_alpha(0.98), theme.glass_border);
        for (i, (r, item)) in menu.item_rects().into_iter().zip(menu.items).enumerate() {
            let r = r.inset((-4.0, 0.0));
            if menu.hover == Some(i) {
                cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.panel_control, None, &RoundedRect::from_rect(r, 5.0));
            }
            let current = i == menu.current;
            let color = if current { theme.text_strong } else { theme.text_hi };
            let style = TextStyle::BODY.weighted(if current { 700.0 } else { 500.0 });
            let text_x = if menu.kind == MenuKind::Type {
                let icon = if current { theme.text_strong } else { theme.text_mid };
                draw_type_icon(cx.scene, BandType::from_index(i), Point::new(r.x0 + 24.0, r.y0 + 8.0), icon);
                r.x0 + 56.0
            } else {
                r.x0 + 24.0
            };
            cx.text.draw(cx.scene, item, text_x, r.y0 + 7.0, style, color, Align::Left);
            if current {
                let mut tick = BezPath::new();
                let (tx, ty) = (r.x0 + 11.0, r.center().y);
                tick.move_to((tx - 3.5, ty));
                tick.line_to((tx - 1.0, ty + 3.0));
                tick.line_to((tx + 4.0, ty - 3.0));
                let round = Stroke::new(1.6).with_caps(mite_ui::kurbo::Cap::Round).with_join(mite_ui::kurbo::Join::Round);
                cx.scene.stroke(&round, Affine::IDENTITY, theme.accent, None, &tick);
            }
        }
    }

    /// Opens (or, if it is already open, closes) a band's dropdown.
    fn toggle_menu(&mut self, kind: MenuKind, band: usize, anchor: Rect, host: &dyn ParamHost) {
        if self.menu.as_ref().is_some_and(|m| m.kind == kind) {
            self.menu = None;
            return;
        }
        let ids = &self.ids[band];
        let (items, id): (&'static [&'static str], &str) = match kind {
            MenuKind::Type => (&BandType::NAMES, &ids.kind),
            MenuKind::Slope => (SLOPE.choices, &ids.slope),
            MenuKind::Stereo => (&Placement::NAMES, &ids.stereo),
        };
        let current = Self::plain(host, id).round().max(0.0) as usize;
        self.menu = Some(Menu { kind, band, anchor, items, current, hover: None });
    }

    /// Routes an event to the open dropdown. It is modal: everything is consumed, and a click
    /// outside closes it.
    fn menu_event(&mut self, event: &UiEvent, host: &dyn ParamHost) -> bool {
        let Some(menu) = &mut self.menu else { return false };
        let item_at = |p: Point| menu.item_rects().iter().position(|r| r.contains(p));
        match *event {
            UiEvent::PointerMove { pos, .. } => {
                menu.hover = item_at(pos);
                self.pointer = Some(pos);
            }
            UiEvent::PointerDown { pos, button: MouseButton::Left, .. } => {
                if let Some(i) = item_at(pos) {
                    let ids = &self.ids[menu.band];
                    let id = match menu.kind {
                        MenuKind::Type => &ids.kind,
                        MenuKind::Slope => &ids.slope,
                        MenuKind::Stereo => &ids.stereo,
                    };
                    Self::set(host, id, i as f32);
                    if menu.kind == MenuKind::Type {
                        // Cuts are Butterworth (flat) at Q 0.71; a Q carried over from a bell would
                        // add a resonant bump.
                        if BandType::from_index(i).has_slope() {
                            Self::set(host, &ids.q, std::f32::consts::FRAC_1_SQRT_2);
                        }
                        self.panel_knobs.clear();
                    }
                }
                // A pick, a click outside, or the button again: all close the menu.
                self.menu = None;
            }
            UiEvent::PointerDown { .. } => self.menu = None,
            UiEvent::PointerLeft => menu.hover = None,
            _ => {}
        }
        true
    }

    /// Empty state (mockup "EqEmpty"): an invitation in the middle of the display.
    fn draw_empty_state(&self, cx: &mut DrawCx<'_>) {
        let theme = cx.theme;
        let c = Point::new(PLOT_W / 2.0, DISPLAY_TOP + 290.0);
        let mut icon = BezPath::new();
        icon.move_to((c.x - 30.0, c.y - 22.0));
        icon.curve_to((c.x - 12.0, c.y - 22.0), (c.x - 8.0, c.y - 44.0), (c.x, c.y - 44.0));
        icon.curve_to((c.x + 8.0, c.y - 44.0), (c.x + 12.0, c.y - 22.0), (c.x + 30.0, c.y - 22.0));
        let dashed = Stroke::new(2.4).with_caps(mite_ui::kurbo::Cap::Round).with_dashes(0.0, [5.0, 5.0]);
        if theme.shadows {
            cx.scene.stroke(&dashed.clone().with_dashes(0.0, [5.0, 5.0]), Affine::IDENTITY, theme.accent.multiply_alpha(0.25), None, &icon);
        }
        cx.scene.stroke(&dashed, Affine::IDENTITY, theme.accent, None, &icon);
        cx.text.draw(cx.scene, "Double-click anywhere to add a band", c.x, c.y - 8.0, TextStyle::TITLE.sized(18.0), theme.text_hi, Align::Center);
        cx.text.draw(cx.scene, "then drag it · Shift for fine · Alt-drag or wheel for width", c.x, c.y + 20.0, TextStyle::BODY.sized(13.0), theme.text_mid, Align::Center);
    }

    fn draw_footer(&self, cx: &mut DrawCx<'_>) {
        let theme = cx.theme;
        let top = FOOTER_TOP;
        cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.window, None, &Rect::new(0.0, top, WIDTH, HEIGHT));
        cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, theme.line, None, &Line::new((0.0, top + 0.5), (WIDTH, top + 0.5)));

        // Processing mode: only Zero Latency exists in this slice; the others are shown disabled.
        let track = Rect::new(12.0, top + 10.0, 12.0 + 364.0, top + 46.0);
        panel(cx.scene, RoundedRect::from_rect(track, 8.0), theme.well, theme.well_border);
        let mut x = track.x0 + 2.0;
        for (i, (label, w)) in [("Zero Latency", 118.0), ("Natural Phase", 118.0), ("Linear Phase", 120.0)].iter().enumerate() {
            let r = Rect::new(x, track.y0 + 2.0, x + w, track.y1 - 2.0);
            if i == 0 {
                raised(cx.scene, RoundedRect::from_rect(r, 6.0), theme.control, theme.control_border, theme.shadows);
            }
            let color = if i == 0 { theme.text_strong } else { theme.text_disabled };
            cx.text.draw(cx.scene, label, r.center().x, r.y0 + 9.0, TextStyle::BODY.weighted(700.0), color, Align::Center);
            x += w + 2.0;
        }
        cx.text.draw(cx.scene, "LATENCY", 392.0, top + 15.0, TextStyle::LABEL.sized(9.0), theme.text_lo, Align::Left);
        cx.text.draw(cx.scene, "0 ms", 392.0, top + 28.0, TextStyle::READOUT.sized(11.0), theme.text_hi, Align::Left);

        let plain = cx.host.plain(OUT_GAIN.id, cx.host.normalized(OUT_GAIN.id));
        cx.text.draw(cx.scene, "OUTPUT", 1218.0, top + 15.0, TextStyle::LABEL.sized(9.0), theme.text_lo, Align::Left);
        cx.text.draw(cx.scene, &OUT_GAIN.format(plain), 1218.0, top + 28.0, TextStyle::READOUT.sized(11.0), theme.text_hi, Align::Left);
    }

    fn draw_header(&self, cx: &mut DrawCx<'_>) {
        let theme = cx.theme;
        cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.window, None, &Rect::new(0.0, 0.0, WIDTH, HEADER));
        cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, theme.line, None, &Line::new((0.0, HEADER - 0.5), (WIDTH, HEADER - 0.5)));
        draw_mark(cx, 16.0, 15.0, 26.0);
        draw_title(cx, 52.0, 18.0, "EQ");
    }

    fn draw_help(&self, cx: &mut DrawCx<'_>) {
        let theme = cx.theme;
        cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.help_strip, None, &Rect::new(0.0, HELP_TOP, WIDTH, FOOTER_TOP));
        cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, theme.line, None, &Line::new((0.0, HELP_TOP + 0.5), (WIDTH, HELP_TOP + 0.5)));
        let text = self.help_text(cx.host);
        cx.text.draw(cx.scene, &text, 14.0, HELP_TOP + 7.0, TextStyle::BODY.sized(11.0), theme.text_mid, Align::Left);
    }
}

impl View for EqView {
    fn logical_size(&self) -> (f64, f64) {
        (WIDTH, HEIGHT)
    }

    fn zoom(&self) -> f64 {
        ZOOM
    }

    fn draw(&mut self, cx: &mut DrawCx<'_>) {
        let theme = cx.theme;
        let bands: Vec<BandSettings> = (0..BANDS).map(|b| self.band(cx.host, b)).collect();
        self.draw_header(cx);
        cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.display, None, &Rect::new(0.0, DISPLAY_TOP, WIDTH, HELP_TOP));
        self.draw_grid(cx);
        cx.scene.push_clip_layer(Fill::NonZero, Affine::IDENTITY, &Rect::new(0.0, DISPLAY_TOP, PLOT_W, HELP_TOP));
        self.draw_spectrum(cx);
        self.draw_curves(cx, &bands);
        cx.scene.pop_layer();
        self.draw_nodes(cx, &bands);
        if !bands.iter().any(|b| b.enabled) {
            self.draw_empty_state(cx);
        }
        self.meter.draw(cx);
        let r = self.meter.rect;
        cx.text.draw(cx.scene, "OUT", r.center().x, DISPLAY_TOP + 596.0, TextStyle::LABEL.sized(9.0), theme.text_lo, Align::Center);
        self.draw_panel(cx, &bands);
        self.draw_help(cx);
        self.draw_footer(cx);
        self.out_knob.draw(cx);
        self.draw_menu(cx);
    }

    fn event(&mut self, event: &UiEvent, host: &dyn ParamHost) -> bool {
        if self.menu.is_some() {
            return self.menu_event(event, host);
        }
        // Footer output dial.
        if self.out_knob.event(event, host) {
            return true;
        }
        let in_panel = |hits: &Option<PanelHits>, p: Point| hits.is_some_and(|h| h.panel.contains(p));

        match *event {
            UiEvent::PointerMove { pos, mods } => {
                self.pointer = Some(pos);
                if let Some(d) = &mut self.drag {
                    let factor = if mods.fine() { 0.1 } else { 1.0 };
                    let (dx, dy) = (pos.x - d.last.x, pos.y - d.last.y);
                    d.last = pos;
                    let ids = &self.ids[d.band];
                    match d.mode {
                        DragMode::Move => {
                            d.log_freq = (d.log_freq + dx * factor * (FREQ_MAX / FREQ_MIN).ln() / PLOT_W)
                                .clamp(FREQ_MIN.ln(), FREQ_MAX.ln());
                            host.set_normalized(&ids.freq, host.normalize(&ids.freq, d.log_freq.exp() as f32));
                            let kind = BandType::from_index(Self::plain(host, &ids.kind).round() as usize);
                            if kind.has_gain() {
                                d.gain = (d.gain - dy * factor / PX_PER_DB).clamp(-30.0, 30.0);
                                host.set_normalized(&ids.gain, host.normalize(&ids.gain, d.gain as f32));
                            }
                        }
                        DragMode::Width => {
                            // interaction.md: one octave of width per 120 px; dragging up narrows.
                            d.log_q = (d.log_q - dy * factor * LN_2 / 120.0).clamp(0.025f64.ln(), 40f64.ln());
                            host.set_normalized(&ids.q, host.normalize(&ids.q, d.log_q.exp() as f32));
                        }
                    }
                    return true;
                }
                let mut changed = false;
                for k in &mut self.panel_knobs {
                    changed |= k.event(event, host);
                }
                let hover = if in_panel(&self.panel_hits, pos) { None } else { self.node_at(host, pos) };
                changed |= hover != self.hover;
                self.hover = hover;
                // The help strip follows the pointer over the display.
                changed || (pos.x < PLOT_W && pos.y > DISPLAY_TOP && pos.y < HELP_TOP)
            }
            UiEvent::PointerDown { pos, button: MouseButton::Left, mods } => {
                if let (Some(hits), Some(b)) = (self.panel_hits, self.selected) {
                    if hits.panel.contains(pos) {
                        let ids = &self.ids[b];
                        if hits.kind.contains(pos) {
                            self.toggle_menu(MenuKind::Type, b, hits.kind, host);
                        } else if let Some(r) = hits.slope.filter(|r| r.contains(pos)) {
                            self.toggle_menu(MenuKind::Slope, b, r, host);
                        } else if hits.stereo.contains(pos) {
                            self.toggle_menu(MenuKind::Stereo, b, hits.stereo, host);
                        } else if hits.bypass.contains(pos) {
                            Self::set(host, &ids.on, 0.0);
                            self.select(None);
                        } else if hits.close.contains(pos) {
                            self.select(None);
                        } else {
                            for k in &mut self.panel_knobs {
                                k.event(event, host);
                            }
                        }
                        return true;
                    }
                }
                if let Some(b) = self.node_at(host, pos) {
                    if self.selected != Some(b) {
                        self.select(Some(b));
                    }
                    let s = self.band(host, b);
                    let mode = if mods.alt { DragMode::Width } else { DragMode::Move };
                    let ids = &self.ids[b];
                    match mode {
                        DragMode::Move => {
                            host.begin_gesture(&ids.freq);
                            if s.kind.has_gain() {
                                host.begin_gesture(&ids.gain);
                            }
                        }
                        DragMode::Width => host.begin_gesture(&ids.q),
                    }
                    self.drag = Some(NodeDrag { band: b, mode, last: pos, log_freq: s.freq.ln(), gain: s.gain_db, log_q: s.q.ln() });
                    return true;
                }
                if pos.y > DISPLAY_TOP && pos.y < HELP_TOP && self.selected.is_some() {
                    self.select(None);
                    return true;
                }
                false
            }
            UiEvent::PointerUp { button: MouseButton::Left, .. } => {
                if let Some(d) = self.drag.take() {
                    let ids = &self.ids[d.band];
                    match d.mode {
                        DragMode::Move => {
                            host.end_gesture(&ids.freq);
                            let kind = BandType::from_index(Self::plain(host, &ids.kind).round() as usize);
                            if kind.has_gain() {
                                host.end_gesture(&ids.gain);
                            }
                        }
                        DragMode::Width => host.end_gesture(&ids.q),
                    }
                    return true;
                }
                let mut changed = false;
                for k in &mut self.panel_knobs {
                    changed |= k.event(event, host);
                }
                changed
            }
            UiEvent::PointerDown { pos, button: MouseButton::Right, .. } => {
                // interaction.md puts delete in the band menu; until menus exist, right-click deletes.
                if let Some(b) = self.node_at(host, pos) {
                    Self::set(host, &self.ids[b].on, 0.0);
                    if self.selected == Some(b) {
                        self.select(None);
                    }
                    return true;
                }
                false
            }
            UiEvent::DoubleClick { pos, .. } => {
                if in_panel(&self.panel_hits, pos) {
                    let mut changed = false;
                    for k in &mut self.panel_knobs {
                        changed |= k.event(event, host);
                    }
                    return changed;
                }
                if let Some(b) = self.node_at(host, pos) {
                    // Resets the band: 0 dB, Q 0.71 (interaction.md).
                    let ids = &self.ids[b];
                    Self::set(host, &ids.gain, 0.0);
                    Self::set(host, &ids.q, std::f32::consts::FRAC_1_SQRT_2);
                    return true;
                }
                if pos.x < PLOT_W && pos.y > PLOT_TOP && pos.y < PLOT_BOT {
                    self.add_band(host, pos);
                    return true;
                }
                false
            }
            UiEvent::Wheel { pos, lines, mods } => {
                if in_panel(&self.panel_hits, pos) {
                    let mut changed = false;
                    for k in &mut self.panel_knobs {
                        changed |= k.event(event, host);
                    }
                    return changed;
                }
                if let Some(b) = self.node_at(host, pos).or(self.selected) {
                    let s = self.band(host, b);
                    if s.kind.has_slope() {
                        // Cuts: the wheel steps the slope, up = steeper.
                        self.slope_wheel += lines;
                        let steps = self.slope_wheel.trunc();
                        if steps == 0.0 {
                            return false;
                        }
                        self.slope_wheel -= steps;
                        let next = (s.slope as f32 + steps).clamp(0.0, (SLOPE.choices.len() - 1) as f32);
                        Self::set(host, &self.ids[b].slope, next);
                        return true;
                    }
                    let id = &self.ids[b].q;
                    let q = Q.wheel(Self::plain(host, id), lines, mods.fine());
                    Self::set(host, id, q);
                    return true;
                }
                false
            }
            UiEvent::PointerLeft => {
                self.pointer = None;
                self.hover = None;
                true
            }
            _ => false,
        }
    }

    fn tick(&mut self, host: &dyn ParamHost) -> bool {
        self.ticks = self.ticks.wrapping_add(1);
        let (l, r) = self.shared.meter.load();
        let mut dirty = self.meter.set_levels(l, r);
        // The analyzer updates every UI tick so motion is as smooth as the display allows.
        dirty |= self.update_spectrum();
        // Host-side changes (automation, presets, undo) since the last frame, checked at frame
        // rate rather than every tick.
        if self.last_signature_check.elapsed() < std::time::Duration::from_millis(16) {
            return dirty;
        }
        self.last_signature_check = Instant::now();
        let mut signature = Vec::with_capacity(1 + BANDS * 7);
        signature.push(host.normalized(OUT_GAIN.id));
        for ids in &self.ids {
            for id in [&ids.on, &ids.kind, &ids.freq, &ids.gain, &ids.q, &ids.slope, &ids.stereo] {
                signature.push(host.normalized(id));
            }
        }
        if signature != self.signature {
            self.signature = signature;
            dirty = true;
        }
        dirty
    }
}
