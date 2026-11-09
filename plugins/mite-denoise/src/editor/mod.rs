//! The Mite DeNoise editor (docs/design/denoise-brief.md §2), in 1280×760 design units.
//!
//! The plot is what the denoiser hears, on its own scale, at the frequencies it works on: the
//! input dim, the output lit, the noise floor in use (the accent, lightly filled under), the
//! threshold over it (dashed) with two grips: at 1 kHz it sets THRESHOLD, at 10 kHz the tilt
//! (EMPHASIS). From the top hangs what each frequency is turned down by. Under the plot LEARN
//! (play noise only, click, then click again), ADAPTIVE, and the dials. IN and OUT meters on the
//! right.

use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Instant;

use mite_core::{ParamDisplay, ParamScale, ParamSpec};
use mite_dsp::denoise::{point_freq, DenoiseSettings, DISPLAY_POINTS, SILENT_DB};
use mite_ui::kurbo::{Affine, BezPath, Cap, Join, Line, Point, Rect, RoundedRect, Stroke};
use mite_ui::paint::{glow_stroke, icon_button, panel};
use mite_ui::peniko::{Color, Fill};
use mite_ui::session::{Session, SessionConfig};
use mite_ui::widgets::brand::{draw_mark, draw_title};
use mite_ui::widgets::history::smooth_path;
use mite_ui::widgets::{Footer, FooterDial, FooterToggle, Help, Knob, KnobKind, ToggleGlyph};
use mite_ui::{write_if_changed, Align, DrawCx, MouseButton, ParamHost, RecordingHost, TextStyle, UiEvent, View};

use crate::params::*;
use crate::presets::{preset_ids, FACTORY, PLUGIN};
use crate::DenoiseShared;

/// Mite DeNoise wears its accent (tokens.json `color.accent.denoise`, linen).
pub const ACCENT: &str = "denoise";

const VIEW_W: f64 = 1280.0;
const VIEW_H: f64 = 760.0;
const HEADER: f64 = 56.0;
const DISPLAY_TOP: f64 = HEADER;
const HELP_TOP: f64 = DISPLAY_TOP + 620.0;
const FOOTER_TOP: f64 = HELP_TOP + 28.0;

/// The plot and its scales: 20 Hz…20 kHz across, the denoiser's levels (dB, a full-scale sine on
/// a bin at 0) up and down, the reduction from the top.
const PLOT: Rect = Rect::new(0.0, DISPLAY_TOP, 1180.0, DISPLAY_TOP + 460.0);
const PLOT_X0: f64 = 44.0;
const PLOT_W: f64 = 1112.0;
const TOP_DB: f64 = -10.0;
const BOTTOM_DB: f64 = -130.0;
const GR_PX_PER_DB: f64 = 3.0;
const GRIP: (f64, f64) = (30.0, 20.0);
/// Where the grips sit: THRESHOLD at 1 kHz, EMPHASIS at 10 kHz.
const THRESHOLD_HZ: f64 = 1000.0;
const EMPHASIS_HZ: f64 = 10_000.0;

/// The strip under the plot.
const STRIP_TOP: f64 = PLOT.y1 + 8.0;
const LEARN: Rect = Rect::new(28.0, STRIP_TOP + 30.0, 112.0, STRIP_TOP + 114.0);
const ADAPT: Rect = Rect::new(128.0, STRIP_TOP + 30.0, 212.0, STRIP_TOP + 114.0);
const DIAL_X0: f64 = 320.0;
const DIAL_STEP: f64 = 136.0;
const DIAL_Y: f64 = STRIP_TOP + 68.0;
const DIAL_D: f64 = 56.0;

const WELLS: [f64; 2] = [1206.0, 1238.0];
const METER_TOP: f64 = DISPLAY_TOP + 20.0;
const METER_BOTTOM: f64 = DISPLAY_TOP + 592.0;
const METER_RANGE: f64 = 60.0;
const SILENT: f32 = -150.0;

/// LEARN: a ring (capture) around a stretch of noise.
const LEARN_GLYPH: &str = "M12 3 A9 9 0 1 1 11.99 3 M6.5 12 L8 10 L9.5 14 L11 9 L12.5 15 L14 10.5 L15.5 13 L17.5 12";
/// ADAPTIVE: a wave and a floor line that follows under it.
const ADAPT_GLYPH: &str = "M3 9 C6 4 8 14 11 9 S16 4 21 9 M3 17 C7 15.5 9 18 12 16.5 S17 15.5 21 17";

pub fn freq_x(f: f64) -> f64 {
    PLOT_X0 + PLOT_W * (f / 20.0).ln() / 1000f64.ln()
}

fn db_y(db: f64) -> f64 {
    PLOT.y0 + 24.0 + (TOP_DB - db.clamp(BOTTOM_DB, TOP_DB)) / (TOP_DB - BOTTOM_DB) * (PLOT.height() - 48.0)
}

fn px_per_db() -> f64 {
    (PLOT.height() - 48.0) / (TOP_DB - BOTTOM_DB)
}

fn draw_glyph(scene: &mut mite_ui::vello::Scene, d: &str, c: Point, size: f64, color: Color, width: f64) {
    let Ok(mut path) = BezPath::from_svg(d) else { return };
    let k = size / 24.0;
    path.apply_affine(Affine::translate((c.x - 12.0 * k, c.y - 12.0 * k)) * Affine::scale(k));
    scene.stroke(&Stroke::new(width).with_caps(Cap::Round).with_join(Join::Round), Affine::IDENTITY, color, None, &path);
}

/// A stand-in host holding every Mite DeNoise parameter at its default (tests, preview, tools).
pub fn memory_host() -> mite_ui::MemoryParamHost {
    fn leak(s: String) -> &'static str {
        Box::leak(s.into_boxed_str())
    }
    let flag = |id: &'static str, default: bool| ParamSpec {
        id,
        name: id,
        unit: "",
        min: 0.0,
        max: 1.0,
        default: f32::from(u8::from(default)),
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
            "bool" => flag(id, f[2] == "true"),
            _ => ParamSpec { id, ..spec_for(id).expect("every manifest line has a spec") },
        };
        host = host.with(spec);
    }
    host
}

/// The denoiser's settings as a host's parameters set them.
pub fn settings(host: &dyn ParamHost) -> DenoiseSettings {
    denoise_settings(|id| host.plain(id, host.normalized(id)))
}

pub fn out_gain(host: &dyn ParamHost) -> f32 {
    host.plain(OUT_GAIN.id, host.normalized(OUT_GAIN.id))
}

/// A grip on the threshold line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Grip {
    Threshold,
    Emphasis,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Hot {
    Grip(Grip),
    Learn,
    Adaptive,
}

/// The curves as drawn (eased toward the engine's last frame).
struct Curves {
    input: Vec<f32>,
    output: Vec<f32>,
    noise: Vec<f32>,
    threshold: Vec<f32>,
    gain: Vec<f32>,
}

impl Curves {
    fn new() -> Self {
        let row = |v| vec![v; DISPLAY_POINTS];
        Self { input: row(SILENT_DB), output: row(SILENT_DB), noise: row(SILENT_DB), threshold: row(SILENT_DB), gain: row(0.0) }
    }
}

pub struct DenoiseView {
    shared: Arc<DenoiseShared>,
    session: Session,
    footer: Footer,
    knobs: Vec<Knob>,
    curves: Curves,
    targets: Curves,
    seen_frames: u64,
    last_tick: Instant,
    started: Instant,
    pointer: Option<Point>,
    hot: Option<Hot>,
    /// A drag: the grip, where it was grabbed, and its value then.
    drag: Option<(Grip, Point, f32)>,
    /// IN and OUT as drawn.
    meters: [f32; 2],
}

impl DenoiseView {
    pub fn new(shared: Arc<DenoiseShared>) -> Self {
        let footer = Footer::new(
            FOOTER_TOP,
            (VIEW_W, VIEW_H),
            vec![
                FooterToggle { id: LISTEN.id, glyph: ToggleGlyph::Listen, words: "Noise only — hear what is taken away: nothing of the music should be in it" },
                FooterToggle { id: BYPASS.id, glyph: ToggleGlyph::Bypass, words: "Bypass — the input as it came (in time: the latency stays)" },
            ],
            vec![FooterDial { spec: OUT_GAIN, kind: KnobKind::Bipolar, name: "OUTPUT", words: "Output gain" }],
        );
        let dials: [(ParamSpec, &str, KnobKind, Option<mite_core::DialCurve>); 6] = [
            (REDUCTION, "REDUCTION", KnobKind::Unipolar, None),
            (THRESHOLD, "THRESHOLD", KnobKind::Unipolar, None),
            (ATTACK, "ATTACK", KnobKind::Unipolar, Some(ATTACK_DIAL)),
            (RELEASE, "RELEASE", KnobKind::Unipolar, Some(RELEASE_DIAL)),
            (SMOOTHING, "SMOOTHING", KnobKind::Unipolar, None),
            (EMPHASIS, "EMPHASIS", KnobKind::Bipolar, None),
        ];
        let knobs = dials
            .into_iter()
            .enumerate()
            .map(|(i, (spec, label, kind, curve))| {
                let k = Knob::new(spec, label, kind, Point::new(DIAL_X0 + i as f64 * DIAL_STEP, DIAL_Y), DIAL_D);
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
            curves: Curves::new(),
            targets: Curves::new(),
            seen_frames: 0,
            last_tick: Instant::now(),
            started: Instant::now(),
            pointer: None,
            hot: None,
            drag: None,
            meters: [SILENT, SILENT],
            shared,
        }
    }

    /// Keeps the user's presets in `dir` instead of the settings folder (tests).
    pub fn with_preset_dir(mut self, dir: std::path::PathBuf) -> Self {
        self.session = self.session.with_preset_dir(dir);
        self
    }

    /// Lets the curves land at once (tools render a single frame).
    pub fn settle(&mut self) {
        self.read_tap();
        for (c, t) in [(&mut self.curves.input, &self.targets.input), (&mut self.curves.output, &self.targets.output), (&mut self.curves.noise, &self.targets.noise), (&mut self.curves.threshold, &self.targets.threshold), (&mut self.curves.gain, &self.targets.gain)] {
            c.copy_from_slice(t);
        }
    }

    // ---- for tests and tools ----------------------------------------------------------------

    pub fn learn_button(&self) -> Rect {
        LEARN
    }

    pub fn adaptive_button(&self) -> Rect {
        ADAPT
    }

    pub fn dial(&self, id: &str) -> Option<Point> {
        self.knobs.iter().find(|k| k.id == id).map(|k| k.center)
    }

    /// A grip's centre, while there is a threshold to put it on.
    pub fn grip(&self, g: Grip) -> Option<Point> {
        let hz = match g {
            Grip::Threshold => THRESHOLD_HZ,
            Grip::Emphasis => EMPHASIS_HZ,
        };
        let db = self.curve_at(&self.curves.threshold, hz)?;
        Some(Point::new(freq_x(hz), db_y(f64::from(db))))
    }

    pub fn level_y(&self, db: f64) -> f64 {
        db_y(db)
    }

    // ---- reading ----------------------------------------------------------------------------

    fn plain(host: &dyn ParamHost, id: &str) -> f32 {
        host.plain(id, host.normalized(id))
    }

    fn set_raw(host: &dyn ParamHost, id: &str, v: f32) {
        host.set_normalized(id, host.normalize(id, v));
    }

    fn gesture(host: &dyn ParamHost, id: &str, v: f32) {
        host.begin_gesture(id);
        Self::set_raw(host, id, v);
        host.end_gesture(id);
    }

    /// A curve's value at `hz` (interpolated between points), or none while it is silent.
    fn curve_at(&self, curve: &[f32], hz: f64) -> Option<f32> {
        let x = (hz / 20.0).ln() / 1000f64.ln() * (DISPLAY_POINTS - 1) as f64;
        let i = (x.floor() as usize).min(DISPLAY_POINTS - 2);
        let t = (x - i as f64) as f32;
        let (a, b) = (curve[i], curve[i + 1]);
        (a > SILENT_DB + 1.0 && b > SILENT_DB + 1.0).then_some(a + (b - a) * t)
    }

    fn read_tap(&mut self) -> bool {
        let tap = &self.shared.tap;
        let frames = tap.frames.load(Ordering::Acquire);
        if frames == self.seen_frames {
            return false;
        }
        self.seen_frames = frames;
        for (dst, src) in [(&mut self.targets.input, &tap.input), (&mut self.targets.output, &tap.output), (&mut self.targets.noise, &tap.noise), (&mut self.targets.threshold, &tap.threshold), (&mut self.targets.gain, &tap.gain)] {
            for (d, s) in dst.iter_mut().zip(src) {
                *d = s.load();
            }
        }
        true
    }

    fn learning(&self) -> bool {
        self.shared.tap.learn.load(Ordering::Relaxed)
    }

    fn has_floor(&self, host: &dyn ParamHost) -> bool {
        Self::plain(host, ADAPTIVE.id) >= 0.5 || self.shared.profile.is_learned()
    }

    // ---- drawing ----------------------------------------------------------------------------

    fn draw_header(&mut self, cx: &mut DrawCx<'_>) {
        let theme = cx.theme;
        cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.window, None, &Rect::new(0.0, 0.0, VIEW_W, HEADER));
        cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, theme.line, None, &Line::new((0.0, HEADER - 0.5), (VIEW_W, HEADER - 0.5)));
        draw_mark(cx, 16.0, 15.0, 26.0);
        draw_title(cx, 52.0, 18.0, "DeNoise");
        self.session.draw(cx);
    }

    fn path(&self, v: &[f32], y: impl Fn(f32) -> f64) -> Vec<Point> {
        (0..DISPLAY_POINTS).map(|i| Point::new(freq_x(point_freq(i, DISPLAY_POINTS)), y(v[i]))).collect()
    }

    fn draw_plot(&self, cx: &mut DrawCx<'_>) {
        let theme = cx.theme;
        // The grid: decades and their halves, and every 20 dB.
        for f in [100.0, 1000.0, 10_000.0] {
            cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, theme.grid_minor, None, &Line::new((freq_x(f), PLOT.y0), (freq_x(f), PLOT.y1)));
        }
        for f in [50.0, 200.0, 500.0, 2000.0, 5000.0] {
            cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, theme.grid_minor.multiply_alpha(0.5), None, &Line::new((freq_x(f), PLOT.y0), (freq_x(f), PLOT.y1)));
        }
        let label = TextStyle { tabular: true, ..TextStyle::BODY.sized(10.0).weighted(600.0) };
        for (f, s) in [(100.0, "100"), (1000.0, "1k"), (10_000.0, "10k")] {
            cx.text.draw(cx.scene, s, freq_x(f) + 4.0, PLOT.y1 - 16.0, label, theme.text_lo, Align::Left);
        }
        for db in (-120..=-20).step_by(20) {
            let y = db_y(f64::from(db));
            cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, theme.grid_minor.multiply_alpha(0.5), None, &Line::new((PLOT_X0, y), (PLOT_X0 + PLOT_W, y)));
            cx.text.draw(cx.scene, &format!("−{}", -db), 8.0, y - 6.0, label, theme.text_lo, Align::Left);
        }
        let c = &self.curves;
        let y = |v: f32| db_y(f64::from(v));
        cx.begin_clip(&Rect::new(PLOT_X0, PLOT.y0, PLOT_X0 + PLOT_W, PLOT.y1));
        // The input: a quiet filled shape.
        if c.input.iter().any(|&v| v > BOTTOM_DB as f32) {
            let pts = self.path(&c.input, y);
            let curve = smooth_path(&pts);
            let mut fill = curve.clone();
            fill.line_to((pts[pts.len() - 1].x, PLOT.y1));
            fill.line_to((pts[0].x, PLOT.y1));
            fill.close_path();
            cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.spectrum_pre.multiply_alpha(0.12), None, &fill);
            cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, theme.spectrum_pre.multiply_alpha(0.45), None, &curve);
        }
        // The noise floor in use: the accent, filled lightly under.
        if c.noise.iter().any(|&v| v > SILENT_DB + 1.0) {
            let pts = self.path(&c.noise, y);
            let curve = smooth_path(&pts);
            let mut fill = curve.clone();
            fill.line_to((pts[pts.len() - 1].x, PLOT.y1));
            fill.line_to((pts[0].x, PLOT.y1));
            fill.close_path();
            cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.accent.multiply_alpha(0.10), None, &fill);
            cx.scene.stroke(&Stroke::new(1.4), Affine::IDENTITY, theme.accent.multiply_alpha(0.8), None, &curve);
        }
        // The output: a lit line.
        if c.output.iter().any(|&v| v > BOTTOM_DB as f32) {
            glow_stroke(cx, &smooth_path(&self.path(&c.output, y)), 1.4, theme.curve.multiply_alpha(0.75), theme.curve.multiply_alpha(0.6), 1.4);
        }
        // The threshold: dashed, over the floor.
        if c.threshold.iter().any(|&v| v > SILENT_DB + 1.0) {
            let hot = matches!(self.hot, Some(Hot::Grip(_))) || self.drag.is_some();
            let dash = Stroke::new(if hot { 1.8 } else { 1.4 }).with_dashes(0.0, [7.0, 5.0]);
            cx.scene.stroke(&dash, Affine::IDENTITY, theme.accent, None, &smooth_path(&self.path(&c.threshold, y)));
        }
        // What each frequency is turned down by, from the top.
        if c.gain.iter().any(|&g| g < -0.1) {
            let pts: Vec<Point> = (0..DISPLAY_POINTS).map(|i| Point::new(freq_x(point_freq(i, DISPLAY_POINTS)), PLOT.y0 + f64::from(-c.gain[i]) * GR_PX_PER_DB)).collect();
            let edge = smooth_path(&pts);
            let mut fill = edge.clone();
            fill.line_to((pts[pts.len() - 1].x, PLOT.y0));
            fill.line_to((pts[0].x, PLOT.y0));
            fill.close_path();
            cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.gain_reduction.multiply_alpha(0.25), None, &fill);
            glow_stroke(cx, &edge, 1.4, theme.gain_reduction, theme.gain_reduction, 2.0);
        }
        cx.end_clip();
        // Learning: the plot's edge breathes in the accent.
        if self.learning() {
            let t = self.started.elapsed().as_secs_f64();
            let a = 0.45 + 0.35 * (t * 4.0).sin();
            let r = RoundedRect::from_rect(Rect::new(PLOT_X0 - 4.0, PLOT.y0 + 4.0, PLOT_X0 + PLOT_W + 4.0, PLOT.y1 - 4.0), 10.0);
            glow_stroke(cx, &r, 1.5, theme.accent.multiply_alpha(a as f32), theme.accent, 6.0);
        }
        for g in [Grip::Threshold, Grip::Emphasis] {
            if let Some(p) = self.grip(g) {
                let hot = self.hot == Some(Hot::Grip(g)) || self.drag.is_some_and(|d| d.0 == g);
                let r = RoundedRect::from_rect(Rect::from_center_size(p, GRIP), 10.0);
                cx.scene.fill(Fill::NonZero, Affine::IDENTITY, if hot { theme.accent } else { theme.display }, None, &r);
                glow_stroke(cx, &r, 1.4, theme.accent, theme.accent, if hot { 4.0 } else { 2.0 });
                let ink = if hot { theme.display } else { theme.accent };
                let mut chev = BezPath::new();
                match g {
                    // Up and down.
                    Grip::Threshold => {
                        chev.move_to((p.x - 3.5, p.y - 2.0));
                        chev.line_to((p.x, p.y - 5.5));
                        chev.line_to((p.x + 3.5, p.y - 2.0));
                        chev.move_to((p.x - 3.5, p.y + 2.0));
                        chev.line_to((p.x, p.y + 5.5));
                        chev.line_to((p.x + 3.5, p.y + 2.0));
                    }
                    // A tilt: a line rising to the right.
                    Grip::Emphasis => {
                        chev.move_to((p.x - 7.0, p.y + 3.5));
                        chev.line_to((p.x + 7.0, p.y - 3.5));
                    }
                }
                cx.scene.stroke(&Stroke::new(1.4).with_caps(Cap::Round).with_join(Join::Round), Affine::IDENTITY, ink, None, &chev);
            }
        }
    }

    fn draw_strip(&self, cx: &mut DrawCx<'_>) {
        let theme = cx.theme;
        cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, theme.line, None, &Line::new((0.0, PLOT.y1 + 0.5), (PLOT.x1, PLOT.y1 + 0.5)));
        let learning = self.learning();
        let ink = icon_button(cx.scene, theme, LEARN, theme.panel_control, self.hot == Some(Hot::Learn), learning, theme.accent);
        draw_glyph(cx.scene, LEARN_GLYPH, LEARN.center(), 40.0, ink, 1.8);
        if learning {
            let t = self.started.elapsed().as_secs_f64();
            let r = RoundedRect::from_rect(LEARN.inflate(3.0, 3.0), 12.0);
            glow_stroke(cx, &r, 1.5, theme.accent.multiply_alpha((0.5 + 0.4 * (t * 4.0).sin()) as f32), theme.accent, 8.0);
        } else if self.shared.profile.is_learned() {
            // A profile is held: a small dot under the glyph.
            cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.accent, None, &mite_ui::kurbo::Circle::new((LEARN.center().x, LEARN.y1 - 9.0), 2.5));
        }
        let adaptive = Self::plain(cx.host, ADAPTIVE.id) >= 0.5;
        let ink = icon_button(cx.scene, theme, ADAPT, theme.panel_control, self.hot == Some(Hot::Adaptive), adaptive, theme.accent);
        draw_glyph(cx.scene, ADAPT_GLYPH, ADAPT.center(), 40.0, ink, 1.8);
        let label = TextStyle::LABEL.sized(9.0);
        cx.text.draw(cx.scene, "LEARN", LEARN.center().x, LEARN.y1 + 10.0, label, theme.text_lo, Align::Center);
        cx.text.draw(cx.scene, "ADAPTIVE", ADAPT.center().x, ADAPT.y1 + 10.0, label, theme.text_lo, Align::Center);
        for k in &self.knobs {
            k.draw(cx);
        }
    }

    fn draw_meters(&self, cx: &mut DrawCx<'_>) {
        let theme = cx.theme;
        cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, theme.line, None, &Line::new((PLOT.x1 + 0.5, DISPLAY_TOP), (PLOT.x1 + 0.5, HELP_TOP)));
        let seg = Stroke::new(8.0).with_dashes(0.0, [2.0, 2.0]);
        let (top, bottom) = (METER_TOP + 4.0, METER_BOTTOM - 4.0);
        let y = |db: f32| bottom - ((f64::from(db) + METER_RANGE).clamp(0.0, METER_RANGE) / METER_RANGE * (bottom - top) / 4.0).round() * 4.0;
        for (i, x) in WELLS.iter().enumerate() {
            panel(cx.scene, RoundedRect::new(*x, METER_TOP, x + 20.0, METER_BOTTOM, 3.0), theme.meter_well, theme.well_border);
            let cx0 = x + 10.0;
            cx.scene.stroke(&seg, Affine::IDENTITY, theme.meter_off, None, &Line::new((cx0, top), (cx0, bottom)));
            let v = self.meters[i];
            let line = Line::new((cx0, bottom), (cx0, y(v)));
            if (line.p1.y - line.p0.y).abs() >= 2.0 {
                let color = if i == 0 { theme.spectrum_pre } else { theme.curve };
                cx.scene.stroke(&seg, Affine::IDENTITY, color, None, &line);
            }
        }
        for (x, t) in [(WELLS[0], "IN"), (WELLS[1], "OUT")] {
            cx.text.draw(cx.scene, t, x + 10.0, DISPLAY_TOP + 596.0, TextStyle::LABEL.sized(9.0), theme.text_lo, Align::Center);
        }
    }

    // ---- help -------------------------------------------------------------------------------

    fn help_text(&self, host: &dyn ParamHost) -> Help {
        let slot = |spec: &ParamSpec| {
            let v = Self::plain(host, spec.id);
            Help::default().text(format!("{} ", spec.name)).slot(spec.format(v), spec.format(spec.max))
        };
        if let Some((g, _, _)) = self.drag {
            return match g {
                Grip::Threshold => slot(&THRESHOLD).text(" over the noise floor — a sound this far over it passes untouched; under it, it is turned down. Shift is fine."),
                Grip::Emphasis => slot(&EMPHASIS).text(" at 10 kHz (and the opposite at 100 Hz) — tilt it up to take more of a hiss, down for a rumble or hum. Shift is fine."),
            };
        }
        match self.hot {
            Some(Hot::Grip(Grip::Threshold)) => return slot(&THRESHOLD).text(" — drag the line up to take more, down to keep more; wheel; double-click: +6 dB"),
            Some(Hot::Grip(Grip::Emphasis)) => return slot(&EMPHASIS).text(" — drag to tilt the threshold about 1 kHz; double-click: flat"),
            Some(Hot::Learn) => {
                return Help::default().text(if self.learning() {
                    "Learning the noise — let it play a stretch with nothing but the noise, then click again to keep it"
                } else if self.shared.profile.is_learned() {
                    "Learn again — click, play a stretch of noise only, click again (with ADAPTIVE off, this profile is the floor). Double-click: forget it."
                } else {
                    "Learn — click, play a stretch of noise only (a pause, the room), click again: that is the floor (switch ADAPTIVE off to use it)"
                });
            }
            Some(Hot::Adaptive) => return Help::default().text("Adaptive — follows the noise floor all the time, for noise that changes; off, the learned profile is the floor"),
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
        if self.learning() {
            return Help::default().text("Learning the noise — click LEARN again when a stretch of noise only has played");
        }
        if !self.has_floor(host) {
            return Help::default().text("No noise floor yet: switch ADAPTIVE on, or LEARN one from a stretch of noise only.");
        }
        Help::default().text("Dim: the input. Lit: the output. The accent line: the noise floor; dashed over it, the threshold (drag its grips). From the top: how far each frequency is turned down.")
    }

    // ---- events -----------------------------------------------------------------------------

    fn busy(&self) -> bool {
        self.drag.is_some() || self.knobs.iter().any(Knob::is_dragging) || self.footer.busy()
    }

    fn hot_at(&self, p: Point) -> Option<Hot> {
        for g in [Grip::Threshold, Grip::Emphasis] {
            if self.grip(g).is_some_and(|c| Rect::from_center_size(c, GRIP).inflate(4.0, 4.0).contains(p)) {
                return Some(Hot::Grip(g));
            }
        }
        if LEARN.contains(p) {
            return Some(Hot::Learn);
        }
        if ADAPT.contains(p) {
            return Some(Hot::Adaptive);
        }
        None
    }

    fn grip_spec(g: Grip) -> ParamSpec {
        match g {
            Grip::Threshold => THRESHOLD,
            Grip::Emphasis => EMPHASIS,
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
                if let Some((g, start, v0)) = self.drag {
                    let k = if mods.fine() { 0.2 } else { 1.0 };
                    let spec = Self::grip_spec(g);
                    let v = v0 - ((pos.y - start.y) / px_per_db()) as f32 * k;
                    Self::set_raw(host, spec.id, spec.snap(v.clamp(spec.min, spec.max)));
                    return true;
                }
                self.hot = self.hot_at(pos);
                true
            }
            UiEvent::PointerLeft => {
                self.pointer = None;
                self.hot = None;
                true
            }
            UiEvent::PointerDown { pos, button: MouseButton::Left, .. } => match self.hot_at(pos) {
                Some(Hot::Grip(g)) => {
                    let id = Self::grip_spec(g).id;
                    host.begin_gesture(id);
                    self.drag = Some((g, pos, Self::plain(host, id)));
                    true
                }
                Some(Hot::Learn) => {
                    let tap = &self.shared.tap;
                    tap.learn.store(!tap.learn.load(Ordering::Relaxed), Ordering::Relaxed);
                    true
                }
                Some(Hot::Adaptive) => {
                    Self::gesture(host, ADAPTIVE.id, if Self::plain(host, ADAPTIVE.id) >= 0.5 { 0.0 } else { 1.0 });
                    true
                }
                None => false,
            },
            UiEvent::PointerUp { button: MouseButton::Left, .. } => {
                if let Some((g, _, _)) = self.drag.take() {
                    host.end_gesture(Self::grip_spec(g).id);
                    return true;
                }
                false
            }
            UiEvent::DoubleClick { pos, .. } => match self.hot_at(pos) {
                Some(Hot::Grip(g)) => {
                    self.drag = None;
                    let spec = Self::grip_spec(g);
                    Self::gesture(host, spec.id, spec.default);
                    true
                }
                Some(Hot::Learn) => {
                    // The two clicks before it started and stopped learning; this forgets.
                    self.shared.tap.learn.store(false, Ordering::Relaxed);
                    self.shared.profile.clear();
                    true
                }
                _ => false,
            },
            UiEvent::Wheel { pos, lines, mods } => match self.hot_at(pos) {
                Some(Hot::Grip(g)) => {
                    let spec = Self::grip_spec(g);
                    let v = spec.wheel(Self::plain(host, spec.id), lines, mods.fine());
                    write_if_changed(host, spec.id, host.normalize(spec.id, v));
                    true
                }
                _ => false,
            },
            _ => false,
        }
    }
}

fn knob_words(id: &str) -> &'static str {
    match id {
        "reduction" => " — the most any frequency is turned down while it sits in the noise",
        "threshold" => " — how far over the noise floor a sound must rise to pass untouched (or drag the dashed line)",
        "attack" => " — how fast a frequency opens when a sound rises out of the noise",
        "release" => " — how fast it closes after: longer is smoother, shorter is cleaner between words",
        "smoothing" => " — the decision spread across neighbouring frequencies: more is calmer (no warbling), less is more exact",
        "emphasis" => " — the threshold tilted about 1 kHz: up takes more of a hiss, down more of a rumble",
        _ => "",
    }
}

impl View for DenoiseView {
    fn logical_size(&self) -> (f64, f64) {
        (VIEW_W, VIEW_H)
    }

    fn draw(&mut self, cx: &mut DrawCx<'_>) {
        let theme = cx.theme;
        self.draw_header(cx);
        cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.display, None, &Rect::new(0.0, DISPLAY_TOP, VIEW_W, HELP_TOP));
        self.draw_plot(cx);
        self.draw_strip(cx);
        self.draw_meters(cx);
        self.help_text(cx.host).draw(cx, Rect::new(0.0, HELP_TOP, VIEW_W, FOOTER_TOP));
        self.footer.draw_frame(cx);
        let fs = f64::from(self.shared.sample_rate.load()).max(1.0);
        self.footer.draw_latency(cx, 24.0, f64::from(self.shared.latency.load()) / fs * 1000.0);
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
        let dt = self.last_tick.elapsed().as_secs_f64().min(0.1);
        self.last_tick = Instant::now();
        let fresh = self.read_tap();
        // Ease: spectra 60 ms up, 300 ms down; the floor, threshold and gain 100 ms.
        let (rise, fall, even) = ((1.0 - (-dt / 0.06).exp()) as f32, (1.0 - (-dt / 0.3).exp()) as f32, (1.0 - (-dt / 0.1).exp()) as f32);
        let mut moved = false;
        let mut ease = |c: &mut Vec<f32>, t: &Vec<f32>, up: f32, down: f32| {
            for (v, &to) in c.iter_mut().zip(t) {
                // From silence (or to it), jump: no sweep in from the floor of the scale.
                let next = if *v <= SILENT_DB + 1.0 || to <= SILENT_DB + 1.0 { to } else { *v + (to - *v) * if to > *v { up } else { down } };
                moved |= (next - *v).abs() > 0.02;
                *v = next;
            }
        };
        ease(&mut self.curves.input, &self.targets.input, rise, fall);
        ease(&mut self.curves.output, &self.targets.output, rise, fall);
        ease(&mut self.curves.noise, &self.targets.noise, even, even);
        ease(&mut self.curves.threshold, &self.targets.threshold, even, even);
        ease(&mut self.curves.gain, &self.targets.gain, even, even);
        let (pin, pout) = self.shared.tap.take_peaks();
        let fall_db = (20.0 * dt) as f32;
        for (m, t) in self.meters.iter_mut().zip([pin, pout]) {
            *m = if t > *m { t } else { (*m - fall_db).max(t).max(SILENT) };
        }
        let _ = host;
        fresh || moved || busy || self.learning() || self.meters.iter().any(|&m| m > SILENT + 1.0)
    }
}
