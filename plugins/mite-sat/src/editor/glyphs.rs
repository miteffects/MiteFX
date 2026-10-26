//! Glyphs (24×24, stroked) and the model icons, each a tiny drawing of its own curve.

use mite_dsp::sat::{transfer, Model};
use mite_ui::kurbo::{Affine, BezPath, Cap, Join, Point, Stroke};
use mite_ui::peniko::Color;
use mite_ui::vello::Scene;

pub const OVERSAMPLE: &str = "M3 12 C5.5 5 8.5 5 11 12 S16.5 19 19 12 M5 20 H6 M9 20 H10 M13 20 H14 M17 20 H18";
pub const POWER: &str = "M12 4 V11 M7.5 6.8 A7 7 0 1 0 16.5 6.8";

/// Draws a 24×24 glyph scaled to `size`, centred on `c`.
pub fn draw(scene: &mut Scene, d: &str, c: Point, size: f64, color: Color, width: f64) {
    let Ok(mut path) = BezPath::from_svg(d) else { return };
    let k = size / 24.0;
    path.apply_affine(Affine::translate((c.x - 12.0 * k, c.y - 12.0 * k)) * Affine::scale(k));
    let stroke = Stroke::new(width).with_caps(Cap::Round).with_join(Join::Round);
    scene.stroke(&stroke, Affine::IDENTITY, color, None, &path);
}

/// The drive each model's icon is drawn at: enough to show its character.
fn icon_drive(model: Model) -> f32 {
    match model {
        Model::Fold => 13.0,
        Model::Rectify => 12.0,
        Model::Diode => 20.0,
        _ => 14.0,
    }
}

/// A model's icon: its own transfer curve (from the DSP) over −1…1, `size` across, centred on
/// `c`.
pub fn draw_model(scene: &mut Scene, model: Model, c: Point, size: f64, color: Color, width: f64) {
    let drive = icon_drive(model);
    // Fit the curve's height to the box, so models that never reach ±1 still read.
    let n = 48;
    let ys: Vec<f64> = (0..=n).map(|i| f64::from(transfer(model, -1.0 + 2.0 * i as f32 / n as f32, drive, 0.0))).collect();
    let peak = ys.iter().fold(0.0f64, |m, v| m.max(v.abs())).max(1e-6);
    let half = size / 2.0;
    let mut path = BezPath::new();
    for (i, y) in ys.iter().enumerate() {
        let p = Point::new(c.x - half + size * i as f64 / n as f64, c.y - y / peak * half * 0.8);
        if i == 0 {
            path.move_to(p);
        } else {
            path.line_to(p);
        }
    }
    let stroke = Stroke::new(width).with_caps(Cap::Round).with_join(Join::Round);
    scene.stroke(&stroke, Affine::IDENTITY, color, None, &path);
}
