//! The crossover rail shared by the multiband plugins (Shape, Sat): band regions over a log
//! frequency axis, a grip per crossover, ghost grips at both ends to pull a new band in, number
//! badges, merging by dragging a crossover onto its neighbour or off an end, the band menu, and
//! the band operations (split, merge, remove, solo, mute) on the host's parameters.
//!
//! Slots never swap (`mite_dsp::crossover`): crossover slot k owns band slot k + 1, band slot 0 is
//! always the lowest region, and the editor numbers bands by frequency. `B` is the most bands.

use vello::kurbo::{Affine, BezPath, Cap, Circle, Join, Line, Point, Rect, RoundedRect, Stroke};
use vello::peniko::{Color, Fill};
use vello::Scene;

use mite_core::ParamSpec;
use mite_dsp::crossover::{Layout, FREQ_MAX, FREQ_MIN, MIN_GAP_OCT};

use crate::paint::glow_stroke;
use crate::widgets::{Dropdown, DropdownEvent, Help};
use crate::{write_if_changed, Align, DrawCx, Modifiers, ParamHost, TextStyle, UiEvent};

/// A crossover grip (14×28) and its hit half-width.
const GRIP: (f64, f64) = (14.0, 28.0);
const GRIP_HIT: f64 = 16.0;
/// A crossover dragged this close to a neighbour (px) merges on release; past an end by this
/// much, it removes the end band.
const MERGE_PX: f64 = 6.0;
const OFF_END_PX: f64 = 10.0;
/// A pull from a ghost grip this far (px) makes a crossover on release.
const PULL_PX: f64 = 6.0;

/// Glyphs (24×24, stroked; docs/design/mockups/shape-main.dc.html `G`).
pub mod glyph {
    pub const MUTE: &str = "M4 9.5 H7.5 L12 5.5 V18.5 L7.5 14.5 H4 Z M15.5 9.5 L20.5 14.5 M20.5 9.5 L15.5 14.5";
    pub const BIN: &str = "M5 7 H19 M10 7 V5 H14 V7 M7 7 L8 20 H16 L17 7 M10.5 10.5 V16.5 M13.5 10.5 V16.5";
    pub const RESET: &str = "M5 12 A7 7 0 1 0 7.1 7 M5 4.5 V8.5 H9";
    pub const COPY: &str = "M8 8 H19 V20 H8 Z M5 16 V4 H15";
    pub const PASTE: &str = "M7 5 H5 V21 H19 V5 H17 M9 3 H15 V7 H9 Z";
    pub const MERGE: &str = "M4 6 C10 6 11 12 20 12 M4 18 C10 18 11 12 20 12 M17 9 L20 12 L17 15";
    pub const SOLO: &str = "M12 3.5 A8.5 8.5 0 1 0 12.01 3.5";
    pub const CROSS: &str = "M7 7 L17 17 M17 7 L7 17";
}

/// Draws a 24×24 glyph scaled to `size`, centred on `c`.
pub fn draw_glyph(scene: &mut Scene, d: &str, c: Point, size: f64, color: Color, width: f64) {
    let Ok(mut path) = BezPath::from_svg(d) else { return };
    let k = size / 24.0;
    path.apply_affine(Affine::translate((c.x - 12.0 * k, c.y - 12.0 * k)) * Affine::scale(k));
    let stroke = Stroke::new(width).with_caps(Cap::Round).with_join(Join::Round);
    scene.stroke(&stroke, Affine::IDENTITY, color, None, &path);
}

pub fn fmt_freq(f: f64) -> String {
    if f >= 10_000.0 {
        format!("{:.0} kHz", f / 1000.0)
    } else if f >= 1000.0 {
        format!("{:.1} kHz", f / 1000.0)
    } else {
        format!("{:.0} Hz", f)
    }
}

/// How a plugin names its crossover and band parameters, and copies a band's sound.
#[derive(Clone, Copy)]
pub struct RailParams {
    pub cross_on: fn(usize) -> String,
    pub cross_freq: fn(usize) -> String,
    /// The crossover frequency's spec (wheel steps).
    pub freq: ParamSpec,
    pub solo: fn(usize) -> String,
    pub mute: fn(usize) -> String,
    /// Copies band `from`'s sound settings (mute included, solo not) onto band `to`.
    pub copy: fn(&dyn ParamHost, usize, usize),
}

/// What the pointer is over on the rail.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RailHot {
    Grip(usize),
    /// A ghost grip: the low end (true) or the high end.
    Ghost(bool),
    Region(usize),
}

/// What dragging a crossover onto something does on release.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Merge {
    /// Onto the crossover below (its slot): the band between them goes, the one above grows.
    Down(usize),
    /// Onto the one above: the band between them goes, the one below grows.
    Up,
    /// Off the low end (the lowest band goes) or the high end (the highest).
    OffLow,
    OffHigh,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum RailDrag {
    Cross { slot: usize, start: Point, f0: f64, merge: Option<Merge> },
    /// Pulling a new crossover in from an end.
    Create { start: Point, now: Point },
}

pub struct Rail<const B: usize> {
    params: RailParams,
    /// The strip: band regions span its width.
    pub area: Rect,
    /// The frequency axis: 20 Hz at `x0`, 20 kHz at `x0 + w`.
    x0: f64,
    w: f64,
    /// The band slot being edited (the wide card).
    pub selected: usize,
    /// Region tint: the selected band's and the others'.
    pub tint: (f32, f32),
    /// The 100 / 1k / 10k labels along the strip's bottom (off when a plot above has them).
    pub labels: bool,
    hot: Option<RailHot>,
    drag: Option<RailDrag>,
}

impl<const B: usize> Rail<B> {
    pub fn new(params: RailParams, area: Rect, x0: f64, w: f64) -> Self {
        Self { params, area, x0, w, selected: 0, tint: (0.22, 0.10), labels: true, hot: None, drag: None }
    }

    pub fn x(&self, f: f64) -> f64 {
        self.x0 + self.w * (f / f64::from(FREQ_MIN)).ln() / 1000f64.ln()
    }

    pub fn f(&self, x: f64) -> f64 {
        f64::from(FREQ_MIN) * 1000f64.powf(((x - self.x0) / self.w).clamp(0.0, 1.0))
    }

    fn plain(host: &dyn ParamHost, id: &str) -> f32 {
        host.plain(id, host.normalized(id))
    }

    fn on(host: &dyn ParamHost, id: &str) -> bool {
        Self::plain(host, id) >= 0.5
    }

    fn set(host: &dyn ParamHost, id: &str, plain: f32) {
        write_if_changed(host, id, host.normalize(id, plain));
    }

    /// Each crossover slot's frequency while on (the last entry is unused).
    pub fn crossovers(&self, host: &dyn ParamHost) -> [Option<f32>; B] {
        std::array::from_fn(|k| (k + 1 < B && Self::on(host, &(self.params.cross_on)(k))).then(|| Self::plain(host, &(self.params.cross_freq)(k))))
    }

    pub fn layout(&self, host: &dyn ParamHost) -> Layout<B> {
        Layout::new(&self.crossovers(host)[..B - 1])
    }

    /// The band being edited: the selected one while it is in use, else the lowest.
    pub fn current(&self, layout: &Layout<B>) -> usize {
        if layout.position(self.selected).is_some() { self.selected } else { 0 }
    }

    /// The soloed band, if any (the first in use).
    pub fn soloed(&self, host: &dyn ParamHost, layout: &Layout<B>) -> Option<usize> {
        layout.bands().iter().copied().find(|&b| Self::on(host, &(self.params.solo)(b)))
    }

    pub fn is_solo(&self, host: &dyn ParamHost, b: usize) -> bool {
        Self::on(host, &(self.params.solo)(b))
    }

    pub fn is_muted(&self, host: &dyn ParamHost, b: usize) -> bool {
        Self::on(host, &(self.params.mute)(b))
    }

    pub fn audible(&self, host: &dyn ParamHost, layout: &Layout<B>, b: usize) -> bool {
        !self.is_muted(host, b) && self.soloed(host, layout).is_none_or(|s| s == b)
    }

    /// Band `b`'s range, e.g. "120 Hz – 500 Hz".
    pub fn range_text(&self, host: &dyn ParamHost, layout: &Layout<B>, b: usize) -> String {
        let x = self.crossovers(host);
        layout.region(b, &x[..B - 1]).map_or(String::new(), |(lo, hi)| format!("{} – {}", fmt_freq(f64::from(lo)), fmt_freq(f64::from(hi))))
    }

    // ---- geometry ---------------------------------------------------------------------------

    /// The region of the band at position `p` (from the bottom); the end bands reach the
    /// strip's ends.
    pub fn region_rect(&self, host: &dyn ParamHost, layout: &Layout<B>, p: usize) -> Rect {
        let x = self.crossovers(host);
        let edge = |q: usize| self.x(f64::from(x[layout.crossovers[q]].unwrap_or(FREQ_MIN)));
        let x0 = if p == 0 { self.area.x0 } else { edge(p - 1) };
        let x1 = if p == layout.n { self.area.x1 } else { edge(p) };
        Rect::new(x0, self.area.y0, x1, self.area.y1)
    }

    fn grip_center(&self, host: &dyn ParamHost, slot: usize) -> Point {
        let f = Self::plain(host, &(self.params.cross_freq)(slot));
        Point::new(self.x(f64::from(f)), self.area.center().y)
    }

    fn ghost_centers(&self) -> [Point; 2] {
        let y = self.area.center().y;
        [Point::new(self.x(f64::from(FREQ_MIN)) + 6.0, y), Point::new(self.x(f64::from(FREQ_MAX)) - 6.0, y)]
    }

    // ---- band operations --------------------------------------------------------------------

    pub fn toggle_solo(&self, host: &dyn ParamHost, band: usize) {
        let on = self.is_solo(host, band);
        for b in 0..B {
            Self::set(host, &(self.params.solo)(b), if b == band && !on { 1.0 } else { 0.0 });
        }
    }

    pub fn clear_solo(&self, host: &dyn ParamHost) -> bool {
        let any = (0..B).any(|b| self.is_solo(host, b));
        for b in 0..B {
            Self::set(host, &(self.params.solo)(b), 0.0);
        }
        any
    }

    pub fn toggle_mute(&self, host: &dyn ParamHost, band: usize) {
        let id = (self.params.mute)(band);
        Self::set(host, &id, if Self::on(host, &id) { 0.0 } else { 1.0 });
    }

    /// Turns crossover `slot` off: its band (the region above it) joins the one below.
    fn remove_crossover(&self, host: &dyn ParamHost, slot: usize) {
        Self::set(host, &(self.params.cross_on)(slot), 0.0);
        Self::set(host, &(self.params.solo)(slot + 1), 0.0);
    }

    /// Makes a crossover at `f` Hz (pushed clear of its neighbours): the band it splits keeps its
    /// settings on both sides, and the new one is selected. `None` when every slot is in use or
    /// there is no room.
    pub fn split(&mut self, host: &dyn ParamHost, f: f64) -> Option<usize> {
        let x = self.crossovers(host);
        let slot = (0..B - 1).find(|&k| x[k].is_none())?;
        let layout = self.layout(host);
        let gap = 2f64.powf(f64::from(MIN_GAP_OCT));
        let xs = &x[..B - 1];
        let p = (0..=layout.n).find(|&p| layout.region(layout.owners[p], xs).is_some_and(|(lo, hi)| f >= f64::from(lo) && f < f64::from(hi))).unwrap_or(0);
        let parent = layout.owners[p];
        let (lo, hi) = layout.region(parent, xs).unwrap_or((FREQ_MIN, FREQ_MAX));
        let (lo, hi) = (if p == 0 { f64::from(lo) } else { f64::from(lo) * gap }, if p == layout.n { f64::from(hi) } else { f64::from(hi) / gap });
        if lo >= hi {
            return None;
        }
        let f = f.clamp(lo.max(f64::from(FREQ_MIN) * 1.06), hi.min(f64::from(FREQ_MAX) / 1.06));
        (self.params.copy)(host, parent, slot + 1);
        Self::set(host, &(self.params.solo)(slot + 1), 0.0);
        Self::set(host, &(self.params.cross_freq)(slot), f as f32);
        Self::set(host, &(self.params.cross_on)(slot), 1.0);
        self.selected = slot + 1;
        Some(slot + 1)
    }

    /// Removes band `b`: its region joins the band below (the lowest band's joins the one above,
    /// which takes its place).
    pub fn remove_band(&mut self, host: &dyn ParamHost, b: usize) {
        let layout = self.layout(host);
        let Some(p) = layout.position(b) else { return };
        if layout.n == 0 {
            return;
        }
        if p == 0 {
            let above = layout.owners[1];
            (self.params.copy)(host, above, 0);
            self.remove_crossover(host, above - 1);
            self.selected = 0;
        } else {
            self.remove_crossover(host, b - 1);
            self.selected = layout.owners[p - 1];
        }
    }

    /// Merges band `b` with its neighbour below (`up` false) or above: `b`'s region grows over
    /// the neighbour's when merging up, and joins the neighbour below when merging down.
    pub fn merge(&mut self, host: &dyn ParamHost, b: usize, up: bool) {
        let layout = self.layout(host);
        let Some(p) = layout.position(b) else { return };
        if up && p < layout.n {
            let above = layout.owners[p + 1];
            self.remove_crossover(host, above - 1);
            self.selected = b;
        } else if !up && p > 0 {
            self.remove_band(host, b);
        }
    }

    /// `f` kept clear of crossover `slot`'s neighbours.
    fn clamp_cross(&self, host: &dyn ParamHost, slot: usize, f: f64) -> f64 {
        let layout = self.layout(host);
        let x = self.crossovers(host);
        let gap = 2f64.powf(f64::from(MIN_GAP_OCT));
        let Some(p) = layout.crossovers[..layout.n].iter().position(|&k| k == slot) else { return f };
        let lo = if p == 0 { f64::from(FREQ_MIN) * 1.06 } else { f64::from(x[layout.crossovers[p - 1]].unwrap_or(FREQ_MIN)) * gap };
        let hi = if p + 1 == layout.n { f64::from(FREQ_MAX) / 1.06 } else { f64::from(x[layout.crossovers[p + 1]].unwrap_or(FREQ_MAX)) / gap };
        f.clamp(lo.min(hi), hi.max(lo))
    }

    // ---- events -----------------------------------------------------------------------------

    pub fn hot(&self) -> Option<RailHot> {
        self.hot
    }

    /// Whether a drag is under way.
    pub fn busy(&self) -> bool {
        self.drag.is_some()
    }

    /// What is under `p` on the strip (4 px of slack above and below).
    pub fn hit(&self, host: &dyn ParamHost, p: Point) -> Option<RailHot> {
        if !self.area.inflate(0.0, 4.0).contains(p) {
            return None;
        }
        let layout = self.layout(host);
        for k in 0..layout.n {
            let slot = layout.crossovers[k];
            if (self.grip_center(host, slot).x - p.x).abs() <= GRIP_HIT {
                return Some(RailHot::Grip(slot));
            }
        }
        if layout.n < B - 1 {
            for (i, c) in self.ghost_centers().into_iter().enumerate() {
                if (c.x - p.x).abs() <= GRIP_HIT {
                    return Some(RailHot::Ghost(i == 0));
                }
            }
        }
        Some(RailHot::Region(layout.band_at(&self.crossovers(host)[..B - 1], self.f(p.x) as f32)))
    }

    /// Follows the pointer (`None`: it left, or something above the rail has it).
    pub fn hover(&mut self, host: &dyn ParamHost, p: Option<Point>) {
        self.hot = p.and_then(|p| self.hit(host, p));
    }

    /// A press: a grip starts a drag, a ghost grip a pull, a region selects its band.
    pub fn press(&mut self, host: &dyn ParamHost, pos: Point) -> bool {
        match self.hit(host, pos) {
            Some(RailHot::Grip(slot)) => {
                let id = (self.params.cross_freq)(slot);
                host.begin_gesture(&id);
                self.drag = Some(RailDrag::Cross { slot, start: pos, f0: f64::from(Self::plain(host, &id)), merge: None });
                true
            }
            Some(RailHot::Ghost(_)) => {
                self.drag = Some(RailDrag::Create { start: pos, now: pos });
                true
            }
            Some(RailHot::Region(b)) => {
                self.selected = b;
                true
            }
            None => false,
        }
    }

    pub fn drag_to(&mut self, host: &dyn ParamHost, pos: Point, mods: Modifiers) -> bool {
        let fine = if mods.fine() { 0.1 } else { 1.0 };
        match self.drag {
            Some(RailDrag::Cross { slot, start, f0, .. }) => {
                let layout = self.layout(host);
                let x = self.crossovers(host);
                let Some(p) = layout.crossovers[..layout.n].iter().position(|&k| k == slot) else { return true };
                // Follow the pointer (Shift: a tenth as far).
                let f = f0 * (self.f(pos.x) / self.f(start.x)).powf(fine);
                let px = self.x(f);
                let lower = (p > 0).then(|| layout.crossovers[p - 1]);
                let upper = (p + 1 < layout.n).then(|| layout.crossovers[p + 1]);
                let at = |k: usize| self.x(f64::from(x[k].unwrap_or(FREQ_MIN)));
                let (x0, x1) = (self.x0, self.x0 + self.w);
                let merge = match (lower, upper) {
                    (Some(k), _) if px <= at(k) + MERGE_PX => Some(Merge::Down(k)),
                    (_, Some(k)) if px >= at(k) - MERGE_PX => Some(Merge::Up),
                    (None, _) if pos.x < x0 - OFF_END_PX => Some(Merge::OffLow),
                    (_, None) if pos.x > x1 + OFF_END_PX => Some(Merge::OffHigh),
                    _ => None,
                };
                let id = (self.params.cross_freq)(slot);
                host.set_normalized(&id, host.normalize(&id, self.clamp_cross(host, slot, f) as f32));
                self.drag = Some(RailDrag::Cross { slot, start, f0, merge });
                true
            }
            Some(RailDrag::Create { start, .. }) => {
                self.drag = Some(RailDrag::Create { start, now: pos });
                true
            }
            None => false,
        }
    }

    pub fn release(&mut self, host: &dyn ParamHost) -> bool {
        match self.drag.take() {
            Some(RailDrag::Cross { slot, merge, .. }) => {
                host.end_gesture(&(self.params.cross_freq)(slot));
                let layout = self.layout(host);
                match merge {
                    Some(Merge::Down(lower)) => {
                        // The band between goes; the one above this crossover grows down.
                        let f = Self::plain(host, &(self.params.cross_freq)(lower));
                        Self::set(host, &(self.params.cross_freq)(slot), f);
                        self.remove_crossover(host, lower);
                        self.selected = slot + 1;
                    }
                    Some(Merge::Up) | Some(Merge::OffHigh) => {
                        let p = layout.crossovers[..layout.n].iter().position(|&k| k == slot).unwrap_or(0);
                        self.remove_crossover(host, slot);
                        self.selected = layout.owners[p];
                    }
                    Some(Merge::OffLow) => {
                        (self.params.copy)(host, slot + 1, 0);
                        self.remove_crossover(host, slot);
                        self.selected = 0;
                    }
                    None => {}
                }
                true
            }
            Some(RailDrag::Create { start, now }) => {
                if now.distance(start) > PULL_PX && (self.x0 - OFF_END_PX..=self.x0 + self.w + OFF_END_PX).contains(&now.x) {
                    self.split(host, self.f(now.x));
                }
                true
            }
            None => false,
        }
    }

    /// Esc while pulling a new crossover in.
    pub fn cancel(&mut self) -> bool {
        matches!(self.drag.take(), Some(RailDrag::Create { .. }))
    }

    /// A double-click: on a grip it merges there (the selected band survives, else the lower);
    /// on a region it splits there.
    pub fn double_click(&mut self, host: &dyn ParamHost, pos: Point) -> bool {
        match self.hit(host, pos) {
            Some(RailHot::Grip(slot)) => {
                self.drag = None;
                let layout = self.layout(host);
                let p = layout.crossovers[..layout.n].iter().position(|&k| k == slot).unwrap_or(0);
                let lower = layout.owners[p];
                if self.selected == slot + 1 {
                    (self.params.copy)(host, slot + 1, lower);
                }
                self.remove_crossover(host, slot);
                self.selected = lower;
                true
            }
            Some(RailHot::Region(_)) => {
                self.split(host, self.f(pos.x));
                true
            }
            _ => false,
        }
    }

    /// The wheel on a grip moves it.
    pub fn wheel(&mut self, host: &dyn ParamHost, pos: Point, lines: f32, mods: Modifiers) -> bool {
        let Some(RailHot::Grip(slot)) = self.hit(host, pos) else { return false };
        let id = (self.params.cross_freq)(slot);
        let f = self.params.freq.wheel(Self::plain(host, &id), lines, mods.fine());
        Self::set(host, &id, self.clamp_cross(host, slot, f64::from(f)) as f32);
        true
    }

    /// The band a crossover drag would take away on release.
    pub fn merging_band(&self, layout: &Layout<B>) -> Option<usize> {
        let Some(RailDrag::Cross { slot, merge: Some(m), .. }) = self.drag else { return None };
        let p = layout.crossovers[..layout.n].iter().position(|&k| k == slot)?;
        Some(match m {
            Merge::Down(_) => layout.owners[p],
            Merge::Up | Merge::OffHigh => slot + 1,
            Merge::OffLow => layout.owners[0],
        })
    }

    /// The help strip while the rail has the pointer or a drag, if it does.
    pub fn help(&self, host: &dyn ParamHost) -> Option<Help> {
        let layout = self.layout(host);
        match self.drag {
            Some(RailDrag::Create { start, now }) if now.distance(start) > PULL_PX => {
                if layout.n >= B - 1 {
                    return Some(Help::default().text(format!("{B} bands is the limit.")));
                }
                let f = self.f(now.x);
                return Some(Help::default().text("New crossover ").slot(fmt_freq(f), "20.0 kHz").text(" — release to split the band there; both sides keep its settings. Esc cancels."));
            }
            Some(RailDrag::Cross { slot, merge, .. }) => {
                let f = f64::from(Self::plain(host, &(self.params.cross_freq)(slot)));
                if let Some(b) = merge.and_then(|_| self.merging_band(&layout)) {
                    let n = layout.position(b).map_or(1, |p| p + 1);
                    return Some(Help::default().text("Merging ").slot(fmt_freq(f), "20.0 kHz").text(format!(" — release to remove band {n}; its neighbour grows and keeps its settings")));
                }
                return Some(Help::default().text("Crossover ").slot(fmt_freq(f), "20.0 kHz").text(" — drag onto a neighbour or off the end to merge; Shift for fine steps"));
            }
            _ => {}
        }
        match self.hot {
            Some(RailHot::Grip(slot)) => {
                let f = f64::from(Self::plain(host, &(self.params.cross_freq)(slot)));
                Some(Help::default().text("Crossover ").slot(fmt_freq(f), "20.0 kHz").text(" — drag sideways; onto a neighbour or off the end merges; double-click merges"))
            }
            Some(RailHot::Ghost(_)) => Some(Help::default().text("Pull a crossover in from here to split the sound into bands")),
            _ => None,
        }
    }

    // ---- drawing ----------------------------------------------------------------------------

    /// The strip, its grid and the band regions: the selected one lit, the others at the quiet
    /// tint plus `breath` (each band's share of the music, 0…1), dimmed under another's solo;
    /// muted bands hatched.
    pub fn draw_under(&self, cx: &mut DrawCx<'_>, layout: &Layout<B>, breath: &[f32; B]) {
        let theme = cx.theme;
        let r = self.area;
        cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.well, None, &r);
        for y in [r.y0 + 0.5, r.y1 - 0.5] {
            cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, theme.well_border, None, &Line::new((r.x0, y), (r.x1, y)));
        }
        for f in [100.0, 1000.0, 10_000.0] {
            cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, theme.grid_minor, None, &Line::new((self.x(f), r.y0), (self.x(f), r.y1)));
        }
        if layout.n == 0 {
            return;
        }
        let soloed = self.soloed(cx.host, layout);
        let merging = self.merging_band(layout);
        let current = self.current(layout);
        for (p, &b) in layout.bands().iter().enumerate() {
            let region = self.region_rect(cx.host, layout, p);
            let audible = self.audible(cx.host, layout, b);
            let mut op = if b == current { self.tint.0 } else { self.tint.1 } + if audible { breath[b].clamp(0.0, 1.0) * 0.12 } else { 0.0 };
            if soloed.is_some_and(|s| s != b) {
                op *= 0.35;
            }
            if merging == Some(b) {
                op = 0.08;
            }
            if self.is_muted(cx.host, b) {
                hatch(cx, region, None, 0.3);
            } else {
                cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.accent.multiply_alpha(op), None, &region);
            }
            if merging == Some(b) {
                cx.scene.stroke(&Stroke::new(1.5).with_dashes(0.0, [4.0, 3.0]), Affine::IDENTITY, theme.accent, None, &region.inset(-1.0));
            }
        }
    }

    /// Frequency labels, crossover grips with their frequencies, the pull in progress, the
    /// ghost grips while there is room, and the band numbers with S or M tags.
    pub fn draw_over(&self, cx: &mut DrawCx<'_>, layout: &Layout<B>) {
        let theme = cx.theme;
        let r = self.area;
        let (ry, rh) = (r.y0, r.height());
        let multi = layout.n > 0;
        let x = self.crossovers(cx.host);
        let freq_style = TextStyle { tabular: true, ..TextStyle::BODY.sized(10.0).weighted(700.0) };
        if multi && self.labels {
            let label = TextStyle { tabular: true, ..TextStyle::BODY.sized(10.0).weighted(600.0) };
            for (f, s) in [(100.0, "100"), (1000.0, "1k"), (10_000.0, "10k")] {
                cx.text.draw(cx.scene, s, self.x(f) + 4.0, ry + rh - 15.0, label, theme.text_lo, Align::Left);
            }
        }
        let dragged = match self.drag {
            Some(RailDrag::Cross { slot, merge, .. }) => Some((slot, merge)),
            _ => None,
        };
        for p in 0..layout.n {
            let slot = layout.crossovers[p];
            let mut c = self.grip_center(cx.host, slot);
            let merge = dragged.filter(|&(k, _)| k == slot).and_then(|(_, m)| m);
            // Merging: the dragged crossover's ghost sits where it lands (on its neighbour, or
            // off the end), and only the neighbour keeps a label.
            match merge {
                Some(Merge::Down(_)) => c.x = self.x(f64::from(x[layout.crossovers[p - 1]].unwrap_or(FREQ_MIN))),
                Some(Merge::Up) => c.x = self.x(f64::from(x[layout.crossovers[p + 1]].unwrap_or(FREQ_MAX))),
                Some(Merge::OffLow) => c.x = self.x0,
                Some(Merge::OffHigh) => c.x = self.x0 + self.w,
                None => {}
            }
            let ghost = merge.is_some();
            let hot = self.hot == Some(RailHot::Grip(slot)) || dragged.is_some_and(|(k, _)| k == slot);
            let line = Line::new((c.x, ry), (c.x, ry + rh));
            if ghost {
                cx.scene.stroke(&Stroke::new(1.5).with_dashes(0.0, [4.0, 3.0]), Affine::IDENTITY, theme.accent, None, &line);
            } else {
                cx.scene.stroke(&Stroke::new(1.5), Affine::IDENTITY, theme.accent.multiply_alpha(0.75), None, &line);
            }
            draw_grip(cx, c, ghost, hot, 1.0);
            if !ghost {
                let f = f64::from(x[slot].unwrap_or(FREQ_MIN));
                cx.text.draw(cx.scene, &fmt_freq(f), c.x - 10.0, ry + 5.0, freq_style, if hot { theme.accent_text } else { theme.text_mid }, Align::Right);
            }
        }
        if let Some(RailDrag::Create { start, now }) = self.drag {
            if now.distance(start) > PULL_PX {
                let f = self.f(now.x);
                let px = self.x(f);
                cx.scene.stroke(&Stroke::new(1.5).with_dashes(0.0, [4.0, 3.0]), Affine::IDENTITY, theme.accent, None, &Line::new((px, ry), (px, ry + rh)));
                draw_grip(cx, Point::new(px, ry + rh / 2.0), true, true, 1.0);
                cx.text.draw(cx.scene, &fmt_freq(f), px - 10.0, ry + 5.0, freq_style, theme.accent_text, Align::Right);
            }
        }
        if layout.n < B - 1 && self.drag.is_none() {
            for (i, c) in self.ghost_centers().into_iter().enumerate() {
                let hot = self.hot == Some(RailHot::Ghost(i == 0));
                draw_grip(cx, c, true, hot, if hot { 1.0 } else { 0.55 });
            }
        }
        if !multi {
            return;
        }
        let soloed = self.soloed(cx.host, layout);
        let merging = self.merging_band(layout);
        let current = self.current(layout);
        for (p, &b) in layout.bands().iter().enumerate() {
            if merging == Some(b) {
                continue;
            }
            let region = self.region_rect(cx.host, layout, p);
            let solo = self.is_solo(cx.host, b);
            let muted = self.is_muted(cx.host, b);
            let alpha = if soloed.is_some_and(|s| s != b) { 0.45 } else { 1.0 };
            let c = Point::new(region.x0 + if p == 0 { 34.0 } else { 18.0 } + 9.0, ry + 15.0);
            draw_badge(cx, c, 9.0, p + 1, b == current, solo, alpha);
            if solo || muted {
                let tag = Rect::new(c.x + 13.0, c.y - 9.0, c.x + 31.0, c.y + 9.0);
                let color = if solo { theme.needle } else { theme.text_hi };
                cx.scene.fill(Fill::NonZero, Affine::IDENTITY, color.multiply_alpha(alpha), None, &RoundedRect::from_rect(tag, 9.0));
                cx.text.draw(cx.scene, if solo { "S" } else { "M" }, tag.center().x, tag.y0 + 3.0, TextStyle::LABEL.sized(10.0).weighted(800.0), theme.display, Align::Center);
            }
        }
    }

    /// A funnel from each band's region down to its card (`cards` in band order from the
    /// bottom), between y `y0` and `y1`.
    pub fn draw_funnels(&self, cx: &mut DrawCx<'_>, layout: &Layout<B>, cards: &[Rect], y0: f64, y1: f64) {
        if layout.n == 0 || y1 - y0 < 2.0 {
            return;
        }
        let theme = cx.theme;
        let current = self.current(layout);
        for (p, card) in cards.iter().enumerate() {
            let r = self.region_rect(cx.host, layout, p);
            let (rx0, rx1) = (r.x0 + 2.0, r.x1 - 2.0);
            let (cx0, cx1) = (card.x0 + 2.0, card.x1 - 2.0);
            let ym = (y0 + y1) / 2.0;
            let mut f = BezPath::new();
            f.move_to((rx0, y0));
            f.curve_to((rx0, ym), (cx0, ym), (cx0, y1));
            f.line_to((cx1, y1));
            f.curve_to((cx1, ym), (rx1, ym), (rx1, y0));
            f.close_path();
            let on = layout.owners[p] == current;
            cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.accent.multiply_alpha(if on { 0.16 } else { 0.05 }), None, &f);
            if on {
                cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, theme.accent.multiply_alpha(0.5), None, &f);
            }
        }
    }
}

/// A crossover grip: lit when hot, dashed when a ghost.
pub fn draw_grip(cx: &mut DrawCx<'_>, c: Point, ghost: bool, hot: bool, alpha: f32) {
    let theme = cx.theme;
    let r = RoundedRect::from_rect(Rect::from_center_size(c, GRIP), 7.0);
    let fill = if ghost { theme.accent.multiply_alpha(0.12) } else if hot { theme.accent } else { theme.display };
    cx.scene.fill(Fill::NonZero, Affine::IDENTITY, fill.multiply_alpha(alpha), None, &r);
    if ghost {
        cx.scene.stroke(&Stroke::new(1.5).with_dashes(0.0, [3.0, 2.0]), Affine::IDENTITY, theme.accent.multiply_alpha(alpha), None, &r);
    } else {
        glow_stroke(cx, &r, 1.5, theme.accent.multiply_alpha(alpha), theme.accent, if hot { 5.0 } else { 3.0 });
    }
    let ink = if hot && !ghost { theme.display } else { theme.accent };
    for dy in [-4.5, 0.0, 4.5] {
        cx.scene.stroke(&Stroke::new(1.5).with_caps(Cap::Round), Affine::IDENTITY, ink.multiply_alpha(alpha), None, &Line::new((c.x - 2.0, c.y + dy), (c.x + 2.0, c.y + dy)));
    }
}

/// A band's number in its circle: lit when selected, ringed white when soloed.
pub fn draw_badge(cx: &mut DrawCx<'_>, c: Point, r: f64, n: usize, on: bool, solo: bool, alpha: f32) {
    let theme = cx.theme;
    let fill = if on { theme.accent } else { theme.accent.multiply_alpha(0.25) };
    cx.scene.fill(Fill::NonZero, Affine::IDENTITY, fill.multiply_alpha(alpha), None, &Circle::new(c, r));
    if on {
        // The rim glows, not the disc: a glowing fill blooms over the number.
        glow_stroke(cx, &Circle::new(c, r - 0.5), 1.0, fill.multiply_alpha(alpha), fill, 4.0);
    }
    if solo {
        glow_stroke(cx, &Circle::new(c, r - 1.0), 2.0, theme.needle.multiply_alpha(alpha), theme.needle, 4.0);
    }
    let ink = if on { on_accent(theme) } else { theme.accent_text };
    cx.text.draw(cx.scene, &n.to_string(), c.x, c.y - r * 0.62, TextStyle::LABEL.sized((r * 1.1) as f32).weighted(800.0), ink.multiply_alpha(alpha), Align::Center);
}

/// Ink on an accent fill: the display colour or the needle's, whichever stands out more (dark
/// ink on Shape's leaf, white on Sat's crimson in dark themes).
fn on_accent(theme: &crate::Theme) -> Color {
    let lum = |c: Color| {
        let lin = |v: f32| if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) };
        let [r, g, b, _] = c.components;
        0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b)
    };
    let contrast = |a: Color| {
        let (x, y) = (lum(a), lum(theme.accent));
        (x.max(y) + 0.05) / (x.min(y) + 0.05)
    };
    // Dark ink keeps a little preference: it is what the design draws on a light accent.
    if contrast(theme.needle) > contrast(theme.display) * 1.15 { theme.needle } else { theme.display }
}

/// A muted band's 45° hatch over `r` (clipped to `round` corners when given).
pub fn hatch(cx: &mut DrawCx<'_>, r: Rect, round: Option<f64>, alpha: f32) {
    let mut path = BezPath::new();
    let h = r.height();
    let mut k = r.x0 - h;
    while k < r.x1 {
        path.move_to((k, r.y1));
        path.line_to((k + h, r.y0));
        k += 6.0;
    }
    match round {
        Some(radius) => cx.scene.push_clip_layer(Fill::NonZero, Affine::IDENTITY, &RoundedRect::from_rect(r, radius)),
        None => cx.scene.push_clip_layer(Fill::NonZero, Affine::IDENTITY, &r),
    }
    cx.scene.stroke(&Stroke::new(3.0), Affine::IDENTITY, cx.theme.text_lo.multiply_alpha(alpha), None, &path);
    cx.scene.pop_layer();
}

/// The band menu (right-click a band).
const MENU_ITEMS: &[&str] = &["Solo", "Mute", "Copy settings", "Paste settings", "Reset band", "Merge with the band below", "Merge with the band above", "Remove band"];
const MENU_GLYPHS: [&str; 8] = [glyph::SOLO, glyph::MUTE, glyph::COPY, glyph::PASTE, glyph::RESET, glyph::MERGE, glyph::MERGE, glyph::BIN];

/// What a pick in the band menu asks the plugin for (the rail does the rest itself).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuAsk {
    Copy(usize),
    Paste(usize),
    Reset(usize),
}

pub struct BandMenu {
    band: usize,
    list: Dropdown,
}

impl BandMenu {
    /// Opens band `band`'s menu at `at` (inside the window band `area`); `paste` when there is
    /// something to paste. Selects the band.
    pub fn open<const B: usize>(rail: &mut Rail<B>, host: &dyn ParamHost, band: usize, at: Point, paste: bool, area: (f64, f64)) -> Option<Self> {
        let layout = rail.layout(host);
        let p = layout.position(band)?;
        let mut shown = vec![0, 1, 2];
        if paste {
            shown.push(3);
        }
        shown.push(4);
        if p > 0 {
            shown.push(5);
        }
        if p < layout.n {
            shown.push(6);
        }
        if layout.n > 0 {
            shown.push(7);
        }
        let anchor = Rect::new(at.x, at.y, at.x + 236.0, at.y + 1.0);
        rail.selected = band;
        Some(Self { band, list: Dropdown::new(anchor, MENU_ITEMS, usize::MAX, area).with_shown(shown).with_min_width(236.0) })
    }

    /// Routes an event to the open menu: `Err(())` while it stays open, `Ok(ask)` once it
    /// closes (with what the plugin must do, if anything).
    #[allow(clippy::result_unit_err)]
    pub fn event<const B: usize>(&mut self, event: &UiEvent, rail: &mut Rail<B>, host: &dyn ParamHost) -> Result<Option<MenuAsk>, ()> {
        let band = self.band;
        match self.list.event(event) {
            DropdownEvent::Open => Err(()),
            DropdownEvent::Closed => Ok(None),
            DropdownEvent::Picked(i) => Ok(match i {
                0 => {
                    rail.toggle_solo(host, band);
                    None
                }
                1 => {
                    rail.toggle_mute(host, band);
                    None
                }
                2 => Some(MenuAsk::Copy(band)),
                3 => Some(MenuAsk::Paste(band)),
                4 => Some(MenuAsk::Reset(band)),
                5 | 6 => {
                    rail.merge(host, band, i == 6);
                    None
                }
                _ => {
                    rail.remove_band(host, band);
                    None
                }
            }),
        }
    }

    /// The list's rows (tests and tools).
    pub fn item_rects(&self) -> Vec<Rect> {
        self.list.item_rects()
    }

    /// Draws the list; "Remove band" with `remove`'s ink and glyph (Shape: danger and a bin; Sat
    /// draws no danger role, so a cross in the text colour).
    pub fn draw(&self, cx: &mut DrawCx<'_>, remove: (Color, &str)) {
        let theme = cx.theme;
        self.list.draw(cx, theme.accent, |scene, i, at, ink| {
            let (ink, d) = if i == 7 { remove } else { (ink, MENU_GLYPHS[i]) };
            draw_glyph(scene, d, at + (11.0, 6.0), 20.0, ink, 1.6);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MemoryParamHost;
    use mite_core::{ParamDisplay, ParamScale};

    fn spec(id: &'static str, min: f32, max: f32, default: f32, scale: ParamScale) -> ParamSpec {
        ParamSpec { id, name: id, unit: "", min, max, default, scale, smoothing_ms: 0.0, decimals: 1, wheel_step: 1.0 / 6.0, wheel_step_fine: 1.0 / 48.0, display: ParamDisplay::Number }
    }

    const IDS: [&str; 4] = ["x1_on", "x2_on", "x3_on", "x4_on"];
    const FREQS: [&str; 4] = ["x1_freq", "x2_freq", "x3_freq", "x4_freq"];
    const SOLO: [&str; 5] = ["b1_solo", "b2_solo", "b3_solo", "b4_solo", "b5_solo"];
    const MUTE: [&str; 5] = ["b1_mute", "b2_mute", "b3_mute", "b4_mute", "b5_mute"];
    const GAIN: [&str; 5] = ["b1_gain", "b2_gain", "b3_gain", "b4_gain", "b5_gain"];

    fn rail() -> (Rail<5>, MemoryParamHost) {
        let mut host = MemoryParamHost::new();
        for k in 0..4 {
            host = host.with(spec(IDS[k], 0.0, 1.0, 0.0, ParamScale::Linear)).with(spec(FREQS[k], 20.0, 20_000.0, 1000.0, ParamScale::Log));
        }
        for b in 0..5 {
            host = host.with(spec(SOLO[b], 0.0, 1.0, 0.0, ParamScale::Linear)).with(spec(MUTE[b], 0.0, 1.0, 0.0, ParamScale::Linear)).with(spec(GAIN[b], -12.0, 12.0, 0.0, ParamScale::Linear));
        }
        let params = RailParams {
            cross_on: |k| IDS[k].to_owned(),
            cross_freq: |k| FREQS[k].to_owned(),
            freq: spec("freq", 20.0, 20_000.0, 1000.0, ParamScale::Log),
            solo: |b| SOLO[b].to_owned(),
            mute: |b| MUTE[b].to_owned(),
            copy: |host, from, to| {
                let v = host.normalized(GAIN[from]);
                write_if_changed(host, GAIN[to], v);
            },
        };
        (Rail::new(params, Rect::new(0.0, 100.0, 1180.0, 180.0), 14.0, 1152.0), host)
    }

    #[test]
    fn the_axis_maps_frequency_both_ways() {
        let (r, _) = rail();
        for f in [20.0, 100.0, 1234.0, 20_000.0] {
            assert!((r.f(r.x(f)) / f - 1.0).abs() < 1e-9);
        }
        assert!((r.x(20.0) - 14.0).abs() < 1e-9 && (r.x(20_000.0) - 1166.0).abs() < 1e-9);
    }

    #[test]
    fn a_split_copies_the_band_and_a_merge_off_the_low_end_keeps_the_upper_one() {
        let (mut r, host) = rail();
        host.set_plain(GAIN[0], 6.0);
        assert_eq!(r.split(&host, 500.0), Some(1));
        assert_eq!(host.plain(GAIN[1], host.normalized(GAIN[1])), 6.0);
        host.set_plain(GAIN[1], -3.0);
        // Drag the 500 Hz grip off the low end: band 1 (the lowest) goes, band 2's sound takes over.
        let c = Point::new(r.x(500.0), 140.0);
        assert!(r.press(&host, c));
        r.drag_to(&host, Point::new(0.0, 140.0), Modifiers::default());
        r.release(&host);
        assert_eq!(r.layout(&host).n, 0);
        assert_eq!(host.plain(GAIN[0], host.normalized(GAIN[0])), -3.0);
    }

    #[test]
    fn splits_stop_at_the_slot_count() {
        let (mut r, host) = rail();
        for f in [100.0, 400.0, 1600.0, 6400.0] {
            assert!(r.split(&host, f).is_some());
        }
        assert_eq!(r.split(&host, 15_000.0), None);
        assert_eq!(r.layout(&host).n, 4);
    }
}
