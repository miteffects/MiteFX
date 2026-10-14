//! MiteFX UI kit.
//!
//! Everything is drawn as vector paths with Vello on wgpu, inside a baseview window. This crate
//! knows nothing about plugin formats: plugins hand it a [`View`] and a [`ParamHost`], and
//! `mite-plugin` connects the host to the plugin framework.
//!
//! All drawing happens in logical pixels; the window applies the DPI scale.

pub mod host;
pub mod paint;
#[cfg(target_os = "linux")]
mod host_sync;
mod present;
pub mod render;
pub mod text;
pub mod tokens;
pub mod view;
pub mod widgets;
pub mod window;

pub use host::{MemoryParamHost, ParamHost};
pub use text::{Align, TextStyle, Typesetter};
pub use tokens::{Theme, ThemeKind};
pub use view::{DrawCx, Modifiers, MouseButton, UiEvent, View};
pub use window::{open_parented, run_standalone, ResizeRequest, WindowOptions, MAX_ZOOM, MIN_ZOOM};

pub use vello;
pub use vello::kurbo;
pub use vello::peniko;
