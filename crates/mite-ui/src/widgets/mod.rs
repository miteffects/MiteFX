//! Widgets from the component sheet (`docs/design/mockups/components.dc.html`).

pub mod brand;
pub mod dropdown;
pub mod eq_icons;
pub mod footer;
pub mod help;
pub mod history;
pub mod key_eq;
pub mod knob;
pub mod matrix;
pub mod meter;
pub mod rail;
pub mod range;

pub use dropdown::{Dropdown, DropdownEvent};
pub use footer::{Footer, FooterDial, FooterToggle, ToggleGlyph};
pub use help::Help;
pub use history::HistoryLane;
pub use key_eq::KeyEq;
pub use knob::{Knob, KnobKind};
pub use matrix::{active_routes, draw_matrix_button, knob_modulation, MatrixConfig, MatrixHot, MatrixPanel};
pub use meter::LevelMeter;
pub use rail::{BandMenu, MenuAsk, Rail, RailHot, RailParams};
pub use range::{ease_range, RangePicker};

/// Focus mode's switch glyph, centred on `c`: four corner brackets around an open middle (the
/// display taking the whole window).
pub fn draw_focus_glyph(scene: &mut vello::Scene, c: vello::kurbo::Point, color: vello::peniko::Color) {
    use vello::kurbo::{Affine, BezPath, Cap, Join, Stroke};
    let mut p = BezPath::new();
    for (sx, sy) in [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
        p.move_to((c.x + sx * 8.0, c.y + sy * 3.5));
        p.line_to((c.x + sx * 8.0, c.y + sy * 8.0));
        p.line_to((c.x + sx * 3.5, c.y + sy * 8.0));
    }
    let stroke = Stroke::new(1.6).with_caps(Cap::Round).with_join(Join::Round);
    scene.stroke(&stroke, Affine::IDENTITY, color, None, &p);
}
