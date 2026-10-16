//! The contract between a plugin's editor and the window.

use vello::kurbo::Point;
use vello::Scene;

use crate::host::ParamHost;
use crate::text::Typesetter;
use crate::tokens::Theme;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Modifiers {
    pub shift: bool,
    pub alt: bool,
    pub ctrl: bool,
    pub meta: bool,
}

impl Modifiers {
    /// The design's fine-adjust modifier (Shift, `settings.fineFactor`).
    pub fn fine(&self) -> bool {
        self.shift
    }
}

/// Input, already converted to logical pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum UiEvent {
    PointerMove { pos: Point, mods: Modifiers },
    PointerDown { pos: Point, button: MouseButton, mods: Modifiers },
    PointerUp { pos: Point, button: MouseButton, mods: Modifiers },
    /// Synthesised by the window: two left presses within 400 ms and 4 px.
    DoubleClick { pos: Point, mods: Modifiers },
    /// Positive `lines` = wheel up / away from the user.
    Wheel { pos: Point, lines: f32, mods: Modifiers },
    PointerLeft,
    /// A key pressed while the editor has focus. A view that does not use it returns `false`
    /// from [`View::event`], and the key goes on to the host (transport shortcuts keep working).
    KeyDown { key: Key, mods: Modifiers },
}

/// The keys views listen for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Escape,
    Delete,
    Character(char),
}

/// Everything a view needs to paint one frame.
pub struct DrawCx<'a> {
    pub scene: &'a mut Scene,
    pub theme: &'a Theme,
    pub text: &'a mut Typesetter,
    pub host: &'a dyn ParamHost,
    /// Logical size of the window.
    pub size: (f64, f64),
}

/// A plugin editor's root. Drawn every frame the window asks for.
pub trait View: Send + 'static {
    /// Size the editor is laid out in (design units). The window is this × [`View::zoom`].
    fn logical_size(&self) -> (f64, f64);

    /// Display zoom applied on top of the platform's DPI scale (UI scale setting). Views draw
    /// and receive events in design units regardless.
    fn zoom(&self) -> f64 {
        1.0
    }

    fn draw(&mut self, cx: &mut DrawCx<'_>);

    /// Returns `true` if the event changed something that needs a redraw.
    fn event(&mut self, event: &UiEvent, host: &dyn ParamHost) -> bool;

    /// Called on the UI timer (~60 Hz). Return `true` to redraw, e.g. when meters moved or the
    /// host changed a parameter (automation, preset load).
    fn tick(&mut self, host: &dyn ParamHost) -> bool {
        let _ = host;
        false
    }
}
