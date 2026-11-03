//! The EQ's band-type and slope icons (22 × 12), shared by Mite EQ and the sidechain EQ (Comp,
//! Gate). Moved out of Mite EQ's editor unchanged (2026-11-03).

use mite_dsp::eq::BandType;
use vello::kurbo::{self, Affine, BezPath, Point, Stroke};
use vello::peniko::Color;
use vello::Scene;

/// Draws a band type's response shape as a 22 × 12 icon with its top-left at `at`.
pub fn draw_type_icon(scene: &mut Scene, kind: BandType, at: Point, color: Color) {
    let mut p = BezPath::new();
    let pt = |x: f64, y: f64| (at.x + x, at.y + y);
    match kind {
        BandType::Bell => {
            p.move_to(pt(0.0, 9.0));
            p.line_to(pt(5.0, 9.0));
            p.curve_to(pt(8.0, 9.0), pt(9.0, 2.0), pt(11.0, 2.0));
            p.curve_to(pt(13.0, 2.0), pt(14.0, 9.0), pt(17.0, 9.0));
            p.line_to(pt(22.0, 9.0));
        }
        BandType::LowShelf => {
            p.move_to(pt(0.0, 3.0));
            p.line_to(pt(6.0, 3.0));
            p.curve_to(pt(10.0, 3.0), pt(12.0, 9.0), pt(16.0, 9.0));
            p.line_to(pt(22.0, 9.0));
        }
        BandType::HighShelf => {
            p.move_to(pt(0.0, 9.0));
            p.line_to(pt(6.0, 9.0));
            p.curve_to(pt(10.0, 9.0), pt(12.0, 3.0), pt(16.0, 3.0));
            p.line_to(pt(22.0, 3.0));
        }
        BandType::LowCut => {
            p.move_to(pt(2.0, 12.0));
            p.curve_to(pt(5.0, 4.0), pt(7.0, 3.0), pt(12.0, 3.0));
            p.line_to(pt(22.0, 3.0));
        }
        BandType::HighCut => {
            p.move_to(pt(0.0, 3.0));
            p.line_to(pt(10.0, 3.0));
            p.curve_to(pt(15.0, 3.0), pt(17.0, 4.0), pt(20.0, 12.0));
        }
        BandType::Notch => {
            p.move_to(pt(0.0, 3.0));
            p.line_to(pt(7.0, 3.0));
            p.curve_to(pt(10.0, 3.0), pt(10.5, 12.0), pt(11.0, 12.0));
            p.curve_to(pt(11.5, 12.0), pt(12.0, 3.0), pt(15.0, 3.0));
            p.line_to(pt(22.0, 3.0));
        }
        BandType::BandPass => {
            p.move_to(pt(1.0, 12.0));
            p.curve_to(pt(6.0, 12.0), pt(8.0, 2.0), pt(11.0, 2.0));
            p.curve_to(pt(14.0, 2.0), pt(16.0, 12.0), pt(21.0, 12.0));
        }
        BandType::TiltShelf => {
            p.move_to(pt(0.0, 10.0));
            p.curve_to(pt(9.0, 10.0), pt(13.0, 2.0), pt(22.0, 2.0));
        }
        BandType::FlatTilt => {
            p.move_to(pt(0.0, 11.0));
            p.line_to(pt(22.0, 1.0));
        }
        BandType::AllPass => {
            // A flat level, with the phase turning: a circular arrow over it.
            p.move_to(pt(0.0, 11.0));
            p.line_to(pt(22.0, 11.0));
            let (c, r, start, sweep) = (Point::new(at.x + 11.0, at.y + 4.5), 4.0, 0.5f64, 4.6f64);
            p.move_to(c + (r * start.cos(), r * start.sin()));
            p.extend(kurbo::Arc::new(c, (r, r), start, sweep, 0.0).append_iter(0.05));
            // The arrowhead follows the arc's direction at its end.
            let e = start + sweep;
            let tip = c + (r * e.cos(), r * e.sin());
            let dir = Point::new(-e.sin(), e.cos()).to_vec2();
            let back = |turn: f64| {
                let (sn, cs) = turn.sin_cos();
                tip - kurbo::Vec2::new(dir.x * cs - dir.y * sn, dir.x * sn + dir.y * cs) * 2.8
            };
            p.move_to(back(0.6));
            p.line_to(tip);
            p.line_to(back(-0.6));
        }
    }
    let stroke = Stroke::new(1.6).with_caps(kurbo::Cap::Round).with_join(kurbo::Join::Round);
    scene.stroke(&stroke, Affine::IDENTITY, color, None, &p);
}

/// Draws a cut's steepness for slope choice `i` of `n` as a 22 × 12 icon: steeper is more upright.
pub fn draw_slope_icon(scene: &mut Scene, kind: BandType, i: usize, n: usize, at: Point, color: Color) {
    let t = i as f64 / (n.max(2) - 1) as f64;
    let run = 12.0 - 11.0 * t.sqrt();
    // Drawn as a low cut; a high cut is its mirror image.
    let mirror = kind == BandType::HighCut;
    let pt = |x: f64, y: f64| (at.x + if mirror { 22.0 - x } else { x }, at.y + y);
    let knee = 13.0;
    let mut p = BezPath::new();
    p.move_to(pt(knee - run, 12.0));
    p.quad_to(pt(knee - 0.1 * run, 3.0), pt(knee + 2.0, 3.0));
    p.line_to(pt(22.0, 3.0));
    let stroke = Stroke::new(1.6).with_caps(kurbo::Cap::Round).with_join(kurbo::Join::Round);
    scene.stroke(&stroke, Affine::IDENTITY, color, None, &p);
}

