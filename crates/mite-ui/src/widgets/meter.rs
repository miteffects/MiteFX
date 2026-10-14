//! Segmented peak meter (component sheet "Peak + RMS · hot above −6", EQ output meter).
//!
//! Each region (off / lit / hot) is a single rectangle filled with a repeating 1×4 px segment
//! pattern, and the glow is one soft blur behind the lit bar. Drawing every 2 px segment as its own
//! path (plus three glow strokes each) made the meter the most expensive thing on screen to encode.

use std::sync::Arc;
use std::time::{Duration, Instant};

use vello::kurbo::{Affine, Rect, RoundedRect, Stroke};
use vello::peniko::{Blob, Color, Extend, Fill, ImageAlphaType, ImageBrush, ImageData, ImageFormat, ImageQuality};

use crate::view::DrawCx;

const FLOOR_DB: f32 = -60.0;
const HOT_DB: f32 = -6.0;
const SEGMENT: f64 = 2.0;
const PITCH: f64 = 4.0;
const BAR_W: f64 = 10.0;
const BAR_GAP: f64 = 6.0;
/// tokens.json `meter.peakHoldMs`.
const PEAK_HOLD: Duration = Duration::from_millis(1500);

/// Repeating segment patterns, one per colour, built once per colour.
#[derive(Debug, Clone, Default)]
struct Patterns {
    cache: Vec<(Color, ImageBrush)>,
}

impl Patterns {
    fn get(&mut self, color: Color) -> ImageBrush {
        if let Some((_, brush)) = self.cache.iter().find(|(c, _)| *c == color) {
            return brush.clone();
        }
        // One pixel wide, one pitch tall: gap (2 px) then segment (2 px), so the pattern anchored
        // at `bottom − PITCH` puts a segment flush with the bottom of the bar.
        let [r, g, b, a] = color.to_rgba8().to_u8_array();
        let rows = [[0u8, 0, 0, 0], [0, 0, 0, 0], [r, g, b, a], [r, g, b, a]];
        let data: Vec<u8> = rows.iter().flatten().copied().collect();
        let image = ImageData {
            data: Blob::new(Arc::new(data)),
            format: ImageFormat::Rgba8,
            alpha_type: ImageAlphaType::Alpha,
            width: 1,
            height: PITCH as u32,
        };
        let brush = ImageBrush::new(image).with_extend(Extend::Repeat).with_quality(ImageQuality::Low);
        self.cache.push((color, brush.clone()));
        brush
    }
}

/// Stereo level meter inside a recessed well. `rect` is the well.
#[derive(Debug, Clone)]
pub struct LevelMeter {
    pub rect: Rect,
    levels_db: [f32; 2],
    /// Peak-hold level and when it was set, per channel.
    holds: [(f32, Instant); 2],
    patterns: Patterns,
}

impl LevelMeter {
    pub fn new(rect: Rect) -> Self {
        let now = Instant::now();
        Self { rect, levels_db: [FLOOR_DB; 2], holds: [(FLOOR_DB, now); 2], patterns: Patterns::default() }
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

    pub fn draw(&mut self, cx: &mut DrawCx<'_>) {
        let theme = cx.theme;
        let well = RoundedRect::from_rect(self.rect, 6.0);
        cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.meter_well, None, &well);
        cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, theme.well_border, None, &well);

        let inner = self.rect.inset(-5.0);
        let rows = (inner.height() / PITCH).floor() as usize;
        let bottom = inner.y1;
        let row_top = |row: usize| bottom - row as f64 * PITCH;
        let hot_row = row_for(HOT_DB, rows);
        let bars_w = 2.0 * BAR_W + BAR_GAP;
        let x0 = inner.center().x - bars_w / 2.0;
        let anchor = |x: f64| Some(Affine::translate((x, bottom - PITCH)));

        for (i, level) in self.levels_db.into_iter().enumerate() {
            let x = x0 + i as f64 * (BAR_W + BAR_GAP);
            // Whole lit segments, plus how far the level reaches into the next one. Drawing that
            // partial segment proportionally makes the meter glide instead of stepping 4 px.
            let exact = rows_exact(level, rows);
            let lit_rows = exact.floor() as usize;
            let partial = (exact - exact.floor()) as f32;
            let lit_top = row_top(lit_rows);
            let cool_top = row_top(lit_rows.min(hot_row));

            let off = Rect::new(x, row_top(rows), x + BAR_W, lit_top);
            let brush = self.patterns.get(theme.meter_off);
            cx.scene.fill(Fill::NonZero, Affine::IDENTITY, &brush, anchor(x), &off);

            if lit_rows > 0 {
                if theme.shadows {
                    let glow = Rect::new(x, lit_top, x + BAR_W, bottom);
                    cx.scene.draw_blurred_rounded_rect(Affine::IDENTITY, glow, theme.accent.multiply_alpha(0.35), 2.0, 2.4);
                }
                let lit = Rect::new(x, cool_top, x + BAR_W, bottom);
                let brush = self.patterns.get(theme.accent);
                cx.scene.fill(Fill::NonZero, Affine::IDENTITY, &brush, anchor(x), &lit);
                if lit_rows > hot_row {
                    let hot = Rect::new(x, lit_top, x + BAR_W, cool_top);
                    let brush = self.patterns.get(theme.warn);
                    cx.scene.fill(Fill::NonZero, Affine::IDENTITY, &brush, anchor(x), &hot);
                }
            }
            if lit_rows < rows && partial > 0.0 {
                let y = row_top(lit_rows + 1) + (PITCH - SEGMENT);
                let color = if lit_rows >= hot_row { theme.warn } else { theme.accent };
                cx.scene.fill(Fill::NonZero, Affine::IDENTITY, color.multiply_alpha(partial), None, &Rect::new(x, y, x + BAR_W, y + SEGMENT));
            }
            let hold_rows = row_for(self.holds[i].0, rows);
            if hold_rows > 0 {
                let y = row_top(hold_rows) + (PITCH - SEGMENT);
                cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.needle, None, &Rect::new(x, y, x + BAR_W, y + SEGMENT));
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
