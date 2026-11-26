//! The deck (moved from Mite Sat's tape panel, 2026-11-26): drawn from the machine. The reels
//! turn at the tape's speed, the trace under them is the transport's speed against nominal as the
//! engine reports it (wow and flutter, ±0.3 % full height), the meter is the peak going into the
//! tape against the deck's operating level (0) and its maximum output level (+10). Then speed,
//! standard, and the dials.

use std::collections::VecDeque;
use std::time::Instant;

use mite_dsp::tape::{Speed, Standard};
use mite_ui::kurbo::{Affine, BezPath, Circle, Line, Point, Rect, RoundedRect, Shape, Stroke};
use mite_ui::paint::{glow_stroke, panel};
use mite_ui::peniko::Fill;
use mite_ui::widgets::{Help, Knob, KnobKind};
use mite_ui::{write_if_changed, Align, DrawCx, MouseButton, ParamHost, TextStyle, UiEvent};

use crate::params::{BIAS, EQ, FLUTTER, HISS, LEVEL, SPEED, WOW};

/// The speed trace: this many frames kept, and its full height (± fraction of nominal).
const TRACE_LEN: usize = 180;
const TRACE_RANGE: f32 = 0.003;
/// The meter: dB against the operating level (−18 dBFS), its range and the maximum output level.
const METER_DB: (f32, f32) = (-24.0, 16.0);
const MOL_DB: f32 = 10.0;
/// The operating level's peak (a −18 dBFS sine), linear.
const OPERATING_PEAK: f32 = 0.125_89;
/// Reels: turns a second at 7.5 ips (they turn with the speed).
const REEL_TURNS_AT_7_5: f64 = 0.35;
/// The meter falls back this fast, dB/s.
const METER_FALL_DB_S: f32 = 20.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Hot {
    Speed(usize),
    Eq(usize),
}

/// What the engine reports for the panel to draw.
#[derive(Debug, Clone, Copy, Default)]
pub struct DeckReading {
    pub running: bool,
    /// Speed against nominal (fraction).
    pub speed_dev: f32,
    /// The peak going into the tape, linear.
    pub field_peak: f32,
}

pub struct Deck {
    rect: Rect,
    knobs: Vec<Knob>,
    hot: Option<Hot>,
    pointer: Option<Point>,
    trace: VecDeque<f32>,
    angle: f64,
    meter_db: f32,
    last: Option<Instant>,
    /// Whether the deck runs (lit) or is bypassed, as last read.
    running: bool,
}

fn plain(host: &dyn ParamHost, id: &str) -> f32 {
    host.plain(id, host.normalized(id))
}

fn set(host: &dyn ParamHost, id: &str, v: f32) {
    write_if_changed(host, id, host.normalize(id, v));
}

/// Speed chips' labels (the deck's own markings).
const SPEED_MARKS: [&str; 4] = ["3¾", "7½", "15", "30"];

impl Deck {
    pub fn new(rect: Rect) -> Self {
        let y = rect.y0 + 178.0;
        let x0 = rect.x0 + 660.0;
        let dial = |spec, label, kind, i: usize, d: f64| Knob::new(spec, label, kind, Point::new(x0 + [0.0, 120.0, 220.0, 320.0, 420.0][i], y), d).with_id(spec.id);
        let knobs = vec![
            dial(LEVEL, "LEVEL", KnobKind::Bipolar, 0, 84.0),
            dial(BIAS, "BIAS", KnobKind::Bipolar, 1, 64.0),
            dial(WOW, "WOW", KnobKind::Unipolar, 2, 64.0),
            dial(FLUTTER, "FLUTTER", KnobKind::Unipolar, 3, 64.0),
            dial(HISS, "HISS", KnobKind::Unipolar, 4, 64.0),
        ];
        Self { rect, knobs, hot: None, pointer: None, trace: VecDeque::with_capacity(TRACE_LEN + 1), angle: 0.0, meter_db: METER_DB.0, last: None, running: true }
    }

    /// The deck's drawing: reels, heads and the trace.
    fn deck_rect(&self) -> Rect {
        Rect::new(self.rect.x0 + 16.0, self.rect.y0 + 44.0, self.rect.x0 + 540.0, self.rect.y0 + 316.0)
    }

    fn meter_rect(&self) -> Rect {
        Rect::new(self.rect.x0 + 566.0, self.rect.y0 + 56.0, self.rect.x0 + 586.0, self.rect.y0 + 300.0)
    }

    fn speed_rects(&self) -> [Rect; 4] {
        std::array::from_fn(|i| {
            let x = self.rect.x0 + 16.0 + i as f64 * 62.0;
            Rect::new(x, self.rect.y0 + 330.0, x + 56.0, self.rect.y0 + 360.0)
        })
    }

    fn eq_rects(&self) -> [Rect; 2] {
        std::array::from_fn(|i| {
            let x = self.rect.x0 + 290.0 + i as f64 * 74.0;
            Rect::new(x, self.rect.y0 + 330.0, x + 68.0, self.rect.y0 + 360.0)
        })
    }

    pub fn busy(&self) -> bool {
        self.knobs.iter().any(Knob::is_dragging)
    }

    fn hot_at(&self, p: Point) -> Option<Hot> {
        if let Some(i) = self.speed_rects().iter().position(|r| r.contains(p)) {
            return Some(Hot::Speed(i));
        }
        self.eq_rects().iter().position(|r| r.contains(p)).map(Hot::Eq)
    }

    /// Whether the deck took the event.
    pub fn event(&mut self, event: &UiEvent, host: &dyn ParamHost) -> bool {
        if let UiEvent::PointerMove { pos, .. } = *event {
            self.pointer = Some(pos);
            self.hot = self.hot_at(pos);
        }
        for k in &mut self.knobs {
            if k.event(event, host) {
                return true;
            }
        }
        match *event {
            UiEvent::PointerMove { pos, .. } => {
                self.pointer = Some(pos);
                self.hot = self.hot_at(pos);
                false
            }
            UiEvent::PointerLeft => {
                self.pointer = None;
                self.hot = None;
                false
            }
            UiEvent::PointerDown { pos, button: MouseButton::Left, .. } => match self.hot_at(pos) {
                Some(Hot::Speed(i)) => {
                    set(host, SPEED.id, i as f32);
                    true
                }
                Some(Hot::Eq(i)) => {
                    set(host, EQ.id, i as f32);
                    true
                }
                None => false,
            },
            _ => false,
        }
    }

    /// What the hovered part does, or `None` when the pointer is elsewhere.
    pub fn help(&self, host: &dyn ParamHost) -> Option<Help> {
        let p = self.pointer?;
        if !self.rect.contains(p) {
            return None;
        }
        if let Some(k) = self.knobs.iter().find(|k| k.bounds().contains(p) || k.is_dragging()) {
            let v = plain(host, &k.id);
            let words = match k.spec.id {
                id if id == LEVEL.id => " — how hard the mix hits the tape: 0 dB puts −18 dBFS at the deck's operating level, +10 over it is its maximum output level. It comes back off after the tape, so it moves the saturation, not the volume.",
                id if id == BIAS.id => " — under-biased (left) is brighter and grittier at every level; over-biased (right) is cleaner and loses its highs.",
                id if id == WOW.id => " — the transport's slow drift in speed (pitch), the same for both channels; 100 % is a worn deck (about 0.6 %).",
                id if id == FLUTTER.id => " — the transport's fast ripple in speed (7 and 13 Hz); 100 % is about 0.25 %.",
                id if id == HISS.id => " — the tape's own noise, through the playback EQ and the head; 100 % is the deck's real level for its speed.",
                _ => "",
            };
            return Some(Help::default().text(format!("{} ", k.spec.name)).slot(k.spec.format(v), k.spec.format(k.spec.max)).text(words));
        }
        let words = match self.hot {
            Some(Hot::Speed(i)) => format!(
                "{} — sets the head bump ({:.0} Hz), how soon the highs saturate and fall off, the EQ's time constants, and the hiss.",
                Speed::NAMES[i],
                mite_dsp::tape::head_bump(Speed::ALL[i]).0
            ),
            Some(Hot::Eq(0)) => "NAB — the American curve: 50 µs (90 at 3.75 ips) and a bass time constant at 50 Hz, so the lows go to tape lighter and come back with more hum and hiss under them.".to_owned(),
            Some(Hot::Eq(_)) => "CCIR — the European curve: 35 µs at 15 ips, 70 at 7.5, no bass time constant; the highs go to tape lighter at 15 ips.".to_owned(),
            None => {
                if self.meter_rect().inflate(10.0, 10.0).contains(p) {
                    "The tape's level: the peak going into it against the deck's operating level (0) and its maximum output level (+10, where the line turns).".to_owned()
                } else if self.deck_rect().contains(p) {
                    "The reels turn at the tape's speed; the trace under them is the speed against nominal now (wow and flutter), ±0.3 % full height.".to_owned()
                } else {
                    "The tape machine.".to_owned()
                }
            }
        };
        Some(Help::default().text(words))
    }

    /// Takes this frame's reading and moves the reels, the trace and the meter on.
    pub fn advance(&mut self, host: &dyn ParamHost, reading: DeckReading) {
        let now = Instant::now();
        let dt = self.last.map_or(0.0, |t| now.duration_since(t).as_secs_f64()).min(0.1);
        self.last = Some(now);
        self.running = reading.running;
        if reading.running {
            let speed = Speed::from_index(plain(host, SPEED.id).round().max(0.0) as usize);
            let turns = REEL_TURNS_AT_7_5 * speed.ips() / 7.5 * (1.0 + f64::from(reading.speed_dev));
            self.angle = (self.angle + std::f64::consts::TAU * turns * dt) % std::f64::consts::TAU;
            self.trace.push_back(reading.speed_dev);
            let db = if reading.field_peak > 0.0 { 20.0 * (reading.field_peak / OPERATING_PEAK).log10() } else { METER_DB.0 };
            self.meter_db = db.max(self.meter_db - METER_FALL_DB_S * dt as f32).clamp(METER_DB.0, METER_DB.1);
        } else {
            self.trace.push_back(0.0);
            self.meter_db = (self.meter_db - METER_FALL_DB_S * dt as f32).max(METER_DB.0);
        }
        while self.trace.len() > TRACE_LEN {
            self.trace.pop_front();
        }
    }

    /// Whether it still moves (the editor keeps redrawing while it does).
    pub fn moving(&self, reading: DeckReading) -> bool {
        reading.running || self.meter_db > METER_DB.0
    }

    pub fn draw(&self, cx: &mut DrawCx<'_>) {
        let theme = cx.theme;
        let host = cx.host;
        let on = self.running;
        if theme.shadows {
            cx.scene.draw_blurred_rounded_rect(Affine::translate((0.0, 6.0)), self.rect, mite_ui::peniko::Color::from_rgba8(0, 0, 0, 160), 12.0, 16.0);
        }
        panel(cx.scene, RoundedRect::from_rect(self.rect, 12.0), theme.display, theme.well_border);
        self.draw_deck(cx, on);
        self.draw_meter(cx, on);

        // Speed and standard.
        let speed = plain(host, SPEED.id).round().max(0.0) as usize;
        for (i, r) in self.speed_rects().iter().enumerate() {
            chip(cx, *r, SPEED_MARKS[i], i == speed, self.hot == Some(Hot::Speed(i)));
        }
        let standard = plain(host, EQ.id).round().max(0.0) as usize;
        for (i, r) in self.eq_rects().iter().enumerate() {
            chip(cx, *r, Standard::NAMES[i], i == standard, self.hot == Some(Hot::Eq(i)));
        }
        cx.text.draw(cx.scene, "IPS", self.rect.x0 + 266.0, self.rect.y0 + 338.0, TextStyle::LABEL.sized(9.0), theme.text_lo, Align::Left);

        for k in &self.knobs {
            k.draw(cx);
        }
    }

    fn draw_deck(&self, cx: &mut DrawCx<'_>, on: bool) {
        let theme = cx.theme;
        let accent = theme.accent;
        let r = self.deck_rect();
        let ink = if on { theme.text_mid } else { theme.text_lo };
        let reel_r = 78.0;
        let left = Point::new(r.x0 + 112.0, r.y0 + 100.0);
        let right = Point::new(r.x1 - 112.0, r.y0 + 100.0);
        let head = Point::new(r.center().x, r.y0 + 196.0);
        // The tape: off each reel's pack, down past the guides, across the head.
        let pack = 0.62 * reel_r;
        let mut path = BezPath::new();
        path.move_to((left.x - pack, left.y));
        path.quad_to((left.x - pack - 6.0, head.y), (left.x, head.y));
        path.line_to((right.x, head.y));
        path.quad_to((right.x + pack + 6.0, head.y), (right.x + pack, right.y));
        let tape = Stroke::new(2.0);
        if on {
            glow_stroke(cx, &path, 2.0, accent.multiply_alpha(0.85), accent, 1.5);
        } else {
            cx.scene.stroke(&tape, Affine::IDENTITY, ink, None, &path);
        }
        for (centre, phase) in [(left, self.angle), (right, self.angle + 0.7)] {
            draw_reel(cx, centre, reel_r, pack, phase, on);
        }
        // The heads on the tape path: erase, record, play.
        for (k, dx) in [-60.0, 0.0, 60.0].iter().enumerate() {
            let hr = Rect::new(head.x + dx - 14.0, head.y + 2.0, head.x + dx + 14.0, head.y + 22.0);
            let rr = RoundedRect::from_rect(hr, 4.0);
            cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.well, None, &rr);
            let edge = if on && k == 2 { accent } else { theme.well_border };
            cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, edge, None, &rr);
        }
        // The speed trace.
        let tr = Rect::new(r.x0 + 20.0, r.y0 + 236.0, r.x1 - 20.0, r.y1 - 4.0);
        let mid = tr.center().y;
        cx.scene.stroke(&Stroke::new(1.0).with_dashes(0.0, [2.0, 4.0]), Affine::IDENTITY, theme.well_border, None, &Line::new((tr.x0, mid), (tr.x1, mid)));
        if self.trace.len() > 1 {
            let n = TRACE_LEN as f64;
            let mut line = BezPath::new();
            let start = TRACE_LEN - self.trace.len();
            for (i, v) in self.trace.iter().enumerate() {
                let x = tr.x0 + (start + i) as f64 / (n - 1.0) * tr.width();
                let y = mid - f64::from((*v / TRACE_RANGE).clamp(-1.0, 1.0)) * 0.5 * tr.height();
                if i == 0 {
                    line.move_to((x, y));
                } else {
                    line.line_to((x, y));
                }
            }
            if on {
                glow_stroke(cx, &line, 1.4, accent, accent, 1.2);
            } else {
                cx.scene.stroke(&Stroke::new(1.2), Affine::IDENTITY, ink, None, &line);
            }
        }
    }

    fn draw_meter(&self, cx: &mut DrawCx<'_>, on: bool) {
        let theme = cx.theme;
        let r = self.meter_rect();
        let y_of = |db: f32| r.y1 - f64::from((db - METER_DB.0) / (METER_DB.1 - METER_DB.0)) * r.height();
        let well = RoundedRect::from_rect(r, 4.0);
        cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.well, None, &well);
        cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, theme.well_border, None, &well);
        if on && self.meter_db > METER_DB.0 {
            let top = y_of(self.meter_db);
            let mol = y_of(MOL_DB);
            let body = Rect::new(r.x0 + 3.0, top.max(mol), r.x1 - 3.0, r.y1 - 3.0);
            cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.accent, None, &RoundedRect::from_rect(body, 2.0));
            if top < mol {
                let over = Rect::new(r.x0 + 3.0, top, r.x1 - 3.0, mol);
                cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.warn, None, &RoundedRect::from_rect(over, 2.0));
            }
        }
        for (db, mark) in [(0.0, "0"), (MOL_DB, "+10"), (-12.0, "−12")] {
            let y = y_of(db);
            cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, theme.text_lo, None, &Line::new((r.x1 + 3.0, y), (r.x1 + 8.0, y)));
            cx.text.draw(cx.scene, mark, r.x1 + 11.0, y - 6.0, TextStyle::LABEL.sized(9.0), theme.text_lo, Align::Left);
        }
    }
}

fn chip(cx: &mut DrawCx<'_>, r: Rect, text: &str, selected: bool, hot: bool) {
    let theme = cx.theme;
    let rr = RoundedRect::from_rect(r, r.height() / 2.0);
    cx.scene.fill(Fill::NonZero, Affine::IDENTITY, if selected { theme.accent.multiply_alpha(0.18) } else { theme.well }, None, &rr);
    if selected {
        glow_stroke(cx, &rr, 1.2, theme.accent, theme.accent, 1.5);
    } else {
        cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, if hot { theme.text_hi } else { theme.well_border }, None, &rr);
    }
    let ink = if selected { theme.accent } else if hot { theme.text_hi } else { theme.text_mid };
    cx.text.draw(cx.scene, text, r.center().x, r.center().y - 7.0, TextStyle::LABEL.sized(11.0).weighted(700.0), ink, Align::Center);
}

/// One reel: its flange, the tape pack, the hub and three spokes at `phase`.
fn draw_reel(cx: &mut DrawCx<'_>, c: Point, r: f64, pack: f64, phase: f64, on: bool) {
    let theme = cx.theme;
    let edge = if on { theme.text_mid } else { theme.well_border };
    cx.scene.stroke(&Stroke::new(1.2), Affine::IDENTITY, edge, None, &Circle::new(c, r));
    cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.well, None, &Circle::new(c, pack));
    cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, theme.well_border, None, &Circle::new(c, pack));
    let hub = 0.22 * r;
    cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.knob_disc, None, &Circle::new(c, hub));
    let mut spokes = BezPath::new();
    for k in 0..3 {
        let a = phase + k as f64 * std::f64::consts::TAU / 3.0;
        let (s, co) = a.sin_cos();
        spokes.move_to((c.x + co * hub * 0.35, c.y + s * hub * 0.35));
        spokes.line_to((c.x + co * (pack - 6.0), c.y + s * (pack - 6.0)));
    }
    if on {
        glow_stroke(cx, &spokes, 2.0, theme.accent, theme.accent, 1.2);
    } else {
        cx.scene.stroke(&Stroke::new(2.0), Affine::IDENTITY, edge, None, &spokes);
    }
    // Windows in the flange, turning with it.
    let mut windows = BezPath::new();
    for k in 0..3 {
        let a = phase + (k as f64 + 0.5) * std::f64::consts::TAU / 3.0;
        let (s, co) = a.sin_cos();
        let p = Point::new(c.x + co * (pack + 0.5 * (r - pack)), c.y + s * (pack + 0.5 * (r - pack)));
        windows.extend(Circle::new(p, 0.3 * (r - pack)).path_elements(0.1));
    }
    cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, edge, None, &windows);
}
