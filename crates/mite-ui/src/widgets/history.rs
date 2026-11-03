//! The scrolling level history shared by Mite Comp and Mite Limit (BLUEPRINT §7.3, "Waveform
//! history with a gain-reduction overlay"): the engine's records (one per slice of audio) binned
//! into columns fixed to their absolute times, eased across neighbours so noise reads as a level
//! envelope, scrolled with the clock between arrivals; its grid and labels; the gain reduction
//! hanging from the top.
//!
//! The plugin keeps the records and decides what each lane shows; this draws them.

use std::collections::VecDeque;

use crate::kurbo::{Affine, BezPath, Line, Point, Rect, Stroke};
use crate::paint::glow_stroke;
use crate::peniko::{Color, Fill, Gradient};
use crate::{Align, DrawCx, TextStyle};

/// Records per column (20 ms at Comp's 5 ms slices).
const PER: usize = 4;

/// A smooth curve through `pts` (Catmull-Rom as cubic Béziers): no corners.
pub fn smooth_path(pts: &[Point]) -> BezPath {
    let mut p = BezPath::new();
    if pts.is_empty() {
        return p;
    }
    p.move_to(pts[0]);
    for i in 0..pts.len().saturating_sub(1) {
        let p0 = pts[i.saturating_sub(1)];
        let (p1, p2) = (pts[i], pts[i + 1]);
        let p3 = pts[(i + 2).min(pts.len() - 1)];
        let c1 = p1 + (p2 - p0) / 6.0;
        let c2 = p2 - (p3 - p1) / 6.0;
        p.curve_to(c1, c2, p2);
    }
    p
}

/// Where the history sits and how fast it scrolls.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HistoryLane {
    /// The lane: `x0` to `x0 + width`, `top` (0 dB is 24 px under it) to `bottom`.
    pub x0: f64,
    pub width: f64,
    pub top: f64,
    pub bottom: f64,
    /// Seconds across the width, and the length of one record.
    pub seconds: f64,
    pub slice_ms: f32,
}

/// One column: its x and the mean of its records.
pub type Column<const W: usize> = (f64, [f32; W]);

impl HistoryLane {
    /// The 0 dB line.
    pub fn zero_y(&self) -> f64 {
        self.top + 24.0
    }

    pub fn rect(&self) -> Rect {
        Rect::new(self.x0, self.top, self.x0 + self.width, self.bottom)
    }

    /// The columns on screen. `seen` counts every record ever read (the newest is `seen − 1`),
    /// `since_fresh` how many records' time has passed since the newest arrived (so the history
    /// moves at the display's rate however the host's blocks fall).
    pub fn columns<const W: usize>(&self, history: &VecDeque<[f32; W]>, seen: usize, since_fresh: f64) -> Vec<Column<W>> {
        let px_per_record = self.width / (self.seconds * 1000.0 / f64::from(self.slice_ms));
        let total = history.len();
        let first_abs = seen.wrapping_sub(total);
        let now = seen as f64 + since_fresh;
        let first_col = first_abs.div_ceil(PER);
        let last_col = seen.div_ceil(PER);
        let mut raw: Vec<Column<W>> = Vec::with_capacity(last_col.saturating_sub(first_col) + 4);
        for col in first_col..last_col {
            let (a, b) = (col * PER, ((col + 1) * PER).min(seen));
            let x = self.x0 + self.width - (now - (a + b) as f64 / 2.0) * px_per_record;
            if x < self.x0 - 20.0 {
                continue;
            }
            let mut out = [0.0f32; W];
            for i in a..b {
                for (o, v) in out.iter_mut().zip(&history[i - first_abs]) {
                    *o += v.max(-90.0);
                }
            }
            for o in &mut out {
                *o /= (b - a) as f32;
            }
            raw.push((x, out));
        }
        (0..raw.len())
            .map(|i| {
                let (a, b) = (raw[i.saturating_sub(1)].1, raw[(i + 1).min(raw.len() - 1)].1);
                let mut r = raw[i].1;
                for (k, v) in r.iter_mut().enumerate() {
                    *v = 0.25 * a[k] + 0.5 * *v + 0.25 * b[k];
                }
                (raw[i].0, r)
            })
            .collect()
    }

    /// The gridlines every 6 dB (a 24 dB range, plus −3) or 12 dB, their labels, the seconds and
    /// the 0 dB line. `range` is the range the history glides to, `level_y` its scale as drawn.
    pub fn draw_grid(&self, cx: &mut DrawCx<'_>, range: f32, level_y: impl Fn(f32) -> f64) {
        let theme = cx.theme;
        let grid = theme.grid_minor;
        let x1 = self.x0 + self.width;
        let step = if range <= 24.5 { 6.0 } else { 12.0 };
        let mut levels: Vec<f32> = (1..).map(|k| -step * k as f32).take_while(|&db| db > -range + 0.5).collect();
        // At 24 dB, −3 as well: the top few dB are where a mix bus lives.
        if range <= 24.5 {
            levels.insert(0, -3.0);
        }
        for &db in &levels {
            cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, grid, None, &Line::new((self.x0, level_y(db)), (x1, level_y(db))));
        }
        for s in 1..6 {
            let x = self.x0 + s as f64 * self.width / 6.0;
            cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, grid, None, &Line::new((x, self.top), (x, self.bottom)));
        }
        cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, theme.grid_zero, None, &Line::new((self.x0, self.zero_y()), (x1, self.zero_y())));
        for db in std::iter::once(0.0).chain(levels.iter().copied()) {
            let label = if db == 0.0 { "0".to_owned() } else { format!("−{:.0}", -db) };
            cx.text.draw(cx.scene, &label, self.x0 + 8.0, level_y(db) + 3.0, TextStyle { tabular: true, ..TextStyle::BODY.sized(10.0).weighted(600.0) }, theme.text_lo, Align::Left);
        }
    }
}

/// A smooth line through the columns at `y(record)`.
pub fn column_line<const W: usize>(cols: &[Column<W>], y: impl Fn(&[f32; W]) -> f64) -> BezPath {
    let pts: Vec<Point> = cols.iter().map(|(x, r)| Point::new(*x, y(r))).collect();
    smooth_path(&pts)
}

/// `path` closed down (or up) to `y` under the columns' span.
pub fn close_to<const W: usize>(mut path: BezPath, cols: &[Column<W>], y: f64) -> BezPath {
    if let (Some(first), Some(last)) = (cols.first(), cols.last()) {
        path.line_to((last.0, y));
        path.line_to((first.0, y));
        path.close_path();
    }
    path
}

/// How a level lane looks: its fill (a vertical gradient from `fill[0]` at `span.0` to `fill[1]`
/// at `span.1`) and its edge (`glow` 0: a plain stroke).
#[derive(Debug, Clone, Copy)]
pub struct LevelLook {
    pub fill: [Color; 2],
    pub span: (f64, f64),
    pub edge: Color,
    pub width: f64,
    pub glow: f64,
}

/// A level lane filled down to `bottom` under its edge.
pub fn draw_level<const W: usize>(cx: &mut DrawCx<'_>, cols: &[Column<W>], edge: &BezPath, bottom: f64, look: LevelLook) {
    let shape = close_to(edge.clone(), cols, bottom);
    let g = Gradient::new_linear((0.0, look.span.0), (0.0, look.span.1)).with_stops(look.fill);
    cx.scene.fill(Fill::NonZero, Affine::IDENTITY, &g, None, &shape);
    if look.glow > 0.0 {
        glow_stroke(cx, edge, look.width, look.edge, look.edge, look.glow);
    } else {
        cx.scene.stroke(&Stroke::new(look.width), Affine::IDENTITY, look.edge, None, edge);
    }
}

/// Gain reduction hanging from `top`: light, fading with depth (to `fade_px`), so deep
/// reduction does not bury the levels under it; its edge lit.
pub fn draw_reduction<const W: usize>(cx: &mut DrawCx<'_>, cols: &[Column<W>], edge: &BezPath, top: f64, fade_px: f64) {
    let gr = cx.theme.gain_reduction;
    let mut fill_path = edge.clone();
    if let (Some(first), Some(last)) = (cols.first(), cols.last()) {
        fill_path.line_to((last.0, top));
        fill_path.line_to((first.0, top));
        fill_path.close_path();
    }
    let fill = Gradient::new_linear((0.0, top), (0.0, top + fade_px)).with_stops([gr.multiply_alpha(0.4), gr.multiply_alpha(0.06)]);
    cx.scene.fill(Fill::NonZero, Affine::IDENTITY, &fill, None, &fill_path);
    glow_stroke(cx, edge, 1.8, gr, gr, 2.4);
}
