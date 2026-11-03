//! The scrolling level history shared by Mite Comp and Mite Limit (BLUEPRINT §7.3, "Waveform
//! history with a gain-reduction overlay"): the engine's records (one per slice of audio) binned
//! into columns fixed to their absolute times, scrolled with the clock between arrivals; its grid
//! and labels; the gain reduction hanging from the top.
//!
//! **Honest peaks (user, 2026-11-03):** a line or grip on the history means what it says: where
//! the drawn input touches the ceiling or a threshold, the limiter or compressor acts. So each
//! column keeps the true peak of its records (and the deepest gain reduction), and the curve
//! through them is monotone between columns (PCHIP): its highs and lows are the columns' own,
//! never above or below them. (Until then the columns were dB means eased across neighbours, so
//! drawn peaks sat several dB under the real ones.)
//!
//! The plugin keeps the records and decides what each lane shows; this draws them.

use std::collections::VecDeque;

use crate::kurbo::{Affine, BezPath, Line, Point, Rect, Stroke};
use crate::paint::glow_stroke;
use crate::peniko::{Color, Fill, Gradient};
use crate::{Align, DrawCx, TextStyle};

/// Records per column (20 ms at Comp's 5 ms slices).
const PER: usize = 4;

/// A smooth curve through `pts` (x increasing) that never overshoots them: monotone cubic
/// Hermite (Fritsch–Carlson tangents, PCHIP) as Béziers. Its extremes are the points' own, and
/// it has no corners (C¹).
pub fn monotone_path(pts: &[Point]) -> BezPath {
    let mut p = BezPath::new();
    let n = pts.len();
    if n == 0 {
        return p;
    }
    p.move_to(pts[0]);
    if n == 1 {
        return p;
    }
    let h: Vec<f64> = pts.windows(2).map(|w| (w[1].x - w[0].x).max(1e-9)).collect();
    let d: Vec<f64> = pts.windows(2).zip(&h).map(|(w, h)| (w[1].y - w[0].y) / h).collect();
    let mut m = vec![0.0; n];
    m[0] = d[0];
    m[n - 1] = d[n - 2];
    for k in 1..n - 1 {
        if d[k - 1] * d[k] > 0.0 {
            let (w1, w2) = (2.0 * h[k] + h[k - 1], h[k] + 2.0 * h[k - 1]);
            m[k] = (w1 + w2) / (w1 / d[k - 1] + w2 / d[k]);
        }
    }
    // The end tangents: no steeper than three times the chord (keeps the ends monotone).
    for (k, dk) in [(0, d[0]), (n - 1, d[n - 2])] {
        if m[k] * dk <= 0.0 {
            m[k] = 0.0;
        } else if m[k].abs() > 3.0 * dk.abs() {
            m[k] = 3.0 * dk;
        }
    }
    for k in 0..n - 1 {
        let (a, b) = (pts[k], pts[k + 1]);
        let t = h[k] / 3.0;
        p.curve_to(Point::new(a.x + t, a.y + m[k] * t), Point::new(b.x - t, b.y - m[k + 1] * t), b);
    }
    p
}

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

/// One column: its x, the highest of its records' fields (levels: their peak) and the lowest
/// (gain reduction: its deepest).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Column<const W: usize> {
    pub x: f64,
    pub high: [f32; W],
    pub low: [f32; W],
}

/// How far above 0 dB the history needs to reach to show `fields` of the records it shows (the
/// last `records`): the highest, plus a dB of air, in steps of 3 dB (0 when nothing goes over).
pub fn headroom_db<const W: usize>(history: &VecDeque<[f32; W]>, records: usize, fields: &[usize]) -> f32 {
    let high = history.iter().rev().take(records).flat_map(|r| fields.iter().map(move |&k| r[k])).fold(f32::MIN, f32::max);
    if high <= 0.0 { 0.0 } else { ((high + 1.0) / 3.0).ceil() * 3.0 }
}

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
        let mut cols: Vec<Column<W>> = Vec::with_capacity(last_col.saturating_sub(first_col) + 4);
        for col in first_col..last_col {
            let (a, b) = (col * PER, ((col + 1) * PER).min(seen));
            let x = self.x0 + self.width - (now - (a + b) as f64 / 2.0) * px_per_record;
            if x < self.x0 - 20.0 || a < first_abs {
                continue;
            }
            let mut c = Column { x, high: [f32::MIN; W], low: [f32::MAX; W] };
            for i in a..b {
                for (k, v) in history[i - first_abs].iter().enumerate() {
                    let v = v.max(-90.0);
                    c.high[k] = c.high[k].max(v);
                    c.low[k] = c.low[k].min(v);
                }
            }
            cols.push(c);
        }
        cols
    }

    /// The gridlines every 6 dB (a 24 dB range, plus −3) or 12 dB, their labels, the seconds and
    /// the 0 dB line. `range` is the range the history glides to (dB under 0), `headroom` how far
    /// above 0 it reaches, `level_y` its scale as drawn.
    pub fn draw_grid(&self, cx: &mut DrawCx<'_>, range: f32, headroom: f32, level_y: impl Fn(f32) -> f64) {
        let theme = cx.theme;
        let grid = theme.grid_minor;
        let x1 = self.x0 + self.width;
        let step = if range <= 24.5 { 6.0 } else { 12.0 };
        let mut levels: Vec<f32> = (1..).map(|k| step * k as f32).take_while(|&db| db < headroom - 0.5).collect();
        levels.reverse();
        levels.extend((1..).map(|k| -step * k as f32).take_while(|&db| db > -range + 0.5));
        // At 24 dB, −3 as well: the top few dB are where a mix bus lives.
        if range <= 24.5 {
            let at = levels.iter().position(|&db| db < 0.0).unwrap_or(levels.len());
            levels.insert(at, -3.0);
        }
        for &db in &levels {
            cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, grid, None, &Line::new((self.x0, level_y(db)), (x1, level_y(db))));
        }
        for s in 1..6 {
            let x = self.x0 + s as f64 * self.width / 6.0;
            cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, grid, None, &Line::new((x, self.top), (x, self.bottom)));
        }
        cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, theme.grid_zero, None, &Line::new((self.x0, level_y(0.0)), (x1, level_y(0.0))));
        for db in std::iter::once(0.0).chain(levels.iter().copied()) {
            let label = if db == 0.0 { "0".to_owned() } else if db > 0.0 { format!("+{db:.0}") } else { format!("−{:.0}", -db) };
            cx.text.draw(cx.scene, &label, self.x0 + 8.0, level_y(db) + 3.0, TextStyle { tabular: true, ..TextStyle::BODY.sized(10.0).weighted(600.0) }, theme.text_lo, Align::Left);
        }
    }
}

/// A line through the columns at `y(column)`, monotone between them (never past a column).
pub fn column_line<const W: usize>(cols: &[Column<W>], y: impl Fn(&Column<W>) -> f64) -> BezPath {
    let pts: Vec<Point> = cols.iter().map(|c| Point::new(c.x, y(c))).collect();
    monotone_path(&pts)
}

/// `path` closed down (or up) to `y` under the columns' span.
pub fn close_to<const W: usize>(mut path: BezPath, cols: &[Column<W>], y: f64) -> BezPath {
    if let (Some(first), Some(last)) = (cols.first(), cols.last()) {
        path.line_to((last.x, y));
        path.line_to((first.x, y));
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
        fill_path.line_to((last.x, top));
        fill_path.line_to((first.x, top));
        fill_path.close_path();
    }
    let fill = Gradient::new_linear((0.0, top), (0.0, top + fade_px)).with_stops([gr.multiply_alpha(0.4), gr.multiply_alpha(0.06)]);
    cx.scene.fill(Fill::NonZero, Affine::IDENTITY, &fill, None, &fill_path);
    glow_stroke(cx, edge, 1.8, gr, gr, 2.4);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lane() -> HistoryLane {
        HistoryLane { x0: 0.0, width: 600.0, top: 0.0, bottom: 400.0, seconds: 6.0, slice_ms: 5.0 }
    }

    #[test]
    fn a_column_keeps_the_true_peak_and_the_deepest_reduction() {
        let mut h: VecDeque<[f32; 2]> = VecDeque::new();
        for i in 0..400 {
            // Field 0: a level with one sharp peak; field 1: a gain reduction with one deep dip.
            h.push_back([if i == 201 { -1.0 } else { -20.0 }, if i == 202 { -9.0 } else { -1.0 }]);
        }
        let cols = lane().columns(&h, 400, 0.0);
        assert_eq!(cols.iter().map(|c| c.high[0]).fold(f32::MIN, f32::max), -1.0);
        assert_eq!(cols.iter().map(|c| c.low[1]).fold(f32::MAX, f32::min), -9.0);
    }

    #[test]
    fn the_curve_never_goes_past_its_points() {
        let pts: Vec<Point> = [(0.0, 50.0), (10.0, 10.0), (20.0, 48.0), (30.0, 47.0), (40.0, 5.0), (50.0, 60.0), (60.0, 60.0)].iter().map(|&(x, y)| Point::new(x, y)).collect();
        let path = monotone_path(&pts);
        let (lo, hi) = (5.0, 60.0);
        for seg in path.segments() {
            for k in 0..=50 {
                let y = mite_eval(seg, f64::from(k) / 50.0);
                assert!(y >= lo - 1e-9 && y <= hi + 1e-9, "{y}");
            }
        }
        // Between two points the curve stays between them.
        for (seg, w) in path.segments().zip(pts.windows(2)) {
            let (a, b) = (w[0].y.min(w[1].y), w[0].y.max(w[1].y));
            for k in 0..=50 {
                let y = mite_eval(seg, f64::from(k) / 50.0);
                assert!(y >= a - 1e-9 && y <= b + 1e-9, "{y} outside {a}…{b}");
            }
        }
    }

    fn mite_eval(seg: crate::kurbo::PathSeg, t: f64) -> f64 {
        use crate::kurbo::ParamCurve;
        seg.eval(t).y
    }

    #[test]
    fn headroom_reaches_over_what_goes_over() {
        let mut h: VecDeque<[f32; 1]> = VecDeque::new();
        h.push_back([-3.0]);
        assert_eq!(headroom_db(&h, 10, &[0]), 0.0);
        h.push_back([4.2]);
        assert_eq!(headroom_db(&h, 10, &[0]), 6.0);
    }
}
