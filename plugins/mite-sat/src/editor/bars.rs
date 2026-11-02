//! The harmonics bars: H2…H9 of a probe tone through the band's own code
//! (`mite_dsp::sat::probe_harmonics`) at the band's centre, the music's peak and its bias (the
//! operating point following the tone's envelope, as in the engine), in dB against the strongest
//! partial. A partial above the base Nyquist is hollow: the
//! curve makes it, the way down removes it. With no music the probe plays at a reference level and
//! the bars are dim: what the band would do, not what it is doing.

use std::time::Instant;

use mite_dsp::sat::{probe_harmonics, Harmonics, Model, Probe, PARTIALS};
use mite_ui::kurbo::{Affine, Point, Rect, RoundedRect, Stroke};
use mite_ui::paint::{glow_fill, panel};
use mite_ui::widgets::Help;
use mite_ui::{Align, DrawCx, TextStyle};

use super::curve::Curve;

/// The bars read from 0 dB at the top to this at the bottom.
const FLOOR_DB: f32 = -72.0;
/// Bars shown: H2…H9.
const BARS: usize = PARTIALS - 1;
/// The peak the probe plays at with no music, and the level under which there is none.
const REFERENCE: f32 = 0.25;
const SILENT: f32 = 1e-3;
/// A probe at most this often (they are cheap, but the UI thread has frames to draw).
const MIN_INTERVAL_S: f64 = 0.05;
/// The peak is read in steps of this many dB, so a steady signal stops re-measuring.
const PEAK_STEP_DB: f32 = 0.5;
/// The bars ease as the peaks do: up in 60 ms, down in 300 ms.
const RISE_S: f64 = 0.06;
const FALL_S: f64 = 0.3;
/// Inner padding, and the room for the numbers under the bars.
const PAD: f64 = 8.0;
const LABEL_H: f64 = 16.0;

/// Everything a measurement depends on.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Key {
    band: usize,
    model: Model,
    drive_db: f32,
    bias: f32,
    peak: i32,
    freq: f32,
    tone_db: f32,
    mix: f32,
    fs: f32,
}

pub struct ProbeBars {
    key: Option<Key>,
    last: Instant,
    target: [f32; BARS],
    hollow: [bool; BARS],
    /// As drawn, dB.
    level: [f32; BARS],
    /// 1 with music, dim without.
    live: f32,
    live_target: f32,
}

impl Default for ProbeBars {
    fn default() -> Self {
        Self { key: None, last: Instant::now(), target: [FLOOR_DB; BARS], hollow: [false; BARS], level: [FLOOR_DB; BARS], live: 0.5, live_target: 0.5 }
    }
}

impl ProbeBars {
    /// Measures again if what the bars depend on moved (at most every 50 ms; a new band or model
    /// at once), and eases them. True while they move.
    pub fn update(&mut self, band: usize, c: &Curve, tone_db: f32, mix: f32, fs: f32, dt: f64) -> bool {
        let silent = c.peak < SILENT;
        let amp = if silent { REFERENCE } else { c.peak.min(4.0) };
        let peak = (20.0 * amp.log10() / PEAK_STEP_DB).round() as i32;
        let key = Key { band, model: c.model, drive_db: c.drive_db, bias: c.bias, peak, freq: c.freq, tone_db, mix, fs };
        let snap = self.key.is_none_or(|k| k.band != band || k.model != c.model);
        if self.key != Some(key) && (snap || self.last.elapsed().as_secs_f64() >= MIN_INTERVAL_S) {
            let h = measure(&key);
            self.key = Some(key);
            self.last = Instant::now();
            for i in 0..BARS {
                self.target[i] = h.db(i + 1).max(FLOOR_DB);
                self.hollow[i] = h.hollow[i + 1];
            }
            self.live_target = if silent { 0.5 } else { 1.0 };
            if snap {
                self.level = self.target;
                self.live = self.live_target;
            }
        }
        let k = |s: f64| (1.0 - (-dt / s).exp()) as f32;
        let mut moved = false;
        for (l, &t) in self.level.iter_mut().zip(&self.target) {
            let d = t - *l;
            if d.abs() > 0.02 {
                *l += d * if d > 0.0 { k(RISE_S) } else { k(FALL_S) };
                moved = true;
            } else {
                *l = t;
            }
        }
        let d = self.live_target - self.live;
        if d.abs() > 0.005 {
            self.live += d * k(FALL_S);
            moved = true;
        } else {
            self.live = self.live_target;
        }
        moved
    }

    /// H2…H9 as drawn: dB, and hollow or not.
    pub fn shown(&self) -> [(f32, bool); BARS] {
        std::array::from_fn(|i| (self.level[i], self.hollow[i]))
    }

    pub fn help(&self, i: usize) -> Help {
        let name = format!("H{}", i + 2);
        let help = Help::default().slot(name, "H9").text(" ");
        let help = if self.level[i] <= FLOOR_DB { help.slot("below −72 dB".to_owned(), "below −72 dB") } else { help.slot(format!("−{:.1} dB", self.level[i].abs()), "−72.0 dB") };
        help.text(if self.hollow[i] {
            " — above Nyquist: the curve makes it, the way down removes it"
        } else if i % 2 == 0 {
            " — even. Measured by a probe tone through this band, against its strongest partial"
        } else {
            " — odd. Measured by a probe tone through this band, against its strongest partial"
        })
    }

    pub fn draw(&self, cx: &mut DrawCx<'_>, r: Rect, focus: Option<usize>, alpha: f32) {
        let theme = cx.theme;
        panel(cx.scene, RoundedRect::from_rect(r, 10.0), theme.well, theme.well_border);
        let plot = plot(r);
        let y = |db: f32| plot.y1 - f64::from((db - FLOOR_DB) / -FLOOR_DB).clamp(0.0, 1.0) * plot.height();
        for db in [-24.0, -48.0] {
            cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, theme.grid_minor, None, &mite_ui::kurbo::Line::new((plot.x0, y(db)), (plot.x1, y(db))));
        }
        let alpha = alpha * self.live;
        for i in 0..BARS {
            let slot = slot(plot, i);
            let (w, cxm) = ((slot.width() * 0.56).min(26.0), slot.center().x);
            let top = y(self.level[i]);
            let bar = RoundedRect::new(cxm - w / 2.0, top.min(plot.y1 - 1.0), cxm + w / 2.0, plot.y1, 2.0);
            let lit = focus == Some(i);
            if self.level[i] > FLOOR_DB + 0.5 {
                if self.hollow[i] {
                    cx.scene.stroke(&Stroke::new(1.2), Affine::IDENTITY, theme.accent.multiply_alpha(0.8 * alpha), None, &bar);
                } else {
                    glow_fill(cx, &bar, theme.accent.multiply_alpha(alpha), if lit { 8.0 } else { 3.0 });
                }
            }
            let ink = if lit { theme.text_hi } else { theme.text_lo };
            cx.text.draw(cx.scene, ["2", "3", "4", "5", "6", "7", "8", "9"][i], cxm, plot.y1 + 2.0, TextStyle { tabular: true, ..TextStyle::LABEL.sized(10.0) }, ink, Align::Center);
        }
    }
}

fn plot(r: Rect) -> Rect {
    Rect::new(r.x0 + PAD, r.y0 + PAD, r.x1 - PAD, r.y1 - PAD / 2.0 - LABEL_H)
}

fn slot(plot: Rect, i: usize) -> Rect {
    let w = plot.width() / BARS as f64;
    Rect::new(plot.x0 + w * i as f64, plot.y0, plot.x0 + w * (i + 1) as f64, plot.y1)
}

/// The partial under `p`, if any.
pub fn hit(r: Rect, p: Point) -> Option<usize> {
    if !r.contains(p) {
        return None;
    }
    let plot = plot(r);
    (0..BARS).find(|&i| {
        let s = slot(plot, i);
        p.x >= s.x0 && p.x < s.x1
    })
}

fn measure(k: &Key) -> Harmonics {
    let amp = 10f32.powf(k.peak as f32 * PEAK_STEP_DB / 20.0);
    probe_harmonics(&Probe { model: k.model, amp, drive_db: k.drive_db, bias: k.bias, freq: k.freq, tone_db: k.tone_db, pivot: k.freq, mix: k.mix, fs: k.fs })
}
