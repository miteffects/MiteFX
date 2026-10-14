//! The Mite EQ editor (docs/design/mockups/eq-main.dc.html), laid out in the mockup's 1280×760
//! design units and shown at [`ZOOM`].
//!
//! Gestures follow docs/design/interaction.md, "Main display gestures".

use std::f64::consts::LN_2;
use std::sync::Arc;
use std::time::Instant;

use mite_dsp::analyzer::Analyzer;
use mite_dsp::eq::{design, BandSettings, BandType, FREQ_MAX, FREQ_MIN};
use mite_ui::kurbo::{Affine, BezPath, Circle, Line, Point, Rect, RoundedRect, Stroke};
use mite_ui::paint::{glow_stroke, panel};
use mite_ui::peniko::{Color, Fill, Gradient};
use mite_ui::widgets::brand::{draw_mark, draw_title};
use mite_ui::widgets::{Knob, KnobKind, LevelMeter};
use mite_ui::{Align, DrawCx, MouseButton, ParamHost, TextStyle, UiEvent, View};

use crate::params::{band_id, BANDS, FREQ, GAIN, ON, OUT_GAIN, Q, SLOPE, TYPE};
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
const PANEL_W: f64 = 476.0;
const PANEL_H: f64 = 158.0;

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

/// Clickable rectangles of the open band panel, set while drawing.
#[derive(Default, Clone, Copy)]
struct PanelHits {
    panel: Rect,
    kind: Rect,
    slope: Option<Rect>,
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
    out_knob: Knob,
    meter: LevelMeter,
    analyzer: Analyzer,
    samples: Vec<f32>,
    bins: Vec<f32>,
    power: Vec<f32>,
    pre: Vec<f64>,
    post: Vec<f64>,
    last_spectrum: Instant,
    responses: Vec<Option<(BandSettings, Vec<f64>)>>,
    signature: Vec<f32>,
    ticks: u32,
}

impl EqView {
    pub fn new(shared: Arc<EqShared>) -> Self {
        let analyzer = Analyzer::new(ANALYZER_SIZE);
        let bins = vec![-150.0; analyzer.bins()];
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
            out_knob,
            meter: LevelMeter::new(Rect::new(1233.5, DISPLAY_TOP + 19.5, 1266.5, DISPLAY_TOP + 592.5)),
            analyzer,
            samples: vec![0.0; ANALYZER_SIZE],
            power: vec![0.0; bins.len()],
            bins,
            pre: vec![-150.0; POINTS],
            post: vec![-150.0; POINTS],
            last_spectrum: Instant::now(),
            responses: (0..BANDS).map(|_| None).collect(),
            signature: Vec::new(),
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

    fn band_color(&self, theme: &mite_ui::Theme, b: usize) -> Color {
        theme.band_ramp[b % theme.band_ramp.len()]
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
        self.panel_knobs.clear();
        self.panel_hits = None;
    }

    fn sample_rate(&self) -> f64 {
        f64::from(self.shared.sample_rate.load()).max(8000.0)
    }

    /// Response of band `b` in dB at every display point, cached until its settings change.
    fn response(&mut self, b: usize, s: &BandSettings) -> &[f64] {
        let stale = !matches!(&self.responses[b], Some((cached, _)) if cached == s);
        if stale {
            let fs = self.sample_rate();
            let sections = design(s, fs);
            let curve = (0..POINTS)
                .map(|i| {
                    let f = xf(i as f64 * PLOT_W / (POINTS - 1) as f64);
                    if f >= fs * 0.5 { f64::NAN } else { sections.response_db(2.0 * std::f64::consts::PI * f / fs) }
                })
                .collect();
            self.responses[b] = Some((*s, curve));
        }
        &self.responses[b].as_ref().expect("just filled").1
    }

    fn update_spectrum(&mut self) -> bool {
        let dt = self.last_spectrum.elapsed().as_secs_f64().min(0.1);
        self.last_spectrum = Instant::now();
        let fs = self.sample_rate();
        let size = self.analyzer.size() as f64;
        let mut changed = false;
        for which in 0..2 {
            let tap = if which == 0 { &self.shared.pre } else { &self.shared.post };
            tap.read_latest(&mut self.samples);
            self.analyzer.analyze(&self.samples, &mut self.bins);
            let curve = if which == 0 { &mut self.pre } else { &mut self.post };
            // Work in power so averaging is energy-correct.
            for (p, &db) in self.power.iter_mut().zip(&self.bins) {
                *p = 10f32.powf(db * 0.1);
            }
            let rise = 1.0 - (-dt / 0.03).exp();
            for (i, value) in curve.iter_mut().enumerate() {
                let x = i as f64 * PLOT_W / (POINTS - 1) as f64;
                let f = xf(x);
                // 1/12-octave smoothing: average the power of every bin within ±1/24 octave, or
                // interpolate where bins are sparser than that (low frequencies).
                let (lo, hi) = (f * 2f64.powf(-1.0 / 24.0) * size / fs, f * 2f64.powf(1.0 / 24.0) * size / fs);
                let power = if hi.floor() - lo.ceil() < 1.0 {
                    let k = f * size / fs;
                    let k0 = (k.floor() as usize).min(self.power.len() - 2);
                    let t = (k - k0 as f64) as f32;
                    f64::from(self.power[k0] * (1.0 - t) + self.power[k0 + 1] * t)
                } else {
                    let (a, b) = (lo.ceil() as usize, (hi.floor() as usize).min(self.power.len() - 1));
                    let span = &self.power[a.min(b)..=b];
                    f64::from(span.iter().sum::<f32>() / span.len() as f32)
                };
                let db = 10.0 * power.max(1e-15).log10();
                // 4.5 dB/oct tilt around 1 kHz (settings default), so pink noise reads flat.
                let tilted = db + 4.5 * (f / 1000.0).log2();
                // Ballistics: ~30 ms rise, 60 dB/s fall.
                let next = if tilted > *value { *value + (tilted - *value) * rise } else { tilted.max(*value - 60.0 * dt) };
                if (next - *value).abs() > 0.05 {
                    changed = true;
                }
                *value = next;
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
            t + " — drag to move, Shift for fine, Alt-drag or wheel for Q, double-click resets, right-click deletes"
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
        let mut knob = |spec, label, kind, id: &str| {
            let k = Knob::new(spec, label, kind, Point::new(origin.x + 14.0 + 42.0 + col * 88.0, origin.y + 92.0), 64.0)
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

    fn panel_origin(&self, host: &dyn ParamHost, b: usize) -> Point {
        let node = Self::node_pos(&self.band(host, b));
        let x = (node.x - PANEL_W / 2.0).clamp(12.0, PLOT_W - PANEL_W - 12.0);
        let y = if node.y > DISPLAY_TOP + 290.0 { node.y - 30.0 - PANEL_H } else { node.y + 30.0 };
        Point::new(x, y)
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
        let mut total = vec![0.0_f64; POINTS];
        let mut any = false;
        // Curves may leave the plot (the display is clipped); clamping only keeps paths finite.
        let clamp = |db: f64| gy(db.clamp(-40.0, 40.0));
        for (b, s) in bands.iter().enumerate() {
            if !s.enabled {
                continue;
            }
            any = true;
            let color = self.band_color(theme, b);
            let selected = self.selected == Some(b);
            let curve = self.response(b, s).to_vec();
            for (t, v) in total.iter_mut().zip(&curve) {
                *t += v;
            }
            let mut fill = Self::curve_path(&curve, clamp);
            let last_x = curve.iter().rposition(|v| v.is_finite()).map_or(PLOT_W, |i| i as f64 * PLOT_W / (POINTS - 1) as f64);
            fill.line_to((last_x, MID));
            fill.line_to((0.0, MID));
            fill.close_path();
            cx.scene.fill(Fill::NonZero, Affine::IDENTITY, color.multiply_alpha(if selected { 0.22 } else { 0.08 }), None, &fill);
        }
        let curve = if any { Self::curve_path(&total, clamp) } else {
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
            let color = self.band_color(theme, b);
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
        let origin = self.panel_origin(cx.host, b);
        if self.panel_knobs.is_empty() || self.panel_knobs[0].center.x != origin.x + 56.0 || self.panel_knobs[0].center.y != origin.y + 92.0
            || self.panel_knobs.len() != if s.kind.has_gain() { 3 } else { 2 }
        {
            self.build_panel_knobs(cx.host, origin);
        }
        let rect = Rect::new(origin.x, origin.y, origin.x + PANEL_W, origin.y + PANEL_H);
        let shape = RoundedRect::from_rect(rect, 14.0);
        if theme.shadows {
            cx.scene.draw_blurred_rounded_rect(Affine::translate((0.0, 18.0)), rect, Color::from_rgba8(0, 0, 0, 140), 14.0, 24.0);
        }
        // Backdrop blur is a later shader pass; the glass tint is drawn nearly opaque until then.
        panel(cx.scene, shape, theme.glass.with_alpha(0.94), theme.glass_border);

        let color = self.band_color(theme, b);
        let row_y = origin.y + 12.0;
        glow_stroke(cx.scene, &Circle::new((origin.x + 19.0, row_y + 16.0), 4.0), 2.0, color, color, 4.0);
        cx.scene.fill(Fill::NonZero, Affine::IDENTITY, color, None, &Circle::new((origin.x + 19.0, row_y + 16.0), 5.0));
        let title = format!("Band {}", b + 1);
        let tw = cx.text.draw(cx.scene, &title, origin.x + 30.0, row_y + 8.0, TextStyle::TITLE.sized(13.0), theme.text_hi, Align::Left);

        let button = |cx: &mut DrawCx<'_>, x: f64, w: f64, text: &str| -> Rect {
            let r = Rect::new(x, row_y, x + w, row_y + 32.0);
            panel(cx.scene, RoundedRect::from_rect(r, 6.0), cx.theme.panel_control, cx.theme.panel_control_border);
            cx.text.draw(cx.scene, text, x + w / 2.0, row_y + 9.0, TextStyle::BODY.weighted(600.0), cx.theme.text_hi, Align::Center);
            r
        };
        let mut x = origin.x + 30.0 + f64::from(tw) + 10.0;
        let kind = button(cx, x, 104.0, BandType::NAMES[s.kind as usize]);
        x += 112.0;
        let slope = s.kind.has_slope().then(|| button(cx, x, 96.0, SLOPE.choices[s.slope.min(SLOPE.choices.len() - 1)]));
        let close = Rect::new(origin.x + PANEL_W - 46.0, row_y, origin.x + PANEL_W - 14.0, row_y + 32.0);
        let bypass = Rect::new(close.x0 - 40.0, row_y, close.x0 - 8.0, row_y + 32.0);
        panel(cx.scene, RoundedRect::from_rect(bypass, 6.0), theme.panel_control, theme.panel_control_border);
        // Power glyph.
        let c = bypass.center();
        let mut glyph = BezPath::new();
        glyph.move_to((c.x, c.y - 5.5));
        glyph.line_to((c.x, c.y - 0.5));
        let arc = mite_ui::kurbo::Arc::new(c + (0.0, 1.0), (4.5, 4.5), -1.0, 5.3, 0.0);
        let round = Stroke::new(1.5).with_caps(mite_ui::kurbo::Cap::Round);
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
        self.panel_hits = Some(PanelHits { panel: rect, kind, slope, bypass, close });
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
                panel(cx.scene, RoundedRect::from_rect(r, 6.0), theme.control, theme.control_border);
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
    }

    fn event(&mut self, event: &UiEvent, host: &dyn ParamHost) -> bool {
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
                            let next = (Self::plain(host, &ids.kind).round() as usize + 1) % BandType::ALL.len();
                            Self::set(host, &ids.kind, next as f32);
                            self.panel_knobs.clear();
                        } else if hits.slope.is_some_and(|r| r.contains(pos)) {
                            let next = (Self::plain(host, &ids.slope).round() as usize + 1) % SLOPE.choices.len();
                            Self::set(host, &ids.slope, next as f32);
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
        // Host-side changes (automation, presets, undo) since the last frame.
        let mut signature = Vec::with_capacity(1 + BANDS * 6);
        signature.push(host.normalized(OUT_GAIN.id));
        for ids in &self.ids {
            for id in [&ids.on, &ids.kind, &ids.freq, &ids.gain, &ids.q, &ids.slope] {
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
