//! The tick-scale dial.
//!
//! Geometry is the mockups' 64-unit dial scaled to `diameter`: 31 ticks over 270° between radii
//! 25 and 29, a disc of radius 19.5 with a drop shadow, and a pointer from 21.5 to 31. The value
//! and unit sit inside the disc, and the label sits below it.

use mite_core::{DialCurve, ParamSpec};
use vello::kurbo::{BezPath, Circle, Point, Rect, Stroke, Vec2};
use vello::peniko::{Color, Fill};

use crate::host::ParamHost;
use crate::paint::glow_stroke;
use crate::text::{Align, TextStyle};
use crate::view::{DrawCx, MouseButton, UiEvent};

/// Vertical drag distance for the full range at 100 % scale (docs/design/interaction.md).
pub const DRAG_PX_PER_RANGE: f32 = 200.0;
/// Shift is always ×0.1 (interaction.md).
pub const FINE_FACTOR: f32 = 0.1;

const TICKS: usize = 31;
const SWEEP_DEG: f64 = 270.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KnobKind {
    /// Lights from the minimum.
    Unipolar,
    /// Lights from the centre tick (gain, pan).
    Bipolar,
}

#[derive(Debug)]
pub struct Knob {
    pub spec: ParamSpec,
    /// Host parameter id. Defaults to `spec.id`; per-band dials use e.g. `b03_freq`.
    pub id: String,
    /// Draw the value and unit inside the disc (standard dials). Small dials show them beside.
    pub value_inside: bool,
    pub label: &'static str,
    pub kind: KnobKind,
    pub center: Point,
    pub diameter: f64,
    /// Lit-tick colour. `None` uses the plugin accent.
    pub color: Option<Color>,
    /// How the dial maps to values, when not the host's normalisation (see [`DialCurve`]).
    pub curve: Option<DialCurve>,
    /// Show the value as the dial's travel, 0–100 %, instead of the parameter's units (a relative
    /// setting, e.g. Comp's release under auto release). The parameter itself is unchanged.
    pub percent: bool,
    hover: bool,
    drag: Option<Drag>,
}

#[derive(Debug, Clone, Copy)]
struct Drag {
    last_y: f64,
    value: f32,
}

impl Knob {
    pub fn new(spec: ParamSpec, label: &'static str, kind: KnobKind, center: Point, diameter: f64) -> Self {
        Self {
            spec,
            id: spec.id.to_owned(),
            value_inside: true,
            label,
            kind,
            center,
            diameter,
            color: None,
            curve: None,
            percent: false,
            hover: false,
            drag: None,
        }
    }

    /// Same dial bound to another host parameter id.
    pub fn with_id(mut self, id: impl Into<String>) -> Self {
        self.id = id.into();
        self
    }

    fn scale(&self) -> f64 {
        let hover = if self.hover && self.drag.is_none() { 1.1 } else { 1.0 };
        self.diameter / 64.0 * hover
    }

    fn point(&self, deg: f64, r: f64) -> Point {
        let a = (deg - 90.0).to_radians();
        self.center + Vec2::new(a.cos(), a.sin()) * (r * self.scale())
    }

    fn tick(&self, path: &mut BezPath, deg: f64, r0: f64, r1: f64) {
        path.move_to(self.point(deg, r0));
        path.line_to(self.point(deg, r1));
    }

    /// The same dial with its own feel (see [`DialCurve`]).
    pub fn with_curve(mut self, curve: DialCurve) -> Self {
        self.curve = Some(curve);
        self
    }

    /// Where the dial points, 0…1.
    fn position(&self, host: &dyn ParamHost) -> f32 {
        let norm = host.normalized(&self.id);
        match self.curve {
            Some(c) => c.to_position(host.plain(&self.id, norm)),
            None => norm,
        }
    }

    /// The value as the dial shows it, and its unit.
    pub fn value_text(&self, host: &dyn ParamHost) -> (String, &'static str) {
        if self.percent {
            return (format!("{:.0}", self.position(host) * 100.0), "%");
        }
        self.spec.format_parts(host.plain(&self.id, host.normalized(&self.id)))
    }

    /// Whether a drag of this dial is in progress.
    pub fn is_dragging(&self) -> bool {
        self.drag.is_some()
    }

    /// Hit area: the dial's square plus the label below it.
    pub fn bounds(&self) -> Rect {
        let r = self.diameter / 2.0;
        Rect::new(self.center.x - r, self.center.y - r, self.center.x + r, self.center.y + r + 20.0)
    }

    pub fn draw(&self, cx: &mut DrawCx<'_>) {
        let s = self.scale();
        let theme = cx.theme;
        let color = self.color.unwrap_or(theme.accent);
        let to_deg = |n: f32| -SWEEP_DEG / 2.0 + SWEEP_DEG * f64::from(n);
        let value_deg = to_deg(self.position(cx.host));
        // Bipolar dials light from the zero point, which is not the middle of an asymmetric
        // range (gain −48…+24 dB has 0 dB at two thirds).
        let origin_deg = to_deg(self.spec.normalize(0.0_f32.clamp(self.spec.min, self.spec.max)));

        let mut lit = BezPath::new();
        let mut dim = BezPath::new();
        for i in 0..TICKS {
            let d = -SWEEP_DEG / 2.0 + i as f64 * SWEEP_DEG / (TICKS - 1) as f64;
            let on = match self.kind {
                KnobKind::Unipolar => d <= value_deg + 0.01,
                KnobKind::Bipolar => d >= value_deg.min(origin_deg) - 0.01 && d <= value_deg.max(origin_deg) + 0.01,
            };
            self.tick(if on { &mut lit } else { &mut dim }, d, 25.0, 29.0);
        }
        let tick_stroke = Stroke::new(2.0 * s).with_caps(vello::kurbo::Cap::Round);
        cx.scene.stroke(&tick_stroke, vello::kurbo::Affine::IDENTITY, theme.tick_off, None, &dim);
        if self.kind == KnobKind::Bipolar {
            let mut centre = BezPath::new();
            self.tick(&mut centre, origin_deg, 22.5, 30.0);
            cx.scene.stroke(&tick_stroke, vello::kurbo::Affine::IDENTITY, theme.text_mid, None, &centre);
        }
        glow_stroke(cx, &lit, 2.0 * s, color, color, 1.8 * s);

        // Disc with its drop shadow (mockup: dy 2.5, blur 2.5, black 75 %).
        let r = 19.5 * s;
        let disc_box = Rect::from_center_size(self.center, (2.0 * r, 2.0 * r));
        if theme.shadows {
            cx.scene.draw_blurred_rounded_rect(
                vello::kurbo::Affine::translate((0.0, 2.5 * s)),
                disc_box,
                Color::from_rgba8(0, 0, 0, 255).multiply_alpha(theme.disc_shadow_alpha),
                r,
                2.5 * s,
            );
        }
        let disc = Circle::new(self.center, r);
        cx.scene.fill(Fill::NonZero, vello::kurbo::Affine::IDENTITY, theme.knob_disc, None, &disc);
        cx.scene.stroke(&Stroke::new(theme.border_width), vello::kurbo::Affine::IDENTITY, theme.knob_disc_border, None, &disc);

        let mut pointer = BezPath::new();
        self.tick(&mut pointer, value_deg, 21.5, 31.0);
        glow_stroke(cx, &pointer, 2.5 * s, theme.needle, color, 2.2 * s);

        // Value + unit inside the disc.
        let (value, unit) = self.value_text(cx.host);
        if !self.value_inside {
            return;
        }
        let value_style = TextStyle::READOUT_DIAL.sized((12.0 * s) as f32);
        let unit_style = TextStyle::UNIT_DIAL.sized((9.0 * s) as f32);
        let block = if unit.is_empty() { 12.0 * s } else { 12.0 * s + 2.0 * s + 9.0 * s };
        let top = self.center.y - block / 2.0 - 2.5 * s;
        cx.text.draw(cx.scene, &value, self.center.x, top, value_style, theme.text_strong, Align::Center);
        if !unit.is_empty() {
            cx.text.draw(cx.scene, unit, self.center.x, top + 14.0 * s, unit_style, theme.text_lo, Align::Center);
        }

        // Label below.
        cx.text.draw(
            cx.scene,
            self.label,
            self.center.x,
            self.center.y + self.diameter / 2.0 + 4.0,
            TextStyle::LABEL,
            theme.text_lo,
            Align::Center,
        );
    }

    /// What the modulation matrix does to this dial: an arc just outside the ticks over its reach
    /// (`lo`…`hi`, shares of the travel from where the dial sits) and a dot where it plays now
    /// (`live`). Call after [`Knob::draw`].
    pub fn draw_modulation(&self, cx: &mut DrawCx<'_>, reach: (f32, f32), live: f32) {
        let s = self.scale();
        let color = self.color.unwrap_or(cx.theme.accent);
        let base = cx.host.normalized(&self.id);
        let at = |off: f32| {
            let n = (base + off).clamp(0.0, 1.0);
            let p = match self.curve {
                Some(c) => c.to_position(cx.host.plain(&self.id, n)),
                None => n,
            };
            -SWEEP_DEG / 2.0 + SWEEP_DEG * f64::from(p)
        };
        let (d0, d1) = (at(reach.0), at(reach.1));
        let steps = (((d1 - d0).abs() / 4.0).ceil() as usize).max(1);
        let mut arc = BezPath::new();
        for i in 0..=steps {
            let p = self.point(d0 + (d1 - d0) * i as f64 / steps as f64, 33.0);
            if i == 0 {
                arc.move_to(p);
            } else {
                arc.line_to(p);
            }
        }
        cx.scene.stroke(&Stroke::new(2.0 * s).with_caps(vello::kurbo::Cap::Round), vello::kurbo::Affine::IDENTITY, color.multiply_alpha(0.45), None, &arc);
        let dot = Circle::new(self.point(at(live), 33.0), 2.6 * s);
        cx.scene.fill(Fill::NonZero, vello::kurbo::Affine::IDENTITY, cx.theme.text_strong, None, &dot);
    }

    /// A live level on the dial (`level` in the dial's own units, e.g. dBFS on a threshold): an
    /// arc just outside the ticks from the start to where the level plays, dim up to the dial's
    /// value and lit past it (what is over a threshold), lit only while `active`. Call after
    /// [`Knob::draw`].
    pub fn draw_level(&self, cx: &mut DrawCx<'_>, level: f32, active: bool) {
        let s = self.scale();
        let color = self.color.unwrap_or(cx.theme.accent);
        let deg = |plain: f32| {
            let p = match self.curve {
                Some(c) => c.to_position(plain.clamp(self.spec.min, self.spec.max)),
                None => self.spec.normalize(plain.clamp(self.spec.min, self.spec.max)),
            };
            -SWEEP_DEG / 2.0 + SWEEP_DEG * f64::from(p)
        };
        if level <= self.spec.min {
            return;
        }
        let start = -SWEEP_DEG / 2.0;
        let at = deg(level);
        let value = deg(cx.host.plain(&self.id, cx.host.normalized(&self.id)));
        let arc = |d0: f64, d1: f64| {
            let steps = (((d1 - d0).abs() / 4.0).ceil() as usize).max(1);
            let mut path = BezPath::new();
            for i in 0..=steps {
                let p = self.point(d0 + (d1 - d0) * i as f64 / steps as f64, 33.0);
                if i == 0 {
                    path.move_to(p);
                } else {
                    path.line_to(p);
                }
            }
            path
        };
        let stroke = Stroke::new(2.0 * s).with_caps(vello::kurbo::Cap::Round);
        cx.scene.stroke(&stroke, vello::kurbo::Affine::IDENTITY, cx.theme.text_lo.multiply_alpha(0.7), None, &arc(start, at.min(value)));
        if at > value {
            let over = arc(value, at);
            if active {
                glow_stroke(cx, &over, 2.0 * s, color, color, 1.6 * s);
            } else {
                cx.scene.stroke(&stroke, vello::kurbo::Affine::IDENTITY, cx.theme.text_mid, None, &over);
            }
        }
    }

    /// Handles input. Returns `true` when the knob needs a redraw.
    pub fn event(&mut self, event: &UiEvent, host: &dyn ParamHost) -> bool {
        let id = self.id.as_str();
        match *event {
            UiEvent::PointerMove { pos, mods } => {
                if let Some(drag) = &mut self.drag {
                    let factor = if mods.fine() { FINE_FACTOR } else { 1.0 };
                    let delta = (drag.last_y - pos.y) as f32 / DRAG_PX_PER_RANGE * factor;
                    drag.last_y = pos.y;
                    drag.value = (drag.value + delta).clamp(0.0, 1.0);
                    // The drag moves the dial; a curved dial turns its position into a value.
                    let value = match self.curve {
                        Some(c) => host.normalize(id, c.to_plain(drag.value)),
                        None => drag.value,
                    };
                    host.set_normalized(id, value);
                    true
                } else {
                    let hover = self.bounds().contains(pos);
                    std::mem::replace(&mut self.hover, hover) != hover
                }
            }
            UiEvent::PointerDown { pos, button: MouseButton::Left, .. } if self.bounds().contains(pos) => {
                host.begin_gesture(id);
                self.drag = Some(Drag { last_y: pos.y, value: self.position(host) });
                true
            }
            UiEvent::PointerUp { button: MouseButton::Left, .. } if self.drag.is_some() => {
                self.drag = None;
                host.end_gesture(id);
                true
            }
            UiEvent::DoubleClick { pos, .. } if self.bounds().contains(pos) => {
                // The first click of the pair already began a gesture and ended it on release.
                host.begin_gesture(id);
                host.set_normalized(id, host.default_normalized(id));
                host.end_gesture(id);
                true
            }
            UiEvent::Wheel { pos, lines, mods } if self.bounds().contains(pos) => {
                let plain = self.spec.wheel(host.plain(id, host.normalized(id)), lines, mods.fine());
                let value = host.normalize(id, plain);
                host.begin_gesture(id);
                host.set_normalized(id, value);
                host.end_gesture(id);
                true
            }
            UiEvent::PointerLeft => std::mem::replace(&mut self.hover, false),
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::{MemoryParamHost, ParamHost};
    use crate::view::Modifiers;
    use mite_core::{ParamDisplay, ParamScale};

    const SPEC: ParamSpec = ParamSpec {
        id: "gain",
        name: "Gain",
        unit: "dB",
        min: -48.0,
        max: 24.0,
        default: 0.0,
        scale: ParamScale::Linear,
        smoothing_ms: 20.0,
        decimals: 1,
        wheel_step: 0.5,
        wheel_step_fine: 0.1,
        display: ParamDisplay::Number,
    };

    fn knob() -> Knob {
        Knob::new(SPEC, "GAIN", KnobKind::Bipolar, Point::new(100.0, 100.0), 64.0)
    }

    fn mods(shift: bool) -> Modifiers {
        Modifiers { shift, ..Default::default() }
    }

    #[test]
    fn drag_up_full_range_reaches_max() {
        let host = MemoryParamHost::new().with(SPEC);
        host.set_normalized("gain", 0.0);
        let mut k = knob();
        k.event(&UiEvent::PointerDown { pos: Point::new(100.0, 100.0), button: MouseButton::Left, mods: mods(false) }, &host);
        k.event(&UiEvent::PointerMove { pos: Point::new(100.0, 100.0 - DRAG_PX_PER_RANGE as f64), mods: mods(false) }, &host);
        assert!((host.normalized("gain") - 1.0).abs() < 1e-6);
    }

    #[test]
    fn shift_drag_is_ten_times_finer() {
        let host = MemoryParamHost::new().with(SPEC);
        host.set_normalized("gain", 0.5);
        let mut k = knob();
        k.event(&UiEvent::PointerDown { pos: Point::new(100.0, 100.0), button: MouseButton::Left, mods: mods(true) }, &host);
        k.event(&UiEvent::PointerMove { pos: Point::new(100.0, 80.0), mods: mods(true) }, &host);
        assert!((host.normalized("gain") - 0.51).abs() < 1e-5);
    }

    #[test]
    fn wheel_steps_half_a_db_and_a_tenth_with_shift() {
        let host = MemoryParamHost::new().with(SPEC);
        let mut k = knob();
        let at = Point::new(100.0, 100.0);
        k.event(&UiEvent::Wheel { pos: at, lines: 1.0, mods: mods(false) }, &host);
        assert!((host.plain("gain", host.normalized("gain")) - 0.5).abs() < 1e-4);
        k.event(&UiEvent::Wheel { pos: at, lines: -1.0, mods: mods(true) }, &host);
        assert!((host.plain("gain", host.normalized("gain")) - 0.4).abs() < 1e-4);
    }

    #[test]
    fn a_percent_dial_reads_its_travel_and_leaves_the_parameter_alone() {
        let release = ParamSpec { id: "release", unit: "ms", min: 10.0, max: 2500.0, default: 150.0, scale: ParamScale::Log, ..SPEC };
        let curve = DialCurve(&[(0.0, 10.0), (0.3, 100.0), (0.75, 300.0), (1.0, 2500.0)]);
        let host = MemoryParamHost::new().with(release);
        host.set_plain("release", 300.0);
        let mut k = Knob::new(release, "RELEASE", KnobKind::Unipolar, Point::new(100.0, 100.0), 48.0).with_curve(curve);
        assert_eq!(k.value_text(&host).1, "ms");
        k.percent = true;
        assert_eq!(k.value_text(&host), ("75".to_owned(), "%"));
        assert!((host.plain("release", host.normalized("release")) - 300.0).abs() < 0.5);
    }

    #[test]
    fn curved_dials_give_common_values_more_travel() {
        let release = ParamSpec { id: "release", unit: "ms", min: 10.0, max: 2500.0, default: 150.0, scale: ParamScale::Log, ..SPEC };
        let curve = DialCurve(&[(0.0, 10.0), (0.3, 100.0), (0.75, 300.0), (1.0, 2500.0)]);
        let host = MemoryParamHost::new().with(release);
        host.set_plain("release", 100.0);
        let mut k = Knob::new(release, "RELEASE", KnobKind::Unipolar, Point::new(100.0, 100.0), 48.0).with_curve(curve);
        // 100 → 300 ms takes 45 % of the dial's travel: 90 px at 200 px per range.
        k.event(&UiEvent::PointerDown { pos: Point::new(100.0, 100.0), button: MouseButton::Left, mods: mods(false) }, &host);
        k.event(&UiEvent::PointerMove { pos: Point::new(100.0, 10.0), mods: mods(false) }, &host);
        let ms = host.plain("release", host.normalized("release"));
        assert!((ms - 300.0).abs() < 1.0, "{ms} ms");
        // No snapping: a one-pixel move changes the value by a fraction of a millisecond step.
        k.event(&UiEvent::PointerMove { pos: Point::new(100.0, 11.0), mods: mods(false) }, &host);
        let next = host.plain("release", host.normalized("release"));
        assert!(next < ms && ms - next < 4.0 && (next - next.round()).abs() > 1e-3, "{next} ms");
    }

    #[test]
    fn double_click_resets_to_default() {
        let host = MemoryParamHost::new().with(SPEC);
        host.set_normalized("gain", 0.9);
        let mut k = knob();
        k.event(&UiEvent::DoubleClick { pos: Point::new(100.0, 100.0), mods: mods(false) }, &host);
        assert_eq!(host.normalized("gain"), SPEC.normalize(SPEC.default));
    }

    #[test]
    fn ignores_clicks_outside() {
        let host = MemoryParamHost::new().with(SPEC);
        host.set_normalized("gain", 0.5);
        let mut k = knob();
        assert!(!k.event(&UiEvent::PointerDown { pos: Point::new(10.0, 10.0), button: MouseButton::Left, mods: mods(false) }, &host));
    }
}
