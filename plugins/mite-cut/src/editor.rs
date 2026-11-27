//! Mite Cut's editor: the session's drum tracks as lanes on one timeline.
//!
//! - **Lanes:** each track's waveform, lit where it is kept and dim where it is cut (the cut is
//!   the one that plays: the curve the audio thread reads), with a mark at every hit found:
//!   accent where it is kept, dim where it is cut, warning where it is close to the threshold
//!   (those are the ones to check). The threshold is drawn across the waveform: a hit must
//!   peak over it in its own drum (less what of it is bleed); THRESH moves it. A click on a mark keeps or cuts that hit; a double-click puts it
//!   back to what the detection says; a double-click on the lane where no hit was found adds
//!   one there.
//! - **Tracks:** the name (a click selects the track for the dials) and the role chip (a click
//!   moves it on, Shift-click back): Kick, Snare, Tom, Hat, Cymbal are cut and judged against
//!   each other; OH and Room are never cut; Off passes.
//! - **Timeline:** the wheel zooms around the pointer, a drag pans, a double-click on the ruler
//!   shows the whole song; the host's playhead runs across.
//! - **Dials:** the selected track's tuning (sensitivity, bleed, pre-roll, hold, where a hit
//!   ends, fade out, floor). **Footer:** Listen Removed (only what is cut) and Bypass.
//! - **Export:** every cut track as a WAV file, for any DAW.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use mite_core::{ParamDisplay, ParamScale, ParamSpec};
use mite_dsp::hits::{Role, Settings};
use mite_ui::kurbo::{Affine, BezPath, Line, Point, Rect, RoundedRect, Stroke};
use mite_ui::peniko::{Color, Fill};
use mite_ui::widgets::brand::{draw_mark, draw_title};
use mite_ui::widgets::footer::{Footer, FooterDial, FooterToggle, ToggleGlyph};
use mite_ui::widgets::{Help, KnobKind};
use mite_ui::{Align, DrawCx, MemoryParamHost, MouseButton, ParamHost, TextStyle, UiEvent, View};

use crate::CutShared;
use crate::params::{BYPASS, REMOVED};
use crate::session::{Clip, Export, OverrideState, PEAK_HOP, Status, Track};

/// Cut is a gate at heart: it wears the Gate accent.
pub const ACCENT: &str = "gate";
pub const WIDTH: f64 = 1100.0;
pub const HEIGHT: f64 = 680.0;
const HEADER: f64 = 48.0;
const FOOTER: f64 = 56.0;
const DIALS: f64 = 60.0;
const RULER: f64 = 26.0;
/// The tracks' column, and the lanes right of it.
const NAMES_W: f64 = 214.0;
const LANES_X: f64 = NAMES_W + 8.0;
const LANES_X1: f64 = WIDTH - 16.0;
const LANES_TOP: f64 = HEADER + RULER;
const LANES_BOTTOM: f64 = HEIGHT - FOOTER - DIALS;
const LANE_MIN: f64 = 44.0;
const LANE_MAX: f64 = 110.0;
const CHIP_W: f64 = 70.0;
/// The export button, in the header.
const EXPORT: Rect = Rect::new(WIDTH - 126.0, 10.0, WIDTH - 16.0, 38.0);
/// A hit mark is this close to the pointer to be under it, px.
const MARK_REACH: f64 = 6.0;
/// Marks are drawn when the kept hits in view are at least this far apart on average, px.
const MARK_SPACING: f64 = 4.0;

#[allow(clippy::too_many_arguments)] // one line per dial reads best
const fn spec(id: &'static str, name: &'static str, unit: &'static str, min: f32, max: f32, default: f32, scale: ParamScale, decimals: u8, display: ParamDisplay) -> ParamSpec {
    ParamSpec { id, name, unit, min, max, default, scale, smoothing_ms: 0.0, decimals, wheel_step: 0.25, wheel_step_fine: 0.05, display }
}

// The selected track's tuning (`mite_dsp::hits::Settings`), on the editor's own dials: they
// live with the track in the document, not in the host's parameters.
const THRESH: ParamSpec = spec("threshold", "Threshold", "dB", -100.0, 0.0, -40.0, ParamScale::Linear, 1, ParamDisplay::Number);
const BLEED: ParamSpec = spec("bleed", "Bleed", "%", 0.0, 100.0, 50.0, ParamScale::Linear, 0, ParamDisplay::Number);
const PRE_ROLL: ParamSpec = spec("pre_roll", "Pre-roll", "ms", 0.0, 20.0, 2.0, ParamScale::Linear, 1, ParamDisplay::Time);
const HOLD: ParamSpec = spec("hold", "Hold", "ms", 0.0, 250.0, 10.0, ParamScale::Linear, 0, ParamDisplay::Time);
const RELEASE: ParamSpec = spec("release", "Ends", "dB", 0.0, 24.0, 6.0, ParamScale::Linear, 1, ParamDisplay::Number);
const FADE_OUT: ParamSpec = spec("fade_out", "Fade out", "ms", 1.0, 500.0, 40.0, ParamScale::Log, 0, ParamDisplay::Time);
const FLOOR: ParamSpec = spec("floor", "Floor", "dB", -80.0, 0.0, -80.0, ParamScale::Linear, 1, ParamDisplay::Number);
const DIAL_SPECS: [ParamSpec; 7] = [THRESH, BLEED, PRE_ROLL, HOLD, RELEASE, FADE_OUT, FLOOR];

/// The dials' settings. `auto`: the automatic threshold the THRESH dial was put on (the track
/// has none set by hand); the dial still there, it stays automatic.
fn settings_of(h: &MemoryParamHost, auto: Option<f32>) -> Settings {
    let p = |s: &ParamSpec| h.plain(s.id, h.normalized(s.id));
    let thr = p(&THRESH);
    let threshold_db = match auto {
        Some(a) if (thr - a).abs() < 0.05 => None,
        _ => Some(thr),
    };
    Settings { threshold_db, bleed: p(&BLEED) / 100.0, pre_roll_ms: p(&PRE_ROLL), hold_ms: p(&HOLD), release_db: p(&RELEASE), fade_out_ms: p(&FADE_OUT), floor_db: p(&FLOOR), ..Settings::default() }
}

/// Puts `s` on the dials, THRESH on `threshold` (what the track's hits read now); returns the
/// automatic threshold shown, if the track has none set by hand.
fn load_settings(h: &MemoryParamHost, s: &Settings, threshold: f32) -> Option<f32> {
    h.set_plain(THRESH.id, s.threshold_db.unwrap_or(threshold).clamp(THRESH.min, THRESH.max));
    h.set_plain(BLEED.id, s.bleed * 100.0);
    h.set_plain(PRE_ROLL.id, s.pre_roll_ms);
    h.set_plain(HOLD.id, s.hold_ms);
    h.set_plain(RELEASE.id, s.release_db);
    h.set_plain(FADE_OUT.id, s.fade_out_ms);
    h.set_plain(FLOOR.id, s.floor_db.max(FLOOR.min));
    s.threshold_db.is_none().then(|| h.plain(THRESH.id, h.normalized(THRESH.id)))
}

/// Settings equal but for a dial's rounding.
fn alike(a: &Settings, b: &Settings) -> bool {
    let v = |s: &Settings| [s.bleed, s.pre_roll_ms, s.hold_ms, s.release_db, s.fade_out_ms, s.floor_db.max(-80.0), s.threshold_db.unwrap_or(1.0)];
    a.threshold_db.is_some() == b.threshold_db.is_some() && v(a).iter().zip(v(b)).all(|(x, y)| (x - y).abs() <= 1e-3 * (1.0 + x.abs()))
}

/// The plugin's parameters as a host in memory holds them (tests, snapshots): its two
/// switches, 0 or 1.
pub fn memory_host() -> MemoryParamHost {
    let switch = |b: mite_core::BoolSpec| ParamSpec { id: b.id, name: b.name, unit: "", min: 0.0, max: 1.0, default: if b.default { 1.0 } else { 0.0 }, scale: ParamScale::Linear, smoothing_ms: 0.0, decimals: 0, wheel_step: 1.0, wheel_step_fine: 1.0, display: ParamDisplay::Number };
    MemoryParamHost::new().with(switch(BYPASS)).with(switch(REMOVED))
}


/// Where the THRESH dial is (the first of the dials; as `Footer::new` lays them out).
fn thresh_dial() -> Rect {
    let x = WIDTH - 84.0 - 92.0 * (DIAL_SPECS.len() - 1) as f64;
    let y = HEIGHT - FOOTER - DIALS + 28.0;
    Rect::new(x - 20.0, y - 20.0, x + 20.0, y + 20.0)
}

/// The next role (or the one before), as a click on the chip moves it.
pub fn next_role(r: Role, back: bool) -> Role {
    let all = Role::ALL;
    let i = all.iter().position(|x| *x == r).unwrap_or(0);
    let n = all.len();
    all[if back { (i + n - 1) % n } else { (i + 1) % n }]
}

/// What the pointer is over.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Hot {
    None,
    /// Track `row`'s name, or its role chip.
    Name(usize),
    Chip(usize),
    /// Hit `hit` of clip `clip` on track `row`.
    Mark(usize, usize, usize),
    /// Track `row`'s lane, at song time `t`.
    Lane(usize, f64),
    Ruler,
    Export,
}

pub struct CutView {
    shared: Arc<CutShared>,
    seen: u64,
    /// The editor's dials, the host's toggles.
    local: MemoryParamHost,
    dials: Footer,
    footer: Footer,
    /// The selected track (its sequence), and the settings last written to it.
    selected: Option<u64>,
    shown_settings: Option<(u64, Settings)>,
    /// The automatic threshold THRESH shows (the selected track has none set by hand).
    auto_shown: Option<f32>,
    /// Song time across the lanes, s; `None` until there is a song (then the whole of it).
    view: Option<(f64, f64)>,
    first_row: usize,
    hot: Hot,
    /// A pan in progress: where it started (x) and the view then.
    pan: Option<(f64, (f64, f64))>,
}

impl CutView {
    pub fn new(shared: Arc<CutShared>) -> Self {
        let local = DIAL_SPECS.iter().fold(MemoryParamHost::new(), |h, s| h.with(*s));
        let words = [
            "The threshold: a hit must reach it in its own drum (alone: its peak, the line on the lane); higher cuts more, the lowest first · double-click: automatic",
            "How loud the other tracks' bleed is taken to be (more: cuts more of what they play)",
            "How long before each hit the cut opens",
            "How long a hit is held after it ends",
            "A hit ends this far above the track's floor (more: shorter)",
            "How long the cut takes to close",
            "What is left between hits (−80 dB: as good as nothing)",
        ];
        let names = ["THRESH", "BLEED", "PRE", "HOLD", "ENDS", "FADE", "FLOOR"];
        let dials: Vec<FooterDial> = DIAL_SPECS.iter().zip(words).zip(names).map(|((s, w), n)| FooterDial { spec: *s, kind: KnobKind::Unipolar, name: n, words: w }).collect();
        let dials = Footer::new(HEIGHT - FOOTER - DIALS, (WIDTH, HEIGHT - FOOTER), Vec::new(), dials);
        let toggles = vec![
            FooterToggle { id: REMOVED.id, glyph: ToggleGlyph::Delta, words: "Listen Removed: only what is cut (nothing real should be in it)" },
            FooterToggle { id: BYPASS.id, glyph: ToggleGlyph::Bypass, words: "Bypass: every track as it is" },
        ];
        let footer = Footer::new(HEIGHT - FOOTER, (WIDTH, HEIGHT), toggles, Vec::new());
        Self { shared, seen: u64::MAX, local, dials, footer, selected: None, shown_settings: None, auto_shown: None, view: None, first_row: 0, hot: Hot::None, pan: None }
    }

    fn lane_h(&self, n: usize) -> f64 {
        ((LANES_BOTTOM - LANES_TOP) / n.max(1) as f64).clamp(LANE_MIN, LANE_MAX)
    }

    fn rows_shown(&self, n: usize) -> usize {
        (((LANES_BOTTOM - LANES_TOP) / self.lane_h(n)).floor() as usize).min(n)
    }

    /// Track `row`'s lane (whole width: names and waveform).
    fn row_rect(&self, row: usize, n: usize) -> Option<Rect> {
        let shown = row.checked_sub(self.first_row)?;
        if shown >= self.rows_shown(n) {
            return None;
        }
        let h = self.lane_h(n);
        let y = LANES_TOP + shown as f64 * h;
        Some(Rect::new(0.0, y, WIDTH, y + h))
    }

    fn chip(r: Rect) -> Rect {
        let y = r.y0 + 8.0;
        Rect::new(NAMES_W - CHIP_W - 10.0, y, NAMES_W - 10.0, y + 20.0)
    }

    fn view(&self) -> (f64, f64) {
        self.view.unwrap_or((0.0, 10.0))
    }

    fn x_of(&self, t: f64) -> f64 {
        let (t0, t1) = self.view();
        LANES_X + (t - t0) / (t1 - t0) * (LANES_X1 - LANES_X)
    }

    fn t_of(&self, x: f64) -> f64 {
        let (t0, t1) = self.view();
        t0 + (x - LANES_X) / (LANES_X1 - LANES_X) * (t1 - t0)
    }

    /// The song's extent, s.
    fn song(tracks: &[Track]) -> Option<(f64, f64)> {
        let clips = tracks.iter().flat_map(|t| t.clips.iter());
        let (a, b) = clips.fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), c| (a.min(c.song_s), b.max(c.song_s + c.len as f64 / c.fs)));
        (b > a).then_some((a, b))
    }

    fn hot_at(&self, p: Point, tracks: &[Track]) -> Hot {
        if EXPORT.contains(p) {
            return Hot::Export;
        }
        if p.y >= HEADER && p.y < LANES_TOP && p.x >= LANES_X {
            return Hot::Ruler;
        }
        for (row, t) in tracks.iter().enumerate() {
            let Some(r) = self.row_rect(row, tracks.len()) else { continue };
            if !r.contains(p) {
                continue;
            }
            if p.x < NAMES_W {
                return if Self::chip(r).contains(p) { Hot::Chip(row) } else { Hot::Name(row) };
            }
            if p.x >= LANES_X && p.x <= LANES_X1 {
                // The nearest mark within reach (marks sit along the lane's top).
                let mut best: Option<(f64, usize, usize)> = None;
                if p.y < r.y0 + 22.0 {
                    let (t0, t1) = self.view();
                    for (ci, c) in t.clips.iter().enumerate() {
                        let in_view = c.hits.iter().filter(|h| h.kept && !h.inside).filter(|h| (t0..t1).contains(&(c.song_s + h.onset as f64 / c.fs))).count();
                        if in_view as f64 * MARK_SPACING > LANES_X1 - LANES_X {
                            continue; // (not drawn: not to be clicked)
                        }
                        for (hi, h) in c.hits.iter().enumerate().filter(|(_, h)| !h.inside) {
                            let d = (self.x_of(c.song_s + h.onset as f64 / c.fs) - p.x).abs();
                            if d <= MARK_REACH && best.is_none_or(|b| d < b.0) {
                                best = Some((d, ci, hi));
                            }
                        }
                    }
                }
                return match best {
                    Some((_, ci, hi)) => Hot::Mark(row, ci, hi),
                    None => Hot::Lane(row, self.t_of(p.x)),
                };
            }
        }
        Hot::None
    }

    fn draw_ruler(&self, cx: &mut DrawCx<'_>) {
        let theme = cx.theme;
        let (t0, t1) = self.view();
        let r = Rect::new(LANES_X, HEADER, LANES_X1, LANES_TOP);
        cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.window, None, &Rect::new(0.0, HEADER, WIDTH, LANES_TOP));
        // A tick every step that leaves 70 px between labels.
        let span = t1 - t0;
        let step = [0.01, 0.02, 0.05, 0.1, 0.2, 0.5, 1.0, 2.0, 5.0, 10.0, 15.0, 30.0, 60.0, 120.0, 300.0].into_iter().find(|s| s / span * r.width() >= 70.0).unwrap_or(600.0);
        let mut t = (t0 / step).ceil() * step;
        while t <= t1 {
            let x = self.x_of(t).round() + 0.5;
            cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, theme.grid_minor, None, &Line::new((x, r.y1 - 6.0), (x, LANES_BOTTOM)));
            let label = if step < 1.0 { format!("{}:{:05.2}", (t / 60.0).floor(), t % 60.0) } else { format!("{}:{:02}", (t / 60.0).floor(), (t % 60.0).round() as i64) };
            cx.text.draw(cx.scene, &label, x + 4.0, r.y0 + 6.0, TextStyle::LABEL.sized(9.0), theme.text_lo, Align::Left);
            t += step;
        }
        cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, theme.line, None, &Line::new((0.0, r.y1 - 0.5), (WIDTH, r.y1 - 0.5)));
    }

    /// A clip's waveform in `lane` (one low–high line per pixel), `from`..`to` its samples.
    fn draw_wave(&self, cx: &mut DrawCx<'_>, lane: Rect, c: &Clip, (from, to): (usize, usize), ink: Color, full: f32) {
        let peaks = &c.peaks;
        if peaks.is_empty() || to <= from {
            return;
        }
        let per_s = c.fs / PEAK_HOP as f64;
        let mid = lane.center().y + 6.0;
        let half = (lane.height() / 2.0 - 10.0) / f64::from(full.max(1e-6));
        let x0 = self.x_of(c.song_s + from as f64 / c.fs).max(LANES_X).floor();
        let x1 = self.x_of(c.song_s + to as f64 / c.fs).min(LANES_X1).ceil();
        if x1 <= x0 {
            return;
        }
        let (lo_k, hi_k) = (from / PEAK_HOP, to.div_ceil(PEAK_HOP).min(peaks.len()));
        let mut top = BezPath::new();
        let mut bottom: Vec<Point> = Vec::new();
        for col in 0..=((x1 - x0) as usize) {
            let x = x0 + col as f64;
            let reach = 0.5 * (self.t_of(x + 1.0) - self.t_of(x)) * per_s;
            let centre = (self.t_of(x + 0.5) - c.song_s) * per_s;
            let a = ((centre - reach).max(0.0) as usize).max(lo_k);
            let b = ((centre + reach).max(0.0) as usize + 1).min(hi_k);
            let (lo, hi) = peaks.get(a.min(b)..b).unwrap_or(&[]).iter().fold((0.0f32, 0.0f32), |(l, h), p| (l.min(p.0), h.max(p.1)));
            let p = Point::new(x, mid - f64::from(hi.clamp(0.0, full)) * half);
            if col == 0 {
                top.move_to(p);
            } else {
                top.line_to(p);
            }
            bottom.push(Point::new(x, mid - f64::from(lo.clamp(-full, 0.0)) * half));
        }
        for p in bottom.iter().rev() {
            top.line_to(*p);
        }
        top.close_path();
        cx.scene.fill(Fill::NonZero, Affine::IDENTITY, ink, None, &top);
    }

    fn draw_lane(&self, cx: &mut DrawCx<'_>, row: usize, t: &Track, n: usize) {
        let theme = cx.theme;
        let Some(r) = self.row_rect(row, n) else { return };
        let lane = Rect::new(LANES_X, r.y0, LANES_X1, r.y1);
        let selected = self.selected == Some(t.sequence);
        if selected {
            cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.accent.multiply_alpha(0.05), None, &r);
        }
        cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, theme.grid_minor, None, &Line::new((0.0, r.y1 - 0.5), (WIDTH, r.y1 - 0.5)));
        // The name, the role chip, the hits kept.
        let name_ink = if selected { theme.text_strong } else { theme.text_hi };
        cx.begin_clip(&Rect::new(10.0, r.y0, NAMES_W - CHIP_W - 16.0, r.y1));
        cx.text.draw(cx.scene, &t.name, 14.0, r.y0 + 10.0, TextStyle::BODY.sized(13.0), name_ink, Align::Left);
        cx.end_clip();
        let cuts = t.role.cuts();
        let chip = Self::chip(r);
        cx.scene.fill(Fill::NonZero, Affine::IDENTITY, if cuts { theme.accent } else { theme.control }, None, &RoundedRect::from_rect(chip, 4.0));
        if self.hot == Hot::Chip(row) {
            cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, theme.text_hi, None, &RoundedRect::from_rect(chip, 4.0));
        }
        cx.text.draw(cx.scene, t.role.name(), chip.center().x, chip.y0 + 4.0, TextStyle::LABEL.sized(10.0), if cuts { theme.window } else { theme.text_mid }, Align::Center);
        let (kept, _) = t.counts();
        let unsure = t.clips.iter().flat_map(|c| c.hits.iter()).filter(|h| !h.forced && !h.inside && (0.25..0.75).contains(&h.confidence)).count();
        let note = match t.role {
            _ if cuts && unsure > 0 => format!("{kept} hits · {unsure} to check"),
            _ if cuts => format!("{kept} hits"),
            Role::Overhead | Role::Room => "never cut".to_owned(),
            _ => "passes".to_owned(),
        };
        cx.text.draw(cx.scene, &note, 14.0, r.y0 + 29.0, TextStyle::BODY.sized(10.0), theme.text_lo, Align::Left);
        if cuts && !t.linked.is_empty() && r.height() >= 58.0 {
            cx.begin_clip(&Rect::new(10.0, r.y0, NAMES_W - 6.0, r.y1));
            cx.text.draw(cx.scene, &format!("with {}", t.linked.join(", ")), 14.0, r.y0 + 43.0, TextStyle::BODY.sized(10.0), theme.accent, Align::Left);
            cx.end_clip();
        } else if cuts && t.compared == 0 && r.height() >= 58.0 {
            cx.text.draw(cx.scene, "judged alone", 14.0, r.y0 + 43.0, TextStyle::BODY.sized(10.0), theme.warn, Align::Left);
        }
        // The waveform (scaled to the track's loudest peak): dim all of it, lit where the cut
        // keeps it (its regions, as played).
        cx.begin_clip(&lane);
        let full = t.clips.iter().flat_map(|c| c.peaks.iter()).fold(0.0f32, |m, p| m.max(-p.0).max(p.1)).max(1e-6);
        let dim = theme.text_lo.multiply_alpha(0.55);
        // The threshold, on a track judged alone (there it is a peak: a hit of this drum must
        // peak over it, less what of it lies outside the drum's bands; with tracks to compare
        // it is a level in the drum's band, which the waveform does not show).
        if cuts && t.compared == 0 {
            if let Some(thr) = t.clips.iter().flat_map(|c| c.hits.first()).map(|h| h.threshold_db).next() {
                let a = f64::from(10f32.powf(thr / 20.0) / full);
                if a < 1.0 {
                    let (mid, half) = (lane.center().y + 6.0, lane.height() / 2.0 - 10.0);
                    for y in [mid - a * half, mid + a * half] {
                        cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, theme.warn.multiply_alpha(0.7), None, &Line::new((LANES_X, y.round() + 0.5), (LANES_X1, y.round() + 0.5)));
                    }
                }
            }
        }
        for (ci, c) in t.clips.iter().enumerate() {
            self.draw_wave(cx, lane, c, (0, c.len), if cuts { dim } else { theme.text_mid }, full);
            if !cuts {
                continue;
            }
            for (a, b) in c.curve.regions() {
                let (xa, xb) = (self.x_of(c.song_s + a as f64 / c.fs), self.x_of(c.song_s + b as f64 / c.fs));
                if xb < LANES_X || xa > LANES_X1 {
                    continue;
                }
                cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.accent.multiply_alpha(0.08), None, &Rect::new(xa, lane.y0 + 18.0, xb.max(xa + 1.0), lane.y1 - 2.0));
                self.draw_wave(cx, lane, c, (a, b.min(c.len)), theme.accent, full);
            }
            // The hits: a mark each at its onset, once they are far enough apart to tell apart.
            let (t0, t1) = self.view();
            let in_view = c.hits.iter().filter(|h| h.kept && !h.inside).filter(|h| (t0..t1).contains(&(c.song_s + h.onset as f64 / c.fs))).count();
            if in_view as f64 * MARK_SPACING > lane.width() {
                continue;
            }
            for (hi, h) in c.hits.iter().enumerate().filter(|(_, h)| !h.inside) {
                let x = self.x_of(c.song_s + h.onset as f64 / c.fs);
                if !(LANES_X - 6.0..=LANES_X1 + 6.0).contains(&x) {
                    continue;
                }
                let unsure = !h.forced && (0.25..0.75).contains(&h.confidence);
                let hot = self.hot == Hot::Mark(row, ci, hi);
                if !h.kept && !unsure && !h.forced && !hot {
                    // Surely bleed: a dot, out of the way.
                    cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.text_lo.multiply_alpha(0.6), None, &mite_ui::kurbo::Circle::new((x, lane.y0 + 7.0), 1.6));
                    continue;
                }
                let ink = if unsure { theme.warn } else if h.kept { theme.accent } else { theme.text_lo };
                let s = if hot { 6.0 } else { 4.5 };
                let mut tri = BezPath::new();
                tri.move_to((x - s, lane.y0 + 4.0));
                tri.line_to((x + s, lane.y0 + 4.0));
                tri.line_to((x, lane.y0 + 4.0 + 1.6 * s));
                tri.close_path();
                if h.kept {
                    cx.scene.fill(Fill::NonZero, Affine::IDENTITY, ink, None, &tri);
                } else {
                    cx.scene.stroke(&Stroke::new(1.2), Affine::IDENTITY, ink, None, &tri);
                }
                if h.forced {
                    // Set by hand: a bar over it.
                    cx.scene.stroke(&Stroke::new(1.5), Affine::IDENTITY, theme.text_hi, None, &Line::new((x - s, lane.y0 + 1.5), (x + s, lane.y0 + 1.5)));
                }
            }
        }
        cx.end_clip();
    }

    fn draw_playhead(&self, cx: &mut DrawCx<'_>) {
        let s = &self.shared.session;
        let t = s.playhead.load(Ordering::Relaxed) as f64 / s.host_rate().max(1.0);
        let x = self.x_of(t);
        if (LANES_X..=LANES_X1).contains(&x) {
            let ink = if s.playing.load(Ordering::Relaxed) { cx.theme.text_strong } else { cx.theme.text_lo };
            cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, ink, None, &Line::new((x.round() + 0.5, HEADER + 4.0), (x.round() + 0.5, LANES_BOTTOM)));
        }
    }

    fn help(&self, tracks: &[Track]) -> Help {
        match self.hot {
            Hot::Mark(row, ci, hi) => {
                let Some(h) = tracks.get(row).and_then(|t| t.clips.get(ci)).and_then(|c| c.hits.get(hi)) else { return Help::default() };
                let state = if h.forced { "set by hand" } else if h.kept { "kept" } else { "cut" };
                let action = if h.kept { "click: cut it" } else { "click: keep it" };
                let back = if h.forced { " · double-click: as detected" } else { "" };
                if h.linked && h.score_db == f32::NEG_INFINITY {
                    return Help::default().text(format!("A hit heard on this drum's other mic, {state} with it · {action}{back}"));
                }
                let over = h.score_db - h.threshold_db;
                let bleed = if h.dominance_db < -0.5 { format!(" ({:.1} dB of it bleed)", -h.dominance_db) } else { String::new() };
                Help::default().text(format!("A hit, {state} · peaks {:.1} dBFS{bleed}, {:.1} dB {} the threshold · {action}{back}", h.peak_db, over.abs(), if over >= 0.0 { "over" } else { "under" }))
            }
            Hot::Chip(_) => Help::default().text("The track's role: click for the next, Shift-click for the one before · OH and Room are never cut, Off passes"),
            Hot::Name(row) if tracks.get(row).is_some_and(|t| t.role.cuts() && t.compared == 0) => Help::default().text("Judged alone, by its own sound: no other drum track has Mite Cut. Put it on the kit's other tracks (kick, snare, toms, hats) and each is judged against the others"),
            Hot::Name(row) if tracks.get(row).is_some_and(|t| !t.linked.is_empty()) => Help::default().text(format!("One drum with {}: each hit is decided on all its mics together (kept if any mic keeps it) and they open and close together · click to tune this track", tracks[row].linked.join(", "))),
            Hot::Name(_) => Help::default().text("Click to tune this track with the dials below"),
            Hot::Lane(..) => Help::default().text("Wheel: zoom · drag: pan · double-click: add a hit here (where none was found)"),
            Hot::Ruler => Help::default().text("Wheel: zoom · drag: pan · double-click: the whole song"),
            Hot::Export => Help::default().text(format!("Write each cut track as a WAV file (32-bit float, stamped with its place on the song) into {}", crate::session::export_root().display())),
            Hot::None => Help::default().text("Each track is judged against the others · lit: kept · dim: cut · amber marks: close to the threshold, check those"),
        }
    }

    /// Where hit `hit` of clip `clip` on track `sequence` is marked (tests and tools).
    pub fn mark_point(&self, sequence: u64, clip: usize, hit: usize) -> Option<Point> {
        let tracks = self.shared.session.tracks();
        let row = tracks.iter().position(|t| t.sequence == sequence)?;
        let r = self.row_rect(row, tracks.len())?;
        let c = tracks[row].clips.get(clip)?;
        Some(Point::new(self.x_of(c.song_s + c.hits.get(hit)?.onset as f64 / c.fs), r.y0 + 8.0))
    }

    /// Where track `sequence`'s role chip is (tests and tools).
    pub fn chip_rect(&self, sequence: u64) -> Option<Rect> {
        let tracks = self.shared.session.tracks();
        let row = tracks.iter().position(|t| t.sequence == sequence)?;
        Some(Self::chip(self.row_rect(row, tracks.len())?))
    }

    fn selected_track<'a>(&self, tracks: &'a [Track]) -> Option<&'a Track> {
        self.selected.and_then(|s| tracks.iter().find(|t| t.sequence == s))
    }
}

impl View for CutView {
    fn logical_size(&self) -> (f64, f64) {
        (WIDTH, HEIGHT)
    }

    fn draw(&mut self, cx: &mut DrawCx<'_>) {
        let theme = cx.theme;
        let s = self.shared.session.clone();
        let tracks = s.tracks();
        cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.display, None, &Rect::new(0.0, LANES_TOP, WIDTH, LANES_BOTTOM));
        self.draw_ruler(cx);
        let n = tracks.len();
        for (row, t) in tracks.iter().enumerate() {
            self.draw_lane(cx, row, t, n);
        }
        if self.first_row + self.rows_shown(n) < n {
            cx.text.draw(cx.scene, &format!("{} more below: scroll the names", n - self.first_row - self.rows_shown(n)), 14.0, LANES_BOTTOM - 16.0, TextStyle::BODY.sized(10.0), theme.text_lo, Align::Left);
        }
        let status = match s.status() {
            Status::Unbound => Some("Not bound to an ARA document: insert Mite Cut (VST3) on each drum track, in a host with ARA"),
            Status::Reading => Some("Reading the tracks from the host…"),
            Status::Detecting => Some("Finding the hits on every track…"),
            Status::Ready if tracks.is_empty() => Some("No clips yet"),
            Status::Ready => None,
        };
        if let Some(text) = status {
            cx.text.draw(cx.scene, text, (LANES_X + LANES_X1) / 2.0, (LANES_TOP + LANES_BOTTOM) / 2.0 - 8.0, TextStyle::BODY.sized(13.0), theme.text_mid, Align::Center);
        }
        self.draw_playhead(cx);
        // Header.
        cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.window, None, &Rect::new(0.0, 0.0, WIDTH, HEADER));
        cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, theme.line, None, &Line::new((0.0, HEADER - 0.5), (WIDTH, HEADER - 0.5)));
        draw_mark(cx, 14.0, 11.0, 26.0);
        draw_title(cx, 48.0, 14.0, "Cut");
        let hot = self.hot == Hot::Export;
        cx.scene.fill(Fill::NonZero, Affine::IDENTITY, if hot { theme.panel_control } else { theme.control }, None, &RoundedRect::from_rect(EXPORT, 5.0));
        cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, if hot { theme.text_mid } else { theme.control_border }, None, &RoundedRect::from_rect(EXPORT, 5.0));
        cx.text.draw(cx.scene, "EXPORT", EXPORT.center().x, EXPORT.y0 + 8.0, TextStyle::LABEL.sized(10.0), theme.text_hi, Align::Center);
        let said = match s.exported() {
            Some(Export::Writing) => Some(("Writing the cut tracks…".to_owned(), theme.text_mid)),
            Some(Export::Done(dir, files)) => Some((format!("{} {} written to {}", files.len(), if files.len() == 1 { "file" } else { "files" }, dir.display()), theme.text_mid)),
            Some(Export::Failed(e)) => Some((format!("Not exported: {e}"), theme.warn)),
            None => None,
        };
        let alone = tracks.iter().filter(|t| t.role.cuts()).count() == 1 && tracks.iter().any(|t| t.role.cuts() && t.compared == 0);
        let said = said.or_else(|| alone.then(|| ("Judged alone: no other drum track with Mite Cut reaches this one. Put it on the kit's other tracks too".to_owned(), theme.warn)));
        if let Some((text, ink)) = said {
            cx.begin_clip(&Rect::new(160.0, 0.0, EXPORT.x0 - 12.0, HEADER));
            cx.text.draw(cx.scene, &text, EXPORT.x0 - 14.0, 17.0, TextStyle::BODY.sized(11.0), ink, Align::Right);
            cx.end_clip();
        }
        // The dials, for the selected track.
        let top = HEIGHT - FOOTER - DIALS;
        cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.window, None, &Rect::new(0.0, top, WIDTH, HEIGHT - FOOTER));
        cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, theme.line, None, &Line::new((0.0, top + 0.5), (WIDTH, top + 0.5)));
        match self.selected_track(&tracks) {
            Some(t) if t.role.cuts() => {
                cx.text.draw(cx.scene, "TUNING", 14.0, top + 15.0, TextStyle::LABEL.sized(9.0), theme.text_lo, Align::Left);
                cx.text.draw(cx.scene, &t.name, 14.0, top + 28.0, TextStyle::BODY.sized(13.0), theme.text_hi, Align::Left);
                let mut sub = DrawCx { scene: &mut *cx.scene, theme: cx.theme, text: &mut *cx.text, host: &self.local, size: cx.size, fx: &mut *cx.fx };
                self.dials.draw_controls(&mut sub);
                if t.settings.threshold_db.is_none() {
                    let r = thresh_dial();
                    cx.text.draw(cx.scene, "AUTO", r.center().x + 22.0, r.center().y + 14.0, TextStyle::LABEL.sized(8.0), theme.text_lo, Align::Left);
                }
            }
            _ => {
                cx.text.draw(cx.scene, "Click a drum track's name to tune it", 14.0, top + 22.0, TextStyle::BODY.sized(12.0), theme.text_lo, Align::Left);
            }
        }
        // Footer: help on the left, the toggles on the right.
        self.footer.draw_frame(cx);
        self.footer.draw_controls(cx);
        let help = self.dials.help(&self.local).or_else(|| self.footer.help(cx.host)).unwrap_or_else(|| self.help(&tracks));
        help.draw(cx, Rect::new(14.0, HEIGHT - FOOTER + 8.0, WIDTH - 230.0, HEIGHT - 8.0));
    }

    fn event(&mut self, event: &UiEvent, host: &dyn ParamHost) -> bool {
        let s = self.shared.session.clone();
        let tracks = s.tracks();
        // A double-click on THRESH: back to the automatic threshold.
        if let UiEvent::DoubleClick { pos, .. } = *event {
            if thresh_dial().contains(pos) {
                if let Some(t) = self.selected_track(&tracks).filter(|t| t.role.cuts()) {
                    s.set_settings(t.sequence, Settings { threshold_db: None, ..t.settings });
                    self.shown_settings = None;
                    return true;
                }
            }
        }
        if self.selected_track(&tracks).is_some_and(|t| t.role.cuts()) && self.dials.event(event, &self.local) {
            return true;
        }
        if self.footer.event(event, host) {
            return true;
        }
        match *event {
            UiEvent::PointerMove { pos, .. } => {
                if let Some((x0, (t0, t1))) = self.pan {
                    let dt = (pos.x - x0) / (LANES_X1 - LANES_X) * (t1 - t0);
                    self.view = Some((t0 - dt, t1 - dt));
                    return true;
                }
                let hot = self.hot_at(pos, &tracks);
                let changed = hot != self.hot;
                self.hot = hot;
                changed
            }
            UiEvent::PointerLeft => std::mem::replace(&mut self.hot, Hot::None) != Hot::None,
            UiEvent::PointerDown { pos, button: MouseButton::Left, mods } => match self.hot_at(pos, &tracks) {
                Hot::Export => {
                    if s.exported() != Some(Export::Writing) {
                        s.export(None);
                    }
                    true
                }
                Hot::Chip(row) => {
                    let t = &tracks[row];
                    s.set_role(t.sequence, next_role(t.role, mods.shift));
                    true
                }
                Hot::Name(row) => {
                    self.selected = Some(tracks[row].sequence);
                    true
                }
                Hot::Mark(row, ci, hi) => {
                    let c = &tracks[row].clips[ci];
                    let h = c.hits[hi];
                    s.set_override(c.modification, OverrideState { at: h.onset, keep: Some(!h.kept), start: None, end: None });
                    true
                }
                Hot::Lane(..) | Hot::Ruler => {
                    self.pan = Some((pos.x, self.view()));
                    false
                }
                Hot::None => false,
            },
            UiEvent::PointerUp { .. } => self.pan.take().is_some(),
            UiEvent::DoubleClick { pos, .. } => match self.hot_at(pos, &tracks) {
                Hot::Mark(row, ci, hi) => {
                    // As detected: the override goes.
                    let c = &tracks[row].clips[ci];
                    s.set_override(c.modification, OverrideState { at: c.hits[hi].onset, keep: None, start: None, end: None });
                    true
                }
                Hot::Lane(row, t) => {
                    let Some(c) = tracks[row].clips.iter().find(|c| t >= c.song_s && t < c.song_s + c.len as f64 / c.fs) else { return false };
                    if tracks[row].role.cuts() {
                        s.set_override(c.modification, OverrideState { at: ((t - c.song_s) * c.fs) as usize, keep: Some(true), start: None, end: None });
                    }
                    true
                }
                Hot::Ruler => {
                    self.view = Self::song(&tracks);
                    true
                }
                _ => false,
            },
            UiEvent::Wheel { pos, lines, mods } => {
                if pos.x < NAMES_W {
                    let n = tracks.len();
                    let max = n.saturating_sub(self.rows_shown(n));
                    let before = self.first_row;
                    self.first_row = if lines > 0.0 { self.first_row.saturating_sub(1) } else { (self.first_row + 1).min(max) };
                    return before != self.first_row;
                }
                if pos.x < LANES_X || pos.y < HEADER || pos.y > LANES_BOTTOM {
                    return false;
                }
                let (t0, t1) = self.view();
                if mods.shift {
                    let dt = -f64::from(lines) * 0.1 * (t1 - t0);
                    self.view = Some((t0 + dt, t1 + dt));
                } else {
                    let at = self.t_of(pos.x);
                    let k = 0.8f64.powf(f64::from(lines));
                    let span = ((t1 - t0) * k).clamp(0.05, 3600.0);
                    let f = (at - t0) / (t1 - t0);
                    self.view = Some((at - f * span, at - f * span + span));
                }
                true
            }
            _ => false,
        }
    }

    fn tick(&mut self, _host: &dyn ParamHost) -> bool {
        let s = self.shared.session.clone();
        let v = s.version.load(Ordering::Acquire);
        let mut redraw = v != self.seen;
        self.seen = v;
        let tracks = s.tracks();
        if self.view.is_none() {
            if let Some(song) = Self::song(&tracks) {
                self.view = Some(song);
                redraw = true;
            }
        }
        if self.selected_track(&tracks).is_none() {
            self.selected = tracks.iter().find(|t| t.role.cuts()).or(tracks.first()).map(|t| t.sequence);
        }
        // The dials follow the selected track; a turn writes to it. THRESH shows the threshold
        // its hits are read with: the automatic one until turned.
        if let Some(t) = self.selected_track(&tracks) {
            let current = t.clips.iter().flat_map(|c| c.hits.first()).map(|h| h.threshold_db).next().unwrap_or(THRESH.default);
            let reload = match self.shown_settings {
                Some((seq, shown)) if seq == t.sequence => {
                    // (Sensitivity is no dial now: an older session's stays as it was.)
                    let now = Settings { sensitivity: t.settings.sensitivity, ..settings_of(&self.local, self.auto_shown) };
                    if now != shown {
                        s.set_settings(seq, now);
                        if now.threshold_db.is_some() {
                            self.auto_shown = None;
                        }
                        self.shown_settings = Some((seq, now));
                        redraw = true;
                        false
                    } else {
                        // Changed elsewhere (another editor, the host's undo), or the automatic
                        // threshold moved (other settings, a role).
                        let moved = t.settings.threshold_db.is_none() && self.auto_shown.is_some_and(|a| (a - current).abs() > 0.05);
                        !self.dials.busy() && (!alike(&t.settings, &shown) || moved)
                    }
                }
                _ => true,
            };
            if reload {
                self.auto_shown = load_settings(&self.local, &t.settings, current);
                self.shown_settings = Some((t.sequence, Settings { sensitivity: t.settings.sensitivity, ..settings_of(&self.local, self.auto_shown) }));
                redraw = true;
            }
        }
        // The playhead moves while the host plays.
        redraw || s.playing.load(Ordering::Relaxed)
    }
}
