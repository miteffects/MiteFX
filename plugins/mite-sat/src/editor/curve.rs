//! The transfer square: input −1…1 across, output −1…1 up, the band's curve as the DSP plays it
//! (`mite_dsp::sat::transfer` at the live operating point, times auto gain), lit where the music
//! is, with a dot riding the curve at the band's peak either way.

use mite_dsp::sat::{transfer, Model};
use mite_ui::kurbo::{Affine, BezPath, Circle, Line, Point, Rect, RoundedRect, Stroke};
use mite_ui::paint::{glow_fill, glow_stroke, panel};
use mite_ui::peniko::Fill;
use mite_ui::DrawCx;

/// Inner padding of the square, px.
const PAD: f64 = 10.0;
/// Curve samples across.
const SAMPLES: usize = 241;

/// What the square draws.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Curve {
    pub model: Model,
    pub drive_db: f32,
    /// The operating point (Tube's drifts with the music).
    pub point: f32,
    /// Auto gain's correction, linear (1 when off).
    pub auto: f32,
    /// The band's peak going in, linear (where the music reaches on the curve).
    pub peak: f32,
}

impl Curve {
    pub fn at(&self, x: f32) -> f32 {
        transfer(self.model, x, self.drive_db, self.point) * self.auto
    }
}

pub fn to_screen(r: Rect, x: f64, y: f64) -> Point {
    let w = r.width() - 2.0 * PAD;
    let h = r.height() - 2.0 * PAD;
    Point::new(r.x0 + PAD + (x + 1.0) / 2.0 * w, r.y1 - PAD - (y + 1.0) / 2.0 * h)
}

fn path(r: Rect, f: impl Fn(f32) -> f32, from: f32, to: f32) -> BezPath {
    let mut p = BezPath::new();
    let n = ((SAMPLES as f32 * (to - from) / 2.0).ceil() as usize).max(2);
    for i in 0..=n {
        let x = from + (to - from) * i as f32 / n as f32;
        let q = to_screen(r, f64::from(x), f64::from(f(x)));
        if i == 0 {
            p.move_to(q);
        } else {
            p.line_to(q);
        }
    }
    p
}

/// Draws the square in `r`. `ghost`: another model's curve at the same settings (hovering a
/// model icon). `big`: the selected card's square (grid and dots).
pub fn draw(cx: &mut DrawCx<'_>, r: Rect, curve: &Curve, ghost: Option<Model>, big: bool, alpha: f32) {
    let theme = cx.theme;
    let frame = RoundedRect::from_rect(r, 10.0);
    panel(cx.scene, frame, theme.well, theme.well_border);
    cx.scene.push_clip_layer(Fill::NonZero, Affine::IDENTITY, &frame);
    let grid = |cx: &mut DrawCx<'_>, a: Point, b: Point, strong: bool| {
        cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, if strong { theme.grid_major } else { theme.grid_minor }, None, &Line::new(a, b));
    };
    if big {
        for v in [-0.5, 0.5] {
            grid(cx, to_screen(r, v, -1.2), to_screen(r, v, 1.2), false);
            grid(cx, to_screen(r, -1.2, v), to_screen(r, 1.2, v), false);
        }
    }
    grid(cx, to_screen(r, 0.0, -1.2), to_screen(r, 0.0, 1.2), true);
    grid(cx, to_screen(r, -1.2, 0.0), to_screen(r, 1.2, 0.0), true);
    // The identity: what the band was.
    cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, theme.text_lo.multiply_alpha(0.35 * alpha), None, &Line::new(to_screen(r, -1.0, -1.0), to_screen(r, 1.0, 1.0)));
    if let Some(m) = ghost {
        let g = Curve { model: m, ..*curve };
        cx.scene.stroke(&Stroke::new(1.5), Affine::IDENTITY, theme.accent.multiply_alpha(0.4 * alpha), None, &path(r, |x| g.at(x), -1.0, 1.0));
    }
    let whole = path(r, |x| curve.at(x), -1.0, 1.0);
    cx.scene.stroke(&Stroke::new(if big { 1.6 } else { 1.3 }), Affine::IDENTITY, theme.accent.multiply_alpha(0.55 * alpha), None, &whole);
    // Lit where the music is.
    let p = curve.peak.min(1.0);
    if p > 1e-3 {
        let lit = path(r, |x| curve.at(x), -p, p);
        glow_stroke(cx, &lit, if big { 2.6 } else { 2.0 }, theme.accent.multiply_alpha(alpha), theme.accent, 6.0);
        if big {
            for x in [-p, p] {
                let c = to_screen(r, f64::from(x), f64::from(curve.at(x)));
                glow_fill(cx, &Circle::new(c, 4.5), theme.accent.multiply_alpha(alpha), 5.0);
                cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.needle.multiply_alpha(alpha), None, &Circle::new(c, 2.0));
            }
        }
    }
    cx.scene.pop_layer();
}
