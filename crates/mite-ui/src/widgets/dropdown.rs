//! An open dropdown list (component sheet: menu): glass rows with an icon and a name, the current
//! choice lit in the owner's colour. It is modal while open: the owner routes every event to it
//! until it reports a pick or closes.

use vello::kurbo::{Affine, Point, Rect, RoundedRect, Stroke};
use vello::peniko::{Color, Fill};
use vello::Scene;

use crate::paint::glass_panel;
use crate::text::{Align, TextStyle};
use crate::view::{DrawCx, MouseButton, UiEvent};

const ROW_H: f64 = 30.0;

/// What an event did to an open dropdown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DropdownEvent {
    /// Still open (hover moved, or nothing).
    Open,
    /// Item `i` (an index into the items) was picked; close it.
    Picked(usize),
    /// A click elsewhere: close it.
    Closed,
}

pub struct Dropdown {
    /// The button it opened from.
    pub anchor: Rect,
    pub items: &'static [&'static str],
    /// The choices offered, in order (indices into `items`; not every item need be on offer).
    pub shown: Vec<usize>,
    pub current: usize,
    /// The hovered row (an index into `shown`).
    pub hover: Option<usize>,
    pub min_width: f64,
    /// The band of the window the list must stay inside (top, bottom).
    pub area: (f64, f64),
}

impl Dropdown {
    pub fn new(anchor: Rect, items: &'static [&'static str], current: usize, area: (f64, f64)) -> Self {
        Self { anchor, items, shown: (0..items.len()).collect(), current, hover: None, min_width: 150.0, area }
    }

    pub fn with_shown(mut self, shown: Vec<usize>) -> Self {
        self.shown = shown;
        self
    }

    pub fn with_min_width(mut self, width: f64) -> Self {
        self.min_width = width;
        self
    }

    /// Row rectangles (one per shown choice): below the button when they fit inside the area,
    /// otherwise above; rows get shorter when a long list would not fit at all.
    pub fn item_rects(&self) -> Vec<Rect> {
        let (area_top, area_bottom) = self.area;
        let count = self.shown.len();
        let w = self.anchor.width().max(self.min_width);
        let room = (area_bottom - area_top - 20.0).max(self.anchor.y0 - area_top - 10.0);
        let row = ROW_H.min((room - 8.0) / count.max(1) as f64);
        let h = count as f64 * row + 8.0;
        let top = if self.anchor.y1 + 4.0 + h <= area_bottom {
            self.anchor.y1 + 4.0
        } else if self.anchor.y0 - 4.0 - h >= area_top {
            self.anchor.y0 - 4.0 - h
        } else {
            area_top + 10.0
        };
        (0..count)
            .map(|i| {
                let y = top + 4.0 + i as f64 * row;
                Rect::new(self.anchor.x0, y, self.anchor.x0 + w, y + row)
            })
            .collect()
    }

    pub fn bounds(&self) -> Rect {
        let items = self.item_rects();
        let (Some(first), Some(last)) = (items.first(), items.last()) else { return self.anchor };
        Rect::new(first.x0, first.y0 - 4.0, first.x1, last.y1 + 4.0)
    }

    /// Draws the list over everything; `icon` draws item `i`'s 22 × 12 icon at a point.
    pub fn draw(&self, cx: &mut DrawCx<'_>, color: Color, icon: impl Fn(&mut Scene, usize, Point, Color)) {
        let theme = cx.theme;
        let bounds = self.bounds();
        if theme.shadows {
            cx.scene.draw_blurred_rounded_rect(Affine::translate((0.0, 8.0)), bounds, Color::from_rgba8(0, 0, 0, 150), 8.0, 14.0);
        }
        glass_panel(cx, RoundedRect::from_rect(bounds, 8.0), theme.blur_menu);
        for (k, r) in self.item_rects().into_iter().enumerate() {
            let i = self.shown[k];
            let r = r.inset((-4.0, -1.0));
            let shape = RoundedRect::from_rect(r, 6.0);
            let current = i == self.current;
            let hovered = self.hover == Some(k);
            // The current choice glows in the owner's colour; hover is a quiet lift.
            if current {
                cx.scene.fill(Fill::NonZero, Affine::IDENTITY, color.multiply_alpha(if hovered { 0.24 } else { 0.16 }), None, &shape);
                cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, color.multiply_alpha(0.55), None, &shape);
            } else if hovered {
                cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.panel_control, None, &shape);
            }
            let ink = if current { color } else if hovered { theme.text_hi } else { theme.text_mid };
            let mid = r.center().y;
            icon(cx.scene, i, Point::new(r.x0 + 12.0, mid - 6.0), ink);
            let text = if current { theme.text_strong } else { theme.text_hi };
            let style = TextStyle::BODY.weighted(if current { 700.0 } else { 500.0 });
            cx.text.draw(cx.scene, self.items[i], r.x0 + 44.0, mid - 8.0, style, text, Align::Left);
        }
    }

    pub fn event(&mut self, event: &UiEvent) -> DropdownEvent {
        let item_at = |d: &Self, p: Point| d.item_rects().iter().position(|r| r.contains(p));
        match *event {
            UiEvent::PointerMove { pos, .. } => self.hover = item_at(self, pos),
            UiEvent::PointerDown { pos, button: MouseButton::Left, .. } => {
                // A pick, a click outside, or the button again: all close the list.
                return match item_at(self, pos) {
                    Some(k) => DropdownEvent::Picked(self.shown[k]),
                    None => DropdownEvent::Closed,
                };
            }
            UiEvent::PointerDown { .. } => return DropdownEvent::Closed,
            UiEvent::PointerLeft => self.hover = None,
            _ => {}
        }
        DropdownEvent::Open
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::view::Modifiers;

    const ITEMS: &[&str] = &["One", "Two", "Three"];

    #[test]
    fn opens_below_when_it_fits_and_above_when_it_does_not() {
        let below = Dropdown::new(Rect::new(10.0, 100.0, 110.0, 130.0), ITEMS, 0, (50.0, 600.0));
        assert!(below.item_rects()[0].y0 > 130.0);
        let above = Dropdown::new(Rect::new(10.0, 540.0, 110.0, 570.0), ITEMS, 0, (50.0, 600.0));
        assert!(above.item_rects()[2].y1 < 540.0);
    }

    #[test]
    fn a_click_on_a_row_picks_its_item_and_elsewhere_closes() {
        let mut d = Dropdown::new(Rect::new(10.0, 100.0, 110.0, 130.0), ITEMS, 0, (50.0, 600.0)).with_shown(vec![0, 2]);
        let row = d.item_rects()[1].center();
        let mods = Modifiers::default();
        assert_eq!(d.event(&UiEvent::PointerMove { pos: row, mods }), DropdownEvent::Open);
        assert_eq!(d.hover, Some(1));
        assert_eq!(d.event(&UiEvent::PointerDown { pos: row, button: MouseButton::Left, mods }), DropdownEvent::Picked(2));
        let away = Point::new(500.0, 500.0);
        assert_eq!(d.event(&UiEvent::PointerDown { pos: away, button: MouseButton::Left, mods }), DropdownEvent::Closed);
    }
}
