//! Text: Manrope (embedded, OFL) shaped by parley and drawn as Vello glyph runs.

use std::sync::Arc;

use parley::{
    Alignment, AlignmentOptions, FontContext, FontFamily, FontFeatures, FontWeight, LayoutContext,
    PositionedLayoutItem, StyleProperty,
};
use vello::kurbo::Affine;
use vello::peniko::{Blob, Color, Fill};
use vello::{Glyph, Scene};

const MANROPE: &[u8] = include_bytes!("../fonts/Manrope-Variable.ttf");

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Align {
    Left,
    Center,
    Right,
}

/// A text style from the design's type scale (`tokens.json` → `type`).
#[derive(Debug, Clone, Copy)]
pub struct TextStyle {
    pub size: f32,
    pub weight: f32,
    /// Letter spacing in em.
    pub tracking: f32,
    /// Tabular figures, for every readout.
    pub tabular: bool,
}

impl TextStyle {
    pub const TITLE: Self = Self { size: 15.0, weight: 700.0, tracking: 0.0, tabular: false };
    pub const BODY: Self = Self { size: 12.0, weight: 500.0, tracking: 0.0, tabular: false };
    pub const LABEL: Self = Self { size: 10.0, weight: 700.0, tracking: 0.08, tabular: false };
    pub const READOUT: Self = Self { size: 11.5, weight: 600.0, tracking: 0.0, tabular: true };
    pub const READOUT_DIAL: Self = Self { size: 12.0, weight: 700.0, tracking: 0.0, tabular: true };
    pub const UNIT_DIAL: Self = Self { size: 9.0, weight: 600.0, tracking: 0.0, tabular: false };

    pub const fn sized(self, size: f32) -> Self {
        Self { size, ..self }
    }

    pub const fn weighted(self, weight: f32) -> Self {
        Self { weight, ..self }
    }
}

/// Shapes and draws text. One per window.
pub struct Typesetter {
    fonts: FontContext,
    layouts: LayoutContext<()>,
}

impl Default for Typesetter {
    fn default() -> Self {
        Self::new()
    }
}

impl Typesetter {
    pub fn new() -> Self {
        let mut fonts = FontContext::new();
        fonts.collection.register_fonts(Blob::new(Arc::new(MANROPE)), None);
        Self { fonts, layouts: LayoutContext::new() }
    }

    /// Draws `text` with its baseline-box top-left (or top-centre / top-right) at `(x, y)` in
    /// logical pixels. Returns the advance width.
    #[allow(clippy::too_many_arguments)]
    pub fn draw(
        &mut self,
        scene: &mut Scene,
        text: &str,
        x: f64,
        y: f64,
        style: TextStyle,
        color: Color,
        align: Align,
    ) -> f32 {
        let mut builder = self.layouts.ranged_builder(&mut self.fonts, text, 1.0, false);
        builder.push_default(StyleProperty::FontFamily(FontFamily::Source("Manrope".into())));
        builder.push_default(StyleProperty::FontSize(style.size));
        builder.push_default(StyleProperty::FontWeight(FontWeight::new(style.weight)));
        if style.tracking != 0.0 {
            builder.push_default(StyleProperty::LetterSpacing(style.tracking * style.size));
        }
        if style.tabular {
            builder.push_default(StyleProperty::FontFeatures(FontFeatures::from("\"tnum\" 1")));
        }
        let mut layout = builder.build(text);
        layout.break_all_lines(None);
        layout.align(Alignment::Start, AlignmentOptions::default());
        let width = layout.width();
        let dx = match align {
            Align::Left => 0.0,
            Align::Center => -(width as f64) / 2.0,
            Align::Right => -(width as f64),
        };
        let origin = Affine::translate((x + dx, y));
        for line in layout.lines() {
            for item in line.items() {
                let PositionedLayoutItem::GlyphRun(glyph_run) = item else { continue };
                let run = glyph_run.run();
                let coords: Vec<i16> = run.normalized_coords().iter().map(|c| c.to_bits()).collect();
                scene
                    .draw_glyphs(run.font())
                    .font_size(run.font_size())
                    .normalized_coords(&coords)
                    .transform(origin)
                    .brush(color)
                    .hint(false)
                    .draw(
                        Fill::NonZero,
                        glyph_run.positioned_glyphs().map(|g| Glyph { id: g.id, x: g.x, y: g.y }),
                    );
            }
        }
        width
    }
}
