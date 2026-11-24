//! The miteFX mark and plugin title (tokens board, "LOGO DIRECTION").

use vello::kurbo::{Affine, BezPath, Cap, Stroke};

use crate::paint::glow_stroke;
use crate::text::{Align, TextStyle};
use crate::view::DrawCx;

/// Draws the mark — a mite seen from above carrying the plugin's signal glyph — in a `size`-px
/// square at `(x, y)`, from the mockup's 32-unit paths.
pub fn draw_mark(cx: &mut DrawCx<'_>, x: f64, y: f64, size: f64) {
    let theme = cx.theme;
    let s = size / 32.0;
    let to = Affine::translate((x, y)) * Affine::scale(s);
    let path = |d: &str| {
        let mut p = BezPath::from_svg(d).expect("valid mark path");
        p.apply_affine(to);
        p
    };
    let legs = path("M11.2 14 L6.5 9.5 M10.4 17 L4.5 15 M10.2 20.5 L4.5 21.5 M10.8 24 L6 28 M20.8 14 L25.5 9.5 M21.6 17 L27.5 15 M21.8 20.5 L27.5 21.5 M21.2 24 L26 28");
    let body = path("M16 11 C20.5 11 22 15 22 19 C22 24 19.5 28 16 28 C12.5 28 10 24 10 19 C10 15 11.5 11 16 11 Z");
    let head = path("M18.6 7.4 A2.6 2.6 0 1 1 13.4 7.4 A2.6 2.6 0 1 1 18.6 7.4 Z");
    let signal = path("M11.8 21 C14 21 14.6 16.5 16 16.5 C17.4 16.5 18 21 20.2 21");
    let round = |w: f64| Stroke::new(w * s).with_caps(Cap::Round);
    cx.scene.stroke(&round(1.4), Affine::IDENTITY, theme.text_lo, None, &legs);
    cx.scene.stroke(&round(1.6), Affine::IDENTITY, theme.text_hi, None, &body);
    cx.scene.stroke(&round(1.6), Affine::IDENTITY, theme.text_hi, None, &head);
    glow_stroke(cx, &signal, 1.8 * s, theme.accent, theme.accent, 3.0 * s);
}

/// "Mite" in ExtraBold followed by the plugin name in Medium, as in the header mockups.
/// Returns the total width.
pub fn draw_title(cx: &mut DrawCx<'_>, x: f64, y: f64, plugin: &str) -> f64 {
    let theme = cx.theme;
    let w = cx.text.draw(cx.scene, "Mite", x, y, TextStyle::TITLE.weighted(800.0), theme.text_hi, Align::Left);
    let rest = format!(" {plugin}");
    let w2 = cx.text.draw(cx.scene, &rest, x + f64::from(w), y, TextStyle::TITLE.weighted(500.0), theme.text_mid, Align::Left);
    f64::from(w + w2)
}
