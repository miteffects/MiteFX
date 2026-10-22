//! The help strip (docs/design/interaction.md): one line saying what is under the pointer. It is
//! the UI's tooltip, so controls themselves carry no words.

use vello::kurbo::{Affine, Line, Rect, Stroke};
use vello::peniko::Fill;

use crate::text::{Align, TextStyle};
use crate::view::DrawCx;

/// One line of the help strip: plain text, and values in slots as wide as the widest value they
/// can hold (the template), so moving the pointer changes the numbers without moving the words.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Help(Vec<(String, Option<String>)>);

impl Help {
    pub fn text(mut self, text: impl Into<String>) -> Self {
        self.0.push((text.into(), None));
        self
    }

    pub fn slot(mut self, value: impl Into<String>, widest: impl Into<String>) -> Self {
        self.0.push((value.into(), Some(widest.into())));
        self
    }

    /// The line as plain text (tests).
    pub fn plain(&self) -> String {
        self.0.iter().map(|(t, _)| t.as_str()).collect()
    }

    /// Draws the strip in `rect`: its background, a top rule, and the line.
    pub fn draw(&self, cx: &mut DrawCx<'_>, rect: Rect) {
        let theme = cx.theme;
        cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.help_strip, None, &rect);
        cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, theme.line, None, &Line::new((rect.x0, rect.y0 + 0.5), (rect.x1, rect.y0 + 0.5)));
        // Tabular digits, so every number in a slot takes the same width.
        let style = TextStyle { tabular: true, ..TextStyle::BODY.sized(11.0) };
        let mut x = rect.x0 + 14.0;
        for (text, widest) in &self.0 {
            let width = cx.text.draw(cx.scene, text, x, rect.y0 + 7.0, style, theme.text_mid, Align::Left);
            let slot = widest.as_ref().map_or(0.0, |w| cx.text.measure(w, style));
            x += f64::from(width.max(slot));
        }
    }
}
