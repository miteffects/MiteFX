//! The footer every plugin shares (BLUEPRINT §7.2, as built in the EQ): the plugin's own mode
//! controls on the left (drawn by the plugin), then the latency readout; on the right, icon
//! toggles (auto gain, delta, bypass), then dials with their name and value beside them.

use mite_core::ParamSpec;
use vello::kurbo::{Affine, BezPath, Cap, Join, Line, Point, Rect, Stroke};
use vello::peniko::{Color, Fill};
use vello::Scene;

use crate::host::ParamHost;
use crate::paint::icon_button;
use crate::text::{Align, TextStyle};
use crate::view::{DrawCx, MouseButton, UiEvent};
use crate::widgets::{Help, Knob, KnobKind};

/// Distance between dial centres, the toggles' size, and the dials' diameter.
const DIAL_STEP: f64 = 92.0;
const TOGGLE: f64 = 34.0;
const DIAL: f64 = 34.0;

/// The footer toggles' glyphs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToggleGlyph {
    /// Auto gain / auto makeup: a boost brought back to the line.
    Auto,
    /// Delta: Δ.
    Delta,
    /// Bypass: power.
    Bypass,
    /// Clip: a curve easing into a ceiling line.
    Clip,
    /// True peak: a wave whose crest falls between two samples (dots), the crest ringed.
    TruePeak,
    /// Listen (hear the detector's key): headphones.
    Listen,
    /// Freeze (hold a reverb's tail): a snowflake.
    Freeze,
    /// Mute (silence the output, e.g. while tuning): a speaker with a cross.
    Mute,
}

/// A toggle: its parameter (on at ≥ 0.5), glyph, and what the help strip says about it.
#[derive(Debug, Clone, Copy)]
pub struct FooterToggle {
    pub id: &'static str,
    pub glyph: ToggleGlyph,
    pub words: &'static str,
}

/// A dial: its parameter, look, the name beside it, and what the help strip says about it.
#[derive(Debug, Clone, Copy)]
pub struct FooterDial {
    pub spec: ParamSpec,
    pub kind: KnobKind,
    pub name: &'static str,
    pub words: &'static str,
}

pub struct Footer {
    top: f64,
    size: (f64, f64),
    toggles: Vec<FooterToggle>,
    toggle_hits: Vec<Rect>,
    dials: Vec<(Knob, FooterDial)>,
    pointer: Option<Point>,
}

impl Footer {
    /// A footer from `top` to the bottom of a window of `size`, with `toggles` left of `dials`
    /// (both left to right; the last dial sits 84 px from the right edge).
    pub fn new(top: f64, size: (f64, f64), toggles: Vec<FooterToggle>, dials: Vec<FooterDial>) -> Self {
        let n = dials.len();
        let dials = dials
            .into_iter()
            .enumerate()
            .map(|(i, d)| {
                let x = size.0 - 84.0 - DIAL_STEP * (n - 1 - i) as f64;
                let mut k = Knob::new(d.spec, "", d.kind, Point::new(x, top + 28.0), DIAL);
                k.value_inside = false;
                (k, d)
            })
            .collect();
        Self { top, size, toggle_hits: vec![Rect::ZERO; toggles.len()], toggles, dials, pointer: None }
    }

    pub fn top(&self) -> f64 {
        self.top
    }

    /// The footer's background and its top rule.
    pub fn draw_frame(&self, cx: &mut DrawCx<'_>) {
        let theme = cx.theme;
        let (w, h) = self.size;
        cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.window, None, &Rect::new(0.0, self.top, w, h));
        cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, theme.line, None, &Line::new((0.0, self.top + 0.5), (w, self.top + 0.5)));
    }

    /// LATENCY and the latency the plugin reports, at `x`.
    pub fn draw_latency(&self, cx: &mut DrawCx<'_>, x: f64, latency_ms: f64) {
        let theme = cx.theme;
        let latency = if latency_ms < 0.05 { "0 ms".to_owned() } else { format!("{latency_ms:.1} ms") };
        cx.text.draw(cx.scene, "LATENCY", x, self.top + 15.0, TextStyle::LABEL.sized(9.0), theme.text_lo, Align::Left);
        cx.text.draw(cx.scene, &latency, x, self.top + 28.0, TextStyle::READOUT.sized(11.0), theme.text_hi, Align::Left);
    }

    /// The toggles, the dials' names and values, and the dials.
    pub fn draw_controls(&mut self, cx: &mut DrawCx<'_>) {
        let theme = cx.theme;
        let top = self.top;
        let first_dial = self.dials.first().map_or(self.size.0 - 84.0, |(k, _)| k.center.x);
        let n = self.toggles.len();
        for (i, t) in self.toggles.iter().enumerate() {
            let x = first_dial - 40.0 - (n - i) as f64 * (TOGGLE + 6.0);
            let r = Rect::new(x, top + 11.0, x + TOGGLE, top + 11.0 + TOGGLE);
            let on = plain(cx.host, t.id) >= 0.5;
            let hot = self.pointer.is_some_and(|p| r.contains(p));
            let ink = icon_button(cx.scene, theme, r, theme.panel_control, hot, on, theme.accent);
            draw_toggle_glyph(cx.scene, t.glyph, r.center(), ink);
            self.toggle_hits[i] = r;
        }
        for (k, d) in &self.dials {
            let x = k.center.x + 22.0;
            cx.text.draw(cx.scene, d.name, x, top + 15.0, TextStyle::LABEL.sized(9.0), theme.text_lo, Align::Left);
            cx.text.draw(cx.scene, &k.spec.format(plain(cx.host, k.spec.id)), x, top + 28.0, TextStyle::READOUT.sized(11.0), theme.text_hi, Align::Left);
        }
        for (k, _) in &self.dials {
            k.draw(cx);
        }
    }

    /// The dials' and toggles' events. `true` if the footer used it.
    pub fn event(&mut self, event: &UiEvent, host: &dyn ParamHost) -> bool {
        match *event {
            UiEvent::PointerMove { pos, .. } => self.pointer = Some(pos),
            UiEvent::PointerLeft => self.pointer = None,
            _ => {}
        }
        for (k, _) in &mut self.dials {
            if k.event(event, host) {
                return true;
            }
        }
        if let UiEvent::PointerDown { pos, button: MouseButton::Left, .. } = *event {
            if let Some(i) = self.toggle_hits.iter().position(|r| r.contains(pos)) {
                let id = self.toggles[i].id;
                let on = plain(host, id) >= 0.5;
                host.begin_gesture(id);
                host.set_normalized(id, host.normalize(id, if on { 0.0 } else { 1.0 }));
                host.end_gesture(id);
                return true;
            }
        }
        false
    }

    /// Where the toggle for `id` was last drawn (tests and tools).
    pub fn toggle_rect(&self, id: &str) -> Option<Rect> {
        self.toggles.iter().position(|t| t.id == id).map(|i| self.toggle_hits[i])
    }

    /// A dial is being dragged.
    pub fn busy(&self) -> bool {
        self.dials.iter().any(|(k, _)| k.is_dragging())
    }

    /// The help strip's words for the toggle or dial under the pointer.
    pub fn help(&self, host: &dyn ParamHost) -> Option<Help> {
        let p = self.pointer?;
        if let Some(i) = self.toggle_hits.iter().position(|r| r.contains(p)) {
            let t = &self.toggles[i];
            return Some(Help::default().text(t.words).text(if plain(host, t.id) >= 0.5 { " · on" } else { "" }));
        }
        let (k, d) = self.dials.iter().find(|(k, _)| k.bounds().contains(p))?;
        // The slot fits the widest value the dial can show.
        let (lo, hi) = (k.spec.format(k.spec.min), k.spec.format(k.spec.max));
        let widest = if lo.chars().count() >= hi.chars().count() { lo } else { hi };
        Some(Help::default().text(d.words).text(" · ").slot(k.spec.format(plain(host, k.spec.id)), widest).text(" — drag; double-click resets"))
    }
}

fn plain(host: &dyn ParamHost, id: &str) -> f32 {
    host.plain(id, host.normalized(id))
}

/// A toggle's glyph centred on `c`.
pub fn draw_toggle_glyph(scene: &mut Scene, glyph: ToggleGlyph, c: Point, color: Color) {
    let round = Stroke::new(1.5).with_caps(Cap::Round).with_join(Join::Round);
    match glyph {
        ToggleGlyph::Auto => {
            let mut bump = BezPath::new();
            bump.move_to((c.x - 8.0, c.y + 1.0));
            bump.curve_to((c.x - 3.0, c.y + 1.0), (c.x - 3.0, c.y - 7.0), (c.x, c.y - 7.0));
            bump.curve_to((c.x + 3.0, c.y - 7.0), (c.x + 3.0, c.y + 1.0), (c.x + 8.0, c.y + 1.0));
            scene.stroke(&round, Affine::IDENTITY, color.multiply_alpha(0.5), None, &bump);
            let mut line = BezPath::new();
            line.move_to((c.x - 8.0, c.y + 6.0));
            line.line_to((c.x + 8.0, c.y + 6.0));
            scene.stroke(&round, Affine::IDENTITY, color, None, &line);
        }
        ToggleGlyph::Delta => {
            let mut delta = BezPath::new();
            delta.move_to((c.x, c.y - 6.5));
            delta.line_to((c.x + 7.0, c.y + 5.5));
            delta.line_to((c.x - 7.0, c.y + 5.5));
            delta.close_path();
            scene.stroke(&round, Affine::IDENTITY, color, None, &delta);
        }
        ToggleGlyph::Clip => {
            let mut curve = BezPath::new();
            curve.move_to((c.x - 8.0, c.y + 6.0));
            curve.line_to((c.x - 5.0, c.y + 6.0));
            curve.curve_to((c.x - 2.0, c.y + 6.0), (c.x - 1.5, c.y - 4.0), (c.x + 2.0, c.y - 4.0));
            curve.line_to((c.x + 8.0, c.y - 4.0));
            scene.stroke(&round, Affine::IDENTITY, color, None, &curve);
            let mut ceiling = BezPath::new();
            ceiling.move_to((c.x - 8.0, c.y - 7.0));
            ceiling.line_to((c.x + 8.0, c.y - 7.0));
            scene.stroke(&round, Affine::IDENTITY, color.multiply_alpha(0.5), None, &ceiling);
        }
        ToggleGlyph::TruePeak => {
            let mut wave = BezPath::new();
            wave.move_to((c.x - 8.0, c.y + 6.0));
            wave.curve_to((c.x - 4.0, c.y + 6.0), (c.x - 3.0, c.y - 5.0), (c.x, c.y - 5.0));
            wave.curve_to((c.x + 3.0, c.y - 5.0), (c.x + 4.0, c.y + 6.0), (c.x + 8.0, c.y + 6.0));
            scene.stroke(&round, Affine::IDENTITY, color.multiply_alpha(0.6), None, &wave);
            for dx in [-3.2, 3.2] {
                scene.fill(Fill::NonZero, Affine::IDENTITY, color, None, &vello::kurbo::Circle::new((c.x + dx, c.y - 2.4), 1.5));
            }
            scene.stroke(&Stroke::new(1.2), Affine::IDENTITY, color, None, &vello::kurbo::Circle::new((c.x, c.y - 5.0), 2.6));
        }
        ToggleGlyph::Listen => {
            if let Ok(mut p) = BezPath::from_svg("M4 14 V12 A8 8 0 0 1 20 12 V14 M4 14 H7 V20 H4 Z M20 14 H17 V20 H20 Z") {
                p.apply_affine(Affine::translate((c.x - 9.6, c.y - 9.6)) * Affine::scale(0.8));
                scene.stroke(&round, Affine::IDENTITY, color, None, &p);
            }
        }
        ToggleGlyph::Freeze => {
            let mut flake = BezPath::new();
            for k in 0..3 {
                let a = std::f64::consts::PI * (0.5 + k as f64 / 3.0);
                let (dx, dy) = (a.cos(), a.sin());
                flake.move_to((c.x - 7.5 * dx, c.y - 7.5 * dy));
                flake.line_to((c.x + 7.5 * dx, c.y + 7.5 * dy));
                // A small V near each end.
                for end in [-1.0, 1.0] {
                    let (ex, ey) = (c.x + end * 5.0 * dx, c.y + end * 5.0 * dy);
                    for side in [-1.0, 1.0] {
                        let b = a + side * std::f64::consts::FRAC_PI_4 * end;
                        flake.move_to((ex, ey));
                        flake.line_to((ex + end * 2.6 * b.cos(), ey + end * 2.6 * b.sin()));
                    }
                }
            }
            scene.stroke(&round, Affine::IDENTITY, color, None, &flake);
        }
        ToggleGlyph::Mute => {
            let mut cone = BezPath::new();
            cone.move_to((c.x - 7.5, c.y - 2.5));
            cone.line_to((c.x - 4.5, c.y - 2.5));
            cone.line_to((c.x - 0.5, c.y - 6.5));
            cone.line_to((c.x - 0.5, c.y + 6.5));
            cone.line_to((c.x - 4.5, c.y + 2.5));
            cone.line_to((c.x - 7.5, c.y + 2.5));
            cone.close_path();
            scene.stroke(&round, Affine::IDENTITY, color, None, &cone);
            let mut cross = BezPath::new();
            cross.move_to((c.x + 2.5, c.y - 3.0));
            cross.line_to((c.x + 8.0, c.y + 3.0));
            cross.move_to((c.x + 8.0, c.y - 3.0));
            cross.line_to((c.x + 2.5, c.y + 3.0));
            scene.stroke(&round, Affine::IDENTITY, color, None, &cross);
        }
        ToggleGlyph::Bypass => {
            let mut stem = BezPath::new();
            stem.move_to((c.x, c.y - 6.5));
            stem.line_to((c.x, c.y - 1.0));
            scene.stroke(&round, Affine::IDENTITY, color, None, &stem);
            scene.stroke(&round, Affine::IDENTITY, color, None, &vello::kurbo::Arc::new(c + (0.0, 0.5), (5.5, 5.5), -1.0, 5.3, 0.0));
        }
    }
}

/// A raised button that opens a dropdown: an icon, its text and a chevron; the border lights in
/// the accent while it is open or hovered.
pub fn draw_dropdown_button(cx: &mut DrawCx<'_>, r: Rect, text: &str, open: bool, hot: bool, icon: impl FnOnce(&mut Scene, Point)) {
    let theme = cx.theme;
    let border = if open || hot { theme.accent } else { theme.panel_control_border };
    crate::paint::raised(cx.scene, vello::kurbo::RoundedRect::from_rect(r, 6.0), theme.panel_control, border, theme.shadows);
    icon(cx.scene, Point::new(r.x0 + 10.0, r.y0 + 12.0));
    cx.text.draw(cx.scene, text, r.x0 + 40.0, r.y0 + 11.0, TextStyle::BODY.weighted(600.0), theme.text_hi, Align::Left);
    let mut chevron = BezPath::new();
    let (x, y) = (r.x1 - 14.0, r.center().y);
    chevron.move_to((x - 4.0, y - 2.0));
    chevron.line_to((x, y + 2.0));
    chevron.line_to((x + 4.0, y - 2.0));
    let round = Stroke::new(1.5).with_caps(Cap::Round).with_join(Join::Round);
    cx.scene.stroke(&round, Affine::IDENTITY, theme.text_mid, None, &chevron);
}
