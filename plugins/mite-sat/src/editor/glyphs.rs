//! Glyphs (24×24, stroked) and the model icons, each a tiny drawing of its own curve.

use std::sync::OnceLock;

use mite_dsp::sat::{magnet_loop, transfer, Model};
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

/// Tape's and Transformer's icons: their loop (from the DSP) where their emphasis is flat
/// (Tape at 200 Hz, Transformer at 1 kHz), so it shows the core's curve and loop; traced once.
fn icon_loop(model: Model) -> &'static [(f32, f32)] {
    static LOOPS: OnceLock<[Vec<(f32, f32)>; 2]> = OnceLock::new();
    let loops = LOOPS.get_or_init(|| [magnet_loop(Model::Tape, 1.0 / mite_dsp::sat::CALIBRATION, 12.0, 0.0, 200.0, 64), magnet_loop(Model::Transformer, 1.0 / mite_dsp::sat::CALIBRATION, 9.0, 0.0, 1000.0, 64)]);
    &loops[model.magnet().unwrap_or(0)]
}

/// A stereo mode's icon: two overlapping rings (left and right), with a dot on what is saturated
/// separately: nothing for plain stereo, the overlap for Mid, the outer parts for Side, all for M/S.
pub fn draw_stereo(scene: &mut Scene, mode: usize, c: Point, size: f64, color: Color) {
    let r = size * 0.3;
    let dx = size * 0.17;
    for s in [-1.0, 1.0] {
        scene.stroke(&Stroke::new(1.3), Affine::IDENTITY, color, None, &mite_ui::kurbo::Circle::new((c.x + s * dx, c.y), r));
    }
    let dot = |scene: &mut Scene, x: f64| scene.fill(mite_ui::peniko::Fill::NonZero, Affine::IDENTITY, color, None, &mite_ui::kurbo::Circle::new((x, c.y), size * 0.075));
    if mode == 1 || mode == 3 {
        dot(scene, c.x);
    }
    if mode == 2 || mode == 3 {
        dot(scene, c.x - dx - r * 0.52);
        dot(scene, c.x + dx + r * 0.52);
    }
}

/// A model's icon: its own transfer curve (from the DSP) over −1…1, `size` across, centred on
/// `c` (Tape and Transformer: their loop).
pub fn draw_model(scene: &mut Scene, model: Model, c: Point, size: f64, color: Color, width: f64) {
    let stroke = Stroke::new(width).with_caps(Cap::Round).with_join(Join::Round);
    let half = size / 2.0;
    if model.magnet().is_some() {
        let pts = icon_loop(model);
        let peak = pts.iter().fold(0.0f32, |m, p| m.max(p.1.abs())).max(1e-6);
        let mut path = BezPath::new();
        for (i, &(x, y)) in pts.iter().enumerate() {
            let p = Point::new(c.x + f64::from(x) * f64::from(mite_dsp::sat::CALIBRATION) * half, c.y - f64::from(y / peak) * half * 0.8);
            if i == 0 {
                path.move_to(p);
            } else {
                path.line_to(p);
            }
        }
        path.close_path();
        scene.stroke(&stroke, Affine::IDENTITY, color, None, &path);
        return;
    }
    let drive = icon_drive(model);
    // Fit the curve's height to the box, so models that never reach ±1 still read.
    let n = 48;
    let ys: Vec<f64> = (0..=n).map(|i| f64::from(transfer(model, (-1.0 + 2.0 * i as f32 / n as f32) / mite_dsp::sat::CALIBRATION, drive, 0.0))).collect();
    let peak = ys.iter().fold(0.0f64, |m, v| m.max(v.abs())).max(1e-6);
    let mut path = BezPath::new();
    for (i, y) in ys.iter().enumerate() {
        let p = Point::new(c.x - half + size * i as f64 / n as f64, c.y - y / peak * half * 0.8);
        if i == 0 {
            path.move_to(p);
        } else {
            path.line_to(p);
        }
    }
    scene.stroke(&stroke, Affine::IDENTITY, color, None, &path);
}
