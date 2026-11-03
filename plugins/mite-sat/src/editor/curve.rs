//! The transfer square: input −1…1 across, output −1…1 up, the band's curve as the DSP plays it
//! (`mite_dsp::sat::transfer` at the live operating point, times auto gain), lit where the music
//! is, with a dot riding the curve at the band's peak either way. Tape and Transformer: the faint
//! curve is their core's anhysteretic curve (what the loop closes onto), and the lit part is the
//! loop the band's music makes (`mite_dsp::sat::magnet_loop`: a sine at the band's peak through
//! its real core and emphasis, at the band's centre frequency). A full-scale loop is not drawn:
//! that deep in saturation the emphasis turns the harmonics against the fundamental and the trace
//! twists into a figure of eight, true but unreadable.

use std::cell::RefCell;
use std::sync::Arc;

use mite_dsp::sat::{engagement, magnet_loop, transfer, Model};
use mite_ui::kurbo::{Affine, BezPath, Circle, Line, Point, Rect, RoundedRect, Stroke};
use mite_ui::paint::{glow_fill, glow_stroke, panel};
use mite_ui::peniko::Fill;
use mite_ui::DrawCx;

/// Inner padding of the square, px.
const PAD: f64 = 10.0;
/// Curve samples across.
const SAMPLES: usize = 241;
/// Loop points over a cycle.
const LOOP_POINTS: usize = 144;

/// What the square draws.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Curve {
    pub model: Model,
    pub drive_db: f32,
    /// The operating point the engine reports (it follows the music's envelope).
    pub point: f32,
    /// The band's bias, −1…1 (a traced loop works its own point out from it).
    pub bias: f32,
    /// Auto gain's correction, linear (1 when off).
    pub auto: f32,
    /// The dynamics' gain now, linear (the curve breathes with it).
    pub dynamics: f32,
    /// Where the dynamics start, as an input peak (linear), while they act.
    pub threshold: Option<f32>,
    /// The band's peak going in, linear (where the music reaches on the curve).
    pub peak: f32,
    /// The band's centre, Hz (where a loop is traced: Tape's and Transformer's emphasis make it
    /// depend on the frequency).
    pub freq: f32,
}

impl Curve {
    pub fn at(&self, x: f32) -> f32 {
        transfer(self.model, x, self.drive_db, self.point) * self.auto * self.dynamics
    }

    /// Tape and Transformer: the loop for a sine of amplitude `amp`, as the band plays it
    /// (`x + a·(F − x)` times auto gain).
    fn trace(&self, amp: f32, cache: &Loops) -> Option<Arc<Vec<(f32, f32)>>> {
        self.model.magnet()?;
        let raw = cache.get(self, amp);
        let a = engagement(self.drive_db);
        let gain = self.auto * self.dynamics;
        Some(Arc::new(raw.iter().map(|&(x, f)| (x, (x + a * (f - x)) * gain)).collect()))
    }
}

/// Loops traced lately, by everything they depend on (a trace runs the hysteresis core for two
/// cycles; most frames draw the same full-scale loops again).
#[derive(Default)]
pub struct Loops(RefCell<Vec<(LoopKey, Trace)>>);

/// Model, drive, bias, frequency and amplitude (bits); the traced points.
type LoopKey = [u32; 5];
type Trace = Arc<Vec<(f32, f32)>>;

impl Loops {
    const KEEP: usize = 24;

    fn get(&self, c: &Curve, amp: f32) -> Arc<Vec<(f32, f32)>> {
        let key = [c.model.index() as u32, c.drive_db.to_bits(), c.bias.to_bits(), c.freq.to_bits(), amp.to_bits()];
        let mut list = self.0.borrow_mut();
        if let Some(i) = list.iter().position(|(k, _)| *k == key) {
            let hit = list.remove(i);
            let v = hit.1.clone();
            list.push(hit);
            return v;
        }
        let v = Arc::new(magnet_loop(c.model, amp, c.drive_db, c.bias, c.freq, LOOP_POINTS));
        if list.len() == Self::KEEP {
            list.remove(0);
        }
        list.push((key, v.clone()));
        v
    }
}

pub fn to_screen(r: Rect, x: f64, y: f64) -> Point {
    let w = r.width() - 2.0 * PAD;
    let h = r.height() - 2.0 * PAD;
    Point::new(r.x0 + PAD + (x + 1.0) / 2.0 * w, r.y1 - PAD - (y + 1.0) / 2.0 * h)
}

fn path(r: Rect, f: impl Fn(f32) -> f32, from: f32, to: f32) -> BezPath {
    let mut p = BezPath::new();
    let n = ((SAMPLES as f32 * (to - from) / 2.0).ceil() as usize).max(2);
    for i in 0..=n {
        let x = from + (to - from) * i as f32 / n as f32;
        let q = to_screen(r, f64::from(x), f64::from(f(x)));
        if i == 0 {
            p.move_to(q);
        } else {
            p.line_to(q);
        }
    }
    p
}

/// A traced loop as a closed path (its points are dense samples of a smooth cycle).
fn loop_path(r: Rect, pts: &[(f32, f32)]) -> BezPath {
    let mut p = BezPath::new();
    for (i, &(x, y)) in pts.iter().enumerate() {
        let q = to_screen(r, f64::from(x), f64::from(y));
        if i == 0 {
            p.move_to(q);
        } else {
            p.line_to(q);
        }
    }
    p.close_path();
    p
}

/// Draws the square in `r`. `ghost`: another model's curve at the same settings (hovering a
/// model icon). `big`: the selected card's square (grid and dots).
pub fn draw(cx: &mut DrawCx<'_>, r: Rect, curve: &Curve, ghost: Option<Model>, big: bool, alpha: f32, loops: &Loops) {
    let theme = cx.theme;
    let frame = RoundedRect::from_rect(r, 10.0);
    panel(cx.scene, frame, theme.well, theme.well_border);
    cx.begin_clip(&frame);
    let grid = |cx: &mut DrawCx<'_>, a: Point, b: Point, strong: bool| {
        cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, if strong { theme.grid_major } else { theme.grid_minor }, None, &Line::new(a, b));
    };
    if big {
        for v in [-0.5, 0.5] {
            grid(cx, to_screen(r, v, -1.2), to_screen(r, v, 1.2), false);
            grid(cx, to_screen(r, -1.2, v), to_screen(r, 1.2, v), false);
        }
    }
    grid(cx, to_screen(r, 0.0, -1.2), to_screen(r, 0.0, 1.2), true);
    grid(cx, to_screen(r, -1.2, 0.0), to_screen(r, 1.2, 0.0), true);
    // The dynamics' threshold: where the music's peak starts to move them.
    if let Some(t) = curve.threshold {
        let t = f64::from(t.min(1.0));
        let dash = Stroke::new(1.0).with_dashes(0.0, [3.0, 3.0]);
        for x in [-t, t] {
            cx.scene.stroke(&dash, Affine::IDENTITY, theme.accent.multiply_alpha(0.45 * alpha), None, &Line::new(to_screen(r, x, -1.2), to_screen(r, x, 1.2)));
        }
    }
    // The identity: what the band was.
    cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, theme.text_lo.multiply_alpha(0.35 * alpha), None, &Line::new(to_screen(r, -1.0, -1.0), to_screen(r, 1.0, 1.0)));
    let whole_of = |c: &Curve| path(r, |x| c.at(x), -1.0, 1.0);
    if let Some(m) = ghost {
        let g = Curve { model: m, ..*curve };
        cx.scene.stroke(&Stroke::new(1.5), Affine::IDENTITY, theme.accent.multiply_alpha(0.4 * alpha), None, &whole_of(&g));
    }
    cx.scene.stroke(&Stroke::new(if big { 1.6 } else { 1.3 }), Affine::IDENTITY, theme.accent.multiply_alpha(0.55 * alpha), None, &whole_of(curve));
    // Lit where the music is.
    let p = curve.peak.min(1.0);
    if p > 1e-3 {
        let (lit, ends) = match curve.trace(p, loops) {
            // A loop's ends: where the sine peaks (a quarter and three quarters round).
            Some(pts) => (loop_path(r, &pts), [pts[LOOP_POINTS / 4], pts[3 * LOOP_POINTS / 4]]),
            None => (path(r, |x| curve.at(x), -p, p), [(-p, curve.at(-p)), (p, curve.at(p))]),
        };
        glow_stroke(cx, &lit, if big { 2.6 } else { 2.0 }, theme.accent.multiply_alpha(alpha), theme.accent, 6.0);
        if big {
            for (x, y) in ends {
                let c = to_screen(r, f64::from(x), f64::from(y));
                glow_fill(cx, &Circle::new(c, 4.5), theme.accent.multiply_alpha(alpha), 5.0);
                cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.needle.multiply_alpha(alpha), None, &Circle::new(c, 2.0));
            }
        }
    }
    cx.end_clip();
}
