//! The Mite Tape editor, in 1280×760 design units.
//!
//! On top the spectrum: the input dim, the output lit, and the deck's response as measured
//! (`crate::dsp::response`: noise through a machine set as the dials are, so the head bump, the
//! top falling off and the bias's trade are what the engine does, not a drawing of it). Under it
//! the deck: reels at the tape's speed, the transport's speed trace, the level going into the
//! tape, speed and EQ, and the dials. IN and OUT meters on the right; MIX, OUTPUT and Bypass in
//! the footer.

mod deck;
mod spectrum;

use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Instant;

use mite_core::{ParamDisplay, ParamScale, ParamSpec};
use mite_dsp::analyzer::{spectrum_point_freq, SPECTRUM_POINTS};
use mite_ui::kurbo::{Affine, Line, Point, Rect, RoundedRect, Stroke};
use mite_ui::paint::{glow_stroke, panel};
use mite_ui::peniko::Fill;
use mite_ui::session::{Session, SessionConfig};
use mite_ui::widgets::brand::{draw_mark, draw_title};
use mite_ui::widgets::history::smooth_path;
use mite_ui::widgets::{Footer, FooterDial, FooterToggle, Help, KnobKind, ToggleGlyph};
use mite_ui::{Align, DrawCx, ParamHost, RecordingHost, TextStyle, UiEvent, View};

use crate::params::*;
use crate::presets::{preset_ids, FACTORY, PLUGIN};
use crate::TapeShared;
pub use deck::DeckReading;
use deck::Deck;
use spectrum::{freq_x, Spectrum};

/// Mite Tape wears the Tape accent (tokens.json `color.accent.tape`, oxide).
pub const ACCENT: &str = "tape";

const VIEW_W: f64 = 1280.0;
const VIEW_H: f64 = 760.0;
const HEADER: f64 = 56.0;
const DISPLAY_TOP: f64 = HEADER;
const HELP_TOP: f64 = DISPLAY_TOP + 620.0;
const FOOTER_TOP: f64 = HELP_TOP + 28.0;

/// The spectrum, with the deck's response on its own scale (±12 dB about the middle).
const SPEC: Rect = Rect::new(0.0, DISPLAY_TOP, 1180.0, DISPLAY_TOP + 236.0);
pub(crate) const PLOT_X0: f64 = 28.0;
pub(crate) const PLOT_W: f64 = 1124.0;
const RESP_DB: f64 = 12.0;
/// The deck panel under it.
const DECK: Rect = Rect::new(8.0, SPEC.y1 + 8.0, 1172.0, HELP_TOP - 8.0);

const WELLS: [f64; 2] = [1206.0, 1238.0];
const METER_TOP: f64 = DISPLAY_TOP + 20.0;
const METER_BOTTOM: f64 = DISPLAY_TOP + 592.0;
const METER_RANGE: f64 = 60.0;
const SILENT: f32 = -150.0;

fn resp_y(db: f64) -> f64 {
    let mid = SPEC.y0 + 0.42 * SPEC.height();
    mid - db.clamp(-RESP_DB, RESP_DB) / RESP_DB * 0.36 * SPEC.height()
}

/// A stand-in host holding every Mite Tape parameter at its default (tests, preview, tools).
pub fn memory_host() -> mite_ui::MemoryParamHost {
    fn leak(s: String) -> &'static str {
        Box::leak(s.into_boxed_str())
    }
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
    let mut host = mite_ui::MemoryParamHost::new();
    for line in manifest().lines() {
        let f: Vec<&str> = line.split(" | ").collect();
        let id = leak(f[0].to_owned());
        let spec = match f[1] {
            "bool" => index(id, 2, usize::from(f[2] == "true")),
            "choice" => index(id, f[2].split(", ").count(), f[3].parse().unwrap_or(0)),
            _ => ParamSpec { id, ..spec_for(id).expect("every manifest line has a spec") },
        };
        host = host.with(spec);
    }
    host
}

/// The engine's settings as a host's parameters set them.
pub fn settings(host: &dyn ParamHost) -> TapeSettings {
    tape_settings(|id| host.plain(id, host.normalized(id)))
}

/// What the response was measured for: the machine's settings that shape it, and the rate.
#[derive(Debug, Clone, Copy, PartialEq)]
struct ResponseKey {
    speed: usize,
    standard: usize,
    bias: f32,
    level: f32,
    fs: f32,
}

pub struct TapeView {
    shared: Arc<TapeShared>,
    session: Session,
    footer: Footer,
    deck: Deck,
    spectrum: Spectrum,
    /// The deck's measured response, what it was measured for, and a measurement under way.
    response: Option<(ResponseKey, Vec<f32>)>,
    measuring: Option<(ResponseKey, JoinHandle<Vec<f32>>)>,
    last_tick: Instant,
    pointer: Option<Point>,
    meters: [f32; 4],
}

impl TapeView {
    pub fn new(shared: Arc<TapeShared>) -> Self {
        let footer = Footer::new(
            FOOTER_TOP,
            (VIEW_W, VIEW_H),
            vec![
                FooterToggle { id: AUTO_GAIN.id, glyph: ToggleGlyph::Auto, words: "Auto gain — the tape as loud as the input, measured over a few seconds (driven hard it compresses; the head bump and the top it loses move the loudness too), so Bypass and MIX compare fairly" },
                FooterToggle { id: BYPASS.id, glyph: ToggleGlyph::Bypass, words: "Bypass — the input as it came (in time with the deck's latency)" },
            ],
            vec![
                FooterDial { spec: MIX, kind: KnobKind::Unipolar, name: "MIX", words: "Mix — the tape against the input (in time with it, so it never combs)" },
                FooterDial { spec: OUT_GAIN, kind: KnobKind::Bipolar, name: "OUTPUT", words: "Output gain" },
            ],
        );
        Self {
            session: Session::new(SessionConfig { plugin: PLUGIN, factory: FACTORY, sound_ids: preset_ids(), snapshot_ids: preset_ids() }, shared.snapshots.clone(), shared.preset.clone()),
            footer,
            deck: Deck::new(DECK),
            spectrum: Spectrum::new(),
            response: None,
            measuring: None,
            last_tick: Instant::now(),
            pointer: None,
            meters: [SILENT; 4],
            shared,
        }
    }

    pub fn with_preset_dir(mut self, dir: std::path::PathBuf) -> Self {
        self.session = self.session.with_preset_dir(dir);
        self
    }

    /// Lets the spectrum settle and waits for the response (snapshots and tests).
    pub fn settle(&mut self, host: &dyn ParamHost) {
        for _ in 0..60 {
            self.spectrum.update(&self.shared, 0.3);
            self.follow_response(host);
            std::thread::sleep(std::time::Duration::from_millis(16));
        }
        while self.measuring.is_some() {
            self.follow_response(host);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    /// The deck's response as drawn, dB at each spectrum point, once measured (tests and tools).
    pub fn response_db(&self) -> Option<&[f32]> {
        self.response.as_ref().map(|(_, r)| r.as_slice())
    }

    fn fs(&self) -> f32 {
        self.shared.sample_rate.load().max(8000.0)
    }

    fn reading(&self, host: &dyn ParamHost) -> DeckReading {
        DeckReading { running: !settings(host).bypass, speed_dev: self.shared.speed_dev.load(), field_peak: self.shared.field_peak.load() }
    }

    /// Takes a finished measurement and starts the next one when the settings moved on (one at a
    /// time, on its own thread: it takes tens of milliseconds). Whether the drawn response changed.
    fn follow_response(&mut self, host: &dyn ParamHost) -> bool {
        let mut changed = false;
        if self.measuring.as_ref().is_some_and(|(_, h)| h.is_finished()) {
            if let Some((key, handle)) = self.measuring.take() {
                if let Ok(r) = handle.join() {
                    self.response = Some((key, r));
                    changed = true;
                }
            }
        }
        let s = settings(host).machine;
        let key = ResponseKey { speed: s.speed.index(), standard: s.standard.index(), bias: s.bias, level: s.drive_db, fs: self.fs() };
        let have = self.response.as_ref().map(|(k, _)| *k);
        if self.measuring.is_none() && have != Some(key) {
            let fs = key.fs;
            self.measuring = std::thread::Builder::new().name("tape-response".into()).spawn(move || crate::dsp::response(&s, fs)).ok().map(|h| (key, h));
        }
        changed
    }

    fn draw_header(&mut self, cx: &mut DrawCx<'_>) {
        let theme = cx.theme;
        cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.window, None, &Rect::new(0.0, 0.0, VIEW_W, HEADER));
        cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, theme.line, None, &Line::new((0.0, HEADER - 0.5), (VIEW_W, HEADER - 0.5)));
        draw_mark(cx, 16.0, 15.0, 26.0);
        draw_title(cx, 52.0, 18.0, "Tape");
        self.session.draw(cx);
    }

    fn draw_spectrum(&self, cx: &mut DrawCx<'_>) {
        let theme = cx.theme;
        let accent = theme.accent;
        cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.well, None, &SPEC);
        cx.begin_clip(&SPEC);
        self.spectrum.draw(cx, SPEC);
        let label = TextStyle { tabular: true, ..TextStyle::BODY.sized(10.0).weighted(600.0) };
        for (db, t) in [(0.0, "0 dB"), (6.0, "+6"), (-6.0, "−6")] {
            cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, if db == 0.0 { theme.grid_major } else { theme.grid_minor }, None, &Line::new((PLOT_X0, resp_y(db)), (SPEC.x1, resp_y(db))));
            cx.text.draw(cx.scene, t, 6.0, resp_y(db) - 6.0, label, theme.text_lo, Align::Left);
        }
        cx.end_clip();
        if let Some((_, r)) = &self.response {
            // Only 20 Hz–20 kHz: the glow layer is not clipped by the plot.
            let pts: Vec<Point> = (0..SPECTRUM_POINTS)
                .filter(|&i| (20.0..=20_000.0).contains(&spectrum_point_freq(i)))
                .map(|i| Point::new(freq_x(spectrum_point_freq(i)), resp_y(f64::from(r[i]))))
                .collect();
            if pts.len() > 1 {
                let dim = if settings(cx.host).bypass { 0.35 } else { 1.0 };
                glow_stroke(cx, &smooth_path(&pts), 2.0, accent.multiply_alpha(dim), accent.multiply_alpha(dim), 3.0);
            }
        }
    }

    fn draw_meters(&self, cx: &mut DrawCx<'_>) {
        let theme = cx.theme;
        cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, theme.line, None, &Line::new((SPEC.x1 + 0.5, DISPLAY_TOP), (SPEC.x1 + 0.5, HELP_TOP)));
        for x in WELLS {
            panel(cx.scene, RoundedRect::new(x, METER_TOP, x + 20.0, METER_BOTTOM, 3.0), theme.meter_well, theme.well_border);
        }
        let seg = Stroke::new(8.0).with_dashes(0.0, [2.0, 2.0]);
        let (top, bottom) = (METER_TOP + 4.0, METER_BOTTOM - 4.0);
        let span = bottom - top;
        let y_of = |db: f32| bottom - ((f64::from(db) + METER_RANGE).clamp(0.0, METER_RANGE) / METER_RANGE * span / 4.0).round() * 4.0;
        for (i, x) in WELLS.iter().flat_map(|&w| [w + 5.0, w + 15.0]).enumerate() {
            cx.scene.stroke(&seg, Affine::IDENTITY, theme.meter_off, None, &Line::new((x, top), (x, bottom)));
            let line = Line::new((x, bottom), (x, y_of(self.meters[i])));
            let color = if i < 2 { theme.spectrum_pre } else { theme.curve };
            if (line.p1.y - line.p0.y).abs() >= 2.0 {
                cx.scene.stroke(&seg, Affine::IDENTITY, color, None, &line);
            }
        }
        for (x, t) in [(WELLS[0], "IN"), (WELLS[1], "OUT")] {
            cx.text.draw(cx.scene, t, x + 10.0, DISPLAY_TOP + 596.0, TextStyle::LABEL.sized(9.0), theme.text_lo, Align::Center);
        }
    }

    fn help_text(&self, host: &dyn ParamHost) -> Help {
        if let Some(h) = self.deck.help(host) {
            return h;
        }
        if let Some(h) = self.footer.help(host) {
            return h;
        }
        if let Some(h) = self.session.help() {
            return h;
        }
        if self.pointer.is_some_and(|p| SPEC.contains(p)) {
            return Help::default().text("Dim: the input. Lit: the output. The line: what the deck does to a quiet signal at these settings, measured through it (the head bump, the top it loses, the bias's trade).");
        }
        Help::default().text("Turn LEVEL up to drive the tape; pick the speed and the EQ; WOW and FLUTTER move its speed, HISS is its noise.")
    }

    fn busy(&self) -> bool {
        self.deck.busy() || self.footer.busy()
    }

    fn handle_event(&mut self, event: &UiEvent, host: &dyn ParamHost) -> bool {
        if let UiEvent::KeyDown { key, mods } = *event {
            return self.session.key(key, mods, host).unwrap_or(false);
        }
        if let Some(answer) = self.session.event(event, host) {
            return answer;
        }
        if self.footer.event(event, host) {
            return true;
        }
        if let UiEvent::PointerMove { pos, .. } = *event {
            self.pointer = Some(pos);
        }
        if let UiEvent::PointerLeft = *event {
            self.pointer = None;
        }
        self.deck.event(event, host) || matches!(event, UiEvent::PointerMove { .. } | UiEvent::PointerLeft)
    }
}

impl View for TapeView {
    fn logical_size(&self) -> (f64, f64) {
        (VIEW_W, VIEW_H)
    }

    fn draw(&mut self, cx: &mut DrawCx<'_>) {
        let theme = cx.theme;
        self.draw_header(cx);
        cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.display, None, &Rect::new(0.0, DISPLAY_TOP, VIEW_W, HELP_TOP));
        self.draw_spectrum(cx);
        self.deck.draw(cx);
        self.draw_meters(cx);
        self.help_text(cx.host).draw(cx, Rect::new(0.0, HELP_TOP, VIEW_W, FOOTER_TOP));
        self.footer.draw_frame(cx);
        self.footer.draw_controls(cx);
        self.session.draw_menu(cx);
    }

    fn event(&mut self, event: &UiEvent, host: &dyn ParamHost) -> bool {
        self.session.begin(host);
        let recording = RecordingHost::new(host);
        let changed = self.handle_event(event, &recording);
        let busy = self.busy();
        self.session.end(host, recording.wrote(), busy);
        self.session.take_replaced();
        changed
    }

    fn tick(&mut self, host: &dyn ParamHost) -> bool {
        let busy = self.busy();
        self.session.follow(host, busy);
        let dt = self.last_tick.elapsed().as_secs_f32().min(0.1);
        self.last_tick = Instant::now();
        let fall = 20.0 * dt;
        let (il, ir) = self.shared.input_meter.load();
        let (ol, or) = self.shared.output_meter.load();
        for (m, t) in self.meters.iter_mut().zip([il, ir, ol, or]) {
            *m = if t > *m { t } else { (*m - fall).max(t) };
        }
        let reading = self.reading(host);
        self.deck.advance(host, reading);
        let spectrum = self.spectrum.update(&self.shared, f64::from(dt));
        let response = self.follow_response(host);
        spectrum || response || busy || self.measuring.is_some() || self.deck.moving(reading) || self.meters.iter().any(|&m| m > SILENT + 1.0)
    }
}
