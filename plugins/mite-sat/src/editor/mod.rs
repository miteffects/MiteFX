//! The Mite Sat editor (docs/design/sat-brief.md §3), in 1280×760 design units.
//!
//! The display, top to bottom: the spectrum (input dim, output lit, the harmonics the saturation
//! adds filled in the accent) and the band's card: its model icons (each a drawing of its own
//! curve), the transfer square (the curve the DSP plays, lit where the music reaches) and its
//! dials. Everything drawn comes from `mite_dsp::sat` and the engine's history records.

mod curve;
mod glyphs;
mod spectrum;

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Instant;

use mite_core::{ParamDisplay, ParamScale, ParamSpec};
use mite_dsp::sat::{bias_point, Layout, Model, CROSSOVERS, DRIVE_MAX_DB, TONE_MAX_DB};
use mite_ui::kurbo::{Affine, BezPath, Line, Point, Rect, RoundedRect, Stroke};
use mite_ui::paint::{icon_button, panel};
use mite_ui::peniko::Fill;
use mite_ui::session::{Session, SessionConfig};
use mite_ui::widgets::brand::{draw_mark, draw_title};
use mite_ui::widgets::footer::draw_dropdown_button;
use mite_ui::widgets::{Dropdown, DropdownEvent, Footer, FooterDial, FooterToggle, Help, Knob, KnobKind, ToggleGlyph};
use mite_ui::{write_if_changed, Align, DrawCx, Key, Modifiers, MouseButton, ParamHost, RecordingHost, TextStyle, UiEvent, View};

use crate::dsp::record;
use crate::params::*;
use crate::presets::{preset_ids, FACTORY, PLUGIN};
use crate::SatShared;

pub use spectrum::ANALYZER_SIZE;

/// Mite Sat wears the Sat accent (tokens.json `color.accent.sat`, crimson).
pub const ACCENT: &str = "sat";

const WIDTH: f64 = 1280.0;
const HEIGHT: f64 = 760.0;
const HEADER: f64 = 56.0;
const DISPLAY_TOP: f64 = HEADER;
const DISPLAY_H: f64 = 620.0;
const HELP_TOP: f64 = DISPLAY_TOP + DISPLAY_H;
const FOOTER_TOP: f64 = HELP_TOP + 28.0;

/// The display left of the meters.
const MAIN_W: f64 = 1180.0;
/// The spectrum: 20 Hz at x 14, 20 kHz at x 1166.
pub(crate) const PLOT_X0: f64 = 14.0;
pub(crate) const PLOT_W: f64 = 1152.0;
const SPECTRUM: Rect = Rect::new(PLOT_X0, DISPLAY_TOP + 12.0, PLOT_X0 + PLOT_W, DISPLAY_TOP + 236.0);
/// The band's card.
const CARD: Rect = Rect::new(PLOT_X0, DISPLAY_TOP + 252.0, PLOT_X0 + PLOT_W, DISPLAY_TOP + 608.0);
/// Meter release (tokens.json `meter.releaseDbPerSec`).
const METER_RELEASE_DB_S: f32 = 20.0;
const METER_TOP: f64 = DISPLAY_TOP + 20.0;
const METER_BOTTOM: f64 = DISPLAY_TOP + 592.0;
/// The band's peak on the curve: 60 ms up, 300 ms down (as the spectrum).
const PEAK_RISE_S: f64 = 0.06;
const PEAK_FALL_S: f64 = 0.3;
/// Drag on the curve: drive per px up, bias per px across (Shift: a fifth), tone per px (Alt).
const DRIVE_PER_PX: f32 = 0.12;
const BIAS_PER_PX: f32 = 0.7;
const TONE_PER_PX: f32 = 0.06;

/// The card's parts.
fn square() -> Rect {
    Rect::new(CARD.x0 + 18.0, CARD.y0 + 60.0, CARD.x0 + 18.0 + 280.0, CARD.y0 + 340.0)
}

fn model_rect(i: usize) -> Rect {
    let x = CARD.x0 + 18.0 + i as f64 * 50.0;
    Rect::new(x, CARD.y0 + 12.0, x + 44.0, CARD.y0 + 48.0)
}

fn bypass_rect() -> Rect {
    Rect::new(CARD.x1 - 54.0, CARD.y0 + 12.0, CARD.x1 - 18.0, CARD.y0 + 48.0)
}

/// The models offered (those built so far), in icon order.
fn offered() -> Vec<Model> {
    Model::ALL.into_iter().filter(|m| m.is_built()).collect()
}

/// The card's dials: drive large, then bias, tone, mix and output.
const DIALS: [(ParamSpec, &str, KnobKind); 5] = [
    (B_DRIVE, "DRIVE", KnobKind::Unipolar),
    (B_BIAS, "BIAS", KnobKind::Bipolar),
    (B_TONE, "TONE", KnobKind::Bipolar),
    (B_MIX, "MIX", KnobKind::Unipolar),
    (B_OUT, "OUTPUT", KnobKind::Bipolar),
];

/// The dials spread evenly over the card right of the square.
fn dial_center(k: usize) -> (Point, f64) {
    let y = square().center().y - 4.0;
    let (x0, x1) = (square().x1 + 30.0, CARD.x1 - 20.0);
    let step = (x1 - x0) / DIALS.len() as f64;
    (Point::new(x0 + step * (k as f64 + 0.5), y), if k == 0 { 104.0 } else { 76.0 })
}

/// A Catmull-Rom curve through `pts` (no corners between samples).
pub(crate) fn smooth_path(pts: &[Point]) -> BezPath {
    let mut p = BezPath::new();
    if pts.is_empty() {
        return p;
    }
    p.move_to(pts[0]);
    for i in 0..pts.len().saturating_sub(1) {
        let p0 = pts[i.saturating_sub(1)];
        let (p1, p2) = (pts[i], pts[i + 1]);
        let p3 = pts[(i + 2).min(pts.len() - 1)];
        p.curve_to(p1 + (p2 - p0) / 6.0, p2 - (p3 - p1) / 6.0, p2);
    }
    p
}

fn fmt_signed(v: f64, unit: &str) -> String {
    format!("{}{:.1}{unit}", if v > 0.05 { "+" } else if v < -0.05 { "−" } else { "" }, v.abs())
}

/// A stand-in host holding every Mite Sat parameter at its default, for tests, the preview and
/// benchmarks.
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
            _ => {
                // Globals by their whole id, slot parameters (`x2_freq`, `lfo1_rate`, `m3_amt`) by
                // the part after the slot.
                let globals = [MIX, OUT_PAN, OUT_GAIN, ENV_ATT, ENV_REL, ENV_GAIN];
                let short = id.split_once('_').map_or(id, |(_, rest)| rest);
                let pool: &[ParamSpec] = if id.starts_with('x') {
                    &[X_FREQ]
                } else if id.starts_with("lfo") {
                    &[LFO_RATE, LFO_PHASE]
                } else if id.starts_with('b') {
                    &BAND_SPECS
                } else {
                    &[M_AMT]
                };
                let base = globals.into_iter().find(|s| s.id == id).or_else(|| pool.iter().copied().find(|s| s.id == short)).expect("every manifest line has a spec");
                ParamSpec { id, default: f[4].parse().unwrap_or(base.default), ..base }
            }
        };
        host = host.with(spec);
    }
    host
}

/// The engine's settings as a host's parameters set them (as `SatParams::settings` does), for
/// tests, the preview and tools that drive the engine from a stand-in host.
pub fn settings(host: &dyn ParamHost) -> mite_dsp::sat::SatSettings {
    let p = |id: &str| host.plain(id, host.normalized(id));
    let on = |id: &str| p(id) >= 0.5;
    mite_dsp::sat::SatSettings {
        crossovers: std::array::from_fn(|k| on(&cross_id(k, X_ON.id)).then(|| p(&cross_id(k, X_FREQ.id)))),
        bands: std::array::from_fn(|b| mite_dsp::sat::BandSat {
            model: Model::from_index(p(&band_id(b, B_MODEL.id)).round().max(0.0) as usize),
            drive_db: p(&band_id(b, B_DRIVE.id)),
            bias: p(&band_id(b, B_BIAS.id)) / 100.0,
            tone_db: p(&band_id(b, B_TONE.id)),
            mix: p(&band_id(b, B_MIX.id)) / 100.0,
            out_db: p(&band_id(b, B_OUT.id)),
            bypass: on(&band_id(b, B_BYPASS.id)),
            mute: on(&band_id(b, B_MUTE.id)),
            solo: on(&band_id(b, B_SOLO.id)),
        }),
        auto_gain: on(AUTO_GAIN.id),
    }
}

/// The oversampling factor a host's parameter sets.
pub fn oversampling(host: &dyn ParamHost) -> usize {
    factor(host.plain(OVERSAMPLING.id, host.normalized(OVERSAMPLING.id)).round() as i32)
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Drag {
    /// On the transfer square: drive up and down, bias across (Alt: tone).
    Curve { start: Point, drive: f32, bias: f32, tone: f32, alt: bool },
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Hot {
    Square,
    Model(Model),
    Bypass,
    Oversampling,
}

pub struct SatView {
    shared: Arc<SatShared>,
    session: Session,
    footer: Footer,
    pointer: Option<Point>,
    hot: Option<Hot>,
    drag: Option<Drag>,
    os_menu: Option<Dropdown>,
    os_hit: Rect,
    knobs: Vec<Knob>,
    knob_band: Option<usize>,
    /// The newest history records, the tap's read position, and the meters as drawn (IN L/R,
    /// OUT L/R) with their ballistics.
    history: VecDeque<[f32; record::WIDTH]>,
    seen: usize,
    meters: [f32; 4],
    last_fresh: Instant,
    last_tick: Instant,
    spectrum: spectrum::Spectrum,
    /// The selected band's peak going in (linear), operating point and auto gain, eased.
    peak: f32,
    point: f32,
    auto: f32,
}

impl SatView {
    pub fn new(shared: Arc<SatShared>) -> Self {
        let footer = Footer::new(
            FOOTER_TOP,
            (WIDTH, HEIGHT),
            vec![
                FooterToggle { id: AUTO_GAIN.id, glyph: ToggleGlyph::Auto, words: "Auto gain — each band comes out as loud as it went in, however hard it is driven" },
                FooterToggle { id: DELTA.id, glyph: ToggleGlyph::Delta, words: "Delta — hear only what the saturation adds and takes away" },
                FooterToggle { id: BYPASS.id, glyph: ToggleGlyph::Bypass, words: "Bypass — the whole plugin, latency kept" },
            ],
            vec![
                FooterDial { spec: MIX, kind: KnobKind::Unipolar, name: "MIX", words: "Mix — the saturation against the dry signal, delay-compensated" },
                FooterDial { spec: OUT_PAN, kind: KnobKind::Bipolar, name: "PAN", words: "Output balance" },
                FooterDial { spec: OUT_GAIN, kind: KnobKind::Bipolar, name: "OUTPUT", words: "Output gain" },
            ],
        );
        Self {
            session: Session::new(SessionConfig { plugin: PLUGIN, factory: FACTORY, sound_ids: preset_ids(), snapshot_ids: preset_ids() }, shared.snapshots.clone(), shared.preset.clone()),
            footer,
            pointer: None,
            hot: None,
            drag: None,
            os_menu: None,
            os_hit: Rect::ZERO,
            knobs: Vec::new(),
            knob_band: None,
            history: VecDeque::with_capacity(64),
            seen: 0,
            meters: [-150.0; 4],
            last_fresh: Instant::now(),
            last_tick: Instant::now(),
            spectrum: spectrum::Spectrum::new(),
            peak: 0.0,
            point: 0.0,
            auto: 1.0,
            shared,
        }
    }

    /// Keeps the user's presets in `dir` instead of the settings folder (tests).
    pub fn with_preset_dir(mut self, dir: std::path::PathBuf) -> Self {
        self.session = self.session.with_preset_dir(dir);
        self
    }

    pub fn session(&self) -> &Session {
        &self.session
    }

    /// The transfer square's rectangle (tests and tools).
    pub fn square(&self) -> Rect {
        square()
    }

    /// Where model `m`'s icon sits, if it is offered (tests and tools).
    pub fn model_icon(&self, m: Model) -> Option<Rect> {
        offered().iter().position(|&o| o == m).map(model_rect)
    }

    /// Opens the oversampling list (tools).
    pub fn open_oversampling(&mut self, host: &dyn ParamHost) {
        self.os_menu = Some(Self::os_list(host));
    }

    /// The open oversampling list's rows, 1× first (tests and tools).
    pub fn oversampling_rows(&self) -> Vec<Rect> {
        self.os_menu.as_ref().map(Dropdown::item_rects).unwrap_or_default()
    }

    // ---- parameters -------------------------------------------------------------------------

    fn plain(host: &dyn ParamHost, id: &str) -> f32 {
        host.plain(id, host.normalized(id))
    }

    fn on(host: &dyn ParamHost, id: &str) -> bool {
        Self::plain(host, id) >= 0.5
    }

    /// Writes a plain value as one gesture.
    fn set(host: &dyn ParamHost, id: &str, plain: f32) {
        write_if_changed(host, id, host.normalize(id, plain));
    }

    /// Writes without a gesture (inside one already begun).
    fn set_raw(host: &dyn ParamHost, id: &str, plain: f32) {
        host.set_normalized(id, host.normalize(id, plain));
    }

    fn layout(host: &dyn ParamHost) -> Layout {
        Layout::new(&std::array::from_fn::<_, CROSSOVERS, _>(|k| Self::on(host, &cross_id(k, X_ON.id)).then(|| Self::plain(host, &cross_id(k, X_FREQ.id)))))
    }

    /// The band the card shows (one band for now: the lowest region).
    fn band(host: &dyn ParamHost) -> usize {
        Self::layout(host).owners[0]
    }

    fn model(host: &dyn ParamHost, b: usize) -> Model {
        Model::from_index(Self::plain(host, &band_id(b, B_MODEL.id)).round().max(0.0) as usize)
    }

    fn curve(&self, host: &dyn ParamHost, b: usize) -> curve::Curve {
        let model = Self::model(host, b);
        let bias = Self::plain(host, &band_id(b, B_BIAS.id)) / 100.0;
        // Tube's point drifts with the music: the engine reports where it sits.
        let point = if model.is_stateful() && self.history.back().is_some() { self.point } else { bias_point(bias) };
        let auto = if Self::on(host, AUTO_GAIN.id) { self.auto } else { 1.0 };
        curve::Curve { model, drive_db: Self::plain(host, &band_id(b, B_DRIVE.id)), point, auto, peak: self.peak }
    }

    fn os_list(host: &dyn ParamHost) -> Dropdown {
        let current = Self::plain(host, OVERSAMPLING.id).round().max(0.0) as usize;
        let anchor = Rect::new(16.0, FOOTER_TOP + 9.0, 126.0, FOOTER_TOP + 47.0);
        Dropdown::new(anchor, OVERSAMPLING.choices, current, (DISPLAY_TOP, FOOTER_TOP)).with_min_width(150.0)
    }

    // ---- drawing ----------------------------------------------------------------------------

    fn draw_header(&mut self, cx: &mut DrawCx<'_>) {
        let theme = cx.theme;
        cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.window, None, &Rect::new(0.0, 0.0, WIDTH, HEADER));
        cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, theme.line, None, &Line::new((0.0, HEADER - 0.5), (WIDTH, HEADER - 0.5)));
        draw_mark(cx, 16.0, 15.0, 26.0);
        draw_title(cx, 52.0, 18.0, "Sat");
        self.session.draw(cx);
    }

    fn draw_card(&self, cx: &mut DrawCx<'_>, b: usize) {
        let theme = cx.theme;
        let frame = RoundedRect::from_rect(CARD, 12.0);
        cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.window, None, &frame);
        cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.control.multiply_alpha(0.55), None, &frame);
        cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, theme.well_border, None, &frame);
        let bypassed = Self::on(cx.host, &band_id(b, B_BYPASS.id));
        let current = Self::model(cx.host, b);
        let hot = |r: Rect| self.pointer.is_some_and(|p| r.contains(p));
        for (i, m) in offered().into_iter().enumerate() {
            let r = model_rect(i);
            let ink = icon_button(cx.scene, theme, r, theme.panel_control, hot(r), m == current, theme.accent);
            glyphs::draw_model(cx.scene, m, r.center(), 26.0, ink, 1.6);
        }
        let r = bypass_rect();
        let ink = icon_button(cx.scene, theme, r, theme.panel_control, hot(r), !bypassed, theme.accent);
        glyphs::draw(cx.scene, glyphs::POWER, r.center(), 18.0, ink, 1.8);
        let ghost = match self.hot {
            Some(Hot::Model(m)) if m != current => Some(m),
            _ => None,
        };
        let alpha = if bypassed { 0.45 } else { 1.0 };
        curve::draw(cx, square(), &self.curve(cx.host, b), ghost, true, alpha);
        for k in &self.knobs {
            k.draw(cx);
        }
    }

    fn draw_meters(&self, cx: &mut DrawCx<'_>) {
        let theme = cx.theme;
        cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, theme.line, None, &Line::new((MAIN_W + 0.5, DISPLAY_TOP), (MAIN_W + 0.5, HELP_TOP)));
        for x in [1196.0, 1238.0] {
            panel(cx.scene, RoundedRect::new(x, METER_TOP, x + 26.0, METER_BOTTOM, 3.0), theme.meter_well, theme.well_border);
        }
        let seg = Stroke::new(8.0).with_dashes(0.0, [2.0, 2.0]);
        let (top, bottom) = (METER_TOP + 4.0, METER_BOTTOM - 4.0);
        let span = bottom - top;
        let level_y = |db: f32| bottom - (f64::from(db.clamp(-60.0, 0.0) + 60.0) / 60.0 * span / 4.0).round() * 4.0;
        for (i, x) in [1203.0, 1213.0, 1245.0, 1255.0].into_iter().enumerate() {
            cx.scene.stroke(&seg, Affine::IDENTITY, theme.meter_off, None, &Line::new((x, top), (x, bottom)));
            let line = Line::new((x, bottom), (x, level_y(self.meters[i])));
            // Over 0 dBFS the segment lights in warn (Sat draws no danger role).
            let color = if self.meters[i] > 0.0 { theme.warn } else if i < 2 { theme.spectrum_pre } else { theme.curve };
            if (line.p1.y - line.p0.y).abs() >= 2.0 {
                cx.scene.stroke(&seg, Affine::IDENTITY, color, None, &line);
                if i >= 2 {
                    cx.glow().stroke(&Stroke::new(8.0), Affine::IDENTITY, color.multiply_alpha(0.3), None, &line);
                }
            }
        }
        for (x, t) in [(1209.0, "IN"), (1251.0, "OUT")] {
            cx.text.draw(cx.scene, t, x, DISPLAY_TOP + 596.0, TextStyle::LABEL.sized(9.0), theme.text_lo, Align::Center);
        }
    }

    fn draw_footer(&mut self, cx: &mut DrawCx<'_>) {
        let theme = cx.theme;
        self.footer.draw_frame(cx);
        let r = Rect::new(16.0, FOOTER_TOP + 9.0, 126.0, FOOTER_TOP + 47.0);
        let hot = self.pointer.is_some_and(|p| r.contains(p));
        let text = OVERSAMPLING.choices[(Self::plain(cx.host, OVERSAMPLING.id).round().max(0.0) as usize).min(3)];
        draw_dropdown_button(cx, r, text, self.os_menu.is_some(), hot, |scene, at| glyphs::draw(scene, glyphs::OVERSAMPLE, at + (8.0, 6.0), 22.0, theme.text_mid, 1.6));
        self.os_hit = r;
        let sample_rate = f64::from(self.shared.sample_rate.load()).max(1.0);
        self.footer.draw_latency(cx, 142.0, f64::from(self.shared.latency.load()) / sample_rate * 1000.0);
        self.footer.draw_controls(cx);
        if let Some(list) = &self.os_menu {
            list.draw(cx, theme.accent, |scene, i, at, ink| {
                // Each factor's icon: as many waves as it oversamples.
                let n = [1usize, 2, 4, 8][i];
                let mut p = BezPath::new();
                for k in 0..=32 {
                    let x = at.x + 1.0 + 20.0 * k as f64 / 32.0;
                    let y = at.y + 12.0 - 5.0 * (std::f64::consts::TAU * n as f64 * k as f64 / 32.0 / 2.0).sin();
                    if k == 0 {
                        p.move_to((x, y));
                    } else {
                        p.line_to((x, y));
                    }
                }
                scene.stroke(&Stroke::new(1.4), Affine::IDENTITY, ink, None, &p);
            });
        }
    }

    fn draw_states(&self, cx: &mut DrawCx<'_>) {
        let theme = cx.theme;
        if Self::on(cx.host, DELTA.id) {
            let r = RoundedRect::from_rect(Rect::new(SPECTRUM.x0 + 10.0, SPECTRUM.y0 + 10.0, SPECTRUM.x0 + 46.0, SPECTRUM.y0 + 46.0), 10.0);
            cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.accent.multiply_alpha(0.18), None, &r);
            mite_ui::paint::glow_stroke(cx, &r, 1.5, theme.accent, theme.accent, 9.0);
            mite_ui::widgets::footer::draw_toggle_glyph(cx.scene, ToggleGlyph::Delta, r.center(), theme.accent);
        }
    }

    // ---- help -------------------------------------------------------------------------------

    fn help_text(&self, host: &dyn ParamHost) -> Help {
        let b = Self::band(host);
        let drive = f64::from(Self::plain(host, &band_id(b, B_DRIVE.id)));
        let bias = f64::from(Self::plain(host, &band_id(b, B_BIAS.id)));
        let tone = f64::from(Self::plain(host, &band_id(b, B_TONE.id)));
        if let Some(Drag::Curve { alt, .. }) = self.drag {
            if alt {
                return Help::default().text("Tone ").slot(fmt_signed(tone, " dB"), "+12.0 dB").text(" — tilts what the saturation gives, around the band's centre.");
            }
            return Help::default().text("Drive ").slot(format!("{drive:.1} dB"), "36.0 dB").text("  bias ").slot(format!("{bias:+.0} %"), "+100 %").text(" — up drives harder, across moves the operating point. Shift is fine.");
        }
        if let Some(list) = &self.os_menu {
            let fs = f64::from(self.shared.sample_rate.load()).max(1.0);
            let hovered = self.pointer.and_then(|p| list.item_rects().iter().position(|r| r.contains(p)));
            if let Some(i) = hovered {
                let f = [1usize, 2, 4, 8][i];
                let ms = mite_dsp::oversample::Oversampler::latency_for(f) as f64 / fs * 1000.0;
                return Help::default().text(format!("{} — ", OVERSAMPLING.choices[i])).text(if f == 1 { "no oversampling: no latency, the most aliasing".to_owned() } else { format!("the curves run {f} times faster, so their harmonics fold back far less; adds {ms:.2} ms of latency") });
            }
        }
        match self.hot {
            Some(Hot::Square) => Help::default().text("Drive ").slot(format!("{drive:.1} dB"), "36.0 dB").text("  bias ").slot(format!("{bias:+.0} %"), "+100 %").text(" — drag up for more drive, across for bias; Alt-drag: tone; wheel: drive; double-click resets."),
            Some(Hot::Model(m)) => Help::default().text(format!("{} — {}", m.name(), model_words(m))),
            Some(Hot::Bypass) => Help::default().text(if Self::on(host, &band_id(b, B_BYPASS.id)) { "The band passes clean — click to saturate it again." } else { "Band on — click to let it pass clean (still heard)." }),
            Some(Hot::Oversampling) => Help::default().text("Oversampling — how many times faster the curves run, so their harmonics do not fold back into the audible band."),
            None => {
                if let Some(h) = self.footer.help(host) {
                    return h;
                }
                if let Some(k) = self.knobs.iter().find(|k| self.pointer.is_some_and(|p| k.bounds().contains(p))) {
                    let v = Self::plain(host, &k.id);
                    return Help::default().text(format!("{} ", k.spec.name)).slot(k.spec.format(v), k.spec.format(k.spec.max)).text(knob_words(k.spec.id));
                }
                if let Some(h) = self.session.help() {
                    return h;
                }
                if drive == 0.0 {
                    Help::default().text("Clean: drag the curve up, or turn DRIVE, to saturate.")
                } else {
                    Help::default().text("Lit: where the music reaches on the curve. Crimson in the spectrum: the harmonics it adds.")
                }
            }
        }
    }

    // ---- events -----------------------------------------------------------------------------

    fn sync_knobs(&mut self, host: &dyn ParamHost) {
        let b = Self::band(host);
        if self.knob_band == Some(b) || self.knobs.iter().any(Knob::is_dragging) {
            return;
        }
        self.knobs = DIALS
            .iter()
            .enumerate()
            .map(|(k, (spec, label, kind))| {
                let (c, d) = dial_center(k);
                let knob = Knob::new(*spec, label, *kind, c, d).with_id(band_id(b, spec.id));
                if k == 0 { knob.with_curve(DRIVE_DIAL) } else { knob }
            })
            .collect();
        self.knob_band = Some(b);
    }

    fn busy(&self) -> bool {
        self.drag.is_some() || self.knobs.iter().any(Knob::is_dragging) || self.footer.busy()
    }

    fn hot_at(&self, p: Point) -> Option<Hot> {
        if square().contains(p) {
            return Some(Hot::Square);
        }
        if let Some(i) = (0..offered().len()).find(|&i| model_rect(i).contains(p)) {
            return Some(Hot::Model(offered()[i]));
        }
        if bypass_rect().contains(p) {
            return Some(Hot::Bypass);
        }
        if self.os_hit.contains(p) {
            return Some(Hot::Oversampling);
        }
        None
    }

    fn key_event(&mut self, key: Key, mods: Modifiers, host: &dyn ParamHost) -> bool {
        if let Some(used) = self.session.key(key, mods, host) {
            return used;
        }
        let b = Self::band(host);
        match key {
            Key::Escape if self.os_menu.is_some() => {
                self.os_menu = None;
                true
            }
            Key::Up | Key::Down => {
                let step = if mods.fine() { 0.1 } else { 0.5 } * if key == Key::Up { 1.0 } else { -1.0 };
                let id = band_id(b, B_DRIVE.id);
                Self::set(host, &id, (Self::plain(host, &id) + step).clamp(0.0, DRIVE_MAX_DB));
                true
            }
            Key::Left | Key::Right => {
                // The previous or next model.
                let models = offered();
                let at = models.iter().position(|&m| m == Self::model(host, b)).unwrap_or(0);
                let next = if key == Key::Right { (at + 1).min(models.len() - 1) } else { at.saturating_sub(1) };
                Self::set(host, &band_id(b, B_MODEL.id), models[next].index() as f32);
                true
            }
            _ => false,
        }
    }

    fn handle_event(&mut self, event: &UiEvent, host: &dyn ParamHost) -> bool {
        if let UiEvent::KeyDown { key, mods } = *event {
            return self.key_event(key, mods, host);
        }
        if let Some(list) = &mut self.os_menu {
            if let UiEvent::PointerMove { pos, .. } = *event {
                self.pointer = Some(pos);
            }
            match list.event(event) {
                DropdownEvent::Open => {}
                DropdownEvent::Picked(i) => {
                    self.os_menu = None;
                    Self::set(host, OVERSAMPLING.id, i as f32);
                }
                DropdownEvent::Closed => self.os_menu = None,
            }
            return true;
        }
        if let Some(answer) = self.session.event(event, host) {
            return answer;
        }
        if self.footer.event(event, host) {
            return true;
        }
        for k in &mut self.knobs {
            if k.event(event, host) {
                return true;
            }
        }
        let b = Self::band(host);
        match *event {
            UiEvent::PointerMove { pos, mods } => {
                self.pointer = Some(pos);
                if let Some(Drag::Curve { start, drive, bias, tone, alt }) = self.drag {
                    let k = if mods.fine() { 0.2 } else { 1.0 };
                    let (dx, dy) = ((pos.x - start.x) as f32, (start.y - pos.y) as f32);
                    if alt {
                        Self::set_raw(host, &band_id(b, B_TONE.id), (tone + dy * TONE_PER_PX * k).clamp(-TONE_MAX_DB, TONE_MAX_DB));
                    } else {
                        Self::set_raw(host, &band_id(b, B_DRIVE.id), (drive + dy * DRIVE_PER_PX * k).clamp(0.0, DRIVE_MAX_DB));
                        Self::set_raw(host, &band_id(b, B_BIAS.id), (bias + dx * BIAS_PER_PX * k).clamp(-100.0, 100.0));
                    }
                    return true;
                }
                self.hot = self.hot_at(pos);
                true
            }
            UiEvent::PointerLeft => {
                self.pointer = None;
                self.hot = None;
                true
            }
            UiEvent::PointerDown { pos, button: MouseButton::Left, mods } => match self.hot_at(pos) {
                Some(Hot::Square) => {
                    let alt = mods.alt;
                    let ids: Vec<String> = if alt { vec![band_id(b, B_TONE.id)] } else { vec![band_id(b, B_DRIVE.id), band_id(b, B_BIAS.id)] };
                    for id in &ids {
                        host.begin_gesture(id);
                    }
                    let p = |s: &ParamSpec| Self::plain(host, &band_id(b, s.id));
                    self.drag = Some(Drag::Curve { start: pos, drive: p(&B_DRIVE), bias: p(&B_BIAS), tone: p(&B_TONE), alt });
                    true
                }
                Some(Hot::Model(m)) => {
                    Self::set(host, &band_id(b, B_MODEL.id), m.index() as f32);
                    true
                }
                Some(Hot::Bypass) => {
                    let on = Self::on(host, &band_id(b, B_BYPASS.id));
                    Self::set(host, &band_id(b, B_BYPASS.id), if on { 0.0 } else { 1.0 });
                    true
                }
                Some(Hot::Oversampling) => {
                    self.os_menu = Some(Self::os_list(host));
                    true
                }
                None => false,
            },
            UiEvent::PointerUp { button: MouseButton::Left, .. } => match self.drag.take() {
                Some(Drag::Curve { alt, .. }) => {
                    let ids: Vec<String> = if alt { vec![band_id(b, B_TONE.id)] } else { vec![band_id(b, B_DRIVE.id), band_id(b, B_BIAS.id)] };
                    for id in &ids {
                        host.end_gesture(id);
                    }
                    true
                }
                None => false,
            },
            UiEvent::DoubleClick { pos, .. } if square().contains(pos) => {
                self.drag = None;
                Self::set(host, &band_id(b, B_DRIVE.id), B_DRIVE.default);
                Self::set(host, &band_id(b, B_BIAS.id), B_BIAS.default);
                true
            }
            UiEvent::Wheel { pos, lines, mods } if square().contains(pos) => {
                let id = band_id(b, B_DRIVE.id);
                Self::set(host, &id, B_DRIVE.wheel(Self::plain(host, &id), lines, mods.fine()));
                true
            }
            _ => false,
        }
    }

    fn after_session(&mut self) {
        if self.session.take_replaced() {
            self.knob_band = None;
        }
    }
}

fn model_words(m: Model) -> &'static str {
    match m {
        Model::Tube => "asymmetric, so even harmonics; pushed harder, its bias drifts and it warms further",
        Model::Diode => "a hard knee into a soft, never-ending climb",
        Model::Soft => "a gentle cubic: odd harmonics, rounding into a ceiling",
        Model::Hard => "clipping, its corner rounded so it folds nothing harsh back",
        Model::Fold => "a wavefolder: past the top it folds back down",
        Model::Rectify => "full-wave: the fundamental gives way to the octave above",
        Model::Tape | Model::Transformer => "",
    }
}

fn knob_words(id: &str) -> &'static str {
    match id {
        "drive" => " — how hard the band goes into the curve (0 dB: clean)",
        "bias" => " — moves the operating point off centre: even harmonics",
        "tone" => " — tilts what the saturation gives around the band's centre",
        "mix" => " — this band's saturation against its clean signal",
        "out" => " — this band's level in the sum",
        _ => "",
    }
}

impl View for SatView {
    fn logical_size(&self) -> (f64, f64) {
        (WIDTH, HEIGHT)
    }

    fn draw(&mut self, cx: &mut DrawCx<'_>) {
        self.sync_knobs(cx.host);
        let theme = cx.theme;
        let b = Self::band(cx.host);
        self.draw_header(cx);
        cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.display, None, &Rect::new(0.0, DISPLAY_TOP, WIDTH, HELP_TOP));
        self.spectrum.draw(cx, SPECTRUM);
        self.draw_card(cx, b);
        self.draw_meters(cx);
        self.draw_states(cx);
        self.help_text(cx.host).draw(cx, Rect::new(0.0, HELP_TOP, WIDTH, FOOTER_TOP));
        self.draw_footer(cx);
        self.session.draw_menu(cx);
    }

    fn event(&mut self, event: &UiEvent, host: &dyn ParamHost) -> bool {
        self.sync_knobs(host);
        self.session.begin(host);
        let recording = RecordingHost::new(host);
        let changed = self.handle_event(event, &recording);
        let busy = self.busy();
        self.session.end(host, recording.wrote(), busy);
        self.after_session();
        changed
    }

    fn tick(&mut self, host: &dyn ParamHost) -> bool {
        let busy = self.busy();
        self.session.follow(host, busy);
        let dt = self.last_tick.elapsed().as_secs_f64().min(0.1);
        self.last_tick = Instant::now();
        let mut fresh = false;
        let history = &mut self.history;
        self.shared.history.read_since(&mut self.seen, |r| {
            if history.len() == 64 {
                history.pop_front();
            }
            history.push_back(*r);
            fresh = true;
        });
        if fresh {
            self.last_fresh = Instant::now();
        }
        if let Some(r) = self.history.back().copied() {
            let targets = [r[record::IN], r[record::IN + 1], r[record::OUT], r[record::OUT + 1]];
            for (m, t) in self.meters.iter_mut().zip(targets) {
                let fall = METER_RELEASE_DB_S * dt as f32;
                *m = if t > *m { t } else { (*m - fall).max(t) };
            }
        }
        // The band's peak, point and auto gain, eased like the spectrum (the last 20 ms).
        let b = Self::band(host);
        let recent = self.history.iter().rev().take(4);
        let peak_db = recent.clone().map(|r| r[record::BAND_IN + b]).fold(-150.0f32, f32::max);
        let target = if peak_db > -120.0 { 10f32.powf(peak_db / 20.0) } else { 0.0 };
        let k = |s: f64| (1.0 - (-dt / s).exp()) as f32;
        let before = (self.peak, self.point, self.auto);
        self.peak += (target - self.peak) * if target > self.peak { k(PEAK_RISE_S) } else { k(PEAK_FALL_S) };
        if let Some(r) = self.history.back() {
            self.point += (r[record::POINT + b] - self.point) * k(0.03);
            self.auto += (r[record::AUTO + b] - self.auto) * k(0.03);
        }
        let moved = (self.peak - before.0).abs() > 1e-4 || (self.point - before.1).abs() > 1e-4 || (self.auto - before.2).abs() > 1e-4;
        let spectrum = self.spectrum.update(&self.shared, dt);
        let flowing = self.last_fresh.elapsed().as_secs_f32() < 0.25;
        fresh || flowing || moved || spectrum || self.drag.is_some() || self.meters.iter().any(|&m| m > -149.0)
    }
}
