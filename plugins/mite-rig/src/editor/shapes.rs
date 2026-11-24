//! What each block looks like on the board: abstract, but itself. Pedals are stompboxes (knobs,
//! a footswitch, the effect's sign on the face), an amp is a head with its handle, controls and
//! grille, a combo or full rig has its speaker under the controls, a cabinet shows its four
//! speakers, a volume pedal is a rocker seen from the side. Straight edges and cut corners, never
//! a rounded box.

use mite_ui::kurbo::{Affine, BezPath, Cap, Circle, Join, Line, Point, Rect, Stroke};
use mite_ui::peniko::{Color, Fill};
use mite_ui::vello::Scene;
use mite_ui::DrawCx;

use crate::blocks::Kind;
use crate::rig::Gear;

/// How a block is drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Look {
    Stomp(Sign),
    Head,
    Combo,
    Cabinet,
    Rocker,
    /// A wah: a rocker with its toe switch and the sweep on its side.
    Wah,
}

/// A stompbox's sign.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sign {
    Drive,
    Gate,
    Tone,
    Boost,
    Comp,
    Eq,
    Delay,
    Reverb,
    Mod,
    Tremolo,
    Double,
    Tuner,
    Octave,
}

impl Look {
    pub fn of(kind: Kind, gear: Gear) -> Self {
        match kind {
            Kind::Pedal => Look::Stomp(Sign::Drive),
            Kind::Amp if gear == Gear::FullRig => Look::Combo,
            Kind::Amp => Look::Head,
            Kind::Cab => Look::Cabinet,
            Kind::Gate => Look::Stomp(Sign::Gate),
            Kind::Tone => Look::Stomp(Sign::Tone),
            Kind::Boost => Look::Stomp(Sign::Boost),
            Kind::Volume => Look::Rocker,
            Kind::Wah => Look::Wah,
            Kind::Octaver => Look::Stomp(Sign::Octave),
            Kind::Comp => Look::Stomp(Sign::Comp),
            Kind::Eq => Look::Stomp(Sign::Eq),
            Kind::Delay => Look::Stomp(Sign::Delay),
            Kind::Reverb => Look::Stomp(Sign::Reverb),
            Kind::Mod => Look::Stomp(Sign::Mod),
            Kind::Tremolo => Look::Stomp(Sign::Tremolo),
            Kind::Double => Look::Stomp(Sign::Double),
            Kind::Tuner => Look::Stomp(Sign::Tuner),
            // A host's plugin (Miteboard): drawn as a generic stompbox here.
            Kind::Plugin => Look::Stomp(Sign::Tone),
        }
    }
}

/// Where a look carries its on/off light: on a stompbox's face by its footswitch, on an amp's
/// control strip, on a cabinet's frame, on a rocker's base.
pub fn led(look: Look, area: Rect) -> Point {
    let c = area.center();
    match look {
        Look::Stomp(_) => Point::new(c.x + 18.0, area.y1 - 11.0),
        Look::Head => Point::new(area.x1 - 13.0, area.y0 + 16.0 + 7.0),
        Look::Combo => Point::new(c.x + 36.0, area.y0 + 8.0 + 6.5),
        Look::Cabinet => Point::new(c.x, c.y),
        Look::Rocker | Look::Wah => Point::new(c.x + 26.0, area.y1 - 6.0),
    }
}

/// A rectangle with its corners cut (a chamfer of `c`).
pub fn chamfer(r: Rect, c: f64) -> BezPath {
    let mut p = BezPath::new();
    p.move_to((r.x0 + c, r.y0));
    p.line_to((r.x1 - c, r.y0));
    p.line_to((r.x1, r.y0 + c));
    p.line_to((r.x1, r.y1 - c));
    p.line_to((r.x1 - c, r.y1));
    p.line_to((r.x0 + c, r.y1));
    p.line_to((r.x0, r.y1 - c));
    p.line_to((r.x0, r.y0 + c));
    p.close_path();
    p
}

/// The colour a kind wears (the suite plugin it comes from, or one near its family): the
/// `color.accent` key in the tokens.
pub fn accent_key(kind: Kind) -> &'static str {
    match kind {
        Kind::Pedal => "sat",
        Kind::Amp | Kind::Cab => "rig",
        Kind::Gate => "gate",
        Kind::Comp => "comp",
        Kind::Boost => "limit",
        Kind::Tone => "filter",
        Kind::Eq => "eq",
        Kind::Volume => "utility",
        Kind::Wah => "soothe",
        Kind::Octaver => "pitch",
        Kind::Mod => "mod",
        Kind::Tremolo => "shape",
        Kind::Double => "double",
        Kind::Delay => "delay",
        Kind::Reverb => "verb",
        Kind::Tuner => "tuner",
        Kind::Plugin => "utility",
    }
}

/// Colours for one drawing.
pub struct Ink {
    /// Laid over the body (the kind's colour, faint).
    pub tint: Color,
    pub body: Color,
    pub edge: Color,
    pub detail: Color,
    pub lit: Color,
}

fn stroke(cx: &mut DrawCx<'_>, path: &impl mite_ui::kurbo::Shape, w: f64, c: Color) {
    cx.scene.stroke(&Stroke::new(w).with_caps(Cap::Round).with_join(Join::Round), Affine::IDENTITY, c, None, path);
}

fn fill(cx: &mut DrawCx<'_>, path: &impl mite_ui::kurbo::Shape, c: Color) {
    cx.scene.fill(Fill::NonZero, Affine::IDENTITY, c, None, path);
}

fn knob(cx: &mut DrawCx<'_>, at: Point, r: f64, ink: &Ink) {
    fill(cx, &Circle::new(at, r), ink.detail.multiply_alpha(0.35));
    stroke(cx, &Circle::new(at, r), 1.0, ink.detail);
    stroke(cx, &Line::new(at, (at.x, at.y - r * 0.8)), 1.2, ink.lit);
}

fn sign(cx: &mut DrawCx<'_>, s: Sign, c: Point, ink: &Ink) {
    let k = 1.0;
    let mut p = BezPath::new();
    match s {
        // A clipped wave.
        Sign::Drive => {
            p.move_to((c.x - 11.0 * k, c.y + 4.0));
            p.line_to((c.x - 7.0, c.y - 4.0));
            p.line_to((c.x - 2.0, c.y - 4.0));
            p.line_to((c.x + 2.0, c.y + 4.0));
            p.line_to((c.x + 7.0, c.y + 4.0));
            p.line_to((c.x + 11.0, c.y - 4.0));
        }
        // A gate: two posts and a bar.
        Sign::Gate => {
            p.move_to((c.x - 11.0, c.y + 5.0));
            p.line_to((c.x - 6.0, c.y + 5.0));
            p.line_to((c.x - 6.0, c.y - 5.0));
            p.line_to((c.x + 6.0, c.y - 5.0));
            p.line_to((c.x + 6.0, c.y + 5.0));
            p.line_to((c.x + 11.0, c.y + 5.0));
        }
        // Three sliders.
        Sign::Tone => {
            for (dx, h) in [(-7.0, 3.0), (0.0, -3.0), (7.0, 1.0)] {
                p.move_to((c.x + dx, c.y - 6.0));
                p.line_to((c.x + dx, c.y + 6.0));
                p.move_to((c.x + dx - 2.5, c.y + h));
                p.line_to((c.x + dx + 2.5, c.y + h));
            }
        }
        // An arrow up.
        Sign::Boost => {
            p.move_to((c.x, c.y + 6.0));
            p.line_to((c.x, c.y - 6.0));
            p.move_to((c.x - 5.0, c.y - 1.0));
            p.line_to((c.x, c.y - 6.0));
            p.line_to((c.x + 5.0, c.y - 1.0));
        }
        // A curve bending over a knee.
        Sign::Comp => {
            p.move_to((c.x - 10.0, c.y + 6.0));
            p.line_to((c.x - 1.0, c.y - 2.0));
            p.line_to((c.x + 10.0, c.y - 5.0));
        }
        // Seven slider caps at different heights.
        Sign::Eq => {
            for (k, h) in [2.0, -1.0, -4.0, 0.0, 3.0, -2.0, 1.0].into_iter().enumerate() {
                let x = c.x - 9.0 + k as f64 * 3.0;
                p.move_to((x, c.y + 6.0));
                p.line_to((x, c.y + h));
            }
        }
        // Echoes, each fainter: dots in a row.
        Sign::Delay => {
            for (k, r) in [2.6, 2.0, 1.5, 1.0].into_iter().enumerate() {
                let x = c.x - 9.0 + k as f64 * 6.0;
                p.extend(mite_ui::kurbo::Shape::path_elements(&Circle::new((x, c.y), r), 0.1));
            }
        }
        // A decaying tail.
        Sign::Reverb => {
            p.move_to((c.x - 10.0, c.y - 6.0));
            p.line_to((c.x - 10.0, c.y + 6.0));
            p.quad_to((c.x - 4.0, c.y - 2.0), (c.x + 10.0, c.y + 5.0));
        }
        // A sine.
        Sign::Mod => {
            p.move_to((c.x - 10.0, c.y));
            p.curve_to((c.x - 6.0, c.y - 9.0), (c.x - 2.0, c.y - 9.0), (c.x, c.y));
            p.curve_to((c.x + 2.0, c.y + 9.0), (c.x + 6.0, c.y + 9.0), (c.x + 10.0, c.y));
        }
        // A wave in a swelling envelope.
        Sign::Tremolo => {
            p.move_to((c.x - 10.0, c.y + 5.0));
            p.line_to((c.x - 5.0, c.y - 5.0));
            p.line_to((c.x, c.y + 5.0));
            p.line_to((c.x + 5.0, c.y - 5.0));
            p.line_to((c.x + 10.0, c.y + 5.0));
        }
        // Two lines, side by side.
        Sign::Double => {
            p.move_to((c.x - 10.0, c.y - 2.0));
            p.quad_to((c.x, c.y - 8.0), (c.x + 10.0, c.y - 2.0));
            p.move_to((c.x - 10.0, c.y + 3.0));
            p.quad_to((c.x, c.y - 3.0), (c.x + 10.0, c.y + 3.0));
        }
        // A needle over a scale.
        // A wave and, under it, the same at half the frequency.
        Sign::Octave => {
            p.move_to((c.x - 11.0, c.y - 3.0));
            p.quad_to((c.x - 8.25, c.y - 9.0), (c.x - 5.5, c.y - 3.0));
            p.quad_to((c.x - 2.75, c.y + 3.0), (c.x, c.y - 3.0));
            p.quad_to((c.x + 2.75, c.y - 9.0), (c.x + 5.5, c.y - 3.0));
            p.quad_to((c.x + 8.25, c.y + 3.0), (c.x + 11.0, c.y - 3.0));
            p.move_to((c.x - 11.0, c.y + 5.0));
            p.quad_to((c.x - 5.5, c.y - 1.0), (c.x, c.y + 5.0));
            p.quad_to((c.x + 5.5, c.y + 11.0), (c.x + 11.0, c.y + 5.0));
        }
        Sign::Tuner => {
            p.move_to((c.x - 9.0, c.y + 4.0));
            p.quad_to((c.x, c.y - 4.0), (c.x + 9.0, c.y + 4.0));
            p.move_to((c.x, c.y + 6.0));
            p.line_to((c.x + 3.0, c.y - 6.0));
        }
    }
    stroke(cx, &p, 1.6, ink.lit);
}

/// Draws a block's look inside `area` (its top part on the tile). Returns its outline (for the
/// selection's outline).
pub fn draw(cx: &mut DrawCx<'_>, look: Look, area: Rect, ink: &Ink) -> BezPath {
    let c = area.center();
    match look {
        Look::Stomp(s) => {
            let body = Rect::new(c.x - 30.0, area.y0, c.x + 30.0, area.y1);
            // A stompbox: slightly narrower at the top, as the classics are.
            let mut out = BezPath::new();
            out.move_to((body.x0 + 4.0, body.y0));
            out.line_to((body.x1 - 4.0, body.y0));
            out.line_to((body.x1, body.y0 + 6.0));
            out.line_to((body.x1, body.y1 - 4.0));
            out.line_to((body.x1 - 4.0, body.y1));
            out.line_to((body.x0 + 4.0, body.y1));
            out.line_to((body.x0, body.y1 - 4.0));
            out.line_to((body.x0, body.y0 + 6.0));
            out.close_path();
            fill(cx, &out, ink.body);
            fill(cx, &out, ink.tint);
            stroke(cx, &out, 1.2, ink.edge);
            for dx in [-15.0, 0.0, 15.0] {
                knob(cx, Point::new(c.x + dx, body.y0 + 11.0), 4.2, ink);
            }
            sign(cx, s, Point::new(c.x, body.y0 + 31.0), ink);
            // The footswitch.
            fill(cx, &Circle::new((c.x, body.y1 - 11.0), 6.0), ink.detail.multiply_alpha(0.4));
            stroke(cx, &Circle::new((c.x, body.y1 - 11.0), 6.0), 1.2, ink.detail);
            out
        }
        Look::Head => {
            let body = Rect::new(area.x0 + 4.0, area.y0 + 16.0, area.x1 - 4.0, area.y1);
            let out = chamfer(body, 5.0);
            // The handle.
            let mut h = BezPath::new();
            h.move_to((c.x - 18.0, body.y0));
            h.line_to((c.x - 14.0, area.y0 + 7.0));
            h.line_to((c.x + 14.0, area.y0 + 7.0));
            h.line_to((c.x + 18.0, body.y0));
            stroke(cx, &h, 2.0, ink.edge);
            fill(cx, &out, ink.body);
            fill(cx, &out, ink.tint);
            stroke(cx, &out, 1.2, ink.edge);
            // The control strip, then the grille.
            let strip = body.y0 + 13.0;
            stroke(cx, &Line::new((body.x0 + 4.0, strip), (body.x1 - 4.0, strip)), 1.0, ink.detail.multiply_alpha(0.6));
            for i in 0..6 {
                knob(cx, Point::new(body.x0 + 14.0 + i as f64 * 15.0, body.y0 + 7.0), 3.4, ink);
            }
            let g = Rect::new(body.x0 + 6.0, strip + 4.0, body.x1 - 6.0, body.y1 - 5.0);
            let mut grille = BezPath::new();
            let mut x = g.x0 - g.height();
            while x < g.x1 {
                grille.move_to((x, g.y1));
                grille.line_to((x + g.height(), g.y0));
                x += 7.0;
            }
            cx.begin_clip(&g);
            stroke(cx, &grille, 0.8, ink.detail.multiply_alpha(0.35));
            cx.end_clip();
            out
        }
        Look::Combo => {
            let body = Rect::new(c.x - 44.0, area.y0 + 8.0, c.x + 44.0, area.y1);
            let out = chamfer(body, 5.0);
            let mut h = BezPath::new();
            h.move_to((c.x - 14.0, body.y0));
            h.line_to((c.x - 11.0, area.y0 + 1.0));
            h.line_to((c.x + 11.0, area.y0 + 1.0));
            h.line_to((c.x + 14.0, body.y0));
            stroke(cx, &h, 2.0, ink.edge);
            fill(cx, &out, ink.body);
            fill(cx, &out, ink.tint);
            stroke(cx, &out, 1.2, ink.edge);
            let strip = body.y0 + 12.0;
            stroke(cx, &Line::new((body.x0 + 4.0, strip), (body.x1 - 4.0, strip)), 1.0, ink.detail.multiply_alpha(0.6));
            for i in 0..5 {
                knob(cx, Point::new(body.x0 + 14.0 + i as f64 * 15.0, body.y0 + 6.5), 3.2, ink);
            }
            let sp = Point::new(c.x, (strip + body.y1) / 2.0);
            let r = ((body.y1 - strip) / 2.0 - 3.0).min(20.0);
            stroke(cx, &Circle::new(sp, r), 1.2, ink.detail);
            stroke(cx, &Circle::new(sp, r * 0.45), 1.0, ink.detail.multiply_alpha(0.7));
            fill(cx, &Circle::new(sp, r * 0.18), ink.detail.multiply_alpha(0.6));
            out
        }
        Look::Cabinet => {
            let body = Rect::new(c.x - 32.0, area.y0, c.x + 32.0, area.y1);
            let out = chamfer(body, 5.0);
            fill(cx, &out, ink.body);
            fill(cx, &out, ink.tint);
            stroke(cx, &out, 1.2, ink.edge);
            for (dx, dy) in [(-1.0, -1.0), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)] {
                let p = Point::new(c.x + dx * 15.0, c.y + dy * 14.0);
                stroke(cx, &Circle::new(p, 11.5), 1.1, ink.detail);
                fill(cx, &Circle::new(p, 3.0), ink.detail.multiply_alpha(0.6));
            }
            out
        }
        Look::Rocker | Look::Wah => {
            // A volume pedal from the side: a base, and the rocker plate tilted on it.
            let base = Rect::new(c.x - 34.0, area.y1 - 12.0, c.x + 34.0, area.y1);
            let mut out = BezPath::new();
            out.move_to((base.x0, base.y1));
            out.line_to((base.x1, base.y1));
            out.line_to((base.x1, base.y0));
            out.line_to((base.x1 - 4.0, area.y0 + 26.0));
            out.line_to((base.x0 + 2.0, area.y0 + 10.0));
            out.line_to((base.x0, area.y0 + 14.0));
            out.close_path();
            fill(cx, &out, ink.body);
            fill(cx, &out, ink.tint);
            stroke(cx, &out, 1.2, ink.edge);
            stroke(cx, &Line::new((base.x0 + 4.0, base.y0), (base.x1 - 4.0, base.y0)), 1.0, ink.detail.multiply_alpha(0.6));
            // The tread on the plate.
            let mut tread = BezPath::new();
            for i in 1..6 {
                let t = i as f64 / 6.0;
                let a = Point::new(base.x0 + 2.0 + t * (base.x1 - 6.0 - base.x0), area.y0 + 10.0 + t * 16.0);
                tread.move_to((a.x, a.y + 2.0));
                tread.line_to((a.x, a.y + 7.0));
            }
            stroke(cx, &tread, 1.0, ink.lit);
            if look == Look::Wah {
                // The toe switch under the raised end, and the sweep (a resonant peak) on the base.
                let toe = Point::new(base.x0 + 7.0, area.y0 + 4.5);
                fill(cx, &Circle::new(toe, 3.2), ink.detail.multiply_alpha(0.5));
                stroke(cx, &Circle::new(toe, 3.2), 1.0, ink.detail);
                let mut peak = BezPath::new();
                let (y, x0) = (base.y1 - 3.0, c.x - 18.0);
                peak.move_to((x0, y));
                peak.curve_to((x0 + 10.0, y), (x0 + 12.0, base.y0 - 3.0), (x0 + 16.0, base.y0 - 3.0));
                peak.curve_to((x0 + 20.0, base.y0 - 3.0), (x0 + 22.0, y), (x0 + 32.0, y));
                stroke(cx, &peak, 1.3, ink.lit);
            }
            out
        }
    }
}

/// How wide a look is drawn (at board size).
pub fn width(look: Look) -> f64 {
    match look {
        Look::Stomp(_) => 60.0,
        Look::Head => 108.0,
        Look::Combo => 88.0,
        Look::Cabinet => 64.0,
        Look::Rocker | Look::Wah => 68.0,
    }
}

/// A look drawn small (or large) around `center`, `scale` times its board size: the palette's and
/// the add menu's icons.
pub fn draw_icon(cx: &mut DrawCx<'_>, look: Look, center: Point, scale: f64, ink: &Ink) {
    let mut tmp = Scene::new();
    {
        let area = Rect::from_center_size(Point::ZERO, (crate::editor::TILE_W, crate::editor::LOOK_H));
        let mut sub = DrawCx { scene: &mut tmp, theme: cx.theme, text: &mut *cx.text, host: cx.host, size: cx.size, fx: &mut *cx.fx };
        draw(&mut sub, look, area, ink);
    }
    cx.scene.append(&tmp, Some(Affine::translate(center.to_vec2()) * Affine::scale(scale)));
}
