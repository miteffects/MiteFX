//! The Mite Delay editor (docs/design/delay-brief.md §2), in 1280×760 design units.
//!
//! On top the echo plot: time across (from the input at the left), each tap's echoes as stems
//! from the middle line, the left side up and the right side down, as loud as they come out
//! (`mite_dsp::delay::echoes` at 1 kHz: the level, the loop's EQ and losses, the feedback, the
//! pan, ping-pong). A tap's first echo is its grip (across: its time, snapped to notes while it
//! is synced; up and down: its level); its second echo is a smaller grip (up and down: the
//! feedback). Under it, left, the loop EQ over the input's and the echoes' spectra (Mite EQ's
//! bands, none by default, every echo passes them once more). Right, the dials in their groups:
//! the machine, the modulation, the space, the ducking and the tap last touched.

mod spectrum;

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Instant;

use mite_core::{ParamDisplay, ParamScale, ParamSpec};
use mite_dsp::delay::{echoes, loop_gain, DelaySettings, Mode, LOOP_BANDS, MAX_TIME_MS, MIN_TIME_MS, TAPS};
use mite_dsp::eq::{analog_response_db, BandType, SLOPES_DB};
use mite_ui::kurbo::{Affine, BezPath, Circle, Line, Point, Rect, RoundedRect, Stroke};
use mite_ui::paint::{glow_stroke, panel};
use mite_ui::peniko::{Color, Fill};
use mite_ui::session::{Session, SessionConfig};
use mite_ui::widgets::brand::{draw_mark, draw_title};
use mite_ui::widgets::eq_icons::draw_type_icon;
use mite_ui::widgets::history;
use mite_ui::widgets::{Dropdown, DropdownEvent, Footer, FooterDial, FooterToggle, Help, Knob, KnobKind, ToggleGlyph};
use mite_ui::{write_if_changed, Align, DrawCx, MouseButton, ParamHost, RecordingHost, TextStyle, UiEvent, View};

use crate::dsp::{record, HISTORY_RECORDS};
use crate::params::*;
use crate::presets::{preset_ids, FACTORY, PLUGIN};
use crate::DelayShared;
use spectrum::{freq_x, Spectrum};

/// Mite Delay wears the Delay accent (tokens.json `color.accent.delay`, orchid tint).
pub const ACCENT: &str = "delay";

const VIEW_W: f64 = 1280.0;
const VIEW_H: f64 = 760.0;
const HEADER: f64 = 56.0;
const DISPLAY_TOP: f64 = HEADER;
const HELP_TOP: f64 = DISPLAY_TOP + 620.0;
const FOOTER_TOP: f64 = HELP_TOP + 28.0;

/// The echo plot: time across from `TIME_X0`, the levels mirrored about `ECHO_MID`.
const ECHO: Rect = Rect::new(0.0, DISPLAY_TOP, 1180.0, DISPLAY_TOP + 300.0);
const TIME_X0: f64 = 28.0;
const TIME_W: f64 = 1124.0;
const ECHO_TOP: f64 = ECHO.y0 + 16.0;
const ECHO_BOTTOM: f64 = ECHO.y1 - 24.0;
const ECHO_MID: f64 = 0.5 * (ECHO_TOP + ECHO_BOTTOM);
/// The levels drawn: from the middle line (this, dB) to the edges (+6 dB).
const FLOOR_DB: f64 = -48.0;
const CEIL_DB: f64 = 6.0;
/// The time spans the plot can show (s): the smallest that holds two echoes of the longest tap.
const WINDOWS: [f64; 7] = [0.25, 0.5, 1.0, 2.0, 4.0, 8.0, 16.0];
const MAX_ECHOES: usize = 64;
const GRIP_R: f64 = 8.0;
const FB_R: f64 = 5.5;
const GRIP_HIT: f64 = 14.0;

/// The loop EQ's plot under it, left.
const EQ: Rect = Rect::new(0.0, ECHO.y1 + 8.0, 740.0, HELP_TOP - 4.0);
pub(crate) const PLOT_X0: f64 = 28.0;
pub(crate) const PLOT_W: f64 = 696.0;
const EQ_TOP: f64 = EQ.y0 + 18.0;
const EQ_BOTTOM: f64 = EQ.y1 - 20.0;
const EQ_DB: f64 = 24.0;
const CURVE_POINTS: usize = 200;

/// The dial groups beside it: three rows, a wide and a narrow panel in each but the last.
const DIAL_D: f64 = 44.0;
const DIAL_DY: f64 = 52.0;
const GROUP_COLS: [(f64, f64); 2] = [(752.0, 1062.0), (1070.0, 1172.0)];
const GROUP_ROWS: [(f64, f64); 3] = [(EQ.y0, EQ.y0 + 100.0), (EQ.y0 + 104.0, EQ.y0 + 204.0), (EQ.y0 + 208.0, EQ.y1)];
const PILL_H: f64 = 22.0;

const WELLS: [f64; 2] = [1206.0, 1238.0];
const METER_TOP: f64 = DISPLAY_TOP + 20.0;
const METER_BOTTOM: f64 = DISPLAY_TOP + 592.0;
const METER_RANGE: f64 = 60.0;
const METER_RELEASE_DB_S: f32 = 20.0;
const SILENT: f32 = -150.0;

/// The loop band menu: every shape, then Remove.
const BAND_MENU: [&str; 11] = ["Bell", "Low Shelf", "High Shelf", "Low Cut", "High Cut", "Notch", "Band Pass", "Tilt Shelf", "Flat Tilt", "All Pass", "Remove"];
/// The SYNC pill's menu: every note, then free time.
const NOTE_MENU: [&str; 21] = ["1/64", "1/32T", "1/32", "1/16T", "1/32.", "1/16", "1/8T", "1/16.", "1/8", "1/4T", "1/8.", "1/4", "1/2T", "1/4.", "1/2", "1/1T", "1/2.", "1/1", "1/1.", "2/1", "Free time (ms)"];
/// A tap's menu, by (synced, ping-pong): its time's kind, its sides, Remove.
const TAP_MENUS: [[&[&str]; 2]; 2] = [[&["Sync to tempo", "Ping-pong", "Remove"], &["Sync to tempo", "Straight (not ping-pong)", "Remove"]], [&["Free time", "Ping-pong", "Remove"], &["Free time", "Straight (not ping-pong)", "Remove"]]];

fn time_x(window: f64, s: f64) -> f64 {
    TIME_X0 + TIME_W * s / window
}

fn x_time(window: f64, x: f64) -> f64 {
    (x - TIME_X0) / TIME_W * window
}

/// A level's distance from the middle line, as a share of the half height.
fn level_k(db: f64) -> f64 {
    ((db - FLOOR_DB) / (CEIL_DB - FLOOR_DB)).clamp(0.0, 1.0)
}

/// A level's y above the middle line (the left side's half; a grip's).
fn level_y(db: f64) -> f64 {
    ECHO_MID - level_k(db) * (ECHO_MID - ECHO_TOP)
}

/// The level at `y` (either half: the distance from the middle line).
fn y_level(y: f64) -> f64 {
    let k = ((y - ECHO_MID).abs() / (ECHO_MID - ECHO_TOP)).clamp(0.0, 1.0);
    FLOOR_DB + k * (CEIL_DB - FLOOR_DB)
}

fn x_freq(x: f64) -> f64 {
    20.0 * 1000f64.powf((x - PLOT_X0) / PLOT_W)
}

fn eq_y(db: f64) -> f64 {
    let mid = 0.5 * (EQ_TOP + EQ_BOTTOM);
    mid - db.clamp(-EQ_DB, EQ_DB) / EQ_DB * 0.5 * (EQ_BOTTOM - EQ_TOP)
}

fn y_eq(y: f64) -> f64 {
    let mid = 0.5 * (EQ_TOP + EQ_BOTTOM);
    (mid - y) / (0.5 * (EQ_BOTTOM - EQ_TOP)) * EQ_DB
}

fn fmt_ms(ms: f64) -> String {
    if ms >= 1000.0 {
        format!("{:.2} s", ms / 1000.0)
    } else if ms >= 100.0 {
        format!("{ms:.0} ms")
    } else {
        format!("{ms:.1} ms")
    }
}

fn fmt_hz(hz: f64) -> String {
    if hz >= 1000.0 {
        format!("{:.2} kHz", hz / 1000.0)
    } else {
        format!("{hz:.0} Hz")
    }
}

fn fmt_db(v: f64) -> String {
    format!("{}{:.1} dB", if v < -0.05 { "−" } else if v > 0.05 { "+" } else { "" }, v.abs())
}

/// A stand-in host holding every Mite Delay parameter at its default (tests, preview, tools).
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

/// The echoes' settings as a host's parameters set them at `tempo` (as `DelayParams::settings`).
pub fn settings(host: &dyn ParamHost, tempo: f32) -> DelaySettings {
    delay_settings(|id| host.plain(id, host.normalized(id)), tempo)
}

/// The output stage as a host's parameters set it.
pub fn output(host: &dyn ParamHost) -> crate::dsp::Output {
    let p = |id: &str| host.plain(id, host.normalized(id));
    crate::dsp::Output { mix: p(MIX.id) / 100.0, gain_db: p(OUT_GAIN.id), bypass: p(BYPASS.id) >= 0.5 }
}

/// A grip.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Node {
    /// A tap's first echo (from 0), while it is on.
    Tap(usize),
    /// A tap's second echo: its feedback.
    Feedback(usize),
    /// A loop EQ band (from 0), while it is on.
    Band(usize),
}

/// A pill: a machine (Digital, Tape, BBD), or the selected tap's sync or ping-pong.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pill {
    Mode(Mode),
    Sync,
    PingPong,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Drag {
    /// A grip: where it was grabbed and where it sat.
    Node { node: Node, start: Point, at: Point },
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Hot {
    Node(Node),
    Pill(Pill),
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum MenuFor {
    Tap(usize),
    Band(usize),
    /// A new loop band of the picked shape where the plot was right-clicked.
    NewBand(Point),
    /// The selected tap's note (or free time).
    Note(usize),
}

/// A panel of dials that work together.
struct Group {
    name: &'static str,
    words: &'static str,
    rect: Rect,
}

pub struct DelayView {
    shared: Arc<DelayShared>,
    session: Session,
    footer: Footer,
    knobs: Vec<Knob>,
    /// The selected tap's dials (rebuilt when another tap is picked).
    tap_knobs: Vec<Knob>,
    groups: Vec<Group>,
    spectrum: Spectrum,
    /// The tap the TAP panel shows (the last one touched).
    selected: usize,
    /// The time span shown (s); held while a tap is dragged.
    window: f64,
    curve: Vec<f64>,
    machine: Vec<f64>,
    curve_for: Option<(DelaySettings, f32, usize)>,
    pointer: Option<Point>,
    hot: Option<Hot>,
    drag: Option<Drag>,
    menu: Option<(MenuFor, Dropdown)>,
    meters: [f32; 4],
    history: VecDeque<[f32; record::WIDTH]>,
    seen: usize,
    last_tick: Instant,
}

impl DelayView {
    pub fn new(shared: Arc<DelayShared>) -> Self {
        let footer = Footer::new(
            FOOTER_TOP,
            (VIEW_W, VIEW_H),
            vec![FooterToggle { id: BYPASS.id, glyph: ToggleGlyph::Bypass, words: "Bypass — the input alone" }],
            vec![
                FooterDial { spec: MIX, kind: KnobKind::Unipolar, name: "MIX", words: "Mix — the echoes against the dry signal (100 %: the echoes alone)" },
                FooterDial { spec: OUT_GAIN, kind: KnobKind::Bipolar, name: "OUTPUT", words: "Output gain" },
            ],
        );
        // (name, words, row, col, dials, where the dials start across the panel (0…1)).
        type Dial = (ParamSpec, &'static str, KnobKind);
        type Layout = (&'static str, &'static str, usize, usize, Vec<Dial>, f64);
        let layout: [Layout; 4] = [
            ("MACHINE", "what the echoes go through each pass", 0, 0, vec![(DRIVE, "DRIVE", KnobKind::Unipolar)], 0.72),
            ("MOD", "the line's drift (wow, flutter, chorus by machine)", 0, 1, vec![(MOD_DEPTH, "DEPTH", KnobKind::Unipolar), (MOD_RATE, "RATE", KnobKind::Unipolar)], 0.0),
            ("DUCK", "the echoes stepping back while the input plays", 1, 0, vec![(DUCK, "DEPTH", KnobKind::Unipolar), (DUCK_THRESH, "THRESH", KnobKind::Unipolar), (DUCK_RELEASE, "RELEASE", KnobKind::Unipolar)], 0.0),
            ("SPACE", "the echoes' blur and spread", 1, 1, vec![(DIFFUSION, "DIFFUSE", KnobKind::Unipolar), (WIDTH, "WIDTH", KnobKind::Unipolar)], 0.0),
        ];
        let mut knobs = Vec::new();
        let mut groups = Vec::new();
        for (name, words, row, col, dials, from) in layout {
            let rect = Rect::new(GROUP_COLS[col].0, GROUP_ROWS[row].0, GROUP_COLS[col].1, GROUP_ROWS[row].1);
            let n = dials.len() as f64;
            let x0 = rect.x0 + rect.width() * from;
            let w = rect.x1 - x0;
            for (i, (spec, label, kind)) in dials.into_iter().enumerate() {
                let at = Point::new(x0 + w * (i as f64 + 0.5) / n, rect.y0 + DIAL_DY);
                knobs.push(Knob::new(spec, label, kind, at, DIAL_D));
            }
            groups.push(Group { name, words, rect });
        }
        groups.push(Group { name: "TAP", words: "the tap last touched", rect: Self::tap_panel() });
        let mut view = Self {
            session: Session::new(SessionConfig { plugin: PLUGIN, factory: FACTORY, sound_ids: preset_ids(), snapshot_ids: preset_ids() }, shared.snapshots.clone(), shared.preset.clone()),
            footer,
            knobs,
            tap_knobs: Vec::new(),
            groups,
            spectrum: Spectrum::new(),
            selected: 0,
            window: 1.0,
            curve: vec![0.0; CURVE_POINTS],
            machine: vec![0.0; CURVE_POINTS],
            curve_for: None,
            pointer: None,
            hot: None,
            drag: None,
            menu: None,
            meters: [SILENT; 4],
            history: VecDeque::with_capacity(HISTORY_RECORDS),
            seen: 0,
            last_tick: Instant::now(),
            shared,
        };
        view.select(0);
        view
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

    fn tap_panel() -> Rect {
        Rect::new(GROUP_COLS[0].0, GROUP_ROWS[2].0, GROUP_COLS[1].1, GROUP_ROWS[2].1)
    }

    /// Shows tap `t`'s dials in the TAP panel.
    fn select(&mut self, t: usize) {
        self.selected = t;
        let r = Self::tap_panel();
        let specs: [(ParamSpec, &'static str, KnobKind); 3] = [(T_LEVEL, "LEVEL", KnobKind::Bipolar), (T_FEEDBACK, "FEEDBACK", KnobKind::Unipolar), (T_PAN, "PAN", KnobKind::Bipolar)];
        let w = r.width() * 0.62;
        self.tap_knobs = specs.iter().enumerate().map(|(i, (spec, label, kind))| Knob::new(*spec, label, *kind, Point::new(r.x0 + w * (i as f64 + 0.5) / 3.0, r.y0 + DIAL_DY), DIAL_D).with_id(tap_id(t, spec.id))).collect();
    }

    // ---- geometry for tests and tools -------------------------------------------------------

    fn fs(&self) -> f64 {
        f64::from(self.shared.sample_rate.load()).max(8000.0)
    }

    pub fn tempo(&self) -> f32 {
        self.shared.tempo.load()
    }

    /// The tap the TAP panel shows.
    pub fn selected(&self) -> usize {
        self.selected
    }

    /// The time span the echo plot shows, s.
    pub fn window(&self) -> f64 {
        self.window
    }

    /// Where a time sits across the echo plot.
    pub fn time_x(&self, seconds: f64) -> f64 {
        time_x(self.window, seconds)
    }

    pub fn level_y(&self, db: f64) -> f64 {
        level_y(db)
    }

    pub fn eq_y(&self, db: f64) -> f64 {
        eq_y(db)
    }

    pub fn freq_x(&self, hz: f64) -> f64 {
        freq_x(hz)
    }

    /// The open menu's rows (tests and tools).
    pub fn menu_rows(&self) -> Vec<Rect> {
        self.menu.as_ref().map(|(_, m)| m.item_rects()).unwrap_or_default()
    }

    /// The dial groups: name and panel.
    pub fn dial_groups(&self) -> Vec<(&'static str, Rect)> {
        self.groups.iter().map(|g| (g.name, g.rect)).collect()
    }

    pub fn dial(&self, id: &str) -> Option<Point> {
        self.knobs.iter().chain(&self.tap_knobs).find(|k| k.id == id).map(|k| k.center)
    }

    pub fn footer_toggle(&self, id: &str) -> Option<Rect> {
        self.footer.toggle_rect(id)
    }

    /// A pill's rect.
    pub fn pill(&self, pill: Pill) -> Rect {
        match pill {
            Pill::Mode(m) => {
                let g = Rect::new(GROUP_COLS[0].0, GROUP_ROWS[0].0, GROUP_COLS[0].1, GROUP_ROWS[0].1);
                let x = g.x0 + 14.0 + 70.0 * m.index() as f64;
                Rect::new(x, g.y0 + 42.0, x + 64.0, g.y0 + 42.0 + PILL_H)
            }
            Pill::Sync | Pill::PingPong => {
                let r = Self::tap_panel();
                let x = r.x0 + r.width() * 0.62 + 10.0;
                let y = r.y0 + if pill == Pill::Sync { 30.0 } else { 60.0 };
                Rect::new(x, y, r.x1 - 14.0, y + PILL_H)
            }
        }
    }

    /// Where a grip sits.
    pub fn node(&self, host: &dyn ParamHost, node: Node) -> Point {
        let s = settings(host, self.tempo());
        self.node_in(&s, node)
    }

    fn node_in(&self, s: &DelaySettings, node: Node) -> Point {
        match node {
            Node::Tap(t) => Point::new(time_x(self.window, f64::from(s.taps[t].time_ms) * 0.001), level_y(f64::from(s.taps[t].level_db))),
            Node::Feedback(t) => {
                let tap = &s.taps[t];
                let h = loop_gain(s, t, 1000.0, self.fs());
                let g = f64::from(tap.feedback) * h;
                let db = if g > 1e-6 { f64::from(tap.level_db) + 20.0 * g.log10() } else { FLOOR_DB };
                Point::new(time_x(self.window, 2.0 * f64::from(tap.time_ms) * 0.001), level_y(db))
            }
            Node::Band(b) => {
                let band = s.eq[b];
                Point::new(freq_x(band.freq), eq_y(if band.kind.has_gain() { band.gain_db } else { 0.0 }))
            }
        }
    }

    /// The grips that exist: each tap that is on (its feedback grip too) and each loop band.
    fn nodes(&self, host: &dyn ParamHost) -> Vec<Node> {
        let mut v: Vec<Node> = (0..LOOP_BANDS).filter(|&b| Self::on(host, &band_id(b, E_ON.id))).map(Node::Band).collect();
        for t in (0..TAPS).filter(|&t| Self::on(host, &tap_id(t, T_ON.id))) {
            v.push(Node::Feedback(t));
        }
        // The selected tap last, so it wins a hit.
        let mut taps: Vec<usize> = (0..TAPS).filter(|&t| Self::on(host, &tap_id(t, T_ON.id))).collect();
        taps.sort_by_key(|&t| t == self.selected);
        v.extend(taps.into_iter().map(Node::Tap));
        v
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

    /// Writes one parameter as its own undo step.
    fn gesture(host: &dyn ParamHost, id: &str, plain: f32) {
        host.begin_gesture(id);
        Self::set(host, id, plain);
        host.end_gesture(id);
    }

    fn node_params(node: Node) -> Vec<String> {
        match node {
            Node::Tap(t) => vec![tap_id(t, T_NOTE.id), tap_id(t, T_TIME.id), tap_id(t, T_LEVEL.id)],
            Node::Feedback(t) => vec![tap_id(t, T_FEEDBACK.id)],
            Node::Band(b) => vec![band_id(b, E_FREQ.id), band_id(b, E_GAIN.id)],
        }
    }

    /// The smallest window holding two echoes of the longest tap.
    fn fit_window(s: &DelaySettings) -> f64 {
        let longest = s.taps.iter().filter(|t| t.on).map(|t| f64::from(t.time_ms) * 0.001).fold(0.0, f64::max);
        WINDOWS.iter().copied().find(|&w| w >= 2.2 * longest).unwrap_or(WINDOWS[WINDOWS.len() - 1])
    }

    // ---- drawing ----------------------------------------------------------------------------

    fn draw_header(&mut self, cx: &mut DrawCx<'_>) {
        let theme = cx.theme;
        cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.window, None, &Rect::new(0.0, 0.0, VIEW_W, HEADER));
        cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, theme.line, None, &Line::new((0.0, HEADER - 0.5), (VIEW_W, HEADER - 0.5)));
        draw_mark(cx, 16.0, 15.0, 26.0);
        draw_title(cx, 52.0, 18.0, "Delay");
        self.session.draw(cx);
    }

    fn update_curves(&mut self, s: &DelaySettings) {
        let fs = self.fs();
        let key = (*s, fs as f32, self.selected);
        if self.curve_for == Some(key) {
            return;
        }
        let neutral = DelaySettings { eq: [mite_dsp::eq::BandSettings::default(); LOOP_BANDS], ..*s };
        for i in 0..CURVE_POINTS {
            let f = x_freq(PLOT_X0 + PLOT_W * i as f64 / (CURVE_POINTS - 1) as f64);
            self.curve[i] = s.eq.iter().filter(|b| b.enabled).map(|b| analog_response_db(b, f)).sum();
            self.machine[i] = 20.0 * loop_gain(&neutral, self.selected, f, fs).max(1e-6).log10();
        }
        self.curve_for = Some(key);
    }

    fn draw_echoes(&self, cx: &mut DrawCx<'_>, s: &DelaySettings) {
        let theme = cx.theme;
        let host = cx.host;
        let accent = theme.accent;
        cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.well, None, &ECHO);
        let label = TextStyle { tabular: true, ..TextStyle::BODY.sized(10.0).weighted(600.0) };
        // Levels: the middle line, and −36…0 dB on both sides.
        for db in [-36.0, -24.0, -12.0, 0.0] {
            let k = level_k(db) * (ECHO_MID - ECHO_TOP);
            for y in [ECHO_MID - k, ECHO_MID + k] {
                cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, if db == 0.0 { theme.grid_major } else { theme.grid_minor }, None, &Line::new((TIME_X0, y), (ECHO.x1, y)));
            }
            let text = if db == 0.0 { "0 dB".to_owned() } else { format!("−{}", -db as i32) };
            cx.text.draw(cx.scene, &text, 6.0, ECHO_MID - k - 6.0, label, theme.text_lo, Align::Left);
        }
        cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, theme.grid_major, None, &Line::new((TIME_X0, ECHO_MID), (ECHO.x1, ECHO_MID)));
        cx.text.draw(cx.scene, "L", 10.0, ECHO_TOP - 2.0, TextStyle::LABEL.sized(9.0), theme.text_mid, Align::Left);
        cx.text.draw(cx.scene, "R", 10.0, ECHO_BOTTOM - 10.0, TextStyle::LABEL.sized(9.0), theme.text_mid, Align::Left);
        // Time: the beats while a tap is synced, and a ruler.
        let tempo = f64::from(self.tempo());
        let synced = (0..TAPS).any(|t| s.taps[t].on && Self::on(host, &tap_id(t, T_SYNC.id)));
        if synced {
            let beat = 60.0 / tempo;
            let beats = (self.window / beat).floor() as usize;
            if beats <= 96 {
                for b in 1..=beats {
                    let x = time_x(self.window, b as f64 * beat);
                    let bar = b % 4 == 0;
                    cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, if bar { theme.grid_major } else { theme.grid_minor.multiply_alpha(0.7) }, None, &Line::new((x, ECHO_TOP), (x, ECHO_BOTTOM)));
                }
            }
        }
        let step = self.window / 4.0;
        for k in 1..=4 {
            let t = step * k as f64;
            let x = time_x(self.window, t);
            if !synced {
                cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, theme.grid_minor, None, &Line::new((x, ECHO_TOP), (x, ECHO_BOTTOM)));
            }
            cx.text.draw(cx.scene, &fmt_ms(t * 1000.0), x - 4.0, ECHO.y1 - 18.0, label, theme.text_lo, Align::Right);
        }
        // The input, at time 0.
        cx.scene.stroke(&Stroke::new(1.5), Affine::IDENTITY, theme.spectrum_pre.multiply_alpha(0.8), None, &Line::new((TIME_X0, ECHO_TOP + 10.0), (TIME_X0, ECHO_BOTTOM - 10.0)));
        cx.begin_clip(&ECHO);
        // Each tap's echoes: the left side up, the right side down; the selected tap lit.
        let fs = self.fs();
        let mut out = [[0.0f32; 2]; MAX_ECHOES];
        let order: Vec<usize> = (0..TAPS).filter(|&t| s.taps[t].on && t != self.selected).chain((self.selected < TAPS && s.taps[self.selected].on).then_some(self.selected)).collect();
        for t in order {
            let lit = t == self.selected;
            echoes(s, t, 1000.0, fs, &mut out);
            let period = f64::from(s.taps[t].time_ms) * 0.001;
            let ink = if lit { accent } else { accent.multiply_alpha(0.5) };
            let mut tops = BezPath::new();
            for (k, e) in out.iter().enumerate() {
                let x = time_x(self.window, period * (k + 1) as f64);
                if x > ECHO.x1 {
                    break;
                }
                let [l, r] = e.map(|a| if a > 1e-6 { 20.0 * f64::from(a).log10() } else { -200.0 });
                if l < FLOOR_DB && r < FLOOR_DB {
                    break;
                }
                let (yl, yr) = (ECHO_MID - level_k(l) * (ECHO_MID - ECHO_TOP), ECHO_MID + level_k(r) * (ECHO_MID - ECHO_TOP));
                let w = if k == 0 { 3.0 } else { 2.0 };
                let stem = Line::new((x, yl), (x, yr));
                if lit && k == 0 {
                    glow_stroke(cx, &stem, w, ink, ink, 3.0);
                } else {
                    cx.scene.stroke(&Stroke::new(w), Affine::IDENTITY, ink.multiply_alpha(if k == 0 { 1.0 } else { 0.85 }), None, &stem);
                }
                if k == 0 {
                    tops.move_to((x, yl));
                } else {
                    tops.line_to((x, yl));
                }
            }
            if lit {
                cx.scene.stroke(&Stroke::new(1.0).with_dashes(0.0, [3.0, 3.0]), Affine::IDENTITY, ink.multiply_alpha(0.45), None, &tops);
            }
        }
        cx.end_clip();
        // The grips: each tap's first echo (numbered) and its feedback grip on the second.
        for node in self.nodes(host) {
            let (t, fb) = match node {
                Node::Tap(t) => (t, false),
                Node::Feedback(t) => (t, true),
                Node::Band(_) => continue,
            };
            let p = self.node_in(s, node);
            if p.x > ECHO.x1 - 4.0 {
                continue;
            }
            let hot = self.hot == Some(Hot::Node(node)) || matches!(self.drag, Some(Drag::Node { node: n, .. }) if n == node);
            let lit = t == self.selected;
            let ink = if lit { accent } else { accent.multiply_alpha(0.6) };
            if fb {
                let c = Circle::new(p, FB_R);
                cx.scene.fill(Fill::NonZero, Affine::IDENTITY, if hot { ink } else { theme.display }, None, &c);
                cx.scene.stroke(&Stroke::new(1.4), Affine::IDENTITY, ink, None, &c);
            } else {
                let c = Circle::new(p, GRIP_R);
                cx.scene.fill(Fill::NonZero, Affine::IDENTITY, if hot { ink } else { theme.display }, None, &c);
                if lit {
                    glow_stroke(cx, &c, 1.6, ink, ink, if hot { 4.0 } else { 2.0 });
                } else {
                    cx.scene.stroke(&Stroke::new(1.3), Affine::IDENTITY, ink, None, &c);
                }
                let num = TextStyle { tabular: true, ..TextStyle::BODY.sized(9.0).weighted(700.0) };
                cx.text.draw(cx.scene, &format!("{}", t + 1), p.x, p.y - 5.5, num, if hot { theme.display } else { ink }, Align::Center);
                // A synced tap carries its note above it.
                if Self::on(host, &tap_id(t, T_SYNC.id)) {
                    let note = NOTES[(Self::plain(host, &tap_id(t, T_NOTE.id)).round() as usize).min(NOTES.len() - 1)];
                    cx.text.draw(cx.scene, note, p.x, p.y - 24.0, TextStyle::LABEL.sized(9.0), ink, Align::Center);
                }
            }
        }
    }

    fn draw_eq(&self, cx: &mut DrawCx<'_>, s: &DelaySettings) {
        let theme = cx.theme;
        let host = cx.host;
        cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.well, None, &EQ);
        let label = TextStyle { tabular: true, ..TextStyle::BODY.sized(10.0).weighted(600.0) };
        for (db, t) in [(12.0, "+12"), (0.0, "0 dB"), (-12.0, "−12")] {
            let y = eq_y(db);
            cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, if db == 0.0 { theme.grid_major } else { theme.grid_minor }, None, &Line::new((PLOT_X0, y), (EQ.x1, y)));
            cx.text.draw(cx.scene, t, 6.0, y - 6.0, label, theme.text_lo, Align::Left);
        }
        cx.text.draw(cx.scene, "LOOP", EQ.x1 - 12.0, EQ.y0 + 8.0, TextStyle::LABEL.sized(9.0), theme.text_mid, Align::Right);
        cx.begin_clip(&EQ);
        self.spectrum.draw(cx, EQ);
        let xs: Vec<f64> = (0..CURVE_POINTS).map(|i| PLOT_X0 + PLOT_W * i as f64 / (CURVE_POINTS - 1) as f64).collect();
        // The machine's own losses each pass (for the selected tap: a BBD's depend on its time).
        if s.mode != Mode::Digital {
            let pts: Vec<Point> = xs.iter().zip(&self.machine).map(|(&x, &db)| Point::new(x, eq_y(db))).collect();
            cx.scene.stroke(&Stroke::new(1.3).with_dashes(0.0, [5.0, 4.0]), Affine::IDENTITY, theme.accent.multiply_alpha(0.55), None, &history::smooth_path(&pts));
        }
        // The bands' sum, lit.
        let ink = theme.curve;
        let pts: Vec<Point> = xs.iter().zip(&self.curve).map(|(&x, &db)| Point::new(x, eq_y(db))).collect();
        let path = history::smooth_path(&pts);
        let mut fill = path.clone();
        fill.line_to((pts[pts.len() - 1].x, eq_y(0.0)));
        fill.line_to((pts[0].x, eq_y(0.0)));
        fill.close_path();
        cx.scene.fill(Fill::NonZero, Affine::IDENTITY, ink.multiply_alpha(0.1), None, &fill);
        glow_stroke(cx, &path, 2.0, ink, ink, 3.0);
        cx.end_clip();
        for node in self.nodes(host) {
            let Node::Band(b) = node else { continue };
            let p = self.node_in(s, node);
            let hot = self.hot == Some(Hot::Node(node)) || matches!(self.drag, Some(Drag::Node { node: n, .. }) if n == node);
            let band = s.eq[b];
            if band.kind == BandType::Bell {
                let bw = 2.0 / std::f64::consts::LN_2 * (1.0 / (2.0 * band.q)).asinh();
                let (x0, x1) = (freq_x(band.freq * 2f64.powf(-bw / 2.0)), freq_x(band.freq * 2f64.powf(bw / 2.0)));
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
            glow_stroke(cx, &c, 1.6, ink, ink, if hot { 4.0 } else { 2.0 });
            let num = TextStyle { tabular: true, ..TextStyle::BODY.sized(9.0).weighted(700.0) };
            cx.text.draw(cx.scene, &format!("{}", b + 1), p.x, p.y - 5.5, num, if hot { theme.display } else { ink }, Align::Center);
        }
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
        cx.text.draw(cx.scene, text, c.x, c.y - 5.5, TextStyle::LABEL.sized(9.0), if on { ink } else if hot { theme.text_hi } else { theme.text_mid }, Align::Center);
    }

    fn draw_groups(&self, cx: &mut DrawCx<'_>, s: &DelaySettings) {
        let theme = cx.theme;
        let host = cx.host;
        for g in &self.groups {
            panel(cx.scene, RoundedRect::from_rect(g.rect, 10.0), theme.well, theme.well_border);
            let name = if g.name == "TAP" { format!("TAP {}", self.selected + 1) } else { g.name.to_owned() };
            cx.text.draw(cx.scene, &name, g.rect.x0 + 12.0, g.rect.y0 + 9.0, TextStyle::LABEL.sized(9.0), theme.text_mid, Align::Left);
        }
        for m in Mode::ALL {
            self.draw_pill(cx, Pill::Mode(m), &Mode::NAMES[m.index()].to_uppercase(), s.mode == m);
        }
        let t = self.selected;
        let tap_on = s.taps[t].on;
        let sync = Self::on(host, &tap_id(t, T_SYNC.id));
        let sync_text = if sync { format!("SYNC {}", NOTES[(Self::plain(host, &tap_id(t, T_NOTE.id)).round() as usize).min(NOTES.len() - 1)]) } else { format!("FREE {}", fmt_ms(f64::from(s.taps[t].time_ms))) };
        self.draw_pill(cx, Pill::Sync, &sync_text, sync && tap_on);
        self.draw_pill(cx, Pill::PingPong, "PING-PONG", s.taps[t].pingpong && tap_on);
    }

    fn draw_meters(&self, cx: &mut DrawCx<'_>) {
        let theme = cx.theme;
        cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, theme.line, None, &Line::new((ECHO.x1 + 0.5, DISPLAY_TOP), (ECHO.x1 + 0.5, HELP_TOP)));
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
        let band = matches!(what, MenuFor::Band(_) | MenuFor::NewBand(_));
        let color = if band { cx.theme.curve } else { cx.theme.accent };
        let remove = list.items.len() - 1;
        list.draw(cx, color, |scene, i, at, ink| {
            if i == remove {
                let mut x = BezPath::new();
                x.move_to(at + (6.0, 6.0));
                x.line_to(at + (16.0, 16.0));
                x.move_to(at + (16.0, 6.0));
                x.line_to(at + (6.0, 16.0));
                scene.stroke(&Stroke::new(1.6), Affine::IDENTITY, ink, None, &x);
            } else if matches!(what, MenuFor::Note(_)) {
            } else if band {
                draw_type_icon(scene, BandType::from_index(i), at + (0.0, 6.0), ink);
            } else {
                draw_tap_icon(scene, i, at, ink);
            }
        });
    }

    // ---- help -------------------------------------------------------------------------------

    fn tap_help(&self, host: &dyn ParamHost, t: usize, dragging: bool) -> Help {
        let s = settings(host, self.tempo());
        let tap = s.taps[t];
        let time = if Self::on(host, &tap_id(t, T_SYNC.id)) {
            format!("{} · {}", NOTES[(Self::plain(host, &tap_id(t, T_NOTE.id)).round() as usize).min(NOTES.len() - 1)], fmt_ms(f64::from(tap.time_ms)))
        } else {
            fmt_ms(f64::from(tap.time_ms))
        };
        Help::default()
            .text(format!("Tap {} · ", t + 1))
            .slot(time, "1/1. · 5.00 s")
            .text(" · ")
            .slot(fmt_db(f64::from(tap.level_db)), "−48.0 dB")
            .text(format!(" · pan {:+.0}{}", tap.pan * 100.0, if tap.pingpong { " · ping-pong" } else { "" }))
            .text(if dragging { " — Shift is fine." } else { " — drag: time and level; wheel: pan; right-click: sync, ping-pong, remove; double-click removes it." })
    }

    fn help_text(&self, host: &dyn ParamHost) -> Help {
        if let Some((what, list)) = &self.menu {
            let i = self.pointer.and_then(|p| list.item_rects().iter().position(|r| r.contains(p))).map(|k| list.shown[k]);
            return match (i, what) {
                (Some(i), _) if i == list.items.len() - 1 => Help::default().text("Remove — it stops (its settings are kept for when it comes back)"),
                (Some(i), MenuFor::Note(_)) if i < NOTES.len() => Help::default().text(format!("{} — the tap's time at the host's tempo ({})", NOTE_MENU[i], fmt_ms(f64::from(note_ms(i, self.tempo()))))),
                (Some(_), MenuFor::Note(_)) => Help::default().text("Free time — the tap's time in ms, off the tempo (nothing moves)"),
                (Some(0), MenuFor::Tap(_)) => Help::default().text("The tap's time: a note of the host's tempo, or free in ms (nothing moves when you switch)"),
                (Some(1), MenuFor::Tap(_)) => Help::default().text("Ping-pong: the echoes alternate left and right (the input summed to mono starts on the left)"),
                (Some(i), MenuFor::NewBand(_)) => Help::default().text(format!("{} — a new loop band of this shape where you clicked", list.items[i])),
                (Some(i), MenuFor::Band(_)) => Help::default().text(format!("{} — this band takes the shape", list.items[i])),
                _ => Help::default().text("Pick one; click elsewhere to close."),
            };
        }
        let node = match (self.drag, self.hot) {
            (Some(Drag::Node { node, .. }), _) => Some((node, true)),
            (None, Some(Hot::Node(node))) => Some((node, false)),
            _ => None,
        };
        if let Some((node, dragging)) = node {
            let s = settings(host, self.tempo());
            return match node {
                Node::Tap(t) => self.tap_help(host, t, dragging),
                Node::Feedback(t) => Help::default()
                    .text(format!("Tap {} feedback ", t + 1))
                    .slot(format!("{:.0} %", s.taps[t].feedback * 100.0), "100 %")
                    .text(" — each echo against the one before; drag the second echo up or down."),
                Node::Band(b) => {
                    let band = s.eq[b];
                    let mut h = Help::default().text(format!("Loop band {} · {} · ", b + 1, BAND_MENU[band.kind as usize])).slot(fmt_hz(band.freq), "20.00 kHz");
                    if band.kind.has_gain() {
                        h = h.text(" · ").slot(format!("{:+.1} dB", band.gain_db), "+24.0 dB");
                    }
                    if band.kind.has_slope() {
                        h = h.text(" · ").slot(format!("{} dB/oct", SLOPES_DB[band.slope.min(SLOPES_DB.len() - 1)]), "96 dB/oct");
                    } else {
                        h = h.text(" · Q ").slot(format!("{:.2}", band.q), "10.00");
                    }
                    let wheel = if band.kind.has_slope() { "slope" } else { "Q" };
                    h.text(if dragging { " — every echo passes it once more.".to_owned() } else { format!(" — drag; wheel: {wheel}; right-click: shape or remove; double-click removes it.") })
                }
            };
        }
        match self.hot {
            Some(Hot::Pill(Pill::Mode(Mode::Digital))) => return Help::default().text("Digital — clean echoes; a new time crossfades"),
            Some(Hot::Pill(Pill::Mode(Mode::Tape))) => return Help::default().text("Tape — saturates softly, loses the top and the deep lows each pass, wows and flutters; a new time glides (the pitch bends)"),
            Some(Hot::Pill(Pill::Mode(Mode::Bbd))) => return Help::default().text("BBD — a bucket brigade: darker the longer the time, a harder saturation, a chorus; a new time glides"),
            Some(Hot::Pill(Pill::Sync)) => return Help::default().text("The selected tap's time — click: pick a note of the host's tempo (or free time); wheel: the next note. Dragging the tap across snaps to notes too."),
            Some(Hot::Pill(Pill::PingPong)) => return Help::default().text("Ping-pong: the selected tap's echoes alternate left and right. Click to switch."),
            _ => {}
        }
        if let Some(h) = self.footer.help(host) {
            return h;
        }
        if let Some(k) = self.knobs.iter().chain(&self.tap_knobs).find(|k| self.pointer.is_some_and(|p| k.bounds().contains(p))) {
            let v = Self::plain(host, &k.id);
            return Help::default().text(format!("{} ", k.spec.name)).slot(k.spec.format(v), k.spec.format(k.spec.max)).text(knob_words(k.spec.id));
        }
        if let Some(g) = self.groups.iter().find(|g| self.pointer.is_some_and(|p| g.rect.contains(p))) {
            return Help::default().text(format!("{} — {}", g.name, g.words));
        }
        if let Some(h) = self.session.help() {
            return h;
        }
        if self.pointer.is_some_and(|p| ECHO.contains(p)) {
            return Help::default().text("The echoes, left side up, right side down, as loud as they come out. Double-click for a new tap there (up to four).");
        }
        if self.pointer.is_some_and(|p| EQ.contains(p)) {
            return Help::default().text("The loop EQ: every echo passes it once more. Double-click for a bell; right-click for any shape. Dashed: the machine's own losses.");
        }
        Help::default().text("Top: the echoes; drag a tap's grip for its time and level, its second echo for the feedback. Below: the loop EQ and the dials.")
    }

    // ---- events -----------------------------------------------------------------------------

    fn busy(&self) -> bool {
        self.drag.is_some() || self.menu.is_some() || self.knobs.iter().chain(&self.tap_knobs).any(Knob::is_dragging) || self.footer.busy()
    }

    fn hot_at(&self, host: &dyn ParamHost, p: Point) -> Option<Hot> {
        for pill in Mode::ALL.map(Pill::Mode).into_iter().chain([Pill::Sync, Pill::PingPong]) {
            if self.pill(pill).inflate(2.0, 2.0).contains(p) {
                return Some(Hot::Pill(pill));
            }
        }
        if ECHO.contains(p) || EQ.contains(p) {
            for node in self.nodes(host).into_iter().rev() {
                let r = if matches!(node, Node::Feedback(_)) { FB_R + 5.0 } else { GRIP_HIT };
                if self.node(host, node).distance(p) < r {
                    return Some(Hot::Node(node));
                }
            }
        }
        None
    }

    /// A tap's time from a pointer's x: a note while the tap is synced.
    fn set_tap_time(&self, host: &dyn ParamHost, t: usize, x: f64, raw: bool) {
        let ms = (x_time(self.window, x) * 1000.0).clamp(f64::from(MIN_TIME_MS), f64::from(MAX_TIME_MS)) as f32;
        let write = if raw { Self::set_raw } else { Self::set };
        if Self::on(host, &tap_id(t, T_SYNC.id)) {
            write(host, &tap_id(t, T_NOTE.id), nearest_note(ms, self.tempo()) as f32);
        } else {
            write(host, &tap_id(t, T_TIME.id), T_TIME.snap(ms));
        }
    }

    /// A new tap where the echo plot was double-clicked (its time and level there). Whether
    /// one was made (there are four).
    fn add_tap(&mut self, host: &dyn ParamHost, at: Point) -> bool {
        let Some(t) = (0..TAPS).find(|&t| !Self::on(host, &tap_id(t, T_ON.id))) else { return false };
        let level = T_LEVEL.snap((y_level(at.y) as f32).clamp(T_LEVEL.min, T_LEVEL.max));
        self.set_tap_time(host, t, at.x, false);
        Self::set(host, &tap_id(t, T_LEVEL.id), level);
        Self::set(host, &tap_id(t, T_ON.id), 1.0);
        self.select(t);
        true
    }

    /// A new loop band of `kind` at `at`. Whether one was made (there are six).
    fn add_band(&self, host: &dyn ParamHost, kind: usize, at: Point) -> bool {
        let Some(b) = (0..LOOP_BANDS).find(|&b| !Self::on(host, &band_id(b, E_ON.id))) else { return false };
        let kind_t = BandType::from_index(kind);
        let gain = if kind_t.has_gain() { E_GAIN.snap((y_eq(at.y) as f32).clamp(E_GAIN.min, E_GAIN.max)) } else { 0.0 };
        let q = if kind_t == BandType::Bell { 1.0 } else { E_Q.default };
        let f = (x_freq(at.x) as f32).clamp(E_FREQ.min, E_FREQ.max);
        for (id, v) in [(band_id(b, E_FREQ.id), E_FREQ.snap(f)), (band_id(b, E_TYPE.id), kind as f32), (band_id(b, E_GAIN.id), gain), (band_id(b, E_Q.id), q), (band_id(b, E_SLOPE.id), E_SLOPE.default as f32), (band_id(b, E_ON.id), 1.0)] {
            Self::set(host, &id, v);
        }
        true
    }

    fn click_pill(&mut self, host: &dyn ParamHost, pill: Pill) {
        let t = self.selected;
        match pill {
            Pill::Mode(m) => Self::gesture(host, MODE.id, m.index() as f32),
            Pill::Sync => {
                let current = if Self::on(host, &tap_id(t, T_SYNC.id)) { Self::plain(host, &tap_id(t, T_NOTE.id)).round() as usize } else { NOTES.len() };
                self.menu = Some((MenuFor::Note(t), Dropdown::new(self.pill(pill), &NOTE_MENU, current, (DISPLAY_TOP, HELP_TOP)).with_min_width(150.0)));
            }
            Pill::PingPong => {
                let id = tap_id(t, T_PINGPONG.id);
                Self::gesture(host, &id, if Self::on(host, &id) { 0.0 } else { 1.0 });
            }
        }
    }

    /// Switches a tap between synced and free without moving it: free takes the note's ms, synced
    /// the nearest note.
    fn toggle_sync(&self, host: &dyn ParamHost, t: usize) {
        let s = settings(host, self.tempo());
        let id = tap_id(t, T_SYNC.id);
        let now = s.taps[t].time_ms;
        if Self::on(host, &id) {
            Self::gesture(host, &tap_id(t, T_TIME.id), T_TIME.snap(now));
            Self::gesture(host, &id, 0.0);
        } else {
            Self::gesture(host, &tap_id(t, T_NOTE.id), nearest_note(now, self.tempo()) as f32);
            Self::gesture(host, &id, 1.0);
        }
    }

    fn press(&mut self, host: &dyn ParamHost, pos: Point) -> bool {
        match self.hot_at(host, pos) {
            Some(Hot::Pill(pill)) => {
                self.click_pill(host, pill);
                true
            }
            Some(Hot::Node(node)) => {
                if let Node::Tap(t) | Node::Feedback(t) = node {
                    if t != self.selected {
                        self.select(t);
                    }
                }
                for id in Self::node_params(node) {
                    host.begin_gesture(&id);
                }
                self.drag = Some(Drag::Node { node, start: pos, at: self.node(host, node) });
                true
            }
            None => false,
        }
    }

    fn end_drag(&mut self, host: &dyn ParamHost) {
        if let Some(Drag::Node { node, .. }) = self.drag.take() {
            for id in Self::node_params(node) {
                host.end_gesture(&id);
            }
            self.window = Self::fit_window(&settings(host, self.tempo()));
        }
    }

    /// A grip dragged to `to` (where it should now sit).
    fn move_node(&self, host: &dyn ParamHost, node: Node, to: Point) {
        match node {
            Node::Tap(t) => {
                self.set_tap_time(host, t, to.x, true);
                Self::set_raw(host, &tap_id(t, T_LEVEL.id), T_LEVEL.snap((y_level(to.y.min(ECHO_MID)) as f32).clamp(T_LEVEL.min, T_LEVEL.max)));
            }
            Node::Feedback(t) => {
                // The second echo through the pointer: its level against the first's, less the
                // loop's own gain.
                let s = settings(host, self.tempo());
                let h = loop_gain(&s, t, 1000.0, self.fs()).max(1e-6);
                let db = y_level(to.y.min(ECHO_MID));
                let fb = if db <= FLOOR_DB + 0.01 { 0.0 } else { 10f64.powf((db - f64::from(s.taps[t].level_db)) / 20.0) / h };
                Self::set_raw(host, &tap_id(t, T_FEEDBACK.id), T_FEEDBACK.snap((fb as f32 * 100.0).clamp(0.0, 100.0)));
            }
            Node::Band(b) => {
                let f = (x_freq(to.x) as f32).clamp(E_FREQ.min, E_FREQ.max);
                Self::set_raw(host, &band_id(b, E_FREQ.id), E_FREQ.snap(f));
                let kind = BandType::from_index(Self::plain(host, &band_id(b, E_TYPE.id)).round() as usize);
                if kind.has_gain() {
                    Self::set_raw(host, &band_id(b, E_GAIN.id), E_GAIN.snap((y_eq(to.y) as f32).clamp(E_GAIN.min, E_GAIN.max)));
                }
            }
        }
    }

    fn open_menu(&mut self, host: &dyn ParamHost, pos: Point) -> bool {
        let anchor = Rect::from_center_size(pos, (2.0, 2.0));
        let area = (DISPLAY_TOP, HELP_TOP);
        match self.hot_at(host, pos) {
            Some(Hot::Node(Node::Tap(t) | Node::Feedback(t))) => {
                self.select(t);
                let sync = usize::from(Self::on(host, &tap_id(t, T_SYNC.id)));
                let pp = usize::from(Self::on(host, &tap_id(t, T_PINGPONG.id)));
                self.menu = Some((MenuFor::Tap(t), Dropdown::new(anchor, TAP_MENUS[sync][pp], usize::MAX, area).with_min_width(210.0)));
            }
            Some(Hot::Node(Node::Band(b))) => {
                let kind = Self::plain(host, &band_id(b, E_TYPE.id)).round() as usize;
                self.menu = Some((MenuFor::Band(b), Dropdown::new(anchor, &BAND_MENU, kind, area).with_min_width(170.0)));
            }
            None if EQ.contains(pos) => {
                self.menu = Some((MenuFor::NewBand(pos), Dropdown::new(anchor, &BAND_MENU, usize::MAX, area).with_shown((0..BAND_MENU.len() - 1).collect()).with_min_width(170.0)));
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
                    MenuFor::NewBand(at) => {
                        self.add_band(host, i, at);
                    }
                    MenuFor::Band(b) if i == remove => Self::set(host, &band_id(b, E_ON.id), 0.0),
                    MenuFor::Band(b) => Self::gesture(host, &band_id(b, E_TYPE.id), i as f32),
                    MenuFor::Note(t) if i < NOTES.len() => {
                        Self::gesture(host, &tap_id(t, T_NOTE.id), i as f32);
                        if !Self::on(host, &tap_id(t, T_SYNC.id)) {
                            Self::gesture(host, &tap_id(t, T_SYNC.id), 1.0);
                        }
                    }
                    MenuFor::Note(t) => {
                        if Self::on(host, &tap_id(t, T_SYNC.id)) {
                            self.toggle_sync(host, t);
                        }
                    }
                    MenuFor::Tap(t) if i == remove => Self::set(host, &tap_id(t, T_ON.id), 0.0),
                    MenuFor::Tap(t) if i == 0 => self.toggle_sync(host, t),
                    MenuFor::Tap(t) => {
                        let id = tap_id(t, T_PINGPONG.id);
                        Self::gesture(host, &id, if Self::on(host, &id) { 0.0 } else { 1.0 });
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
        for k in self.knobs.iter_mut().chain(self.tap_knobs.iter_mut()) {
            if k.event(event, host) {
                return true;
            }
        }
        match *event {
            UiEvent::PointerMove { pos, mods } => {
                self.pointer = Some(pos);
                let k = if mods.fine() { 0.2 } else { 1.0 };
                if let Some(Drag::Node { node, start, at }) = self.drag {
                    let mut to = at + (pos - start) * k;
                    to.y = match node {
                        Node::Band(_) => to.y.clamp(EQ_TOP, EQ_BOTTOM),
                        _ => to.y.clamp(ECHO_TOP, ECHO_BOTTOM),
                    };
                    self.move_node(host, node, to);
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
                    Some(Hot::Node(Node::Tap(t))) => Self::set(host, &tap_id(t, T_ON.id), 0.0),
                    Some(Hot::Node(Node::Feedback(t))) => Self::set(host, &tap_id(t, T_FEEDBACK.id), T_FEEDBACK.default),
                    Some(Hot::Node(Node::Band(b))) => Self::set(host, &band_id(b, E_ON.id), 0.0),
                    Some(Hot::Pill(_)) => return false,
                    None if ECHO.contains(pos) && pos.x > TIME_X0 => return self.add_tap(host, pos),
                    None if EQ.contains(pos) => return self.add_band(host, 0, pos),
                    None => return false,
                }
                true
            }
            UiEvent::Wheel { pos, lines, mods } => {
                let fine = mods.fine();
                match self.hot_at(host, pos) {
                    Some(Hot::Node(Node::Tap(t))) => {
                        let id = tap_id(t, T_PAN.id);
                        Self::set(host, &id, T_PAN.wheel(Self::plain(host, &id), lines, fine));
                    }
                    Some(Hot::Node(Node::Feedback(t))) => {
                        let id = tap_id(t, T_FEEDBACK.id);
                        Self::set(host, &id, T_FEEDBACK.wheel(Self::plain(host, &id), lines, fine));
                    }
                    Some(Hot::Pill(Pill::Sync)) => {
                        let t = self.selected;
                        if Self::on(host, &tap_id(t, T_SYNC.id)) {
                            let id = tap_id(t, T_NOTE.id);
                            Self::set(host, &id, (Self::plain(host, &id) + lines.signum()).clamp(0.0, (NOTES.len() - 1) as f32));
                        } else {
                            let id = tap_id(t, T_TIME.id);
                            Self::set(host, &id, T_TIME.wheel(Self::plain(host, &id), lines, fine));
                        }
                    }
                    Some(Hot::Node(Node::Band(b))) => {
                        if BandType::from_index(Self::plain(host, &band_id(b, E_TYPE.id)).round() as usize).has_slope() {
                            let id = band_id(b, E_SLOPE.id);
                            let n = E_SLOPE.choices.len() as f32 - 1.0;
                            Self::set(host, &id, (Self::plain(host, &id) + lines.signum()).clamp(0.0, n));
                        } else {
                            let id = band_id(b, E_Q.id);
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

/// A tap menu row's glyph: a note (the time's kind) or two arrows (ping-pong).
fn draw_tap_icon(scene: &mut mite_ui::vello::Scene, i: usize, at: Point, ink: Color) {
    let mut p = BezPath::new();
    if i == 0 {
        p.move_to(at + (13.0, 4.0));
        p.line_to(at + (13.0, 15.0));
        p.move_to(at + (13.0, 4.0));
        p.line_to(at + (17.0, 7.0));
        scene.stroke(&Stroke::new(1.5), Affine::IDENTITY, ink, None, &p);
        scene.fill(Fill::NonZero, Affine::IDENTITY, ink, None, &mite_ui::kurbo::Ellipse::new(at + (10.5, 15.5), (3.0, 2.2), -0.4));
    } else {
        p.move_to(at + (4.0, 8.0));
        p.line_to(at + (18.0, 8.0));
        p.line_to(at + (15.0, 5.0));
        p.move_to(at + (18.0, 14.0));
        p.line_to(at + (4.0, 14.0));
        p.line_to(at + (7.0, 17.0));
        scene.stroke(&Stroke::new(1.5), Affine::IDENTITY, ink, None, &p);
    }
}

fn knob_words(id: &str) -> &'static str {
    match id {
        "drive" => " — how hard the machine saturates each pass (loud repeats thicken and stay in bounds)",
        "mod_depth" => " — how far the line's time drifts: chorus (Digital), wow and flutter (Tape), a triangle chorus (BBD)",
        "mod_rate" => " — how fast that drift moves",
        "diffusion" => " — each echo smeared a little more each pass: from clean repeats to a wash",
        "width" => " — the echoes' stereo spread (0 %: mono, 150 %: wider)",
        "duck" => " — how far the echoes are turned down while the input plays (0: off)",
        "duck_thresh" => " — where the input starts turning the echoes down",
        "duck_release" => " — how fast the echoes come back after the input stops",
        "level" => " — the first echo's level",
        "feedback" => " — each echo against the one before (also: drag the second echo)",
        "pan" => " — where the tap's echoes sit (also: the wheel on its grip)",
        _ => "",
    }
}

impl View for DelayView {
    fn logical_size(&self) -> (f64, f64) {
        (VIEW_W, VIEW_H)
    }

    fn draw(&mut self, cx: &mut DrawCx<'_>) {
        let s = settings(cx.host, self.tempo());
        if self.drag.is_none() {
            self.window = Self::fit_window(&s);
        }
        self.update_curves(&s);
        let theme = cx.theme;
        self.draw_header(cx);
        cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.display, None, &Rect::new(0.0, DISPLAY_TOP, VIEW_W, HELP_TOP));
        // Nothing glows through an open menu.
        let hole = self.menu.as_ref().map(|(_, m)| RoundedRect::from_rect(m.bounds(), 8.0));
        if let Some(h) = hole {
            cx.begin_glow_hole(h);
        }
        self.draw_echoes(cx, &s);
        self.draw_eq(cx, &s);
        self.draw_groups(cx, &s);
        for k in self.knobs.iter().chain(&self.tap_knobs) {
            k.draw(cx);
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
        // The selected tap stays one that is on.
        if !Self::on(host, &tap_id(self.selected, T_ON.id)) {
            if let Some(t) = (0..TAPS).find(|&t| Self::on(host, &tap_id(t, T_ON.id))) {
                self.select(t);
            }
        }
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
        if let Some(r) = self.history.back().copied() {
            let targets = [r[record::IN], r[record::IN + 1], r[record::OUT], r[record::OUT + 1]];
            for (m, t) in self.meters.iter_mut().zip(targets) {
                let fall = METER_RELEASE_DB_S * dt;
                *m = if t > *m { t } else { (*m - fall).max(t) };
            }
        }
        let spectrum = self.spectrum.update(&self.shared, f64::from(dt));
        fresh || spectrum || busy || self.meters.iter().any(|&m| m > SILENT + 1.0)
    }
}
