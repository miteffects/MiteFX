//! Shared painting helpers that stand in for the design's CSS/SVG effects.
//!
//! Glow is real bloom: the glowing shape also goes into the frame's glow layer, which the
//! compositor blurs and lays over it (see [`crate::effects`]).

use vello::kurbo::{Affine, Cap, RoundedRect, Shape, Stroke};
use vello::peniko::{Color, Fill};
use vello::Scene;

use crate::view::DrawCx;

/// Strokes `shape` in `color` with a soft `glow` halo of radius ≈ `sigma` around it, like the
/// mockups' `feGaussianBlur` + merge (glow = same colour) or `feDropShadow` (glow = accent).
pub fn glow_stroke(cx: &mut DrawCx<'_>, shape: &impl Shape, width: f64, color: Color, glow: Color, sigma: f64) {
    glow_source(cx, shape, width, glow, sigma);
    cx.scene.stroke(&Stroke::new(width).with_caps(Cap::Round), Affine::IDENTITY, color, None, shape);
}

/// Fills `shape` with a glow halo around it.
pub fn glow_fill(cx: &mut DrawCx<'_>, shape: &impl Shape, color: Color, sigma: f64) {
    if cx.theme.shadows {
        cx.glow().fill(Fill::NonZero, Affine::IDENTITY, color.multiply_alpha(GLOW_ALPHA), None, shape);
    }
    glow_source(cx, shape, 0.0, color, sigma);
    cx.scene.fill(Fill::NonZero, Affine::IDENTITY, color, None, shape);
}

/// How strongly a glow source shines into the bloom at `sigma` = [`GLOW_REFERENCE`].
const GLOW_ALPHA: f32 = 0.55;
const GLOW_REFERENCE: f64 = 4.0;

/// The bloom blurs every glow by the same two Gaussians, so `sigma` sets the halo's strength
/// (a wider glow in the mockups reads as a brighter one at these sizes). The source is the
/// shape itself, as an SVG glow blurs the graphic. Off in high contrast (no glow there).
fn glow_source(cx: &mut DrawCx<'_>, shape: &impl Shape, width: f64, glow: Color, sigma: f64) {
    if !cx.theme.shadows {
        return;
    }
    let strength = GLOW_ALPHA * (sigma / GLOW_REFERENCE).clamp(0.5, 1.6) as f32;
    let stroke = Stroke::new(width.max(1.0)).with_caps(Cap::Round);
    cx.glow().stroke(&stroke, Affine::IDENTITY, glow.multiply_alpha(strength.min(1.0)), None, shape);
}

/// A glass panel (`elevation.glass`): the frame under it frosted with a `blur` σ, then the
/// theme's glass tint, its 1 px border and a faint top highlight. Draw its drop shadow first.
pub fn glass_panel(cx: &mut DrawCx<'_>, rect: RoundedRect, blur: f64) {
    let theme = cx.theme;
    let fill = if cx.glass(rect, blur) { theme.glass } else { theme.glass.with_alpha(0.97) };
    panel(cx.scene, rect, fill, theme.glass_border);
    let (r, b) = (rect.radii().top_left, rect.rect());
    let mut highlight = vello::kurbo::BezPath::new();
    highlight.move_to((b.x0 + r, b.y0 + 1.5));
    highlight.line_to((b.x1 - r, b.y0 + 1.5));
    // `elevation.glass`: inset 0 1px 0, white at 5 % (dark) or 80 % (light).
    let alpha = if theme.kind == crate::tokens::ThemeKind::Light { 204 } else { 13 };
    cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, Color::from_rgba8(255, 255, 255, alpha), None, &highlight);
}

/// The design's `raised` elevation: a two-layer drop shadow under a rounded rect.
pub fn raised_shadow(scene: &mut Scene, rect: RoundedRect) {
    let r = rect.radii().top_left;
    let b = rect.rect();
    scene.draw_blurred_rounded_rect(
        Affine::translate((0.0, 4.0)),
        b,
        Color::from_rgba8(0, 0, 0, 102),
        r,
        6.0,
    );
    scene.draw_blurred_rounded_rect(Affine::translate((0.0, 1.0)), b, Color::from_rgba8(0, 0, 0, 178), r, 1.0);
}

/// Filled rounded rect with a 1 px inner border, the design's control/well shape.
pub fn panel(scene: &mut Scene, rect: RoundedRect, fill: Color, border: Color) {
    scene.fill(Fill::NonZero, Affine::IDENTITY, fill, None, &rect);
    let inset = RoundedRect::from_rect(rect.rect().inset(-0.5), rect.radii().top_left - 0.5);
    scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, border, None, &inset);
}

/// A raised control (button, active segment): `panel` plus a soft drop shadow and a faint top
/// highlight, so it reads as pressable. `shadows` is the theme's flag (off in high contrast).
pub fn raised(scene: &mut Scene, rect: RoundedRect, fill: Color, border: Color, shadows: bool) {
    let r = rect.radii().top_left;
    if shadows {
        scene.draw_blurred_rounded_rect(Affine::translate((0.0, 2.0)), rect.rect(), Color::from_rgba8(0, 0, 0, 120), r, 3.0);
    }
    panel(scene, rect, fill, border);
    let b = rect.rect();
    let mut highlight = vello::kurbo::BezPath::new();
    highlight.move_to((b.x0 + r, b.y0 + 1.5));
    highlight.line_to((b.x1 - r, b.y0 + 1.5));
    scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, Color::from_rgba8(255, 255, 255, 16), None, &highlight);
}

/// Every icon button looks and answers alike: raised on `fill`, its border lit in `color` while
/// hovered or on, tinted while on. Returns the ink for its glyph.
pub fn icon_button(scene: &mut Scene, theme: &crate::Theme, r: vello::kurbo::Rect, fill: Color, hot: bool, on: bool, color: Color) -> Color {
    let shape = RoundedRect::from_rect(r, 6.0);
    raised(scene, shape, fill, if hot || on { color } else { theme.panel_control_border }, theme.shadows);
    if on {
        scene.fill(Fill::NonZero, Affine::IDENTITY, color.multiply_alpha(0.16), None, &shape);
        color
    } else if hot {
        theme.text_hi
    } else {
        theme.text_mid
    }
}
