//! The Mite Pitch editor (docs/design/pitch-brief.md §3), in 1280×760 design units: the take's
//! notes as blobs on a pitch grid (time across, pitch up, a blob's thickness its level), each with
//! the pitch curve it will play, the piano down the left, a time ruler, the playhead.
//!
//! Tools (header; keys 1–6): **main** (drag a note: up and down its pitch, snapped as the footer
//! says, Alt the other way, Shift fine; sideways its timing; its edges its length; double-click:
//! onto the snap), **vibrato** and **drift** (drag up and down: more or less of the fast wobble or
//! the slow slides; double-click: as sung), **formant**, **level** (drag up and down; double-
//! click: as sung), **split** (click a note: split it there; click where two notes meet: join
//! them). Every tool acts on the selection: click, Shift-click, drag a box, Ctrl+A. The footer:
//! TRANSFER, the snap (semitones, the key's scale, free), the key, and the Correct macro (Centre:
//! the selection, or every note, pulled onto the snap; Drift: their slow drift taken out).
//! Wheel: scroll in time (Ctrl: zoom, Shift: pitch). Ctrl+Z / Ctrl+Shift+Z: undo, redo.

use std::collections::BTreeSet;
use std::sync::atomic::Ordering;
use std::sync::Arc;

use mite_core::{BoolSpec, ParamDisplay, ParamScale, ParamSpec};
use mite_dsp::melody::{Analysis, Note, NoteEdit};
use mite_ui::kurbo::{Affine, BezPath, Circle, Line, Point, Rect, RoundedRect, Stroke};
use mite_ui::paint::glow_stroke;
use mite_ui::peniko::Fill;
use mite_ui::widgets::brand::{draw_mark, draw_title};
use mite_ui::widgets::dropdown::{Dropdown, DropdownEvent};
use mite_ui::widgets::footer::draw_dropdown_button;
use mite_ui::widgets::rail::draw_glyph;
use mite_ui::widgets::{Footer, FooterToggle, Help, ToggleGlyph};
use mite_ui::{Align, DrawCx, Key, Modifiers, MouseButton, ParamHost, TextStyle, UiEvent, View};

use crate::params::BYPASS;
use crate::session::{Snap, Snapshot, Status};
use crate::PitchShared;

/// Mite Pitch wears the Pitch accent (tokens.json `color.accent.pitch`, blush).
pub const ACCENT: &str = "pitch";

const VIEW_W: f64 = 1280.0;
const VIEW_H: f64 = 760.0;
const HEADER: f64 = 56.0;
const DISPLAY_TOP: f64 = HEADER;
const HELP_TOP: f64 = DISPLAY_TOP + 620.0;
const FOOTER_TOP: f64 = HELP_TOP + 28.0;
const KEYS: Rect = Rect::new(0.0, DISPLAY_TOP, 64.0, HELP_TOP - 26.0);
const ROLL: Rect = Rect::new(64.0, DISPLAY_TOP, VIEW_W, HELP_TOP - 26.0);
const RULER: Rect = Rect::new(64.0, HELP_TOP - 26.0, VIEW_W, HELP_TOP);
const TRANSFER: Rect = Rect::new(16.0, FOOTER_TOP + 14.0, 156.0, FOOTER_TOP + 42.0);
/// The snap's three buttons, from here, each this wide.
const SNAP_X: f64 = 176.0;
const SNAP_W: f64 = 36.0;
const KEY_BUTTON: Rect = Rect::new(298.0, FOOTER_TOP + 10.0, 440.0, FOOTER_TOP + 46.0);
/// The Correct macro's two sliders.
const CENTRE_SLIDER: Rect = Rect::new(540.0, FOOTER_TOP + 22.0, 720.0, FOOTER_TOP + 34.0);
const DRIFT_SLIDER: Rect = Rect::new(820.0, FOOTER_TOP + 22.0, 1000.0, FOOTER_TOP + 34.0);
/// The tools' buttons in the header, from here, each this wide.
const TOOLS_X: f64 = 440.0;
const TOOL_W: f64 = 44.0;
/// The time shown at first, s, and the bounds of zooming.
const SPAN_S: f64 = 10.0;
const SPAN_RANGE_S: (f64, f64) = (0.5, 600.0);
/// The pitch shown around the notes, cents.
const PITCH_PAD: f64 = 400.0;
const MIN_PITCH_SPAN: f64 = 1200.0;
/// Undo steps kept.
const UNDO_DEPTH: usize = 200;
/// A note's edges are grips this wide, px.
const EDGE_GRIP: f64 = 6.0;
/// Shift: drags this much finer.
const FINE: f64 = 0.1;
/// Drag sensitivity of the vertical tools: px for the whole range of vibrato and drift (0–2),
/// px per formant semitone, px per dB.
const AMOUNT_PX: f64 = 200.0;
const FORMANT_PX: f64 = 14.0;
const LEVEL_PX: f64 = 6.0;
const FORMANT_RANGE: f64 = 12.0;
const LEVEL_RANGE: (f64, f64) = (-36.0, 12.0);
const STRETCH_RANGE: (f64, f64) = (0.25, 4.0);

const NAMES: [&str; 12] = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];
const BLACK: [bool; 12] = [false, true, false, true, false, false, true, false, true, false, true, false];
const KEY_NAMES: [&str; 24] = [
    "C major", "C minor", "C# major", "C# minor", "D major", "D minor", "D# major", "D# minor", "E major", "E minor", "F major", "F minor", "F# major", "F# minor", "G major", "G minor", "G# major", "G# minor", "A major", "A minor", "A# major", "A# minor", "B major", "B minor",
];

/// The tools, in their order in the header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tool {
    Main,
    Vibrato,
    Drift,
    Formant,
    Level,
    Split,
}

pub const TOOLS: [Tool; 6] = [Tool::Main, Tool::Vibrato, Tool::Drift, Tool::Formant, Tool::Level, Tool::Split];

impl Tool {
    fn glyph(self) -> &'static str {
        match self {
            Tool::Main => "M7 4 V18.5 L10.6 15 L13.2 20.5 L15.4 19.5 L12.8 14 H18 Z",
            Tool::Vibrato => "M3 12 C5 6.5 7 6.5 9 12 C11 17.5 13 17.5 15 12 C17 6.5 19 6.5 21 12",
            Tool::Drift => "M3 8 C8 8 9.5 16 14 16 C17.5 16 18.5 12 21 12",
            Tool::Formant => "M3 18 C5 18 6 7 8.5 7 C11 7 11 14 13 14 C15 14 15.5 9.5 17.5 9.5 C19.5 9.5 20 18 21 18",
            Tool::Level => "M5.5 18 V14 M10 18 V10 M14.5 18 V7 M19 18 V4",
            Tool::Split => "M12 3 V21 M10 8.5 H7 C4.5 8.5 3 10 3 12 C3 14 4.5 15.5 7 15.5 H10 M14 8.5 H17 C19.5 8.5 21 10 21 12 C21 14 19.5 15.5 17 15.5 H14",
        }
    }

    fn words(self) -> &'static str {
        match self {
            Tool::Main => "Main (1) — drag a note: up and down its pitch, sideways its timing, its edges its length; double-click: onto the snap",
            Tool::Vibrato => "Vibrato (2) — drag a note up for more of its vibrato, down for less; double-click: as sung",
            Tool::Drift => "Drift (3) — drag a note up for more of its slow slides and sags, down for less; double-click: as sung",
            Tool::Formant => "Formant (4) — drag a note up or down to move its formants (the voice's size); double-click: as sung",
            Tool::Level => "Level (5) — drag a note up or down to make it louder or quieter; double-click: as sung",
            Tool::Split => "Split (6) — click a note to split it there; click where two notes meet to join them",
        }
    }
}

const SNAPS: [Snap; 3] = [Snap::Semitone, Snap::Scale, Snap::Off];

fn snap_glyph(s: Snap) -> &'static str {
    match s {
        Snap::Semitone => "M4 5 H20 V19 H4 Z M9.3 5 V13 M14.7 5 V13",
        Snap::Scale => "M4 19 H8 V15 H12 V11 H16 V7 H20",
        Snap::Off => "M3 14 C7 6 10 18 14 11 C16 7.5 18 9 21 9",
    }
}

fn snap_words(s: Snap) -> &'static str {
    match s {
        Snap::Semitone => "Snap to semitones — Alt while dragging: free",
        Snap::Scale => "Snap to the key's scale — Alt while dragging: free",
        Snap::Off => "Free — pitch moves by the cent; Alt while dragging: snap to semitones",
    }
}

pub fn memory_host() -> mite_ui::MemoryParamHost {
    let flag = |b: BoolSpec| ParamSpec { id: b.id, name: b.name, unit: "", min: 0.0, max: 1.0, default: f32::from(u8::from(b.default)), scale: ParamScale::Linear, smoothing_ms: 0.0, decimals: 0, wheel_step: 1.0, wheel_step_fine: 1.0, display: ParamDisplay::Number };
    mite_ui::MemoryParamHost::new().with(flag(BYPASS))
}

fn note_name(cents: f64) -> String {
    let k = (cents / 100.0).round() as i64;
    format!("{}{}", NAMES[k.rem_euclid(12) as usize], k.div_euclid(12) - 1)
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Hot {
    Note(usize),
    /// Where notes `i` and `i + 1` meet (split tool).
    Seam(usize),
    Edge(usize, bool),
    Transfer,
    Tool(Tool),
    Snap(Snap),
    KeyButton,
    Centre,
    Drift,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Grip {
    /// Main tool, the body: the axis (true: time) is chosen by the first few pixels.
    Body(Option<bool>),
    /// An edge (true: the right).
    Edge(bool),
    /// The vertical tools.
    Amount,
}

#[derive(Debug, Clone)]
struct Drag {
    grip: Grip,
    start: Point,
    /// The notes it moves and their edits when it began.
    notes: Vec<(usize, NoteEdit)>,
    undo: Snapshot,
}

#[derive(Debug, Clone)]
struct Macro {
    centre: bool,
    notes: Vec<(usize, NoteEdit)>,
    undo: Snapshot,
}

pub struct PitchView {
    shared: Arc<PitchShared>,
    footer: Footer,
    tool: Tool,
    /// The view: its first second (from the take's start), its span (s), its pitch range (cents).
    t0: f64,
    span: f64,
    pitch: (f64, f64),
    /// Whether the view was fitted to this take.
    fitted: bool,
    selected: BTreeSet<usize>,
    /// The notes' count the selection belongs to (a split or merge clears it).
    selected_of: usize,
    drag: Option<Drag>,
    box_from: Option<Point>,
    macro_drag: Option<Macro>,
    /// The macro sliders' values (0–1), until the selection changes.
    centre: f64,
    drift: f64,
    key_menu: Option<Dropdown>,
    hot: Option<Hot>,
    pointer: Option<Point>,
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
    seen_version: u64,
}

impl PitchView {
    pub fn new(shared: Arc<PitchShared>) -> Self {
        let footer = Footer::new(FOOTER_TOP, (VIEW_W, VIEW_H), vec![FooterToggle { id: BYPASS.id, glyph: ToggleGlyph::Bypass, words: "Bypass — the input as it comes (the take is kept)" }], vec![]);
        Self {
            shared,
            footer,
            tool: Tool::Main,
            t0: 0.0,
            span: SPAN_S,
            pitch: (4800.0, 7200.0),
            fitted: false,
            selected: BTreeSet::new(),
            selected_of: 0,
            drag: None,
            box_from: None,
            macro_drag: None,
            centre: 0.0,
            drift: 0.0,
            key_menu: None,
            hot: None,
            pointer: None,
            undo: Vec::new(),
            redo: Vec::new(),
            seen_version: 0,
        }
    }

    // ---- for tests ----------------------------------------------------------------------------

    pub fn transfer_button(&self) -> Rect {
        TRANSFER
    }

    pub fn tool_button(&self, t: Tool) -> Rect {
        let k = TOOLS.iter().position(|&x| x == t).unwrap_or(0) as f64;
        Rect::new(TOOLS_X + k * TOOL_W, 10.0, TOOLS_X + k * TOOL_W + TOOL_W - 6.0, 46.0)
    }

    pub fn snap_button(&self, s: Snap) -> Rect {
        let k = SNAPS.iter().position(|&x| x == s).unwrap_or(0) as f64;
        Rect::new(SNAP_X + k * SNAP_W, FOOTER_TOP + 12.0, SNAP_X + k * SNAP_W + SNAP_W - 4.0, FOOTER_TOP + 44.0)
    }

    pub fn key_button(&self) -> Rect {
        KEY_BUTTON
    }

    pub fn key_menu_rows(&self) -> Vec<Rect> {
        self.key_menu.as_ref().map(Dropdown::item_rects).unwrap_or_default()
    }

    pub fn centre_slider(&self) -> Rect {
        CENTRE_SLIDER
    }

    pub fn drift_slider(&self) -> Rect {
        DRIFT_SLIDER
    }

    pub fn tool(&self) -> Tool {
        self.tool
    }

    pub fn selected(&self) -> Vec<usize> {
        self.selected.iter().copied().collect()
    }

    /// The x of take time `t` (s) and the y of pitch `cents`.
    pub fn x_of(&self, t: f64) -> f64 {
        ROLL.x0 + (t - self.t0) / self.span * ROLL.width()
    }

    pub fn y_of(&self, cents: f64) -> f64 {
        ROLL.y1 - (cents - self.pitch.0) / (self.pitch.1 - self.pitch.0) * ROLL.height()
    }

    fn t_of(&self, x: f64) -> f64 {
        self.t0 + (x - ROLL.x0) / ROLL.width() * self.span
    }

    pub fn cents_per_px(&self) -> f64 {
        (self.pitch.1 - self.pitch.0) / ROLL.height()
    }

    fn row_h(&self) -> f64 {
        ROLL.height() / ((self.pitch.1 - self.pitch.0) / 100.0)
    }

    /// Fits the view to the notes (once per take).
    fn fit(&mut self, a: &Analysis) {
        if self.fitted || a.notes.is_empty() {
            return;
        }
        self.fitted = true;
        let lo = a.notes.iter().map(|n| n.cents).fold(f64::MAX, f64::min) - PITCH_PAD;
        let hi = a.notes.iter().map(|n| n.cents).fold(f64::MIN, f64::max) + PITCH_PAD;
        let mid = 0.5 * (lo + hi);
        let half = (0.5 * (hi - lo)).max(0.5 * MIN_PITCH_SPAN);
        self.pitch = (mid - half, mid + half);
        self.t0 = (a.notes[0].start as f64 / a.fs - 0.5).max(0.0);
        let last = a.notes.last().map_or(0.0, |n| n.end as f64 / a.fs);
        self.span = (last + 0.5 - self.t0).clamp(2.0, SPAN_S);
    }

    /// A note's rectangle as drawn (its edit applied).
    pub fn note_rect(&self, i: usize) -> Option<Rect> {
        let a = self.shared.session.analysis()?;
        let edits = self.shared.session.edits();
        Some(self.rect_of(a.notes.get(i)?, &edits.get(i).copied().unwrap_or_default(), a.fs))
    }

    fn span_of(n: &Note, e: &NoteEdit, fs: f64) -> (f64, f64) {
        let s = (n.start as f64 + e.offset) / fs;
        (s, s + (n.end - n.start) as f64 * e.stretch / fs)
    }

    fn rect_of(&self, n: &Note, e: &NoteEdit, fs: f64) -> Rect {
        let (s, t) = Self::span_of(n, e, fs);
        let yc = self.y_of(n.cents + e.shift_cents);
        let h = (self.row_h() * 0.8).clamp(6.0, 22.0);
        Rect::new(self.x_of(s), yc - h / 2.0, self.x_of(t).max(self.x_of(s) + 3.0), yc + h / 2.0)
    }

    fn hot_at(&self, p: Point) -> Option<Hot> {
        if TRANSFER.contains(p) {
            return Some(Hot::Transfer);
        }
        if let Some(&t) = TOOLS.iter().find(|&&t| self.tool_button(t).contains(p)) {
            return Some(Hot::Tool(t));
        }
        if let Some(&s) = SNAPS.iter().find(|&&s| self.snap_button(s).contains(p)) {
            return Some(Hot::Snap(s));
        }
        if KEY_BUTTON.contains(p) {
            return Some(Hot::KeyButton);
        }
        if CENTRE_SLIDER.inflate(8.0, 10.0).contains(p) {
            return Some(Hot::Centre);
        }
        if DRIFT_SLIDER.inflate(8.0, 10.0).contains(p) {
            return Some(Hot::Drift);
        }
        if !ROLL.contains(p) {
            return None;
        }
        let a = self.shared.session.analysis()?;
        let edits = self.shared.session.edits();
        let e = |i: usize| edits.get(i).copied().unwrap_or_default();
        if self.tool == Tool::Split {
            // Where two notes meet (as drawn), within a few pixels.
            for i in 0..a.notes.len().saturating_sub(1) {
                let (r0, r1) = (self.rect_of(&a.notes[i], &e(i), a.fs), self.rect_of(&a.notes[i + 1], &e(i + 1), a.fs));
                if (r1.x0 - r0.x1).abs() < 4.0 && (p.x - r0.x1).abs() < 5.0 && p.y > r0.y0.min(r1.y0) - 4.0 && p.y < r0.y1.max(r1.y1) + 4.0 {
                    return Some(Hot::Seam(i));
                }
            }
        }
        // The topmost (last drawn) note under the pointer, a little forgiving.
        let i = (0..a.notes.len()).rev().find(|&i| self.rect_of(&a.notes[i], &e(i), a.fs).inflate(2.0, 3.0).contains(p))?;
        let r = self.rect_of(&a.notes[i], &e(i), a.fs);
        if self.tool == Tool::Main && r.width() > 3.0 * EDGE_GRIP {
            if p.x < r.x0 + EDGE_GRIP {
                return Some(Hot::Edge(i, false));
            }
            if p.x > r.x1 - EDGE_GRIP {
                return Some(Hot::Edge(i, true));
            }
        }
        Some(Hot::Note(i))
    }

    fn snapshot(&self) -> Option<Snapshot> {
        self.shared.session.snapshot()
    }

    fn push_undo(&mut self, s: Snapshot) {
        self.undo.push(s);
        if self.undo.len() > UNDO_DEPTH {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    /// The selection, or every note when nothing is selected.
    fn targets(&self) -> Vec<usize> {
        let n = self.shared.session.analysis().map_or(0, |a| a.notes.len());
        if self.selected.is_empty() {
            (0..n).collect()
        } else {
            self.selected.iter().copied().filter(|&i| i < n).collect()
        }
    }

    fn select(&mut self, i: usize, mods: Modifiers) {
        if mods.ctrl {
            if !self.selected.insert(i) {
                self.selected.remove(&i);
            }
        } else if mods.shift {
            self.selected.insert(i);
        } else if !self.selected.contains(&i) {
            self.selected.clear();
            self.selected.insert(i);
        }
        self.centre = 0.0;
        self.drift = 0.0;
    }

    /// Where pitch snaps while dragging (Alt: the other way).
    fn snap_now(&self, mods: Modifiers) -> Snap {
        let s = self.shared.session.scale().snap;
        match (s, mods.alt) {
            (_, false) => s,
            (Snap::Off, true) => Snap::Semitone,
            (_, true) => Snap::Off,
        }
    }

    // ---- drawing ------------------------------------------------------------------------------

    fn draw_header(&self, cx: &mut DrawCx<'_>) {
        let theme = cx.theme;
        cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.window, None, &Rect::new(0.0, 0.0, VIEW_W, HEADER));
        cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, theme.line, None, &Line::new((0.0, HEADER - 0.5), (VIEW_W, HEADER - 0.5)));
        draw_mark(cx, 16.0, 15.0, 26.0);
        draw_title(cx, 52.0, 18.0, "Pitch");
        for t in TOOLS {
            let r = self.tool_button(t);
            let on = self.tool == t;
            let hot = self.hot == Some(Hot::Tool(t));
            let rr = RoundedRect::from_rect(r, 8.0);
            if on {
                cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.accent.multiply_alpha(0.16), None, &rr);
                cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, theme.accent.multiply_alpha(0.6), None, &rr);
            } else if hot {
                cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.panel_control, None, &rr);
            }
            let ink = if on { theme.accent } else if hot { theme.text_hi } else { theme.text_mid };
            draw_glyph(cx.scene, t.glyph(), r.center(), 22.0, ink, 1.6);
        }
        let s = &self.shared.session;
        let words = match s.status() {
            Status::Empty => "no take yet".to_owned(),
            Status::Armed => "armed: play the part".to_owned(),
            Status::Recording => "recording…".to_owned(),
            Status::Analysing => "finding the notes…".to_owned(),
            Status::Ready => match (s.take(), s.analysis()) {
                (Some(t), Some(a)) => format!("{} notes · {:.1} s", a.notes.len(), t.len as f64 / t.sample_rate),
                _ => String::new(),
            },
            Status::Problem(p) => p,
        };
        let words = fit(cx, &words, TextStyle::BODY, 400.0);
        cx.text.draw(cx.scene, &words, VIEW_W - 20.0, 20.0, TextStyle::BODY, theme.text_mid, Align::Right);
    }

    fn draw_grid(&self, cx: &mut DrawCx<'_>) {
        let theme = cx.theme;
        cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.display, None, &ROLL);
        cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.well, None, &KEYS);
        let scale = self.shared.session.scale();
        let rh = self.row_h();
        let first = (self.pitch.0 / 100.0).floor() as i64;
        let last = (self.pitch.1 / 100.0).ceil() as i64;
        for note in first..=last.min(first + 400) {
            let k = note.rem_euclid(12) as usize;
            let (y0, y1) = (self.y_of((note as f64 + 0.5) * 100.0), self.y_of((note as f64 - 0.5) * 100.0));
            let row = Rect::new(ROLL.x0, y0.max(ROLL.y0), ROLL.x1, y1.min(ROLL.y1));
            if row.height() <= 0.0 {
                continue;
            }
            // Out of the key: darker when snapping to it, a little otherwise.
            if !scale.contains(k) {
                let a = if scale.snap == Snap::Scale { 0.75 } else { 0.4 };
                cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.well.multiply_alpha(a), None, &row);
            }
            if BLACK[k] {
                cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.display, None, &Rect::new(KEYS.x0, row.y0, KEYS.x0 + 38.0, row.y1));
            }
            if k == scale.tonic {
                cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.accent.multiply_alpha(0.05), None, &row);
            }
            if k == 0 {
                cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, theme.grid_major, None, &Line::new((KEYS.x0, y1), (ROLL.x1, y1)));
            } else if rh >= 8.0 {
                cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, theme.grid_minor.multiply_alpha(0.5), None, &Line::new((ROLL.x0, y1), (ROLL.x1, y1)));
            }
            let mid = (y0 + y1) / 2.0;
            if (k == 0 || (rh >= 13.0 && !BLACK[k])) && mid > KEYS.y0 + 6.0 && mid < KEYS.y1 - 6.0 {
                let name = format!("{}{}", NAMES[k], note.div_euclid(12) - 1);
                cx.text.draw(cx.scene, &name, KEYS.x1 - 6.0, mid - 6.0, TextStyle::LABEL.sized(9.0), if k == 0 { theme.text_hi } else { theme.text_lo }, Align::Right);
            }
        }
        cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, theme.line, None, &Line::new((ROLL.x0 - 0.5, ROLL.y0), (ROLL.x0 - 0.5, ROLL.y1)));
        // The ruler: seconds from the take's start (a step that keeps labels apart).
        cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.well, None, &RULER);
        let step = [0.1, 0.25, 0.5, 1.0, 2.0, 5.0, 10.0, 15.0, 30.0, 60.0].into_iter().find(|s| s / self.span * ROLL.width() >= 70.0).unwrap_or(120.0);
        let mut k = (self.t0 / step).ceil() as i64;
        let mut guard = 0;
        while (k as f64) * step <= self.t0 + self.span && guard < 200 {
            let t = k as f64 * step;
            let x = self.x_of(t);
            cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, theme.grid_minor, None, &Line::new((x, ROLL.y0), (x, ROLL.y1)));
            if x + 44.0 > ROLL.x1 {
                break;
            }
            let label = if step < 1.0 { format!("{t:.2} s") } else { format!("{t:.0} s") };
            cx.text.draw(cx.scene, &label, x + 4.0, RULER.y0 + 6.0, TextStyle { tabular: true, ..TextStyle::BODY.sized(10.0) }, theme.text_lo, Align::Left);
            k += 1;
            guard += 1;
        }
    }

    fn draw_notes(&mut self, cx: &mut DrawCx<'_>) {
        let theme = cx.theme;
        let accent = theme.accent;
        let s = self.shared.session.clone();
        let Some(a) = s.analysis() else { return };
        self.fit(&a);
        let edits = s.edits();
        let loud = a.notes.iter().map(|n| n.db).fold(f64::MIN, f64::max);
        cx.begin_clip(&ROLL);
        let (v0, v1) = (self.t0, self.t0 + self.span);
        for (i, n) in a.notes.iter().enumerate() {
            let e = edits.get(i).copied().unwrap_or_default();
            let (ts, te) = Self::span_of(n, &e, a.fs);
            if te < v0 || ts > v1 {
                continue;
            }
            let r = self.rect_of(n, &e, a.fs);
            let sel = self.selected.contains(&i);
            let hot = matches!(self.hot, Some(Hot::Note(j) | Hot::Edge(j, _)) if j == i);
            // Where it was sung, when it has moved.
            if e.shift_cents.abs() > 1.0 || e.offset.abs() > 1.0 || (e.stretch - 1.0).abs() > 1e-3 {
                let (os, oe) = Self::span_of(n, &NoteEdit::default(), a.fs);
                let yc = self.y_of(n.cents);
                let ghost = Rect::new(self.x_of(os), yc - r.height() / 2.0, self.x_of(oe).max(self.x_of(os) + 3.0), yc + r.height() / 2.0);
                cx.scene.stroke(&Stroke::new(1.0).with_dashes(0.0, [3.0, 3.0]), Affine::IDENTITY, accent.multiply_alpha(0.35), None, &RoundedRect::from_rect(ghost, ghost.height() / 2.0));
            }
            // Thickness by level (as edited): the loudest note full, 30 dB under it half.
            let k = (0.5 + 0.5 * ((n.db + e.gain_db - loud + 30.0) / 30.0).clamp(0.0, 1.0)) * r.height() / 2.0;
            let body = Rect::new(r.x0, r.center().y - k, r.x1, r.center().y + k);
            let rr = RoundedRect::from_rect(body, body.height() / 2.0);
            cx.scene.fill(Fill::NonZero, Affine::IDENTITY, accent.multiply_alpha(if sel { 0.55 } else if hot { 0.42 } else { 0.28 }), None, &rr);
            if sel {
                glow_stroke(cx, &rr, 1.4, accent, accent, 2.5);
            } else {
                cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, accent.multiply_alpha(0.7), None, &rr);
            }
            // Formants moved: a bar across it, up or down by how far.
            if e.formant_semitones.abs() > 0.05 {
                let y = body.center().y - (e.formant_semitones / FORMANT_RANGE) * (body.height() / 2.0 - 1.0);
                cx.scene.stroke(&Stroke::new(1.6), Affine::IDENTITY, theme.text_strong.multiply_alpha(0.8), None, &Line::new((body.x0 + body.height() / 2.0, y), (body.x1 - body.height() / 2.0, y)));
            }
            // The edge grip, when hot.
            if let Some(Hot::Edge(j, right)) = self.hot {
                if j == i {
                    let x = if right { r.x1 - 2.0 } else { r.x0 + 2.0 };
                    cx.scene.stroke(&Stroke::new(2.0), Affine::IDENTITY, theme.text_strong, None, &Line::new((x, r.y0 - 2.0), (x, r.y1 + 2.0)));
                }
            }
            // The pitch curve, as it will play.
            let mut curve = BezPath::new();
            let mut started = false;
            for (f, d) in n.drift.iter().enumerate() {
                let t_in = (f * a.hop) as f64;
                let voiced = a.frames.get((n.start + f * a.hop) / a.hop).is_some_and(|fr| fr.cents.is_some());
                if !voiced {
                    started = false;
                    continue;
                }
                let sl = n.slow.get(f).copied().unwrap_or(0.0);
                let c = n.cents + e.shift_cents + e.drift * sl + e.modulation * (d - sl);
                let p = Point::new(self.x_of((n.start as f64 + e.offset + t_in * e.stretch) / a.fs), self.y_of(c));
                if started {
                    curve.line_to(p);
                } else {
                    curve.move_to(p);
                    started = true;
                }
            }
            let emph = sel || matches!(self.tool, Tool::Vibrato | Tool::Drift);
            cx.scene.stroke(&Stroke::new(if emph { 1.4 } else { 1.1 }), Affine::IDENTITY, theme.text_strong.multiply_alpha(if emph { 0.95 } else { 0.55 }), None, &curve);
        }
        // Where two notes meet (split tool, hot): the join.
        if let Some(Hot::Seam(i)) = self.hot {
            if let Some(r) = self.note_rect(i) {
                cx.scene.stroke(&Stroke::new(2.0).with_dashes(0.0, [3.0, 3.0]), Affine::IDENTITY, theme.text_strong, None, &Line::new((r.x1, r.y0 - 8.0), (r.x1, r.y1 + 8.0)));
            }
        }
        // The split tool's cut, where it would go.
        if let (Tool::Split, Some(Hot::Note(i)), Some(p)) = (self.tool, self.hot, self.pointer) {
            if let Some(r) = self.note_rect(i) {
                cx.scene.stroke(&Stroke::new(1.5), Affine::IDENTITY, theme.text_strong, None, &Line::new((p.x, r.y0 - 6.0), (p.x, r.y1 + 6.0)));
            }
        }
        // The box being dragged.
        if let (Some(a), Some(b)) = (self.box_from, self.pointer) {
            let r = Rect::from_points(a, b);
            cx.scene.fill(Fill::NonZero, Affine::IDENTITY, accent.multiply_alpha(0.08), None, &r);
            cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, accent.multiply_alpha(0.6), None, &r);
        }
        // The playhead.
        if let Some(t) = s.take() {
            let at = (s.playhead.load(Ordering::Relaxed) - t.start) as f64 / t.sample_rate;
            if (v0..=v1).contains(&at) {
                let x = self.x_of(at);
                cx.scene.stroke(&Stroke::new(1.4), Affine::IDENTITY, theme.text_hi, None, &Line::new((x, ROLL.y0), (x, ROLL.y1)));
            }
        }
        cx.end_clip();
    }

    fn draw_empty(&self, cx: &mut DrawCx<'_>) {
        let theme = cx.theme;
        let (title, words) = match self.shared.session.status() {
            Status::Empty => ("No take yet", "Click TRANSFER, then play the part you want to edit: it is recorded from where the transport starts until it stops."),
            Status::Armed => ("Armed", "Press play in the host: the take is recorded from where the transport starts until it stops."),
            Status::Recording => ("Recording", "Stop the transport to finish the take."),
            Status::Analysing => ("Finding the notes", "A moment: the take is being analysed."),
            Status::Problem(_) => ("No take", "See the line above."),
            Status::Ready => return,
        };
        let c = ROLL.center();
        cx.text.draw(cx.scene, title, c.x, c.y - 30.0, TextStyle::BODY.sized(20.0).weighted(700.0), theme.text_hi, Align::Center);
        let words = fit(cx, words, TextStyle::BODY.sized(13.0), ROLL.width() - 80.0);
        cx.text.draw(cx.scene, &words, c.x, c.y + 4.0, TextStyle::BODY.sized(13.0), theme.text_lo, Align::Center);
    }

    fn draw_footer_controls(&self, cx: &mut DrawCx<'_>) {
        let theme = cx.theme;
        // TRANSFER.
        let status = self.shared.session.status();
        let on = matches!(status, Status::Armed | Status::Recording);
        let hot = self.hot == Some(Hot::Transfer);
        let rr = RoundedRect::from_rect(TRANSFER, TRANSFER.height() / 2.0);
        let ink = if matches!(status, Status::Recording) { theme.danger } else { theme.accent };
        cx.scene.fill(Fill::NonZero, Affine::IDENTITY, if on { ink.multiply_alpha(0.2) } else { theme.well }, None, &rr);
        if on {
            glow_stroke(cx, &rr, 1.2, ink, ink, 2.5);
        } else {
            cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, if hot { theme.text_hi } else { theme.well_border }, None, &rr);
        }
        let label = match status {
            Status::Recording => "RECORDING",
            Status::Armed => "ARMED",
            _ => "TRANSFER",
        };
        let c = TRANSFER.center();
        cx.text.draw(cx.scene, label, c.x, c.y - 6.0, TextStyle::LABEL.sized(10.0), if on { ink } else if hot { theme.text_hi } else { theme.text_mid }, Align::Center);
        // The snap.
        let scale = self.shared.session.scale();
        for s in SNAPS {
            let r = self.snap_button(s);
            let on = scale.snap == s;
            let hot = self.hot == Some(Hot::Snap(s));
            let rr = RoundedRect::from_rect(r, 7.0);
            if on {
                cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.accent.multiply_alpha(0.16), None, &rr);
                cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, theme.accent.multiply_alpha(0.6), None, &rr);
            } else if hot {
                cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.panel_control, None, &rr);
            }
            draw_glyph(cx.scene, snap_glyph(s), r.center(), 18.0, if on { theme.accent } else if hot { theme.text_hi } else { theme.text_mid }, 1.5);
        }
        // The key.
        let key = KEY_NAMES[scale.tonic * 2 + usize::from(scale.minor)];
        let accent = theme.accent;
        draw_dropdown_button(cx, KEY_BUTTON, key, self.key_menu.is_some(), self.hot == Some(Hot::KeyButton), |scene, at| draw_glyph(scene, snap_glyph(Snap::Scale), at + (8.0, 6.0), 18.0, accent, 1.5));
        // The Correct macro.
        for (r, v, name, h) in [(CENTRE_SLIDER, self.centre, "CENTRE", Hot::Centre), (DRIFT_SLIDER, self.drift, "DRIFT", Hot::Drift)] {
            let hot = self.hot == Some(h) || self.macro_drag.as_ref().is_some_and(|m| m.centre == (h == Hot::Centre));
            cx.text.draw(cx.scene, name, r.x0 - 12.0, r.center().y - 6.0, TextStyle::LABEL.sized(10.0), if hot { theme.text_hi } else { theme.text_mid }, Align::Right);
            let track = RoundedRect::from_rect(Rect::new(r.x0, r.center().y - 2.0, r.x1, r.center().y + 2.0), 2.0);
            cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.grid_major, None, &track);
            let x = r.x0 + v * r.width();
            if v > 0.0 {
                cx.scene.fill(Fill::NonZero, Affine::IDENTITY, accent, None, &RoundedRect::from_rect(Rect::new(r.x0, r.center().y - 2.0, x, r.center().y + 2.0), 2.0));
            }
            let knob = Circle::new((x, r.center().y), if hot { 7.0 } else { 6.0 });
            cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.knob_disc, None, &knob);
            cx.scene.stroke(&Stroke::new(1.2), Affine::IDENTITY, if v > 0.0 || hot { accent } else { theme.knob_disc_border }, None, &knob);
        }
    }

    fn help_text(&self, host: &dyn ParamHost) -> Help {
        if let Some(h) = self.footer.help(host) {
            return h;
        }
        let s = &self.shared.session;
        let note = self.drag.as_ref().and_then(|d| d.notes.first().map(|n| n.0)).or(match self.hot {
            Some(Hot::Note(i) | Hot::Edge(i, _)) => Some(i),
            _ => None,
        });
        if let (Some(i), Some(a)) = (note, s.analysis()) {
            if let Some(n) = a.notes.get(i) {
                let e = s.edits().get(i).copied().unwrap_or_default();
                let c = n.cents + e.shift_cents;
                let off = c - (c / 100.0).round() * 100.0;
                let name = note_name(c);
                let what = match self.tool {
                    Tool::Main => Help::default().text(format!("{name} ")).slot(format!("{off:+.0} cents"), "+50 cents").text(format!(" · sung {} {:+.0} · ", note_name(n.cents), n.cents - (n.cents / 100.0).round() * 100.0)).slot(format!("{:.2} s", (n.end - n.start) as f64 * e.stretch / a.fs), "10.00 s"),
                    Tool::Vibrato => Help::default().text(format!("{name} · vibrato ")).slot(format!("{:.0} %", e.modulation * 100.0), "200 %"),
                    Tool::Drift => Help::default().text(format!("{name} · drift ")).slot(format!("{:.0} %", e.drift * 100.0), "200 %"),
                    Tool::Formant => Help::default().text(format!("{name} · formant ")).slot(format!("{:+.1} st", e.formant_semitones), "+12.0 st"),
                    Tool::Level => Help::default().text(format!("{name} · level ")).slot(format!("{:+.1} dB", e.gain_db), "−36.0 dB"),
                    Tool::Split => Help::default().text(format!("{name} — click to split it here")),
                };
                return what.text(if self.drag.is_some() || self.tool == Tool::Split { "" } else { " — Shift: fine" });
            }
        }
        match self.hot {
            Some(Hot::Seam(_)) => Help::default().text("Click to join these two notes"),
            Some(Hot::Transfer) => Help::default().text("Transfer — record the next stretch the transport plays (it replaces the take); click again to cancel"),
            Some(Hot::Tool(t)) => Help::default().text(t.words()),
            Some(Hot::Snap(sn)) => Help::default().text(snap_words(sn)),
            Some(Hot::KeyButton) => Help::default().text("The key (found from the notes): its scale shades the grid and is what the scale snap snaps to"),
            Some(Hot::Centre) => Help::default().text("Correct, centre — pull the selected notes (or every note) onto the snap, by this much"),
            Some(Hot::Drift) => Help::default().text("Correct, drift — take the slow slides out of the selected notes (or every note), by this much"),
            _ => Help::default().text(self.tool.words()),
        }
    }

    // ---- editing ------------------------------------------------------------------------------

    fn begin_drag(&mut self, grip: Grip, pos: Point) {
        let Some(undo) = self.snapshot() else { return };
        let edits = self.shared.session.edits();
        let notes = self.selected.iter().filter_map(|&i| edits.get(i).map(|e| (i, *e))).collect();
        self.drag = Some(Drag { grip, start: pos, notes, undo });
    }

    fn drag_to(&mut self, pos: Point, mods: Modifiers) {
        let Some(mut d) = self.drag.clone() else { return };
        let s = self.shared.session.clone();
        let Some(a) = s.analysis() else { return };
        let fine = if mods.fine() { FINE } else { 1.0 };
        let (dx, dy) = ((pos.x - d.start.x) * fine, (pos.y - d.start.y) * fine);
        let dt = dx / ROLL.width() * self.span * a.fs;
        let snap = self.snap_now(mods);
        let scale = s.scale();
        if let Grip::Body(None) = d.grip {
            if (pos.x - d.start.x).abs().max((pos.y - d.start.y).abs()) < 4.0 {
                return;
            }
            d.grip = Grip::Body(Some((pos.x - d.start.x).abs() > (pos.y - d.start.y).abs()));
            if let Some(cur) = self.drag.as_mut() {
                cur.grip = d.grip;
            }
        }
        let changes: Vec<(usize, NoteEdit)> = d
            .notes
            .iter()
            .filter_map(|&(i, e0)| {
                let n = a.notes.get(i)?;
                let mut e = e0;
                let len = (n.end - n.start) as f64;
                match (d.grip, self.tool) {
                    (Grip::Body(Some(true)), _) => e.offset = e0.offset + dt,
                    (Grip::Body(Some(false)), _) => {
                        let target = scale.snap(n.cents + e0.shift_cents - dy * self.cents_per_px(), snap);
                        e.shift_cents = target - n.cents;
                    }
                    (Grip::Body(None), _) => {}
                    (Grip::Edge(true), _) => e.stretch = ((len * e0.stretch + dt) / len).clamp(STRETCH_RANGE.0, STRETCH_RANGE.1),
                    (Grip::Edge(false), _) => {
                        let stretch = ((len * e0.stretch - dt) / len).clamp(STRETCH_RANGE.0, STRETCH_RANGE.1);
                        // The end stays: the start moves by what the length lost.
                        e.offset = e0.offset + len * (e0.stretch - stretch);
                        e.stretch = stretch;
                    }
                    (Grip::Amount, Tool::Vibrato) => e.modulation = (e0.modulation - dy / AMOUNT_PX * 2.0).clamp(0.0, 2.0),
                    (Grip::Amount, Tool::Drift) => e.drift = (e0.drift - dy / AMOUNT_PX * 2.0).clamp(0.0, 2.0),
                    (Grip::Amount, Tool::Formant) => e.formant_semitones = (e0.formant_semitones - dy / FORMANT_PX).clamp(-FORMANT_RANGE, FORMANT_RANGE),
                    (Grip::Amount, Tool::Level) => e.gain_db = (e0.gain_db - dy / LEVEL_PX).clamp(LEVEL_RANGE.0, LEVEL_RANGE.1),
                    (Grip::Amount, _) => {}
                }
                Some((i, e))
            })
            .collect();
        s.set_many(&changes);
    }

    fn end_drag(&mut self) {
        if let Some(d) = self.drag.take() {
            if self.snapshot().is_some_and(|now| now != d.undo) {
                self.push_undo(d.undo);
            }
        }
    }

    /// The macro at `v` (0–1) over the notes it began with.
    fn macro_to(&mut self, v: f64) {
        let Some(m) = self.macro_drag.clone() else { return };
        let s = self.shared.session.clone();
        let Some(a) = s.analysis() else { return };
        let scale = s.scale();
        let snap = if scale.snap == Snap::Off { Snap::Semitone } else { scale.snap };
        let changes: Vec<(usize, NoteEdit)> = m
            .notes
            .iter()
            .filter_map(|&(i, e0)| {
                let n = a.notes.get(i)?;
                let mut e = e0;
                if m.centre {
                    let now = n.cents + e0.shift_cents;
                    e.shift_cents = e0.shift_cents + (scale.snap(now, snap) - now) * v;
                } else {
                    e.drift = e0.drift * (1.0 - v);
                }
                Some((i, e))
            })
            .collect();
        if m.centre {
            self.centre = v;
        } else {
            self.drift = v;
        }
        s.set_many(&changes);
    }

    fn handle_event(&mut self, event: &UiEvent, host: &dyn ParamHost) -> bool {
        if let Some(menu) = &mut self.key_menu {
            match menu.event(event) {
                DropdownEvent::Picked(k) => {
                    let mut sc = self.shared.session.scale();
                    sc.tonic = k / 2;
                    sc.minor = k % 2 == 1;
                    self.shared.session.set_scale(sc);
                    self.key_menu = None;
                }
                DropdownEvent::Closed => self.key_menu = None,
                DropdownEvent::Open => {}
            }
            return true;
        }
        if self.footer.event(event, host) {
            return true;
        }
        let session = self.shared.session.clone();
        match *event {
            UiEvent::KeyDown { key: Key::Character(c), mods } if mods.ctrl && matches!(c, 'z' | 'Z' | 'y' | 'Y') => {
                let redo = matches!(c, 'y' | 'Y') || mods.shift;
                let Some(now) = self.snapshot() else { return true };
                let from = if redo { self.redo.pop() } else { self.undo.pop() };
                if let Some(snap) = from {
                    if redo {
                        self.undo.push(now);
                    } else {
                        self.redo.push(now);
                    }
                    session.restore(&snap);
                }
                true
            }
            UiEvent::KeyDown { key: Key::Character('a' | 'A'), mods } if mods.ctrl => {
                self.selected = (0..session.analysis().map_or(0, |a| a.notes.len())).collect();
                true
            }
            UiEvent::KeyDown { key: Key::Escape, .. } => {
                self.selected.clear();
                true
            }
            UiEvent::KeyDown { key: Key::Character(c), mods } if !mods.ctrl && ('1'..='6').contains(&c) => {
                self.tool = TOOLS[(c as u8 - b'1') as usize];
                true
            }
            UiEvent::PointerMove { pos, mods } => {
                self.pointer = Some(pos);
                if self.drag.is_some() {
                    self.drag_to(pos, mods);
                    return true;
                }
                if let Some(m) = &self.macro_drag {
                    let r = if m.centre { CENTRE_SLIDER } else { DRIFT_SLIDER };
                    self.macro_to(((pos.x - r.x0) / r.width()).clamp(0.0, 1.0));
                    return true;
                }
                self.hot = if self.box_from.is_some() { None } else { self.hot_at(pos) };
                true
            }
            UiEvent::PointerLeft => {
                self.pointer = None;
                self.hot = None;
                true
            }
            UiEvent::PointerDown { pos, button: MouseButton::Left, mods } => match self.hot_at(pos) {
                Some(Hot::Transfer) => {
                    let armed = matches!(session.status(), Status::Armed);
                    session.set_armed(!armed);
                    true
                }
                Some(Hot::Tool(t)) => {
                    self.tool = t;
                    true
                }
                Some(Hot::Snap(sn)) => {
                    let mut sc = session.scale();
                    sc.snap = sn;
                    session.set_scale(sc);
                    true
                }
                Some(Hot::KeyButton) => {
                    let sc = session.scale();
                    self.key_menu = Some(Dropdown::new(KEY_BUTTON, &KEY_NAMES, sc.tonic * 2 + usize::from(sc.minor), (DISPLAY_TOP, FOOTER_TOP)).with_min_width(170.0));
                    true
                }
                Some(h @ (Hot::Centre | Hot::Drift)) => {
                    let Some(undo) = self.snapshot() else { return true };
                    let edits = session.edits();
                    let notes = self.targets().into_iter().filter_map(|i| edits.get(i).map(|e| (i, *e))).collect();
                    let centre = h == Hot::Centre;
                    self.macro_drag = Some(Macro { centre, notes, undo });
                    let r = if centre { CENTRE_SLIDER } else { DRIFT_SLIDER };
                    self.macro_to(((pos.x - r.x0) / r.width()).clamp(0.0, 1.0));
                    true
                }
                Some(Hot::Seam(i)) => {
                    if let Some(undo) = self.snapshot() {
                        if session.merge(i) {
                            self.push_undo(undo);
                            self.selected.clear();
                        }
                    }
                    true
                }
                Some(Hot::Note(i)) if self.tool == Tool::Split => {
                    let (Some(a), Some(undo)) = (session.analysis(), self.snapshot()) else { return true };
                    let e = session.edits().get(i).copied().unwrap_or_default();
                    let n = &a.notes[i];
                    // The pointer's time, back in the take (undoing the note's move and stretch).
                    let t_out = self.t_of(pos.x) * a.fs;
                    let at = n.start as f64 + (t_out - n.start as f64 - e.offset) / e.stretch;
                    if session.split(i, at.max(0.0) as usize) {
                        self.push_undo(undo);
                        self.selected.clear();
                    }
                    true
                }
                Some(Hot::Note(i)) => {
                    self.select(i, mods);
                    if self.selected.contains(&i) {
                        let grip = if self.tool == Tool::Main { Grip::Body(None) } else { Grip::Amount };
                        self.begin_drag(grip, pos);
                    }
                    true
                }
                Some(Hot::Edge(i, right)) => {
                    self.select(i, mods);
                    if self.selected.contains(&i) {
                        self.begin_drag(Grip::Edge(right), pos);
                    }
                    true
                }
                None if ROLL.contains(pos) => {
                    if !mods.shift && !mods.ctrl {
                        self.selected.clear();
                    }
                    self.box_from = Some(pos);
                    true
                }
                None => false,
            },
            UiEvent::PointerUp { pos, button: MouseButton::Left, .. } => {
                if self.drag.is_some() {
                    self.end_drag();
                    return true;
                }
                if let Some(m) = self.macro_drag.take() {
                    if self.snapshot().is_some_and(|now| now != m.undo) {
                        self.push_undo(m.undo);
                    }
                    return true;
                }
                if let Some(from) = self.box_from.take() {
                    let r = Rect::from_points(from, pos);
                    if let Some(a) = session.analysis() {
                        let edits = session.edits();
                        for (i, n) in a.notes.iter().enumerate() {
                            let nr = self.rect_of(n, &edits.get(i).copied().unwrap_or_default(), a.fs);
                            if r.width() > 2.0 && nr.intersect(r).area() > 0.0 {
                                self.selected.insert(i);
                            }
                        }
                    }
                    self.hot = self.hot_at(pos);
                    return true;
                }
                false
            }
            UiEvent::DoubleClick { pos, mods } => match self.hot_at(pos) {
                Some(Hot::Note(i) | Hot::Edge(i, _)) if self.tool != Tool::Split => {
                    self.drag = None;
                    let Some(a) = session.analysis() else { return true };
                    // The first click's drag left its own undo step only if something moved.
                    let Some(undo) = self.snapshot() else { return true };
                    if !self.selected.contains(&i) {
                        self.select(i, mods);
                    }
                    let scale = session.scale();
                    let snap = if scale.snap == Snap::Off { Snap::Semitone } else { scale.snap };
                    let edits = session.edits();
                    let d = NoteEdit::default();
                    let changes: Vec<(usize, NoteEdit)> = self
                        .selected
                        .iter()
                        .filter_map(|&j| {
                            let (n, mut e) = (a.notes.get(j)?, *edits.get(j)?);
                            match self.tool {
                                Tool::Main => e.shift_cents = scale.snap(n.cents + e.shift_cents, snap) - n.cents,
                                Tool::Vibrato => e.modulation = d.modulation,
                                Tool::Drift => e.drift = d.drift,
                                Tool::Formant => e.formant_semitones = d.formant_semitones,
                                Tool::Level => e.gain_db = d.gain_db,
                                Tool::Split => {}
                            }
                            Some((j, e))
                        })
                        .collect();
                    session.set_many(&changes);
                    if self.snapshot().is_some_and(|now| now != undo) {
                        self.push_undo(undo);
                    }
                    true
                }
                _ => false,
            },
            UiEvent::Wheel { pos, lines, mods } if ROLL.contains(pos) || RULER.contains(pos) || KEYS.contains(pos) => {
                let take_s = session.take().map_or(SPAN_S, |t| t.len as f64 / t.sample_rate);
                if mods.ctrl {
                    let at = self.t_of(pos.x);
                    let span = (self.span * 0.85f64.powf(f64::from(lines))).clamp(SPAN_RANGE_S.0, SPAN_RANGE_S.1.max(take_s));
                    self.t0 = (at - (pos.x - ROLL.x0) / ROLL.width() * span).max(0.0);
                    self.span = span;
                } else if mods.shift || KEYS.contains(pos) {
                    let d = f64::from(lines) * 100.0;
                    self.pitch = (self.pitch.0 + d, self.pitch.1 + d);
                } else {
                    self.t0 = (self.t0 - f64::from(lines) * 0.1 * self.span).clamp(0.0, (take_s - 0.5 * self.span).max(0.0));
                }
                true
            }
            _ => false,
        }
    }
}

/// `text` cut to fit `width` (an ellipsis when cut); each step drops a character of the text, so
/// it always ends.
fn fit(cx: &mut DrawCx<'_>, text: &str, style: TextStyle, width: f64) -> String {
    if f64::from(cx.text.measure(text, style)) <= width {
        return text.to_owned();
    }
    let mut chars: Vec<char> = text.chars().collect();
    while !chars.is_empty() {
        chars.pop();
        let cut = chars.iter().collect::<String>().trim_end().to_owned() + "…";
        if f64::from(cx.text.measure(&cut, style)) <= width {
            return cut;
        }
    }
    "…".to_owned()
}

impl View for PitchView {
    fn logical_size(&self) -> (f64, f64) {
        (VIEW_W, VIEW_H)
    }

    fn draw(&mut self, cx: &mut DrawCx<'_>) {
        let theme = cx.theme;
        self.draw_header(cx);
        cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.display, None, &Rect::new(0.0, DISPLAY_TOP, VIEW_W, HELP_TOP));
        self.draw_grid(cx);
        self.draw_notes(cx);
        self.draw_empty(cx);
        self.help_text(cx.host).draw(cx, Rect::new(0.0, HELP_TOP, VIEW_W, FOOTER_TOP));
        self.footer.draw_frame(cx);
        self.footer.draw_controls(cx);
        self.draw_footer_controls(cx);
        if let Some(menu) = &self.key_menu {
            let accent = cx.theme.accent;
            menu.draw(cx, accent, |_, _, _, _| {});
        }
    }

    fn event(&mut self, event: &UiEvent, host: &dyn ParamHost) -> bool {
        self.handle_event(event, host)
    }

    fn tick(&mut self, _host: &dyn ParamHost) -> bool {
        let s = self.shared.session.clone();
        let changed = s.poll();
        let version = s.version.load(Ordering::Acquire);
        let fresh = version != self.seen_version;
        self.seen_version = version;
        // Splits and merges renumber the notes: the selection goes.
        let n = s.analysis().map_or(0, |a| a.notes.len());
        if n != self.selected_of {
            self.selected.clear();
            self.selected_of = n;
        }
        // A new take: fit the view again.
        if matches!(s.status(), Status::Analysing | Status::Recording) {
            self.fitted = false;
        }
        // Follow the playhead a page at a time while the take plays.
        let playing = s.playing.load(Ordering::Relaxed);
        if playing && self.drag.is_none() {
            if let Some(t) = s.take() {
                let at = (s.playhead.load(Ordering::Relaxed) - t.start) as f64 / t.sample_rate;
                if (at > self.t0 + self.span * 0.95 || at < self.t0) && at >= 0.0 && at <= t.len as f64 / t.sample_rate {
                    self.t0 = (at - 0.05 * self.span).max(0.0);
                }
            }
        }
        changed || fresh || playing || s.busy() || matches!(s.status(), Status::Armed | Status::Recording | Status::Analysing)
    }
}
