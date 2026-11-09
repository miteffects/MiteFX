//! The Mite Verb editor (docs/design/verb-brief.md §2), in 1280×760 design units.
//!
//! The plot on top holds both of the reverb's EQs over the input's and the reverb's spectra:
//! the decay EQ (how long each frequency rings, `mite_dsp::reverb::DecayModel`, on a log
//! seconds scale) and the tone EQ (the reverb's level per frequency, ±24 dB about the middle).
//! Neither has a band until one is made (user, 2026-11-04: "fully flexible eq like eq plugin …
//! dont make default bands"). The two glyph pills top left pick which one the pointer edits; the
//! other stays drawn, dim, and grabbing one of its bands picks it. The decay curve is drawn and
//! dragged against `DecayModel` (one mean line, designed once per change: a drag costs well under
//! a millisecond; the exact 32-line mean froze the UI). Under the plot the dials, large, grouped by
//! what they do (SPACE, DUCK; EARLY, SHIMMER, MOD); the meters on the right. No level history
//! (user, 2026-11-09: "it works without that graph"): the ducking threshold is a dial with the
//! input on its ring.

mod spectrum;

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Instant;

use mite_core::{ParamDisplay, ParamScale, ParamSpec};
use mite_dsp::eq::{analog_response_db, BandSettings, BandType, SLOPES_DB};
use mite_dsp::reverb::{DecayModel, DecayShape, ReverbSettings, DECAY_BANDS, TONE_BANDS};
use mite_ui::widgets::history;
use mite_ui::kurbo::{Affine, BezPath, Cap, Circle, Join, Line, Point, Rect, RoundedRect, Stroke};
use mite_ui::paint::{glow_stroke, panel};
use mite_ui::peniko::{Color, Fill};
use mite_ui::session::{Session, SessionConfig};
use mite_ui::widgets::brand::{draw_mark, draw_title};
use mite_ui::widgets::eq_icons::draw_type_icon;
use mite_ui::widgets::{Dropdown, DropdownEvent, Footer, FooterDial, FooterToggle, Help, Knob, KnobKind, ToggleGlyph};
use mite_ui::{write_if_changed, Align, DrawCx, MouseButton, ParamHost, RecordingHost, TextStyle, UiEvent, View};

use crate::dsp::{record, HISTORY_RECORDS};
use crate::params::*;
use crate::presets::{preset_ids, FACTORY, PLUGIN};
use crate::VerbShared;
use spectrum::{freq_x, Spectrum};

/// Mite Verb wears the Verb accent (tokens.json `color.accent.verb`, ice).
pub const ACCENT: &str = "verb";

const VIEW_W: f64 = 1280.0;
const VIEW_H: f64 = 760.0;
const HEADER: f64 = 56.0;
const DISPLAY_TOP: f64 = HEADER;
const DISPLAY_H: f64 = 620.0;
const HELP_TOP: f64 = DISPLAY_TOP + DISPLAY_H;
const FOOTER_TOP: f64 = HELP_TOP + 28.0;

/// The plot.
const PLOT: Rect = Rect::new(0.0, DISPLAY_TOP, 1180.0, DISPLAY_TOP + 392.0);
pub(crate) const PLOT_X0: f64 = 28.0;
pub(crate) const PLOT_W: f64 = 1124.0;
/// The decay axis: 0.1…30 s, log, between these.
const DECAY_TOP: f64 = PLOT.y0 + 26.0;
const DECAY_BOTTOM: f64 = PLOT.y1 - 20.0;
const DECAY_MIN: f64 = 0.1;
const DECAY_MAX: f64 = 30.0;
/// The tone axis: ±this about the middle of the decay axis.
const TONE_DB: f64 = 24.0;
const CURVE_POINTS: usize = 240;
const GRIP_R: f64 = 8.0;
const GRIP_HIT: f64 = 14.0;
/// The layer pills, top left of the plot.
const PILL: (f64, f64) = (38.0, 24.0);
const PILL_X: [f64; 2] = [44.0, 88.0];
const PILL_Y: f64 = PLOT.y0 + 8.0;

/// The dials' area under the plot, and the meters' range (dB).
const DIALS_TOP: f64 = PLOT.y1 + 8.0;
const DIALS_BOTTOM: f64 = HELP_TOP - 4.0;
const METER_RANGE: f64 = 60.0;

/// The dials, grouped: two rows of panels, each as wide as its dials need.
const DIAL_D: f64 = 64.0;
/// The dials' span (x) and the gap between panels.
const GROUPS_X: (f64, f64) = (8.0, 1172.0);
const GROUP_GAP: f64 = 8.0;
const GROUP_ROWS: [(f64, f64); 2] = [(DIALS_TOP, DIALS_TOP + 104.0), (DIALS_BOTTOM - 104.0, DIALS_BOTTOM)];
/// A dial's centre below its panel's top.
const DIAL_DY: f64 = 55.0;

const WELLS: [f64; 2] = [1206.0, 1238.0];
const METER_TOP: f64 = DISPLAY_TOP + 20.0;
const METER_BOTTOM: f64 = DISPLAY_TOP + 592.0;
const METER_RELEASE_DB_S: f32 = 20.0;
const SILENT: f32 = -150.0;

/// The shape menus: every shape, then Remove.
const DECAY_MENU: [&str; 5] = ["Bell", "Low Shelf", "High Shelf", "Tilt", "Remove"];
const TONE_MENU: [&str; 11] = ["Bell", "Low Shelf", "High Shelf", "Low Cut", "High Cut", "Notch", "Band Pass", "Tilt Shelf", "Flat Tilt", "All Pass", "Remove"];

/// The frequency at `x` on the plot.
fn x_freq(x: f64) -> f64 {
    20.0 * 1000f64.powf((x - PLOT_X0) / PLOT_W)
}

/// A decay time's y on the plot (log, clamped to the axis).
fn decay_y(t: f64) -> f64 {
    let k = (t.clamp(DECAY_MIN, DECAY_MAX) / DECAY_MIN).ln() / (DECAY_MAX / DECAY_MIN).ln();
    DECAY_BOTTOM - k * (DECAY_BOTTOM - DECAY_TOP)
}

fn y_decay(y: f64) -> f64 {
    let k = (DECAY_BOTTOM - y) / (DECAY_BOTTOM - DECAY_TOP);
    DECAY_MIN * (DECAY_MAX / DECAY_MIN).powf(k)
}

/// A tone gain's y on the plot (0 dB in the middle).
fn tone_y(db: f64) -> f64 {
    let mid = 0.5 * (DECAY_TOP + DECAY_BOTTOM);
    mid - db.clamp(-TONE_DB, TONE_DB) / TONE_DB * 0.5 * (DECAY_BOTTOM - DECAY_TOP)
}

fn y_tone(y: f64) -> f64 {
    let mid = 0.5 * (DECAY_TOP + DECAY_BOTTOM);
    (mid - y) / (0.5 * (DECAY_BOTTOM - DECAY_TOP)) * TONE_DB
}




fn fmt_s(t: f64) -> String {
    if t >= 10.0 {
        format!("{t:.1} s")
    } else {
        format!("{t:.2} s")
    }
}

fn fmt_hz(hz: f64) -> String {
    if hz >= 1000.0 {
        format!("{:.2} kHz", hz / 1000.0)
    } else {
        format!("{hz:.0} Hz")
    }
}

/// A stand-in host holding every Mite Verb parameter at its default (tests, preview, tools).
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
            "choice" => index(id, f[2].split(", ").count(), f[3].parse().unwrap_or(0)),
            _ => ParamSpec { id, ..spec_for(id).expect("every manifest line has a spec") },
        };
        host = host.with(spec);
    }
    host
}

/// The reverb's settings as a host's parameters set them (as `VerbParams::settings` does).
pub fn settings(host: &dyn ParamHost) -> ReverbSettings {
    reverb_settings(|id| host.plain(id, host.normalized(id)))
}

/// The output stage as a host's parameters set it.
pub fn output(host: &dyn ParamHost) -> crate::dsp::Output {
    let p = |id: &str| host.plain(id, host.normalized(id));
    crate::dsp::Output { mix: p(MIX.id) / 100.0, gain_db: p(OUT_GAIN.id), bypass: p(BYPASS.id) >= 0.5 }
}

/// Which EQ the plot edits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layer {
    Decay,
    Tone,
}

/// A grip on the plot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Node {
    /// A decay band (from 0), while it is on.
    Decay(usize),
    /// A tone band (from 0), while it is on.
    Tone(usize),
}

impl Node {
    fn layer(self) -> Layer {
        match self {
            Node::Tone(_) => Layer::Tone,
            Node::Decay(_) => Layer::Decay,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Drag {
    /// A grip: where it was grabbed and where it sat, and a decay band's multiplier then.
    Node { node: Node, start: Point, at: Point, mult: f32 },
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Hot {
    Node(Node),
    Layer(Layer),
}

/// What an open shape menu is for.
#[derive(Debug, Clone, Copy, PartialEq)]
enum MenuFor {
    /// A band's shape (or its removal).
    Band(Node),
    /// A new band of the picked shape where the plot was right-clicked.
    New(Layer, Point),
}

/// A panel of dials that work together.
struct Group {
    name: &'static str,
    words: &'static str,
    rect: Rect,
}

pub struct VerbView {
    shared: Arc<VerbShared>,
    session: Session,
    footer: Footer,
    knobs: Vec<Knob>,
    groups: Vec<Group>,
    spectrum: Spectrum,
    layer: Layer,
    /// The decay curve and the tone curve as last worked out, and what for.
    curve: Vec<f64>,
    curve_for: Option<(ReverbSettings, f32)>,
    /// The decay model for the settings it was made from (grips ask it many times an event).
    model: std::cell::RefCell<Option<(ReverbSettings, DecayModel)>>,
    tone: Vec<f64>,
    tone_for: Option<[BandSettings; TONE_BANDS]>,
    pointer: Option<Point>,
    hot: Option<Hot>,
    drag: Option<Drag>,
    menu: Option<(MenuFor, Dropdown)>,
    history: VecDeque<[f32; record::WIDTH]>,
    seen: usize,
    /// IN L/R, OUT L/R as drawn.
    meters: [f32; 4],
    last_fresh: Instant,
    last_tick: Instant,
}

impl VerbView {
    pub fn new(shared: Arc<VerbShared>) -> Self {
        let footer = Footer::new(
            FOOTER_TOP,
            (VIEW_W, VIEW_H),
            vec![
                FooterToggle { id: FREEZE.id, glyph: ToggleGlyph::Freeze, words: "Freeze — hold the tail as it is, forever; new sound stays out" },
                FooterToggle { id: BYPASS.id, glyph: ToggleGlyph::Bypass, words: "Bypass — the input alone" },
            ],
            vec![
                FooterDial { spec: MIX, kind: KnobKind::Unipolar, name: "MIX", words: "Mix — the reverb against the dry signal (100 %: the reverb alone)" },
                FooterDial { spec: OUT_GAIN, kind: KnobKind::Bipolar, name: "OUTPUT", words: "Output gain" },
            ],
        );
        type Dial = (ParamSpec, &'static str, Option<mite_core::DialCurve>);
        // Two rows; each panel as wide as its dials need.
        let layout: [(&'static str, &'static str, usize, Vec<Dial>); 5] = [
            ("SPACE", "the room itself", 0, vec![(SIZE, "SIZE", None), (DECAY, "DECAY", None), (DIFFUSION, "DIFFUSION", None), (WIDTH, "WIDTH", None), (PREDELAY, "PREDELAY", Some(PREDELAY_DIAL))]),
            ("DUCK", "the reverb stepping back while the input plays: the ring on THRESHOLD is the input, lit where it is over", 0, vec![(DUCK, "DEPTH", None), (DUCK_THRESH, "THRESHOLD", None), (DUCK_RELEASE, "RELEASE", None)]),
            ("EARLY", "the first reflections", 1, vec![(EARLY, "LEVEL", None), (EARLY_SIZE, "SIZE", None), (EARLY_SEND, "SEND", None)]),
            ("SHIMMER", "the tail climbing by an interval, pass after pass; LOW and HIGH bound what climbs", 1, vec![(SHIMMER, "FEEDBACK", None), (SHIMMER_PITCH, "PITCH", None), (SHIMMER_LOW, "LOW", None), (SHIMMER_HIGH, "HIGH", None)]),
            ("MOD", "the room's slow drift", 1, vec![(MOD_DEPTH, "DEPTH", None), (MOD_RATE, "RATE", None)]),
        ];
        let mut rects = Vec::new();
        for (row, &(top, bottom)) in GROUP_ROWS.iter().enumerate() {
            let in_row: Vec<usize> = (0..layout.len()).filter(|&g| layout[g].2 == row).collect();
            let dials: usize = in_row.iter().map(|&g| layout[g].3.len()).sum();
            let per = (GROUPS_X.1 - GROUPS_X.0 - GROUP_GAP * (in_row.len() - 1) as f64) / dials as f64;
            let mut x = GROUPS_X.0;
            for &g in &in_row {
                let w = per * layout[g].3.len() as f64;
                rects.push((g, Rect::new(x, top, x + w, bottom)));
                x += w + GROUP_GAP;
            }
        }
        rects.sort_by_key(|r| r.0);
        let mut knobs = Vec::new();
        let mut groups = Vec::new();
        for ((name, words, _, dials), (_, rect)) in layout.into_iter().zip(rects) {
            let n = dials.len() as f64;
            for (i, (spec, label, curve)) in dials.into_iter().enumerate() {
                let at = Point::new(rect.x0 + rect.width() * (i as f64 + 0.5) / n, rect.y0 + DIAL_DY);
                let kind = if spec.min < 0.0 && spec.max > 0.0 { KnobKind::Bipolar } else { KnobKind::Unipolar };
                let k = Knob::new(spec, label, kind, at, DIAL_D);
                knobs.push(match curve {
                    Some(c) => k.with_curve(c),
                    None => k,
                });
            }
            groups.push(Group { name, words, rect });
        }
        Self {
            session: Session::new(SessionConfig { plugin: PLUGIN, factory: FACTORY, sound_ids: preset_ids(), snapshot_ids: preset_ids() }, shared.snapshots.clone(), shared.preset.clone()),
            footer,
            knobs,
            groups,
            spectrum: Spectrum::new(),
            layer: Layer::Decay,
            curve: vec![0.0; CURVE_POINTS],
            curve_for: None,
            model: std::cell::RefCell::new(None),
            tone: vec![0.0; CURVE_POINTS],
            tone_for: None,
            pointer: None,
            hot: None,
            drag: None,
            menu: None,
            history: VecDeque::with_capacity(HISTORY_RECORDS),
            seen: 0,
            meters: [SILENT; 4],
            last_fresh: Instant::now(),
            last_tick: Instant::now(),
            shared,
        }
    }

    pub fn with_preset_dir(mut self, dir: std::path::PathBuf) -> Self {
        self.session = self.session.with_preset_dir(dir);
        self
    }

    pub fn session(&self) -> &Session {
        &self.session
    }

    /// Reads the spectra at once (tools render a single frame).
    pub fn settle_spectrum(&mut self) {
        for _ in 0..60 {
            self.spectrum.update(&self.shared, 0.3);
            std::thread::sleep(std::time::Duration::from_millis(16));
        }
    }

    // ---- geometry for tests and tools -------------------------------------------------------

    fn fs(&self) -> f64 {
        f64::from(self.shared.sample_rate.load()).max(8000.0)
    }

    fn curve_freq(i: usize) -> f64 {
        x_freq(PLOT_X0 + PLOT_W * i as f64 / (CURVE_POINTS - 1) as f64)
    }

    /// The EQ the plot edits.
    pub fn layer(&self) -> Layer {
        self.layer
    }

    /// The pill that picks `layer`.
    pub fn layer_pill(&self, layer: Layer) -> Rect {
        let x = PILL_X[usize::from(layer == Layer::Tone)];
        Rect::new(x, PILL_Y, x + PILL.0, PILL_Y + PILL.1)
    }

    /// The open shape menu's rows (tests and tools).
    pub fn menu_rows(&self) -> Vec<Rect> {
        self.menu.as_ref().map(|(_, m)| m.item_rects()).unwrap_or_default()
    }

    /// The dial groups: name and panel.
    pub fn dial_groups(&self) -> Vec<(&'static str, Rect)> {
        self.groups.iter().map(|g| (g.name, g.rect)).collect()
    }

    /// Where a grip sits.
    pub fn node(&self, host: &dyn ParamHost, node: Node) -> Point {
        let s = settings(host);
        match node {
            Node::Decay(b) => {
                let f = f64::from(s.bands[b].freq);
                Point::new(freq_x(f), decay_y(self.model(&s).at(f)))
            }
            Node::Tone(b) => {
                let band = s.eq[b];
                Point::new(freq_x(band.freq), tone_y(if band.kind.has_gain() { band.gain_db } else { 0.0 }))
            }
        }
    }

    /// The decay model for `s` (cached: made again only when the settings change).
    fn model(&self, s: &ReverbSettings) -> DecayModel {
        let mut cache = self.model.borrow_mut();
        let s = ReverbSettings { freeze: false, ..*s };
        match *cache {
            Some((for_s, m)) if for_s == s => m,
            _ => {
                let m = DecayModel::new(&s, self.fs());
                *cache = Some((s, m));
                m
            }
        }
    }

    /// The grips that exist: each decay band and each tone band that is on. The edited layer's
    /// last, so they win a hit.
    fn nodes(&self, host: &dyn ParamHost) -> Vec<Node> {
        let decay: Vec<Node> = (0..DECAY_BANDS).filter(|&b| Self::on(host, &band_id(b, D_ON.id))).map(Node::Decay).collect();
        let tone: Vec<Node> = (0..TONE_BANDS).filter(|&b| Self::on(host, &tone_id(b, E_ON.id))).map(Node::Tone).collect();
        match self.layer {
            Layer::Decay => tone.into_iter().chain(decay).collect(),
            Layer::Tone => decay.into_iter().chain(tone).collect(),
        }
    }

    pub fn decay_y(&self, t: f64) -> f64 {
        decay_y(t)
    }

    pub fn tone_y(&self, db: f64) -> f64 {
        tone_y(db)
    }


    pub fn dial(&self, id: &str) -> Option<Point> {
        self.knobs.iter().find(|k| k.id == id).map(|k| k.center)
    }

    pub fn footer_toggle(&self, id: &str) -> Option<Rect> {
        self.footer.toggle_rect(id)
    }

    // ---- parameters -------------------------------------------------------------------------

    fn plain(host: &dyn ParamHost, id: &str) -> f32 {
        host.plain(id, host.normalized(id))
    }

    fn on(host: &dyn ParamHost, id: &str) -> bool {
        Self::plain(host, id) >= 0.5
    }

    fn set(host: &dyn ParamHost, id: &str, plain: f32) {
        write_if_changed(host, id, host.normalize(id, plain));
    }

    fn set_raw(host: &dyn ParamHost, id: &str, plain: f32) {
        host.set_normalized(id, host.normalize(id, plain));
    }

    /// Decay band `b`'s multiplier that puts the curve at `target` seconds at `freq` (bisection:
    /// the curve there rises with the multiplier).
    fn solve_mult(&self, s: &ReverbSettings, b: usize, freq: f64, target: f64) -> f32 {
        let (mut lo, mut hi) = (f64::from(D_MULT.min).ln(), f64::from(D_MULT.max).ln());
        let fs = self.fs();
        // 20 halvings: 1/250 000 of the range, far under a pixel.
        for _ in 0..20 {
            let m = 0.5 * (lo + hi);
            let mut t = *s;
            t.bands[b].mult = m.exp() as f32;
            if DecayModel::new(&t, fs).at(freq) < target {
                lo = m;
            } else {
                hi = m;
            }
        }
        (0.5 * (lo + hi)).exp() as f32
    }

    fn frozen(&self, host: &dyn ParamHost) -> bool {
        Self::on(host, FREEZE.id) || self.history.back().is_some_and(|r| r[record::FREEZE] > 0.5)
    }

    fn node_params(node: Node) -> Vec<String> {
        match node {
            Node::Decay(b) => vec![band_id(b, D_FREQ.id), band_id(b, D_MULT.id)],
            Node::Tone(b) => vec![tone_id(b, E_FREQ.id), tone_id(b, E_GAIN.id)],
        }
    }

    // ---- drawing ----------------------------------------------------------------------------

    fn draw_header(&mut self, cx: &mut DrawCx<'_>) {
        let theme = cx.theme;
        cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.window, None, &Rect::new(0.0, 0.0, VIEW_W, HEADER));
        cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, theme.line, None, &Line::new((0.0, HEADER - 0.5), (VIEW_W, HEADER - 0.5)));
        draw_mark(cx, 16.0, 15.0, 26.0);
        draw_title(cx, 52.0, 18.0, "Verb");
        self.session.draw(cx);
    }

    fn update_curves(&mut self, host: &dyn ParamHost) {
        let s = ReverbSettings { freeze: false, ..settings(host) };
        let fs = self.fs();
        if self.curve_for != Some((s, fs as f32)) {
            let m = self.model(&s);
            for (i, v) in self.curve.iter_mut().enumerate() {
                *v = m.at(Self::curve_freq(i));
            }
            self.curve_for = Some((s, fs as f32));
        }
        if self.tone_for != Some(s.eq) {
            for (i, v) in self.tone.iter_mut().enumerate() {
                let f = Self::curve_freq(i);
                *v = s.eq.iter().filter(|b| b.enabled).map(|b| analog_response_db(b, f)).sum();
            }
            self.tone_for = Some(s.eq);
        }
    }

    fn draw_plot(&self, cx: &mut DrawCx<'_>) {
        let theme = cx.theme;
        let host = cx.host;
        let decay_on = self.layer == Layer::Decay;
        cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.well, None, &PLOT);
        // The grid of the edited layer: seconds on the left, or dB on the right.
        let label = TextStyle { tabular: true, ..TextStyle::BODY.sized(10.0).weighted(600.0) };
        if decay_on {
            for (t, s) in [(0.2, "0.2"), (0.5, "0.5"), (1.0, "1 s"), (2.0, "2"), (5.0, "5"), (10.0, "10"), (20.0, "20")] {
                let y = decay_y(t);
                cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, if t == 1.0 || t == 10.0 { theme.grid_major } else { theme.grid_minor }, None, &Line::new((PLOT_X0, y), (PLOT.x1, y)));
                cx.text.draw(cx.scene, s, 6.0, y - 6.0, label, theme.text_lo, Align::Left);
            }
        } else {
            for (db, s) in [(18.0, "+18"), (12.0, "+12"), (6.0, "+6"), (0.0, "0 dB"), (-6.0, "−6"), (-12.0, "−12"), (-18.0, "−18")] {
                let y = tone_y(db);
                cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, if db == 0.0 { theme.grid_major } else { theme.grid_minor }, None, &Line::new((PLOT_X0, y), (PLOT.x1, y)));
                cx.text.draw(cx.scene, s, 6.0, y - 6.0, label, theme.text_lo, Align::Left);
            }
        }
        cx.begin_clip(&PLOT);
        self.spectrum.draw(cx, PLOT);
        let xs: Vec<f64> = (0..CURVE_POINTS).map(|i| freq_x(Self::curve_freq(i))).collect();
        // The tone curve: dim unless edited (and hidden while flat and not edited).
        let tone_flat = self.tone.iter().all(|v| v.abs() < 0.01);
        if !(tone_flat && decay_on) {
            let ink = theme.curve;
            let pts: Vec<Point> = xs.iter().zip(&self.tone).map(|(&x, &db)| Point::new(x, tone_y(db))).collect();
            let path = history::smooth_path(&pts);
            let mut fill = path.clone();
            fill.line_to((pts[pts.len() - 1].x, tone_y(0.0)));
            fill.line_to((pts[0].x, tone_y(0.0)));
            fill.close_path();
            cx.scene.fill(Fill::NonZero, Affine::IDENTITY, ink.multiply_alpha(if decay_on { 0.04 } else { 0.1 }), None, &fill);
            if decay_on {
                cx.scene.stroke(&Stroke::new(1.4), Affine::IDENTITY, ink.multiply_alpha(0.4), None, &path);
            } else {
                glow_stroke(cx, &path, 2.0, ink, ink, 3.0);
            }
        }
        // The decay curve, its area faintly filled; dim while the tone is edited.
        let pts: Vec<Point> = xs.iter().zip(&self.curve).map(|(&x, &t)| Point::new(x, decay_y(t))).collect();
        let path = history::smooth_path(&pts);
        let mut fill = path.clone();
        fill.line_to((pts[pts.len() - 1].x, PLOT.y1));
        fill.line_to((pts[0].x, PLOT.y1));
        fill.close_path();
        let frozen = self.frozen(host);
        let accent = theme.accent;
        cx.scene.fill(Fill::NonZero, Affine::IDENTITY, accent.multiply_alpha(if frozen || !decay_on { 0.04 } else { 0.09 }), None, &fill);
        if frozen {
            // Frozen, the decay is endless: the line at the top; the curve dim (what comes back).
            cx.scene.stroke(&Stroke::new(1.5).with_dashes(0.0, [6.0, 4.0]), Affine::IDENTITY, accent.multiply_alpha(0.5), None, &path);
            glow_stroke(cx, &Line::new((PLOT_X0, PLOT.y0 + 6.0), (PLOT.x1, PLOT.y0 + 6.0)), 2.0, accent, accent, 4.0);
        } else if decay_on {
            glow_stroke(cx, &path, 2.2, accent, accent, 4.0);
        } else {
            cx.scene.stroke(&Stroke::new(1.5), Affine::IDENTITY, accent.multiply_alpha(0.45), None, &path);
        }
        cx.end_clip();
        // The grips.
        let s = settings(host);
        for node in self.nodes(host) {
            let p = self.node(host, node);
            let hot = self.hot == Some(Hot::Node(node)) || matches!(self.drag, Some(Drag::Node { node: n, .. }) if n == node);
            let lit = node.layer() == self.layer;
            let ink = if node.layer() == Layer::Tone { theme.curve } else { accent };
            let ink = if lit { ink } else { ink.multiply_alpha(0.45) };
            match node {
                Node::Decay(b) | Node::Tone(b) => {
                    // A bell's width as a bracket under it (to where it does half its dB).
                    let (bell, q, freq) = match node {
                        Node::Decay(_) => (s.bands[b].shape == DecayShape::Bell, f64::from(s.bands[b].q), f64::from(s.bands[b].freq)),
                        Node::Tone(_) => (s.eq[b].kind == BandType::Bell, s.eq[b].q, s.eq[b].freq),
                    };
                    if bell && lit {
                        let bw = 2.0 / std::f64::consts::LN_2 * (1.0 / (2.0 * q)).asinh();
                        let (x0, x1) = (freq_x(freq * 2f64.powf(-bw / 2.0)), freq_x(freq * 2f64.powf(bw / 2.0)));
                        let y = p.y + 16.0;
                        let mut br = BezPath::new();
                        br.move_to((x0, y - 4.0));
                        br.line_to((x0, y));
                        br.line_to((x1, y));
                        br.line_to((x1, y - 4.0));
                        cx.scene.stroke(&Stroke::new(1.2), Affine::IDENTITY, ink.multiply_alpha(if hot { 0.9 } else { 0.45 }), None, &br);
                    }
                    let c = Circle::new(p, GRIP_R);
                    cx.scene.fill(Fill::NonZero, Affine::IDENTITY, if hot { ink } else { theme.display }, None, &c);
                    if lit {
                        glow_stroke(cx, &c, 1.6, ink, ink, if hot { 4.0 } else { 2.0 });
                    } else {
                        cx.scene.stroke(&Stroke::new(1.3), Affine::IDENTITY, ink, None, &c);
                    }
                    let num = TextStyle { tabular: true, ..TextStyle::BODY.sized(9.0).weighted(700.0) };
                    cx.text.draw(cx.scene, &format!("{}", b + 1), p.x, p.y - 5.5, num, if hot { theme.display } else { ink }, Align::Center);
                }
            }
        }
        // The layer pills: the decay EQ (a tail falling) and the tone EQ (a bell on a line).
        for layer in [Layer::Decay, Layer::Tone] {
            let r = RoundedRect::from_rect(self.layer_pill(layer), 8.0);
            let on = self.layer == layer;
            let hot = self.hot == Some(Hot::Layer(layer));
            let ink = if layer == Layer::Tone { theme.curve } else { accent };
            cx.scene.fill(Fill::NonZero, Affine::IDENTITY, if on { ink.multiply_alpha(0.18) } else { theme.display }, None, &r);
            if on {
                glow_stroke(cx, &r, 1.3, ink, ink, 4.0);
            } else {
                cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, if hot { theme.text_hi } else { theme.well_border }, None, &r);
            }
            draw_layer_glyph(cx, layer, r.rect().center(), if on { ink } else if hot { theme.text_hi } else { theme.text_lo });
        }
        if frozen {
            let r = RoundedRect::from_rect(Rect::new(PLOT.x1 - 56.0, PLOT.y0 + 14.0, PLOT.x1 - 20.0, PLOT.y0 + 50.0), 10.0);
            cx.scene.fill(Fill::NonZero, Affine::IDENTITY, accent.multiply_alpha(0.18), None, &r);
            glow_stroke(cx, &r, 1.5, accent, accent, 9.0);
            mite_ui::widgets::footer::draw_toggle_glyph(cx.scene, ToggleGlyph::Freeze, r.center(), accent);
        }
    }


    /// The dial groups' panels, each named at its top left.
    fn draw_groups(&self, cx: &mut DrawCx<'_>) {
        let theme = cx.theme;
        for g in &self.groups {
            panel(cx.scene, RoundedRect::from_rect(g.rect, 10.0), theme.well, theme.well_border);
            cx.text.draw(cx.scene, g.name, g.rect.x0 + 12.0, g.rect.y0 + 9.0, TextStyle::LABEL.sized(9.0), theme.text_mid, Align::Left);
        }
    }

    fn draw_meters(&self, cx: &mut DrawCx<'_>) {
        let theme = cx.theme;
        cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, theme.line, None, &Line::new((PLOT.x1 + 0.5, DISPLAY_TOP), (PLOT.x1 + 0.5, HELP_TOP)));
        for x in WELLS {
            panel(cx.scene, RoundedRect::new(x, METER_TOP, x + 20.0, METER_BOTTOM, 3.0), theme.meter_well, theme.well_border);
        }
        let seg = Stroke::new(8.0).with_dashes(0.0, [2.0, 2.0]);
        let (top, bottom) = (METER_TOP + 4.0, METER_BOTTOM - 4.0);
        let span = bottom - top;
        let level_y = |db: f32| bottom - ((f64::from(db) + METER_RANGE).clamp(0.0, METER_RANGE) / METER_RANGE * span / 4.0).round() * 4.0;
        for (i, x) in WELLS.iter().flat_map(|&w| [w + 5.0, w + 15.0]).enumerate() {
            cx.scene.stroke(&seg, Affine::IDENTITY, theme.meter_off, None, &Line::new((x, top), (x, bottom)));
            let line = Line::new((x, bottom), (x, level_y(self.meters[i])));
            let color = if i < 2 { theme.spectrum_pre } else { theme.curve };
            if (line.p1.y - line.p0.y).abs() >= 2.0 {
                cx.scene.stroke(&seg, Affine::IDENTITY, color, None, &line);
                if i >= 2 {
                    cx.glow().stroke(&Stroke::new(8.0), Affine::IDENTITY, color.multiply_alpha(0.3), None, &line);
                }
            }
        }
        for (x, t) in [(WELLS[0], "IN"), (WELLS[1], "OUT")] {
            cx.text.draw(cx.scene, t, x + 10.0, DISPLAY_TOP + 596.0, TextStyle::LABEL.sized(9.0), theme.text_lo, Align::Center);
        }
    }

    fn draw_menu(&self, cx: &mut DrawCx<'_>) {
        let Some((what, list)) = &self.menu else { return };
        let tone = matches!(what, MenuFor::Band(Node::Tone(_)) | MenuFor::New(Layer::Tone, _));
        let color = if tone { cx.theme.curve } else { cx.theme.accent };
        let remove = list.items.len() - 1;
        list.draw(cx, color, |scene, i, at, ink| {
            if i == remove {
                let mut x = BezPath::new();
                x.move_to(at + (6.0, 6.0));
                x.line_to(at + (16.0, 16.0));
                x.move_to(at + (16.0, 6.0));
                x.line_to(at + (6.0, 16.0));
                scene.stroke(&Stroke::new(1.6), Affine::IDENTITY, ink, None, &x);
            } else {
                let kind = if tone { BandType::from_index(i) } else { DecayShape::from_index(i).band_type() };
                draw_type_icon(scene, kind, at + (0.0, 6.0), ink);
            }
        });
    }

    // ---- help -------------------------------------------------------------------------------

    fn node_help(&self, host: &dyn ParamHost, node: Node, dragging: bool) -> Help {
        let s = settings(host);
        match node {
            Node::Decay(b) => {
                let band = s.bands[b];
                let f = f64::from(band.freq);
                let t = self.model(&s).at(f);
                Help::default()
                    .text(format!("Decay band {} · {} · ", b + 1, DECAY_MENU[band.shape.index()]))
                    .slot(fmt_s(t), "30.00 s")
                    .text(" at ")
                    .slot(fmt_hz(f), "16.00 kHz")
                    .text(format!(" · ×{:.2} the decay · Q {:.2}", band.mult, band.q))
                    .text(if dragging { " — Shift is fine." } else { " — drag; wheel: width; right-click: shape or remove; double-click removes it." })
            }
            Node::Tone(b) => {
                let band = s.eq[b];
                let mut h = Help::default().text(format!("Tone band {} · {} · ", b + 1, TONE_MENU[band.kind as usize])).slot(fmt_hz(band.freq), "20.00 kHz");
                if band.kind.has_gain() {
                    h = h.text(" · ").slot(format!("{:+.1} dB", band.gain_db), "+24.0 dB");
                }
                if band.kind.has_slope() {
                    h = h.text(" · ").slot(format!("{} dB/oct", SLOPES_DB[band.slope.min(SLOPES_DB.len() - 1)]), "96 dB/oct");
                } else {
                    h = h.text(" · Q ").slot(format!("{:.2}", band.q), "10.00");
                }
                let wheel = if band.kind.has_slope() { "slope" } else { "Q" };
                h.text(if dragging { " — Shift is fine.".to_owned() } else { format!(" — drag; wheel: {wheel}; right-click: shape or remove; double-click removes it.") })
            }
        }
    }

    fn help_text(&self, host: &dyn ParamHost) -> Help {
        if let Some((what, list)) = &self.menu {
            let i = self.pointer.and_then(|p| list.item_rects().iter().position(|r| r.contains(p))).map(|k| list.shown[k]);
            return match (i, what) {
                (Some(i), _) if i == list.items.len() - 1 => Help::default().text("Remove — the band stops (its settings are kept for when it comes back)"),
                (Some(i), MenuFor::New(..)) => Help::default().text(format!("{} — a new band of this shape where you clicked", list.items[i])),
                (Some(i), MenuFor::Band(_)) => Help::default().text(format!("{} — this band takes the shape", list.items[i])),
                (None, _) => Help::default().text("Pick a shape; click elsewhere to close."),
            };
        }
        match self.drag {
            Some(Drag::Node { node, .. }) => return self.node_help(host, node, true),
            None => {}
        }
        match self.hot {
            Some(Hot::Node(node)) => return self.node_help(host, node, false),
            Some(Hot::Layer(Layer::Decay)) => return Help::default().text("Decay EQ — how long each frequency rings. Double-click the plot for a band; right-click for its shape."),
            Some(Hot::Layer(Layer::Tone)) => return Help::default().text("Tone EQ — the reverb's own EQ (cuts, shelves, bells…); the dry signal is untouched. Double-click the plot for a band; right-click for its shape."),
            None => {}
        }
        if let Some(h) = self.footer.help(host) {
            return h;
        }
        if let Some(k) = self.knobs.iter().find(|k| self.pointer.is_some_and(|p| k.bounds().contains(p))) {
            let v = Self::plain(host, &k.id);
            return Help::default().text(format!("{} ", k.spec.name)).slot(k.spec.format(v), k.spec.format(k.spec.max)).text(knob_words(k.spec.id));
        }
        if let Some(g) = self.groups.iter().find(|g| self.pointer.is_some_and(|p| g.rect.contains(p))) {
            return Help::default().text(format!("{} — {}", g.name, g.words));
        }
        if let Some(h) = self.session.help() {
            return h;
        }
        if self.frozen(host) {
            return Help::default().text("Frozen: the tail holds forever and new sound stays out. The dim curve is the decay that returns when you let go.");
        }
        if self.pointer.is_some_and(|p| PLOT.contains(p)) {
            return match self.layer {
                Layer::Decay => Help::default().text("Decay EQ: the curve is how long the reverb rings at each frequency. Double-click for a band through the pointer; right-click to pick its shape."),
                Layer::Tone => Help::default().text("Tone EQ on the reverb: double-click for a bell at the pointer; right-click to pick any shape (cuts, shelves, notch…)."),
            };
        }
        Help::default().text("Top: the decay EQ (how long each frequency rings) and the tone EQ (the reverb's sound); the pills top left pick which one you edit.")
    }

    // ---- events -----------------------------------------------------------------------------

    fn busy(&self) -> bool {
        self.drag.is_some() || self.menu.is_some() || self.knobs.iter().any(Knob::is_dragging) || self.footer.busy()
    }

    fn hot_at(&self, host: &dyn ParamHost, p: Point) -> Option<Hot> {
        for layer in [Layer::Decay, Layer::Tone] {
            if self.layer_pill(layer).inflate(2.0, 2.0).contains(p) {
                return Some(Hot::Layer(layer));
            }
        }
        if PLOT.contains(p) {
            for node in self.nodes(host).into_iter().rev() {
                if self.node(host, node).distance(p) < GRIP_HIT {
                    return Some(Hot::Node(node));
                }
            }
        }
        None
    }

    /// A new band in `layer` of shape `kind` (an index into that layer's shapes) at `at`: a decay
    /// band puts the curve through the pointer, a tone band its gain. Whether one was made (each
    /// layer has eight).
    fn add_band(&self, host: &dyn ParamHost, layer: Layer, kind: usize, at: Point) -> bool {
        let f = x_freq(at.x);
        match layer {
            Layer::Decay => {
                let Some(b) = (0..DECAY_BANDS).find(|&b| !Self::on(host, &band_id(b, D_ON.id))) else { return false };
                let shape = DecayShape::from_index(kind);
                let f = (f as f32).clamp(D_FREQ.min, D_FREQ.max);
                let q = if shape == DecayShape::Bell { D_Q.default } else { std::f32::consts::FRAC_1_SQRT_2 };
                let mut s = settings(host);
                s.bands[b] = mite_dsp::reverb::DecayBand { on: true, shape, freq: f, mult: 1.0, q };
                let target = y_decay(at.y.clamp(DECAY_TOP, DECAY_BOTTOM));
                // A tilt does nothing at its pivot: it starts flat, to be dragged.
                let m = if shape == DecayShape::Tilt { 1.0 } else { self.solve_mult(&s, b, f64::from(f), target) };
                for (id, v) in [(band_id(b, D_FREQ.id), D_FREQ.snap(f)), (band_id(b, D_TYPE.id), kind as f32), (band_id(b, D_Q.id), q), (band_id(b, D_MULT.id), D_MULT.snap(m.clamp(D_MULT.min, D_MULT.max))), (band_id(b, D_ON.id), 1.0)] {
                    Self::set(host, &id, v);
                }
            }
            Layer::Tone => {
                let Some(b) = (0..TONE_BANDS).find(|&b| !Self::on(host, &tone_id(b, E_ON.id))) else { return false };
                let kind_t = BandType::from_index(kind);
                let gain = if kind_t.has_gain() { E_GAIN.snap((y_tone(at.y) as f32).clamp(E_GAIN.min, E_GAIN.max)) } else { 0.0 };
                let q = if kind_t == BandType::Bell { 1.0 } else { E_Q.default };
                for (id, v) in [(tone_id(b, E_FREQ.id), E_FREQ.snap((f as f32).clamp(E_FREQ.min, E_FREQ.max))), (tone_id(b, E_TYPE.id), kind as f32), (tone_id(b, E_GAIN.id), gain), (tone_id(b, E_Q.id), q), (tone_id(b, E_SLOPE.id), E_SLOPE.default as f32), (tone_id(b, E_ON.id), 1.0)] {
                    Self::set(host, &id, v);
                }
            }
        }
        true
    }

    fn press(&mut self, host: &dyn ParamHost, pos: Point) -> bool {
        match self.hot_at(host, pos) {
            Some(Hot::Layer(layer)) => {
                self.layer = layer;
                true
            }
            Some(Hot::Node(node)) => {
                self.layer = node.layer();
                for id in Self::node_params(node) {
                    host.begin_gesture(&id);
                }
                let s = settings(host);
                let mult = if let Node::Decay(b) = node { s.bands[b].mult } else { 1.0 };
                self.drag = Some(Drag::Node { node, start: pos, at: self.node(host, node), mult });
                true
            }
            None => false,
        }
    }

    fn end_drag(&mut self, host: &dyn ParamHost) {
        match self.drag.take() {
            Some(Drag::Node { node, .. }) => {
                for id in Self::node_params(node) {
                    host.end_gesture(&id);
                }
            }
            None => {}
        }
    }

    /// A grip dragged to `to` (where it should now sit on the plot).
    fn move_node(&self, host: &dyn ParamHost, node: Node, to: Point, drag: (Point, f32)) {
        let (at, mult) = drag;
        let mut s = settings(host);
        match node {
            Node::Decay(b) => {
                let f = (x_freq(to.x) as f32).clamp(D_FREQ.min, D_FREQ.max);
                Self::set_raw(host, &band_id(b, D_FREQ.id), D_FREQ.snap(f));
                s.bands[b].freq = f;
                let m = if s.bands[b].shape == DecayShape::Tilt {
                    // Nothing moves at a tilt's pivot: up doubles it per 80 px.
                    mult * 2f32.powf(((at.y - to.y) / 80.0) as f32)
                } else {
                    self.solve_mult(&s, b, f64::from(f), y_decay(to.y))
                };
                Self::set_raw(host, &band_id(b, D_MULT.id), D_MULT.snap(m.clamp(D_MULT.min, D_MULT.max)));
            }
            Node::Tone(b) => {
                let f = (x_freq(to.x) as f32).clamp(E_FREQ.min, E_FREQ.max);
                Self::set_raw(host, &tone_id(b, E_FREQ.id), E_FREQ.snap(f));
                if s.eq[b].kind.has_gain() {
                    Self::set_raw(host, &tone_id(b, E_GAIN.id), E_GAIN.snap((y_tone(to.y) as f32).clamp(E_GAIN.min, E_GAIN.max)));
                }
            }
        }
    }

    fn open_menu(&mut self, host: &dyn ParamHost, pos: Point) -> bool {
        let anchor = Rect::from_center_size(pos, (2.0, 2.0));
        let area = (DISPLAY_TOP, HELP_TOP);
        let s = settings(host);
        match self.hot_at(host, pos) {
            Some(Hot::Node(node @ Node::Decay(b))) => {
                self.layer = Layer::Decay;
                self.menu = Some((MenuFor::Band(node), Dropdown::new(anchor, &DECAY_MENU, s.bands[b].shape.index(), area).with_min_width(170.0)));
            }
            Some(Hot::Node(node @ Node::Tone(b))) => {
                self.layer = Layer::Tone;
                self.menu = Some((MenuFor::Band(node), Dropdown::new(anchor, &TONE_MENU, s.eq[b].kind as usize, area).with_min_width(170.0)));
            }
            None if PLOT.contains(pos) => {
                let (items, n): (&'static [&'static str], usize) = match self.layer {
                    Layer::Decay => (&DECAY_MENU, DECAY_MENU.len() - 1),
                    Layer::Tone => (&TONE_MENU, TONE_MENU.len() - 1),
                };
                self.menu = Some((MenuFor::New(self.layer, pos), Dropdown::new(anchor, items, usize::MAX, area).with_shown((0..n).collect()).with_min_width(170.0)));
            }
            _ => return false,
        }
        true
    }

    fn menu_event(&mut self, event: &UiEvent, host: &dyn ParamHost) -> bool {
        let Some((what, list)) = &mut self.menu else { return false };
        if let UiEvent::PointerMove { pos, .. } = *event {
            self.pointer = Some(pos);
        }
        let what = *what;
        let remove = list.items.len() - 1;
        match list.event(event) {
            DropdownEvent::Open => {}
            DropdownEvent::Closed => self.menu = None,
            DropdownEvent::Picked(i) => {
                self.menu = None;
                match what {
                    MenuFor::New(layer, at) => {
                        self.add_band(host, layer, i, at);
                    }
                    MenuFor::Band(node) => {
                        let (on, kind) = match node {
                            Node::Decay(b) => (band_id(b, D_ON.id), band_id(b, D_TYPE.id)),
                            Node::Tone(b) => (tone_id(b, E_ON.id), tone_id(b, E_TYPE.id)),
                                        };
                        if i == remove {
                            Self::set(host, &on, 0.0);
                        } else {
                            host.begin_gesture(&kind);
                            Self::set(host, &kind, i as f32);
                            host.end_gesture(&kind);
                        }
                    }
                }
            }
        }
        true
    }

    fn handle_event(&mut self, event: &UiEvent, host: &dyn ParamHost) -> bool {
        if self.menu.is_some() {
            return self.menu_event(event, host);
        }
        if let UiEvent::KeyDown { key, mods } = *event {
            return self.session.key(key, mods, host).unwrap_or(false);
        }
        if let Some(answer) = self.session.event(event, host) {
            return answer;
        }
        if self.footer.event(event, host) {
            return true;
        }
        for k in &mut self.knobs {
            if k.event(event, host) {
                return true;
            }
        }
        match *event {
            UiEvent::PointerMove { pos, mods } => {
                self.pointer = Some(pos);
                let k = if mods.fine() { 0.2 } else { 1.0 };
                match self.drag {
                    Some(Drag::Node { node, start, at, mult }) => {
                        let mut to = at + (pos - start) * k;
                        to.y = to.y.clamp(DECAY_TOP, DECAY_BOTTOM);
                        self.move_node(host, node, to, (at, mult));
                        return true;
                    }
                    None => {}
                }
                self.hot = self.hot_at(host, pos);
                true
            }
            UiEvent::PointerLeft => {
                self.pointer = None;
                self.hot = None;
                true
            }
            UiEvent::PointerDown { pos, button: MouseButton::Left, .. } => self.press(host, pos),
            UiEvent::PointerDown { pos, button: MouseButton::Right, .. } => {
                self.end_drag(host);
                self.open_menu(host, pos)
            }
            UiEvent::PointerUp { button: MouseButton::Left, .. } => {
                let was = self.drag.is_some();
                self.end_drag(host);
                was
            }
            UiEvent::DoubleClick { pos, .. } => {
                let hot = self.hot_at(host, pos);
                self.end_drag(host);
                match hot {
                    Some(Hot::Node(Node::Decay(b))) => Self::set(host, &band_id(b, D_ON.id), 0.0),
                    Some(Hot::Node(Node::Tone(b))) => Self::set(host, &tone_id(b, E_ON.id), 0.0),
                    Some(Hot::Layer(_)) => return false,
                    None if PLOT.contains(pos) => return self.add_band(host, self.layer, 0, pos),
                    None => return false,
                }
                true
            }
            UiEvent::Wheel { pos, lines, mods } => {
                let fine = mods.fine();
                match self.hot_at(host, pos) {
                    Some(Hot::Node(Node::Decay(b))) => {
                        let id = band_id(b, D_Q.id);
                        Self::set(host, &id, D_Q.wheel(Self::plain(host, &id), lines, fine));
                    }
                    Some(Hot::Node(Node::Tone(b))) => {
                        if BandType::from_index(Self::plain(host, &tone_id(b, E_TYPE.id)).round() as usize).has_slope() {
                            let id = tone_id(b, E_SLOPE.id);
                            let n = E_SLOPE.choices.len() as f32 - 1.0;
                            Self::set(host, &id, (Self::plain(host, &id) + lines.signum()).clamp(0.0, n));
                        } else {
                            let id = tone_id(b, E_Q.id);
                            Self::set(host, &id, E_Q.wheel(Self::plain(host, &id), lines, fine));
                        }
                    }
                    _ => return false,
                }
                true
            }
            _ => false,
        }
    }
}

/// A layer pill's glyph: a tail falling away (decay) or a bell on its line (tone).
fn draw_layer_glyph(cx: &mut DrawCx<'_>, layer: Layer, c: Point, ink: Color) {
    let mut p = BezPath::new();
    match layer {
        Layer::Decay => {
            p.move_to((c.x - 10.0, c.y - 7.0));
            p.curve_to((c.x - 6.0, c.y + 4.0), (c.x - 1.0, c.y + 6.0), (c.x + 10.0, c.y + 7.0));
        }
        Layer::Tone => {
            p.move_to((c.x - 11.0, c.y + 4.0));
            p.line_to((c.x - 6.0, c.y + 4.0));
            p.curve_to((c.x - 2.0, c.y + 4.0), (c.x - 2.5, c.y - 7.0), (c.x, c.y - 7.0));
            p.curve_to((c.x + 2.5, c.y - 7.0), (c.x + 2.0, c.y + 4.0), (c.x + 6.0, c.y + 4.0));
            p.line_to((c.x + 11.0, c.y + 4.0));
        }
    }
    cx.scene.stroke(&Stroke::new(1.6).with_caps(Cap::Round).with_join(Join::Round), Affine::IDENTITY, ink, None, &p);
}

fn knob_words(id: &str) -> &'static str {
    match id {
        "size" => " — the room: how far apart its walls are (small rooms ring close and dense, halls spread out)",
        "decay" => " — how long the reverb takes to fall 60 dB where no decay band changes it",
        "predelay" => " — the gap before the reverb starts: keeps a voice in front of its room",
        "diffusion" => " — how fast the echoes blur into a wash (low: each reflection heard)",
        "early" => " — the first reflections off the walls: they tell the ear the room's size",
        "width" => " — the reverb's stereo spread (0 %: mono, 150 %: wider than the room)",
        "mod_depth" => " — a slow drift in the room that keeps the tail from ringing metallic",
        "mod_rate" => " — how fast that drift moves",
        "duck" => " — how far the reverb is turned down while the input plays (0: off)",
        "duck_release" => " — how fast the reverb comes back after the input stops",
        "early_size" => " — how long the first reflections take to arrive: the room's walls before its tail",
        "early_send" => " — how much of the first reflections feeds the tail (more: a tail that grows out of them)",
        _ => "",
    }
}

impl View for VerbView {
    fn logical_size(&self) -> (f64, f64) {
        (VIEW_W, VIEW_H)
    }

    fn draw(&mut self, cx: &mut DrawCx<'_>) {
        self.update_curves(cx.host);
        let theme = cx.theme;
        self.draw_header(cx);
        cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.display, None, &Rect::new(0.0, DISPLAY_TOP, VIEW_W, HELP_TOP));
        // Nothing glows through an open shape menu.
        let hole = self.menu.as_ref().map(|(_, m)| RoundedRect::from_rect(m.bounds(), 8.0));
        if let Some(h) = hole {
            cx.begin_glow_hole(h);
        }
        self.draw_plot(cx);
        self.draw_groups(cx);
        for k in &self.knobs {
            k.draw(cx);
        }
        // The input on the threshold dial: where it plays, lit over the threshold (ducking).
        if let Some(k) = self.knobs.iter().find(|k| k.id == DUCK_THRESH.id) {
            let input = self.meters[0].max(self.meters[1]);
            k.draw_level(cx, input, Self::plain(cx.host, DUCK.id) > 0.0);
        }
        self.draw_meters(cx);
        self.help_text(cx.host).draw(cx, Rect::new(0.0, HELP_TOP, VIEW_W, FOOTER_TOP));
        self.footer.draw_frame(cx);
        self.footer.draw_controls(cx);
        if hole.is_some() {
            cx.end_glow_hole();
        }
        self.draw_menu(cx);
        self.session.draw_menu(cx);
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
        let dt = self.last_tick.elapsed().as_secs_f32().min(0.1);
        self.last_tick = Instant::now();
        let mut fresh = false;
        let history = &mut self.history;
        self.shared.history.read_since(&mut self.seen, |r| {
            if history.len() == HISTORY_RECORDS {
                history.pop_front();
            }
            history.push_back(*r);
            fresh = true;
        });
        if fresh {
            self.last_fresh = Instant::now();
        }
        if let Some(r) = self.history.back().copied() {
            let targets = [r[record::IN], r[record::IN + 1], r[record::OUT], r[record::OUT + 1]];
            for (m, t) in self.meters.iter_mut().zip(targets) {
                let fall = METER_RELEASE_DB_S * dt;
                *m = if t > *m { t } else { (*m - fall).max(t) };
            }
        }
        let spectrum = self.spectrum.update(&self.shared, f64::from(dt));
        let flowing = self.last_fresh.elapsed().as_secs_f32() < 0.25;
        fresh || flowing || spectrum || busy || self.meters.iter().any(|&m| m > SILENT + 1.0)
    }
}
