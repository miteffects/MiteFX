//! The Mite Tuner editor (docs/design/tuner-brief.md §2), in 1280×760 design units.
//!
//! Big: the note fills the middle of the window, its octave beside it, an arrow each side lit
//! toward the way to turn the peg. Under it the cents strip (−50…+50) with its needle, the
//! frequency and the cents, and a trace of the last six seconds of cents (a string drifting or
//! settling is seen, not guessed). Above it the tuning's strings, the nearest one lit. In tune
//! (within ±2 cents) everything in the middle glows. The pitch is found here, off the audio
//! thread, from the input tap (`mite_dsp::pitch`).

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::{Duration, Instant};

use mite_core::{ParamDisplay, ParamScale, ParamSpec};
use mite_dsp::pitch::{note_hz, note_of, window_len, Pitch, PitchDetector};
use mite_ui::kurbo::{Affine, BezPath, Cap, Join, Line, Point, Rect, RoundedRect, Stroke};
use mite_ui::paint::{glow_stroke, panel};
use mite_ui::peniko::Fill;
use mite_ui::session::{Session, SessionConfig};
use mite_ui::widgets::brand::{draw_mark, draw_title};
use mite_ui::widgets::{Dropdown, DropdownEvent, Footer, FooterDial, FooterToggle, Help, KnobKind, ToggleGlyph};
use mite_ui::{Align, DrawCx, MouseButton, ParamHost, RecordingHost, TextStyle, UiEvent, View};

use crate::dsp::record;
use crate::params::*;
use crate::presets::{preset_ids, FACTORY, PLUGIN};
use crate::TunerShared;

/// Mite Tuner wears the Tuner accent (tokens.json `color.accent.tuner`, seafoam).
pub const ACCENT: &str = "tuner";

const VIEW_W: f64 = 1280.0;
const VIEW_H: f64 = 760.0;
const HEADER: f64 = 56.0;
const DISPLAY_TOP: f64 = HEADER;
const HELP_TOP: f64 = DISPLAY_TOP + 620.0;
const FOOTER_TOP: f64 = HELP_TOP + 28.0;
const STAGE: Rect = Rect::new(0.0, DISPLAY_TOP, 1180.0, HELP_TOP);
const CENTRE_X: f64 = 590.0;

/// The strings row.
const STRINGS_Y: f64 = DISPLAY_TOP + 24.0;
const STRING_H: f64 = 44.0;
const TUNING_PILL: Rect = Rect::new(28.0, STRINGS_Y, 228.0, STRINGS_Y + STRING_H);
/// The note.
const NOTE_TOP: f64 = DISPLAY_TOP + 14.0;
const NOTE_SIZE: f32 = 220.0;
/// Where the note's letters sit (the middle of a capital).
const NOTE_MID: f64 = NOTE_TOP + 155.0;
/// The cents strip.
const STRIP: Rect = Rect::new(110.0, DISPLAY_TOP + 330.0, 1070.0, DISPLAY_TOP + 390.0);
/// The trace of the last seconds.
const TRACE: Rect = Rect::new(28.0, DISPLAY_TOP + 440.0, 1152.0, HELP_TOP - 12.0);
const TRACE_SECONDS: f64 = 6.0;

/// Within this many cents a string is in tune.
pub const IN_TUNE: f64 = 2.0;
/// The note shown stays this long after the sound goes, fading.
const HOLD: Duration = Duration::from_millis(1500);
/// How often the pitch is found (the detector's windows overlap).
const DETECT_EVERY: Duration = Duration::from_millis(20);

const WELL_X: f64 = 1214.0;
const METER_TOP: f64 = DISPLAY_TOP + 20.0;
const METER_BOTTOM: f64 = DISPLAY_TOP + 592.0;
const METER_RANGE: f64 = 60.0;
const SILENT: f32 = -150.0;

const NAMES: [&str; 12] = ["C", "C", "D", "D", "E", "F", "F", "G", "G", "A", "A", "B"];
const SHARP: [bool; 12] = [false, true, false, true, false, false, true, false, true, false, true, false];

/// A MIDI note's name (`C♯` as letter and sharp) and octave.
pub fn note_name(note: i32) -> (&'static str, bool, i32) {
    let k = note.rem_euclid(12) as usize;
    (NAMES[k], SHARP[k], note.div_euclid(12) - 1)
}

fn note_text(note: i32) -> String {
    let (n, sharp, octave) = note_name(note);
    format!("{n}{}{octave}", if sharp { "#" } else { "" })
}

/// A stand-in host holding every Mite Tuner parameter at its default (tests, preview, tools).
pub fn memory_host() -> mite_ui::MemoryParamHost {
    let index = |id: &'static str, count: usize, default: usize| ParamSpec {
        id,
        name: id,
        unit: "",
        min: 0.0,
        max: (count - 1) as f32,
        default: default as f32,
        scale: ParamScale::Linear,
        smoothing_ms: 0.0,
        decimals: 0,
        wheel_step: 1.0,
        wheel_step_fine: 1.0,
        display: ParamDisplay::Number,
    };
    mite_ui::MemoryParamHost::new().with(REFERENCE).with(index(TUNING.id, TUNINGS.len(), TUNING.default)).with(index(MUTE.id, 2, 0))
}

/// What the tuner reads now.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Reading {
    /// The nearest note (MIDI).
    pub note: i32,
    /// How far off, cents (smoothed while the note holds).
    pub cents: f64,
    pub hz: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Hot {
    Tuning,
    String(usize),
}

pub struct TunerView {
    shared: Arc<TunerShared>,
    session: Session,
    footer: Footer,
    detector: PitchDetector,
    detector_fs: f32,
    window: Vec<f32>,
    reading: Option<Reading>,
    heard: Option<Instant>,
    last_detect: Instant,
    trace: VecDeque<(Instant, Option<f64>)>,
    pointer: Option<Point>,
    hot: Option<Hot>,
    menu: Option<Dropdown>,
    history_seen: usize,
    meter: f32,
    last_tick: Instant,
}

impl TunerView {
    pub fn new(shared: Arc<TunerShared>) -> Self {
        let footer = Footer::new(
            FOOTER_TOP,
            (VIEW_W, VIEW_H),
            vec![FooterToggle { id: MUTE.id, glyph: ToggleGlyph::Mute, words: "Mute — silence the output while you tune (the tuner still hears the input)" }],
            vec![FooterDial { spec: REFERENCE, kind: KnobKind::Bipolar, name: "A4", words: "Reference — the A above middle C, Hz (440 standard; 432, 442… to match a band or a record)" }],
        );
        let fs = shared.sample_rate.load();
        let len = window_len(f64::from(fs));
        Self {
            session: Session::new(SessionConfig { plugin: PLUGIN, factory: FACTORY, sound_ids: preset_ids(), snapshot_ids: preset_ids() }, shared.snapshots.clone(), shared.preset.clone()),
            footer,
            detector: PitchDetector::new(len),
            detector_fs: fs,
            window: vec![0.0; crate::TAP_LEN],
            reading: None,
            heard: None,
            last_detect: Instant::now() - DETECT_EVERY,
            trace: VecDeque::new(),
            pointer: None,
            hot: None,
            menu: None,
            history_seen: 0,
            meter: SILENT,
            last_tick: Instant::now(),
            shared,
        }
    }

    pub fn with_preset_dir(mut self, dir: std::path::PathBuf) -> Self {
        self.session = self.session.with_preset_dir(dir);
        self
    }

    fn plain(host: &dyn ParamHost, id: &str) -> f32 {
        host.plain(id, host.normalized(id))
    }

    fn tuning(host: &dyn ParamHost) -> usize {
        (Self::plain(host, TUNING.id).round().max(0.0) as usize).min(TUNINGS.len() - 1)
    }

    /// What the tuner reads (none while nothing has been heard for a while).
    pub fn reading(&self) -> Option<Reading> {
        self.reading.filter(|_| self.heard.is_some_and(|t| t.elapsed() < HOLD))
    }

    /// The tuning's string nearest the note heard (index, low to high).
    pub fn nearest_string(&self, host: &dyn ParamHost) -> Option<usize> {
        let r = self.reading()?;
        let strings = STRINGS[Self::tuning(host)];
        let midi = f64::from(r.note) + r.cents / 100.0;
        (0..strings.len()).min_by(|&a, &b| (f64::from(strings[a]) - midi).abs().total_cmp(&(f64::from(strings[b]) - midi).abs()))
    }

    pub fn tuning_pill(&self) -> Rect {
        TUNING_PILL
    }

    /// The open tuning menu's rows (tests and tools).
    pub fn menu_rows(&self) -> Vec<Rect> {
        self.menu.as_ref().map(Dropdown::item_rects).unwrap_or_default()
    }

    fn string_rects(count: usize) -> Vec<Rect> {
        let (x0, x1) = (TUNING_PILL.x1 + 24.0, STAGE.x1 - 28.0);
        let gap = 12.0;
        let w = ((x1 - x0 - gap * 6.0) / 7.0).min(118.0);
        let total = count as f64 * w + (count.max(1) - 1) as f64 * gap;
        let start = 0.5 * (x0 + x1) - 0.5 * total;
        (0..count).map(|i| Rect::new(start + i as f64 * (w + gap), STRINGS_Y, start + i as f64 * (w + gap) + w, STRINGS_Y + STRING_H)).collect()
    }

    fn cents_x(cents: f64) -> f64 {
        STRIP.x0 + (cents.clamp(-50.0, 50.0) + 50.0) / 100.0 * STRIP.width()
    }

    /// Finds the pitch of the latest input now (the editor does it every 20 ms).
    pub fn analyze(&mut self, host: &dyn ParamHost) {
        let fs = self.shared.sample_rate.load();
        if fs != self.detector_fs {
            self.detector = PitchDetector::new(window_len(f64::from(fs)));
            self.detector_fs = fs;
        }
        let len = self.detector.len().min(self.window.len());
        self.shared.input.read_latest(&mut self.window[..len]);
        let reference = f64::from(Self::plain(host, REFERENCE.id));
        let found = self.detector.detect(&self.window[..len], f64::from(fs));
        let now = Instant::now();
        match found {
            Some(Pitch { hz, .. }) => {
                let (note, cents) = note_of(hz, reference);
                let held = self.reading().filter(|r| r.note == note);
                // The same note: ease toward the new reading (steady, not sluggish); a new note jumps.
                let cents = match held {
                    Some(r) => r.cents + (cents - r.cents) * 0.45,
                    None => cents,
                };
                self.reading = Some(Reading { note, cents, hz });
                self.heard = Some(now);
                self.trace.push_back((now, Some(cents)));
            }
            None => self.trace.push_back((now, None)),
        }
        while self.trace.front().is_some_and(|(t, _)| now.duration_since(*t).as_secs_f64() > TRACE_SECONDS) {
            self.trace.pop_front();
        }
    }

    // ---- drawing ----------------------------------------------------------------------------

    fn draw_header(&mut self, cx: &mut DrawCx<'_>) {
        let theme = cx.theme;
        cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.window, None, &Rect::new(0.0, 0.0, VIEW_W, HEADER));
        cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, theme.line, None, &Line::new((0.0, HEADER - 0.5), (VIEW_W, HEADER - 0.5)));
        draw_mark(cx, 16.0, 15.0, 26.0);
        draw_title(cx, 52.0, 18.0, "Tuner");
        // No preset header: a measuring tool has nothing to recall (user, 2026-11-24).
    }

    /// How present the reading is: 1 while heard, fading over the hold.
    fn presence(&self) -> f32 {
        match self.heard {
            Some(t) => {
                let age = t.elapsed().as_secs_f32();
                if age < 0.15 {
                    1.0
                } else {
                    (1.0 - (age - 0.15) / (HOLD.as_secs_f32() - 0.15)).clamp(0.0, 1.0)
                }
            }
            None => 0.0,
        }
    }

    fn draw_strings(&self, cx: &mut DrawCx<'_>) {
        let theme = cx.theme;
        let host = cx.host;
        let tuning = Self::tuning(host);
        // The tuning: a pill that opens the list.
        let r = RoundedRect::from_rect(TUNING_PILL, STRING_H / 2.0);
        let hot = self.hot == Some(Hot::Tuning) || self.menu.is_some();
        cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.well, None, &r);
        cx.scene.stroke(&Stroke::new(1.2), Affine::IDENTITY, if hot { theme.accent } else { theme.well_border }, None, &r);
        cx.text.draw(cx.scene, &TUNINGS[tuning].to_uppercase(), TUNING_PILL.x0 + 22.0, TUNING_PILL.center().y - 7.0, TextStyle::LABEL.sized(12.0), theme.text_hi, Align::Left);
        let (x, y) = (TUNING_PILL.x1 - 24.0, TUNING_PILL.center().y);
        let mut chev = BezPath::new();
        chev.move_to((x - 5.0, y - 2.5));
        chev.line_to((x, y + 2.5));
        chev.line_to((x + 5.0, y - 2.5));
        cx.scene.stroke(&Stroke::new(1.5).with_caps(Cap::Round).with_join(Join::Round), Affine::IDENTITY, theme.text_mid, None, &chev);
        let strings = STRINGS[tuning];
        let nearest = self.nearest_string(host);
        let reading = self.reading();
        let fade = self.presence();
        for (i, rect) in Self::string_rects(strings.len()).into_iter().enumerate() {
            let note = strings[i];
            let near = nearest == Some(i);
            let exact = near && reading.is_some_and(|r| r.note == note);
            let in_tune = exact && reading.is_some_and(|r| r.cents.abs() < IN_TUNE);
            let rr = RoundedRect::from_rect(rect, 12.0);
            let accent = theme.accent;
            cx.scene.fill(Fill::NonZero, Affine::IDENTITY, if in_tune { accent.multiply_alpha(0.22 * fade) } else { theme.well }, None, &rr);
            if near {
                glow_stroke(cx, &rr, 1.5, accent.multiply_alpha(fade), accent.multiply_alpha(fade), if in_tune { 6.0 } else { 2.0 });
            } else {
                cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, if self.hot == Some(Hot::String(i)) { theme.text_mid } else { theme.well_border }, None, &rr);
            }
            let (n, sharp, octave) = note_name(note);
            let ink = if near { accent } else { theme.text_mid };
            let style = TextStyle { tabular: true, ..TextStyle::BODY.sized(20.0).weighted(700.0) };
            let label = format!("{n}{}", if sharp { "#" } else { "" });
            let w = cx.text.measure(&label, style) as f64;
            let c = rect.center();
            cx.text.draw(cx.scene, &label, c.x - 0.5 * w - 4.0, c.y - 13.0, style, ink, Align::Left);
            cx.text.draw(cx.scene, &octave.to_string(), c.x + 0.5 * w - 2.0, c.y - 2.0, TextStyle::BODY.sized(11.0).weighted(600.0), ink.multiply_alpha(0.7), Align::Left);
        }
    }

    fn draw_note(&self, cx: &mut DrawCx<'_>) {
        let theme = cx.theme;
        let fade = self.presence();
        let Some(r) = self.reading.filter(|_| fade > 0.0) else {
            // Nothing heard: a dash where the note goes.
            let y = NOTE_MID;
            cx.scene.stroke(&Stroke::new(10.0).with_caps(Cap::Round), Affine::IDENTITY, theme.text_lo.multiply_alpha(0.5), None, &Line::new((CENTRE_X - 50.0, y), (CENTRE_X + 50.0, y)));
            return;
        };
        let in_tune = r.cents.abs() < IN_TUNE;
        let accent = theme.accent;
        let ink = if in_tune { accent } else { theme.text_hi };
        let ink = ink.multiply_alpha(fade);
        let (n, sharp, octave) = note_name(r.note);
        let big = TextStyle { tabular: false, ..TextStyle::BODY.sized(NOTE_SIZE).weighted(700.0) };
        let w = cx.text.measure(n, big) as f64;
        let x = CENTRE_X - 0.5 * w;
        cx.text.draw(cx.scene, n, x, NOTE_TOP, big, ink, Align::Left);
        if sharp {
            cx.text.draw(cx.scene, "#", x + w + 6.0, NOTE_TOP + 26.0, TextStyle { tabular: false, ..TextStyle::BODY.sized(96.0).weighted(700.0) }, ink, Align::Left);
        }
        cx.text.draw(cx.scene, &octave.to_string(), x + w + 10.0, NOTE_TOP + 170.0, TextStyle::BODY.sized(52.0).weighted(600.0), ink.multiply_alpha(0.6), Align::Left);
        // The arrows point to the middle; the one on the side the string is on lights (left: flat,
        // tune up; right: sharp, tune down), as the needle sits.
        let y = NOTE_MID;
        for (side, lit) in [(-1.0, r.cents <= -IN_TUNE), (1.0, r.cents >= IN_TUNE)] {
            let cxp = CENTRE_X + side * 330.0;
            let size = 34.0;
            let mut tri = BezPath::new();
            // Pointing toward the note.
            tri.move_to((cxp + side * -size * 0.6, y));
            tri.line_to((cxp + side * size * 0.6, y - size));
            tri.line_to((cxp + side * size * 0.6, y + size));
            tri.close_path();
            if lit || in_tune {
                let a = accent.multiply_alpha(fade);
                cx.scene.fill(Fill::NonZero, Affine::IDENTITY, a.multiply_alpha(if in_tune { 0.9 } else { 0.75 }), None, &tri);
                glow_stroke(cx, &tri, 1.5, a, a, 4.0);
            } else {
                cx.scene.stroke(&Stroke::new(1.5).with_join(Join::Round), Affine::IDENTITY, theme.well_border, None, &tri);
            }
        }
    }

    fn draw_strip(&self, cx: &mut DrawCx<'_>) {
        let theme = cx.theme;
        let accent = theme.accent;
        let fade = self.presence();
        let reading = self.reading.filter(|_| fade > 0.0);
        let in_tune = reading.is_some_and(|r| r.cents.abs() < IN_TUNE);
        panel(cx.scene, RoundedRect::from_rect(STRIP.inflate(10.0, 8.0), 12.0), theme.well, theme.well_border);
        // The in-tune zone.
        let zone = Rect::new(Self::cents_x(-IN_TUNE), STRIP.y0, Self::cents_x(IN_TUNE), STRIP.y1);
        cx.scene.fill(Fill::NonZero, Affine::IDENTITY, accent.multiply_alpha(if in_tune { 0.35 * fade } else { 0.1 }), None, &zone);
        // Ticks every 5 cents, longer every 10, labels every 25.
        let label = TextStyle { tabular: true, ..TextStyle::BODY.sized(11.0).weighted(600.0) };
        for c in (-50..=50).step_by(5) {
            let x = Self::cents_x(f64::from(c));
            let long = c % 10 == 0;
            let h = if c == 0 { STRIP.height() } else if long { 0.42 * STRIP.height() } else { 0.25 * STRIP.height() };
            let ink = if c == 0 { theme.text_mid } else { theme.grid_major };
            cx.scene.stroke(&Stroke::new(if c == 0 { 2.0 } else { 1.2 }), Affine::IDENTITY, ink, None, &Line::new((x, STRIP.y1), (x, STRIP.y1 - h)));
            if c % 25 == 0 {
                let t = if c > 0 { format!("+{c}") } else if c < 0 { format!("−{}", -c) } else { "0".to_owned() };
                cx.text.draw(cx.scene, &t, x, STRIP.y1 + 12.0, label, theme.text_lo, Align::Center);
            }
        }
        draw_flat(cx, Point::new(STRIP.x0 - 30.0, STRIP.center().y), theme.text_lo);
        cx.text.draw(cx.scene, "#", STRIP.x1 + 30.0, STRIP.center().y - 12.0, TextStyle::BODY.sized(20.0).weighted(600.0), theme.text_lo, Align::Center);
        // The needle.
        if let Some(r) = reading {
            let x = Self::cents_x(r.cents);
            let a = accent.multiply_alpha(fade);
            let needle = Line::new((x, STRIP.y0 - 6.0), (x, STRIP.y1 + 2.0));
            glow_stroke(cx, &needle, 4.0, a, a, if in_tune { 8.0 } else { 4.0 });
            // The frequency and the cents.
            let read = TextStyle { tabular: true, ..TextStyle::BODY.sized(22.0).weighted(600.0) };
            let cents = format!("{}{:.1} ¢", if r.cents >= 0.05 { "+" } else if r.cents <= -0.05 { "−" } else { "" }, r.cents.abs());
            cx.text.draw(cx.scene, &cents, CENTRE_X, STRIP.y0 - 42.0, read, if in_tune { a } else { theme.text_hi.multiply_alpha(fade) }, Align::Center);
            cx.text.draw(cx.scene, &format!("{:.2} Hz", r.hz), STRIP.x0, STRIP.y0 - 40.0, TextStyle { tabular: true, ..TextStyle::BODY.sized(15.0).weighted(600.0) }, theme.text_mid.multiply_alpha(fade), Align::Left);
            let target = note_hz(r.note, f64::from(Self::plain(cx.host, REFERENCE.id)));
            cx.text.draw(cx.scene, &format!("{} = {target:.2} Hz", note_text(r.note)), STRIP.x1, STRIP.y0 - 40.0, TextStyle { tabular: true, ..TextStyle::BODY.sized(15.0).weighted(600.0) }, theme.text_lo.multiply_alpha(fade), Align::Right);
        }
    }

    fn draw_trace(&self, cx: &mut DrawCx<'_>) {
        let theme = cx.theme;
        let accent = theme.accent;
        cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.well, None, &TRACE);
        let y = |c: f64| TRACE.center().y - c.clamp(-50.0, 50.0) / 50.0 * (0.5 * TRACE.height() - 6.0);
        let zone = Rect::new(TRACE.x0, y(IN_TUNE), TRACE.x1, y(-IN_TUNE));
        cx.scene.fill(Fill::NonZero, Affine::IDENTITY, accent.multiply_alpha(0.08), None, &zone);
        for c in [-25.0, 25.0] {
            cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, theme.grid_minor, None, &Line::new((TRACE.x0, y(c)), (TRACE.x1, y(c))));
        }
        cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, theme.grid_major, None, &Line::new((TRACE.x0, y(0.0)), (TRACE.x1, y(0.0))));
        let label = TextStyle { tabular: true, ..TextStyle::BODY.sized(10.0).weighted(600.0) };
        for (c, t) in [(25.0, "+25"), (-25.0, "−25")] {
            cx.text.draw(cx.scene, t, TRACE.x0 + 6.0, y(c) - 13.0, label, theme.text_lo, Align::Left);
        }
        let now = Instant::now();
        let x = |t: Instant| TRACE.x1 - now.duration_since(t).as_secs_f64() / TRACE_SECONDS * TRACE.width();
        cx.begin_clip(&TRACE);
        let mut path = BezPath::new();
        let mut open = false;
        for &(t, c) in &self.trace {
            match c {
                Some(c) => {
                    let p = (x(t), y(c));
                    if open {
                        path.line_to(p);
                    } else {
                        path.move_to(p);
                        open = true;
                    }
                }
                None => open = false,
            }
        }
        glow_stroke(cx, &path, 2.0, accent, accent, 3.0);
        cx.end_clip();
    }

    fn draw_meter(&self, cx: &mut DrawCx<'_>) {
        let theme = cx.theme;
        cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, theme.line, None, &Line::new((STAGE.x1 + 0.5, DISPLAY_TOP), (STAGE.x1 + 0.5, HELP_TOP)));
        panel(cx.scene, RoundedRect::new(WELL_X, METER_TOP, WELL_X + 20.0, METER_BOTTOM, 3.0), theme.meter_well, theme.well_border);
        let seg = Stroke::new(14.0).with_dashes(0.0, [2.0, 2.0]);
        let (top, bottom) = (METER_TOP + 4.0, METER_BOTTOM - 4.0);
        let span = bottom - top;
        let x = WELL_X + 10.0;
        cx.scene.stroke(&seg, Affine::IDENTITY, theme.meter_off, None, &Line::new((x, top), (x, bottom)));
        let yv = bottom - ((f64::from(self.meter) + METER_RANGE).clamp(0.0, METER_RANGE) / METER_RANGE * span / 4.0).round() * 4.0;
        if bottom - yv >= 2.0 {
            cx.scene.stroke(&seg, Affine::IDENTITY, theme.spectrum_pre, None, &Line::new((x, bottom), (x, yv)));
        }
        cx.text.draw(cx.scene, "IN", x, DISPLAY_TOP + 596.0, TextStyle::LABEL.sized(9.0), theme.text_lo, Align::Center);
    }

    fn help_text(&self, host: &dyn ParamHost) -> Help {
        if let Some(menu) = &self.menu {
            let i = self.pointer.and_then(|p| menu.item_rects().iter().position(|r| r.contains(p)));
            return match i {
                Some(0) => Help::default().text("Chromatic — every note, no strings shown"),
                Some(i) => {
                    let strings: Vec<String> = STRINGS[i].iter().map(|&n| note_text(n)).collect();
                    Help::default().text(format!("{} — {}", TUNINGS[i], strings.join(" ")))
                }
                None => Help::default().text("Pick the strings to show; click elsewhere to close."),
            };
        }
        match self.hot {
            Some(Hot::Tuning) => return Help::default().text("Tuning — which strings to show (the tuner reads every note either way)"),
            Some(Hot::String(i)) => {
                let note = STRINGS[Self::tuning(host)][i];
                let hz = note_hz(note, f64::from(Self::plain(host, REFERENCE.id)));
                return Help::default().text(format!("String {} — {} at {hz:.2} Hz", STRINGS[Self::tuning(host)].len() - i, note_text(note)));
            }
            None => {}
        }
        if let Some(h) = self.footer.help(host) {
            return h;
        }
        if self.reading().is_some_and(|r| r.cents.abs() < IN_TUNE) {
            return Help::default().text("In tune (within ±2 cents).");
        }
        Help::default().text("Play one string at a time. The lit arrow is the side you are on: left, flat (tune up); right, sharp (tune down). Below: the last six seconds.")
    }

    fn hot_at(&self, host: &dyn ParamHost, p: Point) -> Option<Hot> {
        if TUNING_PILL.contains(p) {
            return Some(Hot::Tuning);
        }
        Self::string_rects(STRINGS[Self::tuning(host)].len()).iter().position(|r| r.contains(p)).map(Hot::String)
    }

    fn handle_event(&mut self, event: &UiEvent, host: &dyn ParamHost) -> bool {
        if let Some(menu) = &mut self.menu {
            if let UiEvent::PointerMove { pos, .. } = *event {
                self.pointer = Some(pos);
            }
            match menu.event(event) {
                DropdownEvent::Open => {}
                DropdownEvent::Closed => self.menu = None,
                DropdownEvent::Picked(i) => {
                    self.menu = None;
                    host.begin_gesture(TUNING.id);
                    host.set_normalized(TUNING.id, host.normalize(TUNING.id, i as f32));
                    host.end_gesture(TUNING.id);
                }
            }
            return true;
        }
        if let UiEvent::KeyDown { .. } = *event {
            return false;
        }
        if self.footer.event(event, host) {
            return true;
        }
        match *event {
            UiEvent::PointerMove { pos, .. } => {
                self.pointer = Some(pos);
                self.hot = self.hot_at(host, pos);
                true
            }
            UiEvent::PointerLeft => {
                self.pointer = None;
                self.hot = None;
                true
            }
            UiEvent::PointerDown { pos, button: MouseButton::Left, .. } if TUNING_PILL.contains(pos) => {
                self.menu = Some(Dropdown::new(TUNING_PILL, &TUNINGS, Self::tuning(host), (DISPLAY_TOP, HELP_TOP)).with_min_width(TUNING_PILL.width()));
                true
            }
            _ => false,
        }
    }
}

/// A flat sign (the font has none): a stem and its bowl.
fn draw_flat(cx: &mut DrawCx<'_>, c: Point, ink: mite_ui::peniko::Color) {
    let mut p = BezPath::new();
    p.move_to((c.x - 4.0, c.y - 11.0));
    p.line_to((c.x - 4.0, c.y + 7.0));
    p.curve_to((c.x + 2.0, c.y + 4.0), (c.x + 7.0, c.y), (c.x + 4.0, c.y - 2.5));
    p.curve_to((c.x + 2.0, c.y - 4.5), (c.x - 1.5, c.y - 3.0), (c.x - 4.0, c.y - 0.5));
    cx.scene.stroke(&Stroke::new(1.8).with_caps(Cap::Round).with_join(Join::Round), Affine::IDENTITY, ink, None, &p);
}

impl View for TunerView {
    fn logical_size(&self) -> (f64, f64) {
        (VIEW_W, VIEW_H)
    }

    fn draw(&mut self, cx: &mut DrawCx<'_>) {
        let theme = cx.theme;
        self.draw_header(cx);
        cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.display, None, &Rect::new(0.0, DISPLAY_TOP, VIEW_W, HELP_TOP));
        let hole = self.menu.as_ref().map(|m| RoundedRect::from_rect(m.bounds(), 8.0));
        if let Some(h) = hole {
            cx.begin_glow_hole(h);
        }
        self.draw_strings(cx);
        self.draw_note(cx);
        self.draw_strip(cx);
        self.draw_trace(cx);
        self.draw_meter(cx);
        self.help_text(cx.host).draw(cx, Rect::new(0.0, HELP_TOP, VIEW_W, FOOTER_TOP));
        self.footer.draw_frame(cx);
        self.footer.draw_controls(cx);
        if hole.is_some() {
            cx.end_glow_hole();
        }
        if let Some(menu) = &self.menu {
            menu.draw(cx, cx.theme.accent, |_, _, _, _| {});
        }
    }

    fn event(&mut self, event: &UiEvent, host: &dyn ParamHost) -> bool {
        self.session.begin(host);
        let recording = RecordingHost::new(host);
        let changed = self.handle_event(event, &recording);
        let busy = self.menu.is_some() || self.footer.busy();
        self.session.end(host, recording.wrote(), busy);
        self.session.take_replaced();
        changed
    }

    fn tick(&mut self, host: &dyn ParamHost) -> bool {
        let busy = self.menu.is_some() || self.footer.busy();
        self.session.follow(host, busy);
        let dt = self.last_tick.elapsed().as_secs_f32().min(0.1);
        self.last_tick = Instant::now();
        let mut level = SILENT;
        let mut fresh = false;
        self.shared.history.read_since(&mut self.history_seen, |r| {
            level = level.max(r[record::IN].max(r[record::IN + 1]));
            fresh = true;
        });
        self.meter = if level > self.meter { level } else { (self.meter - 20.0 * dt).max(level) };
        if fresh && self.last_detect.elapsed() >= DETECT_EVERY {
            self.last_detect = Instant::now();
            self.analyze(host);
        }
        fresh || busy || self.presence() > 0.0 || self.meter > SILENT + 1.0 || !self.trace.is_empty()
    }
}

