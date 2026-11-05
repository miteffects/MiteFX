//! The Mite DeEss editor (docs/design/deess-brief.md §2), in 1280×760 design units.
//!
//! On top the spectrum: the input dim, the output lit, the trigger band shaded between its two
//! edges (drag an edge, or the band itself), and the reduction as it happens (the filter's own
//! response at the reduction now; dashed, at the full RANGE). Under it, left, the history: the
//! voice (dim), the trigger band (lit), the threshold where it acts (in RELATIVE mode it rides on
//! the voice; drag its grip), and over it the heat-map (where in frequency it turned down, and
//! how far). Right, DETECT and REDUCE, and a big reduction meter.

mod spectrum;

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Instant;

use mite_core::{ParamDisplay, ParamScale, ParamSpec};
use mite_dsp::deess::{reduction_filter, DeEssSettings, Detection, Reduce};
use mite_ui::kurbo::{Affine, BezPath, Cap, Join, Line, Point, Rect, RoundedRect, Stroke};
use mite_ui::paint::{glow_stroke, panel};
use mite_ui::peniko::{Color, Fill};
use mite_ui::session::{Session, SessionConfig};
use mite_ui::widgets::brand::{draw_mark, draw_title};
use mite_ui::widgets::history::{self, HistoryLane};
use mite_ui::widgets::{Footer, FooterDial, FooterToggle, Help, Knob, KnobKind, ToggleGlyph};
use mite_ui::{write_if_changed, Align, DrawCx, MouseButton, ParamHost, RecordingHost, TextStyle, UiEvent, View};

use crate::dsp::{record, HEAT_ROWS, HISTORY_RECORDS, HISTORY_SLICE_MS};
use crate::params::*;
use crate::presets::{preset_ids, FACTORY, PLUGIN};
use crate::DeEssShared;
use spectrum::{freq_x, Spectrum};

/// Mite DeEss wears the DeEss accent (tokens.json `color.accent.deess`, deep sky).
pub const ACCENT: &str = "deess";

const VIEW_W: f64 = 1280.0;
const VIEW_H: f64 = 760.0;
const HEADER: f64 = 56.0;
const DISPLAY_TOP: f64 = HEADER;
const HELP_TOP: f64 = DISPLAY_TOP + 620.0;
const FOOTER_TOP: f64 = HELP_TOP + 28.0;

/// The spectrum, with the reduction's response on its own scale (0 dB near the top, −24 under).
const SPEC: Rect = Rect::new(0.0, DISPLAY_TOP, 1180.0, DISPLAY_TOP + 280.0);
pub(crate) const PLOT_X0: f64 = 28.0;
pub(crate) const PLOT_W: f64 = 1124.0;
const RESP_TOP: f64 = SPEC.y0 + 46.0;
const RESP_BOTTOM: f64 = SPEC.y1 - 28.0;
const RESP_DB: f64 = 24.0;
const EDGE_HANDLE: (f64, f64) = (14.0, 30.0);
const CURVE_POINTS: usize = 200;

/// The history.
const HIST: Rect = Rect::new(0.0, SPEC.y1 + 8.0, 740.0, HELP_TOP - 4.0);
const HIST_SECONDS: f64 = 6.0;
/// The heat-map strip at the history's top, the levels under it.
const HEAT: Rect = Rect::new(HIST.x0, HIST.y0, HIST.x1, HIST.y0 + 96.0);
const LEVELS: Rect = Rect::new(HIST.x0, HEAT.y1 + 4.0, HIST.x1, HIST.y1);
const HIST_RANGE: f64 = 72.0;
const THRESH_GRIP: (f64, f64) = (34.0, 20.0);

const DIAL_D: f64 = 46.0;
const GROUP_COLS: [(f64, f64); 2] = [(752.0, 1062.0), (1070.0, 1172.0)];
const GROUP_ROWS: [(f64, f64); 2] = [(HIST.y0, HIST.y0 + 162.0), (HIST.y1 - 162.0, HIST.y1)];
const PILL_H: f64 = 26.0;

const WELLS: [f64; 2] = [1206.0, 1238.0];
const METER_TOP: f64 = DISPLAY_TOP + 20.0;
const METER_BOTTOM: f64 = DISPLAY_TOP + 592.0;
const METER_RANGE: f64 = 60.0;
const SILENT: f32 = -150.0;

fn x_freq(x: f64) -> f64 {
    20.0 * 1000f64.powf((x - PLOT_X0) / PLOT_W)
}

fn resp_y(db: f64) -> f64 {
    RESP_TOP + (-db).clamp(0.0, RESP_DB) / RESP_DB * (RESP_BOTTOM - RESP_TOP)
}

/// A level's y on the history (0…−72 dBFS).
fn lv_y(db: f64) -> f64 {
    LEVELS.y0 + 8.0 + (-db).clamp(0.0, HIST_RANGE) / HIST_RANGE * (LEVELS.height() - 16.0)
}

fn px_per_db() -> f64 {
    (LEVELS.height() - 16.0) / HIST_RANGE
}

/// A stand-in host holding every Mite DeEss parameter at its default (tests, preview, tools).
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

/// The de-esser's settings as a host's parameters set them.
pub fn settings(host: &dyn ParamHost) -> DeEssSettings {
    deess_settings(|id| host.plain(id, host.normalized(id)))
}

pub fn out_gain(host: &dyn ParamHost) -> f32 {
    host.plain(OUT_GAIN.id, host.normalized(OUT_GAIN.id))
}

/// A pill.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pill {
    Detection(Detection),
    Reduce(Reduce),
}

/// Something to drag.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Grip {
    /// The trigger band's low edge, high edge, or the band itself.
    Lo,
    Hi,
    Band,
    Threshold,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Hot {
    Pill(Pill),
    Grip(Grip),
}

struct Group {
    name: &'static str,
    words: &'static str,
    rect: Rect,
}

pub struct DeEssView {
    shared: Arc<DeEssShared>,
    session: Session,
    footer: Footer,
    knobs: Vec<Knob>,
    groups: Vec<Group>,
    spectrum: Spectrum,
    history: VecDeque<[f32; record::WIDTH]>,
    seen: usize,
    last_fresh: Instant,
    last_tick: Instant,
    pointer: Option<Point>,
    hot: Option<Hot>,
    /// A drag: the grip, where it was grabbed, and the values then (lo, hi, threshold).
    drag: Option<(Grip, Point, (f32, f32, f32))>,
    meters: [f32; 4],
    /// The reduction as drawn (eased), and its peak held a moment.
    gr: f32,
    gr_peak: f32,
    gr_peak_at: Instant,
}

impl DeEssView {
    pub fn new(shared: Arc<DeEssShared>) -> Self {
        let footer = Footer::new(
            FOOTER_TOP,
            (VIEW_W, VIEW_H),
            vec![
                FooterToggle { id: AUTO_GAIN.id, glyph: ToggleGlyph::Auto, words: "Auto gain — gives back the loudness the reduction took (the output as loud as the input, over a few seconds), so Bypass compares fairly" },
                FooterToggle { id: LISTEN.id, glyph: ToggleGlyph::Listen, words: "Listen — the trigger band alone: what the detector hears" },
                FooterToggle { id: BYPASS.id, glyph: ToggleGlyph::Bypass, words: "Bypass — the input as it came (in time with the lookahead)" },
            ],
            vec![FooterDial { spec: OUT_GAIN, kind: KnobKind::Bipolar, name: "OUTPUT", words: "Output gain" }],
        );
        let detect = Rect::new(GROUP_COLS[0].0, GROUP_ROWS[0].0, GROUP_COLS[0].1, GROUP_ROWS[0].1);
        let reduce = Rect::new(GROUP_COLS[0].0, GROUP_ROWS[1].0, GROUP_COLS[0].1, GROUP_ROWS[1].1);
        let meter = Rect::new(GROUP_COLS[1].0, GROUP_ROWS[0].0, GROUP_COLS[1].1, GROUP_ROWS[1].1);
        let dy = 108.0;
        let knobs = vec![
            Knob::new(THRESHOLD, "THRESH", KnobKind::Unipolar, Point::new(detect.x0 + detect.width() * 0.3, detect.y0 + dy), DIAL_D),
            Knob::new(LOOKAHEAD, "LOOKAHEAD", KnobKind::Unipolar, Point::new(detect.x0 + detect.width() * 0.7, detect.y0 + dy), DIAL_D),
            Knob::new(RANGE, "RANGE", KnobKind::Unipolar, Point::new(reduce.x0 + reduce.width() * 0.3, reduce.y0 + dy), DIAL_D),
            Knob::new(RELEASE, "RELEASE", KnobKind::Unipolar, Point::new(reduce.x0 + reduce.width() * 0.7, reduce.y0 + dy), DIAL_D),
        ];
        let groups = vec![
            Group { name: "DETECT", words: "what counts as a sibilant", rect: detect },
            Group { name: "REDUCE", words: "what is turned down, how far and how long", rect: reduce },
            Group { name: "REDUCTION", words: "how far it is turning down now", rect: meter },
        ];
        Self {
            session: Session::new(SessionConfig { plugin: PLUGIN, factory: FACTORY, sound_ids: preset_ids(), snapshot_ids: preset_ids() }, shared.snapshots.clone(), shared.preset.clone()),
            footer,
            knobs,
            groups,
            spectrum: Spectrum::new(),
            history: VecDeque::with_capacity(HISTORY_RECORDS),
            seen: 0,
            last_fresh: Instant::now(),
            last_tick: Instant::now(),
            pointer: None,
            hot: None,
            drag: None,
            meters: [SILENT; 4],
            gr: 0.0,
            gr_peak: 0.0,
            gr_peak_at: Instant::now(),
            shared,
        }
    }

    pub fn with_preset_dir(mut self, dir: std::path::PathBuf) -> Self {
        self.session = self.session.with_preset_dir(dir);
        self
    }

    /// Reads the spectra at once (tools render a single frame).
    pub fn settle_spectrum(&mut self) {
        for _ in 0..60 {
            self.spectrum.update(&self.shared, 0.3);
            std::thread::sleep(std::time::Duration::from_millis(16));
        }
    }

    fn plain(host: &dyn ParamHost, id: &str) -> f32 {
        host.plain(id, host.normalized(id))
    }


    fn set_raw(host: &dyn ParamHost, id: &str, v: f32) {
        host.set_normalized(id, host.normalize(id, v));
    }

    fn gesture(host: &dyn ParamHost, id: &str, v: f32) {
        host.begin_gesture(id);
        write_if_changed(host, id, host.normalize(id, v));
        host.end_gesture(id);
    }

    fn fs(&self) -> f32 {
        self.shared.sample_rate.load().max(8000.0)
    }

    pub fn pill(&self, pill: Pill) -> Rect {
        let (row, k, n) = match pill {
            Pill::Detection(Detection::Relative) => (0, 0, 2),
            Pill::Detection(Detection::Absolute) => (0, 1, 2),
            Pill::Reduce(r) => (1, r.index(), 4),
        };
        let x0 = GROUP_COLS[0].0 + 14.0;
        let w = (GROUP_COLS[0].1 - GROUP_COLS[0].0 - 28.0 - 8.0 * (n - 1) as f64) / n as f64;
        let x = x0 + k as f64 * (w + 8.0);
        let y = GROUP_ROWS[row].0 + 30.0;
        Rect::new(x, y, x + w, y + PILL_H)
    }

    /// Where a grip sits (its handle's centre).
    pub fn grip(&self, host: &dyn ParamHost, grip: Grip) -> Point {
        let (lo, hi) = (f64::from(Self::plain(host, LO.id)), f64::from(Self::plain(host, HI.id)));
        match grip {
            Grip::Lo => Point::new(freq_x(lo), SPEC.y0 + 20.0),
            Grip::Hi => Point::new(freq_x(hi), SPEC.y0 + 20.0),
            Grip::Band => Point::new(0.5 * (freq_x(lo) + freq_x(hi)), SPEC.y0 + 20.0),
            Grip::Threshold => Point::new(HIST.x1 - 8.0 - THRESH_GRIP.0 / 2.0, self.threshold_y(host)),
        }
    }

    /// The threshold's y at the history's right end (in RELATIVE mode: on the voice now).
    fn threshold_y(&self, host: &dyn ParamHost) -> f64 {
        let t = f64::from(Self::plain(host, THRESHOLD.id));
        match settings(host).detection {
            Detection::Absolute => lv_y(t),
            Detection::Relative => {
                let whole = self.history.back().map_or(-30.0, |r| f64::from(r[record::WHOLE]).max(-HIST_RANGE));
                lv_y(whole + t)
            }
        }
    }

    pub fn dial(&self, id: &str) -> Option<Point> {
        self.knobs.iter().find(|k| k.id == id).map(|k| k.center)
    }

    pub fn dial_groups(&self) -> Vec<(&'static str, Rect)> {
        self.groups.iter().map(|g| (g.name, g.rect)).collect()
    }

    pub fn freq_x(&self, hz: f64) -> f64 {
        freq_x(hz)
    }

    pub fn level_y(&self, db: f64) -> f64 {
        lv_y(db)
    }

    // ---- drawing ----------------------------------------------------------------------------

    fn draw_header(&mut self, cx: &mut DrawCx<'_>) {
        let theme = cx.theme;
        cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.window, None, &Rect::new(0.0, 0.0, VIEW_W, HEADER));
        cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, theme.line, None, &Line::new((0.0, HEADER - 0.5), (VIEW_W, HEADER - 0.5)));
        draw_mark(cx, 16.0, 15.0, 26.0);
        draw_title(cx, 52.0, 18.0, "DeEss");
        self.session.draw(cx);
    }

    fn draw_spectrum(&self, cx: &mut DrawCx<'_>, s: &DeEssSettings) {
        let theme = cx.theme;
        let host = cx.host;
        let accent = theme.accent;
        cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.well, None, &SPEC);
        let (lo, hi) = (f64::from(s.lo_hz), f64::from(s.hi_hz));
        let (xl, xh) = (freq_x(lo), freq_x(hi));
        // The trigger band.
        cx.scene.fill(Fill::NonZero, Affine::IDENTITY, accent.multiply_alpha(0.08), None, &Rect::new(xl, SPEC.y0, xh, SPEC.y1));
        cx.begin_clip(&SPEC);
        self.spectrum.draw(cx, SPEC);
        // The reduction's response: dashed at the full range, lit at the reduction now.
        let label = TextStyle { tabular: true, ..TextStyle::BODY.sized(10.0).weighted(600.0) };
        for (db, t) in [(0.0, "0 dB"), (-12.0, "−12"), (-24.0, "−24")] {
            cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, if db == 0.0 { theme.grid_major } else { theme.grid_minor }, None, &Line::new((PLOT_X0, resp_y(db)), (SPEC.x1, resp_y(db))));
            cx.text.draw(cx.scene, t, 6.0, resp_y(db) - 6.0, label, theme.text_lo, Align::Left);
        }
        let fs = self.fs();
        // The filter designed once per curve, its response read at every point.
        let curve = |gr: f32| -> BezPath {
            let sections = reduction_filter(s, gr, fs);
            let pts: Vec<Point> = (0..CURVE_POINTS)
                .map(|i| {
                    let x = PLOT_X0 + PLOT_W * i as f64 / (CURVE_POINTS - 1) as f64;
                    let f = x_freq(x).min(0.49 * f64::from(fs));
                    let db = if s.reduce == Reduce::Wide { -f64::from(gr) } else { sections.iter().map(|c| c.response_db(f, f64::from(fs))).sum() };
                    Point::new(x, resp_y(db))
                })
                .collect();
            history::smooth_path(&pts)
        };
        if s.reduce == Reduce::Spectral {
            // Per frequency, as it happens: the reduction of every bin; dashed, the most RANGE
            // allows across the band.
            let full = {
                let mut p = BezPath::new();
                p.move_to((PLOT_X0, resp_y(0.0)));
                p.line_to((xl, resp_y(0.0)));
                p.line_to((xl, resp_y(-f64::from(s.range_db))));
                p.line_to((xh, resp_y(-f64::from(s.range_db))));
                p.line_to((xh, resp_y(0.0)));
                p.line_to((SPEC.x1, resp_y(0.0)));
                p
            };
            cx.scene.stroke(&Stroke::new(1.2).with_dashes(0.0, [5.0, 4.0]), Affine::IDENTITY, accent.multiply_alpha(0.45), None, &full);
            glow_stroke(cx, &self.spectral_curve(), 2.0, accent, accent, 4.0);
        } else {
            cx.scene.stroke(&Stroke::new(1.2).with_dashes(0.0, [5.0, 4.0]), Affine::IDENTITY, accent.multiply_alpha(0.45), None, &curve(s.range_db));
            let now = curve(self.gr);
            glow_stroke(cx, &now, 2.2, accent, accent, 4.0);
        }
        cx.end_clip();
        // The band's edges and its handles.
        for (grip, x) in [(Grip::Lo, xl), (Grip::Hi, xh)] {
            let hot = self.hot == Some(Hot::Grip(grip)) || self.drag.is_some_and(|(g, _, _)| g == grip || g == Grip::Band);
            cx.scene.stroke(&Stroke::new(1.2), Affine::IDENTITY, accent.multiply_alpha(if hot { 0.9 } else { 0.5 }), None, &Line::new((x, SPEC.y0 + 4.0), (x, SPEC.y1)));
            let p = self.grip(host, grip);
            let r = RoundedRect::from_rect(Rect::from_center_size(p, EDGE_HANDLE), 7.0);
            cx.scene.fill(Fill::NonZero, Affine::IDENTITY, if hot { accent } else { theme.display }, None, &r);
            glow_stroke(cx, &r, 1.4, accent, accent, if hot { 4.0 } else { 2.0 });
            for dx in [-2.0, 2.0] {
                cx.scene.stroke(&Stroke::new(1.2), Affine::IDENTITY, if hot { theme.display } else { accent }, None, &Line::new((p.x + dx, p.y - 6.0), (p.x + dx, p.y + 6.0)));
            }
        }
        let band_hot = self.hot == Some(Hot::Grip(Grip::Band));
        let mid = self.grip(host, Grip::Band);
        cx.text.draw(cx.scene, "TRIGGER", mid.x, SPEC.y0 + 14.0, TextStyle::LABEL.sized(9.0), if band_hot { theme.text_hi } else { accent.multiply_alpha(0.8) }, Align::Center);
    }

    /// The spectral reduction per bin, as the engine last left it, on the response scale.
    fn spectral_curve(&self) -> BezPath {
        let tap = &self.shared.gains;
        let count = tap.count.load(std::sync::atomic::Ordering::Acquire);
        let bin_hz = f64::from(tap.bin_hz.load());
        let mut pts = vec![Point::new(PLOT_X0, resp_y(0.0))];
        if bin_hz > 0.0 {
            for k in 1..count {
                let f = k as f64 * bin_hz;
                if !(20.0..=20_000.0).contains(&f) {
                    continue;
                }
                pts.push(Point::new(freq_x(f), resp_y(-f64::from(tap.gains[k].load()))));
            }
        }
        pts.push(Point::new(SPEC.x1, resp_y(0.0)));
        let mut p = BezPath::new();
        for (i, q) in pts.iter().enumerate() {
            if i == 0 {
                p.move_to(*q);
            } else {
                p.line_to(*q);
            }
        }
        p
    }

    fn since_fresh(&self) -> f64 {
        (self.last_fresh.elapsed().as_secs_f64() * 1000.0 / f64::from(HISTORY_SLICE_MS)).min(4.0)
    }

    fn draw_history(&self, cx: &mut DrawCx<'_>, s: &DeEssSettings) {
        let theme = cx.theme;
        let host = cx.host;
        let accent = theme.accent;
        cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.display, None, &HIST);
        let label = TextStyle { tabular: true, ..TextStyle::BODY.sized(10.0).weighted(600.0) };
        for db in [-12.0, -24.0, -36.0, -48.0, -60.0] {
            cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, theme.grid_minor, None, &Line::new((LEVELS.x0, lv_y(db)), (LEVELS.x1, lv_y(db))));
            cx.text.draw(cx.scene, &format!("−{}", -db as i32), LEVELS.x0 + 6.0, lv_y(db) - 13.0, label, theme.text_lo, Align::Left);
        }
        let lane = HistoryLane { x0: HIST.x0, width: HIST.width(), top: HIST.y0, bottom: HIST.y1, seconds: HIST_SECONDS, slice_ms: HISTORY_SLICE_MS };
        // The heat-map along the top: where in frequency it reduced (2…20 kHz, high at the top),
        // the deeper the brighter.
        cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.well, None, &HEAT);
        cx.text.draw(cx.scene, "20k", HEAT.x0 + 6.0, HEAT.y0 + 1.0, TextStyle { tabular: true, ..TextStyle::BODY.sized(9.0).weighted(600.0) }, theme.text_lo, Align::Left);
        cx.text.draw(cx.scene, "2k", HEAT.x0 + 6.0, HEAT.y1 - 12.0, TextStyle { tabular: true, ..TextStyle::BODY.sized(9.0).weighted(600.0) }, theme.text_lo, Align::Left);
        let cols = lane.columns(&self.history, self.seen, self.since_fresh());
        if cols.len() >= 2 {
            cx.begin_clip(&HEAT);
            let row_h = HEAT.height() / HEAT_ROWS as f64;
            let range = f64::from(s.range_db.max(1.0));
            for w in cols.windows(2) {
                let (c, x1) = (&w[0], w[1].x);
                for r in 0..HEAT_ROWS {
                    let db = f64::from(c.high[record::HEAT + r]);
                    if db > 0.1 {
                        let y0 = HEAT.y1 - (r + 1) as f64 * row_h;
                        let a = (db / range).clamp(0.0, 1.0) as f32;
                        cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.gain_reduction.multiply_alpha(0.15 + 0.85 * a), None, &Rect::new(c.x, y0, x1 + 0.5, y0 + row_h + 0.5));
                    }
                }
            }
            cx.end_clip();
        }
        cx.begin_clip(&LEVELS);
        let t = f64::from(s.threshold_db);
        if cols.len() >= 2 {
            type Col = history::Column<{ record::WIDTH }>;
            let line = |f: &dyn Fn(&Col) -> f64| history::column_line(&cols, f);
            // The voice (the whole input, RMS), dim and filled.
            let whole = line(&|c| lv_y(f64::from(c.high[record::WHOLE])));
            cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.spectrum_pre.multiply_alpha(0.12), None, &history::close_to(whole.clone(), &cols, HIST.y1));
            cx.scene.stroke(&Stroke::new(1.2), Affine::IDENTITY, theme.spectrum_pre.multiply_alpha(0.7), None, &whole);
            // The threshold where it acts: on the voice (relative) or a level (absolute).
            let thresh = match s.detection {
                Detection::Relative => line(&|c| lv_y(f64::from(c.high[record::WHOLE]) + t)),
                Detection::Absolute => line(&|_| lv_y(t)),
            };
            cx.scene.stroke(&Stroke::new(1.2).with_dashes(0.0, [5.0, 4.0]), Affine::IDENTITY, accent.multiply_alpha(0.7), None, &thresh);
            // The trigger band, lit.
            let band = line(&|c| lv_y(f64::from(c.high[record::BAND])));
            let look = history::LevelLook { fill: [accent.multiply_alpha(0.18), accent.multiply_alpha(0.02)], span: (HIST.y0, HIST.y1), edge: accent, width: 1.8, glow: 2.4 };
            history::draw_level(cx, &cols, &band, HIST.y1, look);
        }
        cx.end_clip();
        // The threshold's grip.
        let p = self.grip(host, Grip::Threshold);
        let hot = self.hot == Some(Hot::Grip(Grip::Threshold)) || self.drag.is_some_and(|(g, _, _)| g == Grip::Threshold);
        let r = RoundedRect::from_rect(Rect::from_center_size(p, THRESH_GRIP), 9.0);
        cx.scene.fill(Fill::NonZero, Affine::IDENTITY, if hot { accent } else { theme.display }, None, &r);
        cx.scene.stroke(&Stroke::new(1.3), Affine::IDENTITY, accent, None, &r);
        draw_chevrons(cx, p, if hot { theme.display } else { accent });
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

    fn draw_groups(&self, cx: &mut DrawCx<'_>, s: &DeEssSettings) {
        let theme = cx.theme;
        for g in &self.groups {
            panel(cx.scene, RoundedRect::from_rect(g.rect, 10.0), theme.well, theme.well_border);
            cx.text.draw(cx.scene, g.name, g.rect.x0 + 12.0, g.rect.y0 + 9.0, TextStyle::LABEL.sized(9.0), theme.text_mid, Align::Left);
        }
        self.draw_pill(cx, Pill::Detection(Detection::Relative), "RELATIVE", s.detection == Detection::Relative);
        self.draw_pill(cx, Pill::Detection(Detection::Absolute), "ABSOLUTE", s.detection == Detection::Absolute);
        for r in [Reduce::Shelf, Reduce::Band, Reduce::Wide, Reduce::Spectral] {
            self.draw_pill(cx, Pill::Reduce(r), &Reduce::NAMES[r.index()].to_uppercase(), s.reduce == r);
        }
        // The reduction meter: a bar down from the top (0…24 dB), the peak held.
        let g = self.groups[2].rect;
        let bar = Rect::new(g.center().x - 14.0, g.y0 + 30.0, g.center().x + 14.0, g.y1 - 44.0);
        panel(cx.scene, RoundedRect::from_rect(bar, 4.0), theme.meter_well, theme.well_border);
        let y = |db: f32| bar.y0 + 3.0 + (f64::from(db) / 24.0).clamp(0.0, 1.0) * (bar.height() - 6.0);
        if self.gr > 0.05 {
            cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.gain_reduction, None, &Rect::new(bar.x0 + 3.0, bar.y0 + 3.0, bar.x1 - 3.0, y(self.gr)));
        }
        if self.gr_peak > 0.05 {
            cx.scene.stroke(&Stroke::new(2.0), Affine::IDENTITY, theme.text_hi, None, &Line::new((bar.x0 + 2.0, y(self.gr_peak)), (bar.x1 - 2.0, y(self.gr_peak))));
        }
        for db in [6.0, 12.0, 18.0] {
            cx.text.draw(cx.scene, &format!("{db:.0}"), bar.x1 + 6.0, y(db) - 6.0, TextStyle { tabular: true, ..TextStyle::BODY.sized(9.0).weighted(600.0) }, theme.text_lo, Align::Left);
        }
        let read = format!("{}{:.1} dB", if self.gr_peak > 0.05 { "−" } else { "" }, self.gr_peak);
        cx.text.draw(cx.scene, &read, g.center().x, g.y1 - 30.0, TextStyle { tabular: true, ..TextStyle::BODY.sized(12.0).weighted(700.0) }, theme.text_hi, Align::Center);
    }

    fn draw_meters(&self, cx: &mut DrawCx<'_>) {
        let theme = cx.theme;
        cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, theme.line, None, &Line::new((SPEC.x1 + 0.5, DISPLAY_TOP), (SPEC.x1 + 0.5, HELP_TOP)));
        for x in WELLS {
            panel(cx.scene, RoundedRect::new(x, METER_TOP, x + 20.0, METER_BOTTOM, 3.0), theme.meter_well, theme.well_border);
        }
        let seg = Stroke::new(8.0).with_dashes(0.0, [2.0, 2.0]);
        let (top, bottom) = (METER_TOP + 4.0, METER_BOTTOM - 4.0);
        let span = bottom - top;
        let y_of = |db: f32| bottom - ((f64::from(db) + METER_RANGE).clamp(0.0, METER_RANGE) / METER_RANGE * span / 4.0).round() * 4.0;
        for (i, x) in WELLS.iter().flat_map(|&w| [w + 5.0, w + 15.0]).enumerate() {
            cx.scene.stroke(&seg, Affine::IDENTITY, theme.meter_off, None, &Line::new((x, top), (x, bottom)));
            let line = Line::new((x, bottom), (x, y_of(self.meters[i])));
            let color = if i < 2 { theme.spectrum_pre } else { theme.curve };
            if (line.p1.y - line.p0.y).abs() >= 2.0 {
                cx.scene.stroke(&seg, Affine::IDENTITY, color, None, &line);
            }
        }
        for (x, t) in [(WELLS[0], "IN"), (WELLS[1], "OUT")] {
            cx.text.draw(cx.scene, t, x + 10.0, DISPLAY_TOP + 596.0, TextStyle::LABEL.sized(9.0), theme.text_lo, Align::Center);
        }
    }

    fn help_text(&self, host: &dyn ParamHost) -> Help {
        let s = settings(host);
        let hz = |v: f32| if v >= 1000.0 { format!("{:.2} kHz", v / 1000.0) } else { format!("{v:.0} Hz") };
        let dragging = self.drag.map(|(g, _, _)| g);
        let grip = dragging.or(match self.hot {
            Some(Hot::Grip(g)) => Some(g),
            _ => None,
        });
        if let Some(g) = grip {
            return match g {
                Grip::Lo | Grip::Hi | Grip::Band => Help::default()
                    .text("Trigger band ")
                    .slot(hz(s.lo_hz), "16.00 kHz")
                    .text(" – ")
                    .slot(hz(s.hi_hz), "20.00 kHz")
                    .text(if dragging.is_some() { " — Shift is fine." } else { " — where a sibilant is listened for (drag an edge, or the band); SHELF turns down everything above its low edge, BAND only inside it." }),
                Grip::Threshold => {
                    let t = format!("{:.1} dB", s.threshold_db).replace('-', "−");
                    match s.detection {
                        Detection::Relative => Help::default().text("Threshold ").slot(t, "−60.0 dB").text(" under the voice — a sibilant pokes over it and is brought back to it; it rides on the voice, so a loud line is not de-essed more."),
                        Detection::Absolute => Help::default().text("Threshold ").slot(t, "−60.0 dB").text(" — where the trigger band's level starts being turned down."),
                    }
                }
            };
        }
        match self.hot {
            Some(Hot::Pill(Pill::Detection(Detection::Relative))) => return Help::default().text("Relative — a sibilant is the top of the voice loud against the voice itself: the same de-essing loud or quiet (smart, the default)"),
            Some(Hot::Pill(Pill::Detection(Detection::Absolute))) => return Help::default().text("Absolute — the top past a level (dBFS): for cymbals and anything not a voice"),
            Some(Hot::Pill(Pill::Reduce(Reduce::Shelf))) => return Help::default().text("Shelf — everything above the band's low edge turned down (the classic split de-esser)"),
            Some(Hot::Pill(Pill::Reduce(Reduce::Band))) => return Help::default().text("Band — only the trigger band turned down; the air above it stays"),
            Some(Hot::Pill(Pill::Reduce(Reduce::Wide))) => return Help::default().text("Wide — the whole signal turned down while a sibilant sounds"),
            Some(Hot::Pill(Pill::Reduce(Reduce::Spectral))) => return Help::default().text("Spectral — every frequency of the band on its own: only what pokes over comes down (a whistly s loses its whistle, the hiss beside it stays). 11 ms of latency; the LOOKAHEAD dial does not apply."),
            _ => {}
        }
        if let Some(h) = self.footer.help(host) {
            return h;
        }
        if let Some(k) = self.knobs.iter().find(|k| self.pointer.is_some_and(|p| k.bounds().contains(p))) {
            let v = Self::plain(host, &k.id);
            let words = match k.spec.id {
                "threshold" => " — how far a sibilant may poke (under the voice in RELATIVE, a level in ABSOLUTE); also the grip on the history",
                "range" => " — the most it will turn down",
                "release" => " — how fast it lets go after a sibilant",
                "lookahead_ms" => " — how long before a sibilant the reduction is already in place (it is the latency; 0: none, the s's first edge slips through; SPECTRAL has its own)",
                _ => "",
            };
            return Help::default().text(format!("{} ", k.spec.name)).slot(k.spec.format(v), k.spec.format(k.spec.max)).text(words);
        }
        if let Some(g) = self.groups.iter().find(|g| self.pointer.is_some_and(|p| g.rect.contains(p))) {
            return Help::default().text(format!("{} — {}", g.name, g.words));
        }
        if let Some(h) = self.session.help() {
            return h;
        }
        if self.pointer.is_some_and(|p| HEAT.contains(p)) {
            return Help::default().text("The heat-map: where in frequency (2 kHz at the bottom, 20 kHz at the top) it turned down, and how far (the brighter the deeper), over the last six seconds.");
        }
        if self.pointer.is_some_and(|p| HIST.contains(p)) {
            return Help::default().text("The voice (dim), the trigger band (lit) and the threshold (dashed) where it acts; above, the heat-map of what was turned down.");
        }
        Help::default().text("Top: the trigger band (drag its edges) and the reduction as it happens (dashed: at the full RANGE). Below: the voice, the band and the threshold.")
    }

    // ---- events -----------------------------------------------------------------------------

    fn busy(&self) -> bool {
        self.drag.is_some() || self.knobs.iter().any(Knob::is_dragging) || self.footer.busy()
    }

    fn hot_at(&self, host: &dyn ParamHost, p: Point) -> Option<Hot> {
        let pills = [Pill::Detection(Detection::Relative), Pill::Detection(Detection::Absolute), Pill::Reduce(Reduce::Shelf), Pill::Reduce(Reduce::Band), Pill::Reduce(Reduce::Wide), Pill::Reduce(Reduce::Spectral)];
        for pill in pills {
            if self.pill(pill).inflate(2.0, 2.0).contains(p) {
                return Some(Hot::Pill(pill));
            }
        }
        for g in [Grip::Lo, Grip::Hi] {
            let q = self.grip(host, g);
            if Rect::from_center_size(q, EDGE_HANDLE).inflate(4.0, 4.0).contains(p) || ((p.x - q.x).abs() < 5.0 && SPEC.contains(p)) {
                return Some(Hot::Grip(g));
            }
        }
        let (xl, xh) = (self.grip(host, Grip::Lo).x, self.grip(host, Grip::Hi).x);
        if p.x > xl && p.x < xh && p.y > SPEC.y0 && p.y < SPEC.y0 + 40.0 {
            return Some(Hot::Grip(Grip::Band));
        }
        if Rect::from_center_size(self.grip(host, Grip::Threshold), THRESH_GRIP).inflate(4.0, 3.0).contains(p) {
            return Some(Hot::Grip(Grip::Threshold));
        }
        None
    }

    fn grip_ids(g: Grip) -> &'static [&'static str] {
        match g {
            Grip::Lo => &["lo"],
            Grip::Hi => &["hi"],
            Grip::Band => &["lo", "hi"],
            Grip::Threshold => &["threshold"],
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
                if let Some((g, start, (lo, hi, t))) = self.drag {
                    let k = if mods.fine() { 0.2 } else { 1.0 };
                    let d = (pos - start) * k;
                    // Across: an octave per PLOT_W / 10 px (the plot's own scale).
                    let ratio = |dx: f64| 1000f64.powf(dx / PLOT_W) as f32;
                    match g {
                        Grip::Lo => Self::set_raw(host, LO.id, LO.snap((lo * ratio(d.x)).clamp(LO.min, (hi / 1.2).min(LO.max)))),
                        Grip::Hi => Self::set_raw(host, HI.id, HI.snap((hi * ratio(d.x)).clamp((lo * 1.2).max(HI.min), HI.max))),
                        Grip::Band => {
                            let r = ratio(d.x).clamp(LO.min / lo, HI.max / hi);
                            Self::set_raw(host, LO.id, LO.snap(lo * r));
                            Self::set_raw(host, HI.id, HI.snap(hi * r));
                        }
                        Grip::Threshold => {
                            let v = t - (d.y / px_per_db()) as f32;
                            Self::set_raw(host, THRESHOLD.id, THRESHOLD.snap(v.clamp(THRESHOLD.min, THRESHOLD.max)));
                        }
                    }
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
                Some(Hot::Pill(pill)) => {
                    match pill {
                        Pill::Detection(d) => Self::gesture(host, DETECTION.id, if d == Detection::Absolute { 1.0 } else { 0.0 }),
                        Pill::Reduce(r) => Self::gesture(host, REDUCE.id, r.index() as f32),
                    }
                    true
                }
                Some(Hot::Grip(g)) => {
                    for id in Self::grip_ids(g) {
                        host.begin_gesture(id);
                    }
                    self.drag = Some((g, pos, (Self::plain(host, LO.id), Self::plain(host, HI.id), Self::plain(host, THRESHOLD.id))));
                    true
                }
                None => false,
            },
            UiEvent::PointerUp { button: MouseButton::Left, .. } => {
                if let Some((g, _, _)) = self.drag.take() {
                    for id in Self::grip_ids(g) {
                        host.end_gesture(id);
                    }
                    return true;
                }
                false
            }
            UiEvent::DoubleClick { pos, .. } => match self.hot_at(host, pos) {
                Some(Hot::Grip(g)) => {
                    self.drag = None;
                    for id in Self::grip_ids(g) {
                        let spec = spec_for(id).expect("spec");
                        Self::gesture(host, id, spec.default);
                    }
                    true
                }
                _ => false,
            },
            UiEvent::Wheel { pos, lines, mods } => match self.hot_at(host, pos) {
                Some(Hot::Grip(Grip::Threshold)) => {
                    let v = THRESHOLD.wheel(Self::plain(host, THRESHOLD.id), lines, mods.fine());
                    write_if_changed(host, THRESHOLD.id, host.normalize(THRESHOLD.id, v));
                    true
                }
                _ => false,
            },
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

impl View for DeEssView {
    fn logical_size(&self) -> (f64, f64) {
        (VIEW_W, VIEW_H)
    }

    fn draw(&mut self, cx: &mut DrawCx<'_>) {
        let s = settings(cx.host);
        let theme = cx.theme;
        self.draw_header(cx);
        cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.display, None, &Rect::new(0.0, DISPLAY_TOP, VIEW_W, HELP_TOP));
        self.draw_spectrum(cx, &s);
        self.draw_history(cx, &s);
        self.draw_groups(cx, &s);
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
        let mut gr_new = 0.0f32;
        self.shared.history.read_since(&mut self.seen, |r| {
            if history.len() == HISTORY_RECORDS {
                history.pop_front();
            }
            history.push_back(*r);
            gr_new = gr_new.max(r[record::GR]);
            fresh = true;
        });
        if fresh {
            self.last_fresh = Instant::now();
        }
        let fall = 20.0 * dt;
        if let Some(r) = self.history.back().copied() {
            let targets = [r[record::IN], r[record::IN + 1], r[record::OUT], r[record::OUT + 1]];
            for (m, t) in self.meters.iter_mut().zip(targets) {
                *m = if t > *m { t } else { (*m - fall).max(t) };
            }
        }
        // The reduction: up at once, down at 40 dB/s; the peak held 1.5 s.
        self.gr = if gr_new > self.gr { gr_new } else { (self.gr - 40.0 * dt).max(gr_new).max(0.0) };
        if self.gr >= self.gr_peak || self.gr_peak_at.elapsed().as_secs_f32() > 1.5 {
            self.gr_peak = self.gr;
            self.gr_peak_at = Instant::now();
        }
        let spectrum = self.spectrum.update(&self.shared, f64::from(dt));
        let _ = host;
        fresh || spectrum || busy || self.gr > 0.01 || self.meters.iter().any(|&m| m > SILENT + 1.0)
    }
}
