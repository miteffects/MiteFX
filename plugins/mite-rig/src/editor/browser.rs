//! TONE3000 inside the plugin: everything happens here, nothing opens the website except the
//! one-time sign-in (TONE3000 confirms who you are on its own page).
//!
//! Top: search (type, it searches as you pause), who is signed in. Left: Explore and your lists
//! (Favorites, Downloaded, My Tones), and Explore's filters (gear, order, A2 or A1). Middle: the
//! tones as cards with their cover, maker, counts and a heart; more load as you scroll. Right,
//! once a card is picked: its cover, who made it, its license, its description, and its files,
//! each with a button that loads it into the block it was opened from (or adds a new block for it
//! when it is the other kind: an IR from a capture block), while
//! the rig keeps playing.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use mite_ui::kurbo::{Affine, BezPath, Cap, Circle, Join, Line, Point, Rect, RoundedRect, Stroke};
use mite_ui::paint::{glass_panel, icon_button};
use mite_ui::peniko::{Blob, Fill, ImageAlphaType, ImageBrush, ImageData, ImageFormat, ImageQuality};
use mite_ui::{Align, DrawCx, Key, MouseButton, TextStyle, UiEvent};

use crate::blocks::Kind;
use crate::tone3000::{self, Gear, Model, Phase, Query, Sort, Source, Tone, View};
use crate::RigUi;

/// The panel, over the whole display.
pub const PANEL: Rect = Rect::new(12.0, 64.0, 1268.0, 668.0);
const TOP: f64 = PANEL.y0 + 64.0;
const SIDE: Rect = Rect::new(PANEL.x0 + 14.0, TOP, PANEL.x0 + 196.0, PANEL.y1 - 14.0);
const GRID_X0: f64 = SIDE.x1 + 14.0;
const DETAIL_W: f64 = 392.0;
const CARD_GAP: f64 = 14.0;
const CARD_MIN_W: f64 = 220.0;
const CARD_H: f64 = 248.0;
const IMAGE_H: f64 = 136.0;
const SEARCH: Rect = Rect::new(PANEL.x0 + 196.0, PANEL.y0 + 14.0, PANEL.x0 + 700.0, PANEL.y0 + 50.0);
const CLOSE: Rect = Rect::new(PANEL.x1 - 48.0, PANEL.y0 + 14.0, PANEL.x1 - 14.0, PANEL.y0 + 48.0);
const SIGN_OUT: Rect = Rect::new(PANEL.x1 - 152.0, PANEL.y0 + 14.0, PANEL.x1 - 60.0, PANEL.y0 + 48.0);
const SIDE_ROW: f64 = 30.0;
const FILE_ROW: f64 = 42.0;
/// A pause in typing this long starts the search.
const TYPING: Duration = Duration::from_millis(450);

const SEARCH_GLYPH: &str = "M10.5 4 A6.5 6.5 0 1 1 10.49 4 M15.2 15.2 L20 20";
const HEART: &str = "M12 20 C5 14.5 2.5 11.5 2.5 8.2 C2.5 5.6 4.5 3.7 7 3.7 C9 3.7 10.6 4.9 12 6.8 C13.4 4.9 15 3.7 17 3.7 C19.5 3.7 21.5 5.6 21.5 8.2 C21.5 11.5 19 14.5 12 20 Z";
const DOWN_GLYPH: &str = "M12 4 L12 15 M7 10.5 L12 15.5 L17 10.5 M5 19.5 L19 19.5";

/// What a file's button does: load into the block it was opened from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Load,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Hot {
    Close,
    Search,
    SignIn,
    SignOut,
    OpenCode,
    Source(Source),
    Gear(Gear),
    Sort(Sort),
    Generation(bool),
    Card(usize),
    Heart(usize),
    DetailClose,
    DetailHeart,
    DetailSite,
    Load(usize, Action),
    Retry,
}

pub struct Browser {
    /// The block it was opened from, and its kind.
    pub target: u64,
    pub kind: Kind,
    scroll: f64,
    detail_scroll: f64,
    text: String,
    focused: bool,
    typed: Option<Instant>,
    hot: Option<Hot>,
    brushes: HashMap<String, ImageBrush>,
    view: View,
    seen: u64,
    /// "Loaded into the Amp" and when.
    notice: Option<(String, Instant)>,
    started: Instant,
}

fn glyph(scene: &mut mite_ui::vello::Scene, d: &str, c: Point, size: f64, color: mite_ui::peniko::Color, width: f64, fill: bool) {
    let Ok(mut path) = BezPath::from_svg(d) else { return };
    let k = size / 24.0;
    path.apply_affine(Affine::translate((c.x - 12.0 * k, c.y - 12.0 * k)) * Affine::scale(k));
    if fill {
        scene.fill(Fill::NonZero, Affine::IDENTITY, color, None, &path);
    } else {
        scene.stroke(&Stroke::new(width).with_caps(Cap::Round).with_join(Join::Round), Affine::IDENTITY, color, None, &path);
    }
}

fn thousands(n: u64) -> String {
    if n >= 10_000 {
        format!("{:.0}k", n as f64 / 1000.0)
    } else if n >= 1000 {
        format!("{:.1}k", n as f64 / 1000.0)
    } else {
        n.to_string()
    }
}

/// Cuts `text` to `width` (with an ellipsis).
fn fit(cx: &mut DrawCx<'_>, text: &str, style: TextStyle, width: f64) -> String {
    if f64::from(cx.text.measure(text, style)) <= width {
        return text.to_owned();
    }
    let mut s: String = text.to_owned();
    while !s.is_empty() {
        s.pop();
        let t = format!("{}…", s.trim_end());
        if f64::from(cx.text.measure(&t, style)) <= width {
            return t;
        }
    }
    "…".to_owned()
}

/// Word-wraps `text` to `width`, at most `max` lines (the last one cut).
fn wrap(cx: &mut DrawCx<'_>, text: &str, style: TextStyle, width: f64, max: usize) -> Vec<String> {
    let mut lines = Vec::new();
    for para in text.split('\n') {
        let mut line = String::new();
        for word in para.split_whitespace() {
            let next = if line.is_empty() { word.to_owned() } else { format!("{line} {word}") };
            if f64::from(cx.text.measure(&next, style)) <= width || line.is_empty() {
                line = next;
            } else {
                lines.push(std::mem::replace(&mut line, word.to_owned()));
            }
            if lines.len() >= max {
                break;
            }
        }
        if lines.len() >= max {
            break;
        }
        lines.push(line);
    }
    if lines.len() > max {
        lines.truncate(max);
    }
    if lines.len() == max {
        if let Some(last) = lines.last_mut() {
            *last = fit(cx, &format!("{last} …"), style, width);
        }
    }
    lines.retain(|l| !l.is_empty());
    lines
}

impl Browser {
    pub fn new(target: u64, kind: Kind, ui: &RigUi) -> Self {
        let view = ui.tone3000.view();
        let mut b = Self { target, kind, scroll: 0.0, detail_scroll: 0.0, text: view.feed.query.text.clone(), focused: false, typed: None, hot: None, brushes: HashMap::new(), seen: u64::MAX, view, notice: None, started: Instant::now() };
        b.ensure_feed(ui);
        b
    }

    /// Explore's gear filter for the block it was opened from: cabinets for a Cab, anything but
    /// cabinets for a Capture (kept when it already fits).
    fn gear_for(kind: Kind, current: Gear) -> Gear {
        match (kind, current) {
            (Kind::Cab, _) => Gear::Cab,
            (Kind::Pedal, _) => Gear::Pedal,
            (_, Gear::FullRig) => Gear::FullRig,
            _ => Gear::Amp,
        }
    }

    /// The gear filters a block offers: a Pedal only pedals, an Amp amps and combos, a Cab IRs.
    fn gears(&self) -> &'static [Gear] {
        match self.kind {
            Kind::Pedal => &[Gear::Pedal],
            Kind::Cab => &[Gear::Cab],
            _ => &[Gear::Amp, Gear::FullRig],
        }
    }

    /// Whether a tone holds what this block plays.
    fn fits(&self, t: &Tone) -> bool {
        self.kind.accepts(crate::rig::Gear::of_tone3000(&t.gear, &t.format))
    }

    /// The tones shown: the feed's, those that fit the block (the user's own lists hold every
    /// kind).
    fn tones(&self) -> Vec<Tone> {
        self.view.feed.tones.iter().filter(|t| self.fits(t)).cloned().collect()
    }

    /// Something to show once signed in: Explore, filtered to the slot's gear.
    fn ensure_feed(&mut self, ui: &RigUi) {
        let v = ui.tone3000.view();
        if v.phase != Phase::SignedIn || v.feed.loading {
            return;
        }
        let gear = Self::gear_for(self.kind, v.feed.query.gear);
        let empty = v.feed.tones.is_empty() && v.feed.error.is_none();
        let wrong = v.feed.source == Source::Search && gear != v.feed.query.gear;
        if empty || wrong {
            ui.tone3000.search(v.feed.source, Query { gear, ..v.feed.query.clone() });
        }
    }

    pub fn tick(&mut self, ui: &RigUi) -> bool {
        let mut fresh = false;
        let version = ui.tone3000.version();
        if version != self.seen {
            self.seen = version;
            self.view = ui.tone3000.view();
            // Signed in just now: something to show.
            let v = &self.view;
            if v.phase == Phase::SignedIn && v.feed.tones.is_empty() && !v.feed.loading && v.feed.error.is_none() {
                self.ensure_feed(ui);
            }
            fresh = true;
        }
        if let Some(at) = self.typed {
            if at.elapsed() >= TYPING {
                self.typed = None;
                self.run_search(ui);
            }
        }
        // Near the end of the cards: the next page.
        if self.view.feed.more() && self.scroll + (PANEL.y1 - TOP) + 2.0 * CARD_H >= self.content_height() {
            ui.tone3000.more();
        }
        if self.notice.as_ref().is_some_and(|(_, at)| at.elapsed() > Duration::from_secs(3)) {
            self.notice = None;
            fresh = true;
        }
        fresh || self.typed.is_some() || self.focused || self.view.feed.loading || self.view.downloading.is_some() || self.notice.is_some()
    }

    fn run_search(&mut self, ui: &RigUi) {
        let q = Query { text: self.text.clone(), ..self.view.feed.query.clone() };
        let q = if !q.text.trim().is_empty() && self.view.feed.query.text.trim().is_empty() && q.sort == Sort::Trending { Query { sort: Sort::BestMatch, ..q } } else { q };
        self.scroll = 0.0;
        ui.tone3000.search(Source::Search, q);
        self.view = ui.tone3000.view();
    }

    // ---- geometry -----------------------------------------------------------------------------

    fn grid_x1(&self) -> f64 {
        if self.view.tone.is_some() { PANEL.x1 - 14.0 - DETAIL_W - 14.0 } else { PANEL.x1 - 14.0 }
    }

    fn columns(&self) -> usize {
        (((self.grid_x1() - GRID_X0 + CARD_GAP) / (CARD_MIN_W + CARD_GAP)).floor() as usize).max(1)
    }

    fn card_w(&self) -> f64 {
        let n = self.columns() as f64;
        (self.grid_x1() - GRID_X0 - (n - 1.0) * CARD_GAP) / n
    }

    fn grid_top(&self) -> f64 {
        TOP + 26.0
    }

    fn card_rect(&self, i: usize) -> Rect {
        let n = self.columns();
        let (row, col) = (i / n, i % n);
        let w = self.card_w();
        let x = GRID_X0 + col as f64 * (w + CARD_GAP);
        let y = self.grid_top() + row as f64 * (CARD_H + CARD_GAP) - self.scroll;
        Rect::new(x, y, x + w, y + CARD_H)
    }

    fn heart_rect(&self, i: usize) -> Rect {
        let r = self.card_rect(i);
        Rect::new(r.x1 - 40.0, r.y0 + 8.0, r.x1 - 8.0, r.y0 + 40.0)
    }

    fn content_height(&self) -> f64 {
        let rows = self.tones().len().div_ceil(self.columns()) as f64;
        rows * (CARD_H + CARD_GAP) + 60.0
    }

    fn detail(&self) -> Rect {
        Rect::new(PANEL.x1 - 14.0 - DETAIL_W, TOP, PANEL.x1 - 14.0, PANEL.y1 - 14.0)
    }

    fn detail_image(&self) -> Rect {
        let d = self.detail();
        Rect::new(d.x0, d.y0, d.x1, d.y0 + 200.0)
    }

    fn files_top(&self) -> f64 {
        self.detail().y0 + 384.0 - self.detail_scroll
    }

    fn file_rows(&self) -> Vec<Model> {
        self.view.models.clone().unwrap_or_default()
    }

    /// A file's Load button, when the file is what the block plays (an IR for a Cab, a capture
    /// for a Pedal or an Amp).
    fn load_buttons(&self, i: usize, m: &Model) -> Vec<(Action, Rect)> {
        let d = self.detail();
        let y = self.files_top() + i as f64 * FILE_ROW;
        if (m.kind == "IR") != (self.kind == Kind::Cab) {
            return Vec::new();
        }
        vec![(Action::Load, Rect::new(d.x1 - 8.0 - 92.0, y + 6.0, d.x1 - 8.0, y + FILE_ROW - 6.0))]
    }

    fn side_rows(&self) -> Vec<(Hot, String, bool)> {
        let v = &self.view;
        let mut rows: Vec<(Hot, String, bool)> = Source::ALL.iter().map(|s| (Hot::Source(*s), s.name().to_owned(), v.feed.source == *s)).collect();
        if v.feed.source == Source::Search {
            rows.push((Hot::Close, "GEAR".to_owned(), false));
            rows.extend(self.gears().iter().map(|g| (Hot::Gear(*g), g.name().to_owned(), v.feed.query.gear == *g)));
        }
        rows
    }

    /// Explore's order and generation, as chips along the top of the cards.
    fn order_chips(&self) -> Vec<(Hot, &'static str, bool, Rect)> {
        let v = &self.view;
        if v.feed.source != Source::Search {
            return Vec::new();
        }
        let mut items: Vec<(Hot, &'static str, bool)> = Sort::ALL.iter().map(|s| (Hot::Sort(*s), s.name(), v.feed.query.sort == *s)).collect();
        if v.feed.query.gear != Gear::Cab {
            items.push((Hot::Generation(false), "A2", !v.feed.query.a1));
            items.push((Hot::Generation(true), "A1", v.feed.query.a1));
        }
        // Right-aligned on the title row; widths from the letters (Manrope at 11.5 px).
        let mut x = self.grid_x1();
        let mut out = Vec::new();
        for (i, (h, name, on)) in items.into_iter().enumerate().rev() {
            let w = name.chars().count() as f64 * 6.6 + 22.0;
            let gap = if i + 1 == Sort::ALL.len() { 14.0 } else { 6.0 };
            out.push((h, name, on, Rect::new(x - w, TOP - 8.0, x, TOP + 16.0)));
            x -= w + gap;
        }
        out.reverse();
        out
    }

    fn side_rect(&self, i: usize) -> Rect {
        let rows = self.side_rows();
        // Headings take a little extra room above them.
        let mut y = SIDE.y0;
        for (k, (h, _, _)) in rows.iter().enumerate() {
            if *h == Hot::Close {
                y += 14.0;
            }
            if k == i {
                break;
            }
            y += if *h == Hot::Close { 22.0 } else { SIDE_ROW };
        }
        let heading = rows.get(i).is_some_and(|r| r.0 == Hot::Close);
        Rect::new(SIDE.x0, y, SIDE.x1, y + if heading { 22.0 } else { SIDE_ROW - 2.0 })
    }

    fn sign_in_button() -> Rect {
        let c = PANEL.center().x;
        Rect::new(c - 130.0, PANEL.y0 + 330.0, c + 130.0, PANEL.y0 + 376.0)
    }

    // ---- hit testing ----------------------------------------------------------------------------

    pub fn hot_at(&self, p: Point) -> Option<Hot> {
        if CLOSE.contains(p) {
            return Some(Hot::Close);
        }
        let v = &self.view;
        match v.phase {
            Phase::SignedIn => {}
            Phase::Code { .. } => return Self::sign_in_button().contains(p).then_some(Hot::OpenCode),
            Phase::SignedOut | Phase::Failed(_) => return Self::sign_in_button().contains(p).then_some(Hot::SignIn),
            _ => return None,
        }
        if SIGN_OUT.contains(p) {
            return Some(Hot::SignOut);
        }
        if SEARCH.contains(p) {
            return Some(Hot::Search);
        }
        for (i, (h, _, _)) in self.side_rows().iter().enumerate() {
            if *h != Hot::Close && self.side_rect(i).contains(p) {
                return Some(*h);
            }
        }
        if let Some((h, ..)) = self.order_chips().into_iter().find(|c| c.3.contains(p)) {
            return Some(h);
        }
        if v.tone.is_some() && self.detail().contains(p) {
            let d = self.detail();
            if Rect::new(d.x1 - 40.0, d.y0 + 8.0, d.x1 - 8.0, d.y0 + 40.0).contains(p) {
                return Some(Hot::DetailClose);
            }
            let heart = Rect::new(d.x0, d.y0 + 290.0, d.x0 + 120.0, d.y0 + 322.0);
            if heart.contains(p) {
                return Some(Hot::DetailHeart);
            }
            if Rect::new(d.x0 + 130.0, d.y0 + 290.0, d.x0 + 300.0, d.y0 + 322.0).contains(p) {
                return Some(Hot::DetailSite);
            }
            if p.y >= self.detail().y0 + 340.0 {
                for (i, m) in self.file_rows().iter().enumerate() {
                    for (action, r) in self.load_buttons(i, m) {
                        if r.contains(p) {
                            return Some(Hot::Load(i, action));
                        }
                    }
                }
            }
            return None;
        }
        if p.x >= GRID_X0 && p.x <= self.grid_x1() && p.y >= TOP {
            let tones = self.tones();
            if v.feed.error.is_some() && tones.is_empty() {
                return Some(Hot::Retry);
            }
            for i in 0..tones.len() {
                if self.heart_rect(i).contains(p) {
                    return Some(Hot::Heart(i));
                }
                if self.card_rect(i).contains(p) {
                    return Some(Hot::Card(i));
                }
            }
        }
        None
    }

    // ---- events -------------------------------------------------------------------------------

    /// Handles an event while the panel is open. Returns whether the editor should redraw, and
    /// whether the panel closes.
    pub fn event(&mut self, event: &UiEvent, ui: &Arc<RigUi>) -> (bool, bool) {
        match *event {
            UiEvent::KeyDown { key, mods } => {
                if key == Key::Escape {
                    if self.view.tone.is_some() {
                        ui.tone3000.close_tone();
                        return (true, false);
                    }
                    return (true, true);
                }
                if !self.focused || mods.ctrl || mods.meta {
                    return (false, false);
                }
                match key {
                    Key::Character(c) if !c.is_control() => {
                        if self.text.chars().count() < 80 {
                            self.text.push(c);
                        }
                        self.typed = Some(Instant::now());
                    }
                    Key::Backspace => {
                        self.text.pop();
                        self.typed = Some(Instant::now());
                    }
                    Key::Enter => {
                        self.typed = None;
                        self.run_search(ui);
                    }
                    _ => return (false, false),
                }
                (true, false)
            }
            UiEvent::PointerMove { pos, .. } => {
                self.hot = self.hot_at(pos);
                (true, false)
            }
            UiEvent::PointerLeft => {
                self.hot = None;
                (true, false)
            }
            UiEvent::Wheel { pos, lines, .. } => {
                if self.view.tone.is_some() && self.detail().contains(pos) {
                    let n = self.file_rows().len() as f64;
                    // The files, then up to 24 lines of description.
                    let about = if self.view.tone.as_ref().is_some_and(|t| !t.description.trim().is_empty()) { 50.0 + 24.0 * 18.0 } else { 0.0 };
                    let max = (384.0 + n.max(1.0) * FILE_ROW + about - self.detail().height() + 20.0).max(0.0);
                    self.detail_scroll = (self.detail_scroll - f64::from(lines) * 40.0).clamp(0.0, max);
                } else {
                    let max = (self.content_height() - (PANEL.y1 - self.grid_top())).max(0.0);
                    self.scroll = (self.scroll - f64::from(lines) * 60.0).clamp(0.0, max);
                }
                self.hot = self.hot_at(pos);
                (true, false)
            }
            UiEvent::PointerDown { pos, button: MouseButton::Left, .. } => {
                if !PANEL.contains(pos) {
                    return (true, true);
                }
                let hot = self.hot_at(pos);
                self.focused = hot == Some(Hot::Search);
                let Some(h) = hot else { return (true, false) };
                let t3k = &ui.tone3000;
                match h {
                    Hot::Close => return (true, true),
                    Hot::Search => {}
                    Hot::SignIn => t3k.sign_in(),
                    Hot::OpenCode => {
                        if let Phase::Code { uri, .. } = &self.view.phase {
                            tone3000::open_in_browser(uri);
                        }
                    }
                    Hot::SignOut => t3k.sign_out(),
                    Hot::Source(s) => {
                        self.scroll = 0.0;
                        t3k.close_tone();
                        t3k.search(s, self.view.feed.query.clone());
                    }
                    Hot::Gear(g) => self.refine(ui, Query { gear: g, ..self.view.feed.query.clone() }),
                    Hot::Sort(s) => self.refine(ui, Query { sort: s, ..self.view.feed.query.clone() }),
                    Hot::Generation(a1) => self.refine(ui, Query { a1, ..self.view.feed.query.clone() }),
                    Hot::Retry => t3k.search(self.view.feed.source, self.view.feed.query.clone()),
                    Hot::Card(i) => {
                        if let Some(t) = self.tones().get(i).cloned() {
                            self.detail_scroll = 0.0;
                            t3k.open_tone(t);
                        }
                    }
                    Hot::Heart(i) => {
                        if let Some(t) = self.tones().get(i) {
                            t3k.set_favorite(t.id, !t.is_favorite);
                        }
                    }
                    Hot::DetailClose => t3k.close_tone(),
                    Hot::DetailHeart => {
                        if let Some(t) = &self.view.tone {
                            t3k.set_favorite(t.id, !t.is_favorite);
                        }
                    }
                    Hot::DetailSite => {
                        if let Some(t) = &self.view.tone {
                            tone3000::open_in_browser(&t.url);
                        }
                    }
                    Hot::Load(i, action) => {
                        if let (Some(tone), Some(m)) = (self.view.tone.clone(), self.file_rows().get(i).cloned()) {
                            if self.view.downloading.is_none() {
                                let block = Some(self.target).filter(|_| action == Action::Load);
                                if let Some(block) = block {
                                    self.notice = Some((format!("{} into the {}", m.name, self.kind.name()), Instant::now()));
                                    t3k.download(tone, m, ui.board.clone(), block);
                                }
                            }
                        }
                    }
                }
                self.view = ui.tone3000.view();
                (true, false)
            }
            _ => (false, false),
        }
    }

    fn refine(&mut self, ui: &RigUi, q: Query) {
        self.scroll = 0.0;
        ui.tone3000.close_tone();
        let q = Query { text: self.text.clone(), ..q };
        ui.tone3000.search(Source::Search, q);
    }

    // ---- drawing ------------------------------------------------------------------------------

    fn brush(&mut self, ui: &Arc<RigUi>, url: &str) -> Option<ImageBrush> {
        if let Some(b) = self.brushes.get(url) {
            return Some(b.clone());
        }
        let thumb = ui.tone3000.image(url)?;
        let image = ImageData { data: Blob::new(thumb.rgba.clone()), format: ImageFormat::Rgba8, alpha_type: ImageAlphaType::Alpha, width: thumb.width, height: thumb.height };
        let brush = ImageBrush::new(image).with_quality(ImageQuality::Medium);
        if self.brushes.len() > 400 {
            self.brushes.clear();
        }
        self.brushes.insert(url.to_owned(), brush.clone());
        Some(brush)
    }

    /// The cover filling `rect` (cropped to it), or a quiet placeholder with the gear's initial.
    fn cover(&mut self, cx: &mut DrawCx<'_>, ui: &Arc<RigUi>, tone: &Tone, shape: RoundedRect) {
        let theme = cx.theme;
        let r = shape.rect();
        cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.well, None, &shape);
        let brush = tone.image.as_deref().and_then(|u| self.brush(ui, u));
        match brush {
            Some(b) => {
                let (w, h) = (f64::from(b.image.width), f64::from(b.image.height));
                let k = (r.width() / w).max(r.height() / h);
                let t = Affine::translate((r.center().x - k * w / 2.0, r.center().y - k * h / 2.0)) * Affine::scale(k);
                cx.scene.fill(Fill::NonZero, Affine::IDENTITY, &b, Some(t), &shape);
            }
            None => {
                let initial = tone.gear_name().chars().next().unwrap_or('T').to_string();
                cx.text.draw(cx.scene, &initial, r.center().x, r.center().y - 22.0, TextStyle::BODY.sized(34.0).weighted(800.0), theme.text_disabled, Align::Center);
            }
        }
    }

    fn chip(cx: &mut DrawCx<'_>, x: f64, y: f64, text: &str, lit: bool) -> f64 {
        let theme = cx.theme;
        let style = TextStyle::LABEL.sized(9.5);
        let w = f64::from(cx.text.measure(text, style)) + 14.0;
        let r = RoundedRect::new(x, y, x + w, y + 18.0, 9.0);
        let color = if lit { theme.accent } else { theme.text_lo };
        cx.scene.fill(Fill::NonZero, Affine::IDENTITY, color.multiply_alpha(0.14), None, &r);
        cx.text.draw(cx.scene, text, x + 7.0, y + 3.5, style, color, Align::Left);
        w
    }

    pub fn draw(&mut self, cx: &mut DrawCx<'_>, ui: &Arc<RigUi>) {
        let theme = cx.theme;
        let shape = RoundedRect::from_rect(PANEL, 14.0);
        glass_panel(cx, shape, theme.blur_panel);
        cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.display.multiply_alpha(0.96), None, &shape);
        cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, theme.glass_border, None, &shape);
        // TONE3000's name, as its terms ask.
        cx.text.draw(cx.scene, "TONE3000", PANEL.x0 + 22.0, PANEL.y0 + 17.0, TextStyle::BODY.sized(19.0).weighted(800.0), theme.text_strong, Align::Left);
        cx.text.draw(cx.scene, "Powered by TONE3000", PANEL.x0 + 22.0, PANEL.y0 + 40.0, TextStyle::BODY.sized(9.5), theme.text_lo, Align::Left);
        let ink = icon_button(cx.scene, theme, CLOSE, theme.control, self.hot == Some(Hot::Close), false, theme.accent);
        glyph(cx.scene, "M7.5 7.5 L16.5 16.5 M16.5 7.5 L7.5 16.5", CLOSE.center(), 20.0, ink, 1.7, false);
        let v = self.view.clone();
        match &v.phase {
            Phase::SignedIn => {}
            Phase::NoKey => {
                cx.text.draw(cx.scene, "This build has no TONE3000 key.", PANEL.center().x, PANEL.y0 + 260.0, TextStyle::BODY.sized(14.0), theme.text_mid, Align::Center);
                return;
            }
            Phase::Code { code, .. } => {
                let c = PANEL.center().x;
                cx.text.draw(cx.scene, "Confirm it's you on TONE3000", c, PANEL.y0 + 150.0, TextStyle::BODY.sized(18.0).weighted(700.0), theme.text_strong, Align::Center);
                cx.text.draw(cx.scene, "Your browser opened TONE3000's sign-in page. Sign in there and check it shows this code:", c, PANEL.y0 + 184.0, TextStyle::BODY.sized(13.0), theme.text_mid, Align::Center);
                cx.text.draw(cx.scene, code, c, PANEL.y0 + 222.0, TextStyle { tabular: true, ..TextStyle::BODY.sized(44.0).weighted(800.0) }, theme.accent, Align::Center);
                cx.text.draw(cx.scene, "This is needed only once: Mite Amp stays signed in, and everything else happens here.", c, PANEL.y0 + 290.0, TextStyle::BODY.sized(12.0), theme.text_lo, Align::Center);
                let r = Self::sign_in_button();
                let ink = icon_button(cx.scene, theme, r, theme.control, self.hot == Some(Hot::OpenCode), false, theme.accent);
                cx.text.draw(cx.scene, "Open the sign-in page again", r.center().x, r.center().y - 8.0, TextStyle::BODY.sized(13.0).weighted(700.0), ink, Align::Center);
                return;
            }
            phase => {
                let c = PANEL.center().x;
                cx.text.draw(cx.scene, "Thousands of amps, pedals and cabs, free", c, PANEL.y0 + 150.0, TextStyle::BODY.sized(20.0).weighted(700.0), theme.text_strong, Align::Center);
                let (msg, color) = match phase {
                    Phase::Failed(e) => (e.clone(), theme.danger),
                    Phase::Starting => ("Asking TONE3000 for a sign-in code…".to_owned(), theme.text_mid),
                    _ => ("Sign in once with your free TONE3000 account; after that you search, pick and load everything right here.".to_owned(), theme.text_mid),
                };
                cx.text.draw(cx.scene, &msg, c, PANEL.y0 + 190.0, TextStyle::BODY.sized(13.0), color, Align::Center);
                cx.text.draw(cx.scene, "Signing in opens TONE3000's page in your browser one time, to confirm it's you.", c, PANEL.y0 + 214.0, TextStyle::BODY.sized(12.0), theme.text_lo, Align::Center);
                if *phase != Phase::Starting {
                    let r = Self::sign_in_button();
                    let ink = icon_button(cx.scene, theme, r, theme.control, self.hot == Some(Hot::SignIn), true, theme.accent);
                    cx.text.draw(cx.scene, "Sign in to TONE3000", r.center().x, r.center().y - 8.5, TextStyle::BODY.sized(14.0).weighted(700.0), ink, Align::Center);
                }
                return;
            }
        }
        self.draw_search(cx);
        let ink = icon_button(cx.scene, theme, SIGN_OUT, theme.control, self.hot == Some(Hot::SignOut), false, theme.accent);
        cx.text.draw(cx.scene, "Sign out", SIGN_OUT.center().x, SIGN_OUT.center().y - 7.5, TextStyle::BODY.sized(12.0).weighted(600.0), ink, Align::Center);
        if let Some(user) = &v.user {
            let u = fit(cx, user, TextStyle::BODY.sized(12.5).weighted(700.0), 220.0);
            cx.text.draw(cx.scene, &u, SIGN_OUT.x0 - 12.0, SIGN_OUT.center().y - 8.0, TextStyle::BODY.sized(12.5).weighted(700.0), theme.text_hi, Align::Right);
        }
        self.draw_side(cx);
        self.draw_grid(cx, ui, &v);
        if v.tone.is_some() {
            self.draw_detail(cx, ui, &v);
        }
        if let Some((text, _)) = &self.notice {
            let t = format!("Loading {text}");
            let w = f64::from(cx.text.measure(&t, TextStyle::BODY.sized(12.5).weighted(700.0))) + 32.0;
            let c = PANEL.center().x;
            let r = RoundedRect::new(c - w / 2.0, PANEL.y1 - 54.0, c + w / 2.0, PANEL.y1 - 20.0, 17.0);
            cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.accent, None, &r);
            cx.text.draw(cx.scene, &t, c, PANEL.y1 - 45.0, TextStyle::BODY.sized(12.5).weighted(700.0), theme.display, Align::Center);
        }
    }

    fn draw_search(&self, cx: &mut DrawCx<'_>) {
        let theme = cx.theme;
        let r = RoundedRect::from_rect(SEARCH, 18.0);
        let lit = self.focused || self.hot == Some(Hot::Search);
        cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.well, None, &r);
        cx.scene.stroke(&Stroke::new(if self.focused { 1.5 } else { 1.0 }), Affine::IDENTITY, if lit { theme.accent } else { theme.well_border }, None, &r);
        glyph(cx.scene, SEARCH_GLYPH, Point::new(SEARCH.x0 + 20.0, SEARCH.center().y), 18.0, theme.text_mid, 1.7, false);
        let style = TextStyle::BODY.sized(13.5);
        let x = SEARCH.x0 + 38.0;
        if self.text.is_empty() && !self.focused {
            let prompt = match self.kind {
                Kind::Pedal => "Search pedals: drives, boosts, fuzzes, makers…",
                Kind::Cab => "Search cabinets and IRs, speakers, makers…",
                _ => "Search amps and combos, makers…",
            };
            cx.text.draw(cx.scene, prompt, x, SEARCH.center().y - 8.5, style, theme.text_lo, Align::Left);
        } else {
            let w = cx.text.draw(cx.scene, &self.text, x, SEARCH.center().y - 8.5, style, theme.text_strong, Align::Left);
            if self.focused && (self.started.elapsed().as_millis() / 530) % 2 == 0 {
                let cx0 = x + f64::from(w) + 1.5;
                cx.scene.stroke(&Stroke::new(1.3), Affine::IDENTITY, theme.accent, None, &Line::new((cx0, SEARCH.y0 + 9.0), (cx0, SEARCH.y1 - 9.0)));
            }
        }
        let f = &self.view.feed;
        if f.source == Source::Search || f.total > 0 {
            let count = if f.loading && f.tones.is_empty() {
                "Searching…".to_owned()
            } else if f.total_pages > 1 {
                format!("Showing {} of {} tones · scroll for more", f.tones.len(), thousands(f.total))
            } else {
                format!("{} tones", f.total)
            };
            cx.text.draw(cx.scene, &count, SEARCH.x1 + 14.0, SEARCH.center().y - 7.5, TextStyle::BODY.sized(12.0), theme.text_lo, Align::Left);
        }
    }

    fn draw_side(&self, cx: &mut DrawCx<'_>) {
        let theme = cx.theme;
        for (i, (h, name, on)) in self.side_rows().into_iter().enumerate() {
            let r = self.side_rect(i);
            if h == Hot::Close {
                cx.text.draw(cx.scene, &name, r.x0 + 10.0, r.y0 + 6.0, TextStyle::LABEL.sized(9.0), theme.text_lo, Align::Left);
                continue;
            }
            let big = matches!(h, Hot::Source(_));
            let hot = self.hot == Some(h);
            if on || hot {
                cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.accent.multiply_alpha(if on { 0.16 } else { 0.08 }), None, &RoundedRect::from_rect(r, 8.0));
            }
            let style = if big { TextStyle::BODY.sized(13.5).weighted(700.0) } else { TextStyle::BODY.sized(12.5) };
            cx.text.draw(cx.scene, &name, r.x0 + 10.0, r.center().y - if big { 8.5 } else { 8.0 }, style, if on { theme.accent } else if big { theme.text_hi } else { theme.text_mid }, Align::Left);
        }
        cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, theme.line, None, &Line::new((SIDE.x1 + 6.0, SIDE.y0), (SIDE.x1 + 6.0, SIDE.y1)));
    }

    fn draw_grid(&mut self, cx: &mut DrawCx<'_>, ui: &Arc<RigUi>, v: &View) {
        let theme = cx.theme;
        let area = Rect::new(GRID_X0 - 4.0, TOP, self.grid_x1() + 4.0, PANEL.y1 - 10.0);
        let f = &v.feed;
        let title = match f.source {
            Source::Search if !f.query.text.trim().is_empty() => format!("“{}”", f.query.text.trim()),
            Source::Search => format!("{} · {}", f.query.gear.name(), f.query.sort.name()),
            s => s.name().to_owned(),
        };
        let chips = self.order_chips();
        let room = chips.first().map_or(self.grid_x1(), |c| c.3.x0) - GRID_X0 - 12.0;
        let title = fit(cx, &title, TextStyle::BODY.sized(13.5).weighted(700.0), room);
        cx.text.draw(cx.scene, &title, GRID_X0, TOP - 2.0, TextStyle::BODY.sized(13.5).weighted(700.0), theme.text_hi, Align::Left);
        for (h, name, on, r) in chips {
            let hot = self.hot == Some(h);
            let shape = RoundedRect::from_rect(r, 12.0);
            let color = if on { theme.accent } else if hot { theme.text_hi } else { theme.text_lo };
            cx.scene.fill(Fill::NonZero, Affine::IDENTITY, color.multiply_alpha(if on { 0.16 } else if hot { 0.10 } else { 0.06 }), None, &shape);
            cx.text.draw(cx.scene, name, r.center().x, r.y0 + 4.5, TextStyle::BODY.sized(11.5).weighted(if on { 700.0 } else { 600.0 }), color, Align::Center);
        }
        let shown = self.tones();
        if shown.is_empty() && !(f.loading || f.more()) || shown.is_empty() && f.error.is_some() {
            let (msg, color) = match (&f.error, f.loading) {
                (Some(e), _) => (format!("{e} — click to try again"), theme.danger),
                (None, true) => ("Loading…".to_owned(), theme.text_mid),
                (None, false) => (match f.source {
                    Source::Favorites => "No favourites yet: tap a card's heart and it lands here.",
                    Source::Downloaded => "Nothing downloaded yet.",
                    Source::Created => "You have not published tones.",
                    Source::Search => "No tones match. Try fewer words or another filter.",
                }.to_owned(), theme.text_lo),
            };
            cx.text.draw(cx.scene, &msg, (GRID_X0 + self.grid_x1()) / 2.0, TOP + 140.0, TextStyle::BODY.sized(13.0), color, Align::Center);
            return;
        }
        cx.begin_clip(&Rect::new(area.x0, self.grid_top() - 4.0, area.x1, area.y1));
        let tones = shown;
        for (i, tone) in tones.iter().enumerate() {
            let r = self.card_rect(i);
            if r.y1 < area.y0 || r.y0 > area.y1 {
                continue;
            }
            let selected = v.tone.as_ref().is_some_and(|t| t.id == tone.id);
            let hot = self.hot == Some(Hot::Card(i)) || self.hot == Some(Hot::Heart(i));
            let shape = RoundedRect::from_rect(r, 12.0);
            cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.panel_control, None, &shape);
            cx.scene.stroke(&Stroke::new(if selected { 2.0 } else { 1.0 }), Affine::IDENTITY, if selected { theme.accent } else if hot { theme.accent.multiply_alpha(0.6) } else { theme.panel_control_border }, None, &shape);
            let img = RoundedRect::new(r.x0 + 1.0, r.y0 + 1.0, r.x1 - 1.0, r.y0 + IMAGE_H, mite_ui::kurbo::RoundedRectRadii::new(11.0, 11.0, 0.0, 0.0));
            self.cover(cx, ui, tone, img);
            // The heart, over the cover.
            let hr = self.heart_rect(i);
            cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.display.multiply_alpha(0.7), None, &Circle::new(hr.center(), 15.0));
            let heart_ink = if tone.is_favorite { theme.accent } else if self.hot == Some(Hot::Heart(i)) { theme.text_strong } else { theme.text_mid };
            glyph(cx.scene, HEART, hr.center(), 18.0, heart_ink, 1.6, tone.is_favorite);
            // Gear and generation chips.
            let mut x = r.x0 + 10.0;
            let y = r.y0 + IMAGE_H - 26.0;
            let gear = tone.gear_name();
            let chips: Vec<String> = [Some(gear), (tone.a2_models > 0).then(|| "A2".to_owned()), (tone.irs > 0 && tone.format == "ir").then(|| format!("{} IR", tone.irs))].into_iter().flatten().collect();
            for c in chips {
                let style = TextStyle::LABEL.sized(9.5);
                let w = f64::from(cx.text.measure(&c, style)) + 14.0;
                let cr = RoundedRect::new(x, y, x + w, y + 18.0, 9.0);
                cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.display.multiply_alpha(0.82), None, &cr);
                cx.text.draw(cx.scene, &c, x + 7.0, y + 3.5, style, theme.text_hi, Align::Left);
                x += w + 6.0;
            }
            let tw = r.width() - 24.0;
            let t = fit(cx, &tone.title, TextStyle::BODY.sized(13.5).weighted(700.0), tw);
            cx.text.draw(cx.scene, &t, r.x0 + 12.0, r.y0 + IMAGE_H + 10.0, TextStyle::BODY.sized(13.5).weighted(700.0), theme.text_strong, Align::Left);
            let make = tone.makes.first().cloned().unwrap_or_default();
            let line = if make.is_empty() { format!("by {}", tone.creator) } else { format!("{make} · by {}", tone.creator) };
            let l = fit(cx, &line, TextStyle::BODY.sized(11.5), tw);
            cx.text.draw(cx.scene, &l, r.x0 + 12.0, r.y0 + IMAGE_H + 31.0, TextStyle::BODY.sized(11.5), theme.text_mid, Align::Left);
            let n = if tone.format == "ir" { tone.irs.max(tone.models) } else { tone.models };
            let files = match (tone.format == "ir", n) {
                (true, 1) => "1 IR".to_owned(),
                (true, n) => format!("{n} IRs"),
                (false, 1) => "1 capture".to_owned(),
                (false, n) => format!("{n} captures"),
            };
            cx.text.draw(cx.scene, &files, r.x0 + 12.0, r.y0 + IMAGE_H + 50.0, TextStyle::BODY.sized(11.0), theme.text_lo, Align::Left);
            // Counts along the bottom.
            let by = r.y1 - 26.0;
            glyph(cx.scene, HEART, Point::new(r.x0 + 20.0, by + 8.0), 13.0, theme.text_lo, 1.4, false);
            cx.text.draw(cx.scene, &thousands(tone.favorites), r.x0 + 31.0, by + 1.0, TextStyle::BODY.sized(11.0).weighted(600.0), theme.text_mid, Align::Left);
            glyph(cx.scene, DOWN_GLYPH, Point::new(r.x0 + 82.0, by + 8.0), 13.0, theme.text_lo, 1.5, false);
            cx.text.draw(cx.scene, &thousands(tone.downloads), r.x0 + 93.0, by + 1.0, TextStyle::BODY.sized(11.0).weighted(600.0), theme.text_mid, Align::Left);
            if !tone.license.is_empty() {
                cx.text.draw(cx.scene, &tone.license.to_uppercase(), r.x1 - 12.0, by + 1.0, TextStyle::LABEL.sized(9.0), theme.text_lo, Align::Right);
            }
        }
        if f.loading {
            let y = self.card_rect(self.tones().len()).y0 + 20.0;
            cx.text.draw(cx.scene, "Loading more…", (GRID_X0 + self.grid_x1()) / 2.0, y.min(area.y1 - 24.0), TextStyle::BODY.sized(12.0), theme.text_mid, Align::Center);
        }
        cx.end_clip();
    }

    fn draw_detail(&mut self, cx: &mut DrawCx<'_>, ui: &Arc<RigUi>, v: &View) {
        let Some(tone) = v.tone.clone() else { return };
        let theme = cx.theme;
        let d = self.detail();
        let shape = RoundedRect::from_rect(d.inflate(6.0, 6.0), 14.0);
        cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.panel_control, None, &shape);
        cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, theme.panel_control_border, None, &shape);
        self.cover(cx, ui, &tone, RoundedRect::from_rect(self.detail_image(), 10.0));
        let close = Rect::new(d.x1 - 40.0, d.y0 + 8.0, d.x1 - 8.0, d.y0 + 40.0);
        cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.display.multiply_alpha(0.75), None, &Circle::new(close.center(), 15.0));
        glyph(cx.scene, "M8 8 L16 16 M16 8 L8 16", close.center(), 16.0, if self.hot == Some(Hot::DetailClose) { theme.text_strong } else { theme.text_mid }, 1.7, false);
        let mut y = d.y0 + 212.0;
        let title = fit(cx, &tone.title, TextStyle::BODY.sized(17.0).weighted(800.0), d.width());
        cx.text.draw(cx.scene, &title, d.x0, y, TextStyle::BODY.sized(17.0).weighted(800.0), theme.text_strong, Align::Left);
        y += 26.0;
        let mut by = format!("by {}", tone.creator);
        if let Some(make) = tone.makes.first() {
            by = format!("{make} · {by}");
        }
        let by = fit(cx, &by, TextStyle::BODY.sized(12.0), d.width());
        cx.text.draw(cx.scene, &by, d.x0, y, TextStyle::BODY.sized(12.0), theme.text_mid, Align::Left);
        y += 22.0;
        let mut x = d.x0;
        let mut chips = vec![tone.gear_name()];
        if !tone.license.is_empty() {
            chips.push(format!("License {}", tone.license.to_uppercase()));
        }
        chips.extend(tone.tags.iter().take(3).cloned());
        for c in chips {
            if x > d.x1 - 60.0 {
                break;
            }
            x += Self::chip(cx, x, y, &c, false) + 6.0;
        }
        // Favourite and the tone's page.
        let heart = Rect::new(d.x0, d.y0 + 290.0, d.x0 + 120.0, d.y0 + 322.0);
        let ink = icon_button(cx.scene, theme, heart, theme.control, self.hot == Some(Hot::DetailHeart), tone.is_favorite, theme.accent);
        glyph(cx.scene, HEART, Point::new(heart.x0 + 20.0, heart.center().y), 16.0, ink, 1.6, tone.is_favorite);
        cx.text.draw(cx.scene, if tone.is_favorite { "Favorite" } else { "Add favorite" }, heart.x0 + 34.0, heart.center().y - 7.5, TextStyle::BODY.sized(12.0).weighted(700.0), ink, Align::Left);
        let site = Rect::new(d.x0 + 130.0, d.y0 + 290.0, d.x0 + 300.0, d.y0 + 322.0);
        let site_ink = if self.hot == Some(Hot::DetailSite) { theme.accent } else { theme.text_lo };
        cx.text.draw(cx.scene, "View on tone3000.com", site.x0 + 4.0, site.center().y - 7.5, TextStyle::BODY.sized(11.5), site_ink, Align::Left);
        // The files, then the description under them.
        cx.begin_clip(&Rect::new(d.x0 - 6.0, d.y0 + 334.0, d.x1 + 6.0, d.y1));
        let ft = self.files_top();
        cx.text.draw(cx.scene, "FILES", d.x0, ft - 30.0, TextStyle::LABEL.sized(9.0), theme.text_lo, Align::Left);
        let files = self.file_rows();
        let playing: Vec<std::path::PathBuf> = ui.board.blocks().iter().filter_map(|b| b.live.file().path).collect();
        match &v.models {
            None => {
                cx.text.draw(cx.scene, "Loading files…", d.x0, ft + 8.0, TextStyle::BODY.sized(12.0), theme.text_mid, Align::Left);
            }
            Some(_) if files.is_empty() => {
                let msg = v.models_error.clone().unwrap_or_else(|| "No files this plugin can play.".to_owned());
                cx.text.draw(cx.scene, &msg, d.x0, ft + 8.0, TextStyle::BODY.sized(12.0), theme.text_mid, Align::Left);
            }
            Some(_) => {
                for (i, m) in files.iter().enumerate() {
                    let y = ft + i as f64 * FILE_ROW;
                    if i > 0 {
                        cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, theme.line, None, &Line::new((d.x0, y), (d.x1, y)));
                    }
                    let buttons = self.load_buttons(i, m);
                    let text_w = buttons.last().map_or(d.x1, |(_, r)| r.x0) - d.x0 - 52.0;
                    let badge = if m.kind == "IR" { "IR".to_owned() } else { m.kind.clone() };
                    Self::chip(cx, d.x0, y + 12.0, &badge, m.kind == "A2");
                    let local = tone3000::local_path(&tone, m);
                    let in_use = local.as_ref().is_some_and(|p| playing.contains(p));
                    let downloading = v.downloading == Some(m.id);
                    let name = fit(cx, &m.name, TextStyle::BODY.sized(12.5).weighted(600.0), text_w);
                    cx.text.draw(cx.scene, &name, d.x0 + 44.0, y + 6.0, TextStyle::BODY.sized(12.5).weighted(600.0), theme.text_strong, Align::Left);
                    let sub = if downloading {
                        "Downloading…".to_owned()
                    } else if in_use {
                        "On the board".to_owned()
                    } else if local.as_ref().is_some_and(|p| p.is_file()) {
                        "On this computer".to_owned()
                    } else if m.size.is_empty() {
                        String::new()
                    } else {
                        m.size.clone()
                    };
                    let sub_color = if downloading || in_use { theme.accent } else { theme.text_lo };
                    cx.text.draw(cx.scene, &sub, d.x0 + 44.0, y + 23.0, TextStyle::BODY.sized(10.5), sub_color, Align::Left);
                    for (action, r) in buttons {
                        let ink = icon_button(cx.scene, theme, r, theme.control, self.hot == Some(Hot::Load(i, action)), true, theme.accent);
                        let word = "Load";
                        cx.text.draw(cx.scene, word, r.center().x, r.center().y - 7.5, TextStyle::BODY.sized(12.0).weighted(700.0), ink, Align::Center);
                    }
                }
            }
        }
        if let Some(e) = &v.download_error {
            cx.text.draw(cx.scene, e, d.x0, ft + files.len() as f64 * FILE_ROW + 8.0, TextStyle::BODY.sized(11.5), theme.danger, Align::Left);
        }
        if !tone.description.trim().is_empty() {
            let top = ft + files.len().max(1) as f64 * FILE_ROW + 30.0;
            cx.text.draw(cx.scene, "ABOUT", d.x0, top, TextStyle::LABEL.sized(9.0), theme.text_lo, Align::Left);
            let lines = wrap(cx, &tone.description, TextStyle::BODY.sized(12.0), d.width(), 24);
            for (k, l) in lines.iter().enumerate() {
                cx.text.draw(cx.scene, l, d.x0, top + 20.0 + k as f64 * 18.0, TextStyle::BODY.sized(12.0), theme.text_mid, Align::Left);
            }
        }
        cx.end_clip();
    }

    /// The help strip's words for where the pointer is.
    pub fn help(&self) -> &'static str {
        match self.hot {
            Some(Hot::Search) => "Type to search TONE3000 (it searches when you pause; Enter searches now)",
            Some(Hot::Card(_)) => "Click to see this tone's files and load one",
            Some(Hot::Heart(_)) | Some(Hot::DetailHeart) => "Favorite: it's kept in your TONE3000 favorites, here and on the site",
            Some(Hot::Load(_, Action::Load)) => "Load it into the block (downloaded once, then kept on this computer); the rig keeps playing so you can compare",
            Some(Hot::DetailSite) => "Opens this tone's page in your browser",
            Some(Hot::Gear(_)) | Some(Hot::Sort(_)) | Some(Hot::Generation(_)) => "Narrow the search",
            Some(Hot::SignOut) => "Sign out of TONE3000 on this computer",
            _ => "TONE3000: search, pick a tone, load its files straight into the rig. Esc or × closes.",
        }
    }
}
