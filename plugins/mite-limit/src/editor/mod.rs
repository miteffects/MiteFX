//! The Mite Limit editor (docs/design/limit-brief.md §3), in 1280×760 design units.
//!
//! The display: the history on the left (shared with Comp, `mite_ui::widgets::history`): the
//! input after the gain dim, the output lit, the gain reduction hanging from the top, the
//! ceiling as a line with its grip. Drag the waveform up or down for gain, the grip for the
//! ceiling. Under it the control strip: the styles (each glyph a drawing of its gain shape) and
//! the dials. On the right the meters (IN, GR, OUT) and the loudness column, the one place the
//! suite shows numbers (user, 2026-11-03). Everything drawn comes from the engine's records.

use std::collections::VecDeque;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Instant;

use mite_core::{ParamDisplay, ParamScale, ParamSpec};
use mite_dsp::loudness::SILENCE_LUFS;
use mite_ui::kurbo::{Affine, BezPath, Cap, Join, Line, Point, Rect, RoundedRect, Stroke};
use mite_ui::paint::{glow_stroke, icon_button, panel};
use mite_ui::peniko::{Color, Fill, Gradient};
use mite_ui::session::{Session, SessionConfig};
use mite_ui::widgets::brand::{draw_mark, draw_title};
use mite_ui::widgets::footer::draw_dropdown_button;
use mite_ui::widgets::history::{self, HistoryLane};
use mite_ui::widgets::{ease_range, Dropdown, DropdownEvent, Footer, FooterDial, FooterToggle, Help, Knob, KnobKind, RangePicker, ToggleGlyph};
use mite_ui::{write_if_changed, Align, DrawCx, Key, Modifiers, MouseButton, ParamHost, RecordingHost, TextStyle, UiEvent, View};

use crate::dsp::{record, FACTORS, HISTORY_RECORDS, HISTORY_SLICE_MS};
use crate::params::*;
use crate::presets::{preset_ids, FACTORY, PLUGIN};
use crate::LimitShared;

/// Mite Limit wears the Limit accent (tokens.json `color.accent.limit`, silver).
pub const ACCENT: &str = "limit";

const WIDTH: f64 = 1280.0;
const HEIGHT: f64 = 760.0;
const HEADER: f64 = 56.0;
const DISPLAY_TOP: f64 = HEADER;
const DISPLAY_H: f64 = 620.0;
const HELP_TOP: f64 = DISPLAY_TOP + DISPLAY_H;
const FOOTER_TOP: f64 = HELP_TOP + 28.0;

/// The history: 6 s across the left 936 px, down to the control strip.
const HIST_W: f64 = 936.0;
const HIST_BOTTOM: f64 = DISPLAY_TOP + 468.0;
const HIST_SECONDS: f64 = 6.0;
/// The level lane: 0 dB 24 px under the display's top, the range's floor this much lower.
const LEVEL_SPAN_PX: f64 = HIST_BOTTOM - DISPLAY_TOP - 24.0 - 12.0;
const LEVEL_RANGES: &[f32] = &[12.0, 24.0, 36.0, 48.0];
const LEVEL_RANGE_LABELS: &[&str] = &["Auto", "12 dB", "24 dB", "36 dB", "48 dB"];
/// The ceiling's grip at the history's right edge.
const GRIP_X: f64 = HIST_W - 40.0;
const GRIP_SIZE: (f64, f64) = (34.0, 22.0);

/// The control strip under the history.
const STRIP_TOP: f64 = HIST_BOTTOM + 8.0;
const STYLE_BUTTON: f64 = 40.0;
const STYLE_X: f64 = 24.0;
const DIAL_Y: f64 = STRIP_TOP + 70.0;
const DIAL_D: f64 = 54.0;

/// The meter column and the loudness column.
const COLUMN_X: f64 = HIST_W;
const METER_TOP: f64 = DISPLAY_TOP + 20.0;
const METER_BOTTOM: f64 = DISPLAY_TOP + 592.0;
const WELLS: [f64; 3] = [952.0, 980.0, 1008.0];
const LOUD_X0: f64 = 1050.0;
const LOUD_X1: f64 = 1264.0;
/// Meter release (tokens.json `meter.releaseDbPerSec`).
const METER_RELEASE_DB_S: f32 = 20.0;
/// The meters' and the history's floor when nothing has played.
const SILENT: f32 = -150.0;

/// Style glyphs (24×24, stroked): each a drawing of its gain shape (the last: sliders).
const STYLE_GLYPHS: [&str; 6] = [
    "M3 7 H7 C9.5 7 10 15 12 15 C14 15 15.5 7 21 7",
    "M3 7 H9 C10.5 7 11 18 12 18 C13 18 13.6 7 15 7 H21",
    "M3 6 C5.5 6 6.5 16 8.5 16 H14 C16.5 16 17.5 10.5 21 9.5",
    "M3 8 C8 8 9 13 12 13 C15 13 16 8 21 8",
    "M12 3.5 L18.5 6.2 V11 C18.5 15.6 15.6 19 12 20.5 C8.4 19 5.5 15.6 5.5 11 V6.2 Z M8.5 11.5 L11 14 L15.5 9",
    "M6 4 V20 M12 4 V20 M18 4 V20 M4 8 H8 M10 15 H14 M16 10 H20",
];
const OVERSAMPLE_GLYPH: &str = "M3 12 C5.5 5 8.5 5 11 12 S16.5 19 19 12 M5 20 H6 M9 20 H10 M13 20 H14 M17 20 H18";

thread_local! {
    static LEVEL_RANGE: std::cell::Cell<f64> = const { std::cell::Cell::new(24.0) };
    /// How far above 0 dB the history reaches: the input after the gain goes over, and it must
    /// stay in view (user, 2026-11-03).
    static HEADROOM: std::cell::Cell<f64> = const { std::cell::Cell::new(0.0) };
}

fn set_headroom(db: f32) {
    HEADROOM.with(|h| h.set(f64::from(db)));
}

fn headroom() -> f64 {
    HEADROOM.with(std::cell::Cell::get)
}

fn set_level_range(range: f32) {
    LEVEL_RANGE.with(|r| r.set(f64::from(range)));
}

fn level_range() -> f64 {
    LEVEL_RANGE.with(std::cell::Cell::get)
}

fn history_lane() -> HistoryLane {
    HistoryLane { x0: 0.0, width: HIST_W, top: DISPLAY_TOP, bottom: HIST_BOTTOM, seconds: HIST_SECONDS, slice_ms: HISTORY_SLICE_MS }
}

/// A level's y on the history, at the range and headroom as drawn: the top line is the headroom
/// (0 dB unless the input goes over).
fn lv_y(db: f32) -> f64 {
    let (range, head) = (level_range(), headroom());
    DISPLAY_TOP + 24.0 + (head - f64::from(db)).clamp(-1.0, range + head + 1.0) / (range + head) * LEVEL_SPAN_PX
}

/// Pixels per dB on the history: gain reduction is drawn at the levels' own scale.
fn px_per_db() -> f64 {
    LEVEL_SPAN_PX / (level_range() + headroom())
}

fn draw_glyph(scene: &mut mite_ui::vello::Scene, d: &str, c: Point, size: f64, color: Color, width: f64) {
    let Ok(mut path) = BezPath::from_svg(d) else { return };
    let k = size / 24.0;
    path.apply_affine(Affine::translate((c.x - 12.0 * k, c.y - 12.0 * k)) * Affine::scale(k));
    scene.stroke(&Stroke::new(width).with_caps(Cap::Round).with_join(Join::Round), Affine::IDENTITY, color, None, &path);
}

fn fmt_db(v: f32) -> String {
    if v <= -119.0 {
        "—".to_owned()
    } else {
        format!("{}{:.1}", if v < -0.05 { "−" } else if v > 0.05 { "+" } else { "" }, v.abs())
    }
}

/// A stand-in host holding every Mite Limit parameter at its default, for tests, the preview
/// and benchmarks.
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
    let specs = [GAIN, CEILING, LOOKAHEAD, ATTACK, RELEASE, SLOW, LINK_T, LINK_R, CLIP_LEVEL, PROTECT, TARGET_CUSTOM];
    let mut host = mite_ui::MemoryParamHost::new();
    for line in manifest().lines() {
        let f: Vec<&str> = line.split(" | ").collect();
        let id = leak(f[0].to_owned());
        let spec = match f[1] {
            "bool" => index(id, 2, usize::from(f[2] == "true")),
            "choice" => index(id, f[2].split(", ").count(), f[3].parse().unwrap_or(0)),
            _ => *specs.iter().find(|s| s.id == id).expect("every manifest line has a spec"),
        };
        host = host.with(spec);
    }
    host
}

/// The engine's settings as a host's parameters set them (as `LimitParams::settings` does).
pub fn settings(host: &dyn ParamHost) -> crate::dsp::LimitSettings {
    let p = |id: &str| host.plain(id, host.normalized(id));
    crate::dsp::LimitSettings {
        gain_db: p(GAIN.id),
        ceiling_db: p(CEILING.id),
        style: p(STYLE.id).round().max(0.0) as usize,
        lookahead_ms: p(LOOKAHEAD.id),
        attack_ms: p(ATTACK.id),
        release_ms: p(RELEASE.id),
        slow: p(SLOW.id) / 100.0,
        link_transients: p(LINK_T.id) / 100.0,
        link_release: p(LINK_R.id) / 100.0,
        true_peak: p(TRUE_PEAK.id) >= 0.5,
        oversampling: factor(p(OVERSAMPLING.id).round() as i32),
        delta: p(DELTA.id) >= 0.5,
        bypass: p(BYPASS.id) >= 0.5,
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Drag {
    /// The ceiling's grip.
    Ceiling { start: Point, ceiling: f32 },
    /// The waveform: gain, at the levels' scale (the input follows the pointer).
    Gain { start: Point, gain: f32 },
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Hot {
    Ceiling,
    History,
    Style(usize),
    Loudness,
    Oversampling,
}

pub struct LimitView {
    shared: Arc<LimitShared>,
    session: Session,
    footer: Footer,
    range: RangePicker,
    range_shown: f32,
    range_target: f32,
    /// The history's reach over 0 dB, as drawn, and where it goes.
    headroom: f32,
    headroom_target: f32,
    knobs: Vec<Knob>,
    /// Whether the knobs were made for Custom (its SLOW dial).
    knobs_custom: Option<bool>,
    pointer: Option<Point>,
    hot: Option<Hot>,
    drag: Option<Drag>,
    os_menu: Option<Dropdown>,
    os_hit: Rect,
    history: VecDeque<[f32; record::WIDTH]>,
    seen: usize,
    /// IN L/R, GR L/R, OUT L/R as drawn.
    meters: [f32; 6],
    last_fresh: Instant,
    last_tick: Instant,
}

impl LimitView {
    pub fn new(shared: Arc<LimitShared>) -> Self {
        let footer = Footer::new(
            FOOTER_TOP,
            (WIDTH, HEIGHT),
            vec![
                FooterToggle { id: TRUE_PEAK.id, glyph: ToggleGlyph::TruePeak, words: "True peak — the ceiling holds between the samples too, as a converter or an encoder hears it" },
                FooterToggle { id: DELTA.id, glyph: ToggleGlyph::Delta, words: "Delta — hear only what the limiter takes away, time-aligned" },
                FooterToggle { id: BYPASS.id, glyph: ToggleGlyph::Bypass, words: "Bypass — the input, latency kept" },
            ],
            vec![
                FooterDial { spec: GAIN, kind: KnobKind::Unipolar, name: "GAIN", words: "Gain — how hard the music is pushed into the ceiling (or drag the waveform)" },
                FooterDial { spec: CEILING, kind: KnobKind::Unipolar, name: "CEILING", words: "Ceiling — the most the output ever reaches (or drag its grip)" },
            ],
        );
        let range = RangePicker::new(
            Rect::new(8.0, DISPLAY_TOP + 4.0, 54.0, DISPLAY_TOP + 22.0),
            Rect::new(0.0, DISPLAY_TOP + 24.0, 36.0, HIST_BOTTOM),
            LEVEL_RANGES,
            LEVEL_RANGE_LABELS,
            shared.display_range.clone(),
            (DISPLAY_TOP, HELP_TOP),
        )
        .with_prefix("−");
        Self {
            session: Session::new(SessionConfig { plugin: PLUGIN, factory: FACTORY, sound_ids: preset_ids(), snapshot_ids: preset_ids() }, shared.snapshots.clone(), shared.preset.clone()),
            footer,
            range,
            range_shown: 24.0,
            range_target: 24.0,
            headroom: 0.0,
            headroom_target: 0.0,
            knobs: Vec::new(),
            knobs_custom: None,
            pointer: None,
            hot: None,
            drag: None,
            os_menu: None,
            os_hit: Rect::ZERO,
            history: VecDeque::with_capacity(HISTORY_RECORDS),
            seen: 0,
            meters: [SILENT, SILENT, 0.0, 0.0, SILENT, SILENT],
            last_fresh: Instant::now(),
            last_tick: Instant::now(),
            shared,
        }
    }

    /// Keeps the user's presets in `dir` instead of the settings folder (tests).
    pub fn with_preset_dir(mut self, dir: std::path::PathBuf) -> Self {
        self.session = self.session.with_preset_dir(dir);
        self
    }

    pub fn session(&self) -> &Session {
        &self.session
    }

    /// The level range as drawn, dB.
    pub fn range_shown(&self) -> f32 {
        self.range_shown
    }

    /// Lets the range settle at once (tools render a single frame).
    pub fn settle(&mut self) {
        self.range_shown = self.range_target;
        self.headroom = self.headroom_target;
        set_level_range(self.range_shown);
        set_headroom(self.headroom);
    }

    /// The history's reach over 0 dB, as drawn.
    pub fn headroom(&self) -> f32 {
        self.headroom
    }

    /// The ceiling's grip (tests and tools).
    pub fn ceiling_grip(&self, host: &dyn ParamHost) -> Rect {
        Self::grip_rect(host)
    }

    /// The y of a level on the history (tests and tools).
    pub fn level_y(&self, db: f32) -> f64 {
        set_level_range(self.range_shown);
        set_headroom(self.headroom);
        lv_y(db)
    }

    /// A style's button (tests and tools).
    pub fn style_button(&self, i: usize) -> Rect {
        Self::style_rect(i)
    }

    /// A dial's centre by parameter id (tests and tools).
    pub fn dial(&self, id: &str) -> Option<Point> {
        self.knobs.iter().find(|k| k.id == id).map(|k| k.center)
    }

    pub fn loudness_rect(&self) -> Rect {
        Rect::new(LOUD_X0 - 8.0, DISPLAY_TOP + 8.0, LOUD_X1 + 8.0, HELP_TOP - 8.0)
    }

    pub fn open_oversampling(&mut self, host: &dyn ParamHost) {
        self.os_menu = Some(Self::os_list(host));
    }

    pub fn oversampling_rows(&self) -> Vec<Rect> {
        self.os_menu.as_ref().map(Dropdown::item_rects).unwrap_or_default()
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

    fn style(host: &dyn ParamHost) -> usize {
        (Self::plain(host, STYLE.id).round().max(0.0) as usize).min(STYLE_NAMES.len() - 1)
    }

    fn os_list(host: &dyn ParamHost) -> Dropdown {
        let current = Self::plain(host, OVERSAMPLING.id).round().max(0.0) as usize;
        let anchor = Rect::new(16.0, FOOTER_TOP + 9.0, 126.0, FOOTER_TOP + 47.0);
        Dropdown::new(anchor, OVERSAMPLING.choices, current, (DISPLAY_TOP, FOOTER_TOP)).with_min_width(150.0)
    }

    // ---- geometry ---------------------------------------------------------------------------

    fn grip_rect(host: &dyn ParamHost) -> Rect {
        let y = lv_y(Self::plain(host, CEILING.id));
        Rect::new(GRIP_X, y - GRIP_SIZE.1 / 2.0, GRIP_X + GRIP_SIZE.0, y + GRIP_SIZE.1 / 2.0)
    }

    fn style_rect(i: usize) -> Rect {
        let (col, row) = ((i % 3) as f64, (i / 3) as f64);
        let x = STYLE_X + col * (STYLE_BUTTON + 8.0);
        let y = STRIP_TOP + 22.0 + row * (STYLE_BUTTON + 8.0);
        Rect::new(x, y, x + STYLE_BUTTON, y + STYLE_BUTTON)
    }

    fn since_fresh(&self) -> f64 {
        (self.last_fresh.elapsed().as_secs_f64() * 1000.0 / f64::from(HISTORY_SLICE_MS)).min(4.0)
    }

    // ---- drawing ----------------------------------------------------------------------------

    fn draw_header(&mut self, cx: &mut DrawCx<'_>) {
        let theme = cx.theme;
        cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.window, None, &Rect::new(0.0, 0.0, WIDTH, HEADER));
        cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, theme.line, None, &Line::new((0.0, HEADER - 0.5), (WIDTH, HEADER - 0.5)));
        draw_mark(cx, 16.0, 15.0, 26.0);
        draw_title(cx, 52.0, 18.0, "Limit");
        self.session.draw(cx);
    }

    fn draw_history(&self, cx: &mut DrawCx<'_>) {
        let theme = cx.theme;
        let lane = history_lane();
        cx.scene.push_layer(Fill::NonZero, mite_ui::peniko::Mix::Normal, 1.0, Affine::IDENTITY, &lane.rect());
        lane.draw_grid(cx, self.range_target, headroom() as f32, lv_y);
        let cols = lane.columns(&self.history, self.seen, self.since_fresh());
        if cols.len() >= 2 {
            type Col = history::Column<{ record::WIDTH }>;
            let delta = Self::on(cx.host, DELTA.id);
            let level = |c: &Col, k: usize| c.high[k].max(c.high[k + 1]);
            let line = |f: &dyn Fn(&Col) -> f64| history::column_line(&cols, f);
            // IN (after the gain): its peaks, dim; its line is drawn last, on top.
            let input = line(&|c| lv_y(level(c, record::IN)));
            cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.spectrum_pre.multiply_alpha(0.16), None, &history::close_to(input.clone(), &cols, HIST_BOTTOM));
            // OUT, lit (what was taken away under Delta).
            let output = if delta {
                line(&|c| {
                    let (i, o) = (mite_dsp::db_to_gain(level(c, record::IN)), mite_dsp::db_to_gain(level(c, record::OUT)));
                    lv_y(mite_dsp::gain_to_db((i - o).abs().max(1e-6)))
                })
            } else {
                line(&|c| lv_y(level(c, record::OUT)))
            };
            let a = if delta { 0.3 } else { 0.12 };
            let look = history::LevelLook { fill: [theme.curve.multiply_alpha(a), theme.curve.multiply_alpha(0.01)], span: (DISPLAY_TOP, HIST_BOTTOM), edge: theme.curve, width: 1.8, glow: 2.4 };
            history::draw_level(cx, &cols, &output, HIST_BOTTOM, look);
            // GR from the top, at the levels' scale.
            if cols.iter().any(|c| c.low[record::GR].min(c.low[record::GR + 1]) < -0.05) {
                let edge = line(&|c| DISPLAY_TOP + (f64::from(-c.low[record::GR].min(c.low[record::GR + 1])) * px_per_db()).clamp(0.0, HIST_BOTTOM - DISPLAY_TOP));
                history::draw_reduction(cx, &cols, &edge, DISPLAY_TOP, 0.5 * LEVEL_SPAN_PX);
            }
            // The input's line on top of everything: always in view.
            cx.scene.stroke(&Stroke::new(1.2), Affine::IDENTITY, theme.spectrum_pre.multiply_alpha(0.85), None, &input);
        }
        cx.scene.pop_layer();
        self.draw_ceiling(cx);
        self.range.draw(cx, self.range_shown);
    }

    fn draw_ceiling(&self, cx: &mut DrawCx<'_>) {
        let theme = cx.theme;
        let hot = self.hot == Some(Hot::Ceiling) || matches!(self.drag, Some(Drag::Ceiling { .. }));
        let r = Self::grip_rect(cx.host);
        let y = r.center().y;
        let line = Line::new((0.0, y), (GRIP_X - 2.0, y));
        glow_stroke(cx, &line, 1.5, theme.accent, theme.accent, if hot { 4.0 } else { 2.5 });
        let shape = RoundedRect::from_rect(r, 11.0);
        cx.scene.fill(Fill::NonZero, Affine::IDENTITY, if hot { theme.accent } else { theme.display }, None, &shape);
        glow_stroke(cx, &shape, 1.5, theme.accent, theme.accent, if hot { 4.0 } else { 2.0 });
        let ink = if hot { theme.display } else { theme.accent };
        // A ceiling mark: a bar with a crest pressed under it.
        let c = r.center();
        let mut mark = BezPath::new();
        mark.move_to((c.x - 7.0, c.y - 4.0));
        mark.line_to((c.x + 7.0, c.y - 4.0));
        mark.move_to((c.x - 7.0, c.y + 5.0));
        mark.curve_to((c.x - 3.0, c.y + 5.0), (c.x - 2.5, c.y - 1.5), (c.x, c.y - 1.5));
        mark.curve_to((c.x + 2.5, c.y - 1.5), (c.x + 3.0, c.y + 5.0), (c.x + 7.0, c.y + 5.0));
        cx.scene.stroke(&Stroke::new(1.4).with_caps(Cap::Round).with_join(Join::Round), Affine::IDENTITY, ink, None, &mark);
    }

    fn draw_strip(&self, cx: &mut DrawCx<'_>) {
        let theme = cx.theme;
        cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, theme.line, None, &Line::new((0.0, HIST_BOTTOM + 0.5), (HIST_W, HIST_BOTTOM + 0.5)));
        let current = Self::style(cx.host);
        for (i, glyph) in STYLE_GLYPHS.iter().enumerate() {
            let r = Self::style_rect(i);
            let hot = self.hot == Some(Hot::Style(i));
            let ink = icon_button(cx.scene, theme, r, theme.panel_control, hot, i == current, theme.accent);
            draw_glyph(cx.scene, glyph, r.center(), 24.0, ink, 1.6);
        }
        for k in &self.knobs {
            k.draw(cx);
        }
    }

    fn draw_meters(&self, cx: &mut DrawCx<'_>) {
        let theme = cx.theme;
        cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, theme.line, None, &Line::new((COLUMN_X + 0.5, DISPLAY_TOP), (COLUMN_X + 0.5, HELP_TOP)));
        for x in WELLS {
            panel(cx.scene, RoundedRect::new(x, METER_TOP, x + 20.0, METER_BOTTOM, 3.0), theme.meter_well, theme.well_border);
        }
        let seg = Stroke::new(8.0).with_dashes(0.0, [2.0, 2.0]);
        let (top, bottom) = (METER_TOP + 4.0, METER_BOTTOM - 4.0);
        let span = bottom - top;
        // The history's scale: levels over the range, gain reduction at the same dB per pixel.
        let (range, head) = (level_range(), headroom());
        let level_y = |db: f32| bottom - ((f64::from(db) + range).clamp(0.0, range + head) / (range + head) * span / 4.0).round() * 4.0;
        let gr_y = |g: f32| top + (f64::from(-g).clamp(0.0, range + head) / (range + head) * span).min(span);
        let ceiling = Self::plain(cx.host, CEILING.id);
        for (i, x) in WELLS.iter().flat_map(|&w| [w + 5.0, w + 15.0]).enumerate() {
            cx.scene.stroke(&seg, Affine::IDENTITY, theme.meter_off, None, &Line::new((x, top), (x, bottom)));
            let v = self.meters[i];
            let (line, color) = match i {
                0 | 1 => (Line::new((x, bottom), (x, level_y(v))), theme.spectrum_pre),
                2 | 3 => (Line::new((x, top), (x, gr_y(v))), theme.gain_reduction),
                // Over the ceiling (never, in theory) in danger.
                _ => (Line::new((x, bottom), (x, level_y(v))), if v > ceiling + 0.05 { theme.danger } else { theme.curve }),
            };
            if (line.p1.y - line.p0.y).abs() >= 2.0 {
                cx.scene.stroke(&seg, Affine::IDENTITY, color, None, &line);
                if i >= 2 {
                    cx.glow().stroke(&Stroke::new(8.0), Affine::IDENTITY, color.multiply_alpha(0.3), None, &line);
                }
            }
        }
        // The ceiling across the OUT meter.
        let y = level_y(ceiling).min(bottom);
        cx.scene.stroke(&Stroke::new(2.0), Affine::IDENTITY, theme.accent, None, &Line::new((WELLS[2] - 2.0, y), (WELLS[2] + 22.0, y)));
        for (x, t) in [(WELLS[0], "IN"), (WELLS[1], "GR"), (WELLS[2], "OUT")] {
            cx.text.draw(cx.scene, t, x + 10.0, DISPLAY_TOP + 596.0, TextStyle::LABEL.sized(9.0), theme.text_lo, Align::Center);
        }
    }

    /// The loudness column: the one place with numbers (user, 2026-11-03).
    fn draw_loudness(&self, cx: &mut DrawCx<'_>) {
        let theme = cx.theme;
        let tap = &self.shared.loudness;
        let (m, s, i, lra, tp) = (tap.momentary.load(), tap.short_term.load(), tap.integrated.load(), tap.range.load(), tap.true_peak_max.load());
        let hot = self.hot == Some(Hot::Loudness);
        let label = TextStyle::LABEL.sized(9.0);
        let number = |size: f32| TextStyle { tabular: true, ..TextStyle::BODY.sized(size).weighted(700.0) };
        let ink = if hot { theme.text_strong } else { theme.text_hi };
        // Integrated, large.
        let mut y = DISPLAY_TOP + 24.0;
        cx.text.draw(cx.scene, "INTEGRATED", LOUD_X0, y, label, theme.text_lo, Align::Left);
        cx.text.draw(cx.scene, "LUFS", LOUD_X1, y, label, theme.text_lo, Align::Right);
        y += 14.0;
        cx.text.draw(cx.scene, &fmt_db(i), LOUD_X1, y, number(30.0), ink, Align::Right);
        y += 58.0;
        // Momentary and short-term: a value and a bar (−40…0 LUFS).
        for (name, v) in [("MOMENTARY", m), ("SHORT-TERM", s)] {
            cx.text.draw(cx.scene, name, LOUD_X0, y, label, theme.text_lo, Align::Left);
            cx.text.draw(cx.scene, &fmt_db(v), LOUD_X1, y - 3.0, number(15.0), ink, Align::Right);
            let bar = Rect::new(LOUD_X0, y + 18.0, LOUD_X1, y + 24.0);
            panel(cx.scene, RoundedRect::from_rect(bar, 3.0), theme.meter_well, theme.well_border);
            if v > SILENCE_LUFS + 1.0 {
                let t = ((f64::from(v) + 40.0) / 40.0).clamp(0.0, 1.0);
                let lit = Rect::new(bar.x0 + 1.0, bar.y0 + 1.0, bar.x0 + 1.0 + (bar.width() - 2.0) * t, bar.y1 - 1.0);
                let fill = Gradient::new_linear((bar.x0, 0.0), (bar.x1, 0.0)).with_stops([theme.curve.multiply_alpha(0.35), theme.curve]);
                cx.scene.fill(Fill::NonZero, Affine::IDENTITY, &fill, None, &RoundedRect::from_rect(lit, 2.0));
            }
            y += 50.0;
        }
        y += 10.0;
        // Range, the highest true peak, and peak to loudness.
        let plr = if tp > -119.0 && i > SILENCE_LUFS + 1.0 { tp - i } else { SILENCE_LUFS };
        let ceiling = Self::plain(cx.host, CEILING.id);
        for (name, unit, v, color) in [
            ("RANGE", "LU", if lra > 0.0 || i > SILENCE_LUFS + 1.0 { format!("{lra:.1}") } else { "—".to_owned() }, ink),
            ("TRUE PEAK", "dBTP", fmt_db(tp), if tp > ceiling + 0.05 { theme.danger } else { ink }),
            ("PLR", "dB", if plr > SILENCE_LUFS { format!("{plr:.1}") } else { "—".to_owned() }, ink),
        ] {
            cx.text.draw(cx.scene, name, LOUD_X0, y, label, theme.text_lo, Align::Left);
            cx.text.draw(cx.scene, unit, LOUD_X1, y + 18.0, label, theme.text_lo, Align::Right);
            cx.text.draw(cx.scene, &v, LOUD_X1 - 34.0, y + 13.0, number(15.0), color, Align::Right);
            y += 46.0;
        }
        if hot {
            let r = RoundedRect::from_rect(self.loudness_rect(), 8.0);
            cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, theme.accent.multiply_alpha(0.5), None, &r);
        }
    }

    fn draw_footer(&mut self, cx: &mut DrawCx<'_>) {
        let theme = cx.theme;
        self.footer.draw_frame(cx);
        let r = Rect::new(16.0, FOOTER_TOP + 9.0, 126.0, FOOTER_TOP + 47.0);
        let hot = self.pointer.is_some_and(|p| r.contains(p));
        let text = OVERSAMPLING.choices[(Self::plain(cx.host, OVERSAMPLING.id).round().max(0.0) as usize).min(4)];
        draw_dropdown_button(cx, r, text, self.os_menu.is_some(), hot, |scene, at| draw_glyph(scene, OVERSAMPLE_GLYPH, at + (8.0, 6.0), 22.0, theme.text_mid, 1.6));
        self.os_hit = r;
        let sample_rate = f64::from(self.shared.sample_rate.load()).max(1.0);
        self.footer.draw_latency(cx, 142.0, f64::from(self.shared.latency.load()) / sample_rate * 1000.0);
        self.footer.draw_controls(cx);
        if let Some(list) = &self.os_menu {
            list.draw(cx, theme.accent, |scene, i, at, ink| {
                // Each factor's icon: as many waves as it oversamples.
                let n = FACTORS[i] as f64;
                let mut p = BezPath::new();
                for k in 0..=48 {
                    let x = at.x + 1.0 + 20.0 * k as f64 / 48.0;
                    let y = at.y + 12.0 - 5.0 * (std::f64::consts::TAU * n * k as f64 / 48.0 / 2.0).sin();
                    if k == 0 {
                        p.move_to((x, y));
                    } else {
                        p.line_to((x, y));
                    }
                }
                scene.stroke(&Stroke::new(1.4), Affine::IDENTITY, ink, None, &p);
            });
        }
    }

    fn draw_states(&self, cx: &mut DrawCx<'_>) {
        let theme = cx.theme;
        if Self::on(cx.host, DELTA.id) {
            let r = RoundedRect::from_rect(Rect::new(HIST_W - 56.0, DISPLAY_TOP + 10.0, HIST_W - 20.0, DISPLAY_TOP + 46.0), 10.0);
            cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.accent.multiply_alpha(0.18), None, &r);
            glow_stroke(cx, &r, 1.5, theme.accent, theme.accent, 9.0);
            mite_ui::widgets::footer::draw_toggle_glyph(cx.scene, ToggleGlyph::Delta, r.center(), theme.accent);
        }
    }

    // ---- help -------------------------------------------------------------------------------

    fn help_text(&self, host: &dyn ParamHost) -> Help {
        let ceiling = Self::plain(host, CEILING.id);
        let gain = Self::plain(host, GAIN.id);
        match self.drag {
            Some(Drag::Ceiling { .. }) => return Help::default().text("Ceiling ").slot(format!("{} dB", fmt_db(ceiling)), "−30.0 dB").text(" — nothing comes out above this line. Shift is fine."),
            Some(Drag::Gain { .. }) => return Help::default().text("Gain ").slot(format!("+{gain:.1} dB"), "+30.0 dB").text(" — the music follows the pointer; what rises over the ceiling is limited. Shift is fine."),
            None => {}
        }
        if let Some(list) = &self.os_menu {
            let fs = f64::from(self.shared.sample_rate.load()).max(1.0);
            if let Some(i) = self.pointer.and_then(|p| list.item_rects().iter().position(|r| r.contains(p))) {
                let mut s = settings(host);
                s.oversampling = FACTORS[i];
                let ms = crate::dsp::latency_for(&s, fs as f32) as f64 / fs * 1000.0;
                return Help::default().text(format!("{} — ", OVERSAMPLING.choices[i])).text(if FACTORS[i] == 1 {
                    format!("the limiter runs at the session's rate: the least CPU; {ms:.2} ms of latency in all")
                } else {
                    format!("the limiter runs {} times faster, so its fast gain changes fold back far less; {ms:.2} ms of latency in all", FACTORS[i])
                });
            }
        }
        if self.range.menu_open() || self.pointer.is_some_and(|p| Rect::new(8.0, DISPLAY_TOP + 4.0, 54.0, DISPLAY_TOP + 22.0).contains(p)) {
            return Help::default().text("Level range — how many dB under full scale the history shows (Auto fits the music)");
        }
        match self.hot {
            Some(Hot::Ceiling) => return Help::default().text("Ceiling ").slot(format!("{} dB", fmt_db(ceiling)), "−30.0 dB").text(" — drag it; wheel for fine steps; double-click: −1 dB, the streaming platforms' limit."),
            Some(Hot::History) => {
                if let Some(p) = self.pointer {
                    let (range, head) = (level_range() as f32, headroom() as f32);
                    let level = head - ((p.y - DISPLAY_TOP - 24.0) / LEVEL_SPAN_PX) as f32 * (range + head);
                    let ago = (HIST_W - p.x) / HIST_W * HIST_SECONDS;
                    return Help::default()
                        .slot(format!("{} dB", fmt_db(level)), "−48.0 dB")
                        .text(" · ")
                        .slot(format!("{ago:.1} s ago"), "6.0 s ago")
                        .text(" · gain ")
                        .slot(format!("+{gain:.1} dB"), "+30.0 dB")
                        .text(" — drag the waveform up to push it into the ceiling; double-click: gain 0");
                }
            }
            Some(Hot::Style(i)) => return Help::default().text(format!("{} — {}", STYLE_NAMES[i], style_words(i))),
            Some(Hot::Loudness) => return Help::default().text("Loudness of the output, as EBU R128 measures it: momentary (400 ms), short-term (3 s), integrated since the last reset, its range, the highest true peak, and peak to loudness. Click to reset."),
            Some(Hot::Oversampling) => return Help::default().text("Oversampling — how many times faster the limiter runs, so its fast gain changes do not fold back into the audible band."),
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
        if gain == 0.0 {
            Help::default().text("Drag the waveform up (or turn GAIN) to push the music into the ceiling. The silver line is the ceiling: drag its grip.")
        } else {
            Help::default().text("Dim: the music going in. Lit: what comes out. From the top: the gain the limiter takes away.")
        }
    }

    // ---- events -----------------------------------------------------------------------------

    /// Makes the strip's dials (SLOW only on Custom).
    fn sync_knobs(&mut self, host: &dyn ParamHost) {
        let custom = Self::style(host) == crate::params::STYLE_NAMES.len() - 1;
        if self.knobs_custom == Some(custom) || self.knobs.iter().any(Knob::is_dragging) {
            return;
        }
        let mut dials: Vec<(ParamSpec, &'static str, KnobKind, Option<mite_core::DialCurve>)> = vec![
            (LOOKAHEAD, "LOOKAHEAD", KnobKind::Unipolar, Some(LOOKAHEAD_DIAL)),
            (ATTACK, "ATTACK", KnobKind::Unipolar, Some(ATTACK_DIAL)),
            (RELEASE, "RELEASE", KnobKind::Unipolar, Some(RELEASE_DIAL)),
            (LINK_T, "LINK T", KnobKind::Unipolar, None),
            (LINK_R, "LINK R", KnobKind::Unipolar, None),
        ];
        if custom {
            dials.push((SLOW, "SLOW", KnobKind::Unipolar, None));
        }
        self.knobs = dials
            .into_iter()
            .enumerate()
            .map(|(i, (spec, label, kind, curve))| {
                let knob = Knob::new(spec, label, kind, Point::new(240.0 + i as f64 * 112.0, DIAL_Y), DIAL_D);
                match curve {
                    Some(c) => knob.with_curve(c),
                    None => knob,
                }
            })
            .collect();
        self.knobs_custom = Some(custom);
    }

    fn busy(&self) -> bool {
        self.drag.is_some() || self.knobs.iter().any(Knob::is_dragging) || self.footer.busy()
    }

    fn hot_at(&self, host: &dyn ParamHost, p: Point) -> Option<Hot> {
        if Self::grip_rect(host).inflate(4.0, 4.0).contains(p) {
            return Some(Hot::Ceiling);
        }
        if let Some(i) = (0..STYLE_GLYPHS.len()).find(|&i| Self::style_rect(i).contains(p)) {
            return Some(Hot::Style(i));
        }
        if self.os_hit.contains(p) {
            return Some(Hot::Oversampling);
        }
        if self.loudness_rect().contains(p) {
            return Some(Hot::Loudness);
        }
        if p.x < HIST_W && p.y > DISPLAY_TOP && p.y < HIST_BOTTOM {
            return Some(Hot::History);
        }
        None
    }

    fn key_event(&mut self, key: Key, mods: Modifiers, host: &dyn ParamHost) -> bool {
        if let Some(used) = self.session.key(key, mods, host) {
            return used;
        }
        match key {
            Key::Up | Key::Down => {
                let step = if mods.fine() { 0.1 } else { 1.0 } * if key == Key::Up { 1.0 } else { -1.0 };
                Self::set(host, GAIN.id, (Self::plain(host, GAIN.id) + step).clamp(GAIN.min, GAIN.max));
                true
            }
            _ => false,
        }
    }

    fn press(&mut self, host: &dyn ParamHost, pos: Point) -> bool {
        match self.hot_at(host, pos) {
            Some(Hot::Ceiling) => {
                host.begin_gesture(CEILING.id);
                self.drag = Some(Drag::Ceiling { start: pos, ceiling: Self::plain(host, CEILING.id) });
                true
            }
            Some(Hot::History) => {
                host.begin_gesture(GAIN.id);
                self.drag = Some(Drag::Gain { start: pos, gain: Self::plain(host, GAIN.id) });
                true
            }
            Some(Hot::Style(i)) => {
                Self::set(host, STYLE.id, i as f32);
                true
            }
            Some(Hot::Loudness) => {
                self.shared.loudness.reset.store(true, Ordering::Relaxed);
                true
            }
            Some(Hot::Oversampling) => {
                self.os_menu = Some(Self::os_list(host));
                true
            }
            None => false,
        }
    }

    fn handle_event(&mut self, event: &UiEvent, host: &dyn ParamHost) -> bool {
        if let UiEvent::KeyDown { key, mods } = *event {
            return self.key_event(key, mods, host);
        }
        if let Some(list) = &mut self.os_menu {
            if let UiEvent::PointerMove { pos, .. } = *event {
                self.pointer = Some(pos);
            }
            match list.event(event) {
                DropdownEvent::Open => {}
                DropdownEvent::Picked(i) => {
                    self.os_menu = None;
                    Self::set(host, OVERSAMPLING.id, i as f32);
                }
                DropdownEvent::Closed => self.os_menu = None,
            }
            return true;
        }
        if let Some(answer) = self.session.event(event, host) {
            return answer;
        }
        if self.range.event(event, self.range_shown) {
            return true;
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
                    Some(Drag::Ceiling { start, ceiling }) => {
                        let db = ceiling - ((pos.y - start.y) / px_per_db()) as f32 * k;
                        Self::set_raw(host, CEILING.id, CEILING.snap(db.clamp(CEILING.min, CEILING.max)));
                        return true;
                    }
                    Some(Drag::Gain { start, gain }) => {
                        let db = gain + ((start.y - pos.y) / px_per_db()) as f32 * k;
                        Self::set_raw(host, GAIN.id, GAIN.snap(db.clamp(GAIN.min, GAIN.max)));
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
            UiEvent::PointerUp { button: MouseButton::Left, .. } => match self.drag.take() {
                Some(Drag::Ceiling { .. }) => {
                    host.end_gesture(CEILING.id);
                    true
                }
                Some(Drag::Gain { .. }) => {
                    host.end_gesture(GAIN.id);
                    true
                }
                None => false,
            },
            UiEvent::DoubleClick { pos, .. } => match self.hot_at(host, pos) {
                Some(Hot::Ceiling) => {
                    self.end_drag(host);
                    Self::set(host, CEILING.id, CEILING.default);
                    true
                }
                Some(Hot::History) => {
                    self.end_drag(host);
                    Self::set(host, GAIN.id, GAIN.default);
                    true
                }
                _ => false,
            },
            UiEvent::Wheel { pos, lines, mods } => match self.hot_at(host, pos) {
                Some(Hot::Ceiling) => {
                    Self::set(host, CEILING.id, CEILING.wheel(Self::plain(host, CEILING.id), lines, mods.fine()));
                    true
                }
                Some(Hot::History) => {
                    Self::set(host, GAIN.id, GAIN.wheel(Self::plain(host, GAIN.id), lines, mods.fine()));
                    true
                }
                _ => false,
            },
            _ => false,
        }
    }

    fn end_drag(&mut self, host: &dyn ParamHost) {
        match self.drag.take() {
            Some(Drag::Ceiling { .. }) => host.end_gesture(CEILING.id),
            Some(Drag::Gain { .. }) => host.end_gesture(GAIN.id),
            None => {}
        }
    }

    fn after_session(&mut self, _host: &dyn ParamHost) {
        if self.session.take_replaced() {
            self.knobs_custom = None;
        }
    }
}

fn style_words(i: usize) -> &'static str {
    match i {
        0 => "one smooth release, lightly program-dependent: the least colour",
        1 => "lets go fast after a hit and holds on sustained parts: keeps the punch",
        2 => "fast release that slows as the limiting builds: loud and thick",
        3 => "long attack and release: glue rather than catching",
        4 => "the slowest: no distortion at any cost, for broadcast",
        _ => "the dials as they are, and SLOW: how much the slow envelope holds the gain down",
    }
}

fn knob_words(id: &str) -> &'static str {
    match id {
        "lookahead" => " — the gain starts down this long before a peak, in a smooth S; it is the latency (0: a clipper)",
        "attack" => " — how fast sustained limiting builds the slow envelope that holds the gain down",
        "release" => " — how fast the gain comes back after a peak (the style shapes it)",
        "link_t" => " — how far the channels duck together on a peak (100 %: the image never moves)",
        "link_r" => " — how far they come back together",
        "slow" => " — how much the slow envelope holds the gain down (Custom)",
        _ => "",
    }
}

impl View for LimitView {
    fn logical_size(&self) -> (f64, f64) {
        (WIDTH, HEIGHT)
    }

    fn draw(&mut self, cx: &mut DrawCx<'_>) {
        set_level_range(self.range_shown);
        set_headroom(self.headroom);
        self.sync_knobs(cx.host);
        let theme = cx.theme;
        self.draw_header(cx);
        cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.display, None, &Rect::new(0.0, DISPLAY_TOP, WIDTH, HELP_TOP));
        self.draw_history(cx);
        self.draw_strip(cx);
        self.draw_meters(cx);
        self.draw_loudness(cx);
        self.draw_states(cx);
        self.help_text(cx.host).draw(cx, Rect::new(0.0, HELP_TOP, WIDTH, FOOTER_TOP));
        self.draw_footer(cx);
        self.range.draw_menu(cx);
        self.session.draw_menu(cx);
    }

    fn event(&mut self, event: &UiEvent, host: &dyn ParamHost) -> bool {
        set_level_range(self.range_shown);
        set_headroom(self.headroom);
        self.sync_knobs(host);
        self.session.begin(host);
        let recording = RecordingHost::new(host);
        let changed = self.handle_event(event, &recording);
        let busy = self.busy();
        self.session.end(host, recording.wrote(), busy);
        self.after_session(host);
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
        // The level range: Auto fits the music (its quiet tenth, silence aside), never so narrow
        // that the ceiling falls off it; a picked range is kept. It glides.
        let ceiling = -Self::plain(host, CEILING.id) + 3.0;
        let mut levels: Vec<f32> = self.history.iter().rev().take(1200).map(|r| r[record::IN].max(r[record::IN + 1])).filter(|&l| l > -80.0).collect();
        let content = if levels.len() > 50 {
            levels.sort_by(f32::total_cmp);
            -levels[levels.len() / 10] + 3.0
        } else {
            24.0
        };
        let auto = content.max(ceiling).clamp(12.0, 48.0);
        let fit = LEVEL_RANGES.iter().copied().find(|&v| v >= ceiling).unwrap_or(LEVEL_RANGES[LEVEL_RANGES.len() - 1]);
        self.range_target = self.range.target(auto).max(fit);
        let moved = ease_range(&mut self.range_shown, self.range_target, dt);
        set_level_range(self.range_shown);
        // Headroom over 0 dB while the input (after the gain) goes over (eased, 120 ms).
        self.headroom_target = history::headroom_db(&self.history, (HIST_SECONDS * 1000.0 / f64::from(HISTORY_SLICE_MS)) as usize, &[record::IN, record::IN + 1]);
        let head = self.headroom_target;
        let before = self.headroom;
        self.headroom += (head - self.headroom) * (1.0 - (-dt / 0.12).exp());
        if (self.headroom - head).abs() < 0.01 {
            self.headroom = head;
        }
        set_headroom(self.headroom);
        let moved = moved || self.headroom != before;
        if let Some(r) = self.history.back().copied() {
            let tp = Self::on(host, TRUE_PEAK.id);
            let out = if tp { record::TP } else { record::OUT };
            let targets = [r[record::IN], r[record::IN + 1], r[record::GR], r[record::GR + 1], r[out], r[out + 1]];
            for (i, (m, t)) in self.meters.iter_mut().zip(targets).enumerate() {
                let fall = METER_RELEASE_DB_S * dt;
                *m = if i == 2 || i == 3 { if t < *m { t } else { (*m + fall).min(t) } } else if t > *m { t } else { (*m - fall).max(t) };
            }
        }
        let flowing = self.last_fresh.elapsed().as_secs_f32() < 0.25;
        fresh || flowing || moved || busy || self.meters.iter().enumerate().any(|(i, &m)| if i == 2 || i == 3 { m < -0.01 } else { m > SILENT + 1.0 })
    }
}
