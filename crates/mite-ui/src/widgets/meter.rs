//! Segmented peak meter (component sheet "Peak + RMS · hot above −6", EQ output meter).

use std::time::{Duration, Instant};

use vello::kurbo::{Affine, BezPath, Rect, RoundedRect, Shape as _, Stroke};
use vello::peniko::Fill;

use crate::paint::glow_fill;
use crate::view::DrawCx;

const FLOOR_DB: f32 = -60.0;
const HOT_DB: f32 = -6.0;
const SEGMENT: f64 = 2.0;
const PITCH: f64 = 4.0;
const BAR_W: f64 = 10.0;
const BAR_GAP: f64 = 6.0;
/// tokens.json `meter.peakHoldMs`.
const PEAK_HOLD: Duration = Duration::from_millis(1500);

/// Stereo level meter inside a recessed well. `rect` is the well.
#[derive(Debug, Clone)]
pub struct LevelMeter {
    pub rect: Rect,
    levels_db: [f32; 2],
    /// Peak-hold level and when it was set, per channel.
    holds: [(f32, Instant); 2],
}

impl LevelMeter {
    pub fn new(rect: Rect) -> Self {
        let now = Instant::now();
        Self { rect, levels_db: [FLOOR_DB; 2], holds: [(FLOOR_DB, now); 2] }
    }

    /// Updates the levels; returns `true` if the drawn meter changes.
    pub fn set_levels(&mut self, left_db: f32, right_db: f32) -> bool {
        // 0.01 dB steps: the top segment is drawn fractionally, so small moves are visible.
        let quantise = |db: f32| ((db.max(FLOOR_DB) - FLOOR_DB) * 100.0).round();
        let now = Instant::now();
        let mut changed = false;
        for (i, level) in [left_db, right_db].into_iter().enumerate() {
            changed |= quantise(level) != quantise(self.levels_db[i]);
            self.levels_db[i] = level;
            let (held, at) = self.holds[i];
            if level >= held || now.duration_since(at) > PEAK_HOLD {
                changed |= quantise(level) != quantise(held);
                self.holds[i] = (level, now);
            }
        }
        changed
    }

    pub fn draw(&self, cx: &mut DrawCx<'_>) {
        let theme = cx.theme;
        let well = RoundedRect::from_rect(self.rect, 6.0);
        cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.meter_well, None, &well);
        cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, theme.well_border, None, &well);

        let inner = self.rect.inset(-5.0);
        let bars_w = 2.0 * BAR_W + BAR_GAP;
        let x0 = inner.center().x - bars_w / 2.0;
        let rows = (inner.height() / PITCH).floor() as usize;
        let hot_row = row_for(HOT_DB, rows);

        for (i, level) in self.levels_db.into_iter().enumerate() {
            let x = x0 + i as f64 * (BAR_W + BAR_GAP);
            // Whole lit segments, plus how far the level reaches into the next one. Drawing that
            // partial segment proportionally makes the meter glide instead of stepping 4 px at a
            // time (at 20 dB/s release a whole segment is ~60 ms).
            let exact = rows_exact(level, rows);
            let lit_rows = exact.floor() as usize;
            let partial = (exact - exact.floor()) as f32;
            let mut off = BezPath::new();
            let mut lit = BezPath::new();
            let mut hot = BezPath::new();
            for row in 0..rows {
                let y = inner.y1 - (row as f64 + 1.0) * PITCH + (PITCH - SEGMENT);
                let seg = Rect::new(x, y, x + BAR_W, y + SEGMENT);
                let path = if row >= lit_rows {
                    &mut off
                } else if row >= hot_row {
                    &mut hot
                } else {
                    &mut lit
                };
                path.extend(seg.path_elements(0.1));
            }
            cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.meter_off, None, &off);
            glow_fill(cx.scene, &lit, theme.accent, 1.2);
            glow_fill(cx.scene, &hot, theme.warn, 1.2);
            if lit_rows < rows && partial > 0.0 {
                let y = inner.y1 - (lit_rows as f64 + 1.0) * PITCH + (PITCH - SEGMENT);
                let seg = Rect::new(x, y, x + BAR_W, y + SEGMENT);
                let color = if lit_rows >= hot_row { theme.warn } else { theme.accent };
                cx.scene.fill(Fill::NonZero, Affine::IDENTITY, color.multiply_alpha(partial), None, &seg);
            }
            let hold_rows = row_for(self.holds[i].0, rows);
            if hold_rows > 0 {
                let y = inner.y1 - hold_rows as f64 * PITCH + (PITCH - SEGMENT);
                let peak = Rect::new(x, y, x + BAR_W, y + SEGMENT);
                glow_fill(cx.scene, &peak, theme.needle, 1.2);
            }
        }
    }
}

fn rows_exact(db: f32, rows: usize) -> f64 {
    let t = ((db.max(FLOOR_DB) - FLOOR_DB) / -FLOOR_DB).clamp(0.0, 1.0);
    f64::from(t) * rows as f64
}

fn row_for(db: f32, rows: usize) -> usize {
    rows_exact(db, rows).round() as usize
}
