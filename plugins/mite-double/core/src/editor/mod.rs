//! The Mite Double editor (docs/design/double-brief.md §2), in 1280×760 design units.
//!
//! On top the stereo field: the source and each double as a puck, across where it sits (left to
//! right), up and down how loud; drag one to place it, a ghost puck turns a double on. A ring
//! breathes with each puck's level, and the correlation strip under it says how the pair sums to
//! mono. Under it, left, what the doubles are doing, note by note: their timing (each note's lag,
//! the spread shaded) and their pitch (each note's cents). Right, the dials: the TAKE (how the
//! double is played: DELAY, TIMING, PITCH, VARIATION), CHORUS, the DOUBLES (how many, whether new
//! readings follow the notes) and TONE.

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Instant;

use mite_core::ParamSpec;
use mite_dsp::doubler::MAX_VOICES;
use mite_ui::kurbo::{Affine, BezPath, Circle, Line, Point, Rect, RoundedRect, Stroke};
use mite_ui::paint::{glow_stroke, panel};
use mite_ui::peniko::Fill;
use mite_ui::session::{Session, SessionConfig};
use mite_ui::widgets::brand::{draw_mark, draw_title};
use mite_ui::widgets::history::{self, HistoryLane};
use mite_ui::widgets::{Dropdown, DropdownEvent, Footer, FooterDial, FooterToggle, Help, Knob, KnobKind, ToggleGlyph};
use mite_ui::{write_if_changed, Align, DrawCx, MouseButton, ParamHost, RecordingHost, TextStyle, UiEvent, View};

use crate::dsp::{record, HISTORY_RECORDS, HISTORY_SLICE_MS};
use crate::params::*;
use crate::presets::{preset_ids, FACTORY, PLUGIN};
use crate::DoubleShared;

/// Mite Double wears the Double accent (tokens.json `color.accent.double`, rose).
pub const ACCENT: &str = "double";

const VIEW_W: f64 = 1280.0;
const VIEW_H: f64 = 760.0;
const HEADER: f64 = 56.0;
const DISPLAY_TOP: f64 = HEADER;
const HELP_TOP: f64 = DISPLAY_TOP + 620.0;
const FOOTER_TOP: f64 = HELP_TOP + 28.0;

/// The stereo field: pan across `FIELD_X`, level up `FIELD_Y` (top: +6 dB, bottom: −36 dB).
const FIELD: Rect = Rect::new(0.0, DISPLAY_TOP, 1180.0, DISPLAY_TOP + 300.0);
const FIELD_X: (f64, f64) = (80.0, 1100.0);
const FIELD_Y: (f64, f64) = (FIELD.y0 + 34.0, FIELD.y1 - 64.0);
const TOP_DB: f64 = 6.0;
const BOTTOM_DB: f64 = -36.0;
const PUCK_R: f64 = 17.0;
/// The correlation strip under the field.
const CORR: Rect = Rect::new(440.0, FIELD.y1 - 30.0, 740.0, FIELD.y1 - 20.0);

/// What the doubles do, note by note: timing above, pitch below.
const LANES: Rect = Rect::new(0.0, FIELD.y1 + 8.0, 740.0, HELP_TOP - 4.0);
const LANE_SPLIT: f64 = 0.5 * (LANES.y0 + LANES.y1);
const LANE_SECONDS: f64 = 6.0;
const LANE_X0: f64 = 40.0;
const TIMING_MS: (f64, f64) = (0.0, 90.0);

const DIAL_D: f64 = 46.0;
const DIAL_DY: f64 = 70.0;
const GROUP_COLS: [(f64, f64); 2] = [(752.0, 1062.0), (1070.0, 1172.0)];
const GROUP_ROWS: [(f64, f64); 2] = [(LANES.y0, LANES.y0 + 152.0), (LANES.y1 - 152.0, LANES.y1)];
const PILL_H: f64 = 26.0;

const WELLS: [f64; 2] = [1206.0, 1238.0];
const METER_TOP: f64 = DISPLAY_TOP + 20.0;
const METER_BOTTOM: f64 = DISPLAY_TOP + 592.0;
const METER_RANGE: f64 = 60.0;
const SILENT: f32 = -150.0;

const VOICE_MENU: [&str; 2] = ["Reset place", "Remove"];

fn pan_x(pan: f64) -> f64 {
    FIELD_X.0 + (pan.clamp(-1.0, 1.0) + 1.0) * 0.5 * (FIELD_X.1 - FIELD_X.0)
}

fn x_pan(x: f64) -> f64 {
    ((x - FIELD_X.0) / (FIELD_X.1 - FIELD_X.0) * 2.0 - 1.0).clamp(-1.0, 1.0)
}

fn db_y(db: f64) -> f64 {
    FIELD_Y.0 + (TOP_DB - db.clamp(BOTTOM_DB, TOP_DB)) / (TOP_DB - BOTTOM_DB) * (FIELD_Y.1 - FIELD_Y.0)
}

fn y_db(y: f64) -> f64 {
    TOP_DB - (y - FIELD_Y.0) / (FIELD_Y.1 - FIELD_Y.0) * (TOP_DB - BOTTOM_DB)
}

/// A stand-in host holding every Mite Double parameter at its default (tests, preview, tools).
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
        scale: mite_core::ParamScale::Linear,
        smoothing_ms: 0.0,
        decimals: 0,
        wheel_step: 1.0,
        wheel_step_fine: 1.0,
        display: mite_core::ParamDisplay::Number,
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

/// The doubler's settings as a host's parameters set them.
pub fn settings(host: &dyn ParamHost) -> mite_dsp::doubler::DoublerSettings {
    doubler_settings(|id| host.plain(id, host.normalized(id)))
}

pub fn output(host: &dyn ParamHost) -> crate::dsp::Output {
    let p = |id: &str| host.plain(id, host.normalized(id));
    crate::dsp::Output { gain_db: p(OUT_GAIN.id), bypass: p(BYPASS.id) >= 0.5 }
}

/// A puck in the field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Puck {
    Source,
    /// A double (from 0).
    Voice(usize),
}

/// A pill: how many doubles, whether new readings follow the notes, mono bass.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pill {
    Count(usize),
    Follow,
    MonoBass,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Hot {
    Puck(Puck),
    Pill(Pill),
}

struct Group {
    name: &'static str,
    words: &'static str,
    rect: Rect,
}

pub struct DoubleView {
    shared: Arc<DoubleShared>,
    session: Session,
    footer: Footer,
    knobs: Vec<Knob>,
    groups: Vec<Group>,
    history: VecDeque<[f32; record::WIDTH]>,
    seen: usize,
    last_fresh: Instant,
    last_tick: Instant,
    pointer: Option<Point>,
    hot: Option<Hot>,
    drag: Option<(Puck, Point, Point)>,
    menu: Option<(usize, Dropdown)>,
    meters: [f32; 3],
    /// Each puck's level as drawn (dBFS) and the correlation as drawn.
    rings: [f32; 4],
    corr: f32,
}

impl DoubleView {
    pub fn new(shared: Arc<DoubleShared>) -> Self {
        let footer = Footer::new(
            FOOTER_TOP,
            (VIEW_W, VIEW_H),
            vec![
                FooterToggle { id: AUTO_GAIN.id, glyph: ToggleGlyph::Auto, words: "Auto gain — the doubles together as loud as the loudest one (more takes add width, not level), and the whole as loud as the input (Bypass compares fairly)" },
                FooterToggle { id: BYPASS.id, glyph: ToggleGlyph::Bypass, words: "Bypass — the input alone, as it came" },
            ],
            vec![FooterDial { spec: OUT_GAIN, kind: KnobKind::Bipolar, name: "OUTPUT", words: "Output gain" }],
        );
        type Dial = (ParamSpec, &'static str, KnobKind);
        type Layout = (&'static str, &'static str, usize, usize, Vec<Dial>, f64);
        let layout: [Layout; 3] = [
            (
                "TAKE",
                "how the double is played",
                0,
                0,
                vec![(DELAY, "DELAY", KnobKind::Unipolar), (TIMING, "TIMING", KnobKind::Unipolar), (PITCH, "PITCH", KnobKind::Unipolar), (VARIATION, "VARIATION", KnobKind::Unipolar)],
                0.0,
            ),
            ("CHORUS", "a slow drift on top", 0, 1, vec![(CHORUS, "DEPTH", KnobKind::Unipolar), (CHORUS_RATE, "RATE", KnobKind::Unipolar)], 0.0),
            ("TONE", "the doubles brighter or darker, and where their lows become the source's", 1, 1, vec![(TONE, "TONE", KnobKind::Bipolar), (BASS_FREQ, "MONO", KnobKind::Unipolar)], 0.0),
        ];
        let mut knobs = Vec::new();
        let mut groups = Vec::new();
        for (name, words, row, col, dials, from) in layout {
            let rect = Rect::new(GROUP_COLS[col].0, GROUP_ROWS[row].0, GROUP_COLS[col].1, GROUP_ROWS[row].1);
            let n = dials.len() as f64;
            let x0 = rect.x0 + rect.width() * from;
            for (i, (spec, label, kind)) in dials.into_iter().enumerate() {
                let at = Point::new(x0 + (rect.x1 - x0) * (i as f64 + 0.5) / n, rect.y0 + DIAL_DY);
                knobs.push(Knob::new(spec, label, kind, at, DIAL_D));
            }
            groups.push(Group { name, words, rect });
        }
        groups.push(Group { name: "DOUBLES", words: "how many takes, and when they change", rect: Rect::new(GROUP_COLS[0].0, GROUP_ROWS[1].0, GROUP_COLS[0].1, GROUP_ROWS[1].1) });
        Self {
            session: Session::new(SessionConfig { plugin: PLUGIN, factory: FACTORY, sound_ids: preset_ids(), snapshot_ids: preset_ids() }, shared.snapshots.clone(), shared.preset.clone()),
            footer,
            knobs,
            groups,
            history: VecDeque::with_capacity(HISTORY_RECORDS),
            seen: 0,
            last_fresh: Instant::now(),
            last_tick: Instant::now(),
            pointer: None,
            hot: None,
            drag: None,
            menu: None,
            meters: [SILENT; 3],
            rings: [SILENT; 4],
            corr: 1.0,
            shared,
        }
    }

    pub fn with_preset_dir(mut self, dir: std::path::PathBuf) -> Self {
        self.session = self.session.with_preset_dir(dir);
        self
    }

    fn plain(host: &dyn ParamHost, id: &str) -> f32 {
        host.plain(id, host.normalized(id))
    }

    fn set(host: &dyn ParamHost, id: &str, v: f32) {
        write_if_changed(host, id, host.normalize(id, v));
    }

    fn gesture(host: &dyn ParamHost, id: &str, v: f32) {
        host.begin_gesture(id);
        Self::set(host, id, v);
        host.end_gesture(id);
    }

    /// How many doubles play.
    pub fn voices(host: &dyn ParamHost) -> usize {
        (Self::plain(host, VOICES.id).round().max(0.0) as usize + 1).min(MAX_VOICES)
    }

    fn ids(puck: Puck) -> (String, String) {
        match puck {
            Puck::Source => (SOURCE_PAN.id.to_owned(), SOURCE_LEVEL.id.to_owned()),
            Puck::Voice(v) => (String::from(voice_id(v, "pan")), String::from(voice_id(v, "level"))),
        }
    }

    /// Where a puck sits.
    pub fn puck(&self, host: &dyn ParamHost, puck: Puck) -> Point {
        let (pan, level) = Self::ids(puck);
        Point::new(pan_x(f64::from(Self::plain(host, &pan)) / 100.0), db_y(f64::from(Self::plain(host, &level))))
    }

    pub fn pill(&self, pill: Pill) -> Rect {
        let g = Rect::new(GROUP_COLS[0].0, GROUP_ROWS[1].0, GROUP_COLS[0].1, GROUP_ROWS[1].1);
        match pill {
            Pill::Count(n) => {
                let x = g.x0 + 16.0 + 52.0 * (n - 1) as f64;
                Rect::new(x, g.y0 + 46.0, x + 44.0, g.y0 + 46.0 + PILL_H)
            }
            Pill::Follow => Rect::new(g.x0 + 16.0, g.y0 + 96.0, g.x1 - 16.0, g.y0 + 96.0 + PILL_H),
            Pill::MonoBass => Rect::new(CORR.x1 + 44.0, CORR.center().y - PILL_H / 2.0, CORR.x1 + 214.0, CORR.center().y + PILL_H / 2.0),
        }
    }

    pub fn menu_rows(&self) -> Vec<Rect> {
        self.menu.as_ref().map(|(_, m)| m.item_rects()).unwrap_or_default()
    }

    pub fn dial_groups(&self) -> Vec<(&'static str, Rect)> {
        self.groups.iter().map(|g| (g.name, g.rect)).collect()
    }

    pub fn dial(&self, id: &str) -> Option<Point> {
        self.knobs.iter().find(|k| k.id == id).map(|k| k.center)
    }

    pub fn pan_x(&self, pan: f64) -> f64 {
        pan_x(pan)
    }

    pub fn db_y(&self, db: f64) -> f64 {
        db_y(db)
    }

    // ---- drawing ----------------------------------------------------------------------------

    fn draw_header(&mut self, cx: &mut DrawCx<'_>) {
        let theme = cx.theme;
        cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.window, None, &Rect::new(0.0, 0.0, VIEW_W, HEADER));
        cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, theme.line, None, &Line::new((0.0, HEADER - 0.5), (VIEW_W, HEADER - 0.5)));
        draw_mark(cx, 16.0, 15.0, 26.0);
        draw_title(cx, 52.0, 18.0, "Double");
        self.session.draw(cx);
    }

    fn draw_field(&self, cx: &mut DrawCx<'_>) {
        let theme = cx.theme;
        let host = cx.host;
        cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.well, None, &FIELD);
        let label = TextStyle { tabular: true, ..TextStyle::BODY.sized(10.0).weighted(600.0) };
        for (pan, t) in [(-1.0, "L"), (-0.5, "50"), (0.0, "C"), (0.5, "50"), (1.0, "R")] {
            let x = pan_x(pan);
            let major = pan == 0.0 || pan.abs() == 1.0;
            cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, if major { theme.grid_major } else { theme.grid_minor }, None, &Line::new((x, FIELD_Y.0 - 10.0), (x, FIELD_Y.1 + 10.0)));
            cx.text.draw(cx.scene, t, x, FIELD.y0 + 8.0, TextStyle::LABEL.sized(10.0), if major { theme.text_mid } else { theme.text_lo }, Align::Center);
        }
        for (db, t) in [(0.0, "0 dB"), (-12.0, "−12"), (-24.0, "−24")] {
            let y = db_y(db);
            cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, theme.grid_minor, None, &Line::new((FIELD_X.0 - 20.0, y), (FIELD_X.1 + 20.0, y)));
            cx.text.draw(cx.scene, t, 10.0, y - 6.0, label, theme.text_lo, Align::Left);
        }
        // The pucks: the ghosts of doubles not playing (a click brings them), then the doubles,
        // then the source.
        let voices = Self::voices(host);
        let accent = theme.accent;
        for v in voices..MAX_VOICES {
            let p = self.puck(host, Puck::Voice(v));
            let hot = self.hot == Some(Hot::Puck(Puck::Voice(v)));
            cx.scene.stroke(&Stroke::new(1.2).with_dashes(0.0, [4.0, 4.0]), Affine::IDENTITY, accent.multiply_alpha(if hot { 0.8 } else { 0.35 }), None, &Circle::new(p, PUCK_R));
            cx.text.draw(cx.scene, &format!("{}", v + 1), p.x, p.y - 7.0, TextStyle { tabular: true, ..TextStyle::BODY.sized(12.0).weighted(700.0) }, accent.multiply_alpha(if hot { 0.8 } else { 0.35 }), Align::Center);
        }
        let mut order: Vec<Puck> = (0..voices).map(Puck::Voice).collect();
        order.push(Puck::Source);
        for puck in order {
            let p = self.puck(host, puck);
            let hot = self.hot == Some(Hot::Puck(puck)) || self.drag.is_some_and(|(d, _, _)| d == puck);
            let (ink, ring) = match puck {
                Puck::Source => (theme.text_hi, self.rings[0]),
                Puck::Voice(v) => (accent, self.rings[1 + v]),
            };
            // A ring that breathes with the level it plays at.
            let k = ((f64::from(ring) + 48.0) / 48.0).clamp(0.0, 1.0);
            if k > 0.0 {
                let c = Circle::new(p, PUCK_R + 4.0 + 18.0 * k);
                cx.scene.fill(Fill::NonZero, Affine::IDENTITY, ink.multiply_alpha(0.08 + 0.1 * k as f32), None, &c);
            }
            let c = Circle::new(p, PUCK_R);
            cx.scene.fill(Fill::NonZero, Affine::IDENTITY, if hot { ink } else { theme.display }, None, &c);
            glow_stroke(cx, &c, 1.8, ink, ink, if hot { 5.0 } else { 2.5 });
            let text = match puck {
                Puck::Source => "S".to_owned(),
                Puck::Voice(v) => format!("{}", v + 1),
            };
            cx.text.draw(cx.scene, &text, p.x, p.y - 7.5, TextStyle { tabular: true, ..TextStyle::BODY.sized(13.0).weighted(700.0) }, if hot { theme.display } else { ink }, Align::Center);
        }
        // How the output sums to mono: −1 (cancels) … +1 (mono).
        panel(cx.scene, RoundedRect::from_rect(CORR.inflate(2.0, 2.0), 4.0), theme.meter_well, theme.well_border);
        let x = CORR.x0 + (f64::from(self.corr).clamp(-1.0, 1.0) + 1.0) * 0.5 * CORR.width();
        let mid = CORR.center().x;
        let fill = Rect::new(mid.min(x), CORR.y0, mid.max(x), CORR.y1);
        let ink = if self.corr < 0.0 { theme.warn } else { accent };
        cx.scene.fill(Fill::NonZero, Affine::IDENTITY, ink.multiply_alpha(0.7), None, &fill);
        cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, theme.text_lo, None, &Line::new((mid, CORR.y0 - 3.0), (mid, CORR.y1 + 3.0)));
        cx.text.draw(cx.scene, "−1", CORR.x0 - 8.0, CORR.y0 - 2.0, label, theme.text_lo, Align::Right);
        cx.text.draw(cx.scene, "+1", CORR.x1 + 8.0, CORR.y0 - 2.0, label, theme.text_lo, Align::Left);
        cx.text.draw(cx.scene, "MONO", CORR.center().x, CORR.y1 + 5.0, TextStyle::LABEL.sized(9.0), theme.text_lo, Align::Center);
    }

    fn since_fresh(&self) -> f64 {
        (self.last_fresh.elapsed().as_secs_f64() * 1000.0 / f64::from(HISTORY_SLICE_MS)).min(4.0)
    }

    fn draw_lanes(&self, cx: &mut DrawCx<'_>) {
        let theme = cx.theme;
        let host = cx.host;
        let accent = theme.accent;
        cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.well, None, &LANES);
        cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, theme.line, None, &Line::new((LANES.x0, LANE_SPLIT), (LANES.x1, LANE_SPLIT)));
        let label = TextStyle { tabular: true, ..TextStyle::BODY.sized(10.0).weighted(600.0) };
        let timing = Rect::new(LANE_X0, LANES.y0 + 22.0, LANES.x1 - 8.0, LANE_SPLIT - 8.0);
        let pitch = Rect::new(LANE_X0, LANE_SPLIT + 22.0, LANES.x1 - 8.0, LANES.y1 - 8.0);
        cx.text.draw(cx.scene, "TIMING", LANES.x0 + 12.0, LANES.y0 + 8.0, TextStyle::LABEL.sized(9.0), theme.text_mid, Align::Left);
        cx.text.draw(cx.scene, "PITCH", LANES.x0 + 12.0, LANE_SPLIT + 8.0, TextStyle::LABEL.sized(9.0), theme.text_mid, Align::Left);
        let ms_y = |ms: f64| timing.y1 - (ms - TIMING_MS.0) / (TIMING_MS.1 - TIMING_MS.0) * timing.height();
        let pitch_reach = f64::from(PITCH.max);
        let ct_y = |c: f64| pitch.center().y - c.clamp(-pitch_reach, pitch_reach) / pitch_reach * 0.5 * pitch.height();
        // The spread the settings allow, shaded; the mean lag and unison as lines.
        let delay = f64::from(Self::plain(host, DELAY.id));
        let spread = 0.5 * f64::from(Self::plain(host, TIMING.id));
        let cents = f64::from(Self::plain(host, PITCH.id));
        cx.scene.fill(Fill::NonZero, Affine::IDENTITY, accent.multiply_alpha(0.07), None, &Rect::new(timing.x0, ms_y((delay + spread).min(TIMING_MS.1)), timing.x1, ms_y((delay - spread).max(TIMING_MS.0))));
        cx.scene.fill(Fill::NonZero, Affine::IDENTITY, accent.multiply_alpha(0.07), None, &Rect::new(pitch.x0, ct_y(cents), pitch.x1, ct_y(-cents)));
        cx.scene.stroke(&Stroke::new(1.0).with_dashes(0.0, [4.0, 3.0]), Affine::IDENTITY, theme.grid_major, None, &Line::new((timing.x0, ms_y(delay)), (timing.x1, ms_y(delay))));
        cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, theme.grid_major, None, &Line::new((pitch.x0, ct_y(0.0)), (pitch.x1, ct_y(0.0))));
        for (ms, t) in [(0.0, "0"), (30.0, "30"), (60.0, "60 ms")] {
            cx.text.draw(cx.scene, t, timing.x0 - 6.0, ms_y(ms) - 6.0, label, theme.text_lo, Align::Right);
        }
        for (c, t) in [(20.0, "+20"), (0.0, "0"), (-20.0, "−20")] {
            cx.text.draw(cx.scene, t, pitch.x0 - 6.0, ct_y(c) - 6.0, label, theme.text_lo, Align::Right);
        }
        let lane = HistoryLane { x0: timing.x0, width: timing.width(), top: timing.y0, bottom: pitch.y1, seconds: LANE_SECONDS, slice_ms: HISTORY_SLICE_MS };
        let cols = lane.columns(&self.history, self.seen, self.since_fresh());
        if cols.len() < 2 {
            return;
        }
        cx.begin_clip(&LANES);
        type Col = history::Column<{ record::WIDTH }>;
        // Each note's start: a tick across both lanes.
        for c in cols.iter().filter(|c| c.high[record::ONSET] > 0.5) {
            for (y0, y1) in [(timing.y0, timing.y1), (pitch.y0, pitch.y1)] {
                cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, theme.spectrum_pre.multiply_alpha(0.35), None, &Line::new((c.x, y0), (c.x, y1)));
            }
        }
        for v in 0..Self::voices(host) {
            let alpha = [1.0, 0.7, 0.5][v];
            let ink = accent.multiply_alpha(alpha);
            let playing = |c: &Col| c.high[record::PEAK + v] > -70.0;
            for (rect, field, y) in [(timing, record::DELAY, &ms_y as &dyn Fn(f64) -> f64), (pitch, record::CENTS, &ct_y)] {
                let _ = rect;
                let mut path = BezPath::new();
                let mut open = false;
                for c in &cols {
                    if !playing(c) {
                        open = false;
                        continue;
                    }
                    let p = (c.x, y(f64::from(c.high[field + v])));
                    if open {
                        path.line_to(p);
                    } else {
                        path.move_to(p);
                        open = true;
                    }
                }
                if v == 0 {
                    glow_stroke(cx, &path, 1.8, ink, ink, 2.0);
                } else {
                    cx.scene.stroke(&Stroke::new(1.5), Affine::IDENTITY, ink, None, &path);
                }
            }
        }
        cx.end_clip();
    }

    fn draw_pill(&self, cx: &mut DrawCx<'_>, pill: Pill, text: &str, on: bool) {
        let theme = cx.theme;
        let r = RoundedRect::from_rect(self.pill(pill), PILL_H / 2.0);
        let hot = self.hot == Some(Hot::Pill(pill));
        let ink = theme.accent;
        cx.scene.fill(Fill::NonZero, Affine::IDENTITY, if on { ink.multiply_alpha(0.18) } else { theme.display }, None, &r);
        if on {
            glow_stroke(cx, &r, 1.3, ink, ink, 3.0);
        } else {
            cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, if hot { theme.text_hi } else { theme.well_border }, None, &r);
        }
        let c = r.rect().center();
        cx.text.draw(cx.scene, text, c.x, c.y - 6.0, TextStyle::LABEL.sized(10.0), if on { ink } else if hot { theme.text_hi } else { theme.text_mid }, Align::Center);
    }

    fn draw_groups(&self, cx: &mut DrawCx<'_>) {
        let theme = cx.theme;
        let host = cx.host;
        for g in &self.groups {
            panel(cx.scene, RoundedRect::from_rect(g.rect, 10.0), theme.well, theme.well_border);
            cx.text.draw(cx.scene, g.name, g.rect.x0 + 12.0, g.rect.y0 + 9.0, TextStyle::LABEL.sized(9.0), theme.text_mid, Align::Left);
        }
        let voices = Self::voices(host);
        for n in 1..=MAX_VOICES {
            self.draw_pill(cx, Pill::Count(n), &n.to_string(), voices == n);
        }
        let follow = Self::plain(host, FOLLOW.id) >= 0.5;
        self.draw_pill(cx, Pill::Follow, if follow { "NEW TAKE EVERY NOTE" } else { "NEW TAKE EVERY FEW SECONDS" }, follow);
        let bass = Self::plain(host, MONO_BASS.id) >= 0.5;
        self.draw_pill(cx, Pill::MonoBass, &format!("MONO BASS {:.0} Hz", Self::plain(host, BASS_FREQ.id)), bass);
    }

    fn draw_meters(&self, cx: &mut DrawCx<'_>) {
        let theme = cx.theme;
        cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, theme.line, None, &Line::new((FIELD.x1 + 0.5, DISPLAY_TOP), (FIELD.x1 + 0.5, HELP_TOP)));
        for x in WELLS {
            panel(cx.scene, RoundedRect::new(x, METER_TOP, x + 20.0, METER_BOTTOM, 3.0), theme.meter_well, theme.well_border);
        }
        let seg = Stroke::new(8.0).with_dashes(0.0, [2.0, 2.0]);
        let (top, bottom) = (METER_TOP + 4.0, METER_BOTTOM - 4.0);
        let span = bottom - top;
        let y_of = |db: f32| bottom - ((f64::from(db) + METER_RANGE).clamp(0.0, METER_RANGE) / METER_RANGE * span / 4.0).round() * 4.0;
        let bars = [(WELLS[0] + 10.0, self.meters[0], theme.spectrum_pre, Stroke::new(14.0).with_dashes(0.0, [2.0, 2.0])), (WELLS[1] + 5.0, self.meters[1], theme.curve, seg.clone()), (WELLS[1] + 15.0, self.meters[2], theme.curve, seg.clone())];
        for (x, db, color, stroke) in bars {
            cx.scene.stroke(&stroke, Affine::IDENTITY, theme.meter_off, None, &Line::new((x, top), (x, bottom)));
            let line = Line::new((x, bottom), (x, y_of(db)));
            if (line.p1.y - line.p0.y).abs() >= 2.0 {
                cx.scene.stroke(&stroke, Affine::IDENTITY, color, None, &line);
            }
        }
        for (x, t) in [(WELLS[0], "IN"), (WELLS[1], "OUT")] {
            cx.text.draw(cx.scene, t, x + 10.0, DISPLAY_TOP + 596.0, TextStyle::LABEL.sized(9.0), theme.text_lo, Align::Center);
        }
    }

    // ---- help -------------------------------------------------------------------------------

    fn puck_help(host: &dyn ParamHost, puck: Puck, dragging: bool) -> Help {
        let (pan, level) = Self::ids(puck);
        let p = Self::plain(host, &pan);
        let place = if p.abs() < 0.5 {
            "centre".to_owned()
        } else {
            format!("{:.0} % {}", p.abs(), if p < 0.0 { "left" } else { "right" })
        };
        let name = match puck {
            Puck::Source => "Source (your take)".to_owned(),
            Puck::Voice(v) => format!("Double {}", v + 1),
        };
        Help::default()
            .text(format!("{name} · "))
            .slot(place, "100 % right")
            .text(" · ")
            .slot(format!("{:+.1} dB", Self::plain(host, &level)), "−48.0 dB")
            .text(if dragging { " — Shift is fine." } else { " — drag: across where it sits, up and down how loud; wheel: level; double-click: back to its place." })
    }

    fn help_text(&self, host: &dyn ParamHost) -> Help {
        if let Some((_, menu)) = &self.menu {
            let i = self.pointer.and_then(|p| menu.item_rects().iter().position(|r| r.contains(p)));
            return match i {
                Some(0) => Help::default().text("Reset place — back to its default pan and level"),
                Some(_) => Help::default().text("Remove — this double and those after it stop"),
                None => Help::default().text("Pick one; click elsewhere to close."),
            };
        }
        if let Some((puck, _, _)) = self.drag {
            return Self::puck_help(host, puck, true);
        }
        match self.hot {
            Some(Hot::Puck(Puck::Voice(v))) if v >= Self::voices(host) => return Help::default().text(format!("Double {} — click to bring it in", v + 1)),
            Some(Hot::Puck(puck)) => return Self::puck_help(host, puck, false),
            Some(Hot::Pill(Pill::Count(n))) => return Help::default().text(format!("{n} double{} — each its own take of every note", if n > 1 { "s" } else { "" })),
            Some(Hot::Pill(Pill::MonoBass)) => return Help::default().text("Mono bass — every double's lows (under MONO in TONE; the wheel here too) come from your take, in phase: the low end stays solid summed to mono and the correlation stays up. The doubling lives above."),
            Some(Hot::Pill(Pill::Follow)) => return Help::default().text("Every note: each note gets its own timing, pitch, level and tone, switched before the note arrives (a real second take). Every few seconds: a slowly changing double (tape ADT)."),
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
        if self.pointer.is_some_and(|p| CORR.inflate(40.0, 12.0).contains(p)) {
            return Help::default().text("How the output sums to mono: +1 the same both sides, 0 unrelated (a wide double), below 0 cancelling (it would thin out in mono).");
        }
        if self.pointer.is_some_and(|p| LANES.contains(p)) {
            return Help::default().text("Each double, note by note: when it lands (its lag, the shaded spread is TIMING) and how far off pitch (the shaded band is PITCH). Ticks: the notes heard.");
        }
        Help::default().text("Your take (S) and its doubles: drag them where they sit and how loud. Every note of a double is played again: its own timing, pitch, level and tone.")
    }

    // ---- events -----------------------------------------------------------------------------

    fn busy(&self) -> bool {
        self.drag.is_some() || self.menu.is_some() || self.knobs.iter().any(Knob::is_dragging) || self.footer.busy()
    }

    fn hot_at(&self, host: &dyn ParamHost, p: Point) -> Option<Hot> {
        for pill in (1..=MAX_VOICES).map(Pill::Count).chain([Pill::Follow, Pill::MonoBass]) {
            if self.pill(pill).inflate(2.0, 2.0).contains(p) {
                return Some(Hot::Pill(pill));
            }
        }
        if FIELD.contains(p) {
            let voices = Self::voices(host);
            // The source and the playing doubles first (on top), then the ghosts.
            let order = std::iter::once(Puck::Source).chain((0..voices).rev().map(Puck::Voice)).chain((voices..MAX_VOICES).map(Puck::Voice));
            for puck in order {
                if self.puck(host, puck).distance(p) <= PUCK_R + 3.0 {
                    return Some(Hot::Puck(puck));
                }
            }
        }
        None
    }

    fn reset_puck(host: &dyn ParamHost, puck: Puck) {
        let (pan, level) = Self::ids(puck);
        let (dp, dl) = match puck {
            Puck::Source => (SOURCE_PAN.default, SOURCE_LEVEL.default),
            Puck::Voice(v) => V_DEFAULTS[v],
        };
        Self::gesture(host, &pan, dp);
        Self::gesture(host, &level, dl);
    }

    fn handle_event(&mut self, event: &UiEvent, host: &dyn ParamHost) -> bool {
        if let Some((v, menu)) = &mut self.menu {
            if let UiEvent::PointerMove { pos, .. } = *event {
                self.pointer = Some(pos);
            }
            let v = *v;
            match menu.event(event) {
                DropdownEvent::Open => {}
                DropdownEvent::Closed => self.menu = None,
                DropdownEvent::Picked(i) => {
                    self.menu = None;
                    if i == 0 {
                        Self::reset_puck(host, Puck::Voice(v));
                    } else {
                        // This double and those after it stop (one stays: a doubler doubles).
                        Self::gesture(host, VOICES.id, v.saturating_sub(1) as f32);
                    }
                }
            }
            return true;
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
                if let Some((puck, start, at)) = self.drag {
                    let k = if mods.fine() { 0.2 } else { 1.0 };
                    let to = at + (pos - start) * k;
                    let (pan, level) = Self::ids(puck);
                    host.set_normalized(&pan, host.normalize(&pan, SOURCE_PAN.snap((x_pan(to.x) * 100.0) as f32)));
                    host.set_normalized(&level, host.normalize(&level, SOURCE_LEVEL.snap((y_db(to.y.clamp(FIELD_Y.0, FIELD_Y.1)) as f32).clamp(SOURCE_LEVEL.min, SOURCE_LEVEL.max))));
                    return true;
                }
                self.hot = self.hot_at(host, pos);
                true
            }
            UiEvent::PointerLeft => {
                self.pointer = None;
                self.hot = None;
                true
            }
            UiEvent::PointerDown { pos, button: MouseButton::Left, .. } => match self.hot_at(host, pos) {
                Some(Hot::Pill(Pill::Count(n))) => {
                    Self::gesture(host, VOICES.id, (n - 1) as f32);
                    true
                }
                Some(Hot::Pill(Pill::Follow)) => {
                    let on = Self::plain(host, FOLLOW.id) >= 0.5;
                    Self::gesture(host, FOLLOW.id, if on { 0.0 } else { 1.0 });
                    true
                }
                Some(Hot::Pill(Pill::MonoBass)) => {
                    let on = Self::plain(host, MONO_BASS.id) >= 0.5;
                    Self::gesture(host, MONO_BASS.id, if on { 0.0 } else { 1.0 });
                    true
                }
                Some(Hot::Puck(Puck::Voice(v))) if v >= Self::voices(host) => {
                    Self::gesture(host, VOICES.id, v as f32);
                    true
                }
                Some(Hot::Puck(puck)) => {
                    let (pan, level) = Self::ids(puck);
                    host.begin_gesture(&pan);
                    host.begin_gesture(&level);
                    self.drag = Some((puck, pos, self.puck(host, puck)));
                    true
                }
                None => false,
            },
            UiEvent::PointerUp { button: MouseButton::Left, .. } => {
                if let Some((puck, _, _)) = self.drag.take() {
                    let (pan, level) = Self::ids(puck);
                    host.end_gesture(&pan);
                    host.end_gesture(&level);
                    return true;
                }
                false
            }
            UiEvent::PointerDown { pos, button: MouseButton::Right, .. } => match self.hot_at(host, pos) {
                Some(Hot::Puck(Puck::Voice(v))) if v < Self::voices(host) => {
                    let anchor = Rect::from_center_size(pos, (2.0, 2.0));
                    let items: &'static [&'static str] = &VOICE_MENU;
                    let shown = if v == 0 { vec![0] } else { vec![0, 1] };
                    self.menu = Some((v, Dropdown::new(anchor, items, usize::MAX, (DISPLAY_TOP, HELP_TOP)).with_shown(shown).with_min_width(170.0)));
                    true
                }
                _ => false,
            },
            UiEvent::DoubleClick { pos, .. } => match self.hot_at(host, pos) {
                Some(Hot::Puck(puck)) if !matches!(puck, Puck::Voice(v) if v >= Self::voices(host)) => {
                    self.drag = None;
                    Self::reset_puck(host, puck);
                    true
                }
                _ => false,
            },
            UiEvent::Wheel { pos, lines, mods } => match self.hot_at(host, pos) {
                Some(Hot::Pill(Pill::MonoBass)) => {
                    Self::set(host, BASS_FREQ.id, BASS_FREQ.wheel(Self::plain(host, BASS_FREQ.id), lines, mods.fine()));
                    true
                }
                Some(Hot::Puck(puck)) => {
                    let (_, level) = Self::ids(puck);
                    Self::set(host, &level, SOURCE_LEVEL.wheel(Self::plain(host, &level), lines, mods.fine()));
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
        "delay" => " — how far behind your take the doubles play, on average",
        "timing" => " — how loosely they're played: each note lands somewhere within this of the average (0: a machine)",
        "pitch" => " — how far off pitch a note may be, either way (each note its own, settling as it rings)",
        "variation" => " — how much each note's level and tone differ from the take's",
        "chorus" => " — a slow random drift on top (a little: life; a lot: a chorus)",
        "chorus_rate" => " — how fast that drift moves",
        "tone" => " — the doubles brighter or darker than your take (another guitar, another mic)",
        "bass_freq" => " — under this the doubles' lows are your take's, in phase (MONO BASS on, by the meter)",
        _ => "",
    }
}

impl View for DoubleView {
    fn logical_size(&self) -> (f64, f64) {
        (VIEW_W, VIEW_H)
    }

    fn draw(&mut self, cx: &mut DrawCx<'_>) {
        let theme = cx.theme;
        self.draw_header(cx);
        cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.display, None, &Rect::new(0.0, DISPLAY_TOP, VIEW_W, HELP_TOP));
        let hole = self.menu.as_ref().map(|(_, m)| RoundedRect::from_rect(m.bounds(), 8.0));
        if let Some(h) = hole {
            cx.begin_glow_hole(h);
        }
        self.draw_field(cx);
        self.draw_lanes(cx);
        self.draw_groups(cx);
        for k in &self.knobs {
            k.draw(cx);
        }
        self.draw_meters(cx);
        self.help_text(cx.host).draw(cx, Rect::new(0.0, HELP_TOP, VIEW_W, FOOTER_TOP));
        self.footer.draw_frame(cx);
        self.footer.draw_controls(cx);
        if hole.is_some() {
            cx.end_glow_hole();
        }
        if let Some((_, menu)) = &self.menu {
            menu.draw(cx, cx.theme.accent, |_, _, _, _| {});
        }
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
        let fall = 20.0 * dt;
        if let Some(r) = self.history.back().copied() {
            let targets = [r[record::IN], r[record::OUT], r[record::OUT + 1]];
            for (m, t) in self.meters.iter_mut().zip(targets) {
                *m = if t > *m { t } else { (*m - fall).max(t) };
            }
            let source = r[record::IN] + Self::plain(host, SOURCE_LEVEL.id);
            let rings = [source, r[record::PEAK], r[record::PEAK + 1], r[record::PEAK + 2]];
            for (m, t) in self.rings.iter_mut().zip(rings) {
                *m = if t > *m { t } else { (*m - fall).max(t) };
            }
            // The correlation over the last 300 ms that had sound (a meter, not a flicker).
            let sounding: Vec<f32> = self.history.iter().rev().take(60).filter(|r| r[record::OUT].max(r[record::OUT + 1]) > -70.0).map(|r| r[record::CORR]).collect();
            if !sounding.is_empty() {
                self.corr = sounding.iter().sum::<f32>() / sounding.len() as f32;
            }
        }
        fresh || busy || self.meters.iter().chain(&self.rings).any(|&m| m > SILENT + 1.0)
    }
}
