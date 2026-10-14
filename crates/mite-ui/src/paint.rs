//! Shared painting helpers that stand in for the design's CSS/SVG effects.
//!
//! Phase 0 approximates glow with stacked translucent strokes. The real WGSL bloom pass
//! (docs/design/README.md, "How the mockup's CSS/SVG effects map to the Rust renderer") replaces
//! [`glow_stroke`] without changing call sites.

use vello::kurbo::{Affine, Cap, RoundedRect, Shape, Stroke};
use vello::peniko::{Color, Fill};
use vello::Scene;

/// Strokes `shape` in `color` with a soft `glow` halo of radius ≈ `sigma` underneath, like the
/// mockups' `feGaussianBlur` + merge (glow = same colour) or `feDropShadow` (glow = accent).
pub fn glow_stroke(scene: &mut Scene, shape: &impl Shape, width: f64, color: Color, glow: Color, sigma: f64) {
    glow_halo(scene, shape, width, glow, sigma);
    scene.stroke(&Stroke::new(width).with_caps(Cap::Round), Affine::IDENTITY, color, None, shape);
}

/// Fills `shape` with a glow halo around its outline.
pub fn glow_fill(scene: &mut Scene, shape: &impl Shape, color: Color, sigma: f64) {
    glow_halo(scene, shape, 0.0, color, sigma);
    scene.fill(Fill::NonZero, Affine::IDENTITY, color, None, shape);
}

fn glow_halo(scene: &mut Scene, shape: &impl Shape, width: f64, glow: Color, sigma: f64) {
    const LAYERS: [(f64, f32); 3] = [(1.6, 0.06), (1.0, 0.11), (0.5, 0.18)];
    for (spread, alpha) in LAYERS {
        let stroke = Stroke::new(width + 2.0 * sigma * spread).with_caps(Cap::Round);
        scene.stroke(&stroke, Affine::IDENTITY, glow.multiply_alpha(alpha), None, shape);
    }
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
