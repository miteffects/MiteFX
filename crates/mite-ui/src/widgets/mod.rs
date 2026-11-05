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
