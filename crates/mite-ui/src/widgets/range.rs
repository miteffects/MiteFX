//! A display's dB range (as FabFilter's): Auto fits the range to what is shown, or the user picks
//! one. A small pill on the scale shows the range in force and opens the list; the wheel over
//! the scale steps through the ranges. The choice lives in a shared store the plugin saves with
//! its state (0 = Auto); it is a view setting, not a parameter, so it is never automated.

use std::sync::Arc;

use mite_core::AtomicF32;
use vello::kurbo::{Affine, Point, Rect, RoundedRect, Stroke};
use vello::peniko::Fill;

use crate::text::{Align, TextStyle};
use crate::view::{DrawCx, MouseButton, UiEvent};
use crate::widgets::{Dropdown, DropdownEvent};

pub struct RangePicker {
    /// The pill.
    pub rect: Rect,
    /// Where the wheel steps the range (the scale).
    pub wheel_area: Rect,
    /// The ranges on offer, and the list's rows: "Auto" first, then one per range.
    values: &'static [f32],
    labels: &'static [&'static str],
    store: Arc<AtomicF32>,
    /// The band the list must stay inside (top, bottom).
    area: (f64, f64),
    menu: Option<Dropdown>,
    pointer: Option<Point>,
    /// What the pill puts before the number: ± for a range about 0 dB, − for a depth below it.
    prefix: &'static str,
}

impl RangePicker {
    pub fn new(rect: Rect, wheel_area: Rect, values: &'static [f32], labels: &'static [&'static str], store: Arc<AtomicF32>, area: (f64, f64)) -> Self {
        debug_assert_eq!(labels.len(), values.len() + 1);
        Self { rect, wheel_area, values, labels, store, area, menu: None, pointer: None, prefix: "±" }
    }

    pub fn with_prefix(mut self, prefix: &'static str) -> Self {
        self.prefix = prefix;
        self
    }

    /// The range picked by hand, or `None` for Auto.
    pub fn manual(&self) -> Option<f32> {
        let v = self.store.load();
        (v > 0.0).then_some(v)
    }

    /// The range in force: the manual one, or `auto` (what the view fits) snapped up to the
    /// nearest range on offer.
    pub fn target(&self, auto: f32) -> f32 {
        self.manual().unwrap_or_else(|| self.values.iter().copied().find(|&v| v >= auto).unwrap_or(self.values[self.values.len() - 1]))
    }

    pub fn menu_open(&self) -> bool {
        self.menu.is_some()
    }

    fn current_row(&self) -> usize {
        self.manual().and_then(|m| self.values.iter().position(|&v| (v - m).abs() < 1e-3)).map_or(0, |i| i + 1)
    }

    /// The pill: the range now (`shown`, as eased), and a small A while Auto chooses it.
    pub fn draw(&self, cx: &mut DrawCx<'_>, shown: f32) {
        let theme = cx.theme;
        let hot = self.pointer.is_some_and(|p| self.rect.contains(p)) || self.menu.is_some();
        let shape = RoundedRect::from_rect(self.rect, self.rect.height() / 2.0);
        cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.display.multiply_alpha(0.85), None, &shape);
        cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, if hot { theme.accent } else { theme.control_border }, None, &shape);
        let style = TextStyle { tabular: true, ..TextStyle::READOUT.sized(10.0) };
        let text = format!("{}{:.0}", self.prefix, shown);
        let auto = self.manual().is_none();
        let cy = self.rect.y0 + (self.rect.height() - 12.0) / 2.0;
        if auto {
            let chip = Rect::new(self.rect.x0 + 4.0, self.rect.y0 + 3.0, self.rect.x0 + 16.0, self.rect.y1 - 3.0);
            cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.accent.multiply_alpha(0.22), None, &RoundedRect::from_rect(chip, 3.0));
            cx.text.draw(cx.scene, "A", chip.center().x, cy, TextStyle::LABEL.sized(9.0).weighted(800.0), theme.accent, Align::Center);
            cx.text.draw(cx.scene, &text, (chip.x1 + self.rect.x1) / 2.0, cy, style, theme.text_hi, Align::Center);
        } else {
            cx.text.draw(cx.scene, &text, self.rect.center().x, cy, style, theme.text_hi, Align::Center);
        }
    }

    /// The open list, over everything.
    pub fn draw_menu(&self, cx: &mut DrawCx<'_>) {
        if let Some(menu) = &self.menu {
            menu.draw(cx, cx.theme.accent, |_, _, _, _| {});
        }
    }

    /// Steps the range by `notches` (positive: a wider range), leaving Auto at the range shown.
    fn step(&self, shown: f32, notches: f32) {
        let at = self.values.iter().position(|&v| v >= shown - 1e-3).unwrap_or(self.values.len() - 1) as isize;
        let to = (at + if notches > 0.0 { 1 } else { -1 }).clamp(0, self.values.len() as isize - 1) as usize;
        self.store.store(self.values[to]);
    }

    /// Events: the list is modal; a click on the pill opens it; the wheel over the scale steps.
    /// `shown` is the range in force. Returns `true` if used.
    pub fn event(&mut self, event: &UiEvent, shown: f32) -> bool {
        if let Some(menu) = &mut self.menu {
            match menu.event(event) {
                DropdownEvent::Open => {}
                DropdownEvent::Picked(i) => {
                    self.store.store(if i == 0 { 0.0 } else { self.values[i - 1] });
                    self.menu = None;
                }
                DropdownEvent::Closed => self.menu = None,
            }
            return true;
        }
        match *event {
            UiEvent::PointerMove { pos, .. } => {
                self.pointer = Some(pos);
                false
            }
            UiEvent::PointerLeft => {
                self.pointer = None;
                false
            }
            UiEvent::PointerDown { pos, button: MouseButton::Left, .. } if self.rect.contains(pos) => {
                let mut list = Dropdown::new(self.rect, self.labels, self.current_row(), self.area).with_min_width(110.0);
                // Rows sit left of a pill at the right edge.
                if self.rect.x1 > 1000.0 {
                    list.anchor = Rect::new(self.rect.x1 - 110.0, self.rect.y0, self.rect.x1, self.rect.y1);
                }
                self.menu = Some(list);
                true
            }
            UiEvent::DoubleClick { pos, .. } if self.rect.contains(pos) => {
                // Back to Auto.
                self.store.store(0.0);
                true
            }
            UiEvent::Wheel { pos, lines, .. } if self.wheel_area.contains(pos) || self.rect.contains(pos) => {
                self.step(shown, -lines);
                true
            }
            _ => false,
        }
    }
}

/// Eases a shown range towards `target` (in log terms, so zooming feels even), `dt` seconds on.
/// Returns whether it moved.
pub fn ease_range(shown: &mut f32, target: f32, dt: f32) -> bool {
    if (*shown - target).abs() < 1e-3 {
        *shown = target;
        return false;
    }
    let k = 1.0 - (-dt / 0.12).exp();
    let next = (shown.ln() + (target.ln() - shown.ln()) * k).exp();
    *shown = if (next - target).abs() < 0.01 { target } else { next };
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    const VALUES: &[f32] = &[3.0, 6.0, 12.0, 30.0];
    const LABELS: &[&str] = &["Auto", "3 dB", "6 dB", "12 dB", "30 dB"];

    #[test]
    fn auto_snaps_up_and_a_pick_is_stored() {
        let store = Arc::new(AtomicF32::new(0.0));
        let mut r = RangePicker::new(Rect::new(10.0, 10.0, 50.0, 30.0), Rect::new(0.0, 0.0, 60.0, 300.0), VALUES, LABELS, store.clone(), (0.0, 400.0));
        assert_eq!(r.target(7.0), 12.0);
        assert_eq!(r.target(99.0), 30.0);
        // Wheel down over the scale: a narrower range, picked by hand.
        r.event(&UiEvent::Wheel { pos: Point::new(20.0, 100.0), lines: 1.0, mods: Default::default() }, 12.0);
        assert_eq!(r.manual(), Some(6.0));
        assert_eq!(r.target(99.0), 6.0);
        // Double-click on the pill: Auto again.
        r.event(&UiEvent::DoubleClick { pos: Point::new(20.0, 20.0), mods: Default::default() }, 6.0);
        assert_eq!(r.manual(), None);
    }

    #[test]
    fn the_shown_range_glides() {
        let mut shown = 12.0;
        let mut steps = 0;
        while ease_range(&mut shown, 30.0, 1.0 / 120.0) {
            steps += 1;
            assert!(shown > 12.0 && shown <= 30.0);
        }
        assert!(steps > 10 && shown == 30.0);
    }
}
