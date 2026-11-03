//! The spectrum: the input as a quiet filled shape, the output as a lit line, and the harmonics
//! the saturation adds (where the output stands above the input) filled in the accent.

use std::time::Instant;

use mite_dsp::analyzer::{spectrum_point_freq, Analyzer, LogSpectrum, SPECTRUM_POINTS};
use mite_ui::kurbo::{Affine, Line, Point, Rect, Stroke};
use mite_ui::paint::{glow_fill, glow_stroke};
use mite_ui::peniko::Fill;
use mite_ui::{Align, DrawCx, TextStyle};

use super::{smooth_path, PLOT_W, PLOT_X0};
use crate::SatShared;

pub use mite_dsp::analyzer::ANALYZER_SIZE;

/// The plot's levels at its top and bottom, dB on the analyzer's scale.
const TOP_DB: f32 = -6.0;
const BOTTOM_DB: f32 = -96.0;
const RISE_S: f64 = 0.06;
const FALL_S: f64 = 0.3;
/// The output must stand this far over the input before the gap counts as added, dB.
const ADDED_DB: f32 = 0.5;

pub fn freq_x(f: f64) -> f64 {
    PLOT_X0 + PLOT_W * (f / 20.0).ln() / 1000f64.ln()
}

pub struct Spectrum {
    analyzer: Analyzer,
    log: LogSpectrum,
    samples: Vec<f32>,
    power: Vec<f32>,
    targets: [Vec<f32>; 2],
    /// Input and output as drawn (eased: 60 ms up, 300 ms down).
    pub curves: [Vec<f32>; 2],
    last: Instant,
}

impl Spectrum {
    pub fn new() -> Self {
        let analyzer = Analyzer::new(ANALYZER_SIZE);
        let bins = analyzer.bins();
        Self {
            analyzer,
            log: LogSpectrum::new(ANALYZER_SIZE, 48_000.0),
            samples: vec![0.0; ANALYZER_SIZE],
            power: vec![0.0; bins],
            targets: [vec![-150.0; SPECTRUM_POINTS], vec![-150.0; SPECTRUM_POINTS]],
            curves: [vec![-150.0; SPECTRUM_POINTS], vec![-150.0; SPECTRUM_POINTS]],
            last: Instant::now(),
        }
    }

    /// Reads both taps (at most once per display refresh) and eases the curves towards them.
    /// Whether anything moved.
    pub fn update(&mut self, shared: &SatShared, dt: f64) -> bool {
        let fs = f64::from(shared.sample_rate.load()).max(1.0);
        if fs != self.log.sample_rate() {
            self.log = LogSpectrum::new(ANALYZER_SIZE, fs);
        }
        if self.last.elapsed().as_secs_f64() >= 0.0155 {
            self.last = Instant::now();
            for (tap, target) in [&shared.spectrum, &shared.post].into_iter().zip(self.targets.iter_mut()) {
                tap.read_latest(&mut self.samples);
                if self.samples.iter().all(|s| s.abs() < 1e-7) {
                    target.iter_mut().for_each(|t| *t = -150.0);
                } else {
                    self.analyzer.analyze_power(&self.samples, &mut self.power);
                    self.log.read(&self.power, target);
                }
            }
        }
        let rise = (1.0 - (-dt / RISE_S).exp()) as f32;
        let fall = (1.0 - (-dt / FALL_S).exp()) as f32;
        let mut moved = false;
        for (curve, target) in self.curves.iter_mut().zip(&self.targets) {
            for (v, &t) in curve.iter_mut().zip(target) {
                let next = *v + (t - *v) * if t > *v { rise } else { fall };
                moved |= (next - *v).abs() > 0.02;
                *v = next;
            }
        }
        moved
    }

    /// Draws the plot in `area`; `tints` are regions (the bands') filled in the accent at their
    /// opacity under the curves.
    pub fn draw(&self, cx: &mut DrawCx<'_>, area: Rect, tints: &[(Rect, f32)]) {
        let theme = cx.theme;
        cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.well, None, &area);
        for &(r, op) in tints {
            cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.accent.multiply_alpha(op), None, &r.intersect(area));
        }
        let y = |db: f32| area.y0 + 4.0 + f64::from((TOP_DB - db.clamp(BOTTOM_DB, TOP_DB)) / (TOP_DB - BOTTOM_DB)) * (area.height() - 8.0);
        for db in [-30.0, -60.0] {
            cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, theme.grid_minor, None, &Line::new((area.x0, y(db)), (area.x1, y(db))));
        }
        for f in [100.0, 1000.0, 10_000.0] {
            cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, theme.grid_minor, None, &Line::new((freq_x(f), area.y0), (freq_x(f), area.y1)));
        }
        for f in [50.0, 200.0, 500.0, 2000.0, 5000.0] {
            cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, theme.grid_minor.multiply_alpha(0.5), None, &Line::new((freq_x(f), area.y0), (freq_x(f), area.y1)));
        }
        let label = TextStyle { tabular: true, ..TextStyle::BODY.sized(10.0).weighted(600.0) };
        for (f, s) in [(100.0, "100"), (1000.0, "1k"), (10_000.0, "10k")] {
            cx.text.draw(cx.scene, s, freq_x(f) + 4.0, area.y1 - 15.0, label, theme.text_lo, Align::Left);
        }
        let [pre, post] = &self.curves;
        if !pre.iter().chain(post).any(|&v| v > BOTTOM_DB) {
            return;
        }
        // Only the points inside 20 Hz–20 kHz: the glow layer is not clipped by the plot.
        let inside: Vec<usize> = (0..SPECTRUM_POINTS).filter(|&i| (20.0..=20_000.0).contains(&spectrum_point_freq(i))).collect();
        let xs: Vec<f64> = inside.iter().map(|&i| freq_x(spectrum_point_freq(i))).collect();
        let at = |v: &[f32]| -> Vec<Point> { xs.iter().zip(&inside).map(|(&x, &i)| Point::new(x, y(v[i]))).collect() };
        cx.begin_clip(&area);
        // The input: a quiet filled shape.
        let pre_pts = at(pre);
        let pre_curve = smooth_path(&pre_pts);
        let mut fill = pre_curve.clone();
        fill.line_to((pre_pts[pre_pts.len() - 1].x, area.y1));
        fill.line_to((pre_pts[0].x, area.y1));
        fill.close_path();
        cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.spectrum_pre.multiply_alpha(0.14), None, &fill);
        cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, theme.spectrum_pre.multiply_alpha(0.55), None, &pre_curve);
        // What the saturation adds: the gap where the output stands above the input.
        let added: Vec<f32> = pre.iter().zip(post).map(|(&a, &b)| a + (b - a - ADDED_DB).max(0.0)).collect();
        if inside.iter().any(|&i| added[i] - pre[i] > 0.05) {
            let mut gap = smooth_path(&at(&added));
            for p in pre_pts.iter().rev() {
                gap.line_to(*p);
            }
            gap.close_path();
            cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.accent.multiply_alpha(0.22), None, &gap);
            glow_fill(cx, &gap, theme.accent.multiply_alpha(0.25), 6.0);
        }
        // The output: a lit line.
        if post.iter().any(|&v| v > BOTTOM_DB) {
            glow_stroke(cx, &smooth_path(&at(post)), 1.6, theme.curve, theme.curve, 2.4);
        }
        cx.end_clip();
    }
}
