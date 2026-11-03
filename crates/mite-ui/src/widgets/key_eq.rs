//! The sidechain EQ shared by Mite Comp and Mite Gate (gate-brief §3): the key bands' summed
//! response over 20 Hz…20 kHz, ±18 dB, from the EQ's own analog response, with a node per band.
//! Drag a node for its frequency (and gain, for bands that have one; dragging turns the band on),
//! double-click it to turn the band on or off. The plugin keeps the parameters as
//! `k{N}_{on,type,freq,gain,q}` (the same ids in both plugins).
//!
//! Moved out of Comp's editor (2026-11-03) with its arithmetic unchanged, so Comp draws the same.

use mite_dsp::eq::{analog_response_db, BandSettings, BandType};

use crate::kurbo::{Affine, BezPath, Circle, Line, Point, Rect, RoundedRect, Stroke};
use crate::paint::{glow_stroke, panel};
use crate::peniko::Fill;
use crate::{Align, DrawCx, ParamHost, TextStyle};

/// A node's hit radius, px.
const NODE_HIT: f64 = 12.0;

/// A key band parameter's host id, e.g. `k1_freq` (`band` from 0).
pub fn key_id(band: usize, param: &str) -> String {
    format!("k{}_{param}", band + 1)
}

/// The band `b` as a host's parameters set it.
pub fn band(host: &dyn ParamHost, b: usize) -> BandSettings {
    let p = |param: &str| {
        let id = key_id(b, param);
        host.plain(&id, host.normalized(&id))
    };
    BandSettings {
        enabled: p("on") >= 0.5,
        kind: BandType::from_index(p("type").round().max(0.0) as usize),
        freq: f64::from(p("freq")),
        gain_db: f64::from(p("gain")),
        q: f64::from(p("q")),
        ..BandSettings::default()
    }
}

/// Where band `band`'s node sits in `area`.
pub fn node(area: Rect, band: &BandSettings) -> Point {
    let x = area.x0 + area.width() * (band.freq / 20.0).ln() / 1000f64.ln();
    let g = if band.kind.has_gain() { band.gain_db } else { 0.0 };
    Point::new(x, area.center().y - g * (area.height() / 2.0 - 12.0) / 18.0)
}

/// The EQ well with its `bands` key bands, and the band being dragged.
#[derive(Debug, Clone, PartialEq)]
pub struct KeyEq {
    pub area: Rect,
    pub bands: usize,
    drag: Option<usize>,
}

impl KeyEq {
    pub fn new(area: Rect, bands: usize) -> Self {
        Self { area, bands, drag: None }
    }

    pub fn busy(&self) -> bool {
        self.drag.is_some()
    }

    /// The band whose node is at `pos`.
    pub fn hit(&self, host: &dyn ParamHost, pos: Point) -> Option<usize> {
        (0..self.bands).find(|&b| node(self.area, &band(host, b)).distance(pos) < NODE_HIT)
    }

    pub fn draw(&self, cx: &mut DrawCx<'_>) {
        let theme = cx.theme;
        let area = self.area;
        panel(cx.scene, RoundedRect::from_rect(area, 8.0), theme.well, theme.well_border);
        let fx = |f: f64| area.x0 + area.width() * (f / 20.0).ln() / 1000f64.ln();
        let gy = |g: f64| area.center().y - g * (area.height() / 2.0 - 12.0) / 18.0;
        for f in [100.0, 1000.0, 10_000.0] {
            cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, theme.grid_minor, None, &Line::new((fx(f), area.y0), (fx(f), area.y1)));
        }
        for g in [-12.0, -6.0, 6.0, 12.0] {
            cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, theme.grid_minor, None, &Line::new((area.x0, gy(g)), (area.x1, gy(g))));
        }
        cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, theme.grid_zero, None, &Line::new((area.x0, gy(0.0)), (area.x1, gy(0.0))));
        cx.text.draw(cx.scene, "SIDECHAIN EQ", area.x0 + 10.0, area.y0 + 8.0, TextStyle::LABEL.sized(10.0), theme.text_lo, Align::Left);
        let bands: Vec<BandSettings> = (0..self.bands).map(|b| band(cx.host, b)).collect();
        // The gain at the well's edge: past it the curve is clipped.
        let edge = 18.0 * (area.height() / 2.0) / (area.height() / 2.0 - 12.0);
        let pts: Vec<Point> = (0..=160)
            .map(|i| {
                let f = 20.0 * 1000f64.powf(i as f64 / 160.0);
                let g: f64 = bands.iter().filter(|b| b.enabled).map(|b| analog_response_db(b, f)).sum();
                Point::new(fx(f), gy(soft_edge(g, edge)))
            })
            .collect();
        let mut curve = BezPath::new();
        for (i, &q) in pts.iter().enumerate() {
            if i == 0 { curve.move_to(q) } else { curve.line_to(q) }
        }
        cx.scene.push_clip_layer(Fill::NonZero, Affine::IDENTITY, &RoundedRect::from_rect(area, 8.0));
        glow_stroke(cx, &curve, 2.0, theme.curve, theme.curve, 4.0);
        cx.scene.pop_layer();
        for (b, band) in bands.iter().enumerate() {
            let p = node(area, band);
            let color = theme.band_ramp[0];
            let alpha = if band.enabled { 1.0 } else { 0.35 };
            cx.scene.fill(Fill::NonZero, Affine::IDENTITY, color.multiply_alpha(alpha), None, &Circle::new(p, 7.0));
            glow_stroke(cx, &Circle::new(p, 6.0), 2.0, color.multiply_alpha(alpha), color, if band.enabled { 4.0 } else { 1.0 });
            cx.text.draw(cx.scene, &(b + 1).to_string(), p.x, p.y - 5.5, TextStyle::LABEL.sized(8.0).weighted(800.0), theme.display, Align::Center);
        }
    }

    /// A press on a node starts dragging it (one gesture: on, frequency, gain). `true` if `pos`
    /// is in the well.
    pub fn press(&mut self, host: &dyn ParamHost, pos: Point) -> bool {
        if !self.area.contains(pos) {
            return false;
        }
        if let Some(b) = self.hit(host, pos) {
            for p in ["on", "freq", "gain"] {
                host.begin_gesture(&key_id(b, p));
            }
            let id = key_id(b, "on");
            host.set_normalized(&id, host.normalize(&id, 1.0));
            self.drag = Some(b);
        }
        true
    }

    /// Moves the dragged node. `true` while dragging.
    pub fn drag_to(&mut self, host: &dyn ParamHost, pos: Point) -> bool {
        let Some(b) = self.drag else { return false };
        let area = self.area;
        let f = 20.0 * 1000f64.powf(((pos.x - area.x0) / area.width()).clamp(0.0, 1.0));
        let g = ((area.center().y - pos.y) / (area.height() / 2.0 - 12.0) * 18.0).clamp(-18.0, 18.0);
        let set = |param: &str, v: f32| {
            let id = key_id(b, param);
            host.set_normalized(&id, host.normalize(&id, v));
        };
        set("freq", f as f32);
        if band(host, b).kind.has_gain() {
            set("gain", g as f32);
        }
        true
    }

    /// Ends a drag. `true` if there was one.
    pub fn release(&mut self, host: &dyn ParamHost) -> bool {
        let Some(b) = self.drag.take() else { return false };
        for p in ["on", "freq", "gain"] {
            host.end_gesture(&key_id(b, p));
        }
        true
    }

    /// Double-clicking a node turns its band on or off. `true` if `pos` is in the well.
    pub fn double_click(&mut self, host: &dyn ParamHost, pos: Point) -> bool {
        if !self.area.contains(pos) {
            return false;
        }
        if let Some(b) = self.hit(host, pos) {
            let id = key_id(b, "on");
            let on = host.plain(&id, host.normalized(&id)) >= 0.5;
            crate::write_if_changed(host, &id, host.normalize(&id, if on { 0.0 } else { 1.0 }));
        }
        true
    }
}

/// Past ±16 dB the curve bends smoothly (tanh) towards a level 1 dB beyond the well's edge, so a
/// steep cut leaves the well with no corner (a hard clamp drew a flat floor and a kink the
/// filters do not have) and its glow, which is not clipped, stays close to the well.
fn soft_edge(g: f64, edge: f64) -> f64 {
    let (knee, lim) = (16.0, edge + 1.0);
    if g.abs() <= knee {
        g
    } else {
        g.signum() * (knee + (lim - knee) * ((g.abs() - knee) / (lim - knee)).tanh())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_curve_bends_into_the_edge_without_a_corner() {
        let edge = 22.0;
        assert_eq!(soft_edge(12.0, edge), 12.0);
        let mut prev = soft_edge(15.0, edge);
        let mut slope = 1.0;
        for k in 1..400 {
            let g = 15.0 + k as f64 * 0.1;
            let v = soft_edge(g, edge);
            let s = (v - prev) / 0.1;
            assert!(s <= slope + 1e-6 && s > 0.0, "smooth and rising at {g}");
            assert!(v < edge + 1.0);
            (prev, slope) = (v, s);
        }
        assert!((soft_edge(-60.0, edge) + edge + 1.0).abs() < 0.01);
    }
}
