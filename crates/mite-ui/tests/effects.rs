//! Bloom and glass, measured on rendered pixels. Skipped (passing) where no GPU is available.

use mite_ui::kurbo::{Affine, Line, Rect, RoundedRect};
use mite_ui::paint::glow_stroke;
use mite_ui::peniko::{Color, Fill};
use mite_ui::render::{render_view, Image};
use mite_ui::{DrawCx, MemoryParamHost, ParamHost, Theme, UiEvent, View};

const W: f64 = 200.0;
const H: f64 = 100.0;

/// Draws `paint` on a 200×100 canvas.
struct Canvas<F: FnMut(&mut DrawCx<'_>) + Send + 'static>(F);

impl<F: FnMut(&mut DrawCx<'_>) + Send + 'static> View for Canvas<F> {
    fn logical_size(&self) -> (f64, f64) {
        (W, H)
    }

    fn draw(&mut self, cx: &mut DrawCx<'_>) {
        (self.0)(cx);
    }

    fn event(&mut self, _: &UiEvent, _: &dyn ParamHost) -> bool {
        false
    }
}

fn render(paint: impl FnMut(&mut DrawCx<'_>) + Send + 'static) -> Option<Image> {
    match render_view(&mut Canvas(paint), &MemoryParamHost::new(), &Theme::dark("eq"), 1.0) {
        Ok(image) => Some(image),
        Err(e) if e.contains("GPU") => {
            eprintln!("skipped: {e}");
            None
        }
        Err(e) => panic!("{e}"),
    }
}

/// Mean of the RGB channels at (x, y), 0–255.
fn level(image: &Image, x: u32, y: u32) -> f64 {
    let i = ((y * image.width + x) * 4) as usize;
    image.rgba[i..i + 3].iter().map(|&c| f64::from(c)).sum::<f64>() / 3.0
}

fn white_line(cx: &mut DrawCx<'_>, glow: bool) {
    let line = Line::new((50.0, 10.0), (50.0, 90.0));
    let white = Color::from_rgba8(255, 255, 255, 255);
    if glow {
        glow_stroke(cx, &line, 2.0, white, white, 4.0);
    } else {
        cx.scene.stroke(&mite_ui::kurbo::Stroke::new(2.0), Affine::IDENTITY, white, None, &line);
    }
}

#[test]
fn glow_spreads_a_halo_that_fades_out() {
    let (Some(lit), Some(plain)) = (render(|cx| white_line(cx, true)), render(|cx| white_line(cx, false))) else { return };
    let background = level(&plain, 150, 50);
    assert!((level(&plain, 56, 50) - background).abs() < 1.0, "no halo without glow");
    let near = level(&lit, 54, 50) - background;
    let mid = level(&lit, 60, 50) - background;
    let far = level(&lit, 90, 50) - background;
    assert!(near > 25.0, "halo next to the line: {near}");
    assert!(mid > 4.0 && mid < near, "halo falls off: {near} then {mid}");
    assert!(far < 1.5, "gone 40 px away: {far}");
    // The line itself stays white, and the far side of the canvas is untouched.
    assert!(level(&lit, 50, 50) > 250.0);
    assert!((level(&lit, 150, 50) - background).abs() < 1.0);
}

/// Black left half, white right half, a sharp edge at x = 100.
fn halves(cx: &mut DrawCx<'_>) {
    cx.scene.fill(Fill::NonZero, Affine::IDENTITY, Color::from_rgba8(0, 0, 0, 255), None, &Rect::new(0.0, 0.0, 100.0, H));
    cx.scene.fill(Fill::NonZero, Affine::IDENTITY, Color::from_rgba8(255, 255, 255, 255), None, &Rect::new(100.0, 0.0, W, H));
}

#[test]
fn glass_frosts_only_what_lies_under_it() {
    let Some(image) = render(|cx| {
        halves(cx);
        assert!(cx.glass(RoundedRect::new(40.0, 30.0, 160.0, 90.0, 10.0), 18.0));
        // Drawn above the glass: stays sharp.
        cx.scene.fill(Fill::NonZero, Affine::IDENTITY, Color::from_rgba8(255, 0, 0, 255), None, &Rect::new(60.0, 50.0, 70.0, 60.0));
    }) else {
        return;
    };
    // Outside the panel the edge is sharp.
    assert!(level(&image, 97, 10) < 2.0 && level(&image, 102, 10) > 253.0);
    // Inside, it is blurred across tens of pixels: both sides move towards grey (in linear
    // light, so the white side gives less).
    let (left, right) = (level(&image, 92, 70), level(&image, 108, 70));
    assert!(left > 40.0 && right < 235.0, "blurred edge: {left} | {right}");
    assert!(level(&image, 70, 70) < left && level(&image, 130, 70) > right, "monotone across the edge");
    // The red square drawn after the glass is crisp and unblurred.
    let i = ((55 * image.width + 65) * 4) as usize;
    assert_eq!(&image.rgba[i..i + 3], &[255, 0, 0]);
    let j = ((55 * image.width + 72) * 4) as usize;
    assert!(image.rgba[j] < 120, "no red bleeding past the square");
}

#[test]
fn a_frame_without_effects_is_unchanged_by_the_compositor() {
    // One layer, nothing glowing: rendered straight into the frame, bit-exact with itself and
    // with the halves it draws.
    let Some(image) = render(halves) else { return };
    assert!(level(&image, 99, 50) < 1.0 && level(&image, 100, 50) > 254.0);
}
