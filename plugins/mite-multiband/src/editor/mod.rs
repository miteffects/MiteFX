//! The Mite Multiband editor (docs/design/multiband-brief.md §2), in 1280×760 design units: Sat's
//! and Shape's frame. Top to bottom: the spectrum (input dim, output lit, the bands' regions
//! tinted, and the dynamic response: what the bands' gains do to the spectrum right now, from the
//! crossover's own weights), the shared crossover rail (`mite_ui::widgets::rail`), and a card per
//! band: the selected one wide (the quadrants, the detectors, the band's transfer square from
//! `mite_dsp::dynamics::curve_db` with a live dot, its gain change, the dials), the others narrow.

mod spectrum;

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Instant;

use mite_core::{ParamDisplay, ParamScale, ParamSpec};
use mite_dsp::dynamics::{curve_db, Detector, Quadrant, Stage, STAGES};
use mite_dsp::multiband::{MbBand, MbSettings, BANDS, CROSSOVERS};
use mite_ui::kurbo::{Affine, BezPath, Cap, Circle, Join, Line, Point, Rect, RoundedRect, Stroke};
use mite_ui::paint::{glow_stroke, icon_button, panel};
use mite_ui::peniko::{Color, Fill};
use mite_ui::session::{HeaderLayout, Session, SessionConfig};
use mite_ui::widgets::brand::{draw_mark, draw_title};
use mite_ui::widgets::rail::{draw_badge, draw_glyph, glyph as rail_glyph, hatch, BandMenu, MenuAsk, Rail, RailHot, RailParams};
use mite_ui::widgets::footer::draw_dropdown_button;
use mite_ui::widgets::key_eq::KeySpectrum;
use mite_ui::widgets::{Dropdown, DropdownEvent, Footer, FooterDial, FooterToggle, Help, KeyEq, Knob, KnobKind, ToggleGlyph};
use mite_ui::{write_if_changed, Align, DrawCx, Key, Modifiers, MouseButton, ParamHost, RecordingHost, TextStyle, UiEvent, View};

use crate::dsp::{record, HISTORY_RECORDS, HISTORY_SLICE_MS};
use mite_ui::widgets::history::{self, HistoryLane};
use crate::params::*;
use crate::presets::{preset_ids, FACTORY, PLUGIN};
use crate::MbShared;

pub use mite_dsp::analyzer::ANALYZER_SIZE;

/// Mite Multiband wears the Multiband accent (tokens.json `color.accent.multiband`, mint).
pub const ACCENT: &str = "multiband";

type Layout = mite_dsp::crossover::Layout<BANDS>;

const WIDTH: f64 = 1280.0;
const HEIGHT: f64 = 760.0;
const HEADER: f64 = 56.0;
const DISPLAY_TOP: f64 = HEADER;
const DISPLAY_H: f64 = 620.0;
const HELP_TOP: f64 = DISPLAY_TOP + DISPLAY_H;
const FOOTER_TOP: f64 = HELP_TOP + 28.0;
const MAIN_W: f64 = 1180.0;
pub(crate) const PLOT_X0: f64 = 14.0;
pub(crate) const PLOT_W: f64 = 1152.0;
const PLOT_X1: f64 = PLOT_X0 + PLOT_W;
const METER_RELEASE_DB_S: f32 = 20.0;
const METER_TOP: f64 = DISPLAY_TOP + 20.0;
const METER_BOTTOM: f64 = DISPLAY_TOP + 592.0;
const PEAK_RISE_S: f64 = 0.06;
const PEAK_FALL_S: f64 = 0.3;
const GROW_MS: f64 = 220.0;
const CARD_WIDE: f64 = 600.0;
const CARD_GAP: f64 = 8.0;
/// The transfer square's levels, dB (in and out).
const SQ_RANGE: f32 = 60.0;
/// Dragging the square: threshold per px across, ratio's octaves per px up.
const THRESH_PER_PX: f32 = 0.15;
const RATIO_OCT_PER_PX: f32 = 1.0 / 60.0;
/// The band history: its levels' range (dB under 0) and its time across.
const HIST_RANGE: f32 = 48.0;
const HIST_SECONDS: f64 = 6.0;
/// The gain bar's reach either way, dB (a couple of dB must read clearly: user, 2026-11-03).
const BAR_DB: f64 = 12.0;
/// The dynamic response on the spectrum: 0 dB this far down the plot, and dB per its height.
const RESPONSE_ZERO: f64 = 0.32;
const RESPONSE_SPAN_DB: f64 = 30.0;

/// Glyphs (24×24, stroked): Comp's quadrants and detectors.
const QUAD_GLYPHS: [&str; 4] = ["M3 21 L12 12 C14.5 9.5 16.5 9.2 21 8.4", "M6 23 C8 21 9.5 16 12 12 L21 3", "M3 14.5 C6 14 9.5 13.2 12 12 L21 3", "M3 21 L12 12 C13.8 10.2 14.6 6 15 2.5"];
const DET_GLYPHS: [&str; 3] = ["M3 18 H9 C10.5 18 11 4 12 4 C13 4 13.5 18 15 18 H21", "M3 18 C7 18 8.5 9 12 9 C15.5 9 17 18 21 18", "M4 18 V15 M8 18 V11 M12 18 V8 M16 18 V11 M20 18 V14"];
const POWER: &str = "M12 4 V11 M7.5 6.8 A7 7 0 1 0 16.5 6.8";
const KEY_GLYPH: &str = "M7.5 15.5 A3.5 3.5 0 1 1 7.51 15.5 M10.5 13.5 L20 7 M17 9 L19 12 M14.5 10.7 L16 13";
const EQ_GLYPH: &str = "M4 7 H20 M4 12 H20 M4 17 H20 M8.5 5 V9 M15 10 V14 M10.5 15 V19";
const DELTA_GLYPH: &str = "M12 4.5 L19.5 18.5 H4.5 Z";
/// The footer's sidechain source and EQ buttons; the EQ panel over the cards (opaque).
const SOURCE_BUTTON: Rect = Rect::new(16.0, FOOTER_TOP + 9.0, 156.0, FOOTER_TOP + 47.0);
const EQ_BUTTON: Rect = Rect::new(164.0, FOOTER_TOP + 9.0, 204.0, FOOTER_TOP + 47.0);
const EQ_PANEL: Rect = Rect::new(PLOT_X0, DISPLAY_TOP + 284.0, PLOT_X1, DISPLAY_TOP + 610.0);

#[derive(Debug, Clone, Copy)]
struct Lanes {
    spectrum: Rect,
    rail: Rect,
    cy: f64,
    cb: f64,
}

/// Spectrum top and bottom, rail top and height, cards' top and bottom (from the display top).
const SINGLE: [f64; 6] = [12.0, 222.0, 230.0, 36.0, 274.0, 608.0];
const MULTI: [f64; 6] = [12.0, 192.0, 200.0, 64.0, 290.0, 608.0];

fn lanes(grow: f64) -> Lanes {
    let v: [f64; 6] = std::array::from_fn(|i| SINGLE[i] + (MULTI[i] - SINGLE[i]) * grow);
    let y = |i: usize| DISPLAY_TOP + v[i];
    Lanes { spectrum: Rect::new(PLOT_X0, y(0), PLOT_X1, y(1)), rail: Rect::new(PLOT_X0, y(2), PLOT_X1, y(2) + v[3]), cy: y(4), cb: y(5) }
}

fn rail_params() -> RailParams {
    RailParams { cross_on: |k| cross_id(k, X_ON.id), cross_freq: |k| cross_id(k, X_FREQ.id), freq: X_FREQ, solo: |b| band_id(b, B_SOLO.id), mute: |b| band_id(b, B_MUTE.id), copy: MbView::copy_band }
}

/// A band's sound settings, in copy order.
const BAND_IDS: [&str; 12] = ["on", "quad", "thresh", "ratio", "knee", "range", "attack", "release", "det", "gain", "bypass", "mute"];

/// The wide card's dials: THRESHOLD large, then RATIO, KNEE, RANGE, GAIN, ATTACK, RELEASE.
const DIALS: [(ParamSpec, &str, KnobKind); 7] = [
    (B_THRESH, "THRESHOLD", KnobKind::Unipolar),
    (B_RATIO, "RATIO", KnobKind::Unipolar),
    (B_KNEE, "KNEE", KnobKind::Unipolar),
    (B_RANGE, "RANGE", KnobKind::Unipolar),
    (B_GAIN, "GAIN", KnobKind::Bipolar),
    (B_ATTACK, "ATTACK", KnobKind::Unipolar),
    (B_RELEASE, "RELEASE", KnobKind::Unipolar),
];

fn draw_svg(scene: &mut mite_ui::vello::Scene, d: &str, c: Point, size: f64, color: Color, width: f64) {
    let Ok(mut path) = BezPath::from_svg(d) else { return };
    let k = size / 24.0;
    path.apply_affine(Affine::translate((c.x - 12.0 * k, c.y - 12.0 * k)) * Affine::scale(k));
    scene.stroke(&Stroke::new(width).with_caps(Cap::Round).with_join(Join::Round), Affine::IDENTITY, color, None, &path);
}

/// A stand-in host holding every Mite Multiband parameter at its default.
pub fn memory_host() -> mite_ui::MemoryParamHost {
    fn leak(s: String) -> &'static str {
        Box::leak(s.into_boxed_str())
    }
    let index = |id: &'static str, count: usize, default: usize| ParamSpec { id, name: id, unit: "", min: 0.0, max: (count - 1) as f32, default: default as f32, scale: ParamScale::Linear, smoothing_ms: 0.0, decimals: 0, wheel_step: 1.0, wheel_step_fine: 1.0, display: ParamDisplay::Number };
    let mut host = mite_ui::MemoryParamHost::new();
    for line in manifest().lines() {
        let f: Vec<&str> = line.split(" | ").collect();
        let id = leak(f[0].to_owned());
        let spec = match f[1] {
            "bool" => index(id, 2, usize::from(f[2] == "true")),
            "choice" => index(id, f[2].split(", ").count(), f[3].parse().unwrap_or(0)),
            _ => {
                let base = spec_for(id).expect("every manifest line has a spec");
                ParamSpec { id, default: f[4].parse().unwrap_or(base.default), ..base }
            }
        };
        host = host.with(spec);
    }
    host
}

/// The engine's settings as a host's parameters set them (as `MbParams::settings` does).
pub fn settings(host: &dyn ParamHost) -> MbSettings {
    let p = |id: &str| host.plain(id, host.normalized(id));
    let on = |id: &str| p(id) >= 0.5;
    let external = on(SC_SOURCE.id);
    MbSettings {
        crossovers: std::array::from_fn(|k| on(&cross_id(k, X_ON.id)).then(|| p(&cross_id(k, X_FREQ.id)))),
        bands: std::array::from_fn(|b| MbBand { stage: MbView::stage(host, b), gain_db: p(&band_id(b, B_GAIN.id)), bypass: on(&band_id(b, B_BYPASS.id)), mute: on(&band_id(b, B_MUTE.id)), solo: on(&band_id(b, B_SOLO.id)), external: external && on(&band_id(b, B_EXTERNAL.id)), delta: on(&band_id(b, B_DELTA.id)) }),
        lookahead_ms: p(LOOKAHEAD.id),
        link: p(LINK.id) / 100.0,
        key_eq: std::array::from_fn(|b| mite_ui::widgets::key_eq::band_with(host, b, true)),
        mix: p(MIX.id) / 100.0,
        out_db: p(OUT_GAIN.id),
        delta: on(DELTA.id),
        bypass: on(BYPASS.id),
        listen: external && on(LISTEN.id),
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Drag {
    /// On a band's square: threshold across, ratio up and down.
    Square { band: usize, start: Point, thresh: f32, ratio: f32 },
    /// The threshold line on a band's history: up and down, at the history's scale.
    Thresh { band: usize, start: Point, thresh: f32 },
}

/// A level's y on a band history `r`.
fn hist_y(r: Rect, db: f32) -> f64 {
    r.y0 + 8.0 + (f64::from(-db).clamp(0.0, f64::from(HIST_RANGE)) / f64::from(HIST_RANGE)) * (r.height() - 16.0)
}

fn hist_px_per_db(r: Rect) -> f64 {
    (r.height() - 16.0) / f64::from(HIST_RANGE)
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Hot {
    Square(usize),
    /// A band's history (its threshold line when `true`).
    Hist(usize, bool),
    Quad(usize, usize),
    Det(usize, usize),
    Power(usize),
    Solo(usize),
    Mute(usize),
    Remove(usize),
    Card(usize),
    Rail(RailHot),
    /// The band's external key and its Delta.
    Ext(usize),
    BandDelta(usize),
    Source,
    EqButton,
    EqPanel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CardKind {
    Single,
    Wide,
    Narrow,
}

#[derive(Debug, Clone)]
struct Card {
    band: usize,
    kind: CardKind,
    rect: Rect,
    square: Rect,
    /// The band's gain change bar, beside the square, and its history (wide cards).
    bar: Rect,
    hist: Option<Rect>,
    quads: Vec<Rect>,
    dets: Vec<Rect>,
    power: Option<Rect>,
    solo: Option<Rect>,
    mute: Option<Rect>,
    remove: Option<Rect>,
    ext: Option<Rect>,
    delta: Option<Rect>,
    dials: Vec<(Point, f64)>,
}

impl Card {
    fn buttons(&self) -> [(Option<Rect>, Hot); 6] {
        let b = self.band;
        [(self.power, Hot::Power(b)), (self.solo, Hot::Solo(b)), (self.mute, Hot::Mute(b)), (self.remove, Hot::Remove(b)), (self.ext, Hot::Ext(b)), (self.delta, Hot::BandDelta(b))]
    }
}

pub struct MbView {
    shared: Arc<MbShared>,
    session: Session,
    footer: Footer,
    rail: Rail<BANDS>,
    pointer: Option<Point>,
    hot: Option<Hot>,
    drag: Option<Drag>,
    menu: Option<BandMenu>,
    clipboard: Option<[f32; 12]>,
    knobs: Vec<Knob>,
    knob_key: Option<(Vec<usize>, usize)>,
    history: VecDeque<[f32; record::WIDTH]>,
    seen: usize,
    meters: [f32; 4],
    last_fresh: Instant,
    last_tick: Instant,
    grow: f64,
    spectrum: spectrum::Spectrum,
    /// Each band's input peak (linear) and gain change (dB), eased.
    peak: [f32; BANDS],
    gain: [f32; BANDS],
    source_menu: Option<Dropdown>,
    key_eq: KeyEq,
    key_spectrum: KeySpectrum,
    eq_open: bool,
}

impl MbView {
    pub fn new(shared: Arc<MbShared>) -> Self {
        let footer = Footer::new(
            FOOTER_TOP,
            (WIDTH, HEIGHT),
            vec![
                FooterToggle { id: LISTEN.id, glyph: ToggleGlyph::Listen, words: "Listen — hear the sidechain as the external bands' detectors hear it (after its EQ)" },
                FooterToggle { id: DELTA.id, glyph: ToggleGlyph::Delta, words: "Delta — hear only what the bands' dynamics change" },
                FooterToggle { id: BYPASS.id, glyph: ToggleGlyph::Bypass, words: "Bypass — the input, latency kept" },
            ],
            vec![
                FooterDial { spec: LOOKAHEAD, kind: KnobKind::Unipolar, name: "LOOKAHEAD", words: "Lookahead — every band sees this much ahead (it is the latency)" },
                FooterDial { spec: MIX, kind: KnobKind::Unipolar, name: "MIX", words: "Mix — against the dry signal through the same crossover, so it never combs" },
                FooterDial { spec: OUT_GAIN, kind: KnobKind::Bipolar, name: "OUTPUT", words: "Output gain" },
            ],
        );
        let mut rail = Rail::new(rail_params(), lanes(0.0).rail, PLOT_X0, PLOT_W);
        rail.tint = (0.18, 0.10);
        rail.labels = false;
        Self {
            // The header's controls 36 px further right: "Multiband" is the longest title.
            session: Session::new(SessionConfig { plugin: PLUGIN, factory: FACTORY, sound_ids: preset_ids(), snapshot_ids: preset_ids() }, shared.snapshots.clone(), shared.preset.clone()).with_layout({
                let d = HeaderLayout::default();
                let sh = |x: (f64, f64)| (x.0 + 36.0, x.1 + 36.0);
                HeaderLayout { prev: sh(d.prev), name: sh(d.name), next: sh(d.next), undo: sh(d.undo), redo: sh(d.redo), slot_x0: d.slot_x0 + 36.0, ..d }
            }),
            footer,
            rail,
            pointer: None,
            hot: None,
            drag: None,
            menu: None,
            clipboard: None,
            knobs: Vec::new(),
            knob_key: None,
            history: VecDeque::with_capacity(HISTORY_RECORDS),
            seen: 0,
            meters: [-150.0; 4],
            last_fresh: Instant::now(),
            last_tick: Instant::now(),
            grow: 0.0,
            spectrum: spectrum::Spectrum::new(),
            peak: [0.0; BANDS],
            gain: [0.0; BANDS],
            source_menu: None,
            key_eq: {
                let mut k = KeyEq::new(Rect::new(EQ_PANEL.x0 + 16.0, EQ_PANEL.y0 + 44.0, EQ_PANEL.x1 - 16.0, EQ_PANEL.y1 - 14.0), mite_dsp::multiband::KEY_BANDS).with_slopes();
                k.menu_bounds = (DISPLAY_TOP, FOOTER_TOP);
                k
            },
            key_spectrum: KeySpectrum::new(),
            eq_open: false,
            shared,
        }
    }

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

    pub fn select(&mut self, band: usize) {
        self.rail.selected = band.min(BANDS - 1);
    }

    pub fn selected(&self) -> usize {
        self.rail.selected
    }

    /// Band `b`'s transfer square and quadrant buttons (tests and tools).
    pub fn band_square(&self, host: &dyn ParamHost, b: usize) -> Option<Rect> {
        self.card(host, b).map(|c| c.square)
    }

    /// Band `b`'s history (wide cards) and a level's y on it (tests and tools).
    pub fn band_history(&self, host: &dyn ParamHost, b: usize) -> Option<Rect> {
        self.card(host, b).and_then(|c| c.hist)
    }

    pub fn history_y(&self, r: Rect, db: f32) -> f64 {
        hist_y(r, db)
    }

    pub fn quad_button(&self, host: &dyn ParamHost, b: usize, q: usize) -> Option<Rect> {
        self.card(host, b).and_then(|c| c.quads.get(q).copied())
    }

    pub fn band_power(&self, host: &dyn ParamHost, b: usize) -> Option<Rect> {
        self.card(host, b).and_then(|c| c.power)
    }

    pub fn dial(&self, id: &str) -> Option<Point> {
        self.knobs.iter().find(|k| k.id == id).map(|k| k.center)
    }

    pub fn rail_y(&self) -> f64 {
        self.rail.area.center().y
    }

    pub fn rail_x(&self, f: f64) -> f64 {
        self.rail.x(f)
    }

    pub fn source_button(&self) -> Rect {
        SOURCE_BUTTON
    }

    pub fn source_rows(&self) -> Vec<Rect> {
        self.source_menu.as_ref().map(Dropdown::item_rects).unwrap_or_default()
    }

    pub fn eq_button(&self) -> Rect {
        EQ_BUTTON
    }

    pub fn eq_area(&self) -> Option<Rect> {
        self.eq_open.then_some(self.key_eq.area)
    }

    pub fn eq_node(&self, host: &dyn ParamHost, b: usize) -> Point {
        mite_ui::widgets::key_eq::node(self.key_eq.area, &mite_ui::widgets::key_eq::band_with(host, b, true))
    }

    pub fn open_eq(&mut self) {
        self.eq_open = true;
    }

    pub fn settle_spectrum(&mut self) {
        let fs = f64::from(self.shared.sample_rate.load());
        self.key_spectrum.settle(&self.shared.key_spectrum, fs);
    }

    pub fn band_ext(&self, host: &dyn ParamHost, b: usize) -> Option<Rect> {
        self.card(host, b).and_then(|c| c.ext)
    }

    pub fn band_delta(&self, host: &dyn ParamHost, b: usize) -> Option<Rect> {
        self.card(host, b).and_then(|c| c.delta)
    }

    pub fn footer_toggle(&self, id: &str) -> Option<Rect> {
        self.footer.toggle_rect(id)
    }

    // ---- parameters -------------------------------------------------------------------------

    fn plain(host: &dyn ParamHost, id: &str) -> f32 {
        host.plain(id, host.normalized(id))
    }

    fn on(host: &dyn ParamHost, id: &str) -> bool {
        Self::plain(host, id) >= 0.5
    }

    fn set(host: &dyn ParamHost, id: &str, plain: f32) {
        write_if_changed(host, id, host.normalize(id, plain));
    }

    fn set_raw(host: &dyn ParamHost, id: &str, plain: f32) {
        host.set_normalized(id, host.normalize(id, plain));
    }

    fn layout(host: &dyn ParamHost) -> Layout {
        Layout::new(&std::array::from_fn::<_, CROSSOVERS, _>(|k| Self::on(host, &cross_id(k, X_ON.id)).then(|| Self::plain(host, &cross_id(k, X_FREQ.id)))))
    }

    fn stage(host: &dyn ParamHost, b: usize) -> Stage {
        let p = |s: &str| Self::plain(host, &band_id(b, s));
        Stage {
            on: p(B_ON.id) >= 0.5,
            quadrant: Quadrant::from_index(p(B_QUAD.id).round().max(0.0) as usize),
            threshold_db: p(B_THRESH.id),
            ratio: p(B_RATIO.id),
            knee_db: p(B_KNEE.id),
            range_db: p(B_RANGE.id),
            attack_ms: p(B_ATTACK.id),
            release_ms: p(B_RELEASE.id),
            auto_release: false,
            detector: Detector::from_index(p(B_DET.id).round().max(0.0) as usize),
        }
    }

    fn band_values(host: &dyn ParamHost, b: usize) -> [f32; 12] {
        std::array::from_fn(|i| Self::plain(host, &band_id(b, BAND_IDS[i])))
    }

    fn write_band(host: &dyn ParamHost, b: usize, v: &[f32; 12]) {
        for (id, &x) in BAND_IDS.iter().zip(v) {
            Self::set(host, &band_id(b, id), x);
        }
    }

    fn copy_band(host: &dyn ParamHost, from: usize, to: usize) {
        let v = Self::band_values(host, from);
        Self::write_band(host, to, &v);
    }

    fn reset_band(host: &dyn ParamHost, b: usize) {
        for id in BAND_IDS {
            let id = band_id(b, id);
            write_if_changed(host, &id, host.default_normalized(&id));
        }
    }

    // ---- geometry ---------------------------------------------------------------------------

    fn cards(&self, layout: &Layout) -> Vec<Card> {
        let l = lanes(self.grow);
        let (y0, y1) = (l.cy, l.cb);
        let bands = layout.bands();
        let n = bands.len();
        let current = self.rail.current(layout);
        let narrow = if n > 1 { (PLOT_W - CARD_WIDE - CARD_GAP * (n - 1) as f64) / (n - 1) as f64 } else { 0.0 };
        let mut x = PLOT_X0;
        bands
            .iter()
            .map(|&band| {
                let single = n == 1;
                let wide = single || band == current;
                let w = if single { PLOT_W } else if wide { CARD_WIDE } else { narrow };
                let rect = Rect::new(x, y0, x + w, y1);
                x += w + CARD_GAP;
                if wide {
                    let button = |k: f64| Rect::new(rect.x1 - 10.0 - 34.0 * (k + 1.0) - 6.0 * k, rect.y0 + 10.0, rect.x1 - 10.0 - 34.0 * k - 6.0 * k, rect.y0 + 44.0);
                    // The square and the band's history side by side, the dials in a row under them.
                    let side = rect.height() - 54.0 - 104.0;
                    let square = Rect::new(rect.x0 + 12.0, rect.y0 + 54.0, rect.x0 + 12.0 + side, rect.y0 + 54.0 + side);
                    let bar = Rect::new(square.x1 + 6.0, square.y0, square.x1 + 18.0, square.y1);
                    let hist = Rect::new(bar.x1 + 10.0, square.y0, rect.x1 - 12.0, square.y1);
                    let left = rect.x0 + 40.0;
                    let quads = (0..4).map(|i| Rect::new(left + i as f64 * 42.0, rect.y0 + 10.0, left + i as f64 * 42.0 + 36.0, rect.y0 + 44.0)).collect();
                    let dleft = left + 4.0 * 42.0 + 16.0;
                    let dets = (0..3).map(|i| Rect::new(dleft + i as f64 * 42.0, rect.y0 + 10.0, dleft + i as f64 * 42.0 + 36.0, rect.y0 + 44.0)).collect();
                    let row = square.y1 + 44.0;
                    let step = (rect.width() - 24.0) / 7.0;
                    let dials = (0..7).map(|k| (Point::new(rect.x0 + 12.0 + step * (k as f64 + 0.5), row), if k == 0 { 52.0 } else { 44.0 })).collect();
                    // Right to left: remove, power, mute, solo, then the band's Delta and key.
                    let (power, delta, ext) = if single { (button(0.0), button(1.0), button(2.0)) } else { (button(1.0), button(4.0), button(5.0)) };
                    Card { band, kind: if single { CardKind::Single } else { CardKind::Wide }, rect, square, bar, hist: Some(hist), quads, dets, power: Some(power), solo: (!single).then(|| button(3.0)), mute: (!single).then(|| button(2.0)), remove: (!single).then(|| button(0.0)), ext: Some(ext), delta: Some(delta), dials }
                } else {
                    let button = |k: f64| Rect::new(rect.x1 - 8.0 - 26.0 * (k + 1.0) - 3.0 * k, rect.y0 + 10.0, rect.x1 - 8.0 - 26.0 * k - 3.0 * k, rect.y0 + 36.0);
                    let side = (w - 30.0).min(rect.height() - 140.0);
                    let square = Rect::new(rect.x0 + 8.0, rect.y0 + 46.0, rect.x0 + 8.0 + side, rect.y0 + 46.0 + side);
                    let bar = Rect::new(square.x1 + 5.0, square.y0, square.x1 + 13.0, square.y1);
                    let top = square.y1 + 36.0;
                    let dials = if w >= 216.0 { vec![(Point::new(rect.x0 + 30.0, top), 40.0), (Point::new(rect.x0 + 30.0 + 104.0, top), 40.0)] } else { vec![(Point::new(rect.x0 + 30.0, top), 38.0), (Point::new(rect.x0 + 30.0, top + 52.0), 38.0)] };
                    Card { band, kind: CardKind::Narrow, rect, square, bar, hist: None, quads: Vec::new(), dets: Vec::new(), power: None, solo: Some(button(1.0)), mute: Some(button(0.0)), remove: None, ext: None, delta: None, dials }
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
        draw_title(cx, 52.0, 18.0, "Multiband");
        self.session.draw(cx);
    }

    /// The bands' live gain over frequency at each of `freqs`: each band's gain weighted by the
    /// crossover's own weights (`Layout::weights`), in power. The host is read once.
    fn response_db(&self, host: &dyn ParamHost, layout: &Layout, freqs: &[f64]) -> Vec<f64> {
        let x: [Option<f32>; CROSSOVERS] = std::array::from_fn(|k| Self::on(host, &cross_id(k, X_ON.id)).then(|| Self::plain(host, &cross_id(k, X_FREQ.id))));
        let heard = mite_dsp::multiband::audible(&settings(host), layout);
        let power: [f64; BANDS] = std::array::from_fn(|b| if heard[b] { 10f64.powf((f64::from(self.gain[b]) + if Self::on(host, &band_id(b, B_BYPASS.id)) { 0.0 } else { f64::from(Self::plain(host, &band_id(b, B_GAIN.id))) }) / 10.0) } else { 0.0 });
        freqs
            .iter()
            .map(|&f| {
                let w = layout.weights(&x, f);
                let p: f64 = w.iter().zip(&power).map(|(w, p)| w * p).sum();
                10.0 * p.max(1e-12).log10()
            })
            .collect()
    }

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
                let mut op = if b == current { 0.10 } else { 0.05 } + 0.004 * f64::from(self.gain[b].abs()).min(12.0) as f32;
                if soloed.is_some_and(|s| s != b) {
                    op *= 0.35;
                }
                tints.push((Rect::new(r.x0, area.y0, r.x1, area.y1), op));
            }
        }
        self.spectrum.draw(cx, area, &tints);
        let theme = cx.theme;
        if layout.n > 0 {
            let x = self.rail.crossovers(cx.host);
            for p in 0..layout.n {
                let px = self.rail.x(f64::from(x[layout.crossovers[p]].unwrap_or(20.0)));
                cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, theme.accent.multiply_alpha(0.35), None, &Line::new((px, area.y0), (px, area.y1)));
            }
        }
        // The dynamic response: 0 dB faint, the live curve lit.
        let y0 = area.y0 + area.height() * RESPONSE_ZERO;
        let k = area.height() / RESPONSE_SPAN_DB;
        cx.scene.stroke(&Stroke::new(1.0).with_dashes(0.0, [3.0, 4.0]), Affine::IDENTITY, theme.accent.multiply_alpha(0.3), None, &Line::new((area.x0, y0), (area.x1, y0)));
        let freqs: Vec<f64> = (0..=240).map(|i| 20.0 * 1000f64.powf(f64::from(i) / 240.0)).collect();
        let pts: Vec<Point> = freqs.iter().zip(self.response_db(cx.host, layout, &freqs)).map(|(&f, db)| Point::new(self.rail.x(f), y0 - db * k)).collect();
        let path = history_path(&pts);
        cx.begin_clip(&area);
        glow_stroke(cx, &path, 2.0, theme.accent, theme.accent, 3.5);
        cx.end_clip();
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
                let y = if card.kind == CardKind::Wide { card.rect.y0 + 27.0 } else { card.rect.y0 + 23.0 };
                draw_badge(cx, Point::new(card.rect.x0 + 9.0 + r + 2.0, y), r, p + 1, card.kind == CardKind::Wide, solo, alpha);
            }
            let stage = Self::stage(cx.host, b);
            for (q, &r) in card.quads.iter().enumerate() {
                let ink = icon_button(cx.scene, theme, r, theme.panel_control, hot(r), stage.on && !bypassed && stage.quadrant as usize == q, theme.accent);
                draw_svg(cx.scene, QUAD_GLYPHS[q], r.center(), 24.0, ink, 1.6);
            }
            for (d, &r) in card.dets.iter().enumerate() {
                let ink = icon_button(cx.scene, theme, r, theme.panel_control, hot(r), stage.detector as usize == d, theme.accent);
                draw_svg(cx.scene, DET_GLYPHS[d], r.center(), 22.0, ink, 1.6);
            }
            if let Some(r) = card.power {
                let ink = icon_button(cx.scene, theme, r, theme.panel_control, hot(r), !bypassed, theme.accent);
                draw_svg(cx.scene, POWER, r.center(), r.height() * 0.5, ink, 1.8);
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
            let external = Self::on(cx.host, SC_SOURCE.id);
            if let Some(r) = card.ext {
                // Lit while this band is keyed from the sidechain; dim while the source is internal.
                let on = Self::on(cx.host, &band_id(b, B_EXTERNAL.id));
                let ink = icon_button(cx.scene, theme, r, theme.panel_control, hot(r), on, theme.sidechain);
                draw_svg(cx.scene, KEY_GLYPH, r.center(), r.height() * 0.6, if external || !on { ink } else { ink.multiply_alpha(0.45) }, 1.7);
            }
            if let Some(r) = card.delta {
                let on = Self::on(cx.host, &band_id(b, B_DELTA.id));
                let ink = icon_button(cx.scene, theme, r, theme.panel_control, hot(r), on, theme.accent);
                draw_svg(cx.scene, DELTA_GLYPH, r.center(), r.height() * 0.55, ink, 1.6);
            }
            if let Some(r) = card.remove {
                let ink = icon_button(cx.scene, theme, r, theme.panel_control, hot(r), false, theme.text_hi);
                draw_glyph(cx.scene, rail_glyph::CROSS, r.center(), r.height() * 0.56, ink, 1.8);
            }
            let lit = if bypassed || !stage.on { 0.45 } else { 1.0 } * alpha;
            self.draw_square(cx, card, &stage, lit);
            if let Some(h) = card.hist {
                self.draw_band_history(cx, h, b, &stage, lit);
                // The band's range, in the history's corner.
                let text = if layout.n > 0 { self.rail.range_text(cx.host, layout, b) } else { "20 Hz – 20 kHz".to_owned() };
                cx.text.draw(cx.scene, &text, h.x1 - 10.0, h.y0 + 6.0, TextStyle { tabular: true, ..TextStyle::BODY.sized(11.0).weighted(600.0) }, theme.text_mid, Align::Right);
            }
        }
        for k in &self.knobs {
            k.draw(cx);
            if !k.value_inside {
                let v = Self::plain(cx.host, &k.id);
                let x = k.center.x + k.diameter / 2.0 + 7.0;
                cx.text.draw(cx.scene, k.label, x, k.center.y - 14.0, TextStyle::LABEL.sized(9.0), theme.text_lo, Align::Left);
                cx.text.draw(cx.scene, &k.spec.format(v), x, k.center.y - 1.0, TextStyle { tabular: true, ..TextStyle::BODY.weighted(600.0) }, theme.text_hi, Align::Left);
            }
        }
    }

    /// The band's transfer square (in → out over −60…0 dB, from `curve_db`), a live dot at its
    /// input peak, and its gain change as a bar beside it.
    fn draw_square(&self, cx: &mut DrawCx<'_>, card: &Card, stage: &Stage, lit: f32) {
        let theme = cx.theme;
        let r = card.square;
        let b = card.band;
        panel(cx.scene, RoundedRect::from_rect(r, 8.0), theme.well, theme.well_border);
        let sx = |db: f32| r.x0 + f64::from((db + SQ_RANGE) / SQ_RANGE) * r.width();
        let sy = |db: f32| r.y1 - f64::from((db + SQ_RANGE) / SQ_RANGE) * r.height();
        for db in [-12.0, -24.0, -36.0, -48.0] {
            cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, theme.grid_minor, None, &Line::new((sx(db), r.y0), (sx(db), r.y1)));
            cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, theme.grid_minor, None, &Line::new((r.x0, sy(db)), (r.x1, sy(db))));
        }
        cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, theme.grid_major, None, &Line::new((sx(-SQ_RANGE), sy(-SQ_RANGE)), (sx(0.0), sy(0.0))));
        // A bypassed band passes untouched: its square is the diagonal, its gain 0 dB.
        let bypassed = Self::on(cx.host, &band_id(b, B_BYPASS.id));
        let stage = &Stage { on: stage.on && !bypassed, ..*stage };
        let mut stages = [Stage::default(); STAGES];
        stages[0] = *stage;
        let gain = if bypassed { 0.0 } else { Self::plain(cx.host, &band_id(b, B_GAIN.id)) };
        let pts: Vec<Point> = (0..=120)
            .map(|i| {
                let x = -SQ_RANGE + SQ_RANGE * i as f32 / 120.0;
                let y = if stage.on { curve_db(&stages, x, 0.0) } else { x } + gain;
                Point::new(sx(x), r.y1 - f64::from((y + SQ_RANGE) / SQ_RANGE) * r.height())
            })
            .collect();
        let curve = history_path(&pts);
        cx.begin_clip(&RoundedRect::from_rect(r, 8.0));
        let color = theme.accent.multiply_alpha(lit);
        glow_stroke(cx, &curve, if card.kind == CardKind::Narrow { 1.6 } else { 2.0 }, color, theme.accent.multiply_alpha(0.6 * lit), 3.5);
        // The live dot at the band's input peak, on the curve.
        let level = mite_dsp::gain_to_db(self.peak[b]);
        if level > -SQ_RANGE {
            let out = if stage.on { curve_db(&stages, level, 0.0) } else { level } + gain;
            let dot = Point::new(sx(level), r.y1 - f64::from((out + SQ_RANGE) / SQ_RANGE) * r.height());
            cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.text_strong.multiply_alpha(lit), None, &Circle::new(dot, if card.kind == CardKind::Narrow { 3.5 } else { 5.0 }));
            cx.glow().fill(Fill::NonZero, Affine::IDENTITY, theme.text_strong.multiply_alpha(0.5 * lit), None, &Circle::new(dot, 7.0));
        }
        cx.end_clip();
        // The gain change: down from the middle (cut) or up from it (lift), ±24 dB.
        let bar = card.bar;
        panel(cx.scene, RoundedRect::from_rect(bar, 3.0), theme.meter_well, theme.well_border);
        let mid = bar.center().y;
        let g = f64::from(self.gain[b]).clamp(-BAR_DB, BAR_DB);
        if g.abs() > 0.05 {
            let y = mid - g / BAR_DB * (bar.height() / 2.0 - 2.0);
            let fill = Rect::new(bar.x0 + 2.0, mid.min(y), bar.x1 - 2.0, mid.max(y));
            let color = if g < 0.0 { theme.gain_reduction } else { theme.gain_up };
            cx.scene.fill(Fill::NonZero, Affine::IDENTITY, color.multiply_alpha(lit), None, &RoundedRect::from_rect(fill, 2.0));
        }
        cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, theme.grid_zero, None, &Line::new((bar.x0, mid), (bar.x1, mid)));
    }

    /// The band's history (the shared lane, honest peaks): its input dim, the gain change from
    /// the top (a lift from the bottom) at the levels' own scale, its detector dashed when that is
    /// not its peak, and the threshold as a line to drag.
    fn draw_band_history(&self, cx: &mut DrawCx<'_>, r: Rect, b: usize, stage: &Stage, lit: f32) {
        let theme = cx.theme;
        panel(cx.scene, RoundedRect::from_rect(r, 8.0), theme.well, theme.well_border);
        let lane = HistoryLane { x0: r.x0, width: r.width(), top: r.y0, bottom: r.y1, seconds: HIST_SECONDS, slice_ms: HISTORY_SLICE_MS };
        let y = |db: f32| hist_y(r, db);
        cx.begin_clip(&RoundedRect::from_rect(r, 8.0));
        lane.draw_grid(cx, HIST_RANGE, 0.0, y);
        let cols = lane.columns(&self.history, self.seen, self.since_fresh());
        if cols.len() >= 2 {
            type Col = history::Column<{ record::WIDTH }>;
            let line = |f: &dyn Fn(&Col) -> f64| history::column_line(&cols, f);
            let input = line(&|c| y(c.high[record::BAND_IN + b]));
            cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.spectrum_pre.multiply_alpha(0.16 * lit), None, &history::close_to(input.clone(), &cols, r.y1));
            let k = hist_px_per_db(r);
            if cols.iter().any(|c| c.low[record::BAND_GAIN + b] < -0.05) {
                let edge = line(&|c| r.y0 + (f64::from(-c.low[record::BAND_GAIN + b].min(0.0)) * k).min(r.height()));
                history::draw_reduction(cx, &cols, &edge, r.y0, 0.5 * r.height());
            }
            if cols.iter().any(|c| c.high[record::BAND_GAIN + b] > 0.05) {
                let edge = line(&|c| r.y1 - (f64::from(c.high[record::BAND_GAIN + b].max(0.0)) * k).min(r.height()));
                glow_stroke(cx, &edge, 1.6, theme.gain_up.multiply_alpha(lit), theme.gain_up, 2.4);
            }
            // A bypassed band's detector is idle: no line for it.
            if stage.detector != Detector::Peak && stage.on && !Self::on(cx.host, &band_id(b, B_BYPASS.id)) {
                let det = line(&|c| y(c.high[record::BAND_DET + b]));
                cx.scene.stroke(&Stroke::new(1.3).with_dashes(0.0, [6.0, 3.0]), Affine::IDENTITY, theme.text_hi.multiply_alpha(0.7 * lit), None, &det);
            }
            cx.scene.stroke(&Stroke::new(1.2), Affine::IDENTITY, theme.spectrum_pre.multiply_alpha(0.85 * lit), None, &input);
        }
        // The threshold, on the band's own level.
        let hot = matches!(self.hot, Some(Hot::Hist(hb, true)) if hb == b) || matches!(self.drag, Some(Drag::Thresh { band, .. }) if band == b);
        let ty = y(stage.threshold_db);
        glow_stroke(cx, &Line::new((r.x0, ty), (r.x1, ty)), if hot { 2.0 } else { 1.4 }, theme.accent.multiply_alpha(lit), theme.accent, if hot { 4.0 } else { 2.0 });
        cx.end_clip();
    }

    fn since_fresh(&self) -> f64 {
        (self.last_fresh.elapsed().as_secs_f64() * 1000.0 / f64::from(HISTORY_SLICE_MS)).min(4.0)
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
            let color = if self.meters[i] > 0.0 { theme.danger } else if i < 2 { theme.spectrum_pre } else { theme.curve };
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
        let external = Self::on(cx.host, SC_SOURCE.id);
        draw_dropdown_button(cx, SOURCE_BUTTON, if external { "External" } else { "Internal" }, self.source_menu.is_some(), self.hot == Some(Hot::Source), |scene, at| draw_svg(scene, KEY_GLYPH, at + (8.0, 6.0), 20.0, theme.sidechain, 1.7));
        let eq_on = (0..mite_dsp::multiband::KEY_BANDS).any(|b| Self::on(cx.host, &key_id(b, K_ON.id)));
        let ink = icon_button(cx.scene, theme, EQ_BUTTON, theme.panel_control, self.hot == Some(Hot::EqButton), self.eq_open || eq_on, theme.sidechain);
        draw_svg(cx.scene, EQ_GLYPH, EQ_BUTTON.center(), 22.0, ink, 1.6);
        let sample_rate = f64::from(self.shared.sample_rate.load()).max(1.0);
        self.footer.draw_latency(cx, 220.0, f64::from(self.shared.latency.load()) / sample_rate * 1000.0);
        self.footer.draw_controls(cx);
        if let Some(list) = &self.source_menu {
            list.draw(cx, theme.accent, |_, _, _, _| {});
        }
    }

    /// The sidechain EQ over the cards (opaque), with the key's spectrum under its curve.
    fn draw_eq_panel(&self, cx: &mut DrawCx<'_>) {
        if !self.eq_open {
            return;
        }
        let theme = cx.theme;
        mite_ui::paint::raised(cx.scene, RoundedRect::from_rect(EQ_PANEL, 12.0), theme.window, theme.panel_control_border, theme.shadows);
        draw_svg(cx.scene, KEY_GLYPH, Point::new(EQ_PANEL.x0 + 28.0, EQ_PANEL.y0 + 22.0), 20.0, theme.sidechain, 1.7);
        let words = if Self::on(cx.host, SC_SOURCE.id) { "Sidechain" } else { "Sidechain — the source is Internal: pick External for the bands' key buttons to hear it" };
        cx.text.draw(cx.scene, words, EQ_PANEL.x0 + 46.0, EQ_PANEL.y0 + 15.0, TextStyle::BODY.weighted(700.0), theme.text_hi, Align::Left);
        self.key_eq.draw_with(cx, Some(&self.key_spectrum));
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

    fn help_text(&self, host: &dyn ParamHost) -> Help {
        let layout = Self::layout(host);
        let number = |b: usize| layout.position(b).map_or(1, |p| p + 1);
        if let Some(Drag::Thresh { band, .. }) = self.drag {
            let t = Self::plain(host, &band_id(band, B_THRESH.id));
            return Help::default().text("Threshold ").slot(format!("{t:.1} dB"), "−60.0 dB").text(" — where the band's level (dashed: its detector) crosses this line, its dynamics act. Shift is fine.");
        }
        if let Some(Drag::Square { band, .. }) = self.drag {
            let s = Self::stage(host, band);
            return Help::default().text("Threshold ").slot(format!("{:.1} dB", s.threshold_db), "−60.0 dB").text("  ratio ").slot(format!("{:.1}:1", s.ratio), "100.0:1").text(" — across: threshold, up and down: ratio. Shift is fine.");
        }
        if self.key_eq.menu_open() {
            if let Some(h) = self.key_eq.help(host) {
                return h;
            }
        }
        if let Some(list) = &self.source_menu {
            if let Some(i) = self.pointer.and_then(|p| list.item_rects().iter().position(|r| r.contains(p))) {
                return Help::default().text(if i == 0 { "Internal — every band hears itself" } else { "External — bands with their key lit hear the sidechain (route a track to the plugin's second input)" });
            }
        }
        if let Some(h) = self.rail.help(host) {
            return h;
        }
        match self.hot {
            Some(Hot::Square(b) | Hot::Hist(b, _)) if Self::on(host, &band_id(b, B_BYPASS.id)) => {
                return Help::default().text(format!("Band {} is bypassed — it passes untouched, so nothing here moves. Its power button gives it its dynamics back.", number(b)));
            }
            Some(Hot::Square(b)) => {
                let s = Self::stage(host, b);
                return Help::default().text("Threshold ").slot(format!("{:.1} dB", s.threshold_db), "−60.0 dB").text("  ratio ").slot(format!("{:.1}:1", s.ratio), "100.0:1").text(" — drag across for the threshold, up and down for the ratio; wheel: threshold; double-click resets.");
            }
            Some(Hot::Hist(b, line)) => {
                let s = Self::stage(host, b);
                let what = if s.detector == Detector::Peak { "the band's peaks" } else { "the band's detector (dashed)" };
                return Help::default().text("Band history · threshold ").slot(format!("{:.1} dB", s.threshold_db), "−60.0 dB").text(format!(" — where {what} cross the line, it acts; the gain change hangs from the top at the same scale. {}", if line { "Drag the line." } else { "Drag the mint line for the threshold." }));
            }
            Some(Hot::Quad(_, q)) => return Help::default().text(format!("{} — {}", Quadrant::NAMES[q], quad_words(q))),
            Some(Hot::Det(_, d)) => return Help::default().text(format!("{} — {}", Detector::NAMES[d], det_words(d))),
            Some(Hot::Power(b)) => return Help::default().text(if Self::on(host, &band_id(b, B_BYPASS.id)) { "The band passes untouched — click to give it its dynamics back." } else { "Band on — click to let it pass untouched (still heard)." }),
            Some(Hot::Solo(_)) => return Help::default().text("Solo — hear only this band (S)"),
            Some(Hot::Mute(_)) => return Help::default().text("Mute — take this band out of the sum (M)"),
            Some(Hot::Remove(b)) => return Help::default().text(format!("Remove band {} — its region joins the band below (Delete)", number(b))),
            Some(Hot::Ext(b)) => {
                let words = if !Self::on(host, SC_SOURCE.id) { " (the sidechain source is Internal: pick External in the footer)" } else { "" };
                return Help::default().text(format!("External key — band {} hears the sidechain, EQ'd and split like the audio, instead of itself{words}", number(b)));
            }
            Some(Hot::BandDelta(b)) => return Help::default().text(format!("Band Delta — hear only what band {}'s dynamics change", number(b))),
            Some(Hot::Source) => return Help::default().text("Sidechain — Internal: every band hears itself. External: bands with their key lit hear the sidechain input."),
            Some(Hot::EqButton) => return Help::default().text(if self.eq_open { "Sidechain EQ — click to close" } else { "Sidechain EQ — shape the sidechain the external bands hear, over its spectrum" }),
            Some(Hot::EqPanel) => return self.key_eq.help(host).unwrap_or_default(),
            Some(Hot::Rail(RailHot::Region(b))) | Some(Hot::Card(b)) if layout.n > 0 => {
                let rest = if Self::on(host, &band_id(b, B_BYPASS.id)) { " — bypassed: it passes untouched; its power button gives it its dynamics back" } else { " — click to edit it; double-click the strip to split; right-click for more" };
                return Help::default().slot(format!("Band {}", number(b)), "Band 6").text(" · ").slot(self.rail.range_text(host, &layout, b), "20.0 kHz – 20.0 kHz").text(rest);
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
        if layout.n == 0 {
            Help::default().text("One band: pull a crossover in from either end of the strip, or double-click it, to split the sound into bands.")
        } else {
            Help::default().text("Mint on the spectrum: what the bands' dynamics do to it right now. Drag a crossover sideways; click a band to edit it.")
        }
    }

    // ---- events -----------------------------------------------------------------------------

    fn sync_knobs(&mut self, host: &dyn ParamHost) {
        let layout = Self::layout(host);
        let cards = self.cards(&layout);
        let key = (layout.bands().to_vec(), self.rail.current(&layout));
        if self.knob_key.as_ref() != Some(&key) && !self.knobs.iter().any(Knob::is_dragging) {
            self.knobs.clear();
            for card in &cards {
                let b = card.band;
                if card.kind == CardKind::Narrow {
                    for (k, (spec, label)) in [(B_THRESH, "THRESH"), (B_GAIN, "GAIN")].into_iter().enumerate() {
                        let (c, d) = card.dials[k];
                        let mut knob = Knob::new(spec, label, if k == 1 { KnobKind::Bipolar } else { KnobKind::Unipolar }, c, d).with_id(band_id(b, spec.id));
                        knob.value_inside = false;
                        self.knobs.push(knob);
                    }
                } else {
                    for (k, (spec, label, kind)) in DIALS.iter().enumerate() {
                        let (c, d) = card.dials[k];
                        let knob = Knob::new(*spec, label, *kind, c, d).with_id(band_id(b, spec.id));
                        self.knobs.push(match spec.id {
                            "attack" => knob.with_curve(ATTACK_DIAL),
                            "release" => knob.with_curve(RELEASE_DIAL),
                            _ => knob,
                        });
                    }
                }
            }
            self.knob_key = Some(key);
        }
        let mut centres = cards.iter().flat_map(|c| c.dials.iter().copied());
        for k in &mut self.knobs {
            if let Some((c, _)) = centres.next() {
                k.center = c;
            }
        }
    }

    fn busy(&self) -> bool {
        self.drag.is_some() || self.key_eq.busy() || self.key_eq.menu_open() || self.rail.busy() || self.knobs.iter().any(Knob::is_dragging) || self.footer.busy()
    }

    fn hot_at(&self, host: &dyn ParamHost, p: Point) -> Option<Hot> {
        if self.eq_open && EQ_PANEL.contains(p) {
            return Some(Hot::EqPanel);
        }
        if SOURCE_BUTTON.contains(p) {
            return Some(Hot::Source);
        }
        if EQ_BUTTON.contains(p) {
            return Some(Hot::EqButton);
        }
        if let Some(h) = self.rail.hit(host, p) {
            return Some(Hot::Rail(h));
        }
        for card in self.cards(&Self::layout(host)) {
            if !card.rect.contains(p) {
                continue;
            }
            if card.square.contains(p) {
                return Some(Hot::Square(card.band));
            }
            if let Some(h) = card.hist.filter(|h| h.contains(p)) {
                let on_line = (p.y - hist_y(h, Self::plain(host, &band_id(card.band, B_THRESH.id)))).abs() < 8.0;
                return Some(Hot::Hist(card.band, on_line));
            }
            if let Some(q) = card.quads.iter().position(|r| r.contains(p)) {
                return Some(Hot::Quad(card.band, q));
            }
            if let Some(d) = card.dets.iter().position(|r| r.contains(p)) {
                return Some(Hot::Det(card.band, d));
            }
            if let Some((_, h)) = card.buttons().into_iter().find(|(r, _)| r.is_some_and(|r| r.contains(p))) {
                return Some(h);
            }
            return Some(Hot::Card(card.band));
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
            Key::Character('s') | Key::Character('S') if layout.n > 0 => {
                self.rail.toggle_solo(host, b);
                true
            }
            Key::Character('m') | Key::Character('M') if layout.n > 0 => {
                self.rail.toggle_mute(host, b);
                true
            }
            Key::Delete | Key::Backspace if layout.n > 0 => {
                self.rail.remove_band(host, b);
                true
            }
            Key::Up | Key::Down => {
                let step = if mods.fine() { 0.1 } else { 1.0 } * if key == Key::Up { 1.0 } else { -1.0 };
                let id = band_id(b, B_THRESH.id);
                Self::set(host, &id, (Self::plain(host, &id) + step).clamp(B_THRESH.min, B_THRESH.max));
                true
            }
            Key::Escape => {
                if self.menu.take().is_some() {
                    return true;
                }
                if self.eq_open {
                    self.eq_open = false;
                    return true;
                }
                self.rail.cancel() || self.rail.clear_solo(host)
            }
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

    fn press(&mut self, host: &dyn ParamHost, pos: Point) -> bool {
        match self.hot_at(host, pos) {
            Some(Hot::Rail(_)) => self.rail.press(host, pos),
            Some(Hot::Square(b)) => {
                self.rail.selected = b;
                for id in [B_THRESH.id, B_RATIO.id] {
                    host.begin_gesture(&band_id(b, id));
                }
                self.drag = Some(Drag::Square { band: b, start: pos, thresh: Self::plain(host, &band_id(b, B_THRESH.id)), ratio: Self::plain(host, &band_id(b, B_RATIO.id)) });
                true
            }
            Some(Hot::Quad(b, q)) => {
                // Picking a quadrant also turns the band's dynamics on; the lit one again: off.
                let s = Self::stage(host, b);
                if s.on && s.quadrant as usize == q {
                    Self::set(host, &band_id(b, B_ON.id), 0.0);
                } else {
                    Self::set(host, &band_id(b, B_QUAD.id), q as f32);
                    Self::set(host, &band_id(b, B_ON.id), 1.0);
                }
                true
            }
            Some(Hot::Det(b, d)) => {
                Self::set(host, &band_id(b, B_DET.id), d as f32);
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
            Some(Hot::Card(b)) | Some(Hot::Hist(b, false)) => {
                self.rail.selected = b;
                true
            }
            Some(Hot::Hist(b, true)) => {
                host.begin_gesture(&band_id(b, B_THRESH.id));
                self.drag = Some(Drag::Thresh { band: b, start: pos, thresh: Self::plain(host, &band_id(b, B_THRESH.id)) });
                true
            }
            Some(Hot::Ext(b)) => {
                let id = band_id(b, B_EXTERNAL.id);
                Self::set(host, &id, if Self::on(host, &id) { 0.0 } else { 1.0 });
                true
            }
            Some(Hot::BandDelta(b)) => {
                let id = band_id(b, B_DELTA.id);
                Self::set(host, &id, if Self::on(host, &id) { 0.0 } else { 1.0 });
                true
            }
            Some(Hot::Source) => {
                let current = usize::from(Self::on(host, SC_SOURCE.id));
                self.source_menu = Some(Dropdown::new(SOURCE_BUTTON, SC_SOURCE.choices, current, (DISPLAY_TOP, FOOTER_TOP)).with_min_width(170.0));
                true
            }
            Some(Hot::EqButton) => {
                self.eq_open = !self.eq_open;
                true
            }
            Some(Hot::EqPanel) => true,
            None => false,
        }
    }

    fn handle_event(&mut self, event: &UiEvent, host: &dyn ParamHost) -> bool {
        if let UiEvent::KeyDown { key, mods } = *event {
            return self.key_event(key, mods, host);
        }
        if self.menu.is_some() && !self.session.menu_open() {
            return self.menu_event(event, host);
        }
        if let Some(list) = &mut self.source_menu {
            if let UiEvent::PointerMove { pos, .. } = *event {
                self.pointer = Some(pos);
            }
            match list.event(event) {
                DropdownEvent::Open => {}
                DropdownEvent::Picked(i) => {
                    self.source_menu = None;
                    Self::set(host, SC_SOURCE.id, i as f32);
                }
                DropdownEvent::Closed => self.source_menu = None,
            }
            return true;
        }
        if self.eq_open {
            let in_panel = |p: Point| EQ_PANEL.contains(p);
            let ours = match *event {
                UiEvent::PointerDown { pos, .. } | UiEvent::DoubleClick { pos, .. } | UiEvent::Wheel { pos, .. } => in_panel(pos),
                _ => true,
            };
            if (ours || self.key_eq.menu_open()) && self.key_eq.event(event, host) {
                return true;
            }
            if let UiEvent::PointerDown { pos, .. } | UiEvent::DoubleClick { pos, .. } | UiEvent::Wheel { pos, .. } = *event {
                if in_panel(pos) {
                    return true;
                }
            }
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
                if let Some(Drag::Thresh { band, start, thresh }) = self.drag {
                    let k = if mods.fine() { 0.2 } else { 1.0 };
                    let r = self.card(host, band).and_then(|c| c.hist).unwrap_or(Rect::new(0.0, 0.0, 1.0, 100.0));
                    let d = ((start.y - pos.y) / hist_px_per_db(r)) as f32;
                    Self::set_raw(host, &band_id(band, B_THRESH.id), B_THRESH.snap((thresh + d * k).clamp(B_THRESH.min, B_THRESH.max)));
                    return true;
                }
                if let Some(Drag::Square { band, start, thresh, ratio }) = self.drag {
                    let k = if mods.fine() { 0.2 } else { 1.0 };
                    let (dx, dy) = ((pos.x - start.x) as f32, (start.y - pos.y) as f32);
                    Self::set_raw(host, &band_id(band, B_THRESH.id), (thresh + dx * THRESH_PER_PX * k).clamp(B_THRESH.min, B_THRESH.max));
                    Self::set_raw(host, &band_id(band, B_RATIO.id), (ratio * 2f32.powf(dy * RATIO_OCT_PER_PX * k)).clamp(B_RATIO.min, B_RATIO.max));
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
            UiEvent::PointerDown { pos, button: MouseButton::Left, .. } => self.press(host, pos),
            UiEvent::PointerDown { pos, button: MouseButton::Right, .. } => match self.hot_at(host, pos) {
                Some(Hot::Rail(RailHot::Region(b)) | Hot::Card(b) | Hot::Square(b)) => {
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
                    Some(Drag::Square { band, .. }) => {
                        for id in [B_THRESH.id, B_RATIO.id] {
                            host.end_gesture(&band_id(band, id));
                        }
                        true
                    }
                    Some(Drag::Thresh { band, .. }) => {
                        host.end_gesture(&band_id(band, B_THRESH.id));
                        true
                    }
                    None => false,
                }
            }
            UiEvent::DoubleClick { pos, .. } => match self.hot_at(host, pos) {
                Some(Hot::Rail(_)) => self.rail.double_click(host, pos),
                Some(Hot::Square(b)) => {
                    self.drag = None;
                    Self::set(host, &band_id(b, B_THRESH.id), B_THRESH.default);
                    Self::set(host, &band_id(b, B_RATIO.id), B_RATIO.default);
                    true
                }
                _ => false,
            },
            UiEvent::Wheel { pos, lines, mods } => match self.hot_at(host, pos) {
                Some(Hot::Rail(_)) => self.rail.wheel(host, pos, lines, mods),
                Some(Hot::Square(b)) => {
                    let id = band_id(b, B_THRESH.id);
                    Self::set(host, &id, B_THRESH.wheel(Self::plain(host, &id), lines, mods.fine()));
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

/// A monotone curve through `pts` (no corners, no overshoot).
fn history_path(pts: &[Point]) -> BezPath {
    mite_ui::widgets::history::monotone_path(pts)
}

fn quad_words(q: usize) -> &'static str {
    match q {
        0 => "turns the band down above the threshold",
        1 => "turns the band down further below the threshold (a gentle gate)",
        2 => "brings quiet parts of the band up below the threshold",
        _ => "lifts the band above the threshold (more punch)",
    }
}

fn det_words(d: usize) -> &'static str {
    match d {
        0 => "instant attack: catches every hit in the band",
        1 => "the band's power over 10 ms: steadier",
        _ => "K-weighted over 50 ms: as loudness is heard",
    }
}

fn knob_words(id: &str) -> &'static str {
    match id {
        "thresh" => " — where the band's dynamics start, on its own level",
        "ratio" => " — how strongly",
        "knee" => " — how softly it eases in around the threshold",
        "range" => " — the most it may move the band",
        "gain" => " — the band's level in the sum",
        "attack" => " — how fast it acts",
        "release" => " — how fast it lets go",
        _ => "",
    }
}

impl View for MbView {
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
        self.draw_spectrum(cx, &layout, l.spectrum);
        let breath: [f32; BANDS] = std::array::from_fn(|b| (self.gain[b].abs() / 12.0).min(1.0));
        self.rail.draw_under(cx, &layout, &breath);
        self.rail.draw_over(cx, &layout);
        let cards = self.cards(&layout);
        if self.eq_open {
            // Wider than the panel by a halo's reach: the cards' glowing frames end at its edge.
            cx.begin_glow_hole(RoundedRect::from_rect(EQ_PANEL.inflate(10.0, 10.0), 20.0));
        }
        self.rail.draw_funnels(cx, &layout, &cards.iter().map(|c| c.rect).collect::<Vec<_>>(), l.rail.y1, l.cy);
        self.draw_cards(cx, &layout, &cards);
        if self.eq_open {
            cx.end_glow_hole();
        }
        self.draw_eq_panel(cx);
        self.draw_meters(cx);
        self.draw_states(cx, l.spectrum);
        self.help_text(cx.host).draw(cx, Rect::new(0.0, HELP_TOP, WIDTH, FOOTER_TOP));
        self.draw_footer(cx);
        if let Some(menu) = &self.menu {
            menu.draw(cx, (theme.text_hi, rail_glyph::CROSS));
        }
        self.key_eq.draw_menu(cx);
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
            if history.len() == HISTORY_RECORDS {
                history.pop_front();
            }
            history.push_back(*r);
            fresh = true;
        });
        if fresh {
            self.last_fresh = Instant::now();
        }
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
        let k = |s: f64| (1.0 - (-dt / s).exp()) as f32;
        let mut moved = false;
        for b in 0..BANDS {
            let peak_db = self.history.iter().rev().take(4).map(|r| r[record::BAND_IN + b]).fold(-150.0f32, f32::max);
            let target = if peak_db > -120.0 { 10f32.powf(peak_db / 20.0) } else { 0.0 };
            let before = (self.peak[b], self.gain[b]);
            self.peak[b] += (target - self.peak[b]) * if target > self.peak[b] { k(PEAK_RISE_S) } else { k(PEAK_FALL_S) };
            if let Some(r) = self.history.back() {
                self.gain[b] += (r[record::BAND_GAIN + b] - self.gain[b]) * k(0.05);
            }
            moved |= (self.peak[b] - before.0).abs() > 1e-4 || (self.gain[b] - before.1).abs() > 1e-3;
        }
        let spectrum = self.spectrum.update(&self.shared, dt) | (self.eq_open && self.key_spectrum.update(&self.shared.key_spectrum, f64::from(self.shared.sample_rate.load()), dt));
        let flowing = self.last_fresh.elapsed().as_secs_f32() < 0.25;
        fresh || flowing || moved || grew || spectrum || self.busy() || self.drag.is_some() || self.meters.iter().any(|&m| m > -149.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inert_bands_draw_a_flat_response() {
        // With every band's gain at 0 dB the crossover's weights sum to one: the response is 0 dB.
        let host = memory_host();
        for k in 0..3 {
            host.set_plain(&cross_id(k, X_ON.id), 1.0);
        }
        let view = MbView::new(Arc::new(MbShared::new()));
        let layout = MbView::layout(&host);
        for (f, db) in [30.0, 120.0, 500.0, 2000.0, 9000.0].iter().zip(view.response_db(&host, &layout, &[30.0, 120.0, 500.0, 2000.0, 9000.0])) {
            assert!(db.abs() < 0.05, "{f} Hz: {db}");
        }
    }

    #[test]
    fn a_bypassed_bands_gain_is_not_drawn() {
        // Bypass passes the band untouched (its gain too), so the response stays flat there.
        let host = memory_host();
        host.set_plain(&cross_id(0, X_ON.id), 1.0);
        host.set_plain(&band_id(0, B_GAIN.id), 6.0);
        host.set_plain(&band_id(0, B_BYPASS.id), 1.0);
        let view = MbView::new(Arc::new(MbShared::new()));
        let layout = MbView::layout(&host);
        let db = view.response_db(&host, &layout, &[30.0])[0];
        assert!(db.abs() < 0.05, "{db}");
        host.set_plain(&band_id(0, B_BYPASS.id), 0.0);
        let db = view.response_db(&host, &layout, &[30.0])[0];
        assert!((db - 6.0).abs() < 0.2, "{db}");
    }
}
