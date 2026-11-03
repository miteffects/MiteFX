//! The contract between a plugin's editor and the window.

use vello::kurbo::{Point, RoundedRect};
use vello::Scene;

use crate::effects::{Effects, Glass};
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
    Backspace,
    Enter,
    Up,
    Down,
    Left,
    Right,
    PageUp,
    PageDown,
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
    /// Glow sources and glass layers (see [`DrawCx::glow`], [`DrawCx::glass`]).
    pub fx: &'a mut Effects,
}

impl DrawCx<'_> {
    /// What is drawn here (logical px, like the scene) glows: it is blurred and laid over the
    /// layer. Draw the shape itself into the scene as well.
    pub fn glow(&mut self) -> &mut Scene {
        &mut self.fx.glow
    }

    /// Masks the glow layer out of `hole` until [`Self::end_glow_hole`]: for what a panel will
    /// cover. The glow layer is composited over the whole scene, so a glowing line under an
    /// opaque panel would otherwise shine through it as a blurred haze (user, 2026-11-03).
    pub fn begin_glow_hole(&mut self, hole: vello::kurbo::RoundedRect) {
        let mut mask = vello::kurbo::BezPath::new();
        let (w, h) = self.size;
        mask.extend(vello::kurbo::Shape::path_elements(&vello::kurbo::Rect::new(-10.0, -10.0, w + 10.0, h + 10.0), 0.1));
        mask.extend(vello::kurbo::Shape::path_elements(&hole, 0.1));
        self.fx.glow.push_clip_layer(vello::peniko::Fill::EvenOdd, vello::kurbo::Affine::IDENTITY, &mask);
    }

    pub fn end_glow_hole(&mut self) {
        self.fx.glow.pop_layer();
    }

    /// Call right before drawing a glass panel: everything drawn so far is blurred (`blur` is
    /// the σ in logical px, `blur.*` in tokens.json) into `shape`, and drawing goes on above it.
    /// Returns `false` when the theme has no blur (high contrast): draw the panel opaque.
    pub fn glass(&mut self, shape: RoundedRect, blur: f64) -> bool {
        if !self.theme.shadows {
            return false;
        }
        self.fx.split(self.scene, Some(Glass { shape, blur }));
        true
    }
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
