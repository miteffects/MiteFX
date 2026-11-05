//! The Mite Pitch editor (docs/design/pitch-brief.md §3), in 1280×760 design units: the take's
//! notes as blobs on a pitch grid (time across, pitch up, a blob's thickness its level), each with
//! its pitch curve, the piano down the left, a time ruler, the playhead. Drag a note up or down:
//! its pitch (snapped to semitones, Alt free); double-click: centred on its semitone. Wheel:
//! scroll in time (Ctrl: zoom, Shift: scroll in pitch). Ctrl+Z / Ctrl+Shift+Z: undo, redo. The
//! footer's TRANSFER arms the recording of the next stretch the transport plays.

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Instant;

use mite_core::{BoolSpec, ParamDisplay, ParamScale, ParamSpec};
use mite_dsp::melody::{Analysis, NoteEdit};
use mite_ui::kurbo::{Affine, BezPath, Line, Point, Rect, RoundedRect, Stroke};
use mite_ui::paint::glow_stroke;
use mite_ui::peniko::Fill;
use mite_ui::widgets::brand::{draw_mark, draw_title};
use mite_ui::widgets::{Footer, FooterToggle, Help, ToggleGlyph};
use mite_ui::{Align, DrawCx, Key, MouseButton, ParamHost, TextStyle, UiEvent, View};

use crate::PitchShared;
use crate::params::BYPASS;
use crate::session::Status;

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
const TRANSFER: Rect = Rect::new(16.0, FOOTER_TOP + 14.0, 176.0, FOOTER_TOP + 42.0);
/// The time shown at first, s, and the bounds of zooming.
const SPAN_S: f64 = 10.0;
const SPAN_RANGE_S: (f64, f64) = (0.5, 600.0);
/// The pitch shown around the notes, cents.
const PITCH_PAD: f64 = 400.0;
const MIN_PITCH_SPAN: f64 = 1200.0;
/// Undo steps kept.
const UNDO_DEPTH: usize = 200;

const NAMES: [&str; 12] = [
    "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
];
const BLACK: [bool; 12] = [
    false, true, false, true, false, false, true, false, true, false, true, false,
];

pub fn memory_host() -> mite_ui::MemoryParamHost {
    let flag = |b: BoolSpec| ParamSpec {
        id: b.id,
        name: b.name,
        unit: "",
        min: 0.0,
        max: 1.0,
        default: f32::from(u8::from(b.default)),
        scale: ParamScale::Linear,
        smoothing_ms: 0.0,
        decimals: 0,
        wheel_step: 1.0,
        wheel_step_fine: 1.0,
        display: ParamDisplay::Number,
    };
    mite_ui::MemoryParamHost::new().with(flag(BYPASS))
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Hot {
    Note(usize),
    Transfer,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Drag {
    note: usize,
    start_y: f64,
    start_shift: f64,
}

pub struct PitchView {
    shared: Arc<PitchShared>,
    footer: Footer,
    /// The view: its first second (from the take's start), its span (s), its pitch range (cents).
    t0: f64,
    span: f64,
    pitch: (f64, f64),
    /// The analysis the pitch range was fitted to.
    fitted: Option<usize>,
    selected: Option<usize>,
    drag: Option<Drag>,
    hot: Option<Hot>,
    pointer: Option<Point>,
    undo: Vec<Vec<NoteEdit>>,
    redo: Vec<Vec<NoteEdit>>,
    seen_version: u64,
    last_tick: Instant,
}

impl PitchView {
    pub fn new(shared: Arc<PitchShared>) -> Self {
        let footer = Footer::new(
            FOOTER_TOP,
            (VIEW_W, VIEW_H),
            vec![FooterToggle {
                id: BYPASS.id,
                glyph: ToggleGlyph::Bypass,
                words: "Bypass — the input as it comes (the take is kept)",
            }],
            vec![],
        );
        Self {
            shared,
            footer,
            t0: 0.0,
            span: SPAN_S,
            pitch: (4800.0, 7200.0),
            fitted: None,
            selected: None,
            drag: None,
            hot: None,
            pointer: None,
            undo: Vec::new(),
            redo: Vec::new(),
            seen_version: 0,
            last_tick: Instant::now(),
        }
    }

    pub fn transfer_button(&self) -> Rect {
        TRANSFER
    }

    pub fn selected(&self) -> Option<usize> {
        self.selected
    }

    fn fs(&self) -> f64 {
        self.shared
            .session
            .take()
            .map_or(48_000.0, |t| t.sample_rate)
    }

    fn x_of(&self, t: f64) -> f64 {
        ROLL.x0 + (t - self.t0) / self.span * ROLL.width()
    }

    fn y_of(&self, cents: f64) -> f64 {
        ROLL.y1 - (cents - self.pitch.0) / (self.pitch.1 - self.pitch.0) * ROLL.height()
    }

    fn row_h(&self) -> f64 {
        ROLL.height() / ((self.pitch.1 - self.pitch.0) / 100.0)
    }

    /// Fits the pitch range to the notes (once per analysis).
    fn fit(&mut self, a: &Analysis) {
        let key = a.notes.len() ^ a.frames.len();
        if self.fitted == Some(key) || a.notes.is_empty() {
            return;
        }
        self.fitted = Some(key);
        let lo = a.notes.iter().map(|n| n.cents).fold(f64::MAX, f64::min) - PITCH_PAD;
        let hi = a.notes.iter().map(|n| n.cents).fold(f64::MIN, f64::max) + PITCH_PAD;
        let mid = 0.5 * (lo + hi);
        let half = (0.5 * (hi - lo)).max(0.5 * MIN_PITCH_SPAN);
        self.pitch = (mid - half, mid + half);
        // Start at the first note; a short take fills the view.
        self.t0 = (a.notes[0].start as f64 / a.fs - 0.5).max(0.0);
        let last = a.notes.last().map_or(0.0, |n| n.end as f64 / a.fs);
        self.span = (last + 0.5 - self.t0).clamp(2.0, SPAN_S);
    }

    /// A note's rectangle as drawn (its edit applied), if any of it is in view.
    pub fn note_rect(&self, i: usize) -> Option<Rect> {
        let a = self.shared.session.analysis()?;
        let edits = self.shared.session.edits();
        let (n, e) = (a.notes.get(i)?, edits.get(i).copied().unwrap_or_default());
        Some(self.rect_of(n, &e, a.fs))
    }

    fn rect_of(&self, n: &mite_dsp::melody::Note, e: &NoteEdit, fs: f64) -> Rect {
        let s = (n.start as f64 + e.offset) / fs;
        let len = (n.end - n.start) as f64 * e.stretch / fs;
        let yc = self.y_of(n.cents + e.shift_cents);
        let h = (self.row_h() * 0.8).clamp(6.0, 22.0);
        Rect::new(
            self.x_of(s),
            yc - h / 2.0,
            self.x_of(s + len).max(self.x_of(s) + 3.0),
            yc + h / 2.0,
        )
    }

    fn hot_at(&self, p: Point) -> Option<Hot> {
        if TRANSFER.contains(p) {
            return Some(Hot::Transfer);
        }
        if !ROLL.contains(p) {
            return None;
        }
        let a = self.shared.session.analysis()?;
        let edits = self.shared.session.edits();
        // The topmost (last drawn) note under the pointer, a little forgiving.
        (0..a.notes.len())
            .rev()
            .find(|&i| {
                self.rect_of(
                    &a.notes[i],
                    &edits.get(i).copied().unwrap_or_default(),
                    a.fs,
                )
                .inflate(2.0, 3.0)
                .contains(p)
            })
            .map(Hot::Note)
    }

    fn push_undo(&mut self) {
        self.undo.push(self.shared.session.edits());
        if self.undo.len() > UNDO_DEPTH {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    // ---- drawing ------------------------------------------------------------------------------

    fn draw_header(&self, cx: &mut DrawCx<'_>) {
        let theme = cx.theme;
        cx.scene.fill(
            Fill::NonZero,
            Affine::IDENTITY,
            theme.window,
            None,
            &Rect::new(0.0, 0.0, VIEW_W, HEADER),
        );
        cx.scene.stroke(
            &Stroke::new(1.0),
            Affine::IDENTITY,
            theme.line,
            None,
            &Line::new((0.0, HEADER - 0.5), (VIEW_W, HEADER - 0.5)),
        );
        draw_mark(cx, 16.0, 15.0, 26.0);
        draw_title(cx, 52.0, 18.0, "Pitch");
        let s = &self.shared.session;
        let words = match s.status() {
            Status::Empty => "no take yet".to_owned(),
            Status::Armed => "armed: play the part".to_owned(),
            Status::Recording => "recording…".to_owned(),
            Status::Analysing => "finding the notes…".to_owned(),
            Status::Ready => match (s.take(), s.analysis()) {
                (Some(t), Some(a)) => format!(
                    "{} notes · {:.1} s take",
                    a.notes.len(),
                    t.len as f64 / t.sample_rate
                ),
                _ => String::new(),
            },
            Status::Problem(p) => p,
        };
        let words = fit(cx, &words, TextStyle::BODY, 700.0);
        cx.text.draw(
            cx.scene,
            &words,
            260.0,
            20.0,
            TextStyle::BODY,
            theme.text_mid,
            Align::Left,
        );
    }

    fn draw_grid(&self, cx: &mut DrawCx<'_>) {
        let theme = cx.theme;
        cx.scene
            .fill(Fill::NonZero, Affine::IDENTITY, theme.display, None, &ROLL);
        cx.scene
            .fill(Fill::NonZero, Affine::IDENTITY, theme.well, None, &KEYS);
        let rh = self.row_h();
        let first = (self.pitch.0 / 100.0).floor() as i64;
        let last = (self.pitch.1 / 100.0).ceil() as i64;
        // At most a few hundred rows, whatever the zoom.
        for note in first..=last.min(first + 400) {
            let k = note.rem_euclid(12) as usize;
            let (y0, y1) = (
                self.y_of((note as f64 + 0.5) * 100.0),
                self.y_of((note as f64 - 0.5) * 100.0),
            );
            let row = Rect::new(ROLL.x0, y0.max(ROLL.y0), ROLL.x1, y1.min(ROLL.y1));
            if row.height() <= 0.0 {
                continue;
            }
            if BLACK[k] {
                cx.scene.fill(
                    Fill::NonZero,
                    Affine::IDENTITY,
                    theme.well.multiply_alpha(0.55),
                    None,
                    &row,
                );
                cx.scene.fill(
                    Fill::NonZero,
                    Affine::IDENTITY,
                    theme.display,
                    None,
                    &Rect::new(KEYS.x0, row.y0, KEYS.x0 + 38.0, row.y1),
                );
            }
            if k == 0 {
                cx.scene.stroke(
                    &Stroke::new(1.0),
                    Affine::IDENTITY,
                    theme.grid_major,
                    None,
                    &Line::new((KEYS.x0, y1), (ROLL.x1, y1)),
                );
            } else if rh >= 8.0 {
                cx.scene.stroke(
                    &Stroke::new(1.0),
                    Affine::IDENTITY,
                    theme.grid_minor.multiply_alpha(0.5),
                    None,
                    &Line::new((ROLL.x0, y1), (ROLL.x1, y1)),
                );
            }
            if (k == 0 || (rh >= 13.0 && !BLACK[k]))
                && (y0 + y1) / 2.0 > KEYS.y0 + 6.0
                && (y0 + y1) / 2.0 < KEYS.y1 - 6.0
            {
                let name = format!("{}{}", NAMES[k], note / 12 - 1);
                cx.text.draw(
                    cx.scene,
                    &name,
                    KEYS.x1 - 6.0,
                    (y0 + y1) / 2.0 - 6.0,
                    TextStyle::LABEL.sized(9.0),
                    if k == 0 { theme.text_hi } else { theme.text_lo },
                    Align::Right,
                );
            }
        }
        cx.scene.stroke(
            &Stroke::new(1.0),
            Affine::IDENTITY,
            theme.line,
            None,
            &Line::new((ROLL.x0 - 0.5, ROLL.y0), (ROLL.x0 - 0.5, ROLL.y1)),
        );
        // The ruler: seconds from the take's start (a step that keeps labels apart).
        cx.scene
            .fill(Fill::NonZero, Affine::IDENTITY, theme.well, None, &RULER);
        let step = [0.1, 0.25, 0.5, 1.0, 2.0, 5.0, 10.0, 15.0, 30.0, 60.0]
            .into_iter()
            .find(|s| s / self.span * ROLL.width() >= 70.0)
            .unwrap_or(120.0);
        let mut t = (self.t0 / step).ceil() * step;
        let mut guard = 0;
        while t <= self.t0 + self.span && guard < 200 {
            let x = self.x_of(t);
            cx.scene.stroke(
                &Stroke::new(1.0),
                Affine::IDENTITY,
                theme.grid_minor,
                None,
                &Line::new((x, ROLL.y0), (x, ROLL.y1)),
            );
            let label = if step < 1.0 {
                format!("{t:.2} s")
            } else {
                format!("{t:.0} s")
            };
            cx.text.draw(
                cx.scene,
                &label,
                x + 4.0,
                RULER.y0 + 6.0,
                TextStyle {
                    tabular: true,
                    ..TextStyle::BODY.sized(10.0)
                },
                theme.text_lo,
                Align::Left,
            );
            t += step;
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
            let (ts, te) = (
                (n.start as f64 + e.offset) / a.fs,
                (n.start as f64 + e.offset + (n.end - n.start) as f64 * e.stretch) / a.fs,
            );
            if te < v0 || ts > v1 {
                continue;
            }
            let r = self.rect_of(n, &e, a.fs);
            let sel = self.selected == Some(i);
            let hot = self.hot == Some(Hot::Note(i));
            // Where it was sung, when it has moved.
            if e.shift_cents.abs() > 1.0 {
                let ghost = Rect::new(
                    r.x0,
                    self.y_of(n.cents) - r.height() / 2.0,
                    r.x1,
                    self.y_of(n.cents) + r.height() / 2.0,
                );
                cx.scene.stroke(
                    &Stroke::new(1.0).with_dashes(0.0, [3.0, 3.0]),
                    Affine::IDENTITY,
                    accent.multiply_alpha(0.35),
                    None,
                    &RoundedRect::from_rect(ghost, ghost.height() / 2.0),
                );
            }
            // Thickness by level: the loudest note full, 30 dB under it half.
            let k = (0.5 + 0.5 * ((n.db - loud + 30.0) / 30.0).clamp(0.0, 1.0)) * r.height() / 2.0;
            let body = Rect::new(r.x0, r.center().y - k, r.x1, r.center().y + k);
            let rr = RoundedRect::from_rect(body, body.height() / 2.0);
            cx.scene.fill(
                Fill::NonZero,
                Affine::IDENTITY,
                accent.multiply_alpha(if sel {
                    0.55
                } else if hot {
                    0.42
                } else {
                    0.3
                }),
                None,
                &rr,
            );
            if sel {
                glow_stroke(cx, &rr, 1.4, accent, accent, 2.5);
            } else {
                cx.scene.stroke(
                    &Stroke::new(1.0),
                    Affine::IDENTITY,
                    accent.multiply_alpha(0.7),
                    None,
                    &rr,
                );
            }
            // The pitch curve through it, as it will play.
            let mut curve = BezPath::new();
            let mut started = false;
            for (f, d) in n.drift.iter().enumerate() {
                let t_in = n.start as f64 + (f * a.hop) as f64;
                let t_out = n.start as f64 + e.offset + (t_in - n.start as f64) * e.stretch;
                let voiced = a
                    .frames
                    .get(t_in as usize / a.hop)
                    .is_some_and(|fr| fr.cents.is_some());
                if !voiced {
                    started = false;
                    continue;
                }
                let p = Point::new(
                    self.x_of(t_out / a.fs),
                    self.y_of(n.cents + e.shift_cents + e.drift * d),
                );
                if started {
                    curve.line_to(p);
                } else {
                    curve.move_to(p);
                    started = true;
                }
            }
            cx.scene.stroke(
                &Stroke::new(1.2),
                Affine::IDENTITY,
                theme
                    .text_strong
                    .multiply_alpha(if sel { 0.95 } else { 0.6 }),
                None,
                &curve,
            );
        }
        // The playhead.
        if let Some(t) = s.take() {
            let at = (s.playhead.load(Ordering::Relaxed) - t.start) as f64 / t.sample_rate;
            if (v0..=v1).contains(&at) {
                let x = self.x_of(at);
                cx.scene.stroke(
                    &Stroke::new(1.4),
                    Affine::IDENTITY,
                    theme.text_hi,
                    None,
                    &Line::new((x, ROLL.y0), (x, ROLL.y1)),
                );
            }
        }
        cx.end_clip();
    }

    fn draw_empty(&self, cx: &mut DrawCx<'_>) {
        let theme = cx.theme;
        let (title, words) = match self.shared.session.status() {
            Status::Empty => (
                "No take yet",
                "Click TRANSFER, then play the part you want to edit: it is recorded from where the transport starts until it stops.",
            ),
            Status::Armed => (
                "Armed",
                "Press play in the host: the take is recorded from where the transport starts until it stops.",
            ),
            Status::Recording => ("Recording", "Stop the transport to finish the take."),
            Status::Analysing => ("Finding the notes", "A moment: the take is being analysed."),
            Status::Problem(_) => ("No take", "See the line above."),
            Status::Ready => return,
        };
        let c = ROLL.center();
        cx.text.draw(
            cx.scene,
            title,
            c.x,
            c.y - 30.0,
            TextStyle::BODY.sized(20.0).weighted(700.0),
            theme.text_hi,
            Align::Center,
        );
        let words = fit(cx, words, TextStyle::BODY.sized(13.0), ROLL.width() - 80.0);
        cx.text.draw(
            cx.scene,
            &words,
            c.x,
            c.y + 4.0,
            TextStyle::BODY.sized(13.0),
            theme.text_lo,
            Align::Center,
        );
    }

    fn draw_transfer(&self, cx: &mut DrawCx<'_>) {
        let theme = cx.theme;
        let status = self.shared.session.status();
        let on = matches!(status, Status::Armed | Status::Recording);
        let hot = self.hot == Some(Hot::Transfer);
        let rr = RoundedRect::from_rect(TRANSFER, TRANSFER.height() / 2.0);
        let ink = if matches!(status, Status::Recording) {
            theme.danger
        } else {
            theme.accent
        };
        cx.scene.fill(
            Fill::NonZero,
            Affine::IDENTITY,
            if on {
                ink.multiply_alpha(0.2)
            } else {
                theme.well
            },
            None,
            &rr,
        );
        if on {
            glow_stroke(cx, &rr, 1.2, ink, ink, 2.5);
        } else {
            cx.scene.stroke(
                &Stroke::new(1.0),
                Affine::IDENTITY,
                if hot {
                    theme.text_hi
                } else {
                    theme.well_border
                },
                None,
                &rr,
            );
        }
        let label = match status {
            Status::Recording => "RECORDING",
            Status::Armed => "ARMED",
            _ => "TRANSFER",
        };
        let c = TRANSFER.center();
        cx.text.draw(
            cx.scene,
            label,
            c.x,
            c.y - 6.0,
            TextStyle::LABEL.sized(10.0),
            if on {
                ink
            } else if hot {
                theme.text_hi
            } else {
                theme.text_mid
            },
            Align::Center,
        );
    }

    fn help_text(&self, host: &dyn ParamHost) -> Help {
        if let Some(h) = self.footer.help(host) {
            return h;
        }
        let a = self.shared.session.analysis();
        let note = self.drag.map(|d| d.note).or(match self.hot {
            Some(Hot::Note(i)) => Some(i),
            _ => None,
        });
        if let (Some(i), Some(a)) = (note, a) {
            if let Some(n) = a.notes.get(i) {
                let e = self
                    .shared
                    .session
                    .edits()
                    .get(i)
                    .copied()
                    .unwrap_or_default();
                let c = n.cents + e.shift_cents;
                let k = (c / 100.0).round();
                let name = format!(
                    "{}{}",
                    NAMES[(k as i64).rem_euclid(12) as usize],
                    k as i64 / 12 - 1
                );
                return Help::default().text(format!("{name} ")).slot(format!("{:+.0} cents", c - k * 100.0), "+50 cents").text(format!(" · sung {:+.0} cents off · {:.2} s — drag up and down (Alt: free); double-click: centre on {name}", n.cents - (n.cents / 100.0).round() * 100.0, (n.end - n.start) as f64 / a.fs));
            }
        }
        if self.hot == Some(Hot::Transfer) {
            return Help::default().text("Transfer — record the next stretch the transport plays (it replaces the take); click again to cancel");
        }
        Help::default().text("Drag a note to retune it (snapped to semitones, Alt: free); double-click: centre it. Wheel: scroll (Ctrl: zoom, Shift: pitch). Ctrl+Z: undo.")
    }

    fn handle_event(&mut self, event: &UiEvent, host: &dyn ParamHost) -> bool {
        if self.footer.event(event, host) {
            return true;
        }
        let session = self.shared.session.clone();
        match *event {
            UiEvent::KeyDown {
                key: Key::Character(c),
                mods,
            } if mods.ctrl && (c == 'z' || c == 'Z' || c == 'y') => {
                let redo = c == 'y' || mods.shift;
                let (from, to) = if redo {
                    (&mut self.redo, &mut self.undo)
                } else {
                    (&mut self.undo, &mut self.redo)
                };
                if let Some(edits) = from.pop() {
                    to.push(session.edits());
                    session.set_edits(edits);
                }
                true
            }
            UiEvent::PointerMove { pos, mods } => {
                self.pointer = Some(pos);
                if let Some(d) = self.drag {
                    let Some(a) = session.analysis() else {
                        return true;
                    };
                    let Some(n) = a.notes.get(d.note) else {
                        return true;
                    };
                    let dc = -(pos.y - d.start_y) / ROLL.height() * (self.pitch.1 - self.pitch.0);
                    let mut target = n.cents + d.start_shift + dc;
                    if !mods.alt {
                        target = (target / 100.0).round() * 100.0;
                    }
                    let mut e = session.edits().get(d.note).copied().unwrap_or_default();
                    e.shift_cents = target - n.cents;
                    session.set_edit(d.note, e);
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
            UiEvent::PointerDown {
                pos,
                button: MouseButton::Left,
                ..
            } => match self.hot_at(pos) {
                Some(Hot::Transfer) => {
                    let armed = matches!(session.status(), Status::Armed);
                    session.set_armed(!armed);
                    true
                }
                Some(Hot::Note(i)) => {
                    self.selected = Some(i);
                    self.push_undo();
                    let shift = session.edits().get(i).map_or(0.0, |e| e.shift_cents);
                    self.drag = Some(Drag {
                        note: i,
                        start_y: pos.y,
                        start_shift: shift,
                    });
                    true
                }
                None if ROLL.contains(pos) => {
                    self.selected = None;
                    true
                }
                None => false,
            },
            UiEvent::PointerUp {
                button: MouseButton::Left,
                ..
            } => {
                if let Some(d) = self.drag.take() {
                    // A click without a move leaves no undo step.
                    if self.undo.last().is_some_and(|u| *u == session.edits()) {
                        self.undo.pop();
                    }
                    let _ = d;
                    return true;
                }
                false
            }
            UiEvent::DoubleClick { pos, .. } => match self.hot_at(pos) {
                Some(Hot::Note(i)) => {
                    self.drag = None;
                    let Some(a) = session.analysis() else {
                        return true;
                    };
                    let Some(n) = a.notes.get(i) else { return true };
                    let mut e = session.edits().get(i).copied().unwrap_or_default();
                    let target = ((n.cents + e.shift_cents) / 100.0).round() * 100.0;
                    if (target - n.cents - e.shift_cents).abs() > 0.01 {
                        self.push_undo();
                        e.shift_cents = target - n.cents;
                        session.set_edit(i, e);
                    }
                    true
                }
                _ => false,
            },
            UiEvent::Wheel { pos, lines, mods }
                if ROLL.contains(pos) || RULER.contains(pos) || KEYS.contains(pos) =>
            {
                let take_s = session
                    .take()
                    .map_or(SPAN_S, |t| t.len as f64 / t.sample_rate);
                if mods.ctrl {
                    // Zoom in time around the pointer.
                    let at = self.t0 + (pos.x - ROLL.x0) / ROLL.width() * self.span;
                    let span = (self.span * 0.85f64.powf(f64::from(lines)))
                        .clamp(SPAN_RANGE_S.0, SPAN_RANGE_S.1.max(take_s));
                    self.t0 = (at - (pos.x - ROLL.x0) / ROLL.width() * span).max(0.0);
                    self.span = span;
                } else if mods.shift || KEYS.contains(pos) {
                    let d = f64::from(lines) * 100.0;
                    self.pitch = (self.pitch.0 + d, self.pitch.1 + d);
                } else {
                    self.t0 = (self.t0 - f64::from(lines) * 0.1 * self.span)
                        .clamp(0.0, (take_s - 0.5 * self.span).max(0.0));
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
        cx.scene.fill(
            Fill::NonZero,
            Affine::IDENTITY,
            theme.display,
            None,
            &Rect::new(0.0, DISPLAY_TOP, VIEW_W, HELP_TOP),
        );
        self.draw_grid(cx);
        self.draw_notes(cx);
        self.draw_empty(cx);
        self.help_text(cx.host)
            .draw(cx, Rect::new(0.0, HELP_TOP, VIEW_W, FOOTER_TOP));
        self.footer.draw_frame(cx);
        self.footer.draw_controls(cx);
        self.draw_transfer(cx);
    }

    fn event(&mut self, event: &UiEvent, host: &dyn ParamHost) -> bool {
        self.handle_event(event, host)
    }

    fn tick(&mut self, _host: &dyn ParamHost) -> bool {
        self.last_tick = Instant::now();
        let s = &self.shared.session;
        let changed = s.poll();
        let version = s.version.load(Ordering::Acquire);
        let fresh = version != self.seen_version;
        self.seen_version = version;
        // Follow the playhead a page at a time while the take plays.
        let playing = s.playing.load(Ordering::Relaxed);
        if playing {
            if let Some(t) = s.take() {
                let at = (s.playhead.load(Ordering::Relaxed) - t.start) as f64 / t.sample_rate;
                if self.drag.is_none()
                    && (at > self.t0 + self.span * 0.95 || at < self.t0)
                    && at >= 0.0
                    && at <= t.len as f64 / t.sample_rate
                {
                    self.t0 = (at - 0.05 * self.span).max(0.0);
                }
            }
        }
        let _ = self.fs();
        changed
            || fresh
            || playing
            || s.busy()
            || matches!(
                s.status(),
                Status::Armed | Status::Recording | Status::Analysing
            )
    }
}
