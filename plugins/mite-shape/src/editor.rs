//! The Mite Shape editor (docs/design/mockups/shape-main.dc.html, design pass 5), in the mockup's
//! 1280×760 design units.
//!
//! The display, top to bottom: the history (the hero: input dim, output lit, and under its own
//! 0 dB line the selected band's gain, boosts rising and cuts hanging, the attack part solid and
//! the sustain part light, a tick for every hit the detector finds), the crossover rail over the
//! input spectrum, and a card per band with its transient scope (a test hit through the band's
//! real detector). One band is the same screen with no crossovers: pull one in from either end of
//! the rail, or double-click it, and the rail grows and the card splits. Everything drawn comes
//! from `mite_dsp::shape` and the engine's history records, so the pictures are honest.

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Instant;

use mite_core::{ParamDisplay, ParamScale, ParamSpec};
use mite_dsp::analyzer::{spectrum_point_freq, Analyzer, LogSpectrum, SPECTRUM_POINTS};
use mite_dsp::shape::{simulate_hit, BandShape, Layout, AMOUNT_DB, BANDS, CROSSOVERS, FREQ_MAX, FREQ_MIN, MIN_GAP_OCT};
use mite_ui::kurbo::{Affine, BezPath, Cap, Circle, Join, Line, Point, Rect, RoundedRect, Stroke};
use mite_ui::paint::{glass_panel, glow_fill, glow_stroke, icon_button, panel};
use mite_ui::peniko::{Color, Fill, Gradient};
use mite_ui::session::{Session, SessionConfig};
use mite_ui::widgets::brand::{draw_mark, draw_title};
use mite_ui::widgets::footer::draw_dropdown_button;
use mite_ui::widgets::{Dropdown, DropdownEvent, Footer, FooterDial, FooterToggle, Help, Knob, KnobKind, ToggleGlyph};
use mite_ui::{write_if_changed, Align, DrawCx, Key, Modifiers, MouseButton, ParamHost, RecordingHost, TextStyle, UiEvent, View};

use crate::dsp::record;
use crate::params::*;
use crate::presets::{preset_ids, FACTORY, PLUGIN};
use crate::ShapeShared;

pub use mite_dsp::analyzer::ANALYZER_SIZE;

/// Mite Shape wears the Shape accent (tokens.json `color.accent.shape`, leaf).
pub const ACCENT: &str = "shape";

const WIDTH: f64 = 1280.0;
const HEIGHT: f64 = 760.0;
const HEADER: f64 = 56.0;
const DISPLAY_TOP: f64 = HEADER;
const DISPLAY_H: f64 = 620.0;
const HELP_TOP: f64 = DISPLAY_TOP + DISPLAY_H;
const FOOTER_TOP: f64 = HELP_TOP + 28.0;
const HEADER_ROW: (f64, f64) = (12.0, 44.0);
/// Shape's own header buttons (stereo, detector), after the snapshot letters.
const HEAD_BUTTONS_X: f64 = 760.0;
const HEAD_BUTTON_W: f64 = 40.0;

/// History: 6 s across the left 1180 px.
const HIST_W: f64 = 1180.0;
const HIST_SECONDS: f64 = 6.0;
/// Records per drawn point (10 ms).
const PER_POINT: usize = 2;
/// Gain lane: 5 px/dB, eased flat towards ±11.5 dB (tokens.json `shape.history`).
const GAIN_PX_PER_DB: f64 = 5.0;
const GAIN_SOFT_DB: f64 = 11.5;
/// Level lane: −48…0 dB.
const LEVEL_RANGE_DB: f64 = 48.0;
/// Meter release (tokens.json `meter.releaseDbPerSec`).
const METER_RELEASE_DB_S: f32 = 20.0;
const METER_TOP: f64 = DISPLAY_TOP + 20.0;
const METER_BOTTOM: f64 = DISPLAY_TOP + 592.0;

/// The crossover rail: 20 Hz at x 14, 20 kHz at x 1166.
const RAIL_X0: f64 = 14.0;
const RAIL_W: f64 = 1152.0;
/// A crossover grip (14×28) and its hit area.
const GRIP: (f64, f64) = (14.0, 28.0);
const GRIP_HIT: f64 = 16.0;
/// Dragging a crossover this close to a neighbour (px), or this far past an end, merges.
const MERGE_PX: f64 = 6.0;
const OFF_END_PX: f64 = 10.0;

/// The rail's spectrum: these levels at its top and bottom, dB on the analyzer's scale.
const SPEC_TOP_DB: f32 = -18.0;
const SPEC_BOTTOM_DB: f32 = -84.0;
const SPECTRUM_RISE_S: f64 = 0.06;
const SPECTRUM_FALL_S: f64 = 0.3;

/// Cards: the selected one is this wide (multiband), the gap between cards.
const CARD_WIDE: f64 = 644.0;
const CARD_GAP: f64 = 8.0;
/// Scope time axis: 0…400 ms on a square-root scale.
const SCOPE_MS: f64 = 400.0;
const SCOPE_PAD: f64 = 6.0;

/// The layout grows from one band to several (base motion, 220 ms).
const GROW_MS: f64 = 220.0;

/// Docked panels.
const DETECTOR_SHEET: Rect = Rect::new(12.0, DISPLAY_TOP + 392.0, 832.0, DISPLAY_TOP + 610.0);
const STEREO_POP: Rect = Rect::new(852.0, DISPLAY_TOP + 8.0, 1088.0, DISPLAY_TOP + 120.0);

/// Glyphs (24×24, stroked; docs/design/mockups/shape-main.dc.html `G`).
mod glyph {
    pub const LINK: &str = "M9 7.5 A4.5 4.5 0 1 0 9.01 7.5 M15 7.5 A4.5 4.5 0 1 0 15.01 7.5";
    pub const MS: &str = "M12 4 A8 8 0 1 0 12.01 4 M12 9.5 A2.5 2.5 0 1 0 12.01 9.5";
    pub const LR: &str = "M8.5 7 A5 5 0 1 0 8.51 7 M15.5 7 A5 5 0 1 0 15.51 7";
    pub const SLIDERS: &str = "M4 7 H20 M4 12 H20 M4 17 H20 M8.5 5 V9 M15 10 V14 M10.5 15 V19";
    pub const LISTEN: &str = "M4 14 V12 A8 8 0 0 1 20 12 V14 M4 14 H7 V20 H4 Z M20 14 H17 V20 H20 Z";
    pub const DELTA: &str = "M12 4 L20.5 19 H3.5 Z";
    pub const MUTE: &str = "M4 9.5 H7.5 L12 5.5 V18.5 L7.5 14.5 H4 Z M15.5 9.5 L20.5 14.5 M20.5 9.5 L15.5 14.5";
    pub const BIN: &str = "M5 7 H19 M10 7 V5 H14 V7 M7 7 L8 20 H16 L17 7 M10.5 10.5 V16.5 M13.5 10.5 V16.5";
    pub const RESET: &str = "M5 12 A7 7 0 1 0 7.1 7 M5 4.5 V8.5 H9";
    pub const COPY: &str = "M8 8 H19 V20 H8 Z M5 16 V4 H15";
    pub const PASTE: &str = "M7 5 H5 V21 H19 V5 H17 M9 3 H15 V7 H9 Z";
    pub const MERGE: &str = "M4 6 C10 6 11 12 20 12 M4 18 C10 18 11 12 20 12 M17 9 L20 12 L17 15";
    pub const LR4: &str = "M3 7 H8 C12 7 13 17 21 17 M3 17 C11 17 12 7 16 7 H21";
    pub const SOLO: &str = "M12 3.5 A8.5 8.5 0 1 0 12.01 3.5";
}

/// Draws a 24×24 glyph scaled to `size`, centred on `c`.
fn draw_glyph(scene: &mut mite_ui::vello::Scene, d: &str, c: Point, size: f64, color: Color, width: f64) {
    let Ok(mut path) = BezPath::from_svg(d) else { return };
    let k = size / 24.0;
    path.apply_affine(Affine::translate((c.x - 12.0 * k, c.y - 12.0 * k)) * Affine::scale(k));
    let stroke = Stroke::new(width).with_caps(Cap::Round).with_join(Join::Round);
    scene.stroke(&stroke, Affine::IDENTITY, color, None, &path);
}

/// A stand-in host holding every Mite Shape parameter at its default, for tests, the preview and
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
                let short = id.split_once('_').filter(|(p, _)| p.len() == 2 && (p.starts_with('x') || p.starts_with('b'))).map_or(id, |(_, rest)| rest);
                let base = [LINK, LOOKAHEAD, FLOOR, DET_HP, DET_LP, MIX, OUT_PAN, OUT_GAIN]
                    .into_iter()
                    .find(|s| s.id == id)
                    .or_else(|| {
                        let pool: &[ParamSpec] = if id.starts_with('x') { &[X_FREQ] } else { &[B_ATTACK, B_SUSTAIN, B_TIME, B_TRANSITION, B_MIX, B_OUT] };
                        pool.iter().copied().find(|s| s.id == short)
                    })
                    .expect("every manifest line has a spec");
                ParamSpec { id, default: f[4].parse().unwrap_or(base.default), ..base }
            }
        };
        host = host.with(spec);
    }
    host
}

/// The engine's settings as a host's parameters set them (as `ShapeParams::settings` does), for
/// tests, the preview and tools that drive the engine from a stand-in host.
pub fn settings(host: &dyn ParamHost) -> mite_dsp::shape::ShapeSettings {
    let p = |id: &str| host.plain(id, host.normalized(id));
    let on = |id: &str| p(id) >= 0.5;
    mite_dsp::shape::ShapeSettings {
        crossovers: std::array::from_fn(|k| on(&cross_id(k, X_ON.id)).then(|| p(&cross_id(k, X_FREQ.id)))),
        bands: std::array::from_fn(|b| mite_dsp::shape::BandShape {
            attack_db: p(&band_id(b, B_ATTACK.id)),
            sustain_db: p(&band_id(b, B_SUSTAIN.id)),
            transient_ms: p(&band_id(b, B_TIME.id)),
            transition: p(&band_id(b, B_TRANSITION.id)) / 100.0,
            mix: p(&band_id(b, B_MIX.id)) / 100.0,
            out_db: p(&band_id(b, B_OUT.id)),
            mute: on(&band_id(b, B_MUTE.id)),
            solo: on(&band_id(b, B_SOLO.id)),
        }),
        detector_hp: p(DET_HP.id),
        detector_lp: p(DET_LP.id),
        listen: on(LISTEN.id),
        lookahead_ms: p(LOOKAHEAD.id),
        mid_side: p(STEREO.id) >= 0.5,
        link: p(LINK.id) / 100.0,
        floor_db: p(FLOOR.id),
    }
}

/// The display's vertical layout (absolute y), blended between one band and several.
#[derive(Debug, Clone, Copy)]
struct Lanes {
    /// Level lane top and bottom, the divider, the gain lane's 0 dB line and bottom.
    lv0: f64,
    lv1: f64,
    sep: f64,
    gz: f64,
    h1: f64,
    /// The rail's top and height, the funnels' top, the cards' top and height.
    ry: f64,
    rh: f64,
    fy: f64,
    cy: f64,
    ch: f64,
}

const SINGLE: [f64; 10] = [12.0, 194.0, 204.0, 268.0, 332.0, 340.0, 36.0, 376.0, 384.0, 228.0];
const MULTI: [f64; 10] = [12.0, 164.0, 174.0, 234.0, 292.0, 300.0, 80.0, 380.0, 396.0, 216.0];

fn lanes(grow: f64) -> Lanes {
    let v: Vec<f64> = SINGLE.iter().zip(MULTI).map(|(a, b)| a + (b - a) * grow).collect();
    let y = |i: usize| DISPLAY_TOP + v[i];
    Lanes { lv0: y(0), lv1: y(1), sep: y(2), gz: y(3), h1: y(4), ry: y(5), rh: v[6], fy: y(7), cy: y(8), ch: v[9] }
}

/// The gain lane's soft clamp: linear to 9.5 dB, then easing flat towards 11.5.
fn soft_gain(g: f64) -> f64 {
    let a = g.abs();
    let c = if a < GAIN_SOFT_DB - 2.0 { a } else { GAIN_SOFT_DB - 2.0 + 2.0 * ((a - GAIN_SOFT_DB + 2.0) / 2.0).tanh() };
    c.copysign(g)
}

fn rail_x(f: f64) -> f64 {
    RAIL_X0 + RAIL_W * (f / f64::from(FREQ_MIN)).ln() / 1000f64.ln()
}

fn rail_f(x: f64) -> f64 {
    f64::from(FREQ_MIN) * 1000f64.powf(((x - RAIL_X0) / RAIL_W).clamp(0.0, 1.0))
}

fn fmt_freq(f: f64) -> String {
    if f >= 10_000.0 {
        format!("{:.0} kHz", f / 1000.0)
    } else if f >= 1000.0 {
        format!("{:.1} kHz", f / 1000.0)
    } else {
        format!("{:.0} Hz", f)
    }
}

fn fmt_db(v: f64) -> String {
    format!("{}{:.1}", if v > 0.05 { "+" } else if v < -0.05 { "−" } else { "" }, v.abs())
}

/// Which part of a band's shaping a scope drag or handle sets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Part {
    Attack,
    Sustain,
    Time,
    Transition,
}

/// What dragging a crossover onto something does on release.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Merge {
    /// Onto the crossover below (its slot): the band between them goes, the one above grows.
    Down(usize),
    /// Onto the one above: the band between them goes, the one below grows.
    Up,
    /// Off the low end (the lowest band goes) or the high end (the highest).
    OffLow,
    OffHigh,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Drag {
    Cross { slot: usize, start: Point, f0: f64, merge: Option<Merge> },
    /// Pulling a new crossover in from an end of the rail.
    Create { low: bool, start: Point, now: Point },
    Scope { band: usize, part: Part, start: Point, value: f32 },
    /// The detector filter's high-pass (true) or low-pass node.
    Filter { hp: bool },
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Hot {
    Grip(usize),
    Ghost(bool),
    Region(usize),
    Card(usize),
    Scope(usize, Part),
    History,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Panel {
    Detector,
    Stereo,
}

/// The band menu (right-click a band).
const MENU_ITEMS: &[&str] = &["Solo", "Mute", "Copy settings", "Paste settings", "Reset band", "Merge with the band below", "Merge with the band above", "Remove band"];
const MENU_GLYPHS: [&str; 8] = [glyph::SOLO, glyph::MUTE, glyph::COPY, glyph::PASTE, glyph::RESET, glyph::MERGE, glyph::MERGE, glyph::BIN];

struct Menu {
    band: usize,
    list: Dropdown,
}

/// One band's card: where it is and what is in it.
#[derive(Debug, Clone)]
struct Card {
    band: usize,
    rect: Rect,
    scope: Rect,
    /// The wide one: six dials and the scope's handles.
    big: bool,
    solo: Option<Rect>,
    mute: Option<Rect>,
    bin: Option<Rect>,
    /// Dial centres (six for the big card, two small ones otherwise).
    dials: Vec<Point>,
}

/// Rectangles of the open panel's controls, set while drawing.
#[derive(Default, Clone)]
struct PanelHits {
    rect: Rect,
    close: Rect,
    listen: Option<Rect>,
    filter: Option<Rect>,
    modes: Vec<(Rect, f32)>,
}

/// The band settings a copy carries (the sound ones), in this order.
const BAND_SPECS: [ParamSpec; 6] = [B_ATTACK, B_SUSTAIN, B_TIME, B_TRANSITION, B_MIX, B_OUT];
const BAND_LABELS: [&str; 6] = ["ATTACK", "SUSTAIN", "TIME", "TRANSITION", "MIX", "OUTPUT"];

pub struct ShapeView {
    shared: Arc<ShapeShared>,
    session: Session,
    footer: Footer,
    selected: usize,
    pointer: Option<Point>,
    hot: Option<Hot>,
    drag: Option<Drag>,
    panel: Option<Panel>,
    panel_knobs: Vec<Knob>,
    knobs_for: Option<Panel>,
    hits: PanelHits,
    menu: Option<Menu>,
    head_hits: [Rect; 2],
    lr4_hit: Option<Rect>,
    /// The cards' dials, for the bands and the selection they were made for.
    card_knobs: Vec<Knob>,
    card_key: Option<(Vec<usize>, usize)>,
    /// Copied band settings (attack, sustain, time, transition, mix, output, mute).
    clipboard: Option<[f32; 7]>,
    /// History records (newest last), the tap's read position, and the meters as drawn (IN L/R,
    /// OUT L/R) with their ballistics.
    history: VecDeque<[f32; record::WIDTH]>,
    seen: usize,
    meters: [f32; 4],
    last_fresh: Instant,
    last_tick: Instant,
    /// 0 = one band's layout, 1 = several (eased between).
    grow: f64,
    analyzer: Analyzer,
    log_spectrum: LogSpectrum,
    samples: Vec<f32>,
    power: Vec<f32>,
    spectrum_target: Vec<f32>,
    spectrum: Vec<f32>,
    last_analysis: Instant,
}

impl ShapeView {
    pub fn new(shared: Arc<ShapeShared>) -> Self {
        let footer = Footer::new(
            FOOTER_TOP,
            (WIDTH, HEIGHT),
            vec![
                FooterToggle { id: AUTO_GAIN.id, glyph: ToggleGlyph::Auto, words: "Auto gain — keeps the loudness where it was as the shaping changes it (measured over a second or two)" },
                FooterToggle { id: DELTA.id, glyph: ToggleGlyph::Delta, words: "Delta — hear only what Shape adds or removes" },
                FooterToggle { id: BYPASS.id, glyph: ToggleGlyph::Bypass, words: "Bypass — the whole plugin, latency kept" },
            ],
            vec![
                FooterDial { spec: MIX, kind: KnobKind::Unipolar, name: "MIX", words: "Mix — the shaping against the dry signal, delay-compensated" },
                FooterDial { spec: OUT_PAN, kind: KnobKind::Bipolar, name: "PAN", words: "Output balance" },
                FooterDial { spec: OUT_GAIN, kind: KnobKind::Bipolar, name: "OUTPUT", words: "Output gain" },
            ],
        );
        let analyzer = Analyzer::new(ANALYZER_SIZE);
        let bins = analyzer.bins();
        Self {
            session: Session::new(
                SessionConfig { plugin: PLUGIN, factory: FACTORY, sound_ids: preset_ids(), snapshot_ids: preset_ids() },
                shared.snapshots.clone(),
                shared.preset.clone(),
            ),
            footer,
            selected: 0,
            pointer: None,
            hot: None,
            drag: None,
            panel: None,
            panel_knobs: Vec::new(),
            knobs_for: None,
            hits: PanelHits::default(),
            menu: None,
            head_hits: [Rect::ZERO; 2],
            lr4_hit: None,
            card_knobs: Vec::new(),
            card_key: None,
            clipboard: None,
            history: VecDeque::with_capacity(crate::dsp::HISTORY_RECORDS),
            seen: 0,
            meters: [-150.0; 4],
            last_fresh: Instant::now(),
            last_tick: Instant::now(),
            grow: 0.0,
            analyzer,
            log_spectrum: LogSpectrum::new(ANALYZER_SIZE, 48_000.0),
            samples: vec![0.0; ANALYZER_SIZE],
            power: vec![0.0; bins],
            spectrum_target: vec![-150.0; SPECTRUM_POINTS],
            spectrum: vec![-150.0; SPECTRUM_POINTS],
            last_analysis: Instant::now(),
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

    /// Selects a band slot, for tests and tools.
    pub fn select(&mut self, band: usize) {
        self.selected = band.min(BANDS - 1);
    }

    pub fn selected(&self) -> usize {
        self.selected
    }

    /// Opens a header panel by name (`detector`, `stereo`), for tools.
    pub fn open_panel(&mut self, name: &str) {
        self.panel = match name {
            "detector" => Some(Panel::Detector),
            "stereo" => Some(Panel::Stereo),
            _ => None,
        };
        self.knobs_for = None;
    }

    /// Opens band `band`'s menu, for tools.
    pub fn open_menu(&mut self, band: usize, host: &dyn ParamHost) {
        let layout = Self::layout(host);
        let Some(p) = layout.position(band) else { return };
        let lanes = lanes(self.grow);
        let x = self.region_rect(host, &layout, p, &lanes).center().x;
        self.open_band_menu(band, Point::new(x - 20.0, lanes.ry - 4.0), host);
    }

    /// Lets the layout settle at once (tools render a single frame).
    pub fn settle(&mut self, host: &dyn ParamHost) {
        self.grow = if Self::layout(host).n > 0 { 1.0 } else { 0.0 };
    }

    // ---- parameters -------------------------------------------------------------------------

    fn plain(host: &dyn ParamHost, id: &str) -> f32 {
        host.plain(id, host.normalized(id))
    }

    fn index(host: &dyn ParamHost, id: &str) -> usize {
        Self::plain(host, id).round().max(0.0) as usize
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

    fn crossovers(host: &dyn ParamHost) -> [Option<f32>; CROSSOVERS] {
        std::array::from_fn(|k| Self::on(host, &cross_id(k, X_ON.id)).then(|| Self::plain(host, &cross_id(k, X_FREQ.id))))
    }

    fn layout(host: &dyn ParamHost) -> Layout {
        Layout::new(&Self::crossovers(host))
    }

    fn band(host: &dyn ParamHost, b: usize) -> BandShape {
        let id = |p: &str| band_id(b, p);
        BandShape {
            attack_db: Self::plain(host, &id(B_ATTACK.id)),
            sustain_db: Self::plain(host, &id(B_SUSTAIN.id)),
            transient_ms: Self::plain(host, &id(B_TIME.id)),
            transition: Self::plain(host, &id(B_TRANSITION.id)) / 100.0,
            mix: Self::plain(host, &id(B_MIX.id)) / 100.0,
            out_db: Self::plain(host, &id(B_OUT.id)),
            mute: Self::on(host, &id(B_MUTE.id)),
            solo: Self::on(host, &id(B_SOLO.id)),
        }
    }

    /// The soloed band, if any (the first in use).
    fn soloed(host: &dyn ParamHost, layout: &Layout) -> Option<usize> {
        layout.bands().iter().copied().find(|&b| Self::on(host, &band_id(b, B_SOLO.id)))
    }

    fn audible(host: &dyn ParamHost, layout: &Layout, b: usize) -> bool {
        let solo = Self::soloed(host, layout);
        !Self::on(host, &band_id(b, B_MUTE.id)) && solo.is_none_or(|s| s == b)
    }

    /// The band the gain lane shows: the soloed one, else the selected one.
    fn shown(&self, host: &dyn ParamHost, layout: &Layout) -> usize {
        Self::soloed(host, layout).unwrap_or(if layout.position(self.selected).is_some() { self.selected } else { 0 })
    }

    fn lookahead_ms(host: &dyn ParamHost) -> f64 {
        f64::from(Self::plain(host, LOOKAHEAD.id))
    }

    fn band_values(host: &dyn ParamHost, b: usize) -> [f32; 7] {
        let mut v = [0.0; 7];
        for (k, spec) in BAND_SPECS.iter().enumerate() {
            v[k] = Self::plain(host, &band_id(b, spec.id));
        }
        v[6] = if Self::on(host, &band_id(b, B_MUTE.id)) { 1.0 } else { 0.0 };
        v
    }

    fn write_band(host: &dyn ParamHost, b: usize, v: &[f32; 7]) {
        for (k, spec) in BAND_SPECS.iter().enumerate() {
            Self::set(host, &band_id(b, spec.id), v[k]);
        }
        Self::set(host, &band_id(b, B_MUTE.id), v[6]);
    }

    fn copy_band(host: &dyn ParamHost, from: usize, to: usize) {
        Self::write_band(host, to, &Self::band_values(host, from));
    }

    fn reset_band(host: &dyn ParamHost, b: usize) {
        let mut v = [0.0; 7];
        for (k, spec) in BAND_SPECS.iter().enumerate() {
            v[k] = spec.default;
        }
        Self::write_band(host, b, &v);
    }

    /// Turns crossover `slot` off: its band (the region above it) joins the one below.
    fn remove_crossover(host: &dyn ParamHost, slot: usize) {
        Self::set(host, &cross_id(slot, X_ON.id), 0.0);
        Self::set(host, &band_id(slot + 1, B_SOLO.id), 0.0);
    }

    /// Makes a crossover at `f` Hz (pushed clear of its neighbours): the band it splits keeps its
    /// settings on both sides. Returns the new band, or `None` when five is the limit or there
    /// is no room.
    fn split(&mut self, host: &dyn ParamHost, f: f64) -> Option<usize> {
        let x = Self::crossovers(host);
        let slot = (0..CROSSOVERS).find(|&k| x[k].is_none())?;
        let layout = Layout::new(&x);
        let gap = 2f64.powf(f64::from(MIN_GAP_OCT));
        let p = (0..=layout.n).find(|&p| layout.region(layout.owners[p], &x).is_some_and(|(lo, hi)| f >= f64::from(lo) && f < f64::from(hi))).unwrap_or(0);
        let parent = layout.owners[p];
        let (lo, hi) = layout.region(parent, &x).unwrap_or((FREQ_MIN, FREQ_MAX));
        let (lo, hi) = (if p == 0 { f64::from(lo) } else { f64::from(lo) * gap }, if p == layout.n { f64::from(hi) } else { f64::from(hi) / gap });
        if lo >= hi {
            return None;
        }
        let f = f.clamp(lo.max(f64::from(FREQ_MIN) * 1.06), hi.min(f64::from(FREQ_MAX) / 1.06));
        Self::copy_band(host, parent, slot + 1);
        Self::set(host, &band_id(slot + 1, B_SOLO.id), 0.0);
        Self::set(host, &cross_id(slot, X_FREQ.id), f as f32);
        Self::set(host, &cross_id(slot, X_ON.id), 1.0);
        self.selected = slot + 1;
        Some(slot + 1)
    }

    /// Removes band `b`: its region joins the band below (the lowest band's joins the one above,
    /// which takes its place).
    fn remove_band(&mut self, host: &dyn ParamHost, b: usize) {
        let layout = Self::layout(host);
        let Some(p) = layout.position(b) else { return };
        if layout.n == 0 {
            return;
        }
        if p == 0 {
            let above = layout.owners[1];
            Self::copy_band(host, above, 0);
            Self::remove_crossover(host, above - 1);
            self.selected = 0;
        } else {
            Self::remove_crossover(host, b - 1);
            self.selected = layout.owners[p - 1];
        }
    }

    /// Merges band `b` with its neighbour below (`up` false) or above: `b`'s region grows over
    /// the neighbour's when merging up, and joins the neighbour below when merging down.
    fn merge(&mut self, host: &dyn ParamHost, b: usize, up: bool) {
        let layout = Self::layout(host);
        let Some(p) = layout.position(b) else { return };
        if up && p < layout.n {
            let above = layout.owners[p + 1];
            Self::remove_crossover(host, above - 1);
            self.selected = b;
        } else if !up && p > 0 {
            self.remove_band(host, b);
        }
    }

    // ---- geometry ---------------------------------------------------------------------------

    /// Band at position `p`'s region on the rail, x from the first band's left edge to the last's
    /// right edge.
    fn region_rect(&self, host: &dyn ParamHost, layout: &Layout, p: usize, lanes: &Lanes) -> Rect {
        let x = Self::crossovers(host);
        let edge = |q: usize| rail_x(f64::from(x[layout.crossovers[q]].unwrap_or(FREQ_MIN)));
        let x0 = if p == 0 { 0.0 } else { edge(p - 1) };
        let x1 = if p == layout.n { HIST_W } else { edge(p) };
        Rect::new(x0, lanes.ry, x1, lanes.ry + lanes.rh)
    }

    fn grip_center(host: &dyn ParamHost, slot: usize, lanes: &Lanes) -> Point {
        let f = Self::plain(host, &cross_id(slot, X_FREQ.id));
        Point::new(rail_x(f64::from(f)), lanes.ry + lanes.rh / 2.0)
    }

    fn ghost_centers(lanes: &Lanes) -> [Point; 2] {
        let y = lanes.ry + lanes.rh / 2.0;
        [Point::new(rail_x(f64::from(FREQ_MIN)) + 6.0, y), Point::new(rail_x(f64::from(FREQ_MAX)) - 6.0, y)]
    }

    fn cards(&self, layout: &Layout, lanes: &Lanes) -> Vec<Card> {
        let bands = layout.bands();
        let n = bands.len();
        let (y, h) = (lanes.cy, lanes.ch);
        if n == 1 {
            let rect = Rect::new(0.0, y, HIST_W, y + h);
            let scope = Rect::new(10.0, y + 10.0, 798.0, y + h - 12.0);
            let x0 = 810.0;
            let col = (HIST_W - 10.0 - x0) / 3.0;
            let top = y + 10.0 + ((h - 20.0) - 192.0) / 2.0;
            let dials = (0..6).map(|i| Point::new(x0 + col * ((i % 3) as f64 + 0.5), top + 39.5 + 98.0 * (i / 3) as f64)).collect();
            return vec![Card { band: bands[0], rect, scope, big: true, solo: None, mute: None, bin: None, dials }];
        }
        let narrow = (HIST_W - CARD_WIDE - CARD_GAP * (n - 1) as f64) / (n - 1) as f64;
        let mut x = 0.0;
        bands
            .iter()
            .map(|&band| {
                let big = band == self.shown_selected(layout);
                let w = if big { CARD_WIDE } else { narrow };
                let rect = Rect::new(x, y, x + w, y + h);
                x += w + CARD_GAP;
                let col = if big { 284.0 } else { w - 20.0 };
                let head_y = y + 10.0;
                let right = rect.x0 + 10.0 + col;
                let button = |k: f64| Rect::new(right - 32.0 * (k + 1.0) - 6.0 * k, head_y, right - 32.0 * k - 6.0 * k, head_y + 32.0);
                let (solo, mute, bin) = if big { (button(2.0), button(1.0), Some(button(0.0))) } else { (button(1.0), button(0.0), None) };
                let sh = if big { 160.0 } else if w > 220.0 { 112.0 } else { 80.0 };
                let scope = Rect::new(rect.x0 + 10.0, head_y + 38.0, rect.x0 + 10.0 + col, head_y + 38.0 + sh);
                let dials = if big {
                    let x0 = rect.x0 + 10.0 + 284.0 + 12.0;
                    let cw = (rect.x1 - 10.0 - x0) / 3.0;
                    let top = y + 10.0 + ((h - 20.0) - 192.0) / 2.0;
                    (0..6).map(|i| Point::new(x0 + cw * ((i % 3) as f64 + 0.5), top + 39.5 + 98.0 * (i / 3) as f64)).collect()
                } else {
                    // Two small dials, side by side when there is room.
                    let top = scope.y1 + 6.0 + 20.0;
                    if col >= 200.0 { vec![Point::new(rect.x0 + 10.0 + 19.0, top), Point::new(rect.x0 + 10.0 + 19.0 + 104.0, top)] } else { vec![Point::new(rect.x0 + 10.0 + 19.0, top), Point::new(rect.x0 + 10.0 + 19.0, top + 40.0)] }
                };
                Card { band, rect, scope, big, solo: Some(solo), mute: Some(mute), bin, dials }
            })
            .collect()
    }

    /// The band whose card is wide: the selected one when it is in use, else the lowest.
    fn shown_selected(&self, layout: &Layout) -> usize {
        if layout.position(self.selected).is_some() { self.selected } else { 0 }
    }

    // ---- the scope --------------------------------------------------------------------------

    fn scope_x(r: Rect, t_ms: f64) -> f64 {
        r.x0 + SCOPE_PAD + (r.width() - 2.0 * SCOPE_PAD) * (t_ms.clamp(0.0, SCOPE_MS) / SCOPE_MS).sqrt()
    }

    fn scope_t(r: Rect, x: f64) -> f64 {
        SCOPE_MS * ((x - r.x0 - SCOPE_PAD) / (r.width() - 2.0 * SCOPE_PAD)).clamp(0.0, 1.0).powi(2)
    }

    /// Pixels per dB of gain in a scope.
    fn scope_k(r: Rect) -> f64 {
        r.height() * 0.40 / 12.0
    }

    fn scope_gain_y(r: Rect, g: f64) -> f64 {
        r.y0 + r.height() * 0.56 - soft_gain(g) * Self::scope_k(r)
    }

    fn handle_points(r: Rect, b: &BandShape) -> [(Part, Point); 3] {
        let t = f64::from(b.transient_ms);
        let wd = (t * f64::from(b.transition)).max(1.5);
        let ta = ((t - wd / 2.0) * 0.45).max(0.6);
        let ts = t + wd / 2.0 + (SCOPE_MS - t) * 0.3;
        [
            (Part::Attack, Point::new(Self::scope_x(r, ta), Self::scope_gain_y(r, f64::from(b.attack_db)))),
            (Part::Sustain, Point::new(Self::scope_x(r, ts), Self::scope_gain_y(r, f64::from(b.sustain_db)))),
            (Part::Time, Point::new(Self::scope_x(r, t), r.y1 - 17.0)),
        ]
    }

    /// Which part a press in a band's scope sets: a handle if one is near, Alt the transition,
    /// else attack before the handover line and sustain after it.
    fn scope_part(r: Rect, b: &BandShape, p: Point, mods: Modifiers) -> Part {
        if mods.alt {
            return Part::Transition;
        }
        if let Some(&(part, _)) = Self::handle_points(r, b).iter().find(|(_, c)| c.distance(p) < 14.0) {
            return part;
        }
        if p.x < Self::scope_x(r, f64::from(b.transient_ms)) { Part::Attack } else { Part::Sustain }
    }

    fn draw_scope(&self, cx: &mut DrawCx<'_>, r: Rect, b: &BandShape, big: bool, handles: bool) {
        let theme = cx.theme;
        panel(cx.scene, RoundedRect::from_rect(r, 8.0), theme.well, theme.well_border);
        cx.scene.push_clip_layer(Fill::NonZero, Affine::IDENTITY, &RoundedRect::from_rect(r, 8.0));
        let la = Self::lookahead_ms(cx.host);
        let pts = simulate_hit(b, la, 2000.0, 0.0, SCOPE_MS);
        let t = f64::from(b.transient_ms);
        let wd = (t * f64::from(b.transition)).max(1.5);
        let z = r.y0 + r.height() * 0.56;
        let gy = |g: f64| Self::scope_gain_y(r, g);
        let ly = |db: f64| r.y0 + 4.0 + (-db.clamp(-LEVEL_RANGE_DB, 0.0) / LEVEL_RANGE_DB) * (r.height() - 8.0);
        let sx = |ms: f64| Self::scope_x(r, ms);
        // Grid: 1, 10, 50, 200 ms; ±6 dB.
        for ms in [1.0, 10.0, 50.0, 200.0] {
            cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, theme.grid_minor, None, &Line::new((sx(ms), r.y0), (sx(ms), r.y1)));
        }
        for g in [6.0, -6.0] {
            cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, theme.grid_minor, None, &Line::new((r.x0, gy(g)), (r.x1, gy(g))));
        }
        // The handover, shaded.
        let shade = Rect::new(sx((t - wd / 2.0).max(0.0)), r.y0, sx(t + wd / 2.0), r.y1);
        cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.accent.multiply_alpha(0.12), None, &shade);
        // The hit as it comes in (faint fill) and as it leaves (a quiet line).
        let mut fill_in = BezPath::new();
        fill_in.move_to((r.x0 + SCOPE_PAD, r.y1));
        for p in &pts {
            fill_in.line_to((sx(p.t_ms), ly(p.level_db)));
        }
        fill_in.line_to((r.x1 - SCOPE_PAD, r.y1));
        fill_in.close_path();
        cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.spectrum_pre.multiply_alpha(0.12), None, &fill_in);
        let out_line = polyline(&pts.iter().map(|p| Point::new(sx(p.t_ms), ly(p.level_db + p.gain_db))).collect::<Vec<_>>());
        cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, theme.text_mid.multiply_alpha(0.55), None, &out_line);
        cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, theme.grid_zero, None, &Line::new((r.x0, z), (r.x1, z)));
        // The gain: light where it is sustain, solid where it is attack (gain × weight).
        let area = |f: &dyn Fn(f64, f64) -> f64| {
            let mut a = BezPath::new();
            a.move_to((r.x0 + SCOPE_PAD, z));
            for p in &pts {
                a.line_to((sx(p.t_ms), gy(f(p.gain_db, p.weight))));
            }
            a.line_to((r.x1 - SCOPE_PAD, z));
            a.close_path();
            a
        };
        let any_up = b.attack_db > 0.0 || b.sustain_db > 0.0;
        let any_down = b.attack_db < 0.0 || b.sustain_db < 0.0;
        if any_up {
            cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.gain_up.multiply_alpha(0.28), None, &area(&|g, _| g.max(0.0)));
            cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.gain_up.multiply_alpha(0.65), None, &area(&|g, w| g.max(0.0) * w));
        }
        if any_down {
            cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.gain_reduction.multiply_alpha(0.28), None, &area(&|g, _| g.min(0.0)));
            cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.gain_reduction.multiply_alpha(0.65), None, &area(&|g, w| g.min(0.0) * w));
        }
        // The handover line, and the lookahead.
        let hx = sx(t);
        cx.scene.stroke(&Stroke::new(1.2).with_dashes(0.0, [3.0, 3.0]), Affine::IDENTITY, theme.accent, None, &Line::new((hx, r.y0), (hx, r.y1)));
        if la > 0.0 {
            cx.scene.stroke(&Stroke::new(1.0).with_dashes(0.0, [2.0, 2.0]), Affine::IDENTITY, theme.spectrum_pre, None, &Line::new((sx(la), r.y0), (sx(la), r.y1)));
        }
        let gain = polyline(&pts.iter().map(|p| Point::new(sx(p.t_ms), gy(p.gain_db))).collect::<Vec<_>>());
        glow_stroke(cx, &gain, if big { 2.2 } else { 1.6 }, theme.curve, theme.curve, 3.0);
        cx.scene.pop_layer();
        if big {
            let label = TextStyle { tabular: true, ..TextStyle::BODY.sized(10.0).weighted(600.0) };
            for (ms, s) in [(1.0, "1"), (10.0, "10"), (50.0, "50"), (200.0, "200 ms")] {
                cx.text.draw(cx.scene, s, sx(ms) + 3.0, r.y1 - 15.0, label, theme.text_lo, Align::Left);
            }
        }
        if handles {
            for (part, c) in Self::handle_points(r, b) {
                let hot = matches!(self.hot, Some(Hot::Scope(_, p)) if p == part) || matches!(self.drag, Some(Drag::Scope { part: p, .. }) if p == part);
                let shape = if part == Part::Time { RoundedRect::from_rect(Rect::from_center_size(c, (8.0, 18.0)), 4.0) } else { RoundedRect::from_rect(Rect::from_center_size(c, (12.0, 12.0)), 6.0) };
                glow_fill(cx, &shape, theme.accent, if hot { 7.0 } else { 4.0 });
                cx.scene.stroke(&Stroke::new(2.0), Affine::IDENTITY, theme.needle, None, &shape);
            }
        }
    }

    // ---- drawing ----------------------------------------------------------------------------

    fn draw_header(&mut self, cx: &mut DrawCx<'_>) {
        let theme = cx.theme;
        cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.window, None, &Rect::new(0.0, 0.0, WIDTH, HEADER));
        cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, theme.line, None, &Line::new((0.0, HEADER - 0.5), (WIDTH, HEADER - 0.5)));
        draw_mark(cx, 16.0, 15.0, 26.0);
        draw_title(cx, 52.0, 18.0, "Shape");
        self.session.draw(cx);
        let (y0, y1) = HEADER_ROW;
        let ms = Self::index(cx.host, STEREO.id) == 1;
        let unlinked = Self::plain(cx.host, LINK.id) < 99.5;
        let listen = Self::on(cx.host, LISTEN.id);
        for (i, (d, p)) in [(if ms { glyph::MS } else { glyph::LINK }, Panel::Stereo), (glyph::SLIDERS, Panel::Detector)].into_iter().enumerate() {
            let x = HEAD_BUTTONS_X + i as f64 * (HEAD_BUTTON_W + 6.0);
            let r = Rect::new(x, y0, x + HEAD_BUTTON_W, y1);
            let lit = self.panel == Some(p) || (p == Panel::Stereo && (ms || unlinked)) || (p == Panel::Detector && listen);
            let color = if p == Panel::Detector && listen { theme.sidechain } else { theme.accent };
            let hot = self.pointer.is_some_and(|q| r.contains(q));
            let ink = icon_button(cx.scene, theme, r, theme.panel_control, hot, lit, color);
            draw_glyph(cx.scene, d, r.center(), 20.0, ink, 1.7);
            self.head_hits[i] = r;
        }
    }

    /// Records' worth of time since the newest arrived (up to a few), so the history keeps
    /// moving between host blocks.
    fn since_fresh(&self) -> f64 {
        (self.last_fresh.elapsed().as_secs_f64() * 1000.0 / f64::from(crate::dsp::HISTORY_SLICE_MS)).min(4.0)
    }

    fn draw_history(&self, cx: &mut DrawCx<'_>, layout: &Layout, l: &Lanes) {
        let theme = cx.theme;
        let lv_y = |db: f64| l.lv0 + (-db.clamp(-LEVEL_RANGE_DB, 0.0) / LEVEL_RANGE_DB) * (l.lv1 - l.lv0);
        let g_y = |g: f64| l.gz - soft_gain(g) * GAIN_PX_PER_DB;
        // Grid: levels every 12 dB, gain ±6 dB, a line per second.
        for db in [-12.0, -24.0, -36.0] {
            cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, theme.grid_minor, None, &Line::new((0.0, lv_y(db)), (HIST_W, lv_y(db))));
        }
        for g in [6.0, -6.0] {
            cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, theme.grid_minor, None, &Line::new((0.0, g_y(g)), (HIST_W, g_y(g))));
        }
        for s in 1..6 {
            let x = s as f64 * HIST_W / 6.0;
            cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, theme.grid_minor, None, &Line::new((x, DISPLAY_TOP), (x, l.h1)));
        }
        cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, theme.line, None, &Line::new((0.0, l.sep + 0.5), (HIST_W, l.sep + 0.5)));
        cx.scene.stroke(&Stroke::new(1.2), Affine::IDENTITY, theme.grid_zero, None, &Line::new((0.0, l.gz), (HIST_W, l.gz)));
        let label = TextStyle { tabular: true, ..TextStyle::BODY.sized(10.0).weighted(600.0) };
        for (y, s, c) in [
            (lv_y(0.0) - 2.0, "0", theme.text_lo),
            (lv_y(-12.0), "−12", theme.text_lo),
            (lv_y(-24.0), "−24", theme.text_lo),
            (lv_y(-36.0), "−36", theme.text_lo),
            (g_y(6.0) - 6.0, "+6", theme.gain_up),
            (g_y(0.0) - 6.0, "0", theme.text_lo),
            (g_y(-6.0) - 6.0, "−6", theme.gain_reduction),
        ] {
            cx.text.draw(cx.scene, s, 1176.0, y, label, c, Align::Right);
        }

        // Points of two records (10 ms), fixed to the records' absolute times so they never
        // re-bucket: levels and hits at their peak, gains at their largest size. They scroll with
        // the clock between arrivals.
        let px_per_record = HIST_W / (HIST_SECONDS * 1000.0 / f64::from(crate::dsp::HISTORY_SLICE_MS));
        let total = self.history.len();
        let first_abs = self.seen.wrapping_sub(total);
        let now = self.seen as f64 + self.since_fresh();
        let mut cols: Vec<(f64, [f32; record::WIDTH])> = Vec::with_capacity(620);
        for col in first_abs.div_ceil(PER_POINT)..self.seen.div_ceil(PER_POINT) {
            let (a, b) = (col * PER_POINT, ((col + 1) * PER_POINT).min(self.seen));
            let x = HIST_W - (now - (a + b) as f64 / 2.0) * px_per_record;
            if x < -20.0 {
                continue;
            }
            let mut out = self.history[a - first_abs];
            for i in a + 1..b {
                let r = &self.history[i - first_abs];
                for (k, (o, v)) in out.iter_mut().zip(r).enumerate() {
                    let gain = (record::GAIN_A..record::WEIGHT).contains(&k);
                    *o = if gain { if v.abs() > o.abs() { *v } else { *o } } else { o.max(*v) };
                }
            }
            cols.push((x, out));
        }
        if cols.len() < 2 {
            return;
        }
        let delta = Self::on(cx.host, DELTA.id);
        let listen = Self::on(cx.host, LISTEN.id);
        let level = |r: &[f32; record::WIDTH], k: usize| f64::from(r[k].max(r[k + 1]));
        let line = |f: &dyn Fn(&[f32; record::WIDTH]) -> f64| smooth_path(&cols.iter().map(|(x, r)| Point::new(*x, f(r))).collect::<Vec<_>>());
        let closed = |mut p: BezPath, y: f64| {
            p.line_to((cols[cols.len() - 1].0, y));
            p.line_to((cols[0].0, y));
            p.close_path();
            p
        };
        // The level lane, clipped to itself.
        cx.scene.push_clip_layer(Fill::NonZero, Affine::IDENTITY, &Rect::new(0.0, DISPLAY_TOP, HIST_W, l.sep));
        let input = line(&|r| lv_y(level(r, record::IN)));
        cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.spectrum_pre.multiply_alpha(0.16), None, &closed(input.clone(), l.lv1));
        cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, theme.spectrum_pre.multiply_alpha(0.55), None, &input);
        let out_color = if listen { theme.sidechain } else { theme.curve };
        let output = if delta {
            line(&|r| {
                let (i, o) = (mite_dsp::db_to_gain(level(r, record::IN) as f32), mite_dsp::db_to_gain(level(r, record::OUT) as f32));
                lv_y(f64::from(mite_dsp::gain_to_db((i - o).abs().max(1e-6))))
            })
        } else {
            line(&|r| lv_y(level(r, record::OUT)))
        };
        let a = if delta { 0.3 } else { 0.12 };
        let fill = Gradient::new_linear((0.0, l.lv0), (0.0, l.lv1)).with_stops([out_color.multiply_alpha(a), out_color.multiply_alpha(0.01)]);
        cx.scene.fill(Fill::NonZero, Affine::IDENTITY, &fill, None, &closed(output.clone(), l.lv1));
        glow_stroke(cx, &output, 1.6, out_color, out_color, 2.4);
        cx.scene.pop_layer();

        // The gain lane: the shown band's gain, attack part solid, sustain part light.
        cx.scene.push_clip_layer(Fill::NonZero, Affine::IDENTITY, &Rect::new(0.0, l.sep, HIST_W, l.h1));
        let shown = self.shown(cx.host, layout);
        let muted = !Self::audible(cx.host, layout, shown);
        let gain = |r: &[f32; record::WIDTH], b: usize| f64::from(r[record::GAIN_A + b]);
        let weight = |r: &[f32; record::WIDTH], b: usize| f64::from(r[record::WEIGHT + b]).clamp(0.0, 1.0);
        // Other bands, quietly.
        if layout.n > 0 {
            for &b in layout.bands() {
                if b == shown || !Self::audible(cx.host, layout, b) {
                    continue;
                }
                let edge = line(&|r| g_y(gain(r, b)));
                cx.scene.stroke(&Stroke::new(1.2), Affine::IDENTITY, theme.text_lo.multiply_alpha(0.7), None, &edge);
            }
        }
        if !muted {
            let up = cols.iter().any(|(_, r)| gain(r, shown) > 0.05);
            let down = cols.iter().any(|(_, r)| gain(r, shown) < -0.05);
            for (on, color, sign) in [(up, theme.gain_up, 1.0f64), (down, theme.gain_reduction, -1.0)] {
                if !on {
                    continue;
                }
                let part = |r: &[f32; record::WIDTH]| (gain(r, shown) * sign).max(0.0) * sign;
                let light = closed(line(&|r| g_y(part(r))), l.gz);
                cx.scene.fill(Fill::NonZero, Affine::IDENTITY, color.multiply_alpha(0.28), None, &light);
                let solid = closed(line(&|r| g_y(part(r) * weight(r, shown))), l.gz);
                mite_ui::paint::glow_fill(cx, &solid, color.multiply_alpha(0.7), 2.4);
                cx.scene.stroke(&Stroke::new(1.5), Affine::IDENTITY, color, None, &line(&|r| g_y(part(r))));
            }
            // The other channel, dashed, when the two can differ.
            if Self::plain(cx.host, LINK.id) < 99.5 {
                let second = line(&|r| g_y(f64::from(r[record::GAIN_B + shown])));
                cx.scene.stroke(&Stroke::new(1.4).with_dashes(0.0, [4.0, 3.0]), Affine::IDENTITY, theme.text_hi.multiply_alpha(0.85), None, &second);
            }
        }
        cx.scene.pop_layer();
        // A tick for every hit, as tall as it was sharp.
        let mut ticks = BezPath::new();
        for (x, r) in &cols {
            let h = f64::from(r[record::HIT + shown]);
            if h > 0.0 && *x >= 0.0 {
                ticks.move_to((*x, l.sep));
                ticks.line_to((*x, l.sep + 4.0 + 10.0 * h));
            }
        }
        glow_stroke(cx, &ticks, 2.0, theme.accent, theme.accent, 2.4);
    }

    fn draw_rail(&self, cx: &mut DrawCx<'_>, layout: &Layout, l: &Lanes) {
        let theme = cx.theme;
        let (ry, rh) = (l.ry, l.rh);
        let rail = Rect::new(0.0, ry, HIST_W, ry + rh);
        cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.well, None, &rail);
        for y in [ry + 0.5, ry + rh - 0.5] {
            cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, theme.well_border, None, &Line::new((0.0, y), (HIST_W, y)));
        }
        for f in [100.0, 1000.0, 10_000.0] {
            cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, theme.grid_minor, None, &Line::new((rail_x(f), ry), (rail_x(f), ry + rh)));
        }
        let multi = layout.n > 0;
        let x = Self::crossovers(cx.host);
        let soloed = Self::soloed(cx.host, layout);
        let merging = self.merging_band(cx.host, layout);
        let now_gain = |b: usize| self.history.iter().rev().take(4).map(|r| r[record::GAIN_A + b]).fold(0.0f32, |m, g| if g.abs() > m.abs() { g } else { m });
        // Band regions: the selected one lit, the others breathing with their gain.
        if multi {
            for (p, &b) in layout.bands().iter().enumerate() {
                let r = self.region_rect(cx.host, layout, p, l);
                let muted = Self::on(cx.host, &band_id(b, B_MUTE.id));
                let audible = Self::audible(cx.host, layout, b);
                let on = b == self.shown_selected(layout);
                let breath = (f64::from(now_gain(b).abs()) / 6.0 * 0.12).min(0.12);
                let mut op = (if on { 0.22 } else { 0.10 }) + if audible { breath } else { 0.0 };
                if soloed.is_some_and(|s| s != b) {
                    op *= 0.35;
                }
                if merging == Some(b) {
                    op = 0.08;
                }
                if !muted {
                    cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.accent.multiply_alpha(op as f32), None, &r);
                } else {
                    // Muted: a 45° hatch, no fill.
                    let mut hatch = BezPath::new();
                    let mut k = r.x0 - rh;
                    while k < r.x1 {
                        hatch.move_to((k, r.y1));
                        hatch.line_to((k + rh, r.y0));
                        k += 6.0;
                    }
                    cx.scene.push_clip_layer(Fill::NonZero, Affine::IDENTITY, &r);
                    cx.scene.stroke(&Stroke::new(3.0), Affine::IDENTITY, theme.text_lo.multiply_alpha(0.3), None, &hatch);
                    cx.scene.pop_layer();
                }
                if merging == Some(b) {
                    cx.scene.stroke(&Stroke::new(1.5).with_dashes(0.0, [4.0, 3.0]), Affine::IDENTITY, theme.accent, None, &r.inset(-1.0));
                }
            }
        }
        // The input spectrum, and (several bands) what the bands make of it now.
        let spec_y = |db: f32| ry + 4.0 + f64::from((SPEC_TOP_DB - db.clamp(SPEC_BOTTOM_DB, SPEC_TOP_DB)) / (SPEC_TOP_DB - SPEC_BOTTOM_DB)) * (rh - 8.0);
        if self.spectrum.iter().any(|&v| v > SPEC_BOTTOM_DB) {
            let pts: Vec<Point> = (0..SPECTRUM_POINTS).map(|i| Point::new(rail_x(spectrum_point_freq(i)), spec_y(self.spectrum[i]))).collect();
            let curve = smooth_path(&pts);
            let mut fill = curve.clone();
            fill.line_to((pts[pts.len() - 1].x, ry + rh));
            fill.line_to((pts[0].x, ry + rh));
            fill.close_path();
            cx.scene.push_clip_layer(Fill::NonZero, Affine::IDENTITY, &rail);
            cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.spectrum_pre.multiply_alpha(if multi { 0.2 } else { 0.12 }), None, &fill);
            cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, theme.spectrum_pre.multiply_alpha(0.6), None, &curve);
            if multi {
                let gains: Vec<(usize, f64)> = layout.bands().iter().map(|&b| (b, if Self::audible(cx.host, layout, b) { 10f64.powf(f64::from(now_gain(b)) / 20.0) } else { 0.0 })).collect();
                let out: Vec<Point> = (0..SPECTRUM_POINTS)
                    .map(|i| {
                        let w = layout.weights(&x, spectrum_point_freq(i));
                        let a: f64 = gains.iter().map(|&(b, g)| w[b] * g).sum();
                        Point::new(pts[i].x, spec_y(self.spectrum[i] + 20.0 * a.max(1e-3).log10() as f32))
                    })
                    .collect();
                glow_stroke(cx, &smooth_path(&out), 1.6, theme.curve, theme.curve, 2.4);
            }
            cx.scene.pop_layer();
        }
        let label = TextStyle { tabular: true, ..TextStyle::BODY.sized(10.0).weighted(600.0) };
        if multi {
            for (f, s) in [(100.0, "100"), (1000.0, "1k"), (10_000.0, "10k")] {
                cx.text.draw(cx.scene, s, rail_x(f) + 4.0, ry + rh - 15.0, label, theme.text_lo, Align::Left);
            }
        }
        // Crossovers: a line and a grip each, the frequency above it.
        let drag_slot = match self.drag {
            Some(Drag::Cross { slot, .. }) => Some(slot),
            _ => None,
        };
        for p in 0..layout.n {
            let slot = layout.crossovers[p];
            let mut c = Self::grip_center(cx.host, slot, l);
            let merge = match self.drag {
                Some(Drag::Cross { slot: k, merge, .. }) if k == slot => merge,
                _ => None,
            };
            // Merging: the dragged crossover's ghost sits where it lands (on its neighbour, or
            // off the end), and only the neighbour keeps a label.
            match merge {
                Some(Merge::Down(_)) => c.x = rail_x(f64::from(x[layout.crossovers[p - 1]].unwrap_or(FREQ_MIN))),
                Some(Merge::Up) => c.x = rail_x(f64::from(x[layout.crossovers[p + 1]].unwrap_or(FREQ_MAX))),
                Some(Merge::OffLow) => c.x = RAIL_X0,
                Some(Merge::OffHigh) => c.x = RAIL_X0 + RAIL_W,
                None => {}
            }
            let ghost = merge.is_some();
            let hot = matches!(self.hot, Some(Hot::Grip(k)) if k == slot) || drag_slot == Some(slot);
            let line = Line::new((c.x, ry), (c.x, ry + rh));
            if ghost {
                cx.scene.stroke(&Stroke::new(1.5).with_dashes(0.0, [4.0, 3.0]), Affine::IDENTITY, theme.accent, None, &line);
            } else {
                cx.scene.stroke(&Stroke::new(1.5), Affine::IDENTITY, theme.accent.multiply_alpha(0.75), None, &line);
            }
            self.draw_grip(cx, c, ghost, hot, 1.0);
            if ghost {
                continue;
            }
            let f = f64::from(x[slot].unwrap_or(FREQ_MIN));
            cx.text.draw(cx.scene, &fmt_freq(f), c.x - 10.0, ry + 5.0, TextStyle { tabular: true, ..TextStyle::BODY.sized(10.0).weighted(700.0) }, if hot { theme.accent_text } else { theme.text_mid }, Align::Right);
        }
        // Pulling a new one in: its line and frequency where it would land.
        if let Some(Drag::Create { start, now, .. }) = self.drag {
            if now.distance(start) > 6.0 {
                let f = rail_f(now.x.clamp(RAIL_X0, RAIL_X0 + RAIL_W));
                let cxx = rail_x(f);
                cx.scene.stroke(&Stroke::new(1.5).with_dashes(0.0, [4.0, 3.0]), Affine::IDENTITY, theme.accent, None, &Line::new((cxx, ry), (cxx, ry + rh)));
                self.draw_grip(cx, Point::new(cxx, ry + rh / 2.0), true, true, 1.0);
                cx.text.draw(cx.scene, &fmt_freq(f), cxx - 10.0, ry + 5.0, TextStyle { tabular: true, ..TextStyle::BODY.sized(10.0).weighted(700.0) }, theme.accent_text, Align::Right);
            }
        }
        // Ghost grips at both ends while there is room for another band.
        if layout.n < CROSSOVERS && self.drag.is_none() {
            for (i, c) in Self::ghost_centers(l).into_iter().enumerate() {
                let hot = matches!(self.hot, Some(Hot::Ghost(low)) if low == (i == 0));
                self.draw_grip(cx, c, true, hot, if hot { 1.0 } else { 0.55 });
            }
        }
        // Band numbers, with S or M tags.
        if multi {
            for (p, &b) in layout.bands().iter().enumerate() {
                if merging == Some(b) {
                    continue;
                }
                let r = self.region_rect(cx.host, layout, p, l);
                let on = b == self.shown_selected(layout);
                let solo = Self::on(cx.host, &band_id(b, B_SOLO.id));
                let muted = Self::on(cx.host, &band_id(b, B_MUTE.id));
                let alpha = if soloed.is_some_and(|s| s != b) { 0.45 } else { 1.0 };
                let c = Point::new(r.x0 + if p == 0 { 34.0 } else { 18.0 } + 9.0, ry + 15.0);
                self.draw_badge(cx, c, 9.0, p + 1, on, solo, alpha);
                if solo || muted {
                    let tag = Rect::new(c.x + 13.0, c.y - 9.0, c.x + 31.0, c.y + 9.0);
                    let color = if solo { theme.needle } else { theme.text_hi };
                    cx.scene.fill(Fill::NonZero, Affine::IDENTITY, color.multiply_alpha(alpha), None, &RoundedRect::from_rect(tag, 9.0));
                    cx.text.draw(cx.scene, if solo { "S" } else { "M" }, tag.center().x, tag.y0 + 3.0, TextStyle::LABEL.sized(10.0).weighted(800.0), theme.display, Align::Center);
                }
            }
        }
    }

    /// The band that a crossover drag would take away on release.
    fn merging_band(&self, host: &dyn ParamHost, layout: &Layout) -> Option<usize> {
        let Some(Drag::Cross { slot, merge: Some(m), .. }) = self.drag else { return None };
        let p = layout.crossovers[..layout.n].iter().position(|&k| k == slot)?;
        let _ = host;
        Some(match m {
            Merge::Down(_) => layout.owners[p],
            Merge::Up => slot + 1,
            Merge::OffLow => layout.owners[0],
            Merge::OffHigh => slot + 1,
        })
    }

    fn draw_grip(&self, cx: &mut DrawCx<'_>, c: Point, ghost: bool, hot: bool, alpha: f32) {
        let theme = cx.theme;
        let r = RoundedRect::from_rect(Rect::from_center_size(c, GRIP), 7.0);
        let fill = if ghost { theme.accent.multiply_alpha(0.12) } else if hot { theme.accent } else { theme.display };
        cx.scene.fill(Fill::NonZero, Affine::IDENTITY, fill.multiply_alpha(alpha), None, &r);
        if ghost {
            cx.scene.stroke(&Stroke::new(1.5).with_dashes(0.0, [3.0, 2.0]), Affine::IDENTITY, theme.accent.multiply_alpha(alpha), None, &r);
        } else {
            glow_stroke(cx, &r, 1.5, theme.accent.multiply_alpha(alpha), theme.accent, if hot { 5.0 } else { 3.0 });
        }
        let ink = if hot && !ghost { theme.display } else { theme.accent };
        for dy in [-4.5, 0.0, 4.5] {
            cx.scene.stroke(&Stroke::new(1.5).with_caps(Cap::Round), Affine::IDENTITY, ink.multiply_alpha(alpha), None, &Line::new((c.x - 2.0, c.y + dy), (c.x + 2.0, c.y + dy)));
        }
    }

    /// A band's number in its circle: lit when selected, ringed white when soloed.
    #[allow(clippy::too_many_arguments)]
    fn draw_badge(&self, cx: &mut DrawCx<'_>, c: Point, r: f64, n: usize, on: bool, solo: bool, alpha: f32) {
        let theme = cx.theme;
        let fill = if on { theme.accent } else { theme.accent.multiply_alpha(0.25) };
        if on {
            glow_fill(cx, &Circle::new(c, r), fill.multiply_alpha(alpha), 4.0);
        } else {
            cx.scene.fill(Fill::NonZero, Affine::IDENTITY, fill.multiply_alpha(alpha), None, &Circle::new(c, r));
        }
        if solo {
            glow_stroke(cx, &Circle::new(c, r - 1.0), 2.0, theme.needle.multiply_alpha(alpha), theme.needle, 4.0);
        }
        let ink = if on { theme.display } else { theme.accent_text };
        cx.text.draw(cx.scene, &n.to_string(), c.x, c.y - r * 0.62, TextStyle::LABEL.sized((r * 1.1) as f32).weighted(800.0), ink.multiply_alpha(alpha), Align::Center);
    }

    fn draw_funnels(&self, cx: &mut DrawCx<'_>, layout: &Layout, l: &Lanes, cards: &[Card]) {
        if layout.n == 0 {
            return;
        }
        let theme = cx.theme;
        for (p, card) in cards.iter().enumerate() {
            let r = self.region_rect(cx.host, layout, p, l);
            let (rx0, rx1) = (r.x0 + 2.0, r.x1 - 2.0);
            let (cx0, cx1) = (card.rect.x0 + 2.0, card.rect.x1 - 2.0);
            let (y0, y1) = (l.fy, l.cy);
            if y1 - y0 < 2.0 {
                continue;
            }
            let ym = (y0 + y1) / 2.0;
            let mut f = BezPath::new();
            f.move_to((rx0, y0));
            f.curve_to((rx0, ym), (cx0, ym), (cx0, y1));
            f.line_to((cx1, y1));
            f.curve_to((cx1, ym), (rx1, ym), (rx1, y0));
            f.close_path();
            let on = card.band == self.shown_selected(layout);
            cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.accent.multiply_alpha(if on { 0.16 } else { 0.05 }), None, &f);
            if on {
                cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, theme.accent.multiply_alpha(0.5), None, &f);
            }
        }
    }

    fn draw_cards(&self, cx: &mut DrawCx<'_>, layout: &Layout, cards: &[Card]) {
        let theme = cx.theme;
        let soloed = Self::soloed(cx.host, layout);
        let merging = self.merging_band(cx.host, layout);
        for (p, card) in cards.iter().enumerate() {
            let b = card.band;
            let band = Self::band(cx.host, b);
            let r = RoundedRect::from_rect(card.rect, 12.0);
            let alpha: f32 = if soloed.is_some_and(|s| s != b) { 0.35 } else if band.mute { 0.55 } else if merging == Some(b) { 0.6 } else { 1.0 };
            let multi = layout.n > 0;
            let border = if band.solo { theme.needle } else if card.big && multi { theme.accent } else { theme.well_border };
            cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.window.multiply_alpha(alpha), None, &r);
            cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.control.multiply_alpha(0.55 * alpha), None, &r);
            if merging == Some(b) {
                cx.scene.stroke(&Stroke::new(1.5).with_dashes(0.0, [4.0, 3.0]), Affine::IDENTITY, theme.accent, None, &r);
            } else if band.solo || (card.big && multi) {
                glow_stroke(cx, &r, if band.solo { 2.0 } else { 1.0 }, border.multiply_alpha(alpha), border, 6.0);
            } else {
                cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, border, None, &r);
            }
            if band.mute {
                let mut hatch = BezPath::new();
                let mut k = card.rect.x0 - card.rect.height();
                while k < card.rect.x1 {
                    hatch.move_to((k, card.rect.y1));
                    hatch.line_to((k + card.rect.height(), card.rect.y0));
                    k += 6.0;
                }
                cx.scene.push_clip_layer(Fill::NonZero, Affine::IDENTITY, &r);
                cx.scene.stroke(&Stroke::new(3.0), Affine::IDENTITY, theme.text_lo.multiply_alpha(0.22), None, &hatch);
                cx.scene.pop_layer();
            }
            if multi {
                // Header: number, the band's range (wide card), solo, mute, remove.
                let head = card.rect.y0 + 10.0;
                self.draw_badge(cx, Point::new(card.rect.x0 + 21.0, head + 16.0), 11.0, p + 1, card.big, band.solo, alpha);
                if card.big {
                    let x = Self::crossovers(cx.host);
                    if let Some((lo, hi)) = layout.region(b, &x) {
                        let text = format!("{} – {}", fmt_freq(f64::from(lo)), fmt_freq(f64::from(hi)));
                        cx.text.draw(cx.scene, &text, card.rect.x0 + 40.0, head + 9.0, TextStyle { tabular: true, ..TextStyle::BODY.weighted(600.0) }, theme.text_mid, Align::Left);
                    }
                }
                let hot = |q: Rect| self.pointer.is_some_and(|pt| q.contains(pt));
                if let Some(s) = card.solo {
                    let ink = icon_button(cx.scene, theme, s, theme.panel_control, hot(s), band.solo, theme.needle);
                    cx.text.draw(cx.scene, "S", s.center().x, s.y0 + 8.0, TextStyle::BODY.sized(13.0).weighted(800.0), ink, Align::Center);
                }
                if let Some(m) = card.mute {
                    let ink = icon_button(cx.scene, theme, m, theme.panel_control, hot(m), band.mute, theme.text_hi);
                    draw_glyph(cx.scene, glyph::MUTE, m.center(), 18.0, ink, 1.7);
                }
                if let Some(bin) = card.bin {
                    let ink = icon_button(cx.scene, theme, bin, theme.panel_control, hot(bin), false, theme.danger);
                    draw_glyph(cx.scene, glyph::BIN, bin.center(), 18.0, ink, 1.7);
                }
            }
            self.draw_scope(cx, card.scope, &band, card.big, card.big);
        }
        for k in &self.card_knobs {
            k.draw(cx);
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
        cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, theme.line, None, &Line::new((HIST_W + 0.5, DISPLAY_TOP), (HIST_W + 0.5, HELP_TOP)));
        for x in [1196.0, 1238.0] {
            panel(cx.scene, RoundedRect::new(x, METER_TOP, x + 26.0, METER_BOTTOM, 3.0), theme.meter_well, theme.well_border);
        }
        let seg = Stroke::new(8.0).with_dashes(0.0, [2.0, 2.0]);
        let (top, bottom) = (METER_TOP + 4.0, METER_BOTTOM - 4.0);
        let span = bottom - top;
        let level_y = |db: f32| bottom - (f64::from(db.clamp(-60.0, 0.0) + 60.0) / 60.0 * span / 4.0).round() * 4.0;
        let listen = Self::on(cx.host, LISTEN.id);
        for (i, x) in [1203.0, 1213.0, 1245.0, 1255.0].into_iter().enumerate() {
            cx.scene.stroke(&seg, Affine::IDENTITY, theme.meter_off, None, &Line::new((x, top), (x, bottom)));
            let line = Line::new((x, bottom), (x, level_y(self.meters[i])));
            let color = if i < 2 { theme.spectrum_pre } else if listen { theme.sidechain } else { theme.curve };
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

    fn draw_states(&self, cx: &mut DrawCx<'_>) {
        let theme = cx.theme;
        let status = if Self::on(cx.host, LISTEN.id) {
            let frame = Rect::new(1.0, DISPLAY_TOP + 1.0, WIDTH - 1.0, HELP_TOP - 1.0);
            glow_stroke(cx, &frame, 2.0, theme.sidechain, theme.sidechain, 12.0);
            Some((glyph::LISTEN, theme.sidechain))
        } else if Self::on(cx.host, DELTA.id) {
            Some((glyph::DELTA, theme.accent))
        } else {
            None
        };
        if let Some((d, color)) = status {
            let r = RoundedRect::from_rect(Rect::new(14.0, DISPLAY_TOP + 12.0, 50.0, DISPLAY_TOP + 48.0), 10.0);
            cx.scene.fill(Fill::NonZero, Affine::IDENTITY, color.multiply_alpha(0.18), None, &r);
            glow_stroke(cx, &r, 1.5, color, color, 9.0);
            draw_glyph(cx.scene, d, r.center(), 20.0, color, 2.0);
        }
    }

    // ---- panels -----------------------------------------------------------------------------

    fn build_panel_knobs(&mut self) {
        let Some(p) = self.panel else { return };
        if self.knobs_for == Some(p) {
            return;
        }
        self.panel_knobs = match p {
            Panel::Detector => {
                let r = DETECTOR_SHEET;
                let y = r.center().y - 6.0;
                vec![
                    Knob::new(FLOOR, "FLOOR", KnobKind::Unipolar, Point::new(r.x0 + 620.0, y), 62.0),
                    Knob::new(LOOKAHEAD, "LOOKAHEAD", KnobKind::Unipolar, Point::new(r.x0 + 730.0, y), 62.0),
                ]
            }
            Panel::Stereo => vec![Knob::new(LINK, "LINK", KnobKind::Unipolar, Point::new(STEREO_POP.x1 - 76.0, STEREO_POP.center().y - 6.0), 62.0)],
        };
        self.knobs_for = Some(p);
    }

    /// The detector filter's display area (one band only).
    fn filter_area() -> Rect {
        Rect::new(DETECTOR_SHEET.x0 + 164.0, DETECTOR_SHEET.y0 + 15.0, DETECTOR_SHEET.x0 + 544.0, DETECTOR_SHEET.y0 + 203.0)
    }

    fn filter_x(a: Rect, f: f64) -> f64 {
        a.x0 + 8.0 + (a.width() - 16.0) * (f / f64::from(FREQ_MIN)).ln() / 1000f64.ln()
    }

    fn filter_f(a: Rect, x: f64) -> f64 {
        f64::from(FREQ_MIN) * 1000f64.powf(((x - a.x0 - 8.0) / (a.width() - 16.0)).clamp(0.0, 1.0))
    }

    /// The detector filter's response at `f`, dB (two Butterworth sections each way: LR4).
    fn filter_db(host: &dyn ParamHost, f: f64) -> f64 {
        let (hp, lp) = (f64::from(Self::plain(host, DET_HP.id)), f64::from(Self::plain(host, DET_LP.id)));
        let mut db = 0.0;
        if hp > f64::from(mite_dsp::shape::DETECTOR_HP_OPEN) {
            let r4 = (f / hp).powi(4);
            db += 20.0 * (r4 / (1.0 + r4)).log10();
        }
        if lp < f64::from(mite_dsp::shape::DETECTOR_LP_OPEN) {
            let r4 = (lp / f).powi(4);
            db += 20.0 * (r4 / (1.0 + r4)).log10();
        }
        db
    }

    fn filter_node(host: &dyn ParamHost, hp: bool) -> Point {
        let a = Self::filter_area();
        let f = if hp { Self::plain(host, DET_HP.id) } else { Self::plain(host, DET_LP.id) };
        Point::new(Self::filter_x(a, f64::from(f)), a.y0 + 70.0 + 6.0 * 3.2)
    }

    fn draw_panel(&mut self, cx: &mut DrawCx<'_>, layout: &Layout) {
        let Some(p) = self.panel else {
            self.hits = PanelHits::default();
            return;
        };
        self.build_panel_knobs();
        let theme = cx.theme;
        let rect = if p == Panel::Detector { DETECTOR_SHEET } else { STEREO_POP };
        if theme.shadows {
            cx.scene.draw_blurred_rounded_rect(Affine::translate((0.0, 14.0)), rect, Color::from_rgba8(0, 0, 0, 140), 14.0, 20.0);
        }
        glass_panel(cx, RoundedRect::from_rect(rect, 14.0), theme.blur_panel);
        let mut hits = PanelHits { rect, ..PanelHits::default() };
        let hot = |r: Rect| self.pointer.is_some_and(|q| r.contains(q));
        match p {
            Panel::Detector => {
                draw_glyph(cx.scene, glyph::SLIDERS, Point::new(rect.x0 + 26.0, rect.y0 + 28.0), 20.0, theme.accent, 1.8);
                cx.text.draw(cx.scene, "Detector", rect.x0 + 42.0, rect.y0 + 19.0, TextStyle::TITLE, theme.text_hi, Align::Left);
                let listen = Rect::new(rect.x0 + 16.0, rect.y0 + 58.0, rect.x0 + 68.0, rect.y0 + 98.0);
                let on = Self::on(cx.host, LISTEN.id);
                let ink = icon_button(cx.scene, theme, listen, theme.panel_control, hot(listen), on, theme.sidechain);
                draw_glyph(cx.scene, glyph::LISTEN, listen.center(), 22.0, ink, 1.8);
                hits.listen = Some(listen);
                let close = Rect::new(rect.x1 - 16.0 - 36.0, rect.y0 + 14.0, rect.x1 - 16.0, rect.y0 + 50.0);
                self.draw_close(cx, close);
                hits.close = close;
                // The detector filter (one band only: with several, the crossovers do its job).
                if layout.n == 0 {
                    let a = Self::filter_area();
                    panel(cx.scene, RoundedRect::from_rect(a, 8.0), theme.well, theme.well_border);
                    for f in [100.0, 1000.0, 10_000.0] {
                        cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, theme.grid_minor, None, &Line::new((Self::filter_x(a, f), a.y0), (Self::filter_x(a, f), a.y1)));
                    }
                    let gy = |g: f64| a.y0 + 70.0 - g * 3.2;
                    cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, theme.grid_minor, None, &Line::new((a.x0, gy(0.0)), (a.x1, gy(0.0))));
                    let spec_y = |db: f32| a.y0 + 80.0 + f64::from(-db - 12.0) / 60.0 * 96.0;
                    cx.scene.push_clip_layer(Fill::NonZero, Affine::IDENTITY, &RoundedRect::from_rect(a, 8.0));
                    if self.spectrum.iter().any(|&v| v > SPEC_BOTTOM_DB) {
                        let pts: Vec<Point> = (0..SPECTRUM_POINTS).map(|i| Point::new(Self::filter_x(a, spectrum_point_freq(i)), spec_y(self.spectrum[i]).min(a.y1))).collect();
                        let heard: Vec<Point> = (0..SPECTRUM_POINTS)
                            .map(|i| Point::new(pts[i].x, spec_y(self.spectrum[i] + Self::filter_db(cx.host, spectrum_point_freq(i)).max(-60.0) as f32).min(a.y1)))
                            .collect();
                        for (p, color, alpha) in [(&pts, theme.spectrum_pre, 0.14), (&heard, theme.sidechain, 0.22)] {
                            let mut fill = smooth_path(p);
                            fill.line_to((a.x1, a.y1));
                            fill.line_to((a.x0, a.y1));
                            fill.close_path();
                            cx.scene.fill(Fill::NonZero, Affine::IDENTITY, color.multiply_alpha(alpha), None, &fill);
                        }
                    }
                    let curve: Vec<Point> = (0..=160)
                        .map(|i| {
                            let f = f64::from(FREQ_MIN) * 1000f64.powf(f64::from(i) / 160.0);
                            Point::new(Self::filter_x(a, f), gy(Self::filter_db(cx.host, f).max(-24.0)))
                        })
                        .collect();
                    glow_stroke(cx, &polyline(&curve), 2.0, theme.curve, theme.curve, 4.0);
                    cx.scene.pop_layer();
                    for hp in [true, false] {
                        let c = Self::filter_node(cx.host, hp);
                        glow_fill(cx, &Circle::new(c, 7.0), theme.accent, 5.0);
                        cx.scene.stroke(&Stroke::new(2.0), Affine::IDENTITY, theme.needle, None, &Circle::new(c, 6.0));
                    }
                    let label = TextStyle { tabular: true, ..TextStyle::BODY.sized(10.0).weighted(600.0) };
                    for (f, s) in [(100.0, "100"), (1000.0, "1k"), (10_000.0, "10k")] {
                        cx.text.draw(cx.scene, s, Self::filter_x(a, f) + 3.0, a.y1 - 18.0, label, theme.text_lo, Align::Left);
                    }
                    hits.filter = Some(a);
                }
            }
            Panel::Stereo => {
                let ms = Self::index(cx.host, STEREO.id) == 1;
                for (i, (d, v)) in [(glyph::LR, 0.0), (glyph::MS, 1.0)].into_iter().enumerate() {
                    let r = Rect::new(rect.x0 + 12.0, rect.y0 + 12.0 + i as f64 * 44.0, rect.x0 + 64.0, rect.y0 + 50.0 + i as f64 * 44.0);
                    let on = ms == (v == 1.0);
                    let ink = icon_button(cx.scene, theme, r, theme.panel_control, hot(r), on, theme.accent);
                    draw_glyph(cx.scene, d, r.center(), 22.0, ink, 1.7);
                    hits.modes.push((r, v));
                }
            }
        }
        for k in &self.panel_knobs {
            k.draw(cx);
        }
        self.hits = hits;
    }

    fn draw_close(&self, cx: &mut DrawCx<'_>, close: Rect) {
        let theme = cx.theme;
        let hot = self.pointer.is_some_and(|q| close.contains(q));
        let ink = icon_button(cx.scene, theme, close, theme.panel_control, hot, false, theme.accent);
        let k = close.center();
        let mut x = BezPath::new();
        x.move_to((k.x - 3.0, k.y - 3.0));
        x.line_to((k.x + 3.0, k.y + 3.0));
        x.move_to((k.x + 3.0, k.y - 3.0));
        x.line_to((k.x - 3.0, k.y + 3.0));
        cx.scene.stroke(&Stroke::new(1.6).with_caps(Cap::Round), Affine::IDENTITY, ink, None, &x);
    }

    fn draw_footer(&mut self, cx: &mut DrawCx<'_>, layout: &Layout) {
        let theme = cx.theme;
        self.footer.draw_frame(cx);
        let top = FOOTER_TOP;
        let mut x = 16.0;
        self.lr4_hit = None;
        if layout.n > 0 {
            let r = Rect::new(x, top + 9.0, x + 110.0, top + 47.0);
            let hot = self.pointer.is_some_and(|p| r.contains(p));
            draw_dropdown_button(cx, r, "LR4", false, hot, |scene, at| draw_glyph(scene, glyph::LR4, at + (8.0, 6.0), 22.0, theme.text_mid, 1.7));
            self.lr4_hit = Some(r);
            x += 126.0;
        }
        let sample_rate = f64::from(self.shared.sample_rate.load()).max(1.0);
        self.footer.draw_latency(cx, x, f64::from(self.shared.latency.load()) / sample_rate * 1000.0);
        self.footer.draw_controls(cx);
    }

    fn draw_menu(&self, cx: &mut DrawCx<'_>) {
        let Some(menu) = &self.menu else { return };
        let theme = cx.theme;
        menu.list.draw(cx, theme.accent, |scene, i, at, ink| {
            let ink = if i == 7 { theme.danger } else { ink };
            draw_glyph(scene, MENU_GLYPHS[i], at + (11.0, 6.0), 20.0, ink, 1.6);
        });
    }

    // ---- help -------------------------------------------------------------------------------

    fn help_text(&self, host: &dyn ParamHost) -> Help {
        let layout = Self::layout(host);
        let x = Self::crossovers(host);
        let band_name = |b: usize| format!("Band {}", layout.position(b).map_or(1, |p| p + 1));
        let range = |b: usize| layout.region(b, &x).map_or(String::new(), |(lo, hi)| format!("{} – {}", fmt_freq(f64::from(lo)), fmt_freq(f64::from(hi))));
        match self.drag {
            Some(Drag::Create { start, now, .. }) if now.distance(start) > 6.0 => {
                if layout.n >= CROSSOVERS {
                    return Help::default().text("Five bands is the limit.");
                }
                let f = rail_f(now.x.clamp(RAIL_X0, RAIL_X0 + RAIL_W));
                return Help::default().text("New crossover ").slot(fmt_freq(f), "20.0 kHz").text(" — release to split the band there; both sides keep its settings. Esc cancels.");
            }
            Some(Drag::Cross { slot, merge, .. }) => {
                let f = f64::from(Self::plain(host, &cross_id(slot, X_FREQ.id)));
                if let Some(b) = merge.and_then(|_| self.merging_band(host, &layout)) {
                    return Help::default().text("Merging ").slot(fmt_freq(f), "20.0 kHz").text(format!(" — release to remove {}; its neighbour grows and keeps its settings", band_name(b).to_lowercase()));
                }
                return Help::default().text("Crossover ").slot(fmt_freq(f), "20.0 kHz").text(" — drag onto a neighbour or off the end to merge; Shift for fine steps");
            }
            _ => {}
        }
        if let Some(help) = self.session.help() {
            return help;
        }
        if let Some(help) = self.footer.help(host) {
            return help;
        }
        let p = self.pointer.unwrap_or(Point::ZERO);
        if self.lr4_hit.is_some_and(|r| r.contains(p)) {
            return Help::default().text("Crossover · LR4 — 24 dB/oct, minimum phase; the bands always add up flat");
        }
        for (i, words) in ["Stereo — L/R or M/S, and how tightly the two sides' detectors are linked", "Detector — what the detector hears, Listen, the floor and lookahead"].into_iter().enumerate() {
            if self.head_hits[i].contains(p) {
                return Help::default().text(words);
            }
        }
        if Self::on(host, LISTEN.id) && self.panel.is_none() {
            return Help::default().text("Listening to the detector — you hear what it hears, not the result. Click the headphones in Detector again to stop.");
        }
        if self.panel.is_some() && self.hits.rect.contains(p) {
            if let Some(k) = self.panel_knobs.iter().find(|k| k.bounds().contains(p)) {
                let v = Self::plain(host, &k.id);
                let what = match k.label {
                    "FLOOR" => " — below it a quiet tail is never lifted out of the noise",
                    "LOOKAHEAD" => " — the gain starts early, so a boosted attack keeps its leading edge (adds latency)",
                    _ => " — how tightly the two sides move together",
                };
                return Help::default().text(format!("{} · ", k.label)).slot(k.spec.format(v), k.spec.format(k.spec.max)).text(what);
            }
            if self.hits.listen.is_some_and(|r| r.contains(p)) {
                return Help::default().text("Listen — hear what the detector hears instead of the result");
            }
            if self.hits.filter.is_some_and(|r| r.contains(p)) {
                let (hp, lp) = (Self::plain(host, DET_HP.id), Self::plain(host, DET_LP.id));
                return Help::default().text("Detector hears ").slot(fmt_freq(f64::from(hp)), "2000 Hz").text(" – ").slot(fmt_freq(f64::from(lp)), "20 kHz").text(" — drag the nodes; the hits outside no longer trigger it");
            }
            return Help::default();
        }
        if let Some(k) = self.card_knobs.iter().find(|k| k.bounds().contains(p)) {
            let v = Self::plain(host, &k.id);
            return Help::default().text(format!("{} · ", k.label)).slot(k.spec.format(v), k.spec.format(k.spec.min)).text(" — drag; double-click resets");
        }
        let last = self.history.back();
        let now = |k: usize| last.map_or(-150.0, |r| r[k].max(r[k + 1]));
        let shown = self.shown(host, &layout);
        let gain_now = last.map_or(0.0, |r| r[record::GAIN_A + shown]);
        match self.hot {
            Some(Hot::Grip(slot)) => {
                let f = f64::from(Self::plain(host, &cross_id(slot, X_FREQ.id)));
                return Help::default().text("Crossover ").slot(fmt_freq(f), "20.0 kHz").text(" — drag sideways; onto a neighbour or off the end merges; double-click merges");
            }
            Some(Hot::Ghost(_)) => return Help::default().text("Pull a crossover in from here to split the sound into bands"),
            Some(Hot::Scope(b, part)) => {
                let s = Self::band(host, b);
                return match part {
                    Part::Attack => Help::default().text("Attack ").slot(format!("{} dB", fmt_db(f64::from(s.attack_db))), "+18.0 dB").text(" — drag up or down; how much louder or softer each hit's start is"),
                    Part::Sustain => Help::default().text("Sustain ").slot(format!("{} dB", fmt_db(f64::from(s.sustain_db))), "+18.0 dB").text(" — drag up or down; everything after the hit"),
                    Part::Time => Help::default().text("Transient time ").slot(B_TIME.format(s.transient_ms), "200 ms").text(" — drag sideways; where attack hands over to sustain. Alt-drag for the handover's width"),
                    Part::Transition => Help::default().text("Transition ").slot(format!("{:.0} %", s.transition * 100.0), "200 %").text(" — Alt-drag sideways; how gently attack hands over to sustain"),
                };
            }
            Some(Hot::Region(b)) | Some(Hot::Card(b)) if layout.n > 0 => {
                let s = Self::band(host, b);
                return Help::default()
                    .slot(band_name(b), "Band 5")
                    .text(" · ")
                    .slot(range(b), "20.0 kHz – 20.0 kHz")
                    .text(" · attack ")
                    .slot(fmt_db(f64::from(s.attack_db)), "+18.0")
                    .text(" · sustain ")
                    .slot(fmt_db(f64::from(s.sustain_db)), "+18.0")
                    .text(" — click to edit it; double-click the strip to split; right-click for more");
            }
            _ => {}
        }
        if Self::on(host, DELTA.id) {
            return Help::default().text("Delta — you hear only what Shape adds or removes");
        }
        let neutral = layout.bands().iter().all(|&b| {
            let s = Self::band(host, b);
            s.attack_db.abs() < 0.05 && s.sustain_db.abs() < 0.05
        });
        if neutral && layout.n == 0 {
            return Help::default().text("Attack and sustain are 0 dB, so Shape passes audio unchanged. Ticks mark the hits it detects; drag the scope up to sharpen them.");
        }
        if layout.n > 0 {
            let s = Self::band(host, shown);
            return Help::default()
                .slot(band_name(shown), "Band 5")
                .text(" · ")
                .slot(range(shown), "20.0 kHz – 20.0 kHz")
                .text(" · attack ")
                .slot(fmt_db(f64::from(s.attack_db)), "+18.0")
                .text(" · sustain ")
                .slot(fmt_db(f64::from(s.sustain_db)), "+18.0")
                .text(" — drag a crossover sideways; double-click the strip to split; click a band to edit it");
        }
        Help::default()
            .text("IN ")
            .slot(fmt_db(f64::from(now(record::IN))), "−60.0")
            .text(" · OUT ")
            .slot(fmt_db(f64::from(now(record::OUT))), "−60.0")
            .text(" · gain ")
            .slot(fmt_db(f64::from(gain_now)), "+18.0")
            .text(" — drag the scope's first part for attack and its tail for sustain; pull a crossover in from either end of the strip to split the sound into bands")
    }

    // ---- events -----------------------------------------------------------------------------

    fn open_band_menu(&mut self, band: usize, at: Point, host: &dyn ParamHost) {
        let layout = Self::layout(host);
        let Some(p) = layout.position(band) else { return };
        let mut shown = vec![0, 1, 2];
        if self.clipboard.is_some() {
            shown.push(3);
        }
        shown.push(4);
        if p > 0 {
            shown.push(5);
        }
        if p < layout.n {
            shown.push(6);
        }
        if layout.n > 0 {
            shown.push(7);
        }
        let anchor = Rect::new(at.x, at.y, at.x + 236.0, at.y + 1.0);
        let list = Dropdown::new(anchor, MENU_ITEMS, usize::MAX, (DISPLAY_TOP, FOOTER_TOP)).with_shown(shown).with_min_width(236.0);
        self.selected = band;
        self.menu = Some(Menu { band, list });
    }

    fn menu_event(&mut self, event: &UiEvent, host: &dyn ParamHost) -> bool {
        let Some(menu) = &mut self.menu else { return false };
        if let UiEvent::PointerMove { pos, .. } = *event {
            self.pointer = Some(pos);
        }
        let band = menu.band;
        match menu.list.event(event) {
            DropdownEvent::Open => {}
            DropdownEvent::Picked(i) => {
                self.menu = None;
                match i {
                    0 => self.toggle_solo(host, band),
                    1 => {
                        let on = Self::on(host, &band_id(band, B_MUTE.id));
                        Self::set(host, &band_id(band, B_MUTE.id), if on { 0.0 } else { 1.0 });
                    }
                    2 => self.clipboard = Some(Self::band_values(host, band)),
                    3 => {
                        if let Some(v) = self.clipboard {
                            Self::write_band(host, band, &v);
                        }
                    }
                    4 => Self::reset_band(host, band),
                    5 => self.merge(host, band, false),
                    6 => self.merge(host, band, true),
                    _ => self.remove_band(host, band),
                }
            }
            DropdownEvent::Closed => self.menu = None,
        }
        true
    }

    fn toggle_solo(&mut self, host: &dyn ParamHost, band: usize) {
        let on = Self::on(host, &band_id(band, B_SOLO.id));
        for b in 0..BANDS {
            Self::set(host, &band_id(b, B_SOLO.id), if b == band && !on { 1.0 } else { 0.0 });
        }
    }

    /// Whether a gesture is under way (a drag becomes one undo step when it ends).
    fn busy(&self) -> bool {
        self.drag.is_some() || self.panel_knobs.iter().any(Knob::is_dragging) || self.card_knobs.iter().any(Knob::is_dragging) || self.footer.busy()
    }

    /// Makes the cards' dials for the bands in use and the selection, and moves them where the
    /// cards are now.
    fn sync_knobs(&mut self, host: &dyn ParamHost) {
        let layout = Self::layout(host);
        let cards = self.cards(&layout, &lanes(self.grow));
        let key = (layout.bands().to_vec(), self.shown_selected(&layout));
        if self.card_key.as_ref() != Some(&key) && !self.card_knobs.iter().any(Knob::is_dragging) {
            self.card_knobs.clear();
            for card in &cards {
                let b = card.band;
                if card.big {
                    for (k, spec) in BAND_SPECS.iter().enumerate() {
                        let kind = if matches!(k, 0 | 1 | 5) { KnobKind::Bipolar } else { KnobKind::Unipolar };
                        let mut knob = Knob::new(*spec, BAND_LABELS[k], kind, card.dials[k], 62.0).with_id(band_id(b, spec.id));
                        if k < 2 {
                            knob = knob.with_curve(AMOUNT_DIAL);
                        } else if k == 2 {
                            knob = knob.with_curve(TIME_DIAL);
                        }
                        self.card_knobs.push(knob);
                    }
                } else {
                    for (k, spec) in [B_ATTACK, B_SUSTAIN].into_iter().enumerate() {
                        let mut knob = Knob::new(spec, BAND_LABELS[k], KnobKind::Bipolar, card.dials[k], 34.0).with_id(band_id(b, spec.id)).with_curve(AMOUNT_DIAL);
                        knob.value_inside = false;
                        self.card_knobs.push(knob);
                    }
                }
            }
            self.card_key = Some(key);
        }
        // Follow the cards as the layout eases.
        let mut centres = cards.iter().flat_map(|c| c.dials.iter().copied().take(if c.big { 6 } else { 2 }));
        for k in &mut self.card_knobs {
            if let Some(c) = centres.next() {
                k.center = c;
            }
        }
    }

    fn after_session(&mut self, host: &dyn ParamHost) {
        if self.session.take_replaced() {
            let layout = Self::layout(host);
            if layout.position(self.selected).is_none() {
                self.selected = 0;
            }
            self.knobs_for = None;
            self.card_key = None;
        }
    }

    fn hot_at(&self, host: &dyn ParamHost, p: Point) -> Option<Hot> {
        let layout = Self::layout(host);
        let l = lanes(self.grow);
        let rail = Rect::new(0.0, l.ry, HIST_W, l.ry + l.rh);
        if rail.inflate(0.0, 4.0).contains(p) {
            for k in 0..layout.n {
                let slot = layout.crossovers[k];
                if (Self::grip_center(host, slot, &l).x - p.x).abs() <= GRIP_HIT {
                    return Some(Hot::Grip(slot));
                }
            }
            if layout.n < CROSSOVERS {
                for (i, c) in Self::ghost_centers(&l).into_iter().enumerate() {
                    if (c.x - p.x).abs() <= GRIP_HIT {
                        return Some(Hot::Ghost(i == 0));
                    }
                }
            }
            let f = rail_f(p.x);
            let b = (0..=layout.n).map(|q| layout.owners[q]).find(|&b| layout.region(b, &Self::crossovers(host)).is_some_and(|(lo, hi)| f >= f64::from(lo) && f <= f64::from(hi))).unwrap_or(0);
            return Some(Hot::Region(b));
        }
        for card in self.cards(&layout, &l) {
            if card.scope.contains(p) {
                if card.big {
                    let band = Self::band(host, card.band);
                    return Some(Hot::Scope(card.band, Self::scope_part(card.scope, &band, p, Modifiers::default())));
                }
                return Some(Hot::Card(card.band));
            }
            if card.rect.contains(p) {
                return Some(Hot::Card(card.band));
            }
        }
        (p.y < l.h1 && p.x < HIST_W && p.y > DISPLAY_TOP).then_some(Hot::History)
    }

    fn key_event(&mut self, key: Key, mods: Modifiers, host: &dyn ParamHost) -> bool {
        if let Some(used) = self.session.key(key, mods, host) {
            return used;
        }
        let layout = Self::layout(host);
        match key {
            Key::Escape => {
                if self.menu.is_some() {
                    self.menu = None;
                    return true;
                }
                if matches!(self.drag, Some(Drag::Create { .. })) {
                    self.drag = None;
                    return true;
                }
                if Self::on(host, LISTEN.id) {
                    Self::set(host, LISTEN.id, 0.0);
                    return true;
                }
                if self.panel.is_some() {
                    self.panel = None;
                    return true;
                }
                if Self::soloed(host, &layout).is_some() {
                    for b in 0..BANDS {
                        Self::set(host, &band_id(b, B_SOLO.id), 0.0);
                    }
                    return true;
                }
                false
            }
            Key::Delete | Key::Backspace if layout.n > 0 => {
                let b = self.shown_selected(&layout);
                self.remove_band(host, b);
                true
            }
            Key::Left | Key::Right => {
                let bands = layout.bands();
                let at = layout.position(self.selected).unwrap_or(0);
                let next = if key == Key::Right { (at + 1).min(bands.len() - 1) } else { at.saturating_sub(1) };
                self.selected = bands[next];
                true
            }
            Key::Up | Key::Down => {
                // Attack ±0.5 dB (Shift ±0.1) on the selected band.
                let b = self.shown_selected(&layout);
                let step = if mods.fine() { 0.1 } else { 0.5 } * if key == Key::Up { 1.0 } else { -1.0 };
                let id = band_id(b, B_ATTACK.id);
                Self::set(host, &id, (Self::plain(host, &id) + step).clamp(-AMOUNT_DB, AMOUNT_DB));
                true
            }
            Key::Character(c) if !(mods.ctrl || mods.meta) => match c.to_ascii_lowercase() {
                's' => {
                    let b = self.shown_selected(&layout);
                    self.toggle_solo(host, b);
                    true
                }
                'm' => {
                    let b = self.shown_selected(&layout);
                    let on = Self::on(host, &band_id(b, B_MUTE.id));
                    Self::set(host, &band_id(b, B_MUTE.id), if on { 0.0 } else { 1.0 });
                    true
                }
                _ => false,
            },
            _ => false,
        }
    }

    fn handle_event(&mut self, event: &UiEvent, host: &dyn ParamHost) -> bool {
        if let UiEvent::KeyDown { key, mods } = *event {
            return self.key_event(key, mods, host);
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
        for k in &mut self.panel_knobs {
            if k.event(event, host) {
                return true;
            }
        }
        let over_panel = |p: Point, s: &Self| s.panel.is_some() && s.hits.rect.contains(p);
        let pos = match *event {
            UiEvent::PointerMove { pos, .. } | UiEvent::PointerDown { pos, .. } | UiEvent::DoubleClick { pos, .. } | UiEvent::Wheel { pos, .. } => Some(pos),
            _ => None,
        };
        if !pos.is_some_and(|p| over_panel(p, self)) || matches!(event, UiEvent::PointerUp { .. }) {
            for k in &mut self.card_knobs {
                if k.event(event, host) {
                    return true;
                }
            }
        }
        match *event {
            UiEvent::PointerMove { pos, mods } => {
                self.pointer = Some(pos);
                if let Some(drag) = self.drag {
                    self.drag_to(host, drag, pos, mods);
                    return true;
                }
                self.hot = if over_panel(pos, self) { None } else { self.hot_at(host, pos) };
                true
            }
            UiEvent::PointerLeft => {
                self.pointer = None;
                self.hot = None;
                true
            }
            UiEvent::PointerDown { pos, button: MouseButton::Left, mods } => self.press(host, pos, mods),
            UiEvent::PointerDown { pos, button: MouseButton::Right, .. } => match self.hot_at(host, pos) {
                Some(Hot::Region(b)) | Some(Hot::Card(b)) | Some(Hot::Scope(b, _)) => {
                    self.open_band_menu(b, pos, host);
                    true
                }
                _ => false,
            },
            UiEvent::PointerUp { button: MouseButton::Left, .. } => self.release(host),
            UiEvent::DoubleClick { pos, .. } => self.double_click(host, pos),
            UiEvent::Wheel { pos, lines, mods } => match self.hot_at(host, pos) {
                Some(Hot::Grip(slot)) => {
                    let id = cross_id(slot, X_FREQ.id);
                    let f = X_FREQ.wheel(Self::plain(host, &id), lines, mods.fine());
                    Self::set(host, &id, self.clamp_cross(host, slot, f64::from(f)) as f32);
                    true
                }
                _ => false,
            },
            _ => false,
        }
    }

    /// `f` kept clear of crossover `slot`'s neighbours.
    fn clamp_cross(&self, host: &dyn ParamHost, slot: usize, f: f64) -> f64 {
        let layout = Self::layout(host);
        let x = Self::crossovers(host);
        let gap = 2f64.powf(f64::from(MIN_GAP_OCT));
        let Some(p) = layout.crossovers[..layout.n].iter().position(|&k| k == slot) else { return f };
        let lo = if p == 0 { f64::from(FREQ_MIN) * 1.06 } else { f64::from(x[layout.crossovers[p - 1]].unwrap_or(FREQ_MIN)) * gap };
        let hi = if p + 1 == layout.n { f64::from(FREQ_MAX) / 1.06 } else { f64::from(x[layout.crossovers[p + 1]].unwrap_or(FREQ_MAX)) / gap };
        f.clamp(lo.min(hi), hi.max(lo))
    }

    fn press(&mut self, host: &dyn ParamHost, pos: Point, mods: Modifiers) -> bool {
        if pos.y < HEADER {
            for (i, p) in [Panel::Stereo, Panel::Detector].into_iter().enumerate() {
                if self.head_hits[i].contains(pos) {
                    self.panel = if self.panel == Some(p) { None } else { Some(p) };
                    self.knobs_for = None;
                    return true;
                }
            }
            return true;
        }
        if pos.y > FOOTER_TOP {
            return true;
        }
        if let Some(p) = self.panel.filter(|_| self.hits.rect.contains(pos)) {
            let hits = self.hits.clone();
            if hits.close.contains(pos) {
                self.panel = None;
                return true;
            }
            if hits.listen.is_some_and(|r| r.contains(pos)) {
                let on = Self::on(host, LISTEN.id);
                Self::set(host, LISTEN.id, if on { 0.0 } else { 1.0 });
                return true;
            }
            if let Some(&(_, v)) = hits.modes.iter().find(|(r, _)| r.contains(pos)) {
                Self::set(host, STEREO.id, v);
                return true;
            }
            if p == Panel::Detector && hits.filter.is_some() {
                for hp in [true, false] {
                    if Self::filter_node(host, hp).distance(pos) < 14.0 {
                        host.begin_gesture(if hp { DET_HP.id } else { DET_LP.id });
                        self.drag = Some(Drag::Filter { hp });
                        return true;
                    }
                }
            }
            return true;
        }
        let layout = Self::layout(host);
        let l = lanes(self.grow);
        // A card's buttons.
        for card in self.cards(&layout, &l) {
            if card.solo.is_some_and(|r| r.contains(pos)) {
                self.toggle_solo(host, card.band);
                return true;
            }
            if card.mute.is_some_and(|r| r.contains(pos)) {
                let id = band_id(card.band, B_MUTE.id);
                let on = Self::on(host, &id);
                Self::set(host, &id, if on { 0.0 } else { 1.0 });
                return true;
            }
            if card.bin.is_some_and(|r| r.contains(pos)) {
                self.remove_band(host, card.band);
                return true;
            }
        }
        match self.hot_at(host, pos) {
            Some(Hot::Grip(slot)) => {
                host.begin_gesture(&cross_id(slot, X_FREQ.id));
                let f0 = f64::from(Self::plain(host, &cross_id(slot, X_FREQ.id)));
                self.drag = Some(Drag::Cross { slot, start: pos, f0, merge: None });
                true
            }
            Some(Hot::Ghost(low)) => {
                self.drag = Some(Drag::Create { low, start: pos, now: pos });
                true
            }
            Some(Hot::Region(b)) | Some(Hot::Card(b)) => {
                self.selected = b;
                true
            }
            Some(Hot::Scope(b, _)) => {
                self.selected = b;
                let Some(card) = self.cards(&layout, &l).into_iter().find(|c| c.band == b) else { return true };
                let band = Self::band(host, b);
                let part = Self::scope_part(card.scope, &band, pos, mods);
                let (id, value) = match part {
                    Part::Attack => (B_ATTACK.id, band.attack_db),
                    Part::Sustain => (B_SUSTAIN.id, band.sustain_db),
                    Part::Time => (B_TIME.id, band.transient_ms),
                    Part::Transition => (B_TRANSITION.id, band.transition * 100.0),
                };
                host.begin_gesture(&band_id(b, id));
                self.drag = Some(Drag::Scope { band: b, part, start: pos, value });
                true
            }
            Some(Hot::History) | None => false,
        }
    }

    fn drag_to(&mut self, host: &dyn ParamHost, drag: Drag, pos: Point, mods: Modifiers) {
        let fine = if mods.fine() { 0.1 } else { 1.0 };
        match drag {
            Drag::Cross { slot, start, f0, .. } => {
                let layout = Self::layout(host);
                let x = Self::crossovers(host);
                let Some(p) = layout.crossovers[..layout.n].iter().position(|&k| k == slot) else { return };
                // Follow the pointer (Shift: a tenth as far).
                let f = f0 * (rail_f(pos.x) / rail_f(start.x)).powf(fine);
                let px = rail_x(f);
                let lower = (p > 0).then(|| layout.crossovers[p - 1]);
                let upper = (p + 1 < layout.n).then(|| layout.crossovers[p + 1]);
                let at = |k: usize| rail_x(f64::from(x[k].unwrap_or(FREQ_MIN)));
                let merge = match (lower, upper) {
                    (Some(k), _) if px <= at(k) + MERGE_PX => Some(Merge::Down(k)),
                    (_, Some(k)) if px >= at(k) - MERGE_PX => Some(Merge::Up),
                    (None, _) if pos.x < RAIL_X0 - OFF_END_PX => Some(Merge::OffLow),
                    (_, None) if pos.x > RAIL_X0 + RAIL_W + OFF_END_PX => Some(Merge::OffHigh),
                    _ => None,
                };
                Self::set_raw(host, &cross_id(slot, X_FREQ.id), self.clamp_cross(host, slot, f) as f32);
                self.drag = Some(Drag::Cross { slot, start, f0, merge });
            }
            Drag::Create { low, start, .. } => self.drag = Some(Drag::Create { low, start, now: pos }),
            Drag::Scope { band, part, start, value } => {
                let layout = Self::layout(host);
                let Some(card) = self.cards(&layout, &lanes(self.grow)).into_iter().find(|c| c.band == band) else { return };
                let r = card.scope;
                match part {
                    Part::Attack | Part::Sustain => {
                        let v = value + ((start.y - pos.y) / Self::scope_k(r) * fine) as f32;
                        let id = if part == Part::Attack { B_ATTACK.id } else { B_SUSTAIN.id };
                        Self::set_raw(host, &band_id(band, id), v.clamp(-AMOUNT_DB, AMOUNT_DB));
                    }
                    Part::Time => {
                        let x0 = Self::scope_x(r, f64::from(value));
                        let t = Self::scope_t(r, x0 + (pos.x - start.x) * fine);
                        Self::set_raw(host, &band_id(band, B_TIME.id), B_TIME.snap((t as f32).clamp(B_TIME.min, B_TIME.max)));
                    }
                    Part::Transition => {
                        let v = value + ((pos.x - start.x) * 200.0 / 240.0 * fine) as f32;
                        Self::set_raw(host, &band_id(band, B_TRANSITION.id), v.clamp(0.0, 200.0));
                    }
                }
            }
            Drag::Filter { hp } => {
                let f = Self::filter_f(Self::filter_area(), pos.x) as f32;
                let (id, spec) = if hp { (DET_HP.id, DET_HP) } else { (DET_LP.id, DET_LP) };
                Self::set_raw(host, id, f.clamp(spec.min, spec.max));
            }
        }
    }

    fn release(&mut self, host: &dyn ParamHost) -> bool {
        let Some(drag) = self.drag.take() else { return false };
        match drag {
            Drag::Cross { slot, merge, .. } => {
                host.end_gesture(&cross_id(slot, X_FREQ.id));
                let layout = Self::layout(host);
                match merge {
                    Some(Merge::Down(lower)) => {
                        // The band between goes; the one above this crossover grows down.
                        let f = Self::plain(host, &cross_id(lower, X_FREQ.id));
                        Self::set(host, &cross_id(slot, X_FREQ.id), f);
                        Self::remove_crossover(host, lower);
                        self.selected = slot + 1;
                    }
                    Some(Merge::Up) | Some(Merge::OffHigh) => {
                        let p = layout.crossovers[..layout.n].iter().position(|&k| k == slot).unwrap_or(0);
                        Self::remove_crossover(host, slot);
                        self.selected = layout.owners[p];
                    }
                    Some(Merge::OffLow) => {
                        Self::copy_band(host, slot + 1, 0);
                        Self::remove_crossover(host, slot);
                        self.selected = 0;
                    }
                    None => {}
                }
            }
            Drag::Create { start, now, .. } => {
                if now.distance(start) > 6.0 && (RAIL_X0 - OFF_END_PX..=RAIL_X0 + RAIL_W + OFF_END_PX).contains(&now.x) {
                    self.split(host, rail_f(now.x.clamp(RAIL_X0, RAIL_X0 + RAIL_W)));
                }
            }
            Drag::Scope { band, part, .. } => {
                let id = match part {
                    Part::Attack => B_ATTACK.id,
                    Part::Sustain => B_SUSTAIN.id,
                    Part::Time => B_TIME.id,
                    Part::Transition => B_TRANSITION.id,
                };
                host.end_gesture(&band_id(band, id));
            }
            Drag::Filter { hp } => host.end_gesture(if hp { DET_HP.id } else { DET_LP.id }),
        }
        true
    }

    fn double_click(&mut self, host: &dyn ParamHost, pos: Point) -> bool {
        if self.panel.is_some() && self.hits.rect.contains(pos) {
            if self.hits.filter.is_some() {
                // Double-clicking a node opens the filter there again.
                for (hp, spec) in [(true, DET_HP), (false, DET_LP)] {
                    if Self::filter_node(host, hp).distance(pos) < 14.0 {
                        Self::set(host, spec.id, spec.default);
                    }
                }
            }
            return true;
        }
        match self.hot_at(host, pos) {
            Some(Hot::Grip(slot)) => {
                // Merge here: the selected band survives, otherwise the lower one.
                self.drag = None;
                let layout = Self::layout(host);
                let p = layout.crossovers[..layout.n].iter().position(|&k| k == slot).unwrap_or(0);
                let lower = layout.owners[p];
                if self.selected == slot + 1 {
                    Self::copy_band(host, slot + 1, lower);
                }
                Self::remove_crossover(host, slot);
                self.selected = lower;
                true
            }
            Some(Hot::Region(_)) => {
                self.split(host, rail_f(pos.x));
                true
            }
            Some(Hot::Scope(b, part)) => {
                self.drag = None;
                let (id, spec) = match part {
                    Part::Attack => (B_ATTACK.id, B_ATTACK),
                    Part::Sustain => (B_SUSTAIN.id, B_SUSTAIN),
                    Part::Time => (B_TIME.id, B_TIME),
                    Part::Transition => (B_TRANSITION.id, B_TRANSITION),
                };
                Self::set(host, &band_id(b, id), spec.default);
                true
            }
            _ => false,
        }
    }

    /// Reads the input spectrum (at most once per display refresh) and eases the curve towards it.
    fn update_spectrum(&mut self, dt: f64) -> bool {
        let fs = f64::from(self.shared.sample_rate.load()).max(1.0);
        if fs != self.log_spectrum.sample_rate() {
            self.log_spectrum = LogSpectrum::new(ANALYZER_SIZE, fs);
        }
        if self.last_analysis.elapsed().as_secs_f64() >= 0.0155 {
            self.last_analysis = Instant::now();
            self.shared.spectrum.read_latest(&mut self.samples);
            if self.samples.iter().all(|s| s.abs() < 1e-7) {
                self.spectrum_target.iter_mut().for_each(|t| *t = -150.0);
            } else {
                self.analyzer.analyze_power(&self.samples, &mut self.power);
                self.log_spectrum.read(&self.power, &mut self.spectrum_target);
            }
        }
        let rise = (1.0 - (-dt / SPECTRUM_RISE_S).exp()) as f32;
        let fall = (1.0 - (-dt / SPECTRUM_FALL_S).exp()) as f32;
        let mut changed = false;
        for (v, &t) in self.spectrum.iter_mut().zip(&self.spectrum_target) {
            let next = *v + (t - *v) * if t > *v { rise } else { fall };
            changed |= (next - *v).abs() > 0.02;
            *v = next;
        }
        changed
    }
}

/// Straight segments through `pts` (dense samples of a smooth function).
fn polyline(pts: &[Point]) -> BezPath {
    let mut p = BezPath::new();
    for (i, &q) in pts.iter().enumerate() {
        if i == 0 { p.move_to(q) } else { p.line_to(q) }
    }
    p
}

/// A Catmull-Rom curve through `pts` (no corners between samples).
fn smooth_path(pts: &[Point]) -> BezPath {
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

impl View for ShapeView {
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
        cx.scene.push_clip_layer(Fill::NonZero, Affine::IDENTITY, &Rect::new(0.0, DISPLAY_TOP, HIST_W, HELP_TOP));
        self.draw_history(cx, &layout, &l);
        self.draw_rail(cx, &layout, &l);
        let cards = self.cards(&layout, &l);
        self.draw_funnels(cx, &layout, &l, &cards);
        self.draw_cards(cx, &layout, &cards);
        cx.scene.pop_layer();
        self.draw_meters(cx);
        self.draw_states(cx);
        self.help_text(cx.host).draw(cx, Rect::new(0.0, HELP_TOP, WIDTH, FOOTER_TOP));
        self.draw_footer(cx, &layout);
        self.draw_panel(cx, &layout);
        self.draw_menu(cx);
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
            if history.len() == crate::dsp::HISTORY_RECORDS {
                history.pop_front();
            }
            history.push_back(*r);
            fresh = true;
        });
        if fresh {
            self.last_fresh = Instant::now();
        }
        // The layout eases between one band and several.
        let target = if Self::layout(host).n > 0 { 1.0 } else { 0.0 };
        let step = dt * 1000.0 / GROW_MS;
        let before = self.grow;
        self.grow = if target > self.grow { (self.grow + step).min(1.0) } else { (self.grow - step).max(0.0) };
        let grew = self.grow != before;
        if Self::layout(host).position(self.selected).is_none() {
            self.selected = 0;
        }
        // Meters: instant attack, 20 dB/s release.
        if let Some(r) = self.history.back().copied() {
            let targets = [r[record::IN], r[record::IN + 1], r[record::OUT], r[record::OUT + 1]];
            for (m, t) in self.meters.iter_mut().zip(targets) {
                let fall = METER_RELEASE_DB_S * dt as f32;
                *m = if t > *m { t } else { (*m - fall).max(t) };
            }
        }
        let spectrum = self.update_spectrum(dt);
        let flowing = self.last_fresh.elapsed().as_secs_f32() < 0.25;
        fresh || flowing || grew || spectrum || self.drag.is_some() || self.meters.iter().any(|&m| m > -149.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_rail_maps_frequency_both_ways() {
        for f in [20.0, 150.0, 1000.0, 20_000.0] {
            assert!((rail_f(rail_x(f)) - f).abs() / f < 1e-9);
        }
        assert!((rail_x(20.0) - RAIL_X0).abs() < 1e-9);
    }

    #[test]
    fn the_gain_lane_eases_flat_without_a_corner() {
        assert!((soft_gain(6.0) - 6.0).abs() < 1e-12);
        assert!(soft_gain(30.0) < GAIN_SOFT_DB);
        assert!((soft_gain(-30.0) + soft_gain(30.0)).abs() < 1e-12);
        // Its slope is continuous where the easing starts.
        let k = GAIN_SOFT_DB - 2.0;
        let (a, b) = (soft_gain(k) - soft_gain(k - 1e-6), soft_gain(k + 1e-6) - soft_gain(k));
        assert!((a - b).abs() < 1e-9);
    }
}
