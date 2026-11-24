//! The sidechain EQ shared by Mite Comp and Mite Gate (gate-brief §3): the key bands' summed
//! response over 20 Hz…20 kHz, ±18 dB, from the EQ's own analog response, with a node per band,
//! over the key's spectrum (where the key's energy is; `KeySpectrum`). The plugin keeps the
//! parameters as `k{N}_{on,type,freq,gain,q}` and, where it has them, `k{N}_slope`.
//!
//! As an editor (`KeyEq::event`, user, 2026-11-03: "make it flexible so that user can make custom
//! eq as he wants … all shapes available"): click an empty spot to make a band there; drag a node
//! for its frequency and gain; wheel on a node for its Q (a cut's slope); right-click a node for
//! its shape (every EQ type, each with its icon) or to remove it; double-click switches a band
//! on and off. Each gesture is one undo step.
//!
//! Moved out of Comp's editor (2026-11-03) with its drawing unchanged, so Comp draws the same.

use std::time::Instant;

use mite_core::SpectrumTap;
use mite_dsp::analyzer::{spectrum_point_freq, Analyzer, LogSpectrum, ANALYZER_SIZE, SPECTRUM_POINTS};
use mite_dsp::eq::{analog_response_db, BandSettings, BandType, DEFAULT_SLOPE, SLOPES_DB};

use crate::kurbo::{Affine, BezPath, Circle, Line, Point, Rect, RoundedRect, Stroke};
use crate::paint::{glow_stroke, panel};
use crate::peniko::Fill;
use crate::widgets::eq_icons::draw_type_icon;
use crate::widgets::{Dropdown, DropdownEvent, Help};
use crate::{Align, DrawCx, MouseButton, ParamHost, TextStyle, UiEvent};

/// A node's hit radius, px.
const NODE_HIT: f64 = 12.0;
/// The shape menu: every EQ type, then Remove.
const SHAPES: [&str; 11] = ["Bell", "Low Shelf", "High Shelf", "Low Cut", "High Cut", "Notch", "Band Pass", "Tilt Shelf", "Flat Tilt", "All Pass", "Remove"];
const REMOVE: usize = 10;
/// The key spectrum's scale (the analyzer's dB) from the well's bottom to its top.
const SPECTRUM_DB: (f32, f32) = (-96.0, -12.0);

/// A key band parameter's host id, e.g. `k1_freq` (`band` from 0).
pub fn key_id(band: usize, param: &str) -> mite_core::ParamId {
    mite_core::ParamId::indexed("k", band + 1, 0, param)
}

fn plain(host: &dyn ParamHost, id: &str) -> f32 {
    host.plain(id, host.normalized(id))
}

fn set(host: &dyn ParamHost, id: &str, v: f32) {
    host.set_normalized(id, host.normalize(id, v));
}

/// The band `b` as a host's parameters set it (`slope`: whether the plugin has `k{N}_slope`).
pub fn band_with(host: &dyn ParamHost, b: usize, slope: bool) -> BandSettings {
    let p = |param: &str| plain(host, &key_id(b, param));
    BandSettings {
        enabled: p("on") >= 0.5,
        kind: BandType::from_index(p("type").round().max(0.0) as usize),
        freq: f64::from(p("freq")),
        gain_db: f64::from(p("gain")),
        q: f64::from(p("q")),
        slope: if slope { (p("slope").round().max(0.0) as usize).min(SLOPES_DB.len() - 1) } else { DEFAULT_SLOPE },
        ..BandSettings::default()
    }
}

/// The band `b` for a plugin without slopes (Comp).
pub fn band(host: &dyn ParamHost, b: usize) -> BandSettings {
    band_with(host, b, false)
}

/// Where band `band`'s node sits in `area`.
pub fn node(area: Rect, band: &BandSettings) -> Point {
    let x = area.x0 + area.width() * (band.freq / 20.0).ln() / 1000f64.ln();
    let g = if band.kind.has_gain() { band.gain_db } else { 0.0 };
    Point::new(x, area.center().y - g * (area.height() / 2.0 - 12.0) / 18.0)
}

/// The key's spectrum (read from the plugin's tap), eased: 60 ms up, 300 ms down.
pub struct KeySpectrum {
    analyzer: Analyzer,
    log: LogSpectrum,
    samples: Vec<f32>,
    power: Vec<f32>,
    target: Vec<f32>,
    curve: Vec<f32>,
    last: Instant,
}

impl Default for KeySpectrum {
    fn default() -> Self {
        Self::new()
    }
}

impl KeySpectrum {
    pub fn new() -> Self {
        let analyzer = Analyzer::new(ANALYZER_SIZE);
        let bins = analyzer.bins();
        Self { analyzer, log: LogSpectrum::new(ANALYZER_SIZE, 48_000.0), samples: vec![0.0; ANALYZER_SIZE], power: vec![0.0; bins], target: vec![-150.0; SPECTRUM_POINTS], curve: vec![-150.0; SPECTRUM_POINTS], last: Instant::now() }
    }

    /// Reads the tap (at most once per display refresh) and eases towards it. Whether it moved.
    pub fn update(&mut self, tap: &SpectrumTap, sample_rate: f64, dt: f64) -> bool {
        if sample_rate.max(1.0) != self.log.sample_rate() {
            self.log = LogSpectrum::new(ANALYZER_SIZE, sample_rate.max(1.0));
        }
        if self.last.elapsed().as_secs_f64() >= 0.0155 {
            self.last = Instant::now();
            tap.read_latest(&mut self.samples);
            if self.samples.iter().all(|s| s.abs() < 1e-7) {
                self.target.iter_mut().for_each(|t| *t = -150.0);
            } else {
                self.analyzer.analyze_power(&self.samples, &mut self.power);
                self.log.read(&self.power, &mut self.target);
            }
        }
        let (rise, fall) = ((1.0 - (-dt / 0.06).exp()) as f32, (1.0 - (-dt / 0.3).exp()) as f32);
        let mut moved = false;
        for (v, &t) in self.curve.iter_mut().zip(&self.target) {
            let next = *v + (t - *v) * if t > *v { rise } else { fall };
            moved |= (next - *v).abs() > 0.02;
            *v = next;
        }
        moved
    }

    /// Settles at once (tools render a single frame).
    pub fn settle(&mut self, tap: &SpectrumTap, sample_rate: f64) {
        self.last = Instant::now() - std::time::Duration::from_secs(1);
        self.update(tap, sample_rate, 10.0);
    }

    fn draw(&self, cx: &mut DrawCx<'_>, area: Rect) {
        if !self.curve.iter().any(|&v| v > SPECTRUM_DB.0) {
            return;
        }
        let theme = cx.theme;
        let fx = |f: f64| area.x0 + area.width() * (f / 20.0).ln() / 1000f64.ln();
        let y = |db: f32| area.y1 - f64::from((db.clamp(SPECTRUM_DB.0, SPECTRUM_DB.1) - SPECTRUM_DB.0) / (SPECTRUM_DB.1 - SPECTRUM_DB.0)) * area.height();
        let pts: Vec<Point> = (0..SPECTRUM_POINTS).filter(|&i| (20.0..=20_000.0).contains(&spectrum_point_freq(i))).map(|i| Point::new(fx(spectrum_point_freq(i)), y(self.curve[i]))).collect();
        if pts.len() < 2 {
            return;
        }
        let line = crate::widgets::history::smooth_path(&pts);
        let mut fill = line.clone();
        fill.line_to((pts[pts.len() - 1].x, area.y1));
        fill.line_to((pts[0].x, area.y1));
        fill.close_path();
        cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.sidechain.multiply_alpha(0.16), None, &fill);
        cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, theme.sidechain.multiply_alpha(0.5), None, &line);
    }
}

/// The EQ well with its `bands` key bands.
pub struct KeyEq {
    pub area: Rect,
    pub bands: usize,
    /// Whether the plugin has `k{N}_slope` (cuts' steepness).
    pub slopes: bool,
    drag: Option<usize>,
    selected: Option<usize>,
    hover: Option<usize>,
    pointer: Option<Point>,
    menu: Option<(usize, Dropdown)>,
    /// The bounds the shape menu may use (the plugin's display).
    pub menu_bounds: (f64, f64),
}

impl KeyEq {
    pub fn new(area: Rect, bands: usize) -> Self {
        Self { area, bands, slopes: false, drag: None, selected: None, hover: None, pointer: None, menu: None, menu_bounds: (area.y0, area.y1) }
    }

    pub fn with_slopes(mut self) -> Self {
        self.slopes = true;
        self
    }

    pub fn busy(&self) -> bool {
        self.drag.is_some()
    }

    pub fn menu_open(&self) -> bool {
        self.menu.is_some()
    }

    /// The shape menu's rows (tests and tools).
    pub fn menu_rows(&self) -> Vec<Rect> {
        self.menu.as_ref().map(|(_, m)| m.item_rects()).unwrap_or_default()
    }

    fn band(&self, host: &dyn ParamHost, b: usize) -> BandSettings {
        band_with(host, b, self.slopes)
    }

    /// The band whose node is at `pos` (on bands first, so an off band never hides one).
    pub fn hit(&self, host: &dyn ParamHost, pos: Point) -> Option<usize> {
        let near = |b: usize| node(self.area, &self.band(host, b)).distance(pos) < NODE_HIT;
        (0..self.bands).find(|&b| self.band(host, b).enabled && near(b)).or_else(|| (0..self.bands).find(|&b| near(b)))
    }

    pub fn draw(&self, cx: &mut DrawCx<'_>) {
        self.draw_with(cx, None);
    }

    /// The well, the key's spectrum under the curve (if given), the curve and the nodes.
    pub fn draw_with(&self, cx: &mut DrawCx<'_>, spectrum: Option<&KeySpectrum>) {
        let theme = cx.theme;
        let area = self.area;
        panel(cx.scene, RoundedRect::from_rect(area, 8.0), theme.well, theme.well_border);
        if let Some(sp) = spectrum {
            cx.scene.push_clip_layer(Fill::NonZero, Affine::IDENTITY, &RoundedRect::from_rect(area, 8.0));
            sp.draw(cx, area);
            cx.scene.pop_layer();
        }
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
        let bands: Vec<BandSettings> = (0..self.bands).map(|b| self.band(cx.host, b)).collect();
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
        cx.begin_clip(&RoundedRect::from_rect(area, 8.0));
        glow_stroke(cx, &curve, 2.0, theme.curve, theme.curve, 4.0);
        cx.end_clip();
        for (b, band) in bands.iter().enumerate() {
            let p = node(area, band);
            let color = theme.band_ramp[0];
            let alpha = if band.enabled { 1.0 } else { 0.35 };
            cx.scene.fill(Fill::NonZero, Affine::IDENTITY, color.multiply_alpha(alpha), None, &Circle::new(p, 7.0));
            glow_stroke(cx, &Circle::new(p, 6.0), 2.0, color.multiply_alpha(alpha), color, if band.enabled { 4.0 } else { 1.0 });
            if self.selected == Some(b) || self.hover == Some(b) {
                cx.scene.stroke(&Stroke::new(1.5), Affine::IDENTITY, theme.text_strong.multiply_alpha(0.8), None, &Circle::new(p, 10.5));
            }
            cx.text.draw(cx.scene, &(b + 1).to_string(), p.x, p.y - 5.5, TextStyle::LABEL.sized(8.0).weighted(800.0), theme.display, Align::Center);
        }
    }

    /// The shape menu, drawn last (over everything).
    pub fn draw_menu(&self, cx: &mut DrawCx<'_>) {
        if let Some((_, list)) = &self.menu {
            let accent = cx.theme.accent;
            list.draw(cx, accent, |scene, i, at, ink| {
                if i < REMOVE {
                    draw_type_icon(scene, BandType::from_index(i), at + (0.0, 6.0), ink);
                } else {
                    let mut x = BezPath::new();
                    x.move_to(at + (6.0, 6.0));
                    x.line_to(at + (16.0, 16.0));
                    x.move_to(at + (16.0, 6.0));
                    x.line_to(at + (6.0, 16.0));
                    scene.stroke(&Stroke::new(1.6), Affine::IDENTITY, ink, None, &x);
                }
            });
        }
    }

    /// The help strip's words for what is under the pointer.
    pub fn help(&self, host: &dyn ParamHost) -> Option<Help> {
        if let Some((_, list)) = &self.menu {
            let i = self.pointer.and_then(|p| list.item_rects().iter().position(|r| r.contains(p)))?;
            return Some(Help::default().text(if i == REMOVE { "Remove — the band stops shaping the key (its settings are kept)".to_owned() } else { format!("{} — pick this shape for the band", SHAPES[i]) }));
        }
        let p = self.pointer.filter(|p| self.area.contains(*p))?;
        if let Some(b) = self.drag.or_else(|| self.hit(host, p)) {
            let s = self.band(host, b);
            let freq = if s.freq >= 1000.0 { format!("{:.2} kHz", s.freq / 1000.0) } else { format!("{:.0} Hz", s.freq) };
            let mut h = Help::default().text(format!("Band {} · {} · ", b + 1, SHAPES[s.kind as usize])).slot(freq, "20.00 kHz");
            if s.kind.has_gain() {
                h = h.text(" · ").slot(format!("{:+.1} dB", s.gain_db), "+18.0 dB");
            }
            if self.slopes && s.kind.has_slope() {
                h = h.text(" · ").slot(format!("{} dB/oct", SLOPES_DB[s.slope]), "96 dB/oct");
            } else {
                h = h.text(" · Q ").slot(format!("{:.2}", s.q), "10.00");
            }
            let wheel = if self.slopes && s.kind.has_slope() { "slope" } else { "Q" };
            return Some(h.text(format!(" — drag; wheel: {wheel}; right-click: shape or remove; double-click: on/off")));
        }
        Some(Help::default().text("Sidechain EQ — shapes what the detector hears, over the key's spectrum. Click to make a band; right-click a band for its shape."))
    }

    /// The well's events (and the shape menu's). `true` if used.
    pub fn event(&mut self, event: &UiEvent, host: &dyn ParamHost) -> bool {
        if let Some((b, list)) = &mut self.menu {
            if let UiEvent::PointerMove { pos, .. } = *event {
                self.pointer = Some(pos);
            }
            let b = *b;
            match list.event(event) {
                DropdownEvent::Open => {}
                DropdownEvent::Picked(i) => {
                    self.menu = None;
                    if i == REMOVE {
                        crate::write_if_changed(host, &key_id(b, "on"), host.normalize(&key_id(b, "on"), 0.0));
                    } else {
                        host.begin_gesture(&key_id(b, "type"));
                        host.begin_gesture(&key_id(b, "on"));
                        set(host, &key_id(b, "type"), i as f32);
                        set(host, &key_id(b, "on"), 1.0);
                        host.end_gesture(&key_id(b, "on"));
                        host.end_gesture(&key_id(b, "type"));
                    }
                }
                DropdownEvent::Closed => self.menu = None,
            }
            return true;
        }
        match *event {
            UiEvent::PointerMove { pos, .. } => {
                self.pointer = Some(pos);
                if self.drag_to(host, pos) {
                    return true;
                }
                self.hover = if self.area.contains(pos) { self.hit(host, pos) } else { None };
                false
            }
            UiEvent::PointerLeft => {
                self.pointer = None;
                self.hover = None;
                false
            }
            UiEvent::PointerDown { pos, button: MouseButton::Left, .. } => self.press(host, pos),
            UiEvent::PointerDown { pos, button: MouseButton::Right, .. } => {
                let Some(b) = self.area.contains(pos).then(|| self.hit(host, pos)).flatten() else { return self.area.contains(pos) };
                self.selected = Some(b);
                let s = self.band(host, b);
                let current = if s.enabled { s.kind as usize } else { REMOVE };
                let anchor = Rect::from_center_size(pos, (2.0, 2.0));
                self.menu = Some((b, Dropdown::new(anchor, &SHAPES, current, self.menu_bounds).with_min_width(180.0)));
                true
            }
            UiEvent::PointerUp { button: MouseButton::Left, .. } => self.release(host),
            UiEvent::DoubleClick { pos, .. } => self.double_click(host, pos),
            UiEvent::Wheel { pos, lines, mods } => {
                if !self.area.contains(pos) {
                    return false;
                }
                let Some(b) = self.hit(host, pos) else { return true };
                let s = self.band(host, b);
                if self.slopes && s.kind.has_slope() {
                    let id = key_id(b, "slope");
                    let next = (s.slope as f32 + lines.signum()).clamp(0.0, (SLOPES_DB.len() - 1) as f32);
                    crate::write_if_changed(host, &id, host.normalize(&id, next));
                } else {
                    let id = key_id(b, "q");
                    let step = if mods.fine() { 1.0 / 24.0 } else { 1.0 / 6.0 };
                    let q = (s.q * 2f64.powf(f64::from(lines) * step)).clamp(0.1, 10.0);
                    crate::write_if_changed(host, &id, host.normalize(&id, q as f32));
                }
                true
            }
            _ => false,
        }
    }

    /// A press on a node starts dragging it; on an empty spot it makes a band there (the first
    /// one that is off) and drags that. One gesture either way. `true` if `pos` is in the well.
    pub fn press(&mut self, host: &dyn ParamHost, pos: Point) -> bool {
        if !self.area.contains(pos) {
            return false;
        }
        let b = match self.hit(host, pos) {
            Some(b) => b,
            None => {
                let Some(free) = (0..self.bands).find(|&b| !self.band(host, b).enabled) else { return true };
                host.begin_gesture(&key_id(free, "type"));
                set(host, &key_id(free, "type"), BandType::Bell as usize as f32);
                host.end_gesture(&key_id(free, "type"));
                free
            }
        };
        for p in ["on", "freq", "gain"] {
            host.begin_gesture(&key_id(b, p));
        }
        set(host, &key_id(b, "on"), 1.0);
        self.drag = Some(b);
        self.selected = Some(b);
        self.drag_to(host, pos);
        true
    }

    /// Moves the dragged node. `true` while dragging.
    pub fn drag_to(&mut self, host: &dyn ParamHost, pos: Point) -> bool {
        let Some(b) = self.drag else { return false };
        let area = self.area;
        let f = 20.0 * 1000f64.powf(((pos.x - area.x0) / area.width()).clamp(0.0, 1.0));
        let g = ((area.center().y - pos.y) / (area.height() / 2.0 - 12.0) * 18.0).clamp(-18.0, 18.0);
        set(host, &key_id(b, "freq"), f as f32);
        if self.band(host, b).kind.has_gain() {
            set(host, &key_id(b, "gain"), g as f32);
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
            let on = plain(host, &id) >= 0.5;
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

    #[test]
    fn the_shape_menu_lists_every_eq_type() {
        assert_eq!(&SHAPES[..REMOVE], &BandType::NAMES[..]);
    }
}
