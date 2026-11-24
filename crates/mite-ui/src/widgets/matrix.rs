//! The modulation matrix panel (component sheet: modulation matrix; shared by Delay, Sat and
//! Mod). It opens over the lower part of a plugin's editor: on the left the sources (two LFOs,
//! each its shape over the cycle playing with a dot where it is, its shapes, rate or division,
//! sync and phase; the envelope with its level and its times), on the right the eight slots
//! (source, target, amount, each with what it does now). A target is picked from a sheet of the
//! plugin's targets over the panel. Dials anywhere in the plugin show what moves them
//! ([`knob_modulation`] with [`Knob::draw_modulation`](super::Knob::draw_modulation)).

use mite_core::modmatrix::*;
use mite_dsp::modmatrix::{lfo_at, LfoShape, Source};
use vello::kurbo::{Affine, BezPath, Circle, Line, Point, Rect, RoundedRect, Stroke};
use vello::peniko::Fill;

use crate::host::{write_if_changed, ParamHost};
use crate::paint::{glass_panel, glow_stroke, panel};
use crate::text::{Align, TextStyle};
use crate::view::{DrawCx, MouseButton, UiEvent};
use crate::widgets::history::smooth_path;
use crate::widgets::{Dropdown, DropdownEvent, Help, Knob, KnobKind};

const PILL_H: f64 = 26.0;
const SOURCE_NAMES: [&str; 4] = ["OFF", "LFO 1", "LFO 2", "ENV"];

/// What a plugin's matrix moves.
#[derive(Debug, Clone, Copy)]
pub struct MatrixConfig {
    pub targets: Targets,
    /// Whether `env_src` exists (a sidechain to follow).
    pub sidechain: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MatrixHot {
    Close,
    Shape(usize, LfoShape),
    Sync(usize),
    Division(usize),
    EnvSource,
    SlotSource(usize),
    SlotTarget(usize),
    SlotAmount(usize),
    SlotClear(usize),
    /// A target on the open picker (index into the targets).
    Pick(usize),
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct AmountDrag {
    slot: usize,
    start_x: f64,
    start: f32,
}

pub struct MatrixPanel {
    pub config: MatrixConfig,
    pub rect: Rect,
    knobs: Vec<Knob>,
    hot: Option<MatrixHot>,
    pointer: Option<Point>,
    drag: Option<AmountDrag>,
    /// The division menu (for an LFO).
    menu: Option<(usize, Dropdown)>,
    /// The target sheet (for a slot).
    picking: Option<usize>,
}

fn slot_id(slot: usize, what: &str) -> mite_core::ParamId {
    mod_id(slot, what)
}

fn index(host: &dyn ParamHost, id: &str) -> usize {
    host.plain(id, host.normalized(id)).round().max(0.0) as usize
}

fn plain(host: &dyn ParamHost, id: &str) -> f32 {
    host.plain(id, host.normalized(id))
}

fn set(host: &dyn ParamHost, id: &str, v: f32) {
    write_if_changed(host, id, host.normalize(id, v));
}

impl MatrixPanel {
    pub fn new(config: MatrixConfig, rect: Rect) -> Self {
        let mut knobs = Vec::new();
        for l in 0..LFOS {
            let card = Self::card_rect(rect, l);
            knobs.push(Knob::new(LFO_RATE, "RATE", KnobKind::Unipolar, Point::new(card.x1 - 100.0, card.y0 + 44.0), 40.0).with_id(lfo_id(l, LFO_RATE.id)));
            knobs.push(Knob::new(LFO_PHASE, "PHASE", KnobKind::Unipolar, Point::new(card.x1 - 40.0, card.y0 + 44.0), 40.0).with_id(lfo_id(l, LFO_PHASE.id)));
        }
        let env = Self::card_rect(rect, 2);
        for (k, (spec, label)) in [(ENV_ATT, "ATTACK"), (ENV_REL, "RELEASE"), (ENV_GAIN, "GAIN")].into_iter().enumerate() {
            let kind = if spec.id == ENV_GAIN.id { KnobKind::Bipolar } else { KnobKind::Unipolar };
            knobs.push(Knob::new(spec, label, kind, Point::new(env.x0 + 236.0 + k as f64 * 64.0, env.y0 + 44.0), 40.0));
        }
        Self { config, rect, knobs, hot: None, pointer: None, drag: None, menu: None, picking: None }
    }

    /// A source card (0, 1: the LFOs; 2: the envelope).
    fn card_rect(rect: Rect, k: usize) -> Rect {
        let top = rect.y0 + 34.0;
        let h = (rect.height() - 34.0 - 12.0 - 2.0 * 8.0) / 3.0;
        let y = top + k as f64 * (h + 8.0);
        Rect::new(rect.x0 + 12.0, y, rect.x0 + 560.0, y + h)
    }

    fn wave_rect(&self, l: usize) -> Rect {
        let c = Self::card_rect(self.rect, l);
        Rect::new(c.x0 + 12.0, c.y0 + 24.0, c.x0 + 160.0, c.y1 - 10.0)
    }

    pub fn shape_rect(&self, l: usize, shape: LfoShape) -> Rect {
        let c = Self::card_rect(self.rect, l);
        let i = shape.index() as f64;
        let x = c.x0 + 174.0 + i * 33.0;
        Rect::new(x, c.y0 + 22.0, x + 29.0, c.y0 + 46.0)
    }

    pub fn sync_rect(&self, l: usize) -> Rect {
        let c = Self::card_rect(self.rect, l);
        Rect::new(c.x0 + 174.0, c.y0 + 54.0, c.x0 + 240.0, c.y0 + 54.0 + PILL_H)
    }

    pub fn division_rect(&self, l: usize) -> Rect {
        let c = Self::card_rect(self.rect, l);
        Rect::new(c.x0 + 246.0, c.y0 + 54.0, c.x0 + 330.0, c.y0 + 54.0 + PILL_H)
    }

    fn env_source_rect(&self) -> Rect {
        let c = Self::card_rect(self.rect, 2);
        Rect::new(c.x1 - 116.0, c.y0 + 30.0, c.x1 - 12.0, c.y0 + 30.0 + PILL_H)
    }

    fn close_rect(&self) -> Rect {
        Rect::new(self.rect.x1 - 36.0, self.rect.y0 + 6.0, self.rect.x1 - 10.0, self.rect.y0 + 30.0)
    }

    fn slots_area(&self) -> Rect {
        Rect::new(self.rect.x0 + 576.0, self.rect.y0 + 34.0, self.rect.x1 - 12.0, self.rect.y1 - 12.0)
    }

    fn row_rect(&self, slot: usize) -> Rect {
        let a = self.slots_area();
        let h = (a.height() - 22.0) / MOD_SLOTS as f64;
        let y = a.y0 + 22.0 + slot as f64 * h;
        Rect::new(a.x0, y, a.x1, y + h)
    }

    pub fn slot_source_rect(&self, slot: usize) -> Rect {
        let r = self.row_rect(slot);
        let y = r.center().y - PILL_H / 2.0;
        Rect::new(r.x0 + 28.0, y, r.x0 + 104.0, y + PILL_H)
    }

    pub fn slot_target_rect(&self, slot: usize) -> Rect {
        let r = self.row_rect(slot);
        let y = r.center().y - PILL_H / 2.0;
        Rect::new(r.x0 + 112.0, y, r.x0 + 300.0, y + PILL_H)
    }

    pub fn slot_amount_rect(&self, slot: usize) -> Rect {
        let r = self.row_rect(slot);
        let y = r.center().y - PILL_H / 2.0;
        Rect::new(r.x0 + 310.0, y, r.x1 - 40.0, y + PILL_H)
    }

    fn slot_clear_rect(&self, slot: usize) -> Rect {
        let r = self.row_rect(slot);
        let y = r.center().y - 11.0;
        Rect::new(r.x1 - 30.0, y, r.x1 - 8.0, y + 22.0)
    }

    /// The target sheet's cells (one per target but "None", in columns over the panel).
    pub fn pick_rects(&self) -> Vec<Rect> {
        let n = self.config.targets.names.len();
        let area = self.rect.inset(-16.0);
        let rows = ((area.height() - 30.0) / 28.0).floor().max(1.0) as usize;
        let cols = n.div_ceil(rows).max(1);
        let w = ((area.width() - 8.0 * (cols - 1) as f64) / cols as f64).min(220.0);
        (0..n)
            .map(|i| {
                let (c, r) = (i / rows, i % rows);
                let x = area.x0 + c as f64 * (w + 8.0);
                let y = area.y0 + 30.0 + r as f64 * 28.0;
                Rect::new(x, y, x + w, y + 24.0)
            })
            .collect()
    }

    pub fn busy(&self) -> bool {
        self.drag.is_some() || self.menu.is_some() || self.picking.is_some() || self.knobs.iter().any(Knob::is_dragging)
    }

    /// Whether the panel asked to close (the × was clicked): the owner hides it.
    pub fn hot_at(&self, host: &dyn ParamHost, p: Point) -> Option<MatrixHot> {
        if self.picking.is_some() {
            return self.pick_rects().iter().position(|r| r.contains(p)).map(MatrixHot::Pick);
        }
        if self.close_rect().contains(p) {
            return Some(MatrixHot::Close);
        }
        for l in 0..LFOS {
            for shape in LfoShape::ALL {
                if self.shape_rect(l, shape).contains(p) {
                    return Some(MatrixHot::Shape(l, shape));
                }
            }
            if self.sync_rect(l).contains(p) {
                return Some(MatrixHot::Sync(l));
            }
            if self.division_rect(l).contains(p) {
                return Some(MatrixHot::Division(l));
            }
        }
        if self.config.sidechain && self.env_source_rect().contains(p) {
            return Some(MatrixHot::EnvSource);
        }
        for s in 0..MOD_SLOTS {
            if self.slot_source_rect(s).contains(p) {
                return Some(MatrixHot::SlotSource(s));
            }
            if self.slot_target_rect(s).contains(p) {
                return Some(MatrixHot::SlotTarget(s));
            }
            if self.slot_amount_rect(s).contains(p) {
                return Some(MatrixHot::SlotAmount(s));
            }
            if self.slot_clear_rect(s).contains(p) && index(host, &slot_id(s, M_SRC.id)) != 0 {
                return Some(MatrixHot::SlotClear(s));
            }
        }
        None
    }

    /// Handles an event. `Some(true)`: handled (redraw); `Some(false)`: the panel wants to close;
    /// `None`: not the panel's (outside it).
    pub fn event(&mut self, event: &UiEvent, host: &dyn ParamHost) -> Option<bool> {
        if let Some((l, menu)) = &mut self.menu {
            let l = *l;
            match menu.event(event) {
                DropdownEvent::Open => {}
                DropdownEvent::Closed => self.menu = None,
                DropdownEvent::Picked(i) => {
                    self.menu = None;
                    set(host, &lfo_id(l, LFO_DIV.id), i as f32);
                    set(host, &lfo_id(l, LFO_SYNC.id), 1.0);
                }
            }
            return Some(true);
        }
        if self.picking.is_none() {
            for k in &mut self.knobs {
                if k.event(event, host) {
                    return Some(true);
                }
            }
        }
        match *event {
            UiEvent::PointerMove { pos, mods } => {
                self.pointer = Some(pos);
                if let Some(d) = self.drag {
                    let r = self.slot_amount_rect(d.slot);
                    let fine = if mods.fine() { 0.2 } else { 1.0 };
                    let v = (d.start + ((pos.x - d.start_x) / (0.5 * r.width()) * 100.0 * fine) as f32).clamp(-100.0, 100.0);
                    let id = slot_id(d.slot, M_AMT.id);
                    host.set_normalized(&id, host.normalize(&id, M_AMT.snap(v)));
                    return Some(true);
                }
                self.hot = self.hot_at(host, pos);
                self.rect.contains(pos).then_some(true)
            }
            UiEvent::PointerLeft => {
                self.pointer = None;
                self.hot = None;
                Some(true)
            }
            UiEvent::PointerDown { pos, button: MouseButton::Left, .. } => {
                if let Some(slot) = self.picking {
                    if let Some(t) = self.pick_rects().iter().position(|r| r.contains(pos)) {
                        set(host, &slot_id(slot, "dst"), t as f32);
                        // A fresh route: listen to LFO 1 and move a quarter of the way.
                        if index(host, &slot_id(slot, M_SRC.id)) == 0 && t != 0 {
                            set(host, &slot_id(slot, M_SRC.id), 1.0);
                        }
                        if plain(host, &slot_id(slot, M_AMT.id)) == 0.0 && t != 0 {
                            set(host, &slot_id(slot, M_AMT.id), 25.0);
                        }
                    }
                    self.picking = None;
                    return Some(true);
                }
                let hot = self.hot_at(host, pos);
                match hot {
                    Some(MatrixHot::Close) => return Some(false),
                    Some(MatrixHot::Shape(l, shape)) => set(host, &lfo_id(l, LFO_SHAPE.id), shape.index() as f32),
                    Some(MatrixHot::Sync(l)) => {
                        let id = lfo_id(l, LFO_SYNC.id);
                        set(host, &id, if plain(host, &id) >= 0.5 { 0.0 } else { 1.0 });
                    }
                    Some(MatrixHot::Division(l)) => {
                        let cur = index(host, &lfo_id(l, LFO_DIV.id));
                        self.menu = Some((l, Dropdown::new(self.division_rect(l), LFO_DIV.choices, cur, (self.rect.y0 - 300.0, self.rect.y1)).with_min_width(110.0)));
                    }
                    Some(MatrixHot::EnvSource) => {
                        let cur = index(host, ENV_SRC.id);
                        set(host, ENV_SRC.id, if cur == 0 { 1.0 } else { 0.0 });
                    }
                    Some(MatrixHot::SlotSource(s)) => {
                        let id = slot_id(s, M_SRC.id);
                        set(host, &id, ((index(host, &id) + 1) % Source::ALL.len()) as f32);
                    }
                    Some(MatrixHot::SlotTarget(s)) => self.picking = Some(s),
                    Some(MatrixHot::SlotAmount(s)) => {
                        let id = slot_id(s, M_AMT.id);
                        host.begin_gesture(&id);
                        self.drag = Some(AmountDrag { slot: s, start_x: pos.x, start: plain(host, &id) });
                    }
                    Some(MatrixHot::SlotClear(s)) => {
                        set(host, &slot_id(s, M_SRC.id), 0.0);
                        set(host, &slot_id(s, "dst"), 0.0);
                        set(host, &slot_id(s, M_AMT.id), 0.0);
                    }
                    Some(MatrixHot::Pick(_)) | None => return self.rect.contains(pos).then_some(true),
                }
                Some(true)
            }
            UiEvent::PointerUp { button: MouseButton::Left, .. } => {
                if let Some(d) = self.drag.take() {
                    host.end_gesture(&slot_id(d.slot, M_AMT.id));
                    return Some(true);
                }
                None
            }
            UiEvent::DoubleClick { pos, .. } => match self.hot_at(host, pos) {
                Some(MatrixHot::SlotAmount(s)) => {
                    self.drag = None;
                    set(host, &slot_id(s, M_AMT.id), 0.0);
                    Some(true)
                }
                _ => self.rect.contains(pos).then_some(true),
            },
            UiEvent::Wheel { pos, lines, mods } => match self.hot_at(host, pos) {
                Some(MatrixHot::SlotAmount(s)) => {
                    let id = slot_id(s, M_AMT.id);
                    set(host, &id, M_AMT.wheel(plain(host, &id), lines, mods.fine()));
                    Some(true)
                }
                Some(MatrixHot::Division(l)) => {
                    let id = lfo_id(l, LFO_DIV.id);
                    let next = (index(host, &id) as i32 - lines.signum() as i32).clamp(0, LFO_DIV.choices.len() as i32 - 1);
                    set(host, &id, next as f32);
                    Some(true)
                }
                _ => self.rect.contains(pos).then_some(true),
            },
            UiEvent::KeyDown { key: crate::Key::Escape, .. } if self.picking.is_some() => {
                self.picking = None;
                Some(true)
            }
            _ => None,
        }
    }

    pub fn help(&self, host: &dyn ParamHost) -> Option<Help> {
        if self.picking.is_some() {
            return Some(Help::default().text("Pick what this slot moves (None: nothing); Esc or a click elsewhere keeps it."));
        }
        if let Some((_, menu)) = &self.menu {
            let i = self.pointer.and_then(|p| menu.item_rects().iter().position(|r| r.contains(p)));
            return Some(Help::default().text(match i {
                Some(i) => format!("{} — one LFO cycle this long, in step with the song", LFO_DIV.choices[i]),
                None => "Pick a division; click elsewhere to close.".to_owned(),
            }));
        }
        if let Some(k) = self.knobs.iter().find(|k| self.pointer.is_some_and(|p| k.bounds().contains(p))) {
            let v = plain(host, &k.id);
            let words = match k.spec.id {
                "rate" => " — how fast the LFO runs (when not synced)",
                "phase" => " — where in its cycle the LFO starts (synced: against the bar)",
                "env_att" => " — how fast the envelope rises with the level",
                "env_rel" => " — how fast it falls back",
                _ => " — the level the envelope reads, before it maps −60…0 dB to 0…100 %",
            };
            return Some(Help::default().text(format!("{} ", k.spec.name)).slot(k.spec.format(v), k.spec.format(k.spec.max)).text(words));
        }
        let drag = self.drag.map(|d| MatrixHot::SlotAmount(d.slot));
        Some(Help::default().text(match drag.or(self.hot)? {
            MatrixHot::Close => "Close the modulation matrix (the routes keep playing)".to_owned(),
            MatrixHot::Shape(_, shape) => format!("{} — the LFO's shape", LFO_SHAPE.choices[shape.index()]),
            MatrixHot::Sync(_) => "Sync — the LFO locked to the song: one cycle per division".to_owned(),
            MatrixHot::Division(_) => "Division — one cycle this long when synced; click to pick, wheel to step".to_owned(),
            MatrixHot::EnvSource => "Envelope source — follow the input or the sidechain".to_owned(),
            MatrixHot::SlotSource(_) => "Source — click to step: off, LFO 1, LFO 2, envelope".to_owned(),
            MatrixHot::SlotTarget(_) => "Target — what this slot moves; click to pick".to_owned(),
            MatrixHot::SlotAmount(s) => {
                let v = plain(host, &slot_id(s, M_AMT.id));
                return Some(Help::default().text("Amount ").slot(M_AMT.format(v), "−100 %").text(" — how far it moves the target, as a share of its knob's travel (drag across; double-click: 0; negative: the other way)"));
            }
            MatrixHot::SlotClear(_) => "Clear this slot".to_owned(),
            MatrixHot::Pick(t) => format!("{} — move this", self.config.targets.names[t]),
        }))
    }

    pub fn draw(&self, cx: &mut DrawCx<'_>, tap: &MatrixTap) {
        let theme = cx.theme;
        let accent = theme.accent;
        if theme.shadows {
            cx.scene.draw_blurred_rounded_rect(Affine::translate((0.0, 6.0)), self.rect, vello::peniko::Color::from_rgba8(0, 0, 0, 160), 12.0, 16.0);
        }
        panel(cx.scene, RoundedRect::from_rect(self.rect, 12.0), theme.display, theme.well_border);
        cx.text.draw(cx.scene, "MODULATION", self.rect.x0 + 16.0, self.rect.y0 + 10.0, TextStyle::LABEL.sized(10.0), theme.text_mid, Align::Left);
        // Close.
        let c = self.close_rect().center();
        let ink = if self.hot == Some(MatrixHot::Close) { theme.text_hi } else { theme.text_lo };
        let mut x = BezPath::new();
        x.move_to((c.x - 5.0, c.y - 5.0));
        x.line_to((c.x + 5.0, c.y + 5.0));
        x.move_to((c.x + 5.0, c.y - 5.0));
        x.line_to((c.x - 5.0, c.y + 5.0));
        cx.scene.stroke(&Stroke::new(1.6), Affine::IDENTITY, ink, None, &x);
        let host = cx.host;
        for l in 0..LFOS {
            self.draw_lfo(cx, host, tap, l);
        }
        self.draw_env(cx, host, tap);
        for k in &self.knobs {
            if k.spec.id == LFO_RATE.id {
                let l = usize::from(k.id.starts_with("lfo2"));
                if plain(host, &lfo_id(l, LFO_SYNC.id)) >= 0.5 {
                    let r = 0.5 * k.diameter;
                    cx.scene.stroke(&Stroke::new(1.2).with_dashes(0.0, [3.0, 3.0]), Affine::IDENTITY, accent.multiply_alpha(0.55), None, &Circle::new(k.center, r));
                    cx.text.draw(cx.scene, LFO_DIV.choices[index(host, &lfo_id(l, LFO_DIV.id)).min(15)], k.center.x, k.center.y - 6.0, TextStyle::LABEL.sized(9.0).weighted(700.0), accent, Align::Center);
                    cx.text.draw(cx.scene, k.label, k.center.x, k.center.y + r + 4.0, TextStyle::LABEL, theme.text_lo, Align::Center);
                    continue;
                }
            }
            k.draw(cx);
        }
        self.draw_slots(cx, host, tap);
        if let Some(slot) = self.picking {
            self.draw_picker(cx, host, slot);
        }
        if let Some((_, menu)) = &self.menu {
            menu.draw(cx, accent, |_, _, _, _| {});
        }
        let _ = accent;
    }

    fn pill(&self, cx: &mut DrawCx<'_>, r: Rect, text: &str, on: bool, hot: bool) {
        let theme = cx.theme;
        let rr = RoundedRect::from_rect(r, r.height() / 2.0);
        let ink = theme.accent;
        cx.scene.fill(Fill::NonZero, Affine::IDENTITY, if on { ink.multiply_alpha(0.18) } else { theme.well }, None, &rr);
        if on {
            glow_stroke(cx, &rr, 1.2, ink, ink, 2.5);
        } else {
            cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, if hot { theme.text_hi } else { theme.well_border }, None, &rr);
        }
        let c = r.center();
        cx.text.draw(cx.scene, text, c.x, c.y - 6.0, TextStyle::LABEL.sized(10.0), if on { ink } else if hot { theme.text_hi } else { theme.text_mid }, Align::Center);
    }

    fn draw_lfo(&self, cx: &mut DrawCx<'_>, host: &dyn ParamHost, tap: &MatrixTap, l: usize) {
        let theme = cx.theme;
        let accent = theme.accent;
        let card = Self::card_rect(self.rect, l);
        panel(cx.scene, RoundedRect::from_rect(card, 8.0), theme.well, theme.well_border);
        cx.text.draw(cx.scene, if l == 0 { "LFO 1" } else { "LFO 2" }, card.x0 + 12.0, card.y0 + 7.0, TextStyle::LABEL.sized(9.0), theme.text_mid, Align::Left);
        let shape = LfoShape::from_index(index(host, &lfo_id(l, LFO_SHAPE.id)));
        let phase0 = f64::from(plain(host, &lfo_id(l, LFO_PHASE.id)) / 360.0);
        let w = self.wave_rect(l);
        let mid = w.center().y;
        let h = 0.45 * w.height();
        cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, theme.grid_major, None, &Line::new((w.x0, mid), (w.x1, mid)));
        let p = f64::from(tap.cycles[l].load()) + f64::from(tap.phases[l].load()) + phase0;
        let cycle = p.floor();
        let seed = 0x5EED + l as u64;
        let n = 72;
        let pts: Vec<Point> = (0..=n).map(|i| {
            let t = i as f64 / n as f64;
            Point::new(w.x0 + t * w.width(), mid - h * f64::from(lfo_at(shape, cycle + t, seed)))
        }).collect();
        let path = if matches!(shape, LfoShape::Square | LfoShape::SampleHold | LfoShape::SawUp | LfoShape::SawDown) {
            let mut b = BezPath::new();
            for (i, q) in pts.iter().enumerate() {
                if i == 0 { b.move_to(*q) } else { b.line_to(*q) }
            }
            b
        } else {
            smooth_path(&pts)
        };
        glow_stroke(cx, &path, 1.8, accent, accent, 2.2);
        let f = p.rem_euclid(1.0);
        let dot = Point::new(w.x0 + f * w.width(), mid - h * f64::from(tap.sources[l].load()));
        cx.scene.fill(Fill::NonZero, Affine::IDENTITY, accent, None, &Circle::new(dot, 4.5));
        // Shapes, drawn by the same function.
        for s in LfoShape::ALL {
            let r = self.shape_rect(l, s);
            let rr = RoundedRect::from_rect(r, 6.0);
            let on = s == shape;
            let hot = self.hot == Some(MatrixHot::Shape(l, s));
            cx.scene.fill(Fill::NonZero, Affine::IDENTITY, if on { accent.multiply_alpha(0.16) } else { theme.display }, None, &rr);
            cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, if on { accent } else if hot { theme.text_lo } else { theme.well_border }, None, &rr);
            let inner = r.inset(-5.0);
            let cycles = if matches!(s, LfoShape::SmoothRandom | LfoShape::SampleHold) { 3.0 } else { 1.0 };
            let mut b = BezPath::new();
            for i in 0..=24 {
                let t = i as f64 / 24.0;
                let q = Point::new(inner.x0 + t * inner.width(), inner.center().y - 0.5 * inner.height() * f64::from(lfo_at(s, 3.0 + cycles * t, 9)));
                if i == 0 { b.move_to(q) } else { b.line_to(q) }
            }
            cx.scene.stroke(&Stroke::new(1.3), Affine::IDENTITY, if on { accent } else if hot { theme.text_hi } else { theme.text_mid }, None, &b);
        }
        let sync = plain(host, &lfo_id(l, LFO_SYNC.id)) >= 0.5;
        self.pill(cx, self.sync_rect(l), "SYNC", sync, self.hot == Some(MatrixHot::Sync(l)));
        let div = LFO_DIV.choices[index(host, &lfo_id(l, LFO_DIV.id)).min(15)];
        self.pill(cx, self.division_rect(l), div, sync, self.hot == Some(MatrixHot::Division(l)));
        let rate = f64::from(tap.rates[l].load());
        let words = if rate <= 0.0 { String::new() } else if rate >= 1.0 { format!("{rate:.2} Hz") } else { format!("{:.1} s", 1.0 / rate) };
        cx.text.draw(cx.scene, &words, card.x0 + 340.0, card.y0 + 60.0, TextStyle { tabular: true, ..TextStyle::BODY.sized(10.0) }, theme.text_lo, Align::Left);
    }

    fn draw_env(&self, cx: &mut DrawCx<'_>, host: &dyn ParamHost, tap: &MatrixTap) {
        let theme = cx.theme;
        let accent = theme.accent;
        let card = Self::card_rect(self.rect, 2);
        panel(cx.scene, RoundedRect::from_rect(card, 8.0), theme.well, theme.well_border);
        cx.text.draw(cx.scene, "ENVELOPE", card.x0 + 12.0, card.y0 + 7.0, TextStyle::LABEL.sized(9.0), theme.text_mid, Align::Left);
        // Its level now, 0…100 % (−60…0 dB after its gain).
        let bar = Rect::new(card.x0 + 12.0, card.center().y + 2.0 - 7.0, card.x0 + 160.0, card.center().y + 2.0 + 7.0);
        cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.display, None, &RoundedRect::from_rect(bar, 7.0));
        let v = f64::from(tap.sources[2].load().clamp(0.0, 1.0));
        if v > 0.01 {
            let lit = Rect::new(bar.x0, bar.y0, bar.x0 + v * bar.width(), bar.y1);
            glow_stroke(cx, &Line::new((lit.x0 + 4.0, lit.center().y), (lit.x1.max(lit.x0 + 4.0), lit.center().y)), 8.0, accent, accent, 2.5);
        }
        for t in [0.25, 0.5, 0.75] {
            let x = bar.x0 + t * bar.width();
            cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, theme.grid_minor, None, &Line::new((x, bar.y1 + 3.0), (x, bar.y1 + 7.0)));
        }
        if self.config.sidechain {
            let sc = index(host, ENV_SRC.id) == 1;
            self.pill(cx, self.env_source_rect(), if sc { "SIDECHAIN" } else { "INPUT" }, sc, self.hot == Some(MatrixHot::EnvSource));
        }
    }

    fn draw_slots(&self, cx: &mut DrawCx<'_>, host: &dyn ParamHost, tap: &MatrixTap) {
        let theme = cx.theme;
        let accent = theme.accent;
        let a = self.slots_area();
        let head = TextStyle::LABEL.sized(9.0);
        cx.text.draw(cx.scene, "SOURCE", self.slot_source_rect(0).center().x, a.y0, head, theme.text_lo, Align::Center);
        cx.text.draw(cx.scene, "TARGET", self.slot_target_rect(0).x0 + 10.0, a.y0, head, theme.text_lo, Align::Left);
        cx.text.draw(cx.scene, "AMOUNT", self.slot_amount_rect(0).center().x, a.y0, head, theme.text_lo, Align::Center);
        for s in 0..MOD_SLOTS {
            let row = self.row_rect(s);
            let src = Source::from_index(index(host, &slot_id(s, M_SRC.id)));
            let dst = index(host, &slot_id(s, "dst")).min(self.config.targets.names.len() - 1);
            let amt = plain(host, &slot_id(s, M_AMT.id));
            let live = src != Source::Off && dst != 0 && amt != 0.0;
            cx.text.draw(cx.scene, &(s + 1).to_string(), row.x0 + 10.0, row.center().y - 6.0, TextStyle { tabular: true, ..TextStyle::LABEL.sized(10.0) }, if live { theme.text_mid } else { theme.text_lo }, Align::Center);
            self.pill(cx, self.slot_source_rect(s), SOURCE_NAMES[src.index()], src != Source::Off, self.hot == Some(MatrixHot::SlotSource(s)));
            // Target.
            let t = self.slot_target_rect(s);
            let hot = self.hot == Some(MatrixHot::SlotTarget(s)) || self.picking == Some(s);
            let rr = RoundedRect::from_rect(t, 6.0);
            cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.well, None, &rr);
            cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, if hot { theme.text_hi } else { theme.well_border }, None, &rr);
            let name = if dst == 0 { "—" } else { self.config.targets.names[dst] };
            cx.text.draw(cx.scene, name, t.x0 + 10.0, t.center().y - 7.0, TextStyle::BODY.sized(11.0), if dst == 0 { theme.text_lo } else { theme.text_hi }, Align::Left);
            let mut chev = BezPath::new();
            chev.move_to((t.x1 - 16.0, t.center().y - 2.0));
            chev.line_to((t.x1 - 12.0, t.center().y + 2.0));
            chev.line_to((t.x1 - 8.0, t.center().y - 2.0));
            cx.scene.stroke(&Stroke::new(1.2), Affine::IDENTITY, theme.text_lo, None, &chev);
            // Amount: a bar from the middle, and what it does now (a tick).
            let r = self.slot_amount_rect(s);
            let hot = self.hot == Some(MatrixHot::SlotAmount(s)) || self.drag.is_some_and(|d| d.slot == s);
            let rr = RoundedRect::from_rect(r, 6.0);
            cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.well, None, &rr);
            cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, if hot { theme.text_hi } else { theme.well_border }, None, &rr);
            let cxm = r.center().x;
            cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, theme.grid_major, None, &Line::new((cxm, r.y0 + 4.0), (cxm, r.y1 - 4.0)));
            let half = 0.5 * r.width() - 6.0;
            if amt != 0.0 {
                let x1 = cxm + half * f64::from(amt / 100.0);
                let ink = if live { accent } else { theme.text_lo };
                let bar = Line::new((cxm, r.center().y), (x1, r.center().y));
                if live {
                    glow_stroke(cx, &bar, 6.0, ink, ink, 2.0);
                } else {
                    cx.scene.stroke(&Stroke::new(6.0), Affine::IDENTITY, ink.multiply_alpha(0.5), None, &bar);
                }
            }
            if live {
                let v = match src {
                    Source::Lfo1 => tap.sources[0].load(),
                    Source::Lfo2 => tap.sources[1].load(),
                    Source::Envelope => tap.sources[2].load(),
                    Source::Off => 0.0,
                };
                let x = cxm + half * f64::from(amt / 100.0 * v);
                cx.scene.stroke(&Stroke::new(2.0), Affine::IDENTITY, theme.text_strong, None, &Line::new((x, r.y0 + 3.0), (x, r.y1 - 3.0)));
            }
            cx.text.draw(cx.scene, &M_AMT.format(amt), if amt >= 0.0 { r.x0 + 8.0 } else { r.x1 - 8.0 }, r.center().y - 6.0, TextStyle { tabular: true, ..TextStyle::BODY.sized(10.0) }, theme.text_lo, if amt >= 0.0 { Align::Left } else { Align::Right });
            if src != Source::Off {
                let c = self.slot_clear_rect(s).center();
                let ink = if self.hot == Some(MatrixHot::SlotClear(s)) { theme.text_hi } else { theme.text_lo };
                let mut x = BezPath::new();
                x.move_to((c.x - 4.0, c.y - 4.0));
                x.line_to((c.x + 4.0, c.y + 4.0));
                x.move_to((c.x + 4.0, c.y - 4.0));
                x.line_to((c.x - 4.0, c.y + 4.0));
                cx.scene.stroke(&Stroke::new(1.4), Affine::IDENTITY, ink, None, &x);
            }
        }
    }

    fn draw_picker(&self, cx: &mut DrawCx<'_>, host: &dyn ParamHost, slot: usize) {
        let theme = cx.theme;
        let accent = theme.accent;
        glass_panel(cx, RoundedRect::from_rect(self.rect, 12.0), theme.blur_menu);
        cx.text.draw(cx.scene, &format!("SLOT {} MOVES…", slot + 1), self.rect.x0 + 16.0, self.rect.y0 + 14.0, TextStyle::LABEL.sized(10.0), theme.text_mid, Align::Left);
        let cur = index(host, &slot_id(slot, "dst"));
        for (i, r) in self.pick_rects().into_iter().enumerate() {
            let on = i == cur;
            let hot = self.hot == Some(MatrixHot::Pick(i));
            let rr = RoundedRect::from_rect(r, 6.0);
            if on {
                cx.scene.fill(Fill::NonZero, Affine::IDENTITY, accent.multiply_alpha(0.18), None, &rr);
                cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, accent.multiply_alpha(0.6), None, &rr);
            } else if hot {
                cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.panel_control, None, &rr);
            }
            cx.text.draw(cx.scene, self.config.targets.names[i], r.x0 + 10.0, r.center().y - 7.0, TextStyle::BODY.sized(11.0), if on { theme.text_strong } else if hot { theme.text_hi } else { theme.text_mid }, Align::Left);
        }
    }
}

/// What moves parameter `id`, for its dial: the reach of the routes to it (a share of its travel
/// either way: an LFO swings both ways, the envelope one) and where they put it now. `None` when
/// no route moves it.
pub fn knob_modulation(host: &dyn ParamHost, targets: &Targets, tap: &MatrixTap, id: &str) -> Option<(f32, f32, f32)> {
    let t = targets.index(id)?;
    let (mut lo, mut hi, mut any) = (0.0f32, 0.0f32, false);
    for s in 0..MOD_SLOTS {
        let src = Source::from_index(index(host, &slot_id(s, M_SRC.id)));
        if src == Source::Off || index(host, &slot_id(s, "dst")) != t {
            continue;
        }
        let a = plain(host, &slot_id(s, M_AMT.id)) / 100.0;
        if a == 0.0 {
            continue;
        }
        any = true;
        if src == Source::Envelope {
            lo += a.min(0.0);
            hi += a.max(0.0);
        } else {
            lo -= a.abs();
            hi += a.abs();
        }
    }
    any.then(|| (lo, hi, tap.offset(t)))
}

/// How many slots route something (for the button that opens the panel).
pub fn active_routes(host: &dyn ParamHost) -> usize {
    (0..MOD_SLOTS)
        .filter(|&s| index(host, &slot_id(s, M_SRC.id)) != 0 && index(host, &slot_id(s, "dst")) != 0 && plain(host, &slot_id(s, M_AMT.id)) != 0.0)
        .count()
}

/// The button that opens the panel: "MOD" and how many routes play.
pub fn draw_matrix_button(cx: &mut DrawCx<'_>, r: Rect, open: bool, hot: bool, routes: usize) {
    let theme = cx.theme;
    let ink = theme.accent;
    let rr = RoundedRect::from_rect(r, r.height() / 2.0);
    let lit = open || routes > 0;
    cx.scene.fill(Fill::NonZero, Affine::IDENTITY, if open { ink.multiply_alpha(0.2) } else { theme.well }, None, &rr);
    if lit {
        glow_stroke(cx, &rr, 1.2, ink, ink, if open { 3.0 } else { 1.5 });
    } else {
        cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, if hot { theme.text_hi } else { theme.well_border }, None, &rr);
    }
    let text = if routes > 0 { format!("MOD · {routes}") } else { "MOD".to_owned() };
    let c = r.center();
    cx.text.draw(cx.scene, &text, c.x, c.y - 6.0, TextStyle::LABEL.sized(10.0), if lit { ink } else if hot { theme.text_hi } else { theme.text_mid }, Align::Center);
}
