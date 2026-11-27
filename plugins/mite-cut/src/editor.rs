//! Mite Cut's editor, first cut: the document's tracks, each with its role (a click moves it
//! on, Shift-click back) and how many hits it keeps. The lanes (each track's waveform, its hits
//! marked by confidence, kept and cut stretches, edits by hand) come next.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use mite_dsp::hits::Role;
use mite_ui::kurbo::{Affine, Line, Point, Rect, RoundedRect, Stroke};
use mite_ui::peniko::Fill;
use mite_ui::widgets::brand::{draw_mark, draw_title};
use mite_ui::{Align, DrawCx, ParamHost, TextStyle, UiEvent, View};

use crate::CutShared;
use crate::session::Status;

/// Cut is a gate at heart: it wears the Gate accent.
pub const ACCENT: &str = "gate";
pub const WIDTH: f64 = 640.0;
pub const HEIGHT: f64 = 420.0;
const HEADER: f64 = 48.0;
const HELP: f64 = 28.0;
const ROW: f64 = 34.0;
const TOP: f64 = HEADER + 58.0;
/// The role chip's column.
const CHIP_X: f64 = 250.0;
const CHIP_W: f64 = 92.0;

pub struct CutView {
    shared: Arc<CutShared>,
    seen: u64,
    scroll: usize,
    hover: Option<usize>,
}

impl CutView {
    pub fn new(shared: Arc<CutShared>) -> Self {
        Self { shared, seen: u64::MAX, scroll: 0, hover: None }
    }

    fn chip(row: usize) -> Rect {
        let y = TOP + row as f64 * ROW;
        Rect::new(CHIP_X, y + 6.0, CHIP_X + CHIP_W, y + ROW - 6.0)
    }

    fn rows(&self) -> usize {
        ((HEIGHT - HELP - TOP) / ROW).floor() as usize
    }

    fn row_at(&self, p: Point) -> Option<usize> {
        (0..self.rows()).find(|&r| Self::chip(r).contains(p)).map(|r| r + self.scroll)
    }
}

/// The next role (or the one before), as a click on the chip moves it.
pub fn next_role(r: Role, back: bool) -> Role {
    let all = Role::ALL;
    let i = all.iter().position(|x| *x == r).unwrap_or(0);
    let n = all.len();
    all[if back { (i + n - 1) % n } else { (i + 1) % n }]
}

impl View for CutView {
    fn logical_size(&self) -> (f64, f64) {
        (WIDTH, HEIGHT)
    }

    fn draw(&mut self, cx: &mut DrawCx<'_>) {
        let theme = cx.theme;
        let s = &self.shared.session;
        cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.display, None, &Rect::new(0.0, HEADER, WIDTH, HEIGHT - HELP));
        cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.window, None, &Rect::new(0.0, 0.0, WIDTH, HEADER));
        cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, theme.line, None, &Line::new((0.0, HEADER - 0.5), (WIDTH, HEADER - 0.5)));
        draw_mark(cx, 14.0, 11.0, 26.0);
        draw_title(cx, 48.0, 14.0, "Cut");
        let status = match s.status() {
            Status::Unbound => "Not bound to an ARA document: insert Mite Cut (VST3) on each drum track, in a host with ARA.".to_owned(),
            Status::Reading => "Reading the tracks from the host…".to_owned(),
            Status::Detecting => "Finding the hits on every track…".to_owned(),
            Status::Ready => String::new(),
        };
        let tracks = s.tracks();
        let heading = if status.is_empty() { format!("{} tracks in the session", tracks.len()) } else { status };
        cx.text.draw(cx.scene, &heading, 20.0, HEADER + 12.0, TextStyle::BODY.sized(12.0), theme.text_mid, Align::Left);
        let label = TextStyle::LABEL.sized(9.0);
        cx.text.draw(cx.scene, "TRACK", 20.0, TOP - 14.0, label, theme.text_lo, Align::Left);
        cx.text.draw(cx.scene, "ROLE", CHIP_X, TOP - 14.0, label, theme.text_lo, Align::Left);
        cx.text.draw(cx.scene, "HITS KEPT", CHIP_X + CHIP_W + 110.0, TOP - 14.0, label, theme.text_lo, Align::Left);
        for (row, t) in tracks.iter().skip(self.scroll).take(self.rows()).enumerate() {
            let y = TOP + row as f64 * ROW;
            cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, theme.grid_minor, None, &Line::new((16.0, y + ROW - 0.5), (WIDTH - 16.0, y + ROW - 0.5)));
            cx.text.draw(cx.scene, &t.name, 20.0, y + 10.0, TextStyle::BODY.sized(13.0), theme.text_hi, Align::Left);
            let chip = Self::chip(row);
            let hot = self.hover == Some(row + self.scroll);
            let cuts = t.role.cuts();
            let fill = if cuts { theme.accent } else { theme.control };
            cx.scene.fill(Fill::NonZero, Affine::IDENTITY, fill, None, &RoundedRect::from_rect(chip, 4.0));
            if hot {
                cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, theme.text_hi, None, &RoundedRect::from_rect(chip, 4.0));
            }
            let ink = if cuts { theme.window } else { theme.text_mid };
            cx.text.draw(cx.scene, t.role.name(), chip.center().x, chip.y0 + 4.0, TextStyle::LABEL.sized(11.0), ink, Align::Center);
            if t.guessed {
                cx.text.draw(cx.scene, "from its name", chip.x1 + 8.0, chip.y0 + 5.0, TextStyle::BODY.sized(10.0), theme.text_lo, Align::Left);
            }
            let (kept, all) = t.counts();
            let what = if !cuts {
                match t.role {
                    Role::Overhead | Role::Room => "never cut".to_owned(),
                    _ => "passes".to_owned(),
                }
            } else {
                format!("{kept} of {all}")
            };
            cx.text.draw(cx.scene, &what, CHIP_X + CHIP_W + 110.0, y + 11.0, TextStyle::BODY.sized(12.0), theme.text_mid, Align::Left);
        }
        let top = HEIGHT - HELP;
        cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.help_strip, None, &Rect::new(0.0, top, WIDTH, HEIGHT));
        cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, theme.line, None, &Line::new((0.0, top + 0.5), (WIDTH, top + 0.5)));
        cx.text.draw(cx.scene, "Click a role to change it (Shift: back) · each track is judged against the others · OH and Room are never cut", 14.0, top + 7.0, TextStyle::BODY.sized(11.0), theme.text_mid, Align::Left);
    }

    fn event(&mut self, event: &UiEvent, _host: &dyn ParamHost) -> bool {
        let tracks = self.shared.session.tracks();
        match event {
            UiEvent::PointerMove { pos, .. } => {
                let h = self.row_at(*pos).filter(|&r| r < tracks.len());
                let changed = h != self.hover;
                self.hover = h;
                changed
            }
            UiEvent::PointerLeft => self.hover.take().is_some(),
            UiEvent::PointerDown { pos, mods, .. } => {
                let Some(r) = self.row_at(*pos).filter(|&r| r < tracks.len()) else { return false };
                let t = &tracks[r];
                self.shared.session.set_role(t.sequence, next_role(t.role, mods.shift));
                true
            }
            UiEvent::Wheel { lines, .. } => {
                let max = tracks.len().saturating_sub(self.rows());
                let before = self.scroll;
                self.scroll = if *lines > 0.0 { self.scroll.saturating_sub(1) } else { (self.scroll + 1).min(max) };
                before != self.scroll
            }
            _ => false,
        }
    }

    fn tick(&mut self, _host: &dyn ParamHost) -> bool {
        let v = self.shared.session.version.load(Ordering::Acquire);
        let changed = v != self.seen;
        self.seen = v;
        changed
    }
}
