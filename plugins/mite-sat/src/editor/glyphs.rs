//! Glyphs (24×24, stroked) and the model icons: a pictogram of what each model is (the card's
//! transfer square draws its curve; the icons all looked alike as tiny curves).

use mite_dsp::sat::Model;
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

/// Tape: a cassette, its two reels and the tape window.
const TAPE: &str = "M4 5 H20 A2 2 0 0 1 22 7 V17 A2 2 0 0 1 20 19 H4 A2 2 0 0 1 2 17 V7 A2 2 0 0 1 4 5 Z \
    M10.6 11 A2.6 2.6 0 1 0 5.4 11 A2.6 2.6 0 1 0 10.6 11 Z M18.6 11 A2.6 2.6 0 1 0 13.4 11 A2.6 2.6 0 1 0 18.6 11 Z \
    M10.6 11 H13.4 M7 19 L8.5 15.5 H15.5 L17 19";
/// Tube: a valve, its glass dome, plates and filament, the base and its pins.
const TUBE: &str = "M7 17 V9 A5 5 0 0 1 17 9 V17 M5.5 17 H18.5 V20 H5.5 Z M9 20 V22.5 M12 20 V22.5 M15 20 V22.5 \
    M9.5 7.5 V14.5 M14.5 7.5 V14.5 M11 14.5 L12 11 L13 14.5";
/// Transformer: two coils on an iron core (the schematic symbol).
const TRANSFORMER: &str = "M7 3 V5 A1.75 1.75 0 0 1 7 8.5 A1.75 1.75 0 0 1 7 12 A1.75 1.75 0 0 1 7 15.5 A1.75 1.75 0 0 1 7 19 V21 \
    M17 3 V5 A1.75 1.75 0 0 0 17 8.5 A1.75 1.75 0 0 0 17 12 A1.75 1.75 0 0 0 17 15.5 A1.75 1.75 0 0 0 17 19 V21 \
    M11 4 V20 M13 4 V20";
/// Diode: the schematic symbol, its triangle against the bar.
const DIODE: &str = "M2 12 H8 M8 6.5 V17.5 L16 12 Z M16 6.5 V17.5 M16 12 H22";
/// Soft: a pillow.
const SOFT: &str = "M5 5 Q12 8 19 5 Q16 12 19 19 Q12 16 5 19 Q8 12 5 5 Z";
/// Hard: a brick wall, the ceiling nothing passes.
const HARD: &str = "M3 5.5 H21 V18.5 H3 Z M3 9.8 H21 M3 14.2 H21 M9 5.5 V9.8 M15 5.5 V9.8 M6 9.8 V14.2 M12 9.8 V14.2 \
    M18 9.8 V14.2 M9 14.2 V18.5 M15 14.2 V18.5";
/// Fold: a sheet folded in pleats.
const FOLD: &str = "M3 8 L7.5 5 L12 8 L16.5 5 L21 8 M3 18 L7.5 15 L12 18 L16.5 15 L21 18 \
    M3 8 V18 M7.5 5 V15 M12 8 V18 M16.5 5 V15 M21 8 V18";
/// Harmonic: a harmonic series, bars falling away, a pen's mark on the second (drawn).
const HARMONIC: &str = "M3 21 H21 M5 21 V5 M9 21 V10 M13 21 V14 M17 21 V17 M3 5 H7";

/// Rectify: a bridge rectifier, four diodes round a diamond.
const RECTIFY: &str = "M12 3 L21 12 L12 21 L3 12 Z M12 1 V3 M12 21 V23 M1 12 H3 M21 12 H23 M7.71 9.97 L8.63 6.37 L5.03 7.29 Z M9.97 7.71 L7.29 5.03 M18.97 7.29 L15.37 6.37 L16.29 9.97 Z M16.71 5.03 L14.03 7.71 M9.97 16.29 L6.37 15.37 L7.29 18.97 Z M7.71 14.03 L5.03 16.71 M16.71 18.97 L17.63 15.37 L14.03 16.29 Z M18.97 16.71 L16.29 14.03";

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
    // Left or right alone: a dot in that circle.
    if mode == 4 {
        dot(scene, c.x - dx - r * 0.3);
    }
    if mode == 5 {
        dot(scene, c.x + dx + r * 0.3);
    }
}

/// A model's icon, `size` across, centred on `c`.
pub fn draw_model(scene: &mut Scene, model: Model, c: Point, size: f64, color: Color, width: f64) {
    let d = match model {
        Model::Tape => TAPE,
        Model::Tube => TUBE,
        Model::Transformer => TRANSFORMER,
        Model::Diode => DIODE,
        Model::Soft => SOFT,
        Model::Hard => HARD,
        Model::Fold => FOLD,
        Model::Rectify => RECTIFY,
        Model::Harmonic => HARMONIC,
    };
    draw(scene, d, c, size, color, width);
}
