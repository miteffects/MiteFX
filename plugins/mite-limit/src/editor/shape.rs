//! The gain shape (brief §3.2): the gain the limiter applies to a probe burst, from the DSP's own
//! code (`mite_dsp::limit::probe`), drawn against time: the S-shaped ramp over the lookahead
//! before the burst, the hold through it, the release after. Time runs on an asinh axis (about
//! linear near the burst, logarithmic in the tail), fixed, so dragging never rescales it.
//!
//! Its three parts are handles: drag the ramp sideways for the lookahead, the burst for the
//! attack (how fast sustained limiting builds the slow envelope), the tail for the release.

use mite_dsp::limit::{probe, Settings, PROBE_BURST_MS, PROBE_OVER_DB, PROBE_TAIL_MS};
use mite_ui::kurbo::{Affine, Line, Point, Rect, RoundedRect, Stroke};
use mite_ui::paint::{glow_stroke, panel};
use mite_ui::peniko::{Fill, Gradient};
use mite_ui::widgets::history::monotone_path;
use mite_ui::DrawCx;

/// The probe runs at this rate (the shape in ms is the audio's; the editor's cost stays small).
const RATE: f32 = 12_000.0;
/// The asinh axis: its knee, and the span drawn (ms).
const KNEE_MS: f64 = 4.0;
const T_MIN: f64 = -12.0;
const T_MAX: f64 = PROBE_BURST_MS as f64 + PROBE_TAIL_MS as f64;
/// The gain drawn: 0 dB at the top to this at the bottom.
const FLOOR_DB: f64 = -(PROBE_OVER_DB as f64) - 2.0;

/// Which part of the shape is under the pointer: each a handle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Part {
    Ramp,
    Burst,
    Tail,
}

/// What a shape is computed from.
type Key = [u32; 7];

fn key(s: &Settings, lookahead_ms: f32) -> Key {
    [lookahead_ms.to_bits(), s.attack_ms.to_bits(), s.release_ms.to_bits(), s.style as u32, s.slow.to_bits(), s.link_transients.to_bits(), s.link_release.to_bits()]
}

#[derive(Default)]
pub struct Shape {
    key: Option<Key>,
    points: Vec<(f32, f32)>,
    ghost_key: Option<Key>,
    ghost: Vec<(f32, f32)>,
}

impl Shape {
    /// Recomputes the shape when its settings changed; `ghost` is a style being hovered.
    /// Returns whether anything was recomputed.
    pub fn update(&mut self, s: &Settings, lookahead_ms: f32, ghost: Option<Settings>) -> bool {
        let mut changed = false;
        let k = key(s, lookahead_ms);
        if self.key != Some(k) {
            self.points = thin(probe(s, lookahead_ms, RATE));
            self.key = Some(k);
            changed = true;
        }
        match ghost {
            Some(g) => {
                let k = key(&g, lookahead_ms);
                if self.ghost_key != Some(k) {
                    self.ghost = thin(probe(&g, lookahead_ms, RATE));
                    self.ghost_key = Some(k);
                    changed = true;
                }
            }
            None => {
                changed |= self.ghost_key.is_some();
                self.ghost_key = None;
                self.ghost.clear();
            }
        }
        changed
    }

    /// The gain (dB) at `t_ms`, as computed (tests and tools).
    pub fn gain_at(&self, t_ms: f32) -> f32 {
        self.points.iter().min_by(|a, b| (a.0 - t_ms).abs().total_cmp(&(b.0 - t_ms).abs())).map_or(0.0, |p| p.1)
    }
}

/// The probe's points thinned to about 500 along the axis (where they are drawn), once per
/// change rather than every frame; the gain at each is the probe's own.
fn thin(pts: Vec<(f32, f32)>) -> Vec<(f32, f32)> {
    let step = (axis(T_MAX) - axis(T_MIN)) / 500.0;
    let mut out = Vec::with_capacity(520);
    let mut last = f64::NEG_INFINITY;
    for (i, &(t, db)) in pts.iter().enumerate() {
        let u = axis(f64::from(t));
        if f64::from(t) >= T_MIN && (u - last >= step || i + 1 == pts.len()) {
            out.push((t, db));
            last = u;
        }
    }
    out
}

fn axis(t: f64) -> f64 {
    (t / KNEE_MS).asinh()
}

/// x of time `t_ms` in `r`.
pub fn t_x(r: Rect, t_ms: f64) -> f64 {
    let (a, b) = (axis(T_MIN), axis(T_MAX));
    r.x0 + 8.0 + (axis(t_ms.clamp(T_MIN, T_MAX)) - a) / (b - a) * (r.width() - 16.0)
}

/// Time (ms) at x in `r`.
pub fn x_t(r: Rect, x: f64) -> f64 {
    let (a, b) = (axis(T_MIN), axis(T_MAX));
    let u = a + ((x - r.x0 - 8.0) / (r.width() - 16.0)).clamp(0.0, 1.0) * (b - a);
    KNEE_MS * u.sinh()
}

fn db_y(r: Rect, db: f64) -> f64 {
    r.y0 + 10.0 + (db.clamp(FLOOR_DB, 0.0) / FLOOR_DB) * (r.height() - 20.0)
}

/// The part of the shape at `p`.
pub fn part_at(r: Rect, p: Point) -> Option<Part> {
    if !r.contains(p) {
        return None;
    }
    let t = x_t(r, p.x);
    Some(if t < 0.0 {
        Part::Ramp
    } else if t < f64::from(PROBE_BURST_MS) {
        Part::Burst
    } else {
        Part::Tail
    })
}

fn path(r: Rect, pts: &[(f32, f32)]) -> mite_ui::kurbo::BezPath {
    // One point per 1.5 px of the axis, at the gain the probe computed there.
    let mut out: Vec<Point> = Vec::with_capacity(400);
    let mut last_x = f64::NEG_INFINITY;
    for &(t, db) in pts {
        let x = t_x(r, f64::from(t));
        if f64::from(t) < T_MIN || x - last_x < 1.5 {
            continue;
        }
        out.push(Point::new(x, db_y(r, f64::from(db))));
        last_x = x;
    }
    monotone_path(&out)
}

pub fn draw(cx: &mut DrawCx<'_>, r: Rect, shape: &Shape, hot: Option<Part>) {
    let theme = cx.theme;
    panel(cx.scene, RoundedRect::from_rect(r, 8.0), theme.meter_well, theme.well_border);
    let (b0, b1) = (t_x(r, 0.0), t_x(r, f64::from(PROBE_BURST_MS)));
    // The burst: where the probe is over the ceiling.
    let burst = Rect::new(b0, r.y0 + 2.0, b1, r.y1 - 2.0);
    cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.spectrum_pre.multiply_alpha(0.10), None, &burst);
    // The part under the pointer, lit faintly.
    if let Some(part) = hot {
        let (x0, x1) = match part {
            Part::Ramp => (r.x0 + 2.0, b0),
            Part::Burst => (b0, b1),
            Part::Tail => (b1, r.x1 - 2.0),
        };
        cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.accent.multiply_alpha(0.10), None, &Rect::new(x0, r.y0 + 2.0, x1, r.y1 - 2.0));
    }
    let grid = Stroke::new(1.0);
    for db in [-3.0, -6.0] {
        cx.scene.stroke(&grid, Affine::IDENTITY, theme.grid_minor, None, &Line::new((r.x0 + 6.0, db_y(r, db)), (r.x1 - 6.0, db_y(r, db))));
    }
    // 0 dB, and the burst's edges (the lookahead ends at the first).
    cx.scene.stroke(&grid, Affine::IDENTITY, theme.grid_zero, None, &Line::new((r.x0 + 6.0, db_y(r, 0.0)), (r.x1 - 6.0, db_y(r, 0.0))));
    let dash = Stroke::new(1.0).with_dashes(0.0, [3.0, 3.0]);
    for x in [b0, b1] {
        cx.scene.stroke(&dash, Affine::IDENTITY, theme.text_lo, None, &Line::new((x, r.y0 + 6.0), (x, r.y1 - 6.0)));
    }
    if !shape.ghost.is_empty() {
        let ghost = path(r, &shape.ghost);
        cx.scene.stroke(&Stroke::new(1.4).with_dashes(0.0, [4.0, 3.0]), Affine::IDENTITY, theme.text_mid, None, &ghost);
    }
    if shape.points.is_empty() {
        return;
    }
    let edge = path(r, &shape.points);
    let gr = theme.gain_reduction;
    let mut fill = edge.clone();
    fill.line_to((r.x1 - 8.0, db_y(r, 0.0)));
    fill.line_to((r.x0 + 8.0, db_y(r, 0.0)));
    fill.close_path();
    let g = Gradient::new_linear((0.0, db_y(r, 0.0)), (0.0, db_y(r, FLOOR_DB))).with_stops([gr.multiply_alpha(0.35), gr.multiply_alpha(0.08)]);
    cx.scene.fill(Fill::NonZero, Affine::IDENTITY, &g, None, &fill);
    glow_stroke(cx, &edge, 1.8, gr, gr, 2.4);
}
