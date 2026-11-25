//! The Mite Sat editor (docs/design/sat-brief.md §3), in 1280×760 design units.
//!
//! The display, top to bottom: the spectrum (input dim, output lit, the harmonics the saturation
//! adds filled in the accent, the bands' regions tinted), the crossover rail shared with Shape
//! (`mite_ui::widgets::rail`), and a card per band: the selected one wide, with its model icons
//! (each a drawing of its own curve), the transfer square (the curve the DSP plays, lit where the
//! music reaches), the harmonics a probe tone measures through the band, and its dials; the others narrow, with a small square, DRIVE and MIX. One band
//! is the same screen with a thin rail and one card across. Everything drawn comes from
//! `mite_dsp::sat` and the engine's history records.

mod bars;
mod curve;
mod glyphs;
mod spectrum;
mod tape;

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Instant;

use mite_core::{ParamDisplay, ParamScale, ParamSpec};
use mite_dsp::sat::{bias_point, tone_pivot, Layout, Model, BANDS, DRIVE_MAX_DB, ENGAGE_DB, TONE_MAX_DB};
use mite_ui::kurbo::{Affine, BezPath, Line, Point, Rect, RoundedRect, Stroke};
use mite_ui::paint::{glow_stroke, icon_button, panel};
use mite_ui::peniko::Fill;
use mite_ui::session::{Session, SessionConfig};
use mite_ui::widgets::brand::{draw_mark, draw_title};
use mite_ui::widgets::footer::draw_dropdown_button;
use mite_ui::widgets::rail::{draw_badge, draw_glyph, glyph as rail_glyph, hatch, BandMenu, MenuAsk, Rail, RailHot, RailParams};
use mite_ui::widgets::{active_routes, draw_matrix_button, knob_modulation, Dropdown, DropdownEvent, Footer, FooterDial, FooterToggle, Help, Knob, KnobKind, MatrixConfig, MatrixPanel, ToggleGlyph};
use mite_ui::{write_if_changed, Align, DrawCx, Key, Modifiers, MouseButton, ParamHost, RecordingHost, TextStyle, UiEvent, View};

use crate::dsp::record;
use crate::params::*;
use crate::presets::{preset_ids, FACTORY, PLUGIN};
use crate::SatShared;

pub use spectrum::ANALYZER_SIZE;
pub use tape::DeckReading;

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
/// The spectrum and the rail: 20 Hz at x 14, 20 kHz at x 1166.
pub(crate) const PLOT_X0: f64 = 14.0;
pub(crate) const PLOT_W: f64 = 1152.0;
const PLOT_X1: f64 = PLOT_X0 + PLOT_W;
/// Meter release (tokens.json `meter.releaseDbPerSec`).
const METER_RELEASE_DB_S: f32 = 20.0;
const METER_TOP: f64 = DISPLAY_TOP + 20.0;
/// The modulation matrix: its button (footer, after the oversampling and the latency) and the
/// panel it opens over the cards.
/// The phase list's button (footer, after the matrix button).
const PHASE_BUTTON: Rect = Rect::new(380.0, FOOTER_TOP + 9.0, 540.0, FOOTER_TOP + 47.0);
const MATRIX_BUTTON: Rect = Rect::new(268.0, FOOTER_TOP + 15.0, 364.0, FOOTER_TOP + 41.0);
const MATRIX_PANEL: Rect = Rect::new(8.0, DISPLAY_TOP + 226.0, 1172.0, HELP_TOP - 4.0);
/// The tape machine: its button (footer, after the phase list) and its deck, where the matrix opens.
const TAPE_BUTTON: Rect = Rect::new(556.0, FOOTER_TOP + 15.0, 652.0, FOOTER_TOP + 41.0);
const TAPE_PANEL: Rect = MATRIX_PANEL;
const METER_BOTTOM: f64 = DISPLAY_TOP + 592.0;
/// The bands' peaks on their curves: 60 ms up, 300 ms down (as the spectrum).
const PEAK_RISE_S: f64 = 0.06;
const PEAK_FALL_S: f64 = 0.3;
/// Drag on a curve: drive per px up, bias per px across (Shift: a fifth), tone per px (Alt).
const DRIVE_PER_PX: f32 = 0.2;
const BIAS_PER_PX: f32 = 0.7;
const TONE_PER_PX: f32 = 0.06;
/// The layout grows from one band to several (base motion, 220 ms).
const GROW_MS: f64 = 220.0;
/// Cards: the selected one is this wide (multiband), the gap between cards.
const CARD_WIDE: f64 = 600.0;
const CARD_GAP: f64 = 8.0;

/// The display's vertical layout (absolute y), blended between one band and several.
#[derive(Debug, Clone, Copy)]
struct Lanes {
    spectrum: Rect,
    rail: Rect,
    /// The cards' top and bottom (the funnels run from the rail down to the top).
    cy: f64,
    cb: f64,
}

/// Spectrum top and bottom, rail top and height, cards' top and bottom (from the display top).
const SINGLE: [f64; 6] = [12.0, 222.0, 230.0, 36.0, 274.0, 608.0];
const MULTI: [f64; 6] = [12.0, 172.0, 180.0, 64.0, 270.0, 608.0];

fn lanes(grow: f64) -> Lanes {
    let v: [f64; 6] = std::array::from_fn(|i| SINGLE[i] + (MULTI[i] - SINGLE[i]) * grow);
    let y = |i: usize| DISPLAY_TOP + v[i];
    Lanes { spectrum: Rect::new(PLOT_X0, y(0), PLOT_X1, y(1)), rail: Rect::new(PLOT_X0, y(2), PLOT_X1, y(2) + v[3]), cy: y(4), cb: y(5) }
}

/// How the shared rail reads and writes Sat's crossovers and bands.
fn rail_params() -> RailParams {
    RailParams {
        cross_on: |k| String::from(cross_id(k, X_ON.id)),
        cross_freq: |k| String::from(cross_id(k, X_FREQ.id)),
        freq: X_FREQ,
        solo: |b| String::from(band_id(b, B_SOLO.id)),
        mute: |b| String::from(band_id(b, B_MUTE.id)),
        copy: SatView::copy_band,
    }
}

/// The models offered (those built so far), in icon order.
fn offered() -> Vec<Model> {
    Model::ALL.to_vec()
}

/// The wide card's dials: drive large, then bias, tone, mix and output; under them the
/// dynamics row (the threshold is automatic: no dial).
const DIALS: [(ParamSpec, &str, KnobKind); 14] = [
    (B_DRIVE, "DRIVE", KnobKind::Unipolar),
    (B_BIAS, "BIAS", KnobKind::Bipolar),
    (B_TONE, "TONE", KnobKind::Bipolar),
    (B_MIX, "MIX", KnobKind::Unipolar),
    (B_OUT, "OUTPUT", KnobKind::Bipolar),
    (B_DYN, "DYNAMICS", KnobKind::Bipolar),
    (B_DYN_ATT, "ATTACK", KnobKind::Unipolar),
    (B_DYN_REL, "RELEASE", KnobKind::Unipolar),
    // The band's output balance, under MIX and OUTPUT (user, 2026-11-10: "i need it as output
    // control... just like saturn 2").
    (B_LR, "L/R", KnobKind::Bipolar),
    (B_MS, "M/S", KnobKind::Bipolar),
    // Feedback over its frequency (2026-11-24, as Saturn 2's).
    (B_FB, "FEEDBACK", KnobKind::Unipolar),
    (B_FB_FREQ, "FREQ", KnobKind::Unipolar),
    // Extra drive on the hits and on what sustains (2026-11-24), beside DRIVE.
    (B_PUNCH, "PUNCH", KnobKind::Bipolar),
    (B_BODY, "BODY", KnobKind::Bipolar),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CardKind {
    /// One band: the card across the display.
    Single,
    /// The selected band of several.
    Wide,
    Narrow,
}

/// One band's card: where it is and what is in it.
#[derive(Debug, Clone)]
struct Card {
    band: usize,
    kind: CardKind,
    rect: Rect,
    square: Rect,
    /// The harmonics bars (single and wide cards).
    bars: Option<Rect>,
    /// The model icons (single and wide cards).
    models: Vec<(Model, Rect)>,
    power: Option<Rect>,
    stereo: Option<Rect>,
    solo: Option<Rect>,
    mute: Option<Rect>,
    remove: Option<Rect>,
    /// Dial centres and diameters: the ten of `DIALS`, or DRIVE and MIX on a narrow card.
    dials: Vec<(Point, f64)>,
}

impl Card {
    fn buttons(&self) -> [(Option<Rect>, Hot); 5] {
        let b = self.band;
        [(self.stereo, Hot::Stereo(b)), (self.power, Hot::Power(b)), (self.solo, Hot::Solo(b)), (self.mute, Hot::Mute(b)), (self.remove, Hot::Remove(b))]
    }
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
                let globals = [MIX, OUT_PAN, OUT_GAIN, ENV_ATT, ENV_REL, ENV_GAIN, TAPE_DRIVE, TAPE_BIAS, TAPE_WOW, TAPE_FLUTTER, TAPE_HISS];
                let short = id.split_once('_').map_or(id, |(_, rest)| rest);
                let pool: &[ParamSpec] = if id.starts_with('x') {
                    &[X_FREQ]
                } else if id.starts_with("lfo") {
                    &[LFO_RATE, LFO_PHASE]
                } else if id.starts_with('b') {
                    &BAND_ALL_SPECS
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
            stereo: mite_dsp::sat::StereoMode::from_index(p(&band_id(b, B_STEREO.id)).round().max(0.0) as usize),
            tone_db: p(&band_id(b, B_TONE.id)),
            mix: p(&band_id(b, B_MIX.id)) / 100.0,
            out_db: p(&band_id(b, B_OUT.id)),
            dyn_amount: p(&band_id(b, B_DYN.id)) / 100.0,
            dyn_threshold_db: p(&band_id(b, B_DYN_THR.id)),
            dyn_attack_ms: p(&band_id(b, B_DYN_ATT.id)),
            dyn_release_ms: p(&band_id(b, B_DYN_REL.id)),
            bypass: on(&band_id(b, B_BYPASS.id)),
            mute: on(&band_id(b, B_MUTE.id)),
            solo: on(&band_id(b, B_SOLO.id)),
            lr: p(&band_id(b, B_LR.id)) / 100.0,
            ms: p(&band_id(b, B_MS.id)) / 100.0,
            feedback: p(&band_id(b, B_FB.id)) / 100.0,
            feedback_hz: p(&band_id(b, B_FB_FREQ.id)),
            harmonics_db: std::array::from_fn(|k| p(&band_id(b, B_HARM[k].id))),
            punch_db: p(&band_id(b, B_PUNCH.id)),
            body_db: p(&band_id(b, B_BODY.id)),
        }),
        auto_gain: on(AUTO_GAIN.id),
    }
}

/// The tape machine as a host's parameters set it (as `TapeParams::settings` does).
pub fn tape_settings(host: &dyn ParamHost) -> mite_dsp::tape::MachineSettings {
    let p = |id: &str| host.plain(id, host.normalized(id));
    mite_dsp::tape::MachineSettings {
        on: p(TAPE_ON.id) >= 0.5,
        drive_db: p(TAPE_DRIVE.id),
        bias: p(TAPE_BIAS.id) / 100.0,
        speed: mite_dsp::tape::Speed::from_index(p(TAPE_SPEED.id).round().max(0.0) as usize),
        standard: mite_dsp::tape::Standard::from_index(p(TAPE_EQ.id).round().max(0.0) as usize),
        wow: p(TAPE_WOW.id) / 100.0,
        flutter: p(TAPE_FLUTTER.id) / 100.0,
        hiss: p(TAPE_HISS.id) / 100.0,
    }
}

/// The oversampling factor a host's parameter sets.
pub fn oversampling(host: &dyn ParamHost) -> usize {
    factor(host.plain(OVERSAMPLING.id, host.normalized(OVERSAMPLING.id)).round() as i32)
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Drag {
    /// On a band's transfer square: drive up and down, bias across (Alt: tone).
    Curve { band: usize, start: Point, drive: f32, bias: f32, tone: f32, alt: bool },
    /// On a band's bars: harmonic `k` (0 = H2) of its recipe, to where the pointer is.
    Harmonic { band: usize, k: usize },
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Hot {
    Square(usize),
    /// A partial on the band's bars (0 = H2).
    Harmonic(usize, usize),
    Model(usize, Model),
    Power(usize),
    Stereo(usize),
    Solo(usize),
    Mute(usize),
    Remove(usize),
    Card(usize),
    Rail(RailHot),
    Oversampling,
    Phase,
}

pub struct SatView {
    shared: Arc<SatShared>,
    matrix: MatrixPanel,
    matrix_open: bool,
    matrix_button_hot: bool,
    /// The tape machine's deck (over the cards, as the matrix; one open at a time).
    tape: tape::TapePanel,
    tape_open: bool,
    tape_button_hot: bool,
    session: Session,
    footer: Footer,
    /// The crossovers, band regions and the selection.
    rail: Rail<BANDS>,
    pointer: Option<Point>,
    hot: Option<Hot>,
    drag: Option<Drag>,
    /// The footer's open list: oversampling, or the phase when `phase_menu`.
    os_menu: Option<Dropdown>,
    phase_menu: bool,
    /// The stereo list and the band it is for.
    stereo_menu: Option<(usize, Dropdown)>,
    os_hit: Rect,
    menu: Option<BandMenu>,
    /// Copied band settings (`copy_band`'s order).
    clipboard: Option<[f32; 13]>,
    /// The cards' dials, for the bands and the selection they were made for.
    knobs: Vec<Knob>,
    knob_key: Option<(Vec<usize>, usize)>,
    /// The newest history records, the tap's read position, and the meters as drawn (IN L/R,
    /// OUT L/R) with their ballistics.
    history: VecDeque<[f32; record::WIDTH]>,
    seen: usize,
    meters: [f32; 4],
    last_fresh: Instant,
    last_tick: Instant,
    /// 0 = one band's layout, 1 = several (eased between).
    grow: f64,
    spectrum: spectrum::Spectrum,
    /// Tape's and Transformer's loops, as traced lately.
    loops: curve::Loops,
    /// The harmonics of the band whose card shows them.
    probe: bars::ProbeBars,
    /// Each band's peak going in (linear), operating point, auto gain and dynamics gain, eased.
    peak: [f32; BANDS],
    point: [f32; BANDS],
    auto: [f32; BANDS],
    dynamics: [f32; BANDS],
    /// Each band's automatic dynamics threshold (its own average level), linear, eased.
    threshold: [f32; BANDS],
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
        let mut rail = Rail::new(rail_params(), lanes(0.0).rail, PLOT_X0, PLOT_W);
        rail.tint = (0.18, 0.10);
        rail.labels = false;
        Self {
            session: Session::new(SessionConfig { plugin: PLUGIN, factory: FACTORY, sound_ids: preset_ids(), snapshot_ids: preset_ids() }, shared.snapshots.clone(), shared.preset.clone()).with_preset_state(crate::params::STATE_VERSION, crate::migrate::lift_preset),
            footer,
            rail,
            pointer: None,
            hot: None,
            drag: None,
            os_menu: None,
            phase_menu: false,
            stereo_menu: None,
            os_hit: Rect::ZERO,
            menu: None,
            clipboard: None,
            knobs: Vec::new(),
            knob_key: None,
            history: VecDeque::with_capacity(64),
            seen: 0,
            meters: [-150.0; 4],
            last_fresh: Instant::now(),
            last_tick: Instant::now(),
            grow: 0.0,
            spectrum: spectrum::Spectrum::new(),
            loops: curve::Loops::default(),
            probe: bars::ProbeBars::default(),
            peak: [0.0; BANDS],
            point: [0.0; BANDS],
            auto: [1.0; BANDS],
            dynamics: [1.0; BANDS],
            threshold: [0.0; BANDS],
            // Sat has no sidechain input: its `env_src` stays on the input.
            matrix: MatrixPanel::new(MatrixConfig { targets: TARGETS, sidechain: false }, MATRIX_PANEL),
            matrix_open: false,
            matrix_button_hot: false,
            tape: tape::TapePanel::new(TAPE_PANEL),
            tape_open: false,
            tape_button_hot: false,
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

    /// Lets the layout settle at once (tools render a single frame).
    pub fn settle(&mut self, host: &dyn ParamHost) {
        self.grow = if Self::layout(host).n > 0 { 1.0 } else { 0.0 };
        self.rail.area = lanes(self.grow).rail;
    }

    /// Selects a band slot (tests and tools).
    pub fn select(&mut self, band: usize) {
        self.rail.selected = band.min(BANDS - 1);
    }

    /// The band being edited.
    pub fn selected(&self) -> usize {
        self.rail.selected
    }

    /// The edited band's transfer square (tests and tools).
    pub fn square(&self, host: &dyn ParamHost) -> Rect {
        let layout = Self::layout(host);
        self.card(host, self.rail.current(&layout)).map_or(Rect::ZERO, |c| c.square)
    }

    /// Band `b`'s transfer square, if it is in use (tests and tools).
    pub fn band_square(&self, host: &dyn ParamHost, b: usize) -> Option<Rect> {
        self.card(host, b).map(|c| c.square)
    }

    /// Band `b`'s power button, if its card has one (tests and tools).
    pub fn band_power(&self, host: &dyn ParamHost, b: usize) -> Option<Rect> {
        self.card(host, b).and_then(|c| c.power)
    }

    /// Where model `m`'s icon sits on the edited band's card, if it is offered (tests and tools).
    pub fn model_icon(&self, host: &dyn ParamHost, m: Model) -> Option<Rect> {
        let layout = Self::layout(host);
        self.card(host, self.rail.current(&layout))?.models.iter().find(|(o, _)| *o == m).map(|(_, r)| *r)
    }

    /// The edited band's bars, if its card has them (tests and tools).
    pub fn bars(&self, host: &dyn ParamHost) -> Option<Rect> {
        let layout = Self::layout(host);
        self.card(host, self.rail.current(&layout)).and_then(|c| c.bars)
    }

    /// The edited band's harmonics as drawn: H2…H9, dB against the strongest partial, and whether
    /// each is hollow (tests and tools).
    pub fn harmonics(&self) -> [(f32, bool); 8] {
        self.probe.shown()
    }

    /// Band `b`'s stereo button, if its card has one (tests and tools).
    pub fn band_stereo(&self, host: &dyn ParamHost, b: usize) -> Option<Rect> {
        self.card(host, b).and_then(|c| c.stereo)
    }

    /// The open stereo list's rows, Stereo first (tests and tools).
    pub fn stereo_rows(&self) -> Vec<Rect> {
        self.stereo_menu.as_ref().map(|(_, l)| l.item_rects()).unwrap_or_default()
    }

    /// The rail's centre line (tests and tools).
    /// Where the dial for host parameter `id` sits, if a card shows it.
    pub fn dial(&self, id: &str) -> Option<Point> {
        self.knobs.iter().find(|k| k.id == id).map(|k| k.center)
    }

    pub fn rail_y(&self) -> f64 {
        self.rail.area.center().y
    }

    /// Where `f` Hz sits on the rail (tests and tools).
    pub fn rail_x(&self, f: f64) -> f64 {
        self.rail.x(f)
    }

    /// Opens the oversampling list (tools).
    pub fn open_oversampling(&mut self, host: &dyn ParamHost) {
        self.os_menu = Some(Self::os_list(host));
        self.phase_menu = false;
    }

    /// The open oversampling list's rows, 1× first (tests and tools).
    pub fn oversampling_rows(&self) -> Vec<Rect> {
        self.os_menu.as_ref().map(Dropdown::item_rects).unwrap_or_default()
    }

    /// Opens band `band`'s menu under its region (tools).
    pub fn open_menu(&mut self, band: usize, host: &dyn ParamHost) {
        let layout = Self::layout(host);
        let Some(p) = layout.position(band) else { return };
        let r = self.rail.region_rect(host, &layout, p);
        self.menu = BandMenu::open(&mut self.rail, host, band, Point::new(r.center().x - 20.0, r.y1 + 4.0), self.clipboard.is_some(), (DISPLAY_TOP, FOOTER_TOP));
    }

    /// The open band menu's rows (tests and tools).
    pub fn menu_rows(&self) -> Vec<Rect> {
        self.menu.as_ref().map(BandMenu::item_rects).unwrap_or_default()
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
        Layout::new(&std::array::from_fn::<_, { BANDS - 1 }, _>(|k| Self::on(host, &cross_id(k, X_ON.id)).then(|| Self::plain(host, &cross_id(k, X_FREQ.id)))))
    }

    fn model(host: &dyn ParamHost, b: usize) -> Model {
        Model::from_index(Self::plain(host, &band_id(b, B_MODEL.id)).round().max(0.0) as usize)
    }

    /// A band's sound settings: model, the continuous ones (`BAND_SPECS`), stereo, bypass, mute.
    fn band_values(host: &dyn ParamHost, b: usize) -> [f32; 13] {
        let mut v = [0.0; 13];
        v[0] = Self::plain(host, &band_id(b, B_MODEL.id));
        for (k, spec) in BAND_SPECS.iter().enumerate() {
            v[1 + k] = Self::plain(host, &band_id(b, spec.id));
        }
        v[10] = Self::plain(host, &band_id(b, B_STEREO.id));
        v[11] = Self::plain(host, &band_id(b, B_BYPASS.id));
        v[12] = Self::plain(host, &band_id(b, B_MUTE.id));
        v
    }

    fn write_band(host: &dyn ParamHost, b: usize, v: &[f32; 13]) {
        Self::set(host, &band_id(b, B_MODEL.id), v[0]);
        for (k, spec) in BAND_SPECS.iter().enumerate() {
            Self::set(host, &band_id(b, spec.id), v[1 + k]);
        }
        Self::set(host, &band_id(b, B_STEREO.id), v[10]);
        Self::set(host, &band_id(b, B_BYPASS.id), v[11]);
        Self::set(host, &band_id(b, B_MUTE.id), v[12]);
    }

    fn copy_band(host: &dyn ParamHost, from: usize, to: usize) {
        Self::write_band(host, to, &Self::band_values(host, from));
    }

    fn reset_band(host: &dyn ParamHost, b: usize) {
        let mut v = [0.0; 13];
        v[0] = B_MODEL.default as f32;
        for (k, spec) in BAND_SPECS.iter().enumerate() {
            v[1 + k] = spec.default;
        }
        v[10] = B_STEREO.default as f32;
        Self::write_band(host, b, &v);
    }

    /// How far band `b`'s saturation is engaged, 0…1 (`a` in sat-brief §1.1 before the mix).
    fn engagement(host: &dyn ParamHost, b: usize) -> f32 {
        if Self::on(host, &band_id(b, B_BYPASS.id)) {
            return 0.0;
        }
        let t = (Self::plain(host, &band_id(b, B_DRIVE.id)) / ENGAGE_DB).clamp(0.0, 1.0);
        t * t * (3.0 - 2.0 * t) * Self::plain(host, &band_id(b, B_MIX.id)) / 100.0
    }

    /// Band `b`'s Harmonic recipe, dB (2nd…9th).
    fn harmonics_db(host: &dyn ParamHost, b: usize) -> [f32; 8] {
        std::array::from_fn(|k| Self::plain(host, &band_id(b, B_HARM[k].id)))
    }

    /// What a drag on band `b`'s bars moves: its model and its recipe (one undo step).
    fn harmonic_ids(b: usize) -> Vec<String> {
        std::iter::once(band_id(b, B_MODEL.id)).chain(B_HARM.iter().map(|h| band_id(b, h.id))).map(String::from).collect()
    }

    fn curve(&self, host: &dyn ParamHost, b: usize) -> curve::Curve {
        let model = Self::model(host, b);
        let bias = Self::plain(host, &band_id(b, B_BIAS.id)) / 100.0;
        // The point in silence: the engine reports it (the bias's, eased).
        let point = if self.history.back().is_some() { self.point[b] } else { bias_point(bias) };
        let auto = if Self::on(host, AUTO_GAIN.id) { self.auto[b] } else { 1.0 };
        let x = settings(host).crossovers;
        let freq = tone_pivot(&Layout::new(&x), &x, b);
        let acting = Self::plain(host, &band_id(b, B_DYN.id)) != 0.0 && !Self::on(host, &band_id(b, B_BYPASS.id));
        let threshold = (acting && self.threshold[b] > 0.0).then_some(self.threshold[b]);
        let recipe = mite_dsp::sat::recipe(&Self::harmonics_db(host, b));
        curve::Curve { model, drive_db: Self::plain(host, &band_id(b, B_DRIVE.id)), point, bias, auto, dynamics: self.dynamics[b], threshold, peak: self.peak[b], freq, recipe }
    }

    /// Re-measures the shown band's harmonics when what they depend on moved, and eases the bars.
    fn update_probe(&mut self, host: &dyn ParamHost, layout: &Layout, dt: f64) -> bool {
        let b = self.rail.current(layout);
        let c = self.curve(host, b);
        let x = settings(host);
        let tone_db = x.bands[b].tone_db;
        let mix = if x.bands[b].bypass { 0.0 } else { x.bands[b].mix };
        let fs = self.shared.sample_rate.load().max(1.0);
        self.probe.update(b, &c, tone_db, mix, fs, dt)
    }

    fn phase_list(host: &dyn ParamHost) -> Dropdown {
        let current = Self::plain(host, PHASE.id).round().max(0.0) as usize;
        Dropdown::new(PHASE_BUTTON, PHASE.choices, current, (DISPLAY_TOP, FOOTER_TOP)).with_min_width(170.0)
    }

    fn os_list(host: &dyn ParamHost) -> Dropdown {
        let current = Self::plain(host, OVERSAMPLING.id).round().max(0.0) as usize;
        let anchor = Rect::new(16.0, FOOTER_TOP + 9.0, 126.0, FOOTER_TOP + 47.0);
        Dropdown::new(anchor, OVERSAMPLING.choices, current, (DISPLAY_TOP, FOOTER_TOP)).with_min_width(150.0)
    }

    // ---- geometry ---------------------------------------------------------------------------

    /// The cards, in band order from the bottom.
    fn cards(&self, layout: &Layout) -> Vec<Card> {
        let l = lanes(self.grow);
        let (y0, y1) = (l.cy, l.cb);
        let bands = layout.bands();
        let n = bands.len();
        let icon = |x: f64, y: f64, w: f64, h: f64, step: f64| -> Vec<(Model, Rect)> { offered().into_iter().enumerate().map(|(i, m)| (m, Rect::new(x + i as f64 * step, y, x + i as f64 * step + w, y + h))).collect() };
        if n == 1 {
            let rect = Rect::new(PLOT_X0, y0, PLOT_X1, y1);
            let side = rect.height() - 70.0;
            let square = Rect::new(rect.x0 + 18.0, rect.y0 + 58.0, rect.x0 + 18.0 + side, rect.y0 + 58.0 + side);
            let bars = Rect::new(square.x1 + 16.0, square.y0, square.x1 + 16.0 + 320.0, square.y1);
            let (x0, x1) = (bars.x1 + 16.0, rect.x1 - 12.0);
            // Seven columns: drive, punch over body, bias, tone, feedback over its frequency, mix,
            // output; under bias and tone the dynamics' attack and release, under drive its amount,
            // L/R and M/S under mix and output.
            let step = (x1 - x0) / 7.0;
            let (row1, row2) = (square.y0 + 70.0, square.y0 + 200.0);
            let at = |col: usize, y: f64| Point::new(x0 + step * (col as f64 + 0.5), y);
            // In DIALS order: drive, bias, tone, mix, output, dynamics, attack, release, L/R, M/S,
            // feedback, frequency, punch, body.
            let places: [(usize, f64, f64); 14] = [
                (0, row1, 76.0), (2, row1, 54.0), (3, row1, 54.0), (5, row1, 54.0), (6, row1, 54.0),
                (0, row2, 54.0), (2, row2, 48.0), (3, row2, 48.0), (5, row2, 48.0), (6, row2, 48.0),
                (4, row1, 54.0), (4, row2, 48.0), (1, row1, 54.0), (1, row2, 54.0),
            ];
            let dials: Vec<(Point, f64)> = places.iter().map(|&(c, y, d)| (at(c, y), d)).collect();
            let power = Rect::new(rect.x1 - 54.0, rect.y0 + 12.0, rect.x1 - 18.0, rect.y0 + 48.0);
            return vec![Card { band: bands[0], kind: CardKind::Single, rect, square, bars: Some(bars), models: icon(rect.x0 + 18.0, rect.y0 + 12.0, 44.0, 36.0, 50.0), power: Some(power), stereo: Some(Rect::new(power.x0 - 132.0, power.y0, power.x0 - 8.0, power.y1)), solo: None, mute: None, remove: None, dials }];
        }
        let current = self.rail.current(layout);
        let narrow = (PLOT_W - CARD_WIDE - CARD_GAP * (n - 1) as f64) / (n - 1) as f64;
        let mut x = PLOT_X0;
        bands
            .iter()
            .map(|&band| {
                let wide = band == current;
                let w = if wide { CARD_WIDE } else { narrow };
                let rect = Rect::new(x, y0, x + w, y1);
                x += w + CARD_GAP;
                if wide {
                    // Header: badge, model icons, the range, then S M power and remove on the right.
                    let button = |k: f64| Rect::new(rect.x1 - 10.0 - 32.0 * (k + 1.0) - 6.0 * k, rect.y0 + 10.0, rect.x1 - 10.0 - 32.0 * k - 6.0 * k, rect.y0 + 42.0);
                    let side = rect.height() - 62.0;
                    let square = Rect::new(rect.x0 + 10.0, rect.y0 + 52.0, rect.x0 + 10.0 + side, rect.y0 + 52.0 + side);
                    // The bars along the top of the right side, under them DRIVE large on the left
                    // and the other four in a 2×2 grid, and the dynamics' three in a row below.
                    let (ax0, ax1) = (square.x1 + 12.0, rect.x1 - 8.0);
                    let bars = Rect::new(ax0, square.y0 + 18.0, ax1, square.y0 + 74.0);
                    // Five columns: drive, then bias and tone over mix and output, then feedback
                    // over its frequency, then punch over body.
                    let col = (ax1 - ax0) / 5.0;
                    let (top, step) = (bars.y1 + 28.0, 66.0);
                    let cy = top + step / 2.0;
                    let mut dials = vec![(Point::new(ax0 + col * 0.5, cy - 6.0), 72.0)];
                    for k in 0..4 {
                        dials.push((Point::new(ax0 + col * (1.5 + (k % 2) as f64), top + step * (k / 2) as f64), 44.0));
                    }
                    // The dynamics' three, then L/R and M/S, in one row of five.
                    let row = (top + 2.0 * step + 4.0).min(rect.y1 - 47.0);
                    let col5 = (ax1 - ax0) / 5.0;
                    for k in 0..5 {
                        dials.push((Point::new(ax0 + col5 * (k as f64 + 0.5), row), 40.0));
                    }
                    for k in 0..2 {
                        dials.push((Point::new(ax0 + col * 3.5, top + step * k as f64), 44.0));
                    }
                    for k in 0..2 {
                        dials.push((Point::new(ax0 + col * 4.5, top + step * k as f64), 44.0));
                    }
                    Card {
                        band,
                        kind: CardKind::Wide,
                        rect,
                        square,
                        bars: Some(bars),
                        // Nine icons and the stereo pill share the row (2026-11-24: Harmonic added).
                        models: icon(rect.x0 + 40.0, rect.y0 + 10.0, 32.0, 32.0, 36.0),
                        power: Some(button(1.0)),
                        // The stereo pill fills what the model icons and the buttons leave.
                        stereo: Some(Rect::new(rect.x0 + 44.0 + 36.0 * offered().len() as f64, rect.y0 + 10.0, button(3.0).x0 - 8.0, rect.y0 + 42.0)),
                        solo: Some(button(3.0)),
                        mute: Some(button(2.0)),
                        remove: Some(button(0.0)),
                        dials,
                    }
                } else {
                    let button = |k: f64| Rect::new(rect.x1 - 8.0 - 24.0 * (k + 1.0) - 3.0 * k, rect.y0 + 10.0, rect.x1 - 8.0 - 24.0 * k - 3.0 * k, rect.y0 + 34.0);
                    let side = (w - 16.0).min(rect.height() - 128.0);
                    let square = Rect::new(rect.x0 + 8.0, rect.y0 + 44.0, rect.x0 + 8.0 + side, rect.y0 + 44.0 + side);
                    // Two small dials, side by side when there is room.
                    let top = square.y1 + 30.0;
                    let dials = if w >= 216.0 { vec![(Point::new(rect.x0 + 27.0, top), 34.0), (Point::new(rect.x0 + 27.0 + 104.0, top), 34.0)] } else { vec![(Point::new(rect.x0 + 27.0, top), 34.0), (Point::new(rect.x0 + 27.0, top + 44.0), 34.0)] };
                    // The stereo badge in the square's corner: shows the mode, a click picks one.
                    let stereo = Some(Rect::from_center_size((square.x0 + 13.0, square.y0 + 11.0), (24.0, 20.0)));
                    Card { band, kind: CardKind::Narrow, rect, square, bars: None, models: Vec::new(), power: None, stereo, solo: Some(button(1.0)), mute: Some(button(0.0)), remove: None, dials }
                }
            })
            .collect()
    }

    fn card(&self, host: &dyn ParamHost, b: usize) -> Option<Card> {
        self.cards(&Self::layout(host)).into_iter().find(|c| c.band == b)
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

    /// The spectrum, with each band's region tinted (several bands) and the crossovers through it.
    fn draw_spectrum(&self, cx: &mut DrawCx<'_>, layout: &Layout, area: Rect) {
        let mut tints = Vec::new();
        if layout.n > 0 {
            let current = self.rail.current(layout);
            let soloed = self.rail.soloed(cx.host, layout);
            for (p, &b) in layout.bands().iter().enumerate() {
                if self.rail.is_muted(cx.host, b) {
                    continue;
                }
                let r = self.rail.region_rect(cx.host, layout, p);
                let breath = Self::engagement(cx.host, b) * self.peak[b].min(1.0);
                let mut op = if b == current { 0.10 } else { 0.05 } + 0.06 * breath;
                if soloed.is_some_and(|s| s != b) {
                    op *= 0.35;
                }
                tints.push((Rect::new(r.x0, area.y0, r.x1, area.y1), op));
            }
        }
        self.spectrum.draw(cx, area, &tints);
        if layout.n > 0 {
            let theme = cx.theme;
            let x = self.rail.crossovers(cx.host);
            for p in 0..layout.n {
                let px = self.rail.x(f64::from(x[layout.crossovers[p]].unwrap_or(20.0)));
                cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, theme.accent.multiply_alpha(0.35), None, &Line::new((px, area.y0), (px, area.y1)));
            }
        }
    }

    fn draw_cards(&self, cx: &mut DrawCx<'_>, layout: &Layout, cards: &[Card]) {
        let theme = cx.theme;
        let soloed = self.rail.soloed(cx.host, layout);
        let merging = self.rail.merging_band(layout);
        let hot = |r: Rect| self.pointer.is_some_and(|p| r.contains(p));
        for (p, card) in cards.iter().enumerate() {
            let b = card.band;
            let solo = self.rail.is_solo(cx.host, b);
            let muted = self.rail.is_muted(cx.host, b);
            let bypassed = Self::on(cx.host, &band_id(b, B_BYPASS.id));
            let alpha: f32 = if soloed.is_some_and(|s| s != b) { 0.35 } else if muted { 0.55 } else if merging == Some(b) { 0.6 } else { 1.0 };
            let frame = RoundedRect::from_rect(card.rect, 12.0);
            cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.window.multiply_alpha(alpha), None, &frame);
            cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.control.multiply_alpha(0.55 * alpha), None, &frame);
            if merging == Some(b) {
                cx.scene.stroke(&Stroke::new(1.5).with_dashes(0.0, [4.0, 3.0]), Affine::IDENTITY, theme.accent, None, &frame);
            } else if solo {
                glow_stroke(cx, &frame, 2.0, theme.needle.multiply_alpha(alpha), theme.needle, 6.0);
            } else if card.kind == CardKind::Wide {
                glow_stroke(cx, &frame, 1.0, theme.accent.multiply_alpha(alpha), theme.accent.multiply_alpha(0.45 * alpha), 5.0);
            } else {
                cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, theme.well_border, None, &frame);
            }
            if muted {
                hatch(cx, card.rect, Some(12.0), 0.22);
            }
            if card.kind != CardKind::Single {
                let r = if card.kind == CardKind::Wide { 11.0 } else { 9.0 };
                let y = if card.kind == CardKind::Wide { card.rect.y0 + 26.0 } else { card.rect.y0 + 22.0 };
                draw_badge(cx, Point::new(card.rect.x0 + 9.0 + r + 2.0, y), r, p + 1, card.kind == CardKind::Wide, solo, alpha);
            }
            let current = Self::model(cx.host, b);
            for &(m, r) in &card.models {
                let ink = icon_button(cx.scene, theme, r, theme.panel_control, hot(r), m == current, theme.accent);
                glyphs::draw_model(cx.scene, m, r.center(), r.height() * 0.72, ink, 1.6);
            }
            if card.kind == CardKind::Wide {
                // The band's range, over the dials.
                let text = self.rail.range_text(cx.host, layout, b);
                cx.text.draw(cx.scene, &text, card.rect.x1 - 14.0, card.square.y0 + 2.0, TextStyle { tabular: true, ..TextStyle::BODY.sized(11.0).weighted(600.0) }, theme.text_mid, Align::Right);
            }
            if let Some(r) = card.power {
                let ink = icon_button(cx.scene, theme, r, theme.panel_control, hot(r), !bypassed, theme.accent);
                glyphs::draw(cx.scene, glyphs::POWER, r.center(), r.height() * 0.5, ink, 1.8);
            }
            let mode = Self::plain(cx.host, &band_id(b, B_STEREO.id)).round().max(0.0) as usize;
            if let Some(r) = card.stereo.filter(|_| card.kind != CardKind::Narrow) {
                // A pill with the glyph and the mode's name: where the band's stereo is picked.
                let open = matches!(self.stereo_menu, Some((sb, _)) if sb == b);
                let lit = mode != 0;
                let border = if open || hot(r) || lit { theme.accent } else { theme.panel_control_border };
                mite_ui::paint::raised(cx.scene, RoundedRect::from_rect(r, 6.0), theme.panel_control, border, theme.shadows);
                let ink = if lit { theme.accent } else { theme.text_hi };
                const NAMES: [&str; 6] = ["STEREO", "MID", "SIDE", "M/S", "LEFT", "RIGHT"];
                if r.width() >= 100.0 {
                    glyphs::draw_stereo(cx.scene, mode, Point::new(r.x0 + 16.0, r.center().y), 20.0, ink);
                    cx.text.draw(cx.scene, NAMES[mode.min(5)], r.x0 + 30.0, r.center().y - 7.0, TextStyle::LABEL.sized(11.0), ink, Align::Left);
                } else {
                    // A tight header (the wide card): the name alone, centred.
                    cx.text.draw(cx.scene, NAMES[mode.min(5)], r.center().x, r.center().y - 7.0, TextStyle::LABEL.sized(10.5), ink, Align::Center);
                }
            }
            if let Some(r) = card.solo {
                let ink = icon_button(cx.scene, theme, r, theme.panel_control, hot(r), solo, theme.needle);
                let size = if r.height() > 28.0 { 13.0 } else { 11.0 };
                cx.text.draw(cx.scene, "S", r.center().x, r.center().y - size * 0.62, TextStyle::BODY.sized(size as f32).weighted(800.0), ink, Align::Center);
            }
            if let Some(r) = card.mute {
                let ink = icon_button(cx.scene, theme, r, theme.panel_control, hot(r), muted, theme.text_hi);
                draw_glyph(cx.scene, rail_glyph::MUTE, r.center(), r.height() * 0.56, ink, 1.7);
            }
            if let Some(r) = card.remove {
                // Sat draws no danger role: the remove button is a cross in the text colour.
                let ink = icon_button(cx.scene, theme, r, theme.panel_control, hot(r), false, theme.text_hi);
                draw_glyph(cx.scene, rail_glyph::CROSS, r.center(), r.height() * 0.56, ink, 1.8);
            }
            let ghost = match self.hot {
                Some(Hot::Model(hb, m)) if hb == b && m != current => Some(m),
                _ => None,
            };
            let lit = if bypassed { 0.45 } else { 1.0 } * alpha;
            curve::draw(cx, card.square, &self.curve(cx.host, b), ghost, card.kind != CardKind::Narrow, lit, &self.loops);
            if let Some(r) = card.bars {
                let focus = match self.hot {
                    Some(Hot::Harmonic(hb, i)) if hb == b => Some(i),
                    _ => None,
                };
                let focus = match self.drag {
                    Some(Drag::Harmonic { band, k }) if band == b => Some(k),
                    _ => focus,
                };
                let targets = (Self::model(cx.host, b) == Model::Harmonic).then(|| Self::harmonics_db(cx.host, b));
                self.probe.draw(cx, r, focus, lit, targets);
            }
            if card.kind == CardKind::Narrow {
                // Lit when the band is not plain stereo; dim (but there to click) when it is.
                let r = card.stereo.unwrap_or_default();
                let ink = if mode != 0 { theme.accent.multiply_alpha(alpha) } else if hot(r) { theme.text_hi } else { theme.text_lo.multiply_alpha(0.6 * alpha) };
                glyphs::draw_stereo(cx.scene, mode, r.center(), 16.0, ink);
            }
        }
        for k in &self.knobs {
            k.draw(cx);
            if let Some((lo, hi, live)) = knob_modulation(cx.host, &TARGETS, &self.shared.matrix, &k.id) {
                k.draw_modulation(cx, (lo, hi), live);
            }
            if !k.value_inside {
                // Small dials: the name and value beside them.
                let v = Self::plain(cx.host, &k.id);
                let x = k.center.x + k.diameter / 2.0 + 7.0;
                cx.text.draw(cx.scene, k.label, x, k.center.y - 14.0, TextStyle::LABEL.sized(9.0), theme.text_lo, Align::Left);
                cx.text.draw(cx.scene, &k.spec.format(v), x, k.center.y - 1.0, TextStyle { tabular: true, ..TextStyle::BODY.weighted(600.0) }, theme.text_hi, Align::Left);
            }
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
        let text = OVERSAMPLING.choices[(Self::plain(cx.host, OVERSAMPLING.id).round().max(0.0) as usize).min(OVERSAMPLING.choices.len() - 1)];
        draw_dropdown_button(cx, r, text, self.os_menu.is_some(), hot, |scene, at| glyphs::draw(scene, glyphs::OVERSAMPLE, at + (8.0, 6.0), 22.0, theme.text_mid, 1.6));
        self.os_hit = r;
        let sample_rate = f64::from(self.shared.sample_rate.load()).max(1.0);
        self.footer.draw_latency(cx, 142.0, f64::from(self.shared.latency.load()) / sample_rate * 1000.0);
        self.footer.draw_controls(cx);
        draw_matrix_button(cx, MATRIX_BUTTON, self.matrix_open, self.matrix_button_hot, active_routes(cx.host));
        tape::draw_tape_button(cx, TAPE_BUTTON, self.tape_open, self.tape_button_hot, Self::on(cx.host, TAPE_ON.id));
        let phase = (Self::plain(cx.host, PHASE.id).round().max(0.0) as usize).min(1);
        let hot = self.pointer.is_some_and(|p| PHASE_BUTTON.contains(p));
        draw_dropdown_button(cx, PHASE_BUTTON, PHASE.choices[phase], self.os_menu.is_some() && self.phase_menu, hot, |_, _| {});
        if let Some(list) = self.os_menu.as_ref().filter(|_| self.phase_menu) {
            list.draw(cx, theme.accent, |_, _, _, _| {});
        } else if let Some(list) = &self.os_menu {
            list.draw(cx, theme.accent, |scene, i, at, ink| {
                // Each factor's icon: as many waves as it oversamples (16×: twelve, what reads
                // in 20 px).
                let n = [1usize, 2, 4, 8, 12][i.min(4)];
                let mut p = BezPath::new();
                for k in 0..=64 {
                    let x = at.x + 1.0 + 20.0 * k as f64 / 64.0;
                    let y = at.y + 12.0 - 5.0 * (std::f64::consts::TAU * n as f64 * k as f64 / 64.0 / 2.0).sin();
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

    fn draw_states(&self, cx: &mut DrawCx<'_>, spectrum: Rect) {
        let theme = cx.theme;
        if Self::on(cx.host, DELTA.id) {
            let r = RoundedRect::from_rect(Rect::new(spectrum.x0 + 10.0, spectrum.y0 + 10.0, spectrum.x0 + 46.0, spectrum.y0 + 46.0), 10.0);
            cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.accent.multiply_alpha(0.18), None, &r);
            glow_stroke(cx, &r, 1.5, theme.accent, theme.accent, 9.0);
            mite_ui::widgets::footer::draw_toggle_glyph(cx.scene, ToggleGlyph::Delta, r.center(), theme.accent);
        }
    }

    // ---- help -------------------------------------------------------------------------------

    pub fn matrix_button(&self) -> Rect {
        MATRIX_BUTTON
    }

    pub fn matrix_panel(&self) -> Option<&MatrixPanel> {
        self.matrix_open.then_some(&self.matrix)
    }

    fn help_text(&self, host: &dyn ParamHost) -> Help {
        if self.matrix_open && (self.pointer.is_some_and(|p| MATRIX_PANEL.contains(p)) || self.matrix.busy()) {
            return self.matrix.help(host).unwrap_or_else(|| Help::default().text("The modulation matrix: two LFOs and an envelope, each slot moving one control by a share of its travel (its dial shows the reach and where it is now)."));
        }
        if self.tape_open {
            if let Some(h) = self.tape.help(host) {
                return h;
            }
        }
        if self.tape_button_hot {
            return Help::default().text("Tape — open the deck: one tape machine after the bands (speed, EQ, bias, wow and flutter, hiss)");
        }
        if self.matrix_button_hot {
            return Help::default().text("Modulation — open the matrix: two LFOs and an envelope follower, routed to any band's controls, the crossovers, mix and output");
        }
        let layout = Self::layout(host);
        let number = |b: usize| layout.position(b).map_or(1, |p| p + 1);
        let values = |b: usize| (f64::from(Self::plain(host, &band_id(b, B_DRIVE.id))), f64::from(Self::plain(host, &band_id(b, B_BIAS.id))), f64::from(Self::plain(host, &band_id(b, B_TONE.id))));
        if let Some(Drag::Curve { band, alt, .. }) = self.drag {
            let (drive, bias, tone) = values(band);
            if alt {
                return Help::default().text("Tone ").slot(fmt_signed(tone, " dB"), "+12.0 dB").text(" — tilts what the saturation gives, around the band's centre.");
            }
            return Help::default().text("Drive ").slot(format!("{drive:.1} dB"), "36.0 dB").text("  bias ").slot(format!("{bias:+.0} %"), "+100 %").text(" — up drives harder, across moves the operating point. Shift is fine.");
        }
        if let Some(h) = self.rail.help(host) {
            return h;
        }
        if let Some(list) = &self.os_menu {
            let fs = f64::from(self.shared.sample_rate.load()).max(1.0);
            let hovered = self.pointer.and_then(|p| list.item_rects().iter().position(|r| r.contains(p)));
            if let (Some(i), true) = (hovered, self.phase_menu) {
                let ms = crate::dsp::linear_phase_latency(fs as f32) as f64 / fs * 1000.0;
                return Help::default().text(format!("{} — ", PHASE.choices[i])).text(if i == 0 { "the lowest latency: LR4 crossovers and minimum-phase oversampling; the phase turns around the crossovers and near the top".to_owned() } else { format!("no phase shift anywhere: the bands add back to the input exactly and neutral settings null; adds {ms:.0} ms of latency") });
            }
            if let Some(i) = hovered {
                let f = mite_dsp::sat::FACTORS[i.min(mite_dsp::sat::FACTORS.len() - 1)];
                let linear = Self::plain(host, PHASE.id).round() >= 1.0;
                let ms = mite_dsp::oversample::Oversampler::latency_of(f, crate::dsp::filter_for(linear)) as f64 / fs * 1000.0;
                return Help::default().text(format!("{} — ", OVERSAMPLING.choices[i])).text(if f == 1 { "no oversampling: no latency, the most aliasing".to_owned() } else { format!("the curves run {f} times faster, so their harmonics fold back far less; adds {ms:.2} ms of latency") });
            }
        }
        match self.hot {
            Some(Hot::Square(b)) => {
                let (drive, bias, _) = values(b);
                return Help::default().text("Drive ").slot(format!("{drive:.1} dB"), "36.0 dB").text("  bias ").slot(format!("{bias:+.0} %"), "+100 %").text(" — drag up for more drive, across for bias; Alt-drag: tone; wheel: drive; double-click resets.");
            }
            Some(Hot::Harmonic(_, i)) => return self.probe.help(i),
            Some(Hot::Stereo(b)) => {
                let i = Self::plain(host, &band_id(b, B_STEREO.id)).round().max(0.0) as usize;
                return Help::default().text(format!("{} — {}", B_STEREO.choices[i.min(5)], stereo_words(i)));
            }
            Some(Hot::Model(_, m)) => return Help::default().text(format!("{} — {}", m.name(), model_words(m))),
            Some(Hot::Power(b)) => return Help::default().text(if Self::on(host, &band_id(b, B_BYPASS.id)) { "The band passes clean — click to saturate it again." } else { "Band on — click to let it pass clean (still heard)." }),
            Some(Hot::Solo(_)) => return Help::default().text("Solo — hear only this band (S)"),
            Some(Hot::Mute(_)) => return Help::default().text("Mute — take this band out of the sum (M)"),
            Some(Hot::Remove(b)) => return Help::default().text(format!("Remove band {} — its region joins the band below (Delete)", number(b))),
            Some(Hot::Oversampling) => return Help::default().text("Oversampling — how many times faster the curves run, so their harmonics do not fold back into the audible band."),
            Some(Hot::Phase) => return Help::default().text("Phase — how the bands are split: Minimum Phase (no latency) or Linear Phase (no phase shift between bands, more latency)."),
            Some(Hot::Rail(RailHot::Region(b))) | Some(Hot::Card(b)) if layout.n > 0 => {
                let (drive, _, _) = values(b);
                return Help::default()
                    .slot(format!("Band {}", number(b)), "Band 6")
                    .text(" · ")
                    .slot(self.rail.range_text(host, &layout, b), "20.0 kHz – 20.0 kHz")
                    .text(format!(" · {} · drive ", Self::model(host, b).name()))
                    .slot(format!("{drive:.1} dB"), "36.0 dB")
                    .text(" — click to edit it; double-click the strip to split; right-click for more");
            }
            _ => {}
        }
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
        let b = self.rail.current(&layout);
        if layout.n > 0 {
            return Help::default()
                .slot(format!("Band {}", number(b)), "Band 6")
                .text(" · ")
                .slot(self.rail.range_text(host, &layout, b), "20.0 kHz – 20.0 kHz")
                .text(" — drag a crossover sideways; double-click the strip to split; click a band to edit it");
        }
        if values(b).0 == 0.0 {
            Help::default().text("Clean: drag the curve up, or turn DRIVE, to saturate. Pull a crossover in from either end of the strip to saturate bands apart.")
        } else {
            Help::default().text("Lit: where the music reaches on the curve. The bars: the harmonics it makes. Crimson in the spectrum: what it adds.")
        }
    }

    // ---- events -----------------------------------------------------------------------------

    /// Makes the cards' dials for the bands in use and the selection, and moves them where the
    /// cards are now.
    fn sync_knobs(&mut self, host: &dyn ParamHost) {
        let layout = Self::layout(host);
        let cards = self.cards(&layout);
        let key = (layout.bands().to_vec(), self.rail.current(&layout));
        if self.knob_key.as_ref() != Some(&key) && !self.knobs.iter().any(Knob::is_dragging) {
            self.knobs.clear();
            for card in &cards {
                let b = card.band;
                if card.kind == CardKind::Narrow {
                    for (k, spec) in [B_DRIVE, B_MIX].into_iter().enumerate() {
                        let (c, d) = card.dials[k];
                        let mut knob = Knob::new(spec, DIALS[k * 3].1, KnobKind::Unipolar, c, d).with_id(band_id(b, spec.id));
                        if k == 0 {
                            knob = knob.with_curve(DRIVE_DIAL);
                        }
                        knob.value_inside = false;
                        self.knobs.push(knob);
                    }
                } else {
                    for (k, (spec, label, kind)) in DIALS.iter().enumerate() {
                        let (c, d) = card.dials[k];
                        let knob = Knob::new(*spec, label, *kind, c, d).with_id(band_id(b, spec.id));
                        self.knobs.push(if k == 0 { knob.with_curve(DRIVE_DIAL) } else { knob });
                    }
                }
            }
            self.knob_key = Some(key);
        }
        // Follow the cards as the layout eases.
        let mut centres = cards.iter().flat_map(|c| c.dials.iter().copied());
        for k in &mut self.knobs {
            if let Some((c, _)) = centres.next() {
                k.center = c;
            }
        }
    }

    fn busy(&self) -> bool {
        self.drag.is_some() || self.rail.busy() || self.knobs.iter().any(Knob::is_dragging) || self.footer.busy() || (self.matrix_open && self.matrix.busy()) || (self.tape_open && self.tape.busy())
    }

    fn hot_at(&self, host: &dyn ParamHost, p: Point) -> Option<Hot> {
        if let Some(h) = self.rail.hit(host, p) {
            return Some(Hot::Rail(h));
        }
        for card in self.cards(&Self::layout(host)) {
            if !card.rect.contains(p) {
                continue;
            }
            // The buttons first: a narrow card's stereo badge sits in its square's corner.
            if let Some((_, h)) = card.buttons().into_iter().find(|(r, _)| r.is_some_and(|r| r.contains(p))) {
                return Some(h);
            }
            if card.square.contains(p) {
                return Some(Hot::Square(card.band));
            }
            if let Some(i) = card.bars.and_then(|r| bars::hit(r, p)) {
                return Some(Hot::Harmonic(card.band, i));
            }
            if let Some(&(m, _)) = card.models.iter().find(|(_, r)| r.contains(p)) {
                return Some(Hot::Model(card.band, m));
            }
            return Some(Hot::Card(card.band));
        }
        if self.os_hit.contains(p) {
            return Some(Hot::Oversampling);
        }
        if PHASE_BUTTON.contains(p) {
            return Some(Hot::Phase);
        }
        None
    }

    fn key_event(&mut self, key: Key, mods: Modifiers, host: &dyn ParamHost) -> bool {
        if let Some(used) = self.session.key(key, mods, host) {
            return used;
        }
        let layout = Self::layout(host);
        let b = self.rail.current(&layout);
        match key {
            Key::Escape => {
                if self.os_menu.take().is_some() || self.stereo_menu.take().is_some() || self.menu.take().is_some() || self.rail.cancel() {
                    return true;
                }
                self.rail.clear_solo(host)
            }
            Key::Delete | Key::Backspace if layout.n > 0 => {
                self.rail.remove_band(host, b);
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
            Key::Character(c) if !(mods.ctrl || mods.meta) => match c.to_ascii_lowercase() {
                's' => {
                    self.rail.toggle_solo(host, b);
                    true
                }
                'm' => {
                    self.rail.toggle_mute(host, b);
                    true
                }
                _ => false,
            },
            _ => false,
        }
    }

    fn menu_event(&mut self, event: &UiEvent, host: &dyn ParamHost) -> bool {
        let Some(menu) = &mut self.menu else { return false };
        if let UiEvent::PointerMove { pos, .. } = *event {
            self.pointer = Some(pos);
        }
        let Ok(ask) = menu.event(event, &mut self.rail, host) else { return true };
        self.menu = None;
        match ask {
            Some(MenuAsk::Copy(b)) => self.clipboard = Some(Self::band_values(host, b)),
            Some(MenuAsk::Paste(b)) => {
                if let Some(v) = self.clipboard {
                    Self::write_band(host, b, &v);
                }
            }
            Some(MenuAsk::Reset(b)) => Self::reset_band(host, b),
            None => {}
        }
        true
    }

    fn curve_ids(band: usize, alt: bool) -> Vec<String> {
        if alt { vec![String::from(band_id(band, B_TONE.id))] } else { vec![String::from(band_id(band, B_DRIVE.id)), String::from(band_id(band, B_BIAS.id))] }
    }

    /// Harmonic `k` of the dragged band's recipe to the pointer's height in its bars.
    fn drag_harmonic(&mut self, host: &dyn ParamHost, pos: Point) {
        let Some(Drag::Harmonic { band, k }) = self.drag else { return };
        let layout = Self::layout(host);
        let Some(r) = self.cards(&layout).into_iter().find(|c| c.band == band).and_then(|c| c.bars) else { return };
        Self::set_raw(host, &band_id(band, B_HARM[k].id), bars::db_at(r, pos.y));
    }

    fn press(&mut self, host: &dyn ParamHost, pos: Point, mods: Modifiers) -> bool {
        match self.hot_at(host, pos) {
            Some(Hot::Rail(_)) => self.rail.press(host, pos),
            Some(Hot::Square(b)) => {
                self.rail.selected = b;
                let alt = mods.alt;
                for id in Self::curve_ids(b, alt) {
                    host.begin_gesture(&id);
                }
                let p = |s: &ParamSpec| Self::plain(host, &band_id(b, s.id));
                self.drag = Some(Drag::Curve { band: b, start: pos, drive: p(&B_DRIVE), bias: p(&B_BIAS), tone: p(&B_TONE), alt });
                true
            }
            Some(Hot::Model(b, m)) => {
                Self::set(host, &band_id(b, B_MODEL.id), m.index() as f32);
                true
            }
            Some(Hot::Harmonic(b, k)) => {
                self.rail.selected = b;
                // Drawing: on another model the band becomes Harmonic, its recipe what the bars
                // show it making now, so the sound carries on from where it was.
                for id in Self::harmonic_ids(b) {
                    host.begin_gesture(&id);
                }
                if Self::model(host, b) != Model::Harmonic {
                    Self::set_raw(host, &band_id(b, B_MODEL.id), Model::Harmonic.index() as f32);
                    for (i, (db, _)) in self.probe.shown().iter().enumerate() {
                        Self::set_raw(host, &band_id(b, B_HARM[i].id), db.clamp(mite_dsp::sat::HARMONIC_FLOOR_DB, 0.0));
                    }
                }
                self.drag = Some(Drag::Harmonic { band: b, k });
                self.drag_harmonic(host, pos);
                true
            }
            Some(Hot::Power(b)) => {
                let on = Self::on(host, &band_id(b, B_BYPASS.id));
                Self::set(host, &band_id(b, B_BYPASS.id), if on { 0.0 } else { 1.0 });
                true
            }
            Some(Hot::Solo(b)) => {
                self.rail.toggle_solo(host, b);
                true
            }
            Some(Hot::Mute(b)) => {
                self.rail.toggle_mute(host, b);
                true
            }
            Some(Hot::Remove(b)) => {
                self.rail.remove_band(host, b);
                true
            }
            Some(Hot::Card(b)) => {
                self.rail.selected = b;
                true
            }
            Some(Hot::Oversampling) => {
                self.os_menu = Some(Self::os_list(host));
                self.phase_menu = false;
                true
            }
            Some(Hot::Phase) => {
                self.os_menu = Some(Self::phase_list(host));
                self.phase_menu = true;
                true
            }
            Some(Hot::Stereo(b)) => {
                self.rail.selected = b;
                if let Some(r) = self.card(host, b).and_then(|c| c.stereo) {
                    let current = Self::plain(host, &band_id(b, B_STEREO.id)).round().max(0.0) as usize;
                    self.stereo_menu = Some((b, Dropdown::new(r, B_STEREO.choices, current, (DISPLAY_TOP, FOOTER_TOP)).with_min_width(150.0)));
                }
                true
            }
            None => false,
        }
    }

    fn handle_event(&mut self, event: &UiEvent, host: &dyn ParamHost) -> bool {
        if let UiEvent::KeyDown { key, mods } = *event {
            if self.matrix_open && self.matrix.event(event, host).is_some() {
                return true;
            }
            if self.matrix_open && key == Key::Escape {
                self.matrix_open = false;
                return true;
            }
            if self.tape_open && key == Key::Escape {
                self.tape_open = false;
                return true;
            }
            return self.key_event(key, mods, host);
        }
        if self.matrix_open && self.os_menu.is_none() && self.stereo_menu.is_none() && self.menu.is_none() {
            if let Some(handled) = self.matrix.event(event, host) {
                if !handled {
                    self.matrix_open = false;
                }
                if let UiEvent::PointerMove { pos, .. } = *event {
                    self.pointer = Some(pos);
                    self.hot = None;
                }
                return true;
            }
        }
        if self.tape_open && self.os_menu.is_none() && self.stereo_menu.is_none() && self.menu.is_none() {
            if let Some(handled) = self.tape.event(event, host) {
                if !handled {
                    self.tape_open = false;
                }
                if let UiEvent::PointerMove { pos, .. } = *event {
                    self.pointer = Some(pos);
                    self.hot = None;
                }
                return true;
            }
        }
        match *event {
            UiEvent::PointerMove { pos, .. } => {
                self.matrix_button_hot = MATRIX_BUTTON.contains(pos);
                self.tape_button_hot = TAPE_BUTTON.contains(pos);
            }
            UiEvent::PointerDown { pos, button: MouseButton::Left, .. } if MATRIX_BUTTON.contains(pos) && self.os_menu.is_none() && self.stereo_menu.is_none() => {
                self.matrix_open = !self.matrix_open;
                self.tape_open = false;
                return true;
            }
            UiEvent::PointerDown { pos, button: MouseButton::Left, .. } if TAPE_BUTTON.contains(pos) && self.os_menu.is_none() && self.stereo_menu.is_none() => {
                self.tape_open = !self.tape_open;
                self.matrix_open = false;
                return true;
            }
            _ => {}
        }
        if let Some(list) = &mut self.os_menu {
            if let UiEvent::PointerMove { pos, .. } = *event {
                self.pointer = Some(pos);
            }
            match list.event(event) {
                DropdownEvent::Open => {}
                DropdownEvent::Picked(i) => {
                    self.os_menu = None;
                    Self::set(host, if self.phase_menu { PHASE.id } else { OVERSAMPLING.id }, i as f32);
                }
                DropdownEvent::Closed => self.os_menu = None,
            }
            return true;
        }
        if let Some((b, list)) = &mut self.stereo_menu {
            if let UiEvent::PointerMove { pos, .. } = *event {
                self.pointer = Some(pos);
            }
            let b = *b;
            match list.event(event) {
                DropdownEvent::Open => {}
                DropdownEvent::Picked(i) => {
                    self.stereo_menu = None;
                    Self::set(host, &band_id(b, B_STEREO.id), i as f32);
                }
                DropdownEvent::Closed => self.stereo_menu = None,
            }
            return true;
        }
        if self.menu.is_some() && !self.session.menu_open() {
            return self.menu_event(event, host);
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
        match *event {
            UiEvent::PointerMove { pos, mods } => {
                self.pointer = Some(pos);
                if self.rail.drag_to(host, pos, mods) {
                    return true;
                }
                if let Some(Drag::Harmonic { .. }) = self.drag {
                    self.drag_harmonic(host, pos);
                    return true;
                }
                if let Some(Drag::Curve { band, start, drive, bias, tone, alt }) = self.drag {
                    let k = if mods.fine() { 0.2 } else { 1.0 };
                    let (dx, dy) = ((pos.x - start.x) as f32, (start.y - pos.y) as f32);
                    if alt {
                        Self::set_raw(host, &band_id(band, B_TONE.id), (tone + dy * TONE_PER_PX * k).clamp(-TONE_MAX_DB, TONE_MAX_DB));
                    } else {
                        Self::set_raw(host, &band_id(band, B_DRIVE.id), (drive + dy * DRIVE_PER_PX * k).clamp(0.0, DRIVE_MAX_DB));
                        Self::set_raw(host, &band_id(band, B_BIAS.id), (bias + dx * BIAS_PER_PX * k).clamp(-100.0, 100.0));
                    }
                    return true;
                }
                self.hot = self.hot_at(host, pos);
                self.rail.hover(host, Some(pos));
                true
            }
            UiEvent::PointerLeft => {
                self.pointer = None;
                self.hot = None;
                self.rail.hover(host, None);
                true
            }
            UiEvent::PointerDown { pos, button: MouseButton::Left, mods } => self.press(host, pos, mods),
            UiEvent::PointerDown { pos, button: MouseButton::Right, .. } => match self.hot_at(host, pos) {
                Some(Hot::Rail(RailHot::Region(b)) | Hot::Card(b) | Hot::Square(b) | Hot::Harmonic(b, _)) => {
                    self.menu = BandMenu::open(&mut self.rail, host, b, pos, self.clipboard.is_some(), (DISPLAY_TOP, FOOTER_TOP));
                    true
                }
                _ => false,
            },
            UiEvent::PointerUp { button: MouseButton::Left, .. } => {
                if self.rail.release(host) {
                    return true;
                }
                match self.drag.take() {
                    Some(Drag::Curve { band, alt, .. }) => {
                        for id in Self::curve_ids(band, alt) {
                            host.end_gesture(&id);
                        }
                        true
                    }
                    Some(Drag::Harmonic { band, .. }) => {
                        for id in Self::harmonic_ids(band) {
                            host.end_gesture(&id);
                        }
                        true
                    }
                    None => false,
                }
            }
            UiEvent::DoubleClick { pos, .. } => match self.hot_at(host, pos) {
                Some(Hot::Rail(_)) => self.rail.double_click(host, pos),
                Some(Hot::Square(b)) => {
                    self.drag = None;
                    Self::set(host, &band_id(b, B_DRIVE.id), B_DRIVE.default);
                    Self::set(host, &band_id(b, B_BIAS.id), B_BIAS.default);
                    true
                }
                _ => false,
            },
            UiEvent::Wheel { pos, lines, mods } => match self.hot_at(host, pos) {
                Some(Hot::Rail(_)) => self.rail.wheel(host, pos, lines, mods),
                Some(Hot::Square(b)) => {
                    let id = band_id(b, B_DRIVE.id);
                    Self::set(host, &id, B_DRIVE.wheel(Self::plain(host, &id), lines, mods.fine()));
                    true
                }
                _ => false,
            },
            _ => false,
        }
    }

    fn after_session(&mut self, host: &dyn ParamHost) {
        if self.session.take_replaced() {
            if Self::layout(host).position(self.rail.selected).is_none() {
                self.rail.selected = 0;
            }
            self.knob_key = None;
        }
    }
}

fn model_words(m: Model) -> &'static str {
    match m {
        Model::Tube => "asymmetric, so even harmonics; its bias drifts with the level, so they hold however hard it is pushed",
        Model::Diode => "one diode against two: a hard knee into a soft, never-ending climb, each half its own, so even harmonics",
        Model::Soft => "a gentle cubic: odd harmonics, rounding into a ceiling",
        Model::Hard => "clipping, its corner rounded so it folds nothing harsh back",
        Model::Fold => "a wavefolder: past the top it folds back down",
        Model::Rectify => "full-wave: the fundamental gives way to the octave above",
        Model::Tape => "magnetic hysteresis behind tape's emphasis: the highs saturate first, soft and loose, and the curve opens into a loop",
        Model::Transformer => "an iron core: the lows saturate first (its flux is the integral of the voltage), a narrow loop",
        Model::Harmonic => "drawn: drag the markers over the bars to set each harmonic for a sine reaching full scale; quieter, the low ones lead",
    }
}

fn knob_words(id: &str) -> &'static str {
    match id {
        "drive" => " — how hard the band goes into the curve (0 dB: clean)",
        "bias" => " — moves the operating point off centre, with the level: even harmonics at any drive",
        "tone" => " — tilts what the saturation gives around the band's centre",
        "mix" => " — this band's saturation against its clean signal",
        "out" => " — this band's level in the sum",
        "dyn" => " — after the curve, against the band's own average level (dashed in the square): left compresses, right brings back the hits the curve flattened",
        "dyn_att" => " — how fast the dynamics move once the band rises past its average",
        "dyn_rel" => " — how fast they let go",
        "lr" => " — this band's output panned: left takes the right away, right the left",
        "ms" => " — this band's output, mid against side: left narrows it to mono, right keeps only the sides",
        "fb" => " — the saturated sound fed back through a resonance: the band rings at FREQ, never runs away",
        "fb_freq" => " — where the feedback rings",
        "punch" => " — more drive on the hits (less to the left): grit on the attack, not more level",
        "body" => " — more drive on what sustains after the hits (less to the left): thicker tails, not more level",
        _ => "",
    }
}

fn stereo_words(mode: usize) -> &'static str {
    match mode {
        1 => "only the middle of the image saturates, the sides stay clean. Click to choose",
        2 => "only the sides saturate, the middle stays clean. Click to choose",
        3 => "the middle and the sides saturate each on their own. Click to choose",
        4 => "only the left channel saturates, the right stays clean. Click to choose",
        5 => "only the right channel saturates, the left stays clean. Click to choose",
        _ => "left and right saturate each on their own. Click to choose",
    }
}

impl View for SatView {
    fn logical_size(&self) -> (f64, f64) {
        (WIDTH, HEIGHT)
    }

    fn draw(&mut self, cx: &mut DrawCx<'_>) {
        self.sync_knobs(cx.host);
        let theme = cx.theme;
        let layout = Self::layout(cx.host);
        let l = lanes(self.grow);
        self.draw_header(cx);
        cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.display, None, &Rect::new(0.0, DISPLAY_TOP, WIDTH, HELP_TOP));
        let matrix_hole = (self.matrix_open || self.tape_open).then(|| RoundedRect::from_rect(MATRIX_PANEL, 12.0));
        if let Some(h) = matrix_hole {
            cx.begin_glow_hole(h);
        }
        self.draw_spectrum(cx, &layout, l.spectrum);
        let breath: [f32; BANDS] = std::array::from_fn(|b| Self::engagement(cx.host, b) * self.peak[b].min(1.0));
        self.rail.draw_under(cx, &layout, &breath);
        self.rail.draw_over(cx, &layout);
        let cards = self.cards(&layout);
        self.rail.draw_funnels(cx, &layout, &cards.iter().map(|c| c.rect).collect::<Vec<_>>(), l.rail.y1, l.cy);
        self.draw_cards(cx, &layout, &cards);
        self.draw_meters(cx);
        self.draw_states(cx, l.spectrum);
        self.help_text(cx.host).draw(cx, Rect::new(0.0, HELP_TOP, WIDTH, FOOTER_TOP));
        if matrix_hole.is_some() {
            cx.end_glow_hole();
        }
        if self.matrix_open {
            self.matrix.draw(cx, &self.shared.matrix);
        }
        if self.tape_open {
            self.tape.draw(cx);
        }
        self.draw_footer(cx);
        if let Some((_, list)) = &self.stereo_menu {
            list.draw(cx, theme.accent, |scene, i, at, ink| glyphs::draw_stereo(scene, i, at + (11.0, 12.0), 22.0, ink));
        }
        if let Some(menu) = &self.menu {
            menu.draw(cx, (theme.text_hi, rail_glyph::CROSS));
        }
        self.session.draw_menu(cx);
    }

    fn event(&mut self, event: &UiEvent, host: &dyn ParamHost) -> bool {
        self.sync_knobs(host);
        self.session.begin(host);
        let recording = RecordingHost::new(host);
        let changed = self.handle_event(event, &recording);
        let busy = self.busy();
        self.session.end(host, recording.wrote(), busy);
        self.after_session(host);
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
        // The layout eases between one band and several.
        let layout = Self::layout(host);
        let target = if layout.n > 0 { 1.0 } else { 0.0 };
        let step = dt * 1000.0 / GROW_MS;
        let before = self.grow;
        self.grow = if target > self.grow { (self.grow + step).min(target) } else { (self.grow - step).max(target) };
        let grew = self.grow != before;
        self.rail.area = lanes(self.grow).rail;
        if layout.position(self.rail.selected).is_none() {
            self.rail.selected = 0;
        }
        if let Some(r) = self.history.back().copied() {
            let targets = [r[record::IN], r[record::IN + 1], r[record::OUT], r[record::OUT + 1]];
            for (m, t) in self.meters.iter_mut().zip(targets) {
                let fall = METER_RELEASE_DB_S * dt as f32;
                *m = if t > *m { t } else { (*m - fall).max(t) };
            }
        }
        // Each band's peak, point and auto gain, eased like the spectrum (the last 20 ms).
        let k = |s: f64| (1.0 - (-dt / s).exp()) as f32;
        let mut moved = false;
        for b in 0..BANDS {
            let peak_db = self.history.iter().rev().take(4).map(|r| r[record::BAND_IN + b]).fold(-150.0f32, f32::max);
            let target = if peak_db > -120.0 { 10f32.powf(peak_db / 20.0) } else { 0.0 };
            let before = (self.peak[b], self.point[b], self.auto[b], self.dynamics[b]);
            self.peak[b] += (target - self.peak[b]) * if target > self.peak[b] { k(PEAK_RISE_S) } else { k(PEAK_FALL_S) };
            if let Some(r) = self.history.back() {
                self.point[b] += (r[record::POINT + b] - self.point[b]) * k(0.03);
                self.auto[b] += (r[record::AUTO + b] - self.auto[b]) * k(0.03);
                self.dynamics[b] += (r[record::DYN + b] - self.dynamics[b]) * k(0.03);
                let t = r[record::DYN_THR + b];
                self.threshold[b] = if self.threshold[b] == 0.0 { t } else { self.threshold[b] + (t - self.threshold[b]) * k(0.1) };
            }
            moved |= (self.peak[b] - before.0).abs() > 1e-4 || (self.point[b] - before.1).abs() > 1e-4 || (self.auto[b] - before.2).abs() > 1e-4 || (self.dynamics[b] - before.3).abs() > 1e-4;
        }
        let probe = self.update_probe(host, &layout, dt);
        let spectrum = self.spectrum.update(&self.shared, dt);
        let flowing = self.last_fresh.elapsed().as_secs_f32() < 0.25;
        let deck = self.shared.deck();
        if self.tape_open {
            self.tape.advance(host, deck);
        }
        let deck_moving = self.tape_open && self.tape.moving(deck);
        fresh || flowing || moved || grew || probe || spectrum || deck_moving || self.busy() || self.matrix_open || self.meters.iter().any(|&m| m > -149.0)
    }
}
