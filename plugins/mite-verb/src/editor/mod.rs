//! The Mite Verb editor (docs/design/verb-brief.md §2), in 1280×760 design units.
//!
//! The decay plot on top: the input's and the reverb's spectra behind the decay time per
//! frequency (`mite_dsp::reverb::decay_curve`, what the lines' absorption gives), with three grips
//! on the curve (low, mid, high) and the reverb's low and high cut on the floor. Under it the
//! history (the input dim, the reverb lit: its tail is seen falling; the ducking threshold to drag
//! and the ducking from the top) and the dials; the meters on the right.

mod spectrum;

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Instant;

use mite_core::{ParamDisplay, ParamScale, ParamSpec};
use mite_dsp::reverb::{decay_at, decay_curve, ReverbSettings, DECAY_BANDS};
use mite_ui::kurbo::{Affine, BezPath, Cap, Circle, Join, Line, Point, Rect, RoundedRect, Stroke};
use mite_ui::paint::{glow_stroke, panel};
use mite_ui::peniko::{Color, Fill};
use mite_ui::session::{Session, SessionConfig};
use mite_ui::widgets::brand::{draw_mark, draw_title};
use mite_ui::widgets::history::{self, HistoryLane};
use mite_ui::widgets::{Footer, FooterDial, FooterToggle, Help, Knob, KnobKind, ToggleGlyph};
use mite_ui::{write_if_changed, Align, DrawCx, MouseButton, ParamHost, RecordingHost, TextStyle, UiEvent, View};

use crate::dsp::{record, HISTORY_RECORDS, HISTORY_SLICE_MS};
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

/// The decay plot.
const PLOT: Rect = Rect::new(0.0, DISPLAY_TOP, 1180.0, DISPLAY_TOP + 392.0);
pub(crate) const PLOT_X0: f64 = 28.0;
pub(crate) const PLOT_W: f64 = 1124.0;
/// The decay axis: 0.1…30 s, log, between these.
const DECAY_TOP: f64 = PLOT.y0 + 26.0;
const DECAY_BOTTOM: f64 = PLOT.y1 - 34.0;
const DECAY_MIN: f64 = 0.1;
const DECAY_MAX: f64 = 30.0;
const CURVE_POINTS: usize = 240;
const GRIP_R: f64 = 8.0;
const GRIP_HIT: f64 = 16.0;
/// The cuts' grips sit on the plot's floor.
const CUT_Y: f64 = PLOT.y1 - 14.0;
const CUT_GRIP: (f64, f64) = (16.0, 24.0);

/// The history under the plot.
const HIST_TOP: f64 = PLOT.y1 + 8.0;
const HIST_BOTTOM: f64 = HELP_TOP - 4.0;
const HIST_W: f64 = 776.0;
const HIST_SECONDS: f64 = 6.0;
const HIST_RANGE: f64 = 60.0;
const DUCK_GRIP_X: f64 = HIST_W - 38.0;
const DUCK_GRIP: (f64, f64) = (34.0, 20.0);

/// The dials beside the history.
const DIAL_D: f64 = 46.0;
const DIAL_XS: [f64; 6] = [820.0, 884.0, 948.0, 1012.0, 1076.0, 1140.0];
const DIAL_ROWS: [f64; 2] = [HIST_TOP + 46.0, HIST_TOP + 150.0];

const WELLS: [f64; 2] = [1206.0, 1238.0];
const METER_TOP: f64 = DISPLAY_TOP + 20.0;
const METER_BOTTOM: f64 = DISPLAY_TOP + 592.0;
const METER_RELEASE_DB_S: f32 = 20.0;
const SILENT: f32 = -150.0;

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

/// A level's y on the history (0…−60 dB).
fn lv_y(db: f32) -> f64 {
    HIST_TOP + 6.0 + (-f64::from(db)).clamp(0.0, HIST_RANGE) / HIST_RANGE * (HIST_BOTTOM - HIST_TOP - 12.0)
}

fn px_per_db() -> f64 {
    (HIST_BOTTOM - HIST_TOP - 12.0) / HIST_RANGE
}

fn history_lane() -> HistoryLane {
    HistoryLane { x0: 0.0, width: HIST_W, top: HIST_TOP, bottom: HIST_BOTTOM, seconds: HIST_SECONDS, slice_ms: HISTORY_SLICE_MS }
}

fn fmt_db(v: f32) -> String {
    format!("{}{:.1}", if v < -0.05 { "−" } else { "" }, v.abs())
}

fn fmt_s(t: f64) -> String {
    if t >= 10.0 {
        format!("{t:.1} s")
    } else {
        format!("{t:.2} s")
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

/// The plot's three grips on the curve.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Node {
    Low,
    Mid,
    High,
    /// A decay bell (from 0), while it is on.
    Band(usize),
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Drag {
    /// A grip on the curve: where it was grabbed and where it sat.
    Node { node: Node, start: Point, at: Point },
    /// A cut's grip (`true`: the high cut).
    Cut { high: bool, start: Point, x: f64 },
    /// The ducking threshold on the history.
    Duck { start: Point, thresh: f32 },
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Hot {
    Node(Node),
    Cut(bool),
    Duck,
}

pub struct VerbView {
    shared: Arc<VerbShared>,
    session: Session,
    footer: Footer,
    knobs: Vec<Knob>,
    spectrum: Spectrum,
    /// The decay curve as last worked out, and what for.
    curve: Vec<f64>,
    curve_for: Option<(ReverbSettings, f32)>,
    pointer: Option<Point>,
    hot: Option<Hot>,
    drag: Option<Drag>,
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
        let dials: [(ParamSpec, &'static str, Option<mite_core::DialCurve>); 11] = [
            (SIZE, "SIZE", None),
            (PREDELAY, "PREDELAY", Some(PREDELAY_DIAL)),
            (DIFFUSION, "DIFFUSION", None),
            (WIDTH, "WIDTH", None),
            (MOD_DEPTH, "MOD", None),
            (MOD_RATE, "RATE", None),
            (EARLY, "EARLY", None),
            (EARLY_SIZE, "E.SIZE", None),
            (EARLY_SEND, "SEND", None),
            (DUCK, "DUCK", None),
            (DUCK_RELEASE, "RELEASE", None),
        ];
        let knobs = dials
            .into_iter()
            .enumerate()
            .map(|(i, (spec, label, curve))| {
                let k = Knob::new(spec, label, KnobKind::Unipolar, Point::new(DIAL_XS[i % 6], DIAL_ROWS[i / 6]), DIAL_D);
                match curve {
                    Some(c) => k.with_curve(c),
                    None => k,
                }
            })
            .collect();
        Self {
            session: Session::new(SessionConfig { plugin: PLUGIN, factory: FACTORY, sound_ids: preset_ids(), snapshot_ids: preset_ids() }, shared.snapshots.clone(), shared.preset.clone()),
            footer,
            knobs,
            spectrum: Spectrum::new(),
            curve: vec![0.0; CURVE_POINTS],
            curve_for: None,
            pointer: None,
            hot: None,
            drag: None,
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

    /// Where a grip sits: on the curve, at its frequency.
    pub fn node(&self, host: &dyn ParamHost, node: Node) -> Point {
        let s = settings(host);
        let f = Self::node_freq(&s, node);
        Point::new(freq_x(f), decay_y(decay_at(&s, f, self.fs())))
    }

    fn node_freq(s: &ReverbSettings, node: Node) -> f64 {
        match node {
            Node::Low => f64::from(s.lo_freq),
            Node::High => f64::from(s.hi_freq),
            Node::Mid => (f64::from(s.lo_freq) * f64::from(s.hi_freq)).sqrt(),
            Node::Band(b) => f64::from(s.bands[b].freq),
        }
    }

    /// The grips that exist: the three, then each bell that is on.
    fn nodes(host: &dyn ParamHost) -> Vec<Node> {
        let mut v = vec![Node::Mid, Node::Low, Node::High];
        v.extend((0..DECAY_BANDS).filter(|&b| Self::on(host, &band_id(b, D_ON.id))).map(Node::Band));
        v
    }

    pub fn cut_grip(&self, host: &dyn ParamHost, high: bool) -> Rect {
        let f = Self::plain(host, if high { HI_CUT.id } else { LO_CUT.id });
        Rect::from_center_size((freq_x(f64::from(f)), CUT_Y), CUT_GRIP)
    }

    pub fn duck_grip(&self, host: &dyn ParamHost) -> Rect {
        let y = lv_y(Self::plain(host, DUCK_THRESH.id));
        Rect::new(DUCK_GRIP_X, y - DUCK_GRIP.1 / 2.0, DUCK_GRIP_X + DUCK_GRIP.0, y + DUCK_GRIP.1 / 2.0)
    }

    pub fn decay_y(&self, t: f64) -> f64 {
        decay_y(t)
    }

    pub fn level_y(&self, db: f32) -> f64 {
        lv_y(db)
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

    /// The multiplier of `node` (low, high or a bell) that puts the curve at `target` seconds at
    /// `freq` (bisection: the curve there rises with the multiplier).
    fn solve_mult(&self, s: &ReverbSettings, node: Node, freq: f64, target: f64) -> f32 {
        let spec = match node {
            Node::High => HI_MULT,
            Node::Band(_) => D_MULT,
            _ => LO_MULT,
        };
        let (mut lo, mut hi) = (f64::from(spec.min).ln(), f64::from(spec.max).ln());
        let fs = self.fs();
        for _ in 0..28 {
            let m = 0.5 * (lo + hi);
            let mut t = *s;
            match node {
                Node::High => t.hi_mult = m.exp() as f32,
                Node::Band(b) => t.bands[b].mult = m.exp() as f32,
                _ => t.lo_mult = m.exp() as f32,
            }
            if decay_at(&t, freq, fs) < target {
                lo = m;
            } else {
                hi = m;
            }
        }
        (0.5 * (lo + hi)).exp() as f32
    }

    fn since_fresh(&self) -> f64 {
        (self.last_fresh.elapsed().as_secs_f64() * 1000.0 / f64::from(HISTORY_SLICE_MS)).min(4.0)
    }

    fn frozen(&self, host: &dyn ParamHost) -> bool {
        Self::on(host, FREEZE.id) || self.history.back().is_some_and(|r| r[record::FREEZE] > 0.5)
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

    fn update_curve(&mut self, host: &dyn ParamHost) {
        let s = ReverbSettings { freeze: false, ..settings(host) };
        let fs = self.fs();
        if self.curve_for == Some((s, fs as f32)) {
            return;
        }
        let freqs: Vec<f64> = (0..CURVE_POINTS).map(Self::curve_freq).collect();
        decay_curve(&s, &freqs, fs, &mut self.curve);
        self.curve_for = Some((s, fs as f32));
    }

    fn draw_plot(&self, cx: &mut DrawCx<'_>) {
        let theme = cx.theme;
        let host = cx.host;
        cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.well, None, &PLOT);
        // The decay grid: seconds, labelled on the left.
        let label = TextStyle { tabular: true, ..TextStyle::BODY.sized(10.0).weighted(600.0) };
        for (t, s) in [(0.2, "0.2"), (0.5, "0.5"), (1.0, "1 s"), (2.0, "2"), (5.0, "5"), (10.0, "10"), (20.0, "20")] {
            let y = decay_y(t);
            cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, if t == 1.0 || t == 10.0 { theme.grid_major } else { theme.grid_minor }, None, &Line::new((PLOT_X0, y), (PLOT.x1, y)));
            cx.text.draw(cx.scene, s, 6.0, y - 6.0, label, theme.text_lo, Align::Left);
        }
        cx.begin_clip(&PLOT);
        self.spectrum.draw(cx, PLOT);
        // The cuts: what they take off is shaded.
        let (lo_cut, hi_cut) = (freq_x(f64::from(Self::plain(host, LO_CUT.id))), freq_x(f64::from(Self::plain(host, HI_CUT.id))));
        let shade = theme.display.multiply_alpha(0.55);
        cx.scene.fill(Fill::NonZero, Affine::IDENTITY, shade, None, &Rect::new(PLOT.x0, PLOT.y0, lo_cut, PLOT.y1));
        cx.scene.fill(Fill::NonZero, Affine::IDENTITY, shade, None, &Rect::new(hi_cut, PLOT.y0, PLOT.x1, PLOT.y1));
        // The decay curve, its area faintly filled.
        let pts: Vec<Point> = (0..CURVE_POINTS).map(|i| Point::new(freq_x(Self::curve_freq(i)), decay_y(self.curve[i]))).collect();
        let path = history::smooth_path(&pts);
        let mut fill = path.clone();
        fill.line_to((pts[pts.len() - 1].x, PLOT.y1));
        fill.line_to((pts[0].x, PLOT.y1));
        fill.close_path();
        let frozen = self.frozen(host);
        let accent = theme.accent;
        cx.scene.fill(Fill::NonZero, Affine::IDENTITY, accent.multiply_alpha(if frozen { 0.04 } else { 0.09 }), None, &fill);
        if frozen {
            // Frozen, the decay is endless: the line at the top; the curve dim (what comes back).
            cx.scene.stroke(&Stroke::new(1.5).with_dashes(0.0, [6.0, 4.0]), Affine::IDENTITY, accent.multiply_alpha(0.5), None, &path);
            glow_stroke(cx, &Line::new((PLOT_X0, PLOT.y0 + 6.0), (PLOT.x1, PLOT.y0 + 6.0)), 2.0, accent, accent, 4.0);
        } else {
            glow_stroke(cx, &path, 2.2, accent, accent, 4.0);
        }
        cx.end_clip();
        // The cuts' lines and grips.
        for high in [false, true] {
            let x = if high { hi_cut } else { lo_cut };
            let hot = self.hot == Some(Hot::Cut(high)) || matches!(self.drag, Some(Drag::Cut { high: h, .. }) if h == high);
            cx.scene.stroke(&Stroke::new(1.2).with_dashes(0.0, [4.0, 4.0]), Affine::IDENTITY, theme.text_lo, None, &Line::new((x, PLOT.y0), (x, PLOT.y1)));
            let r = RoundedRect::from_rect(Rect::from_center_size((x, CUT_Y), CUT_GRIP), 6.0);
            cx.scene.fill(Fill::NonZero, Affine::IDENTITY, if hot { theme.text_hi } else { theme.display }, None, &r);
            cx.scene.stroke(&Stroke::new(1.3), Affine::IDENTITY, theme.text_hi, None, &r);
            draw_cut_glyph(cx, Point::new(x, CUT_Y), high, if hot { theme.display } else { theme.text_hi });
        }
        // The grips on the curve.
        let s = settings(host);
        for node in Self::nodes(host) {
            let p = self.node(host, node);
            let hot = self.hot == Some(Hot::Node(node)) || matches!(self.drag, Some(Drag::Node { node: n, .. }) if n == node);
            if let Node::Band(b) = node {
                // A bell: its width as a bracket under it (to where it does half its dB), its
                // number inside.
                let band = s.bands[b];
                let bw = 2.0 / std::f64::consts::LN_2 * (1.0 / (2.0 * f64::from(band.q))).asinh();
                let f = f64::from(band.freq);
                let (x0, x1) = (freq_x(f * 2f64.powf(-bw / 2.0)), freq_x(f * 2f64.powf(bw / 2.0)));
                let y = p.y + 16.0;
                let mut br = BezPath::new();
                br.move_to((x0, y - 4.0));
                br.line_to((x0, y));
                br.line_to((x1, y));
                br.line_to((x1, y - 4.0));
                cx.scene.stroke(&Stroke::new(1.2), Affine::IDENTITY, accent.multiply_alpha(if hot { 0.9 } else { 0.45 }), None, &br);
                cx.scene.fill(Fill::NonZero, Affine::IDENTITY, if hot { accent } else { theme.display }, None, &Circle::new(p, GRIP_R));
                glow_stroke(cx, &Circle::new(p, GRIP_R), 1.6, accent, accent, if hot { 4.0 } else { 2.0 });
                let num = TextStyle { tabular: true, ..TextStyle::BODY.sized(9.0).weighted(700.0) };
                cx.text.draw(cx.scene, &format!("{}", b + 1), p.x, p.y - 5.5, num, if hot { theme.display } else { accent }, Align::Center);
                continue;
            }
            let r = if node == Node::Mid { GRIP_R + 1.5 } else { GRIP_R };
            cx.scene.fill(Fill::NonZero, Affine::IDENTITY, if hot { accent } else { theme.display }, None, &Circle::new(p, r));
            glow_stroke(cx, &Circle::new(p, r), 2.0, accent, accent, if hot { 5.0 } else { 3.0 });
        }
        if frozen {
            let r = RoundedRect::from_rect(Rect::new(PLOT.x1 - 56.0, PLOT.y0 + 14.0, PLOT.x1 - 20.0, PLOT.y0 + 50.0), 10.0);
            cx.scene.fill(Fill::NonZero, Affine::IDENTITY, accent.multiply_alpha(0.18), None, &r);
            glow_stroke(cx, &r, 1.5, accent, accent, 9.0);
            mite_ui::widgets::footer::draw_toggle_glyph(cx.scene, ToggleGlyph::Freeze, r.center(), accent);
        }
    }

    fn draw_history(&self, cx: &mut DrawCx<'_>) {
        let theme = cx.theme;
        let lane = history_lane();
        let rect = lane.rect();
        cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.display, None, &rect);
        for db in [-12.0, -24.0, -36.0, -48.0] {
            cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, theme.grid_minor, None, &Line::new((0.0, lv_y(db)), (HIST_W, lv_y(db))));
        }
        cx.begin_clip(&rect);
        let cols = lane.columns(&self.history, self.seen, self.since_fresh());
        if cols.len() >= 2 {
            type Col = history::Column<{ record::WIDTH }>;
            let level = |c: &Col, k: usize| c.high[k].max(c.high[k + 1]);
            let line = |f: &dyn Fn(&Col) -> f64| history::column_line(&cols, f);
            let input = line(&|c| lv_y(level(c, record::IN)));
            cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.spectrum_pre.multiply_alpha(0.12), None, &history::close_to(input.clone(), &cols, HIST_BOTTOM));
            // The reverb alone, lit: its tail falls after each phrase.
            let wet = line(&|c| lv_y(level(c, record::WET)));
            let accent = theme.accent;
            let look = history::LevelLook { fill: [accent.multiply_alpha(0.16), accent.multiply_alpha(0.01)], span: (HIST_TOP, HIST_BOTTOM), edge: accent, width: 1.8, glow: 2.4 };
            history::draw_level(cx, &cols, &wet, HIST_BOTTOM, look);
            if cols.iter().any(|c| c.low[record::DUCK] < -0.05) {
                let edge = line(&|c| HIST_TOP + (f64::from(-c.low[record::DUCK]) * px_per_db()).clamp(0.0, HIST_BOTTOM - HIST_TOP));
                history::draw_reduction(cx, &cols, &edge, HIST_TOP, 0.5 * (HIST_BOTTOM - HIST_TOP));
            }
            cx.scene.stroke(&Stroke::new(1.2), Affine::IDENTITY, theme.spectrum_pre.multiply_alpha(0.85), None, &input);
        }
        cx.end_clip();
        // The ducking threshold: where the input crosses it, the reverb is turned down. Dim while
        // ducking is off.
        let host = cx.host;
        let on = Self::plain(host, DUCK.id) > 0.0;
        let hot = self.hot == Some(Hot::Duck) || matches!(self.drag, Some(Drag::Duck { .. }));
        let y = lv_y(Self::plain(host, DUCK_THRESH.id));
        let ink = if on { theme.accent } else { theme.text_lo };
        let stroke = Stroke::new(1.3).with_dashes(0.0, [5.0, 4.0]);
        cx.scene.stroke(&stroke, Affine::IDENTITY, ink.multiply_alpha(if on { 0.9 } else { 0.5 }), None, &Line::new((0.0, y), (DUCK_GRIP_X - 2.0, y)));
        let r = RoundedRect::from_rect(self.duck_grip(host), 9.0);
        cx.scene.fill(Fill::NonZero, Affine::IDENTITY, if hot { ink } else { theme.display }, None, &r);
        cx.scene.stroke(&Stroke::new(1.3), Affine::IDENTITY, ink, None, &r);
        draw_chevrons(cx, r.center(), if hot { theme.display } else { ink });
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
        let level_y = |db: f32| bottom - ((f64::from(db) + HIST_RANGE).clamp(0.0, HIST_RANGE) / HIST_RANGE * span / 4.0).round() * 4.0;
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

    // ---- help -------------------------------------------------------------------------------

    fn node_help(&self, host: &dyn ParamHost, node: Node, dragging: bool) -> Help {
        let s = settings(host);
        let f = Self::node_freq(&s, node);
        let t = decay_at(&s, f, self.fs());
        let freq = |hz: f64| if hz >= 1000.0 { format!("{:.2} kHz", hz / 1000.0) } else { format!("{hz:.0} Hz") };
        let tail = if dragging { " — Shift is fine." } else { " — drag it; wheel steps; double-click resets." };
        match node {
            Node::Mid => Help::default().text("Decay ").slot(fmt_s(f64::from(s.decay_s)), "30.00 s").text(" — how long the reverb takes to fall 60 dB in the middle").text(tail),
            Node::Low => Help::default().text("Low decay ").slot(fmt_s(t), "30.00 s").text(" at ").slot(freq(f), "2000 Hz").text(format!(" — under it the decay is ×{:.2} the middle's", s.lo_mult)).text(tail),
            Node::High => Help::default().text("High decay ").slot(fmt_s(t), "30.00 s").text(" at ").slot(freq(f), "16.00 kHz").text(format!(" — over it the decay is ×{:.2} the middle's", s.hi_mult)).text(tail),
            Node::Band(b) => Help::default()
                .text(format!("Band {} ", b + 1))
                .slot(fmt_s(t), "30.00 s")
                .text(" at ")
                .slot(freq(f), "16.00 kHz")
                .text(format!(" — ×{:.2} the decay there; Q {:.2}", s.bands[b].mult, s.bands[b].q))
                .text(if dragging { " — Shift is fine." } else { " — drag it; wheel: width; double-click removes it." }),
        }
    }

    fn help_text(&self, host: &dyn ParamHost) -> Help {
        match self.drag {
            Some(Drag::Node { node, .. }) => return self.node_help(host, node, true),
            Some(Drag::Cut { high, .. }) => {
                let spec = if high { HI_CUT } else { LO_CUT };
                let v = Self::plain(host, spec.id);
                return Help::default().text(if high { "High cut " } else { "Low cut " }).slot(spec.format(v), spec.format(spec.max)).text(" — on the reverb only; the dry signal is untouched.");
            }
            Some(Drag::Duck { .. }) => return Help::default().text("Ducking threshold ").slot(format!("{} dB", fmt_db(Self::plain(host, DUCK_THRESH.id))), "−60.0 dB").text(" — where the input crosses it, the reverb is turned down (1 dB per dB, at most DUCK)."),
            None => {}
        }
        match self.hot {
            Some(Hot::Node(node)) => return self.node_help(host, node, false),
            Some(Hot::Cut(high)) => return Help::default().text(if high { "High cut — darkens the reverb; drag across" } else { "Low cut — thins the reverb's lows; drag across" }),
            Some(Hot::Duck) => return Help::default().text("Ducking threshold — drag it; the reverb is turned down while the input is over it (set DUCK for how far)"),
            None => {}
        }
        if let Some(h) = self.footer.help(host) {
            return h;
        }
        if let Some(k) = self.knobs.iter().find(|k| self.pointer.is_some_and(|p| k.bounds().contains(p))) {
            let v = Self::plain(host, &k.id);
            return Help::default().text(format!("{} ", k.spec.name)).slot(k.spec.format(v), k.spec.format(k.spec.max)).text(knob_words(k.spec.id));
        }
        if let Some(h) = self.session.help() {
            return h;
        }
        if self.frozen(host) {
            return Help::default().text("Frozen: the tail holds forever and new sound stays out. The dim curve is the decay that returns when you let go.");
        }
        Help::default().text("The curve is how long the reverb rings at each frequency. Drag its grips; double-click the curve for a band (four); the cuts on the floor shape the tone.")
    }

    // ---- events -----------------------------------------------------------------------------

    fn busy(&self) -> bool {
        self.drag.is_some() || self.knobs.iter().any(Knob::is_dragging) || self.footer.busy()
    }

    fn hot_at(&self, host: &dyn ParamHost, p: Point) -> Option<Hot> {
        for node in Self::nodes(host).into_iter().rev() {
            if self.node(host, node).distance(p) < GRIP_HIT {
                return Some(Hot::Node(node));
            }
        }
        for high in [true, false] {
            if self.cut_grip(host, high).inflate(3.0, 3.0).contains(p) {
                return Some(Hot::Cut(high));
            }
        }
        if self.duck_grip(host).inflate(3.0, 2.0).contains(p) {
            return Some(Hot::Duck);
        }
        None
    }

    fn node_params(node: Node) -> Vec<String> {
        match node {
            Node::Mid => vec![DECAY.id.to_owned()],
            Node::Low => vec![LO_FREQ.id.to_owned(), LO_MULT.id.to_owned()],
            Node::High => vec![HI_FREQ.id.to_owned(), HI_MULT.id.to_owned()],
            Node::Band(b) => vec![band_id(b, D_FREQ.id), band_id(b, D_MULT.id)],
        }
    }

    /// Double-click on the plot away from the grips: a bell there (the first that is off), the
    /// curve through the pointer. Whether one was made.
    fn add_band(&self, host: &dyn ParamHost, at: Point) -> bool {
        let Some(b) = (0..DECAY_BANDS).find(|&b| !Self::on(host, &band_id(b, D_ON.id))) else { return false };
        let f = (x_freq(at.x) as f32).clamp(D_FREQ.min, D_FREQ.max);
        let mut s = settings(host);
        s.bands[b] = mite_dsp::reverb::DecayBand { on: true, freq: f, mult: 1.0, q: D_Q.default };
        let target = y_decay(at.y.clamp(DECAY_TOP, DECAY_BOTTOM));
        let m = self.solve_mult(&s, Node::Band(b), f64::from(f), target);
        for (id, v) in [(band_id(b, D_FREQ.id), D_FREQ.snap(f)), (band_id(b, D_MULT.id), D_MULT.snap(m.clamp(D_MULT.min, D_MULT.max))), (band_id(b, D_Q.id), D_Q.default), (band_id(b, D_ON.id), 1.0)] {
            Self::set(host, &id, v);
        }
        true
    }

    fn press(&mut self, host: &dyn ParamHost, pos: Point) -> bool {
        match self.hot_at(host, pos) {
            Some(Hot::Node(node)) => {
                for id in Self::node_params(node) {
                    host.begin_gesture(&id);
                }
                self.drag = Some(Drag::Node { node, start: pos, at: self.node(host, node) });
                true
            }
            Some(Hot::Cut(high)) => {
                host.begin_gesture(if high { HI_CUT.id } else { LO_CUT.id });
                self.drag = Some(Drag::Cut { high, start: pos, x: self.cut_grip(host, high).center().x });
                true
            }
            Some(Hot::Duck) => {
                host.begin_gesture(DUCK_THRESH.id);
                self.drag = Some(Drag::Duck { start: pos, thresh: Self::plain(host, DUCK_THRESH.id) });
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
            Some(Drag::Cut { high, .. }) => host.end_gesture(if high { HI_CUT.id } else { LO_CUT.id }),
            Some(Drag::Duck { .. }) => host.end_gesture(DUCK_THRESH.id),
            None => {}
        }
    }

    /// A grip dragged to `to` (where it should now sit on the plot).
    fn move_node(&self, host: &dyn ParamHost, node: Node, to: Point) {
        let mut s = settings(host);
        let target = y_decay(to.y);
        match node {
            Node::Mid => {
                // The curve scales with the decay: put it through the pointer.
                let f = Self::node_freq(&s, node);
                let now = decay_at(&s, f, self.fs());
                let d = (f64::from(s.decay_s) * target / now) as f32;
                Self::set_raw(host, DECAY.id, DECAY.snap(d.clamp(DECAY.min, DECAY.max)));
            }
            Node::Low | Node::High | Node::Band(_) => {
                let (fspec, mspec, fid, mid) = match node {
                    Node::High => (HI_FREQ, HI_MULT, HI_FREQ.id.to_owned(), HI_MULT.id.to_owned()),
                    Node::Band(b) => (D_FREQ, D_MULT, band_id(b, D_FREQ.id), band_id(b, D_MULT.id)),
                    _ => (LO_FREQ, LO_MULT, LO_FREQ.id.to_owned(), LO_MULT.id.to_owned()),
                };
                let f = (x_freq(to.x) as f32).clamp(fspec.min, fspec.max);
                Self::set_raw(host, &fid, fspec.snap(f));
                match node {
                    Node::High => s.hi_freq = f,
                    Node::Band(b) => s.bands[b].freq = f,
                    _ => s.lo_freq = f,
                }
                let m = self.solve_mult(&s, node, f64::from(f), target);
                Self::set_raw(host, &mid, mspec.snap(m.clamp(mspec.min, mspec.max)));
            }
        }
    }

    fn handle_event(&mut self, event: &UiEvent, host: &dyn ParamHost) -> bool {
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
                    Some(Drag::Node { node, start, at }) => {
                        let mut to = at + (pos - start) * k;
                        to.y = to.y.clamp(DECAY_TOP, DECAY_BOTTOM);
                        if node == Node::Mid {
                            to.x = at.x;
                        }
                        self.move_node(host, node, to);
                        return true;
                    }
                    Some(Drag::Cut { high, start, x }) => {
                        let spec = if high { HI_CUT } else { LO_CUT };
                        let f = x_freq(x + (pos.x - start.x) * k) as f32;
                        Self::set_raw(host, spec.id, spec.snap(f.clamp(spec.min, spec.max)));
                        return true;
                    }
                    Some(Drag::Duck { start, thresh }) => {
                        let d = ((start.y - pos.y) / px_per_db()) as f32;
                        Self::set_raw(host, DUCK_THRESH.id, DUCK_THRESH.snap((thresh + d * k as f32).clamp(DUCK_THRESH.min, DUCK_THRESH.max)));
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
            UiEvent::PointerUp { button: MouseButton::Left, .. } => {
                let was = self.drag.is_some();
                self.end_drag(host);
                was
            }
            UiEvent::DoubleClick { pos, .. } => {
                if let Some(Hot::Node(Node::Band(b))) = self.hot_at(host, pos) {
                    self.end_drag(host);
                    Self::set(host, &band_id(b, D_ON.id), 0.0);
                    return true;
                }
                if self.hot_at(host, pos).is_none() && PLOT.contains(pos) && pos.y < CUT_Y - CUT_GRIP.1 {
                    return self.add_band(host, pos);
                }
                let resets: &[ParamSpec] = match self.hot_at(host, pos) {
                    Some(Hot::Node(Node::Mid)) => &[DECAY],
                    Some(Hot::Node(Node::Low)) => &[LO_FREQ, LO_MULT],
                    Some(Hot::Node(Node::High)) => &[HI_FREQ, HI_MULT],
                    Some(Hot::Node(Node::Band(_))) => &[],
                    Some(Hot::Cut(false)) => &[LO_CUT],
                    Some(Hot::Cut(true)) => &[HI_CUT],
                    Some(Hot::Duck) => &[DUCK_THRESH],
                    None => &[],
                };
                if resets.is_empty() {
                    return false;
                }
                self.end_drag(host);
                for spec in resets {
                    Self::set(host, spec.id, spec.default);
                }
                true
            }
            UiEvent::Wheel { pos, lines, mods } => {
                if let Some(Hot::Node(Node::Band(b))) = self.hot_at(host, pos) {
                    let id = band_id(b, D_Q.id);
                    Self::set(host, &id, D_Q.wheel(Self::plain(host, &id), lines, mods.fine()));
                    return true;
                }
                let spec = match self.hot_at(host, pos) {
                    Some(Hot::Node(Node::Mid)) => Some(DECAY),
                    Some(Hot::Node(Node::Low)) => Some(LO_MULT),
                    Some(Hot::Node(Node::High)) => Some(HI_MULT),
                    Some(Hot::Node(Node::Band(_))) => None,
                    Some(Hot::Cut(false)) => Some(LO_CUT),
                    Some(Hot::Cut(true)) => Some(HI_CUT),
                    Some(Hot::Duck) => Some(DUCK_THRESH),
                    None => None,
                };
                match spec {
                    Some(spec) => {
                        Self::set(host, spec.id, spec.wheel(Self::plain(host, spec.id), lines, mods.fine()));
                        true
                    }
                    None => false,
                }
            }
            _ => false,
        }
    }
}

fn draw_chevrons(cx: &mut DrawCx<'_>, c: Point, ink: Color) {
    let mut chev = BezPath::new();
    chev.move_to((c.x - 3.0, c.y - 2.0));
    chev.line_to((c.x, c.y - 5.0));
    chev.line_to((c.x + 3.0, c.y - 2.0));
    chev.move_to((c.x - 3.0, c.y + 2.0));
    chev.line_to((c.x, c.y + 5.0));
    chev.line_to((c.x + 3.0, c.y + 2.0));
    cx.scene.stroke(&Stroke::new(1.3).with_caps(Cap::Round).with_join(Join::Round), Affine::IDENTITY, ink, None, &chev);
}

/// A cut's glyph: its slope, falling to the side it removes.
fn draw_cut_glyph(cx: &mut DrawCx<'_>, c: Point, high: bool, ink: Color) {
    let s = if high { 1.0 } else { -1.0 };
    let mut p = BezPath::new();
    p.move_to((c.x - s * 5.0, c.y - 4.0));
    p.line_to((c.x, c.y - 4.0));
    p.quad_to((c.x + s * 3.0, c.y - 4.0), (c.x + s * 4.0, c.y + 5.0));
    cx.scene.stroke(&Stroke::new(1.4).with_caps(Cap::Round).with_join(Join::Round), Affine::IDENTITY, ink, None, &p);
}

fn knob_words(id: &str) -> &'static str {
    match id {
        "size" => " — the room: how far apart its walls are (small rooms ring close and dense, halls spread out)",
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
        self.update_curve(cx.host);
        let theme = cx.theme;
        self.draw_header(cx);
        cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.display, None, &Rect::new(0.0, DISPLAY_TOP, VIEW_W, HELP_TOP));
        self.draw_plot(cx);
        self.draw_history(cx);
        for k in &self.knobs {
            k.draw(cx);
        }
        self.draw_meters(cx);
        self.help_text(cx.host).draw(cx, Rect::new(0.0, HELP_TOP, VIEW_W, FOOTER_TOP));
        self.footer.draw_frame(cx);
        self.footer.draw_controls(cx);
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
