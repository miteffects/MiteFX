//! The Mite Mod editor (docs/design/mod-brief.md §2), in 1280×760 design units.
//!
//! On top the five modes, each a tile with a picture of what it does. Under them the mode's own
//! view: chorus, ensemble and vibrato draw their voices' readings scrolling in time (the delays,
//! or vibrato's pitch), drag up and down for DELAY (DEPTH for vibrato); flanger and phaser draw
//! their response as it moves, with the span the sweep covers shaded, and a puck to drag (across:
//! MANUAL or CENTRE, up and down: FEEDBACK). Right of it the mode's own controls. Bottom left the
//! LFO: its shape over one cycle with the left and right sides' places riding on it, the shapes,
//! SYNC and the division. Bottom right MOTION (RATE, DEPTH, SPREAD) and SOUND (MIX, WIDTH, the
//! wet's LOW and HIGH CUT).

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Instant;

use mite_core::ParamSpec;
use mite_dsp::modulation::{self as m, Mode, ModSettings, Shape, DIVISIONS, STAGE_COUNTS};
use mite_ui::kurbo::{Affine, BezPath, Circle, Line, Point, Rect, RoundedRect, Stroke};
use mite_ui::paint::{glow_stroke, panel};
use mite_ui::peniko::Fill;
use mite_ui::session::{Session, SessionConfig};
use mite_ui::widgets::brand::{draw_mark, draw_title};
use mite_ui::widgets::history::{smooth_path, HistoryLane};
use mite_ui::widgets::{Dropdown, DropdownEvent, Footer, FooterDial, FooterToggle, Help, Knob, KnobKind, ToggleGlyph};
use mite_ui::{write_if_changed, Align, DrawCx, MouseButton, ParamHost, RecordingHost, TextStyle, UiEvent, View};

use crate::dsp::{record, HISTORY_RECORDS, HISTORY_SLICE_MS};
use crate::params::*;
use crate::presets::{preset_ids, FACTORY, PLUGIN};
use crate::ModShared;

/// Mite Mod wears the Mod accent (tokens.json `color.accent.mod`, lilac).
pub const ACCENT: &str = "mod";

const VIEW_W: f64 = 1280.0;
const VIEW_H: f64 = 760.0;
const HEADER: f64 = 56.0;
const DISPLAY_TOP: f64 = HEADER;
const HELP_TOP: f64 = DISPLAY_TOP + 620.0;
const FOOTER_TOP: f64 = HELP_TOP + 28.0;

/// The mode tiles.
const TILE_Y: (f64, f64) = (DISPLAY_TOP + 10.0, DISPLAY_TOP + 58.0);
const TILE_W: f64 = 164.0;
const TILE_GAP: f64 = 8.0;
/// The mode's view and its own controls.
const PLOT: Rect = Rect::new(0.0, DISPLAY_TOP + 68.0, 880.0, DISPLAY_TOP + 366.0);
const PLOT_IN: Rect = Rect::new(PLOT.x0 + 52.0, PLOT.y0 + 26.0, PLOT.x1 - 16.0, PLOT.y1 - 26.0);
const MODE_GROUP: Rect = Rect::new(892.0, PLOT.y0, 1172.0, PLOT.y1);
/// The LFO, and the dial groups.
const LFO: Rect = Rect::new(0.0, PLOT.y1 + 8.0, 740.0, HELP_TOP - 4.0);
const WAVE: Rect = Rect::new(LFO.x0 + 40.0, LFO.y0 + 30.0, LFO.x0 + 540.0, LFO.y1 - 52.0);
const MOTION: Rect = Rect::new(752.0, LFO.y0, 1172.0, LFO.y0 + 116.0);
const SOUND: Rect = Rect::new(752.0, LFO.y1 - 116.0, 1172.0, LFO.y1);
const DIAL_D: f64 = 46.0;
const PILL_H: f64 = 26.0;
/// The time view shows about this many LFO cycles, within these seconds.
const TIME_CYCLES: f64 = 3.0;
const TIME_SECONDS: (f64, f64) = (0.6, 7.5);
/// The response curves: two points a pixel (a comb's notches are dense at the top).
const CURVE_POINTS: usize = 1640;
const SWEEP_STEPS: usize = 24;
/// The sweep's shaded span needs less: a point every two pixels.
const SWEEP_POINTS: usize = CURVE_POINTS / 4;
/// The response view's dB range.
const RESP_DB: (f64, f64) = (-30.0, 18.0);

const WELLS: [f64; 2] = [1206.0, 1238.0];
const METER_TOP: f64 = DISPLAY_TOP + 20.0;
const METER_BOTTOM: f64 = DISPLAY_TOP + 592.0;
const METER_RANGE: f64 = 60.0;
const SILENT: f32 = -150.0;

fn freq_x(f: f64) -> f64 {
    PLOT_IN.x0 + (f.max(20.0) / 20.0).log10() / 3.0 * PLOT_IN.width()
}

fn x_freq(x: f64) -> f64 {
    20.0 * 1000f64.powf(((x - PLOT_IN.x0) / PLOT_IN.width()).clamp(0.0, 1.0))
}

fn db_y(db: f64) -> f64 {
    PLOT_IN.y1 - (db.clamp(RESP_DB.0, RESP_DB.1) - RESP_DB.0) / (RESP_DB.1 - RESP_DB.0) * PLOT_IN.height()
}

/// FEEDBACK's place on the response view: +95 % at the top, −95 % at the bottom.
fn feedback_y(fb: f64) -> f64 {
    PLOT_IN.y0 + (95.0 - fb.clamp(-95.0, 95.0)) / 190.0 * PLOT_IN.height()
}

fn y_feedback(y: f64) -> f64 {
    95.0 - (y - PLOT_IN.y0) / PLOT_IN.height() * 190.0
}

/// A stand-in host holding every Mite Mod parameter at its default (tests, preview, tools).
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

/// The engine's settings as a host's parameters set them.
pub fn settings(host: &dyn ParamHost) -> ModSettings {
    mod_settings(|id| host.plain(id, host.normalized(id)))
}

pub fn output(host: &dyn ParamHost) -> crate::dsp::Output {
    let p = |id: &str| host.plain(id, host.normalized(id));
    crate::dsp::Output { gain_db: p(OUT_GAIN.id), bypass: p(BYPASS.id) >= 0.5 }
}

/// A pill.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pill {
    Mode(Mode),
    Shape(Shape),
    Sync,
    Division,
    /// Chorus voices (1…4).
    Voices(usize),
    /// Phaser stages (index into `STAGE_COUNTS`).
    Stages(usize),
    ThruZero,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Hot {
    Pill(Pill),
    /// The response view's puck.
    Puck,
    /// The time view (a drag sets DELAY, or vibrato's DEPTH).
    Time,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Drag {
    Puck { start: Point, at: Point },
    Time { start: Point, value: f32 },
}

/// A dial and the modes it shows in.
struct Dial {
    knob: Knob,
    modes: &'static [Mode],
}

struct Group {
    name: &'static str,
    words: &'static str,
    rect: Rect,
}

const ALL: &[Mode] = &Mode::ALL;
const NOT_VIBRATO: &[Mode] = &[Mode::Chorus, Mode::Ensemble, Mode::Flanger, Mode::Phaser];

pub struct ModView {
    shared: Arc<ModShared>,
    session: Session,
    footer: Footer,
    dials: Vec<Dial>,
    groups: Vec<Group>,
    history: VecDeque<[f32; record::WIDTH]>,
    seen: usize,
    last_fresh: Instant,
    last_tick: Instant,
    pointer: Option<Point>,
    hot: Option<Hot>,
    drag: Option<Drag>,
    menu: Option<Dropdown>,
    meters: [f32; 3],
    /// The sweep's span on the response view, for the settings it was worked out for.
    sweep: Option<(ModSettings, Vec<(f64, f64)>)>,
}

impl ModView {
    pub fn new(shared: Arc<ModShared>) -> Self {
        let footer = Footer::new(
            FOOTER_TOP,
            (VIEW_W, VIEW_H),
            vec![FooterToggle { id: BYPASS.id, glyph: ToggleGlyph::Bypass, words: "Bypass — the input as it came (in time with the latency)" }],
            vec![FooterDial { spec: OUT_GAIN, kind: KnobKind::Bipolar, name: "OUTPUT", words: "Output gain" }],
        );
        let at = |r: Rect, i: usize, n: usize, dy: f64| Point::new(r.x0 + r.width() * (i as f64 + 0.5) / n as f64, r.y0 + dy);
        let dial = |spec: ParamSpec, label: &'static str, kind: KnobKind, p: Point, modes: &'static [Mode]| Dial { knob: Knob::new(spec, label, kind, p, DIAL_D), modes };
        let mg = MODE_GROUP;
        let mut dials = vec![
            dial(RATE, "RATE", KnobKind::Unipolar, at(MOTION, 0, 3, 64.0), ALL),
            dial(DEPTH, "DEPTH", KnobKind::Unipolar, at(MOTION, 1, 3, 64.0), ALL),
            dial(SPREAD, "SPREAD", KnobKind::Unipolar, at(MOTION, 2, 3, 64.0), ALL),
            dial(MIX, "MIX", KnobKind::Unipolar, at(SOUND, 0, 4, 64.0), NOT_VIBRATO),
            dial(WIDTH, "WIDTH", KnobKind::Unipolar, at(SOUND, 1, 4, 64.0), ALL),
            dial(LOW_CUT, "LOW CUT", KnobKind::Unipolar, at(SOUND, 2, 4, 64.0), ALL),
            dial(HIGH_CUT, "HIGH CUT", KnobKind::Unipolar, at(SOUND, 3, 4, 64.0), ALL),
            dial(DELAY, "DELAY", KnobKind::Unipolar, at(mg, 0, 2, 96.0), &[Mode::Chorus, Mode::Ensemble]),
            dial(MANUAL, "MANUAL", KnobKind::Unipolar, at(mg, 0, 2, 96.0), &[Mode::Flanger]),
            dial(CENTRE, "CENTRE", KnobKind::Unipolar, at(mg, 0, 2, 96.0), &[Mode::Phaser]),
            dial(FEEDBACK, "FEEDBACK", KnobKind::Bipolar, at(mg, 1, 2, 96.0), &[Mode::Flanger, Mode::Phaser]),
            dial(DEPTH, "DEPTH", KnobKind::Unipolar, at(mg, 0, 1, 96.0), &[Mode::Vibrato]),
        ];
        // Ensemble's one dial sits in the middle.
        dials[7].modes = &[Mode::Chorus];
        dials.push(dial(DELAY, "DELAY", KnobKind::Unipolar, at(mg, 0, 1, 96.0), &[Mode::Ensemble]));
        let groups = vec![
            Group { name: "MOTION", words: "how fast, how far, and how far apart the two sides move", rect: MOTION },
            Group { name: "SOUND", words: "how much of it, how wide, and the wet's lows and highs", rect: SOUND },
        ];
        Self {
            session: Session::new(SessionConfig { plugin: PLUGIN, factory: FACTORY, sound_ids: preset_ids(), snapshot_ids: preset_ids() }, shared.snapshots.clone(), shared.preset.clone()),
            footer,
            dials,
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
            sweep: None,
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

    pub fn mode(host: &dyn ParamHost) -> Mode {
        Mode::from_index(Self::plain(host, MODE.id).round().max(0.0) as usize)
    }

    fn fs(&self) -> f64 {
        f64::from(self.shared.sample_rate.load().max(8000.0))
    }

    fn tempo(&self) -> Option<f64> {
        let t = f64::from(self.shared.tempo.load());
        (t > 0.0).then_some(t)
    }

    pub fn pill(&self, pill: Pill) -> Rect {
        let row = |r: Rect, i: usize, n: usize, y: f64, pad: f64| {
            let w = (r.width() - 2.0 * pad - 6.0 * (n - 1) as f64) / n as f64;
            let x = r.x0 + pad + i as f64 * (w + 6.0);
            Rect::new(x, y, x + w, y + PILL_H)
        };
        match pill {
            Pill::Mode(mode) => {
                let x = 16.0 + mode.index() as f64 * (TILE_W + TILE_GAP);
                Rect::new(x, TILE_Y.0, x + TILE_W, TILE_Y.1)
            }
            Pill::Shape(shape) => {
                let (col, rowi) = (shape.index() % 2, shape.index() / 2);
                let x = LFO.x0 + 560.0 + col as f64 * 86.0;
                let y = LFO.y0 + 30.0 + rowi as f64 * 50.0;
                Rect::new(x, y, x + 80.0, y + 42.0)
            }
            Pill::Sync => Rect::new(WAVE.x0, LFO.y1 - 38.0, WAVE.x0 + 96.0, LFO.y1 - 38.0 + PILL_H),
            Pill::Division => Rect::new(WAVE.x0 + 104.0, LFO.y1 - 38.0, WAVE.x0 + 200.0, LFO.y1 - 38.0 + PILL_H),
            Pill::Voices(n) => row(MODE_GROUP, n - 1, 4, MODE_GROUP.y0 + 196.0, 20.0),
            Pill::Stages(i) => row(MODE_GROUP, i, STAGE_COUNTS.len(), MODE_GROUP.y0 + 196.0, 14.0),
            Pill::ThruZero => Rect::new(MODE_GROUP.x0 + 20.0, MODE_GROUP.y0 + 196.0, MODE_GROUP.x1 - 20.0, MODE_GROUP.y0 + 196.0 + PILL_H),
        }
    }

    /// The pills showing for `mode`.
    fn pills(&self, mode: Mode) -> Vec<Pill> {
        let mut v: Vec<Pill> = Mode::ALL.iter().map(|&m| Pill::Mode(m)).collect();
        v.extend(Shape::ALL.iter().map(|&s| Pill::Shape(s)));
        v.extend([Pill::Sync, Pill::Division]);
        match mode {
            Mode::Chorus => v.extend((1..=4).map(Pill::Voices)),
            Mode::Flanger => v.push(Pill::ThruZero),
            Mode::Phaser => v.extend((0..STAGE_COUNTS.len()).map(Pill::Stages)),
            _ => {}
        }
        v
    }

    fn shown(&self, mode: Mode) -> impl Iterator<Item = &Dial> {
        self.dials.iter().filter(move |d| d.modes.contains(&mode))
    }

    /// Where a dial sits now (in the mode the host is in).
    pub fn dial(&self, host: &dyn ParamHost, id: &str) -> Option<Point> {
        let mode = Self::mode(host);
        self.shown(mode).find(|d| d.knob.id == id).map(|d| d.knob.center)
    }

    pub fn dial_groups(&self) -> Vec<(&'static str, Rect)> {
        let mut g: Vec<(&'static str, Rect)> = self.groups.iter().map(|g| (g.name, g.rect)).collect();
        g.push(("MODE", MODE_GROUP));
        g
    }

    pub fn menu_rows(&self) -> Vec<Rect> {
        self.menu.as_ref().map(Dropdown::item_rects).unwrap_or_default()
    }

    /// The response view's puck: across MANUAL's first notch (flanger) or CENTRE (phaser), up and
    /// down FEEDBACK.
    pub fn puck(&self, host: &dyn ParamHost) -> Point {
        let fb = f64::from(Self::plain(host, FEEDBACK.id));
        let f = match Self::mode(host) {
            Mode::Phaser => f64::from(Self::plain(host, CENTRE.id)),
            _ => 500.0 / f64::from(Self::plain(host, MANUAL.id)).max(0.05),
        };
        Point::new(freq_x(f), feedback_y(fb))
    }

    pub fn plot(&self) -> Rect {
        PLOT
    }

    // ---- the LFO now -------------------------------------------------------------------------

    fn since_fresh_s(&self) -> f64 {
        self.last_fresh.elapsed().as_secs_f64().min(0.05)
    }

    /// The LFO's total phase now: the last record's, carried on at its rate since it came.
    fn phase_now(&self) -> f64 {
        self.history.back().map_or(0.0, |r| f64::from(r[record::CYCLE]) + f64::from(r[record::PHASE]) + f64::from(r[record::RATE]) * self.since_fresh_s())
    }

    // ---- drawing ----------------------------------------------------------------------------

    fn draw_header(&mut self, cx: &mut DrawCx<'_>) {
        let theme = cx.theme;
        cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.window, None, &Rect::new(0.0, 0.0, VIEW_W, HEADER));
        cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, theme.line, None, &Line::new((0.0, HEADER - 0.5), (VIEW_W, HEADER - 0.5)));
        draw_mark(cx, 16.0, 15.0, 26.0);
        draw_title(cx, 52.0, 18.0, "Mod");
        self.session.draw(cx);
    }

    /// A small picture of what a mode does, in `r`.
    fn mode_glyph(cx: &mut DrawCx<'_>, mode: Mode, r: Rect, ink: mite_ui::peniko::Color) {
        let n = 40;
        let mut paths: Vec<BezPath> = Vec::new();
        let x_at = |t: f64| r.x0 + t * r.width();
        let mid = r.center().y;
        let h = 0.42 * r.height();
        let wave = |phase: f64, cycles: f64, amp: f64, fm: f64| {
            let pts: Vec<Point> = (0..=n)
                .map(|i| {
                    let t = i as f64 / n as f64;
                    let ph = std::f64::consts::TAU * (cycles * t + phase + fm * (std::f64::consts::TAU * t).sin() * 0.08);
                    Point::new(x_at(t), mid - amp * h * ph.sin())
                })
                .collect();
            smooth_path(&pts)
        };
        match mode {
            Mode::Chorus => paths.extend([wave(0.0, 1.5, 0.9, 0.0), wave(0.12, 1.5, 0.9, 0.0)]),
            Mode::Ensemble => paths.extend([wave(0.0, 1.5, 0.9, 0.0), wave(0.1, 1.5, 0.75, 0.0), wave(0.2, 1.5, 0.6, 0.0)]),
            Mode::Vibrato => paths.push(wave(0.0, 2.5, 0.9, 3.0)),
            Mode::Flanger | Mode::Phaser => {
                // The comb (evenly spaced notches) or the phaser's few, wide ones.
                let pts: Vec<Point> = (0..=n * 2)
                    .map(|i| {
                        let t = i as f64 / (n * 2) as f64;
                        let v = if mode == Mode::Flanger { (std::f64::consts::PI * 5.0 * t).cos().abs().powf(0.6) } else { (std::f64::consts::PI * (1.0 + 3.0 * t * t)).cos().abs().powf(0.4) };
                        Point::new(x_at(t), r.y1 - v * r.height())
                    })
                    .collect();
                paths.push(smooth_path(&pts));
            }
        }
        for (k, p) in paths.iter().enumerate() {
            cx.scene.stroke(&Stroke::new(1.6), Affine::IDENTITY, ink.multiply_alpha(1.0 - 0.25 * k as f32), None, p);
        }
    }

    fn draw_tiles(&self, cx: &mut DrawCx<'_>, mode: Mode) {
        let theme = cx.theme;
        for &m in &Mode::ALL {
            let r = self.pill(Pill::Mode(m));
            let rr = RoundedRect::from_rect(r, 10.0);
            let on = m == mode;
            let hot = self.hot == Some(Hot::Pill(Pill::Mode(m)));
            let ink = if on { theme.accent } else if hot { theme.text_hi } else { theme.text_mid };
            cx.scene.fill(Fill::NonZero, Affine::IDENTITY, if on { theme.accent.multiply_alpha(0.14) } else { theme.well }, None, &rr);
            if on {
                glow_stroke(cx, &rr, 1.3, theme.accent, theme.accent, 3.0);
            } else {
                cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, if hot { theme.text_lo } else { theme.well_border }, None, &rr);
            }
            Self::mode_glyph(cx, m, Rect::new(r.x0 + 12.0, r.y0 + 12.0, r.x0 + 62.0, r.y1 - 12.0), ink);
            cx.text.draw(cx.scene, &Mode::NAMES[m.index()].to_uppercase(), r.x0 + 74.0, r.center().y - 6.0, TextStyle::LABEL.sized(11.0), ink, Align::Left);
        }
    }

    fn draw_plot(&mut self, cx: &mut DrawCx<'_>, s: &ModSettings) {
        let theme = cx.theme;
        cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.well, None, &PLOT);
        match s.mode {
            Mode::Flanger | Mode::Phaser => self.draw_response(cx, s),
            _ => self.draw_time(cx, s),
        }
        let _ = theme;
    }

    /// Chorus, ensemble: each voice's delay; vibrato: its pitch, scrolling.
    fn draw_time(&self, cx: &mut DrawCx<'_>, s: &ModSettings) {
        let theme = cx.theme;
        let accent = theme.accent;
        let label = TextStyle { tabular: true, ..TextStyle::BODY.sized(10.0).weighted(600.0) };
        // Delays: the voices' span with air around it (a third of it, 1 ms at least).
        let around = |lo: f32, hi: f32| {
            let pad = (0.35 * f64::from(hi - lo)).max(1.0);
            ((f64::from(lo) - pad).max(0.0), f64::from(hi) + pad)
        };
        let (lo, hi, unit, voices) = match s.mode {
            Mode::Chorus => {
                let top = (0..s.voices).map(|v| m::chorus_delay_ms(s, v, 1.0)).fold(0.0, f32::max);
                let bottom = (0..s.voices).map(|v| m::chorus_delay_ms(s, v, -1.0)).fold(f32::MAX, f32::min);
                let (a, b) = around(bottom, top);
                (a, b, "ms", s.voices)
            }
            Mode::Ensemble => {
                let (a, b) = around(m::ensemble_delay_ms(s, -1.0, -1.0), m::ensemble_delay_ms(s, 1.0, 1.0));
                (a, b, "ms", 3)
            }
            _ => {
                let rate = m::rate_hz(s, self.tempo());
                let amp = f64::from(m::vibrato_amp_ms(s, rate)) * 0.001;
                let cents = f64::from(m::cents_of_slope(-std::f64::consts::TAU * rate * amp));
                let reach = (cents * 1.3).max(10.0);
                (-reach, reach, "ct", 1)
            }
        };
        let y_of = |v: f64| PLOT_IN.y1 - (v - lo) / (hi - lo) * PLOT_IN.height();
        // Grid: a few values.
        let step = nice_step((hi - lo) / 4.0);
        let mut v = (lo / step).ceil() * step;
        while v <= hi {
            let y = y_of(v);
            cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, if v.abs() < 1e-9 { theme.grid_major } else { theme.grid_minor }, None, &Line::new((PLOT_IN.x0, y), (PLOT_IN.x1, y)));
            let places = if step < 1.0 { 1 } else { 0 };
            let t = if unit == "ct" && v > 0.0 { format!("+{v:.places$}") } else { format!("{v:.places$}") };
            cx.text.draw(cx.scene, &t, PLOT_IN.x0 - 8.0, y - 6.0, label, theme.text_lo, Align::Right);
            v += step;
        }
        cx.text.draw(cx.scene, if unit == "ms" { "DELAY · ms" } else { "PITCH · cents" }, PLOT.x0 + 12.0, PLOT.y0 + 8.0, TextStyle::LABEL.sized(9.0), theme.text_mid, Align::Left);
        // What the settings allow: each voice's band, shaded, and its middle dashed.
        for v in 0..voices {
            let (a, b) = match s.mode {
                Mode::Chorus => (m::chorus_delay_ms(s, v, -1.0), m::chorus_delay_ms(s, v, 1.0)),
                Mode::Ensemble => (m::ensemble_delay_ms(s, -1.0, -1.0), m::ensemble_delay_ms(s, 1.0, 1.0)),
                _ => {
                    let r = (hi / 1.3) as f32;
                    (-r, r)
                }
            };
            let band = Rect::new(PLOT_IN.x0, y_of(f64::from(b)), PLOT_IN.x1, y_of(f64::from(a)));
            cx.scene.fill(Fill::NonZero, Affine::IDENTITY, accent.multiply_alpha(0.06), None, &band);
            let mid = y_of(0.5 * f64::from(a + b));
            cx.scene.stroke(&Stroke::new(1.0).with_dashes(0.0, [4.0, 3.0]), Affine::IDENTITY, accent.multiply_alpha(0.35), None, &Line::new((PLOT_IN.x0, mid), (PLOT_IN.x1, mid)));
        }
        let seconds = (TIME_CYCLES / m::rate_hz(s, self.tempo())).clamp(TIME_SECONDS.0, TIME_SECONDS.1);
        let lane = HistoryLane { x0: PLOT_IN.x0, width: PLOT_IN.width(), top: PLOT_IN.y0, bottom: PLOT_IN.y1, seconds, slice_ms: HISTORY_SLICE_MS };
        let since = (self.last_fresh.elapsed().as_secs_f64() * 1000.0 / f64::from(HISTORY_SLICE_MS)).min(4.0);
        let cols = lane.columns(&self.history, self.seen, since);
        if cols.len() < 2 {
            return;
        }
        cx.begin_clip(&PLOT_IN.inflate(0.0, 4.0));
        let per_side = if s.mode == Mode::Chorus { 4 } else if s.mode == Mode::Ensemble { 3 } else { 1 };
        for side in [1, 0] {
            for v in 0..voices {
                let field = record::VALUES + v + per_side * side;
                let pts: Vec<Point> = cols.iter().map(|c| Point::new(c.x, y_of(f64::from(c.high[field])))).collect();
                let path = smooth_path(&pts);
                let alpha = [1.0, 0.75, 0.55, 0.4][v.min(3)];
                if side == 0 && v == 0 {
                    glow_stroke(cx, &path, 2.0, accent, accent, 2.5);
                } else if side == 0 {
                    cx.scene.stroke(&Stroke::new(1.6), Affine::IDENTITY, accent.multiply_alpha(alpha), None, &path);
                } else {
                    cx.scene.stroke(&Stroke::new(1.2).with_dashes(0.0, [3.0, 3.0]), Affine::IDENTITY, accent.multiply_alpha(0.45 * alpha), None, &path);
                }
            }
        }
        cx.end_clip();
    }

    /// The sweep's span: per frequency the lowest and highest the response reaches over a cycle.
    fn sweep(&mut self, s: &ModSettings, fs: f64) -> &[(f64, f64)] {
        let key = ModSettings { rate_hz: 0.0, sync: false, division: 0, ..*s };
        if self.sweep.as_ref().is_none_or(|(k, _)| *k != key) {
            let mut span = vec![(f64::MAX, f64::MIN); SWEEP_POINTS + 1];
            let tone = m::Tone::of(s, fs);
            for k in 0..SWEEP_STEPS {
                let lfo = -1.0 + 2.0 * k as f32 / (SWEEP_STEPS - 1) as f32;
                for (i, sp) in span.iter_mut().enumerate() {
                    let f = x_freq(PLOT_IN.x0 + PLOT_IN.width() * i as f64 / SWEEP_POINTS as f64);
                    let db = response(s, &tone, lfo_reading(s, lfo), f, fs);
                    *sp = (sp.0.min(db), sp.1.max(db));
                }
            }
            self.sweep = Some((key, span));
        }
        &self.sweep.as_ref().expect("sweep").1
    }

    /// Flanger, phaser: the response now (left solid, right dashed), the sweep's span shaded.
    fn draw_response(&mut self, cx: &mut DrawCx<'_>, s: &ModSettings) {
        let theme = cx.theme;
        let accent = theme.accent;
        let fs = self.fs();
        let label = TextStyle { tabular: true, ..TextStyle::BODY.sized(10.0).weighted(600.0) };
        for (f, t) in [(50.0, "50"), (100.0, "100"), (200.0, "200"), (500.0, "500"), (1000.0, "1k"), (2000.0, "2k"), (5000.0, "5k"), (10_000.0, "10k")] {
            let x = freq_x(f);
            cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, theme.grid_minor, None, &Line::new((x, PLOT_IN.y0), (x, PLOT_IN.y1)));
            cx.text.draw(cx.scene, t, x, PLOT_IN.y1 + 6.0, label, theme.text_lo, Align::Center);
        }
        for db in [-24.0, -12.0, 0.0, 12.0] {
            let y = db_y(db);
            cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, if db == 0.0 { theme.grid_major } else { theme.grid_minor }, None, &Line::new((PLOT_IN.x0, y), (PLOT_IN.x1, y)));
            cx.text.draw(cx.scene, &format!("{db:+.0}").replace("+0", "0"), PLOT_IN.x0 - 8.0, y - 6.0, label, theme.text_lo, Align::Right);
        }
        cx.text.draw(cx.scene, "RESPONSE · dB", PLOT.x0 + 12.0, PLOT.y0 + 8.0, TextStyle::LABEL.sized(9.0), theme.text_mid, Align::Left);
        let x_at = |i: usize| PLOT_IN.x0 + PLOT_IN.width() * i as f64 / CURVE_POINTS as f64;
        let span_x = |i: usize| PLOT_IN.x0 + PLOT_IN.width() * i as f64 / SWEEP_POINTS as f64;
        let span = self.sweep(s, fs).to_vec();
        let mut band = BezPath::new();
        for (i, &(_, hi)) in span.iter().enumerate() {
            let p = Point::new(span_x(i), db_y(hi));
            if i == 0 {
                band.move_to(p);
            } else {
                band.line_to(p);
            }
        }
        for (i, &(lo, _)) in span.iter().enumerate().rev() {
            band.line_to(Point::new(span_x(i), db_y(lo)));
        }
        band.close_path();
        cx.begin_clip(&PLOT);
        cx.scene.fill(Fill::NonZero, Affine::IDENTITY, accent.multiply_alpha(0.08), None, &band);
        let now = self.history.back();
        let tone = m::Tone::of(s, fs);
        for side in [1, 0] {
            let reading = now.map_or_else(|| lfo_reading(s, 0.0), |r| r[record::VALUES + side]);
            let reading = if reading == 0.0 && s.mode == Mode::Phaser { lfo_reading(s, 0.0) } else { reading };
            let pts: Vec<Point> = (0..=CURVE_POINTS).map(|i| Point::new(x_at(i), db_y(response(s, &tone, reading, x_freq(x_at(i)), fs)))).collect();
            let path = smooth_path(&pts);
            if side == 0 {
                glow_stroke(cx, &path, 2.0, accent, accent, 3.0);
            } else {
                cx.scene.stroke(&Stroke::new(1.3).with_dashes(0.0, [4.0, 3.0]), Affine::IDENTITY, accent.multiply_alpha(0.5), None, &path);
            }
        }
        cx.end_clip();
        // The puck, and where it anchors the sweep.
        let p = self.puck(cx.host);
        cx.scene.stroke(&Stroke::new(1.0).with_dashes(0.0, [3.0, 4.0]), Affine::IDENTITY, accent.multiply_alpha(0.4), None, &Line::new((p.x, PLOT_IN.y0), (p.x, PLOT_IN.y1)));
        let hot = matches!(self.hot, Some(Hot::Puck)) || matches!(self.drag, Some(Drag::Puck { .. }));
        let c = Circle::new(p, 9.0);
        cx.scene.fill(Fill::NonZero, Affine::IDENTITY, if hot { accent } else { theme.display }, None, &c);
        glow_stroke(cx, &c, 1.8, accent, accent, if hot { 5.0 } else { 2.5 });
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

    fn draw_mode_group(&self, cx: &mut DrawCx<'_>, s: &ModSettings) {
        let theme = cx.theme;
        panel(cx.scene, RoundedRect::from_rect(MODE_GROUP, 10.0), theme.well, theme.well_border);
        cx.text.draw(cx.scene, &Mode::NAMES[s.mode.index()].to_uppercase(), MODE_GROUP.x0 + 12.0, MODE_GROUP.y0 + 9.0, TextStyle::LABEL.sized(9.0), theme.text_mid, Align::Left);
        match s.mode {
            Mode::Chorus => {
                for n in 1..=4 {
                    self.draw_pill(cx, Pill::Voices(n), &n.to_string(), s.voices == n);
                }
                cx.text.draw(cx.scene, "VOICES", MODE_GROUP.center().x, MODE_GROUP.y0 + 230.0, TextStyle::LABEL, theme.text_lo, Align::Center);
            }
            Mode::Flanger => self.draw_pill(cx, Pill::ThruZero, "THROUGH ZERO", s.thru_zero),
            Mode::Phaser => {
                for (i, n) in STAGE_COUNTS.iter().enumerate() {
                    self.draw_pill(cx, Pill::Stages(i), &n.to_string(), s.stages == *n);
                }
                cx.text.draw(cx.scene, "STAGES", MODE_GROUP.center().x, MODE_GROUP.y0 + 230.0, TextStyle::LABEL, theme.text_lo, Align::Center);
            }
            _ => {}
        }
    }

    fn draw_lfo(&self, cx: &mut DrawCx<'_>, s: &ModSettings) {
        let theme = cx.theme;
        let accent = theme.accent;
        cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.well, None, &LFO);
        cx.text.draw(cx.scene, "LFO", LFO.x0 + 12.0, LFO.y0 + 8.0, TextStyle::LABEL.sized(9.0), theme.text_mid, Align::Left);
        let mid = WAVE.center().y;
        let h = 0.5 * WAVE.height();
        cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, theme.grid_major, None, &Line::new((WAVE.x0, mid), (WAVE.x1, mid)));
        for t in [0.25, 0.5, 0.75] {
            let x = WAVE.x0 + t * WAVE.width();
            cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, theme.grid_minor, None, &Line::new((x, WAVE.y0), (x, WAVE.y1)));
        }
        // One cycle: the one playing (Random's values are the cycle's own).
        let phase = self.phase_now();
        let cycle = phase.floor();
        let n = 160;
        let pts: Vec<Point> = (0..=n).map(|i| {
            let t = i as f64 / n as f64;
            Point::new(WAVE.x0 + t * WAVE.width(), mid - h * f64::from(m::shape_at(s.shape, cycle + t)))
        }).collect();
        glow_stroke(cx, &smooth_path(&pts), 2.0, accent, accent, 2.5);
        // Where each side is: the left a dot, the right a ring SPREAD ahead.
        let place = |p: f64| {
            let f = p.rem_euclid(1.0);
            Point::new(WAVE.x0 + f * WAVE.width(), mid - h * f64::from(m::shape_at(s.shape, p)))
        };
        let right = place(phase + 0.5 * f64::from(s.spread));
        cx.scene.stroke(&Stroke::new(1.8), Affine::IDENTITY, accent, None, &Circle::new(right, 6.5));
        let left = place(phase);
        cx.scene.fill(Fill::NonZero, Affine::IDENTITY, accent, None, &Circle::new(left, 6.0));
        cx.text.draw(cx.scene, "L", left.x, left.y - 22.0, TextStyle::LABEL.sized(9.0), theme.text_mid, Align::Center);
        cx.text.draw(cx.scene, "R", right.x, right.y - 22.0, TextStyle::LABEL.sized(9.0), theme.text_lo, Align::Center);
        // The shapes, each drawn by the same function.
        for &shape in &Shape::ALL {
            let r = self.pill(Pill::Shape(shape));
            let rr = RoundedRect::from_rect(r, 8.0);
            let on = shape == s.shape;
            let hot = self.hot == Some(Hot::Pill(Pill::Shape(shape)));
            cx.scene.fill(Fill::NonZero, Affine::IDENTITY, if on { accent.multiply_alpha(0.16) } else { theme.display }, None, &rr);
            if on {
                glow_stroke(cx, &rr, 1.2, accent, accent, 2.5);
            } else {
                cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, if hot { theme.text_lo } else { theme.well_border }, None, &rr);
            }
            let inner = r.inset(-12.0);
            // Random over a few cycles (one cycle is a single glide).
            let cycles = if shape == Shape::Random { 3.0 } else { 1.0 };
            let pts: Vec<Point> = (0..=48).map(|i| {
                let t = i as f64 / 48.0;
                Point::new(inner.x0 + t * inner.width(), inner.center().y - 0.5 * inner.height() * f64::from(m::shape_at(shape, 3.0 + cycles * t)))
            }).collect();
            cx.scene.stroke(&Stroke::new(1.6), Affine::IDENTITY, if on { accent } else if hot { theme.text_hi } else { theme.text_mid }, None, &smooth_path(&pts));
        }
        let div = DIVISIONS[s.division.min(DIVISIONS.len() - 1)].0;
        self.draw_pill(cx, Pill::Sync, "SYNC", s.sync);
        self.draw_pill(cx, Pill::Division, div, s.sync);
        // The rate it runs at (a cycle's length when synced).
        let rate = m::rate_hz(s, self.tempo());
        let words = if s.sync && self.tempo().is_none() {
            "waiting for the host's tempo".to_owned()
        } else if rate >= 1.0 {
            format!("{rate:.2} Hz")
        } else {
            format!("{:.1} s a cycle", 1.0 / rate)
        };
        cx.text.draw(cx.scene, &words, WAVE.x1, LFO.y1 - 32.0, TextStyle { tabular: true, ..TextStyle::BODY.sized(11.0) }, theme.text_lo, Align::Right);
    }

    fn draw_groups(&self, cx: &mut DrawCx<'_>, s: &ModSettings) {
        let theme = cx.theme;
        for g in &self.groups {
            panel(cx.scene, RoundedRect::from_rect(g.rect, 10.0), theme.well, theme.well_border);
            cx.text.draw(cx.scene, g.name, g.rect.x0 + 12.0, g.rect.y0 + 9.0, TextStyle::LABEL.sized(9.0), theme.text_mid, Align::Left);
        }
        for d in self.shown(s.mode) {
            if d.knob.id == RATE.id && s.sync {
                Self::draw_plaque(cx, &d.knob, DIVISIONS[s.division.min(DIVISIONS.len() - 1)].0);
            } else {
                d.knob.draw(cx);
            }
        }
        if s.mode == Mode::Vibrato {
            // Where MIX would be: vibrato is all wet.
            let k = Knob::new(MIX, "MIX", KnobKind::Unipolar, Point::new(SOUND.x0 + SOUND.width() * 0.125, SOUND.y0 + 64.0), DIAL_D);
            Self::draw_plaque(cx, &k, "WET");
        }
    }

    /// Where a dial would be, a word in a dashed ring (a synced rate, vibrato's mix).
    fn draw_plaque(cx: &mut DrawCx<'_>, k: &Knob, text: &str) {
        let theme = cx.theme;
        let c = k.center;
        let r = 0.5 * k.diameter;
        cx.scene.stroke(&Stroke::new(1.2).with_dashes(0.0, [3.0, 3.0]), Affine::IDENTITY, theme.accent.multiply_alpha(0.55), None, &Circle::new(c, r));
        cx.text.draw(cx.scene, text, c.x, c.y - 6.0, TextStyle::LABEL.sized(10.0).weighted(700.0), theme.accent, Align::Center);
        cx.text.draw(cx.scene, k.label, c.x, c.y + r + 4.0, TextStyle::LABEL, theme.text_lo, Align::Center);
    }

    fn draw_meters(&self, cx: &mut DrawCx<'_>) {
        let theme = cx.theme;
        cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, theme.line, None, &Line::new((1180.5, DISPLAY_TOP), (1180.5, HELP_TOP)));
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

    fn help_text(&self, host: &dyn ParamHost) -> Help {
        let s = settings(host);
        if let Some(menu) = &self.menu {
            let i = self.pointer.and_then(|p| menu.item_rects().iter().position(|r| r.contains(p)));
            return match i {
                Some(i) => Help::default().text(format!("{} — one LFO cycle this long", DIVISIONS[i].0)),
                None => Help::default().text("Pick a division; click elsewhere to close."),
            };
        }
        let hot = self.drag.map(|d| match d {
            Drag::Puck { .. } => Hot::Puck,
            Drag::Time { .. } => Hot::Time,
        }).or(self.hot);
        match hot {
            Some(Hot::Pill(Pill::Mode(mode))) => {
                return Help::default().text(match mode {
                    Mode::Chorus => "Chorus — up to four voices, each a little behind and swaying: a wider, thicker sound",
                    Mode::Ensemble => "Ensemble — three voices swayed slowly and shimmered fast, a third of a cycle apart: the string-machine sound",
                    Mode::Flanger => "Flanger — a short delay swept against the dry: a comb of notches moving (through zero: the wet passes the dry, the tape flange)",
                    Mode::Phaser => "Phaser — 2 to 24 allpass stages swept: a few wide notches moving",
                    Mode::Vibrato => "Vibrato — the pitch swayed, nothing of the dry left",
                });
            }
            Some(Hot::Pill(Pill::Shape(shape))) => return Help::default().text(format!("{} — the LFO's shape (every one smooth: a delay swept by a step would click)", Shape::NAMES[shape.index()])),
            Some(Hot::Pill(Pill::Sync)) => return Help::default().text("Sync — the LFO locked to the song: one cycle per division, in step with the bar"),
            Some(Hot::Pill(Pill::Division)) => return Help::default().text("Division — one LFO cycle this long when synced; click to pick, wheel to step"),
            Some(Hot::Pill(Pill::Voices(n))) => return Help::default().text(format!("{n} voice{} — each its own delay and place in the cycle", if n > 1 { "s" } else { "" })),
            Some(Hot::Pill(Pill::Stages(i))) => return Help::default().text(format!("{} stages — {} notches; more stages, more notches", STAGE_COUNTS[i], STAGE_COUNTS[i] / 2)),
            Some(Hot::Pill(Pill::ThruZero)) => return Help::default().text("Through zero — the dry waits (5 ms of latency, reported) so the wet can catch it and pass it: at the crossing everything cancels or doubles, the tape flange"),
            Some(Hot::Puck) => {
                let fb = Self::plain(host, FEEDBACK.id);
                let (what, value, widest) = if s.mode == Mode::Phaser { ("Centre", CENTRE.format(s.centre_hz), "8.00 kHz") } else { ("Manual", MANUAL.format(s.manual_ms), "5.00 ms") };
                return Help::default().text(format!("{what} ")).slot(value, widest).text(" · feedback ").slot(FEEDBACK.format(fb), "−95 %").text(" — drag: across where it sweeps, up and down how hard it rings");
            }
            Some(Hot::Time) => {
                return if s.mode == Mode::Vibrato {
                    Help::default().text("Depth ").slot(DEPTH.format(Self::plain(host, DEPTH.id)), "100 %").text(" — drag up and down: how far the pitch sways (the band)")
                } else {
                    Help::default().text("Delay ").slot(DELAY.format(s.delay_ms), "40.0 ms").text(" — drag up and down: how far behind the voices sit (the bands: how far they sway)")
                };
            }
            None => {}
        }
        if let Some(h) = self.footer.help(host) {
            return h;
        }
        if let Some(d) = self.shown(s.mode).find(|d| self.pointer.is_some_and(|p| d.knob.bounds().contains(p))) {
            let k = &d.knob;
            if k.id == RATE.id && s.sync {
                return Help::default().text("Rate — synced: set by the division (SYNC off to set it freely)");
            }
            let v = Self::plain(host, &k.id);
            return Help::default().text(format!("{} ", k.spec.name)).slot(k.spec.format(v), k.spec.format(k.spec.max)).text(knob_words(k.spec.id, s.mode));
        }
        if let Some(g) = self.groups.iter().find(|g| self.pointer.is_some_and(|p| g.rect.contains(p))) {
            return Help::default().text(format!("{} — {}", g.name, g.words));
        }
        if let Some(h) = self.session.help() {
            return h;
        }
        if self.pointer.is_some_and(|p| LFO.contains(p)) {
            return Help::default().text("The LFO: its shape over one cycle; the dot is the left side now, the ring the right (SPREAD ahead).");
        }
        Help::default().text("Pick a mode on top; its view and controls follow. The LFO below moves everything.")
    }

    // ---- events -----------------------------------------------------------------------------

    fn busy(&self) -> bool {
        self.drag.is_some() || self.menu.is_some() || self.dials.iter().any(|d| d.knob.is_dragging()) || self.footer.busy()
    }

    fn hot_at(&self, host: &dyn ParamHost, p: Point) -> Option<Hot> {
        let mode = Self::mode(host);
        for pill in self.pills(mode) {
            if self.pill(pill).inflate(2.0, 2.0).contains(p) {
                return Some(Hot::Pill(pill));
            }
        }
        if PLOT.contains(p) {
            return match mode {
                Mode::Flanger | Mode::Phaser => (self.puck(host).distance(p) <= 16.0).then_some(Hot::Puck),
                _ => PLOT_IN.contains(p).then_some(Hot::Time),
            };
        }
        None
    }

    fn handle_event(&mut self, event: &UiEvent, host: &dyn ParamHost) -> bool {
        if let Some(menu) = &mut self.menu {
            if let UiEvent::PointerMove { pos, .. } = *event {
                self.pointer = Some(pos);
            }
            match menu.event(event) {
                DropdownEvent::Open => {}
                DropdownEvent::Closed => self.menu = None,
                DropdownEvent::Picked(i) => {
                    self.menu = None;
                    Self::gesture(host, DIVISION.id, i as f32);
                    Self::gesture(host, SYNC.id, 1.0);
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
        let mode = Self::mode(host);
        let sync = Self::plain(host, SYNC.id) >= 0.5;
        for d in self.dials.iter_mut().filter(|d| d.modes.contains(&mode)) {
            if d.knob.id == RATE.id && sync {
                continue;
            }
            if d.knob.event(event, host) {
                return true;
            }
        }
        match *event {
            UiEvent::PointerMove { pos, mods } => {
                self.pointer = Some(pos);
                let fine = if mods.fine() { 0.2 } else { 1.0 };
                match self.drag {
                    Some(Drag::Puck { start, at }) => {
                        let to = at + (pos - start) * fine;
                        let f = x_freq(to.x.clamp(PLOT_IN.x0, PLOT_IN.x1));
                        if mode == Mode::Phaser {
                            host.set_normalized(CENTRE.id, host.normalize(CENTRE.id, (f as f32).clamp(CENTRE.min, CENTRE.max)));
                        } else {
                            host.set_normalized(MANUAL.id, host.normalize(MANUAL.id, MANUAL.snap((500.0 / f as f32).clamp(MANUAL.min, MANUAL.max))));
                        }
                        host.set_normalized(FEEDBACK.id, host.normalize(FEEDBACK.id, FEEDBACK.snap(y_feedback(to.y) as f32)));
                        return true;
                    }
                    Some(Drag::Time { start, value }) => {
                        // Up: more (a log spec moves by its position, so DELAY feels even).
                        let d = (start.y - pos.y) / PLOT_IN.height() * fine;
                        let id = if mode == Mode::Vibrato { DEPTH.id } else { DELAY.id };
                        let n = (host.normalize(id, value) + d as f32).clamp(0.0, 1.0);
                        host.set_normalized(id, n);
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
            UiEvent::PointerDown { pos, button: MouseButton::Left, .. } => match self.hot_at(host, pos) {
                Some(Hot::Pill(pill)) => {
                    match pill {
                        Pill::Mode(m) => Self::gesture(host, MODE.id, m.index() as f32),
                        Pill::Shape(s) => Self::gesture(host, SHAPE.id, s.index() as f32),
                        Pill::Sync => Self::gesture(host, SYNC.id, if sync { 0.0 } else { 1.0 }),
                        Pill::Division => {
                            let cur = Self::plain(host, DIVISION.id).round().max(0.0) as usize;
                            static NAMES: std::sync::OnceLock<Vec<&'static str>> = std::sync::OnceLock::new();
                            let names: &'static [&'static str] = NAMES.get_or_init(|| DIVISIONS.iter().map(|d| d.0).collect());
                            self.menu = Some(Dropdown::new(self.pill(Pill::Division), names, cur, (DISPLAY_TOP, HELP_TOP)).with_min_width(120.0));
                        }
                        Pill::Voices(n) => Self::gesture(host, VOICES.id, (n - 1) as f32),
                        Pill::Stages(i) => Self::gesture(host, STAGES.id, i as f32),
                        Pill::ThruZero => {
                            let on = Self::plain(host, THRU_ZERO.id) >= 0.5;
                            Self::gesture(host, THRU_ZERO.id, if on { 0.0 } else { 1.0 });
                        }
                    }
                    true
                }
                Some(Hot::Puck) => {
                    let id = if mode == Mode::Phaser { CENTRE.id } else { MANUAL.id };
                    host.begin_gesture(id);
                    host.begin_gesture(FEEDBACK.id);
                    self.drag = Some(Drag::Puck { start: pos, at: self.puck(host) });
                    true
                }
                Some(Hot::Time) => {
                    let id = if mode == Mode::Vibrato { DEPTH.id } else { DELAY.id };
                    host.begin_gesture(id);
                    self.drag = Some(Drag::Time { start: pos, value: Self::plain(host, id) });
                    true
                }
                None => false,
            },
            UiEvent::PointerUp { button: MouseButton::Left, .. } => match self.drag.take() {
                Some(Drag::Puck { .. }) => {
                    host.end_gesture(if mode == Mode::Phaser { CENTRE.id } else { MANUAL.id });
                    host.end_gesture(FEEDBACK.id);
                    true
                }
                Some(Drag::Time { .. }) => {
                    host.end_gesture(if mode == Mode::Vibrato { DEPTH.id } else { DELAY.id });
                    true
                }
                None => false,
            },
            UiEvent::DoubleClick { pos, .. } => match self.hot_at(host, pos) {
                Some(Hot::Puck) => {
                    self.drag = None;
                    let id = if mode == Mode::Phaser { CENTRE } else { MANUAL };
                    Self::gesture(host, id.id, id.default);
                    Self::gesture(host, FEEDBACK.id, FEEDBACK.default);
                    true
                }
                Some(Hot::Time) => {
                    self.drag = None;
                    let spec = if mode == Mode::Vibrato { DEPTH } else { DELAY };
                    Self::gesture(host, spec.id, spec.default);
                    true
                }
                _ => false,
            },
            UiEvent::Wheel { pos, lines, mods } => match self.hot_at(host, pos) {
                Some(Hot::Pill(Pill::Division)) => {
                    let cur = Self::plain(host, DIVISION.id).round() as i32;
                    let next = (cur - lines.signum() as i32).clamp(0, DIVISIONS.len() as i32 - 1);
                    Self::set(host, DIVISION.id, next as f32);
                    true
                }
                Some(Hot::Puck) => {
                    Self::set(host, FEEDBACK.id, FEEDBACK.wheel(Self::plain(host, FEEDBACK.id), lines, mods.fine()));
                    true
                }
                Some(Hot::Time) => {
                    Self::set(host, DEPTH.id, DEPTH.wheel(Self::plain(host, DEPTH.id), lines, mods.fine()));
                    true
                }
                _ => false,
            },
            _ => false,
        }
    }
}

/// The mode's reading for an LFO value: the flanger's delay (ms) or the phaser's corner (Hz).
fn lfo_reading(s: &ModSettings, lfo: f32) -> f32 {
    if s.mode == Mode::Phaser { m::phaser_fc(s, lfo) } else { m::flanger_tau_ms(s, lfo) }
}

fn response(s: &ModSettings, tone: &m::Tone, reading: f32, f: f64, fs: f64) -> f64 {
    if s.mode == Mode::Phaser { m::phaser_response_with(s, tone, reading, f, fs) } else { m::flanger_response_with(s, tone, reading, f, fs) }
}

/// A grid step of 1, 2 or 5 × a power of ten near `raw`.
fn nice_step(raw: f64) -> f64 {
    let p = 10f64.powf(raw.max(1e-6).log10().floor());
    let r = raw / p;
    p * if r < 1.5 { 1.0 } else if r < 3.5 { 2.0 } else if r < 7.5 { 5.0 } else { 10.0 }
}

fn knob_words(id: &str, mode: Mode) -> &'static str {
    match id {
        "rate" => " — how fast the LFO moves",
        "depth" => match mode {
            Mode::Chorus => " — how far each voice sways (up to half its delay, 6 ms at most)",
            Mode::Ensemble => " — how far the voices sway and shimmer",
            Mode::Flanger => " — how far the comb sweeps (through zero: from 50 % the wet passes the dry)",
            Mode::Phaser => " — how far the notches sweep (±3 octaves at 100 %)",
            Mode::Vibrato => " — how far the pitch sways (100 %: a semitone at the sine's steepest)",
        },
        "spread" => " — how far the right side's LFO runs ahead of the left (180°: opposite, the widest)",
        "feedback" => " — how hard it rings (negative: the other notches)",
        "delay" => " — how far behind the voices sit",
        "manual" => " — where the comb sits (its first notch at 1 / (2 × this))",
        "centre" => " — where the notches sit",
        "mix" => " — how much of the effect against the dry (50 %: the deepest notches)",
        "width" => " — the effect's stereo width (0: mono, 200 %: wider than wide)",
        "low_cut" => " — the effect's lows taken away under this",
        "high_cut" => " — the effect's highs taken away over this",
        _ => "",
    }
}

impl View for ModView {
    fn logical_size(&self) -> (f64, f64) {
        (VIEW_W, VIEW_H)
    }

    fn draw(&mut self, cx: &mut DrawCx<'_>) {
        let theme = cx.theme;
        let s = settings(cx.host);
        self.draw_header(cx);
        cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.display, None, &Rect::new(0.0, DISPLAY_TOP, VIEW_W, HELP_TOP));
        let hole = self.menu.as_ref().map(|m| RoundedRect::from_rect(m.bounds(), 8.0));
        if let Some(h) = hole {
            cx.begin_glow_hole(h);
        }
        self.draw_tiles(cx, s.mode);
        self.draw_plot(cx, &s);
        self.draw_mode_group(cx, &s);
        self.draw_lfo(cx, &s);
        self.draw_groups(cx, &s);
        self.draw_meters(cx);
        self.help_text(cx.host).draw(cx, Rect::new(0.0, HELP_TOP, VIEW_W, FOOTER_TOP));
        self.footer.draw_frame(cx);
        self.footer.draw_controls(cx);
        if hole.is_some() {
            cx.end_glow_hole();
        }
        if let Some(menu) = &self.menu {
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
        }
        // The LFO moves while audio runs (and a beat after).
        let running = self.last_fresh.elapsed().as_secs_f32() < 0.25 && !self.history.is_empty();
        fresh || busy || running || self.meters.iter().any(|&m| m > SILENT + 1.0)
    }
}
