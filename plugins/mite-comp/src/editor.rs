//! The Mite Comp editor (docs/design/mockups/comp-main.dc.html; interaction.md, "Mite Comp"),
//! in the mockup's 1280×760 design units.
//!
//! The display: the GR history on the left (the hero), the transfer square on the right with the
//! stage strip under it, then the IN, GR and OUT meters. Stages are made by pulling the curve off
//! the diagonal; their quadrant comes from the pull. Everything the square, the history and the
//! strip draw comes from `mite_dsp::dynamics` (the same curve the audio runs) and the engine's
//! history records, so the pictures are honest.

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Instant;

use mite_core::{ParamDisplay, ParamScale, ParamSpec};
use mite_dsp::dynamics::{curve_db, level_before, level_thresholds, order, partial_curve_db, simulate_stage, Detector, AUTO_SLOW_FACTOR, Quadrant, Smart, Stage, CHARACTERS, CUSTOM, INF_RATIO, KEY_BANDS, RATIO_MAX, STAGES, STYLES};
use mite_dsp::eq::{analog_response_db, BandSettings, BandType};
use mite_dsp::oversample::Oversampler;
use mite_ui::kurbo::{Affine, BezPath, Cap, Circle, Join, Line, Point, Rect, RoundedRect, Stroke};
use mite_ui::paint::{glass_panel, glow_stroke, icon_button, panel};
use mite_ui::peniko::{Color, Fill, Gradient};
use mite_ui::session::{Session, SessionConfig};
use mite_ui::widgets::brand::{draw_mark, draw_title};
use mite_ui::widgets::footer::draw_dropdown_button;
use mite_ui::widgets::{ease_range, Dropdown, DropdownEvent, Footer, FooterDial, FooterToggle, Help, Knob, KnobKind, RangePicker, ToggleGlyph};
use mite_ui::{write_if_changed, Align, DrawCx, Key, Modifiers, MouseButton, ParamHost, RecordingHost, TextStyle, UiEvent, View};

use crate::dsp::record;
use crate::params::*;
use crate::presets::{preset_ids, FACTORY, PLUGIN};
use crate::CompShared;

/// Mite Comp wears the Comp accent (tokens.json `color.accent.comp`, ember).
pub const ACCENT: &str = "comp";

const WIDTH: f64 = 1280.0;
const HEIGHT: f64 = 760.0;
const HEADER: f64 = 56.0;
const DISPLAY_TOP: f64 = HEADER;
const DISPLAY_H: f64 = 620.0;
const HELP_TOP: f64 = DISPLAY_TOP + DISPLAY_H;
const FOOTER_TOP: f64 = HELP_TOP + 28.0;
const HEADER_ROW: (f64, f64) = (12.0, 44.0);

/// History: 6 s across the left 776 px; levels −60…0 dB over the display; GR hangs from the top
/// at 10 px/dB (to 25 dB), upward gain rises from the bottom (to 16 dB).
const HIST_W: f64 = 776.0;
const HIST_SECONDS: f64 = 6.0;
const HIST_COLUMNS: usize = 300;
/// GR px per dB at the full 60 dB level range (it zooms with the range).
const GR_PX_AT_60: f64 = 10.0;
/// The history's level range (dB below 0 shown): 24/36/48/60 or Auto, as FabFilter's. Each view
/// sets the range it shows before drawing or handling events.
const LEVEL_RANGES: &[f32] = &[24.0, 36.0, 48.0, 60.0];
const LEVEL_RANGE_LABELS: &[&str] = &["Auto", "24 dB", "36 dB", "48 dB", "60 dB"];
thread_local! {
    static LEVEL_RANGE: std::cell::Cell<f64> = const { std::cell::Cell::new(60.0) };
}

fn set_level_range(range: f32) {
    LEVEL_RANGE.with(|r| r.set(f64::from(range)));
}

fn level_range() -> f64 {
    LEVEL_RANGE.with(std::cell::Cell::get)
}

fn gr_px_per_db() -> f64 {
    GR_PX_AT_60 * 60.0 / level_range()
}
/// Threshold lines end in a grip at the history's right edge.
const GRIP_X: f64 = 738.0;
const GRIP_SIZE: (f64, f64) = (34.0, 22.0);

/// Transfer square: −60…0 dB in and out.
const SQ_X: f64 = 796.0;
const SQ_Y: f64 = DISPLAY_TOP + 16.0;
const SQ: f64 = 384.0;
/// A node sits 12 dB into its stage's active side (where the ratio shows, not on the knee).
const NODE_OFFSET_DB: f32 = 12.0;
const NODE_HIT: f64 = 16.0;
/// Pulling the line: a stage forms past 6 px; 4:1 at 48 px, ∞:1 at 160 px.
const PULL_START: f64 = 6.0;
const PULL_4: f64 = 48.0;
const PULL_INF: f64 = 160.0;

/// Stage strip under the square, and the meters.
const STRIP_TOP: f64 = DISPLAY_TOP + 420.0;
const STRIP_H: f64 = 176.0;
const METER_TOP: f64 = DISPLAY_TOP + 20.0;
const METER_BOTTOM: f64 = DISPLAY_TOP + 592.0;
/// Meter release (tokens.json `meter.releaseDbPerSec`).
const METER_RELEASE_DB_S: f32 = 20.0;

/// Docked panels: bottom-left over the history, never over the square.
const PANEL_X: f64 = 12.0;
const STAGE_PANEL: Rect = Rect::new(PANEL_X, DISPLAY_TOP + 402.0, PANEL_X + 708.0, DISPLAY_TOP + 610.0);
const WIDE_PANEL: Rect = Rect::new(PANEL_X, DISPLAY_TOP + 378.0, PANEL_X + 760.0, DISPLAY_TOP + 610.0);
const STEREO_POP: Rect = Rect::new(870.0, DISPLAY_TOP + 8.0, 1106.0, DISPLAY_TOP + 120.0);
/// Comp's own header buttons, after the snapshot letters.
const HEAD_BUTTONS_X: f64 = 760.0;
const HEAD_BUTTON_W: f64 = 40.0;

/// Glyphs (24×24, stroked; docs/design/mockups/comp-main.dc.html `G`).
mod glyph {
    pub const COMP: &str = "M3 21 L12 12 C14.5 9.5 16.5 9.2 21 8.4";
    pub const EXP: &str = "M6 23 C8 21 9.5 16 12 12 L21 3";
    pub const UP: &str = "M3 14.5 C6 14 9.5 13.2 12 12 L21 3";
    pub const UPX: &str = "M3 21 L12 12 C13.8 10.2 14.6 6 15 2.5";
    pub const CHARACTERS: [&str; 7] = [
        "M3 16 H8 C10.5 16 11 10 13.5 10 H21",
        "M3 18 H6.5 C7.5 18 8 5 9.5 5 C11 5 11.5 14 13 14 H21",
        "M3 18 H6 L7.5 6 L10 6 L11 13 H21 M14 9 L16 7 L18 9",
        "M12 3.5 A6 6 0 0 1 15.5 14.4 V17 H8.5 V14.4 A6 6 0 0 1 12 3.5 Z M9 20.5 H15",
        "M3 6 C10 6 10 12 21 12 M3 18 C10 18 10 12 21 12 M3 12 H8",
        "M7 21 V10 A5 5 0 0 1 17 10 V21 Z M10 21 V15 M14 21 V15 M10 15 C11 13 13 13 14 15",
        "M6 4 V20 M12 4 V20 M18 4 V20 M4 8 H8 M10 15 H14 M16 10 H20",
    ];
    pub const SMART: [&str; 4] = [
        "M12 3.5 A8.5 8.5 0 1 0 12.01 3.5 M6 18 L18 6",
        "M3 15 C6 15 7 8 10 8 C13 8 14 15 17 15 C18.5 15 19.5 13 21 11 M17.5 10.5 L21 11 L21 7.5",
        "M9.5 4.5 A2.5 2.5 0 0 1 14.5 4.5 V11 A2.5 2.5 0 0 1 9.5 11 Z M6 10.5 A6 6 0 0 0 18 10.5 M12 16.5 V20.5 M9 20.5 H15",
        "M12 3.5 A8.5 8.5 0 1 0 12.01 3.5 M12 9.5 A2.5 2.5 0 1 0 12.01 9.5 M12 3.5 A8.5 8.5 0 0 1 20.5 12",
    ];
    pub const DETECTORS: [&str; 3] = [
        "M3 18 H9 C10.5 18 11 4 12 4 C13 4 13.5 18 15 18 H21",
        "M3 18 C7 18 8.5 9 12 9 C15.5 9 17 18 21 18",
        "M4 18 V15 M8 18 V11 M12 18 V8 M16 18 V11 M20 18 V14",
    ];
    pub const FF: &str = "M3 12 H21 M7 12 V6 H15 M15 6 L13 4 M15 6 L13 8 M11 9 H15 V15 H11 Z";
    pub const FB: &str = "M3 12 H21 M17 12 V18 H9 V15 M9 15 L7 17 M9 15 L11 17 M11 9 H15 V15 H11 Z";
    pub const KEY: &str = "M7.5 15.5 A3.5 3.5 0 1 1 7.51 15.5 M10.5 13.5 L20 7 M17 9 L19 12 M14.5 10.7 L16 13";
    pub const LINK: &str = "M9 7.5 A4.5 4.5 0 1 0 9.01 7.5 M15 7.5 A4.5 4.5 0 1 0 15.01 7.5";
    pub const SLIDERS: &str = "M4 7 H20 M4 12 H20 M4 17 H20 M8.5 5 V9 M15 10 V14 M10.5 15 V19";
    pub const INTERNAL: &str = "M5 12 A7 7 0 1 0 7.1 7 M5 4.5 V8.5 H9";
    pub const EXTERNAL: &str = "M9 3 V7.5 M15 3 V7.5 M7 7.5 H17 V11 A5 5 0 0 1 7 11 Z M12 16 V21";
    pub const LISTEN: &str = "M4 14 V12 A8 8 0 0 1 20 12 V14 M4 14 H7 V20 H4 Z M20 14 H17 V20 H20 Z";
    pub const LR: &str = "M8.5 7 A5 5 0 1 0 8.51 7 M15.5 7 A5 5 0 1 0 15.51 7";
    pub const MS: &str = "M12 4 A8 8 0 1 0 12.01 4 M12 9.5 A2.5 2.5 0 1 0 12.01 9.5";
    pub const DELTA: &str = "M12 4 L20.5 19 H3.5 Z";
    pub const DIAGONAL: &str = "M3 21 L21 3";

    pub fn quadrant(q: super::Quadrant) -> &'static str {
        match q {
            super::Quadrant::Compress => COMP,
            super::Quadrant::Expand => EXP,
            super::Quadrant::UpCompress => UP,
            super::Quadrant::UpExpand => UPX,
        }
    }
}

/// Draws a 24×24 glyph scaled to `size`, centred on `c`.
fn draw_glyph(scene: &mut mite_ui::vello::Scene, d: &str, c: Point, size: f64, color: Color, width: f64) {
    let Ok(mut path) = BezPath::from_svg(d) else { return };
    let k = size / 24.0;
    path.apply_affine(Affine::translate((c.x - 12.0 * k, c.y - 12.0 * k)) * Affine::scale(k));
    let stroke = Stroke::new(width).with_caps(Cap::Round).with_join(Join::Round);
    scene.stroke(&stroke, Affine::IDENTITY, color, None, &path);
}

/// A stand-in host holding every Mite Comp parameter at its default, for tests, the preview and
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
                let short = id.split_once('_').filter(|(p, _)| p.len() == 2 && (p.starts_with('s') || p.starts_with('k'))).map_or(id, |(_, rest)| rest);
                let base = [
                    C_ATTACK, C_RELEASE, C_DRIVE, C_TAIL, C_PROGRAM, C_BEND, SMART_AMOUNT, TARGET_GR, SIBILANCE, LINK, LOOKAHEAD, HOLD, MAKEUP, MIX, OUT_PAN, OUT_GAIN,
                ]
                .into_iter()
                .find(|s| s.id == id)
                .or_else(|| {
                    let pool: &[ParamSpec] = if id.starts_with('k') { &[K_FREQ, K_GAIN, K_Q] } else { &[S_THRESH, S_RATIO, S_KNEE, S_ATTACK, S_RELEASE, S_RANGE] };
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

/// Default settings a stage gets when it is made, by quadrant (the design's leveller, gate,
/// compressor and lifter): knee, attack, release, range, detector.
fn quadrant_defaults(q: Quadrant) -> (f32, f32, f32, f32, Detector) {
    match q {
        Quadrant::Compress => (6.0, 10.0, 120.0, 60.0, Detector::Rms),
        Quadrant::Expand => (4.0, 0.5, 60.0, 60.0, Detector::Peak),
        Quadrant::UpCompress => (6.0, 30.0, 800.0, 12.0, Detector::Rms),
        Quadrant::UpExpand => (6.0, 1.0, 80.0, 12.0, Detector::Peak),
    }
}

/// The ratio a pull of `d` px makes: 1:1 at the start, 4:1 at 48 px, ∞:1 at 160 px.
fn pull_ratio(d: f64) -> f32 {
    let slope = if d <= PULL_START {
        0.0
    } else if d <= PULL_4 {
        0.75 * (d - PULL_START) / (PULL_4 - PULL_START)
    } else {
        0.75 + 0.25 * ((d - PULL_4) / (PULL_INF - PULL_4)).min(1.0)
    };
    if slope >= 0.995 { RATIO_MAX } else { (1.0 / (1.0 - slope)) as f32 }
}

fn sq_x(db: f32) -> f64 {
    SQ_X + f64::from(db.clamp(-60.0, 0.0) + 60.0) / 60.0 * SQ
}

fn sq_y(db: f32) -> f64 {
    SQ_Y + f64::from(-db.clamp(-60.0, 0.0)) / 60.0 * SQ
}

/// A point in the square as (input dB, output dB).
fn sq_db(p: Point) -> (f32, f32) {
    (((p.x - SQ_X) / SQ * 60.0 - 60.0) as f32, (-(p.y - SQ_Y) / SQ * 60.0) as f32)
}

fn sq_rect() -> Rect {
    Rect::new(SQ_X, SQ_Y, SQ_X + SQ, SQ_Y + SQ)
}

/// History levels: 0 dB 24 px under the display's top, −60 dB 572 px lower.
fn lv_y(db: f32) -> f64 {
    let range = level_range();
    DISPLAY_TOP + 24.0 + (-f64::from(db)).clamp(0.0, range) / range * 572.0
}

fn lv_db(y: f64) -> f32 {
    (-(y - DISPLAY_TOP - 24.0) / 572.0 * level_range()) as f32
}

fn hist_rect() -> Rect {
    Rect::new(0.0, DISPLAY_TOP, HIST_W, HELP_TOP)
}

/// What a drag is doing.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Drag {
    /// A node: along the diagonal moves the threshold, away from it sets the ratio; Alt the knee.
    Node { slot: usize, start: Point, threshold: f32, knee: f32, lock: Option<bool> },
    /// A threshold diamond on the diagonal: threshold only.
    Diamond { slot: usize, start: Point, threshold: f32 },
    /// Pulling the line to make a stage.
    Create { start: Point, input_db: f32, now: Point },
    /// A threshold grip or line on the history.
    Grip { slot: usize, start: Point, threshold: f32 },
    /// Smart's target on the history.
    Target { start: Point, target: f32 },
    /// The envelope scope's attack or release handle.
    Scope { slot: usize, attack: bool, start: Point, value: f32 },
    /// A sidechain EQ node.
    Key { band: usize },
}

/// A docked panel or popover.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Panel {
    Stage(usize),
    Custom,
    Sidechain,
    Detector,
    Stereo,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MenuKind {
    Character,
    Smart,
    StageDetector(usize),
    KeySource,
}

struct Menu {
    kind: MenuKind,
    list: Dropdown,
}

/// What is under the pointer on the display.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Hot {
    Node(usize),
    Diamond(usize),
    Grip(usize),
    Target,
    Line,
    Square,
    History,
}

/// Rectangles of the open panel's controls, set while drawing.
#[derive(Default, Clone)]
struct PanelHits {
    rect: Rect,
    close: Rect,
    remove: Option<Rect>,
    quadrants: Vec<Rect>,
    detector: Option<Rect>,
    scope: Option<Rect>,
    handles: Vec<(bool, Point)>,
    icons: Vec<(Rect, &'static str, f32)>,
    toggle: Option<Rect>,
    /// The stage panel's auto release switch.
    auto_release: Option<Rect>,
    key_area: Option<Rect>,
    key_source: Option<Rect>,
    listen: Option<Rect>,
}

pub struct CompView {
    shared: Arc<CompShared>,
    session: Session,
    /// The history's level range: the picker, the range as drawn (eased), and where it goes.
    range: RangePicker,
    range_shown: f32,
    range_target: f32,
    footer: Footer,
    selected: Option<usize>,
    pointer: Option<Point>,
    hot: Option<Hot>,
    drag: Option<Drag>,
    panel: Option<Panel>,
    panel_knobs: Vec<Knob>,
    knobs_for: Option<Panel>,
    hits: PanelHits,
    menu: Option<Menu>,
    head_hits: [Rect; 3],
    foot_hits: [Rect; 2],
    /// History records (newest last), the tap's read position, and the meters as drawn
    /// (IN L/R, GR A/B, OUT L/R) with their ballistics.
    history: VecDeque<[f32; record::WIDTH]>,
    seen: usize,
    meters: [f32; 6],
    /// When records last arrived (the history scrolls with the clock from there).
    last_fresh: Instant,
    last_tick: Instant,
    ticks: u32,
}

impl CompView {
    pub fn new(shared: Arc<CompShared>) -> Self {
        let footer = Footer::new(
            FOOTER_TOP,
            (WIDTH, HEIGHT),
            vec![
                FooterToggle { id: AUTO_MAKEUP.id, glyph: ToggleGlyph::Auto, words: "Auto makeup — follows the curve, so changing it doesn't change the level (never pumps)" },
                FooterToggle { id: DELTA.id, glyph: ToggleGlyph::Delta, words: "Delta — hear only what the compressor takes away" },
                FooterToggle { id: BYPASS.id, glyph: ToggleGlyph::Bypass, words: "Bypass — the whole compressor, latency kept" },
            ],
            vec![
                FooterDial { spec: MIX, kind: KnobKind::Unipolar, name: "MIX", words: "Mix — the compressor against the dry signal, delay-compensated" },
                FooterDial { spec: OUT_PAN, kind: KnobKind::Bipolar, name: "PAN", words: "Output balance" },
                FooterDial { spec: OUT_GAIN, kind: KnobKind::Bipolar, name: "OUTPUT", words: "Output gain" },
            ],
        );
        Self {
            range: RangePicker::new(
                Rect::new(8.0, DISPLAY_TOP + 4.0, 54.0, DISPLAY_TOP + 22.0),
                Rect::new(0.0, DISPLAY_TOP + 24.0, 36.0, HELP_TOP),
                LEVEL_RANGES,
                LEVEL_RANGE_LABELS,
                shared.display_range.clone(),
                (DISPLAY_TOP, HELP_TOP),
            )
            .with_prefix("−"),
            range_shown: 60.0,
            range_target: 60.0,
            session: Session::new(
                SessionConfig { plugin: PLUGIN, factory: FACTORY, sound_ids: preset_ids(), snapshot_ids: preset_ids() },
                shared.snapshots.clone(),
                shared.preset.clone(),
            ),
            footer,
            selected: None,
            pointer: None,
            hot: None,
            drag: None,
            panel: None,
            panel_knobs: Vec::new(),
            knobs_for: None,
            hits: PanelHits::default(),
            menu: None,
            head_hits: [Rect::ZERO; 3],
            foot_hits: [Rect::ZERO; 2],
            history: VecDeque::with_capacity(crate::dsp::HISTORY_RECORDS),
            seen: 0,
            meters: [-150.0, -150.0, 0.0, 0.0, -150.0, -150.0],
            last_fresh: Instant::now(),
            last_tick: Instant::now(),
            ticks: 0,
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

    /// Selects a stage (its panel opens), for tests and tools.
    pub fn select(&mut self, slot: Option<usize>) {
        self.selected = slot;
        self.panel = slot.map(Panel::Stage);
        self.panel_knobs.clear();
        self.knobs_for = None;
    }

    /// Opens a header panel by name (`custom`, `sidechain`, `detector`, `stereo`), for tools.
    pub fn open_panel(&mut self, name: &str) {
        self.panel = match name {
            "custom" => Some(Panel::Custom),
            "sidechain" => Some(Panel::Sidechain),
            "detector" => Some(Panel::Detector),
            "stereo" => Some(Panel::Stereo),
            _ => None,
        };
        self.panel_knobs.clear();
        self.knobs_for = None;
    }

    /// Opens the Character (`character`) or Smart (`smart`) menu, for tools.
    pub fn open_menu(&mut self, which: &str, host: &dyn ParamHost) {
        let kind = if which == "smart" { MenuKind::Smart } else { MenuKind::Character };
        let r = self.foot_hits[usize::from(kind == MenuKind::Smart)];
        let r = if r.width() > 0.0 { r } else { Rect::new(16.0, FOOTER_TOP + 10.0, 166.0, FOOTER_TOP + 46.0) };
        self.toggle_menu(kind, r, host);
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

    fn stage(host: &dyn ParamHost, slot: usize) -> Stage {
        let id = |p: &str| stage_id(slot, p);
        Stage {
            on: Self::on(host, &id(S_ON.id)),
            quadrant: Quadrant::from_index(Self::index(host, &id(S_QUAD.id))),
            threshold_db: Self::plain(host, &id(S_THRESH.id)),
            ratio: Self::plain(host, &id(S_RATIO.id)),
            knee_db: Self::plain(host, &id(S_KNEE.id)),
            range_db: Self::plain(host, &id(S_RANGE.id)),
            attack_ms: Self::plain(host, &id(S_ATTACK.id)),
            release_ms: Self::plain(host, &id(S_RELEASE.id)),
            auto_release: Self::on(host, &id(S_AUTO_REL.id)),
            detector: Detector::from_index(Self::index(host, &id(S_DET.id))),
        }
    }

    fn stages(host: &dyn ParamHost) -> [Stage; STAGES] {
        std::array::from_fn(|i| Self::stage(host, i))
    }

    /// Stages that are on, in threshold order (slot indices): stage n in the UI is `ranked[n − 1]`.
    fn ranked(stages: &[Stage; STAGES]) -> Vec<usize> {
        order(stages).into_iter().filter(|&i| stages[i].on).collect()
    }

    /// The number the UI shows for `slot` (its rank by threshold, from 1).
    fn number(stages: &[Stage; STAGES], slot: usize) -> usize {
        Self::ranked(stages).iter().position(|&i| i == slot).map_or(slot + 1, |r| r + 1)
    }

    fn style(&self, host: &dyn ParamHost) -> mite_dsp::dynamics::Style {
        let c = self.shared.audition().unwrap_or(Self::index(host, CHARACTER.id)).min(CUSTOM);
        if c < CUSTOM {
            return STYLES[c];
        }
        mite_dsp::dynamics::Style {
            attack: Self::plain(host, C_ATTACK.id),
            release: Self::plain(host, C_RELEASE.id),
            drive_db: Self::plain(host, C_DRIVE.id),
            slow_tail: Self::plain(host, C_TAIL.id) / 100.0,
            program: Self::plain(host, C_PROGRAM.id) / 100.0,
            ratio_bend: Self::plain(host, C_BEND.id) / 100.0,
        }
    }

    /// Records' worth of time since the newest arrived (up to one host block's worth), so the
    /// history keeps moving between arrivals.
    fn since_fresh(&self) -> f64 {
        (self.last_fresh.elapsed().as_secs_f64() * 1000.0 / f64::from(crate::dsp::HISTORY_SLICE_MS)).min(4.0)
    }

    /// Smart's slide now, dB (the newest record's).
    fn offset(&self) -> f32 {
        self.history.back().map_or(0.0, |r| r[record::OFFSET])
    }

    fn smart(host: &dyn ParamHost) -> Smart {
        Smart::from_index(Self::index(host, SMART.id))
    }

    /// Where stage `slot` starts acting, on the input axis (thresholds are set on the input),
    /// with Smart's slide.
    fn threshold_input(stages: &[Stage; STAGES], slot: usize, offset: f32) -> f32 {
        (stages[slot].threshold_db + offset).clamp(-60.0, 0.0)
    }

    /// The node's input level: 12 dB into the stage's active side, kept inside the square.
    fn node_input(stages: &[Stage; STAGES], slot: usize, offset: f32) -> f32 {
        let t = Self::threshold_input(stages, slot, offset);
        if stages[slot].quadrant.acts_above() { (t + NODE_OFFSET_DB).min(-1.5) } else { (t - NODE_OFFSET_DB).max(-58.5) }
    }

    /// The node, on the curve this stage and those below it make: the stages above it never move
    /// it. When one of them starts before the node, the node sits on this stage's own
    /// continuation (drawn dashed), off the final curve.
    fn node_pos(stages: &[Stage; STAGES], slot: usize, offset: f32) -> Point {
        let x = Self::node_input(stages, slot, offset);
        Point::new(sq_x(x), sq_y(partial_curve_db(stages, slot, x, offset)))
    }

    /// The threshold diamond: where the stage starts, on the curve the stages below it make.
    fn diamond_pos(stages: &[Stage; STAGES], slot: usize, offset: f32) -> Point {
        let t = Self::threshold_input(stages, slot, offset);
        Point::new(sq_x(t), sq_y(level_before(stages, slot, t, offset)))
    }

    fn grip_rect(stages: &[Stage; STAGES], slot: usize, offset: f32) -> Rect {
        let y = lv_y(Self::threshold_input(stages, slot, offset));
        Rect::new(GRIP_X, y - GRIP_SIZE.1 / 2.0, GRIP_X + GRIP_SIZE.0, y + GRIP_SIZE.1 / 2.0)
    }

    fn target_rect(host: &dyn ParamHost) -> Rect {
        let y = DISPLAY_TOP + f64::from(Self::plain(host, TARGET_GR.id)) * gr_px_per_db();
        Rect::new(GRIP_X - 30.0, y - 11.0, GRIP_X + GRIP_SIZE.0, y + 11.0)
    }

    fn hot_at(&self, host: &dyn ParamHost, p: Point) -> Option<Hot> {
        let stages = Self::stages(host);
        let offset = self.offset();
        for slot in Self::ranked(&stages).into_iter().rev() {
            if Self::node_pos(&stages, slot, offset).distance(p) <= NODE_HIT {
                return Some(Hot::Node(slot));
            }
        }
        for slot in Self::ranked(&stages) {
            if Self::diamond_pos(&stages, slot, offset).distance(p) <= 8.0 {
                return Some(Hot::Diamond(slot));
            }
        }
        if Self::smart(host).tracks_threshold() && Self::target_rect(host).contains(p) {
            return Some(Hot::Target);
        }
        for slot in Self::ranked(&stages) {
            let r = Self::grip_rect(&stages, slot, offset);
            let line = Rect::new(0.0, r.y0 + 6.0, GRIP_X, r.y1 - 6.0);
            if r.contains(p) || line.contains(p) {
                return Some(Hot::Grip(slot));
            }
        }
        if sq_rect().contains(p) {
            let (x, y) = sq_db(p);
            let on_curve = (f64::from(curve_db(&stages, x, offset) - y) * SQ / 60.0).abs() < 10.0;
            return Some(if on_curve { Hot::Line } else { Hot::Square });
        }
        hist_rect().contains(p).then_some(Hot::History)
    }

    /// The first free stage slot.
    fn free_slot(host: &dyn ParamHost) -> Option<usize> {
        (0..STAGES).find(|&i| !Self::on(host, &stage_id(i, S_ON.id)))
    }

    /// The stage a pull from `start` (input level `input_db`) to `now` would make.
    fn pulled_stage(host: &dyn ParamHost, input_db: f32, start: Point, now: Point) -> Option<Stage> {
        let stages = Self::stages(host);
        let d = now.distance(start);
        if d < PULL_START {
            return None;
        }
        // Above the line (the pointer above the curve where it started) lifts.
        let up = now.y < start.y;
        let ranked = Self::ranked(&stages);
        let above = match ranked.iter().map(|&i| stages[i].threshold_db).min_by(|a, b| (a - input_db).abs().total_cmp(&(b - input_db).abs())) {
            Some(nearest) => input_db >= nearest,
            None => !up,
        };
        let quadrant = Quadrant::from_pull(up, above);
        let (knee_db, attack_ms, release_ms, range_db, detector) = quadrant_defaults(quadrant);
        Some(Stage { on: true, quadrant, threshold_db: input_db.clamp(-60.0, 0.0), ratio: pull_ratio(d), knee_db, range_db, attack_ms, release_ms, auto_release: false, detector })
    }

    /// Writes `stage` into `slot` (one gesture per parameter).
    fn write_stage(host: &dyn ParamHost, slot: usize, s: &Stage) {
        let id = |p: &str| stage_id(slot, p);
        Self::set(host, &id(S_QUAD.id), s.quadrant as usize as f32);
        Self::set(host, &id(S_THRESH.id), s.threshold_db);
        Self::set(host, &id(S_RATIO.id), s.ratio);
        Self::set(host, &id(S_KNEE.id), s.knee_db);
        Self::set(host, &id(S_ATTACK.id), s.attack_ms);
        Self::set(host, &id(S_RELEASE.id), s.release_ms);
        Self::set(host, &id(S_RANGE.id), s.range_db);
        Self::set(host, &id(S_DET.id), s.detector as usize as f32);
        Self::set(host, &id(S_AUTO_REL.id), if s.auto_release { 1.0 } else { 0.0 });
        Self::set(host, &id(S_ON.id), 1.0);
    }

    fn delete_stage(&mut self, host: &dyn ParamHost, slot: usize) {
        Self::set(host, &stage_id(slot, S_ON.id), 0.0);
        Self::set(host, &stage_id(slot, S_SOLO.id), 0.0);
        if self.selected == Some(slot) {
            self.select(None);
        }
    }

    /// Ratio from how far a point sits off the curve the stages below make at the node (`gain_db`
    /// signed), per quadrant, `depth_db` into the stage (in the level it works on).
    fn ratio_from_gain(q: Quadrant, gain_db: f32, depth_db: f32) -> f32 {
        let g = gain_db / depth_db.max(0.5);
        let r = match q {
            Quadrant::Compress => 1.0 / (1.0 - (-g).clamp(0.0, 0.995)),
            Quadrant::UpCompress => 1.0 / (1.0 - g.clamp(0.0, 0.995)),
            Quadrant::Expand => 1.0 - g.min(0.0),
            Quadrant::UpExpand => 1.0 + g.max(0.0),
        };
        if r >= INF_RATIO * 0.99 { RATIO_MAX } else { r.clamp(1.0, RATIO_MAX) }
    }

    // ---- drawing ----------------------------------------------------------------------------

    fn draw_header(&mut self, cx: &mut DrawCx<'_>) {
        let theme = cx.theme;
        cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.window, None, &Rect::new(0.0, 0.0, WIDTH, HEADER));
        cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, theme.line, None, &Line::new((0.0, HEADER - 0.5), (WIDTH, HEADER - 0.5)));
        draw_mark(cx, 16.0, 15.0, 26.0);
        draw_title(cx, 52.0, 18.0, "Comp");
        self.session.draw(cx);
        let (y0, y1) = HEADER_ROW;
        let listen = Self::on(cx.host, LISTEN.id);
        let stereo_glyph = if Self::index(cx.host, STEREO.id) == 1 { glyph::MS } else { glyph::LINK };
        for (i, (d, panel)) in [(glyph::KEY, Panel::Sidechain), (stereo_glyph, Panel::Stereo), (glyph::SLIDERS, Panel::Detector)].into_iter().enumerate() {
            let x = HEAD_BUTTONS_X + i as f64 * (HEAD_BUTTON_W + 6.0);
            let r = Rect::new(x, y0, x + HEAD_BUTTON_W, y1);
            let on = self.panel == Some(panel);
            let hot = self.pointer.is_some_and(|p| r.contains(p));
            let color = if panel == Panel::Sidechain { theme.sidechain } else { theme.accent };
            let lit = on || (panel == Panel::Sidechain && listen);
            let ink = icon_button(cx.scene, theme, r, theme.panel_control, hot, lit, color);
            draw_glyph(cx.scene, d, r.center(), 20.0, ink, 1.7);
            self.head_hits[i] = r;
        }
    }

    fn draw_history(&self, cx: &mut DrawCx<'_>, stages: &[Stage; STAGES]) {
        let theme = cx.theme;
        let grid = theme.grid_minor;
        // Gridlines and labels every 6 dB (24 dB range, plus −3) or 12 dB, from the range it glides to.
        let step = if self.range_target <= 24.5 { 6.0 } else { 12.0 };
        let mut levels: Vec<f32> = (1..).map(|k| -step * k as f32).take_while(|&db| db > -self.range_target + 0.5).collect();
        // At 24 dB, −3 as well: the top few dB are where a mix bus lives.
        if self.range_target <= 24.5 {
            levels.insert(0, -3.0);
        }
        for &db in &levels {
            cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, grid, None, &Line::new((0.0, lv_y(db)), (HIST_W, lv_y(db))));
        }
        for s in 1..6 {
            let x = s as f64 * HIST_W / 6.0;
            cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, grid, None, &Line::new((x, DISPLAY_TOP), (x, HELP_TOP)));
        }
        cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, theme.grid_zero, None, &Line::new((0.0, DISPLAY_TOP + 24.0), (HIST_W, DISPLAY_TOP + 24.0)));
        for db in std::iter::once(0.0).chain(levels.iter().copied()) {
            let label = if db == 0.0 { "0".to_owned() } else { format!("−{:.0}", -db) };
            cx.text.draw(cx.scene, &label, 8.0, lv_y(db) + 3.0, TextStyle { tabular: true, ..TextStyle::BODY.sized(10.0).weighted(600.0) }, theme.text_lo, Align::Left);
        }

        // Columns of 4 records (20 ms), fixed to the records' absolute times so they never
        // re-bucket; each the mean of its records (levels in dB), then eased across neighbours, so
        // noise reads as a level envelope. They scroll with the clock between arrivals, so the
        // history moves at the display's rate however the host's blocks fall.
        const PER: usize = 4;
        let px_per_record = HIST_W / (HIST_SECONDS * 1000.0 / f64::from(crate::dsp::HISTORY_SLICE_MS));
        let total = self.history.len();
        let first_abs = self.seen.wrapping_sub(total);
        let now = self.seen as f64 + self.since_fresh();
        let first_col = first_abs.div_ceil(PER);
        let last_col = self.seen.div_ceil(PER);
        let mut raw: Vec<(f64, [f32; record::WIDTH])> = Vec::with_capacity(HIST_COLUMNS + 4);
        for col in first_col..last_col {
            let (a, b) = (col * PER, ((col + 1) * PER).min(self.seen));
            let x = HIST_W - (now - (a + b) as f64 / 2.0) * px_per_record;
            if x < -20.0 {
                continue;
            }
            let mut out = [0.0f32; record::WIDTH];
            for i in a..b {
                for (o, v) in out.iter_mut().zip(&self.history[i - first_abs]) {
                    *o += v.max(-90.0);
                }
            }
            for o in &mut out {
                *o /= (b - a) as f32;
            }
            raw.push((x, out));
        }
        if raw.is_empty() {
            return;
        }
        let cols: Vec<(f64, [f32; record::WIDTH])> = (0..raw.len())
            .map(|i| {
                let (a, b) = (raw[i.saturating_sub(1)].1, raw[(i + 1).min(raw.len() - 1)].1);
                let mut r = raw[i].1;
                for (k, v) in r.iter_mut().enumerate() {
                    *v = 0.25 * a[k] + 0.5 * *v + 0.25 * b[k];
                }
                (raw[i].0, r)
            })
            .collect();
        if cols.len() < 2 {
            return;
        }
        let delta = Self::on(cx.host, DELTA.id);
        let listen = Self::on(cx.host, LISTEN.id);
        let level = |r: &[f32; record::WIDTH], k: usize| r[k].max(r[k + 1]);
        let line = |f: &dyn Fn(&[f32; record::WIDTH]) -> f64| {
            let pts: Vec<Point> = cols.iter().map(|(x, r)| Point::new(*x, f(r))).collect();
            smooth_path(&pts)
        };
        let closed = |mut p: BezPath, y: f64| {
            p.line_to((cols[cols.len() - 1].0, y));
            p.line_to((cols[0].0, y));
            p.close_path();
            p
        };
        // IN, dim.
        let input = line(&|r| lv_y(level(r, record::IN)));
        cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.spectrum_pre.multiply_alpha(0.16), None, &closed(input.clone(), HELP_TOP));
        cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, theme.spectrum_pre.multiply_alpha(0.55), None, &input);
        // Upward gain from the bottom.
        let up = |r: &[f32; record::WIDTH]| r[record::LIFT].max(r[record::LIFT + 1]);
        if cols.iter().any(|(_, r)| up(r) > 0.05) {
            let edge = line(&|r| HELP_TOP - (f64::from(up(r)) * gr_px_per_db()).min(160.0));
            let fill = Gradient::new_linear((0.0, HELP_TOP), (0.0, HELP_TOP - 160.0)).with_stops([theme.gain_up.multiply_alpha(0.5), theme.gain_up.multiply_alpha(0.14)]);
            cx.scene.fill(Fill::NonZero, Affine::IDENTITY, &fill, None, &closed(edge.clone(), HELP_TOP));
            glow_stroke(cx, &edge, 1.6, theme.gain_up, theme.gain_up, 2.4);
        }
        // OUT, lit (the removed signal under delta; the detector under Listen).
        let out_color = if listen { theme.sidechain } else { theme.curve };
        let output = if delta {
            line(&|r| {
                let (i, o) = (mite_dsp::db_to_gain(level(r, record::IN)), mite_dsp::db_to_gain(level(r, record::OUT)));
                lv_y(mite_dsp::gain_to_db((i - o).abs().max(1e-6)))
            })
        } else {
            line(&|r| lv_y(level(r, record::OUT)))
        };
        let a = if delta { 0.3 } else { 0.12 };
        let fill = Gradient::new_linear((0.0, DISPLAY_TOP), (0.0, HELP_TOP)).with_stops([out_color.multiply_alpha(a), out_color.multiply_alpha(0.01)]);
        cx.scene.fill(Fill::NonZero, Affine::IDENTITY, &fill, None, &closed(output.clone(), HELP_TOP));
        glow_stroke(cx, &output, 1.8, out_color, out_color, 2.4);
        // GR from the top: solid for L/M, dashed for R/S when the channels differ.
        let cut = |r: &[f32; record::WIDTH], c: usize| DISPLAY_TOP + (f64::from(-r[record::CUT + c]) * gr_px_per_db()).clamp(0.0, 250.0);
        if !cols.iter().any(|(_, r)| r[record::CUT].min(r[record::CUT + 1]) < -0.05) {
            return;
        }
        let edge = line(&|r| cut(r, 0).max(cut(r, 1)));
        let mut fill_path = edge.clone();
        fill_path.line_to((cols[cols.len() - 1].0, DISPLAY_TOP));
        fill_path.line_to((cols[0].0, DISPLAY_TOP));
        fill_path.close_path();
        let fill = Gradient::new_linear((0.0, DISPLAY_TOP), (0.0, DISPLAY_TOP + 250.0)).with_stops([theme.gain_reduction.multiply_alpha(0.55), theme.gain_reduction.multiply_alpha(0.18)]);
        cx.scene.fill(Fill::NonZero, Affine::IDENTITY, &fill, None, &fill_path);
        glow_stroke(cx, &edge, 1.8, theme.gain_reduction, theme.gain_reduction, 2.4);
        if Self::link_split(cx.host) {
            let second = line(&|r| cut(r, 1));
            cx.scene.stroke(&Stroke::new(1.6).with_dashes(0.0, [4.0, 3.0]), Affine::IDENTITY, theme.gain_reduction, None, &second);
        }
        let _ = stages;
    }

    /// Whether the two channels' GR can differ (stereo, link under 100 %).
    fn link_split(host: &dyn ParamHost) -> bool {
        Self::plain(host, LINK.id) < 99.5
    }

    fn draw_thresholds(&self, cx: &mut DrawCx<'_>, stages: &[Stage; STAGES]) {
        let theme = cx.theme;
        let offset = self.offset();
        let smart = Self::smart(cx.host).tracks_threshold();
        for slot in Self::ranked(stages) {
            let selected = self.selected == Some(slot);
            let hot = matches!(self.hot, Some(Hot::Grip(k)) if k == slot) || matches!(self.drag, Some(Drag::Grip { slot: k, .. }) if k == slot);
            let alpha = if self.selected.is_none() || selected || hot { 1.0 } else { 0.55 };
            let n = Self::number(stages, slot);
            if smart {
                // Where you set it: dashed, in the quiet text colour.
                let y = lv_y(Self::threshold_input(stages, slot, 0.0));
                cx.scene.stroke(&Stroke::new(1.5).with_dashes(0.0, [5.0, 4.0]), Affine::IDENTITY, theme.text_mid.multiply_alpha(0.7), None, &Line::new((0.0, y), (GRIP_X - 2.0, y)));
            }
            let y = lv_y(Self::threshold_input(stages, slot, offset));
            let line = Line::new((0.0, y), (GRIP_X - 2.0, y));
            if selected || hot {
                glow_stroke(cx, &line, 1.5, theme.accent.multiply_alpha(alpha), theme.accent, 3.0);
            } else {
                cx.scene.stroke(&Stroke::new(1.5), Affine::IDENTITY, theme.accent.multiply_alpha(alpha), None, &line);
            }
            let r = Self::grip_rect(stages, slot, offset);
            let shape = RoundedRect::from_rect(r, 11.0);
            let fill = if selected { theme.accent } else { theme.display };
            cx.scene.fill(Fill::NonZero, Affine::IDENTITY, fill.multiply_alpha(alpha), None, &shape);
            glow_stroke(cx, &shape, 1.5, theme.accent.multiply_alpha(alpha), theme.accent, if hot { 4.0 } else { 2.0 });
            let ink = if selected { theme.display } else { theme.accent };
            // Up and down chevrons, then the stage's number.
            let c = Point::new(r.x0 + 11.0, r.center().y);
            let mut chev = BezPath::new();
            chev.move_to((c.x - 3.0, c.y - 2.0));
            chev.line_to((c.x, c.y - 5.0));
            chev.line_to((c.x + 3.0, c.y - 2.0));
            chev.move_to((c.x - 3.0, c.y + 2.0));
            chev.line_to((c.x, c.y + 5.0));
            chev.line_to((c.x + 3.0, c.y + 2.0));
            cx.scene.stroke(&Stroke::new(1.3).with_caps(Cap::Round).with_join(Join::Round), Affine::IDENTITY, ink.multiply_alpha(alpha), None, &chev);
            cx.text.draw(cx.scene, &n.to_string(), r.x0 + 23.0, r.y0 + 5.0, TextStyle::LABEL.sized(10.0).weighted(800.0), ink.multiply_alpha(alpha), Align::Center);
        }
        if smart {
            // Smart's target: a dashed GR line with its value.
            let r = Self::target_rect(cx.host);
            let y = r.center().y;
            cx.scene.stroke(&Stroke::new(1.5).with_dashes(0.0, [5.0, 4.0]), Affine::IDENTITY, theme.gain_reduction.multiply_alpha(0.9), None, &Line::new((0.0, y), (r.x0, y)));
            let shape = RoundedRect::from_rect(r, 11.0);
            cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.display, None, &shape);
            cx.scene.stroke(&Stroke::new(1.5).with_dashes(0.0, [4.0, 3.0]), Affine::IDENTITY, theme.gain_reduction, None, &shape);
            let v = format!("{:.0} dB", Self::plain(cx.host, TARGET_GR.id));
            cx.text.draw(cx.scene, &v, r.center().x, r.y0 + 5.0, TextStyle { tabular: true, ..TextStyle::LABEL.sized(10.0).weighted(800.0) }, theme.gain_reduction, Align::Center);
        }
    }

    fn draw_square(&self, cx: &mut DrawCx<'_>, stages: &[Stage; STAGES]) {
        let theme = cx.theme;
        let offset = self.offset();
        let r = sq_rect();
        panel(cx.scene, RoundedRect::from_rect(r, 10.0), theme.well, theme.well_border);
        for db in [-48.0, -36.0, -24.0, -12.0] {
            cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, theme.grid_major, None, &Line::new((sq_x(db), r.y0), (sq_x(db), r.y1)));
            cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, theme.grid_major, None, &Line::new((r.x0, sq_y(db)), (r.x1, sq_y(db))));
        }
        let diag = Line::new((sq_x(-60.0), sq_y(-60.0)), (sq_x(0.0), sq_y(0.0)));
        cx.scene.stroke(&Stroke::new(1.2).with_dashes(0.0, [3.0, 4.0]), Affine::IDENTITY, theme.grid_zero, None, &diag);
        let label = TextStyle { tabular: true, ..TextStyle::BODY.sized(10.0).weighted(600.0) };
        for (x, y, t) in [(r.x0 + 22.0, r.y1 - 16.0, "−60"), (r.x1 - 18.0, r.y1 - 16.0, "0"), (r.x0 + 6.0, r.y0 + 6.0, "OUT"), (r.x1 - 30.0, r.y1 + 6.0, "IN")] {
            cx.text.draw(cx.scene, t, x, y, label, theme.text_lo, Align::Left);
        }

        // The curve while a stage is being pulled includes it (ghost), as do the shadings.
        let mut shape = *stages;
        let creating = match self.drag {
            Some(Drag::Create { start, input_db, now }) => Self::pulled_stage(cx.host, input_db, start, now),
            _ => None,
        };
        let refused = creating.is_some() && Self::free_slot(cx.host).is_none();
        if let (Some(s), Some(slot)) = (creating, Self::free_slot(cx.host)) {
            shape[slot] = s;
        }
        let xs: Vec<f32> = (0..=240).map(|i| -60.0 + i as f32 * 0.25).collect();
        let points = |st: &[Stage; STAGES], off: f32| -> Vec<Point> { xs.iter().map(|&x| Point::new(sq_x(x), sq_y(curve_db(st, x, off)))).collect() };
        let shade = |st: &[Stage; STAGES], lower: bool| {
            let mut p = BezPath::new();
            for (i, &x) in xs.iter().enumerate() {
                let y = curve_db(st, x, offset);
                let y = if lower { y.min(x) } else { y.max(x) };
                let pt = (sq_x(x), sq_y(y));
                if i == 0 { p.move_to(pt) } else { p.line_to(pt) }
            }
            p.line_to((sq_x(0.0), sq_y(0.0)));
            p.line_to((sq_x(-60.0), sq_y(-60.0)));
            p.close_path();
            p
        };
        cx.scene.push_clip_layer(Fill::NonZero, Affine::IDENTITY, &RoundedRect::from_rect(r, 10.0));
        cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.gain_reduction.multiply_alpha(0.20), None, &shade(&shape, true));
        cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.gain_up.multiply_alpha(0.22), None, &shade(&shape, false));
        if creating.is_some() {
            let ghost = polyline(&points(&shape, offset));
            let color = if refused { theme.text_disabled } else { theme.accent.multiply_alpha(0.5) };
            cx.scene.stroke(&Stroke::new(1.6).with_dashes(0.0, [5.0, 4.0]), Affine::IDENTITY, color, None, &ghost);
        } else if Self::smart(cx.host).tracks_threshold() && offset.abs() > 0.05 {
            // Where the curve sits as you set it, before Smart's slide.
            let set = polyline(&points(stages, 0.0));
            cx.scene.stroke(&Stroke::new(1.6).with_dashes(0.0, [5.0, 4.0]), Affine::IDENTITY, theme.accent.multiply_alpha(0.5), None, &set);
        }
        let curve = polyline(&points(stages, offset));
        glow_stroke(cx, &curve, 2.4, theme.curve, theme.curve, 5.0);
        cx.scene.pop_layer();

        // Threshold diamonds where each stage starts, on the curve the stages below it make; then
        // the nodes.
        for slot in Self::ranked(stages) {
            let alpha = if self.selected.is_none() || self.selected == Some(slot) { 0.9 } else { 0.45 };
            let c = Self::diamond_pos(stages, slot, offset);
            let mut d = BezPath::new();
            d.move_to((c.x, c.y - 5.0));
            d.line_to((c.x + 5.0, c.y));
            d.line_to((c.x, c.y + 5.0));
            d.line_to((c.x - 5.0, c.y));
            d.close_path();
            mite_ui::paint::glow_fill(cx, &d, theme.accent.multiply_alpha(alpha), 3.0);
        }
        // A node a higher stage has taken the curve away from: this stage's own continuation to it.
        for slot in Self::ranked(stages) {
            let x = Self::node_input(stages, slot, offset);
            if (partial_curve_db(stages, slot, x, offset) - curve_db(stages, x, offset)).abs() < 0.1 {
                continue;
            }
            let t = Self::threshold_input(stages, slot, offset);
            let n = 48;
            let pts: Vec<Point> = (0..=n).map(|i| t + (x - t) * i as f32 / n as f32).map(|u| Point::new(sq_x(u), sq_y(partial_curve_db(stages, slot, u, offset)))).collect();
            cx.scene.stroke(&Stroke::new(1.4).with_dashes(0.0, [3.0, 4.0]), Affine::IDENTITY, theme.accent.multiply_alpha(0.55), None, &polyline(&pts));
        }
        for slot in Self::ranked(stages) {
            let p = Self::node_pos(stages, slot, offset);
            let selected = self.selected == Some(slot);
            let hot = matches!(self.hot, Some(Hot::Node(k)) if k == slot) || matches!(self.drag, Some(Drag::Node { slot: k, .. }) if k == slot);
            let soloed = Self::on(cx.host, &stage_id(slot, S_SOLO.id));
            let dim = Self::solo_slot(cx.host).is_some_and(|k| k != slot);
            let alpha = if dim { 0.35 } else { 1.0 };
            let r = if selected || hot { 9.0 } else { 8.0 };
            let fill = if selected { theme.accent } else { theme.accent.multiply_alpha(0.25) };
            cx.scene.fill(Fill::NonZero, Affine::IDENTITY, fill.multiply_alpha(alpha), None, &Circle::new(p, r));
            let ring = if selected || soloed { theme.needle } else { theme.accent };
            glow_stroke(cx, &Circle::new(p, r - 1.0), 2.0, ring.multiply_alpha(alpha), theme.accent, if selected || hot { 6.0 } else { 3.0 });
            let ink = if selected { theme.display } else { theme.accent_text };
            cx.text.draw(cx.scene, &Self::number(stages, slot).to_string(), p.x, p.y - 6.0, TextStyle::LABEL.sized(9.0).weighted(800.0), ink.multiply_alpha(alpha), Align::Center);
            if soloed {
                cx.text.draw(cx.scene, "S", p.x + 12.0, p.y - 16.0, TextStyle::LABEL.sized(9.0).weighted(800.0), theme.needle, Align::Center);
            }
        }
        if let (Some(_), Some(slot), Some(Drag::Create { .. })) = (creating, Self::free_slot(cx.host), self.drag) {
            // The node of the stage being made.
            let p = Self::node_pos(&shape, slot, offset);
            let color = if refused { theme.text_disabled } else { theme.accent };
            cx.scene.stroke(&Stroke::new(2.0).with_dashes(0.0, [3.0, 3.0]), Affine::IDENTITY, color, None, &Circle::new(p, 8.0));
        }

        // The live dot riding the curve at the detector's level, with its trail.
        let det = |r: &[f32; record::WIDTH]| r[record::DETECTOR];
        if let Some(last) = self.history.back().filter(|r| det(r) > -80.0) {
            let n = self.history.len();
            let trail: Vec<Point> = self.history.range(n.saturating_sub(40)..n).step_by(3).map(|r| {
                let x = det(r).clamp(-60.0, 0.0);
                Point::new(sq_x(x), sq_y(curve_db(stages, x, r[record::OFFSET])))
            }).collect();
            if trail.len() > 1 {
                cx.scene.stroke(&Stroke::new(2.0).with_caps(Cap::Round).with_dashes(0.0, [1.0, 5.0]), Affine::IDENTITY, theme.accent.multiply_alpha(0.28), None, &polyline(&trail));
            }
            let x = det(last).clamp(-60.0, 0.0);
            let p = Point::new(sq_x(x), sq_y(curve_db(stages, x, offset)));
            mite_ui::paint::glow_fill(cx, &Circle::new(p, 6.0), theme.needle, 4.0);
            cx.scene.stroke(&Stroke::new(3.0), Affine::IDENTITY, theme.accent.multiply_alpha(0.25), None, &Circle::new(p, 7.5));
        }
        // Empty: ghost grips above and below the line, pointing where to pull.
        if Self::ranked(stages).is_empty() && self.drag.is_none() {
            let x = -24.0;
            let c = Point::new(sq_x(x), sq_y(x));
            for (dy, color, up) in [(-26.0, theme.gain_up, true), (26.0, theme.accent, false)] {
                let g = Rect::from_center_size(c + (0.0, dy), (22.0, 14.0));
                let shape = RoundedRect::from_rect(g, 7.0);
                cx.scene.fill(Fill::NonZero, Affine::IDENTITY, color.multiply_alpha(0.12), None, &shape);
                cx.scene.stroke(&Stroke::new(1.5).with_dashes(0.0, [3.0, 2.0]), Affine::IDENTITY, color, None, &shape);
                let k = g.center();
                let s = if up { -1.0 } else { 1.0 };
                let mut chev = BezPath::new();
                chev.move_to((k.x - 3.0, k.y - 1.5 * s));
                chev.line_to((k.x, k.y + 1.5 * s));
                chev.line_to((k.x + 3.0, k.y - 1.5 * s));
                cx.scene.stroke(&Stroke::new(1.4).with_caps(Cap::Round).with_join(Join::Round), Affine::IDENTITY, color, None, &chev);
            }
        }
    }

    fn solo_slot(host: &dyn ParamHost) -> Option<usize> {
        (0..STAGES).find(|&i| Self::on(host, &stage_id(i, S_SOLO.id)) && Self::on(host, &stage_id(i, S_ON.id)))
    }

    /// Card `k` of the stage strip (stage k + 1 in threshold order).
    fn strip_rect(k: usize) -> Rect {
        let w = (SQ - 24.0) / 4.0;
        let x = SQ_X + k as f64 * (w + 8.0);
        Rect::new(x, STRIP_TOP, x + w, STRIP_TOP + STRIP_H)
    }

    /// The stage whose strip card is under `p`.
    fn card_at(stages: &[Stage; STAGES], p: Point) -> Option<usize> {
        let k = (0..STAGES).find(|&k| Self::strip_rect(k).contains(p))?;
        Self::ranked(stages).get(k).copied()
    }

    fn draw_strip(&self, cx: &mut DrawCx<'_>, stages: &[Stage; STAGES]) {
        let theme = cx.theme;
        let ranked = Self::ranked(stages);
        let now = self.history.back();
        for k in 0..STAGES {
            let r = Self::strip_rect(k);
            let shape = RoundedRect::from_rect(r, 10.0);
            let Some(&slot) = ranked.get(k) else {
                cx.scene.stroke(&Stroke::new(1.0).with_dashes(0.0, [4.0, 3.0]), Affine::IDENTITY, theme.grid_major.multiply_alpha(0.7), None, &shape);
                continue;
            };
            let s = &stages[slot];
            let selected = self.selected == Some(slot);
            let inert = s.ratio <= 1.001;
            let alpha = if inert { 0.35 } else { 1.0 };
            let hot = !selected && self.pointer.is_some_and(|p| r.contains(p));
            let border = if selected { theme.accent } else if hot { theme.accent.multiply_alpha(0.6) } else { theme.well_border };
            panel(cx.scene, shape, theme.well, border);
            // Number and quadrant.
            let badge = Point::new(r.x0 + 19.0, r.y0 + 18.0);
            cx.scene.fill(Fill::NonZero, Affine::IDENTITY, if selected { theme.accent } else { theme.accent.multiply_alpha(0.25) }, None, &Circle::new(badge, 9.0));
            cx.text.draw(cx.scene, &(k + 1).to_string(), badge.x, badge.y - 6.0, TextStyle::LABEL.sized(10.0).weighted(800.0), if selected { theme.display } else { theme.accent_text }, Align::Center);
            let qcolor = if s.quadrant.is_upward() { theme.gain_up } else { theme.accent };
            draw_glyph(cx.scene, glyph::DIAGONAL, Point::new(r.x0 + 46.0, badge.y), 20.0, theme.grid_zero, 1.0);
            draw_glyph(cx.scene, glyph::quadrant(s.quadrant), Point::new(r.x0 + 46.0, badge.y), 20.0, qcolor.multiply_alpha(alpha), 1.8);
            // The stage's gain now: a segmented bar, cut hanging from the top, lift rising.
            let g = now.map_or(0.0, |rec| rec[record::STAGE + slot]);
            let (top, bottom) = (r.y0 + 40.0, r.y1 - 8.0);
            let cx0 = r.center().x;
            let seg = Stroke::new(14.0).with_dashes(0.0, [2.0, 2.0]);
            cx.scene.stroke(&seg, Affine::IDENTITY, theme.meter_off, None, &Line::new((cx0, top), (cx0, bottom)));
            let len = (f64::from(g.abs()) * 8.0).min(bottom - top);
            if len >= 1.0 {
                let (bar, color) = if g > 0.0 { (Line::new((cx0, bottom), (cx0, bottom - len)), theme.gain_up) } else { (Line::new((cx0, top), (cx0, top + len)), theme.gain_reduction) };
                cx.scene.stroke(&seg, Affine::IDENTITY, color.multiply_alpha(alpha), None, &bar);
                cx.glow().stroke(&Stroke::new(14.0), Affine::IDENTITY, color.multiply_alpha(0.35 * alpha), None, &bar);
            }
        }
    }

    fn draw_meters(&self, cx: &mut DrawCx<'_>) {
        let theme = cx.theme;
        let wells = [1194.0, 1220.0, 1246.0];
        for x in wells {
            panel(cx.scene, RoundedRect::new(x, METER_TOP, x + 20.0, METER_BOTTOM, 3.0), theme.meter_well, theme.well_border);
        }
        let seg = Stroke::new(8.0).with_dashes(0.0, [2.0, 2.0]);
        let (top, bottom) = (METER_TOP + 4.0, METER_BOTTOM - 4.0);
        let span = bottom - top;
        let level_y = |db: f32| bottom - (f64::from(db.clamp(-60.0, 0.0) + 60.0) / 60.0 * span / 4.0).round() * 4.0;
        let gr_y = |g: f32| top + (f64::from((-g).clamp(0.0, 40.0)) * 14.0).min(span);
        let split = Self::link_split(cx.host);
        for (i, x) in [1199.0, 1209.0, 1225.0, 1235.0, 1251.0, 1261.0].into_iter().enumerate() {
            cx.scene.stroke(&seg, Affine::IDENTITY, theme.meter_off, None, &Line::new((x, top), (x, bottom)));
            let v = self.meters[i];
            let (line, color) = match i {
                0 | 1 => (Line::new((x, bottom), (x, level_y(v))), theme.spectrum_pre),
                2 | 3 => {
                    let g = if split || i == 2 { v } else { self.meters[2] };
                    (Line::new((x, top), (x, gr_y(g))), theme.gain_reduction)
                }
                _ => (Line::new((x, bottom), (x, level_y(v))), if Self::on(cx.host, LISTEN.id) { theme.sidechain } else { theme.curve }),
            };
            if (line.p1.y - line.p0.y).abs() >= 2.0 {
                cx.scene.stroke(&seg, Affine::IDENTITY, color, None, &line);
                if i >= 2 {
                    cx.glow().stroke(&Stroke::new(8.0), Affine::IDENTITY, color.multiply_alpha(0.3), None, &line);
                }
            }
        }
        if Self::smart(cx.host).tracks_threshold() {
            let y = gr_y(-Self::plain(cx.host, TARGET_GR.id));
            cx.scene.stroke(&Stroke::new(2.0), Affine::IDENTITY, theme.gain_reduction, None, &Line::new((1218.0, y), (1242.0, y)));
        }
        for (x, t) in [(1190.0, "IN"), (1216.0, "GR"), (1242.0, "OUT")] {
            cx.text.draw(cx.scene, t, x + 14.0, DISPLAY_TOP + 596.0, TextStyle::LABEL.sized(9.0), theme.text_lo, Align::Center);
        }
        if split {
            let ms = Self::index(cx.host, STEREO.id) == 1;
            let (a, b) = if ms { (glyph::MS, glyph::MS) } else { (glyph::LR, glyph::LR) };
            draw_glyph(cx.scene, a, Point::new(1226.0, DISPLAY_TOP + 12.0), 11.0, theme.text_mid, 1.3);
            draw_glyph(cx.scene, b, Point::new(1236.0, DISPLAY_TOP + 12.0), 11.0, theme.text_mid.multiply_alpha(0.6), 1.3);
        }
    }

    fn draw_states(&self, cx: &mut DrawCx<'_>) {
        let theme = cx.theme;
        // Listen: the display frame glows in the sidechain colour, a headphones pill over it.
        if Self::on(cx.host, LISTEN.id) {
            let frame = Rect::new(1.0, DISPLAY_TOP + 1.0, WIDTH - 1.0, HELP_TOP - 1.0);
            glow_stroke(cx, &frame, 2.0, theme.sidechain, theme.sidechain, 12.0);
            self.pill(cx, Point::new(300.0, DISPLAY_TOP + 30.0), glyph::LISTEN, theme.sidechain);
        } else if Self::on(cx.host, DELTA.id) {
            self.pill(cx, Point::new(360.0, DISPLAY_TOP + 30.0), glyph::DELTA, theme.accent);
        }
    }

    /// A status pill: a glyph in a glowing capsule (the help strip says what it means).
    fn pill(&self, cx: &mut DrawCx<'_>, c: Point, d: &str, color: Color) {
        let r = RoundedRect::from_rect(Rect::from_center_size(c, (48.0, 32.0)), 16.0);
        cx.scene.fill(Fill::NonZero, Affine::IDENTITY, color.multiply_alpha(0.18), None, &r);
        glow_stroke(cx, &r, 1.5, color, color, 6.0);
        draw_glyph(cx.scene, d, c, 18.0, color, 1.8);
    }

    // ---- panels -----------------------------------------------------------------------------

    fn knob(spec: ParamSpec, id: String, label: &'static str, center: Point) -> Knob {
        Knob::new(spec, label, KnobKind::Unipolar, center, 56.0).with_id(id)
    }

    fn build_knobs(&mut self) {
        let Some(p) = self.panel else { return };
        if self.knobs_for == Some(p) {
            return;
        }
        self.panel_knobs = match p {
            Panel::Stage(slot) => {
                let x0 = STAGE_PANEL.x0 + 14.0 + 250.0 + 12.0 + 116.0 + 12.0;
                let col = (STAGE_PANEL.x1 - 14.0 - 36.0 - 12.0 - x0) / 3.0;
                let at = |i: usize| Point::new(x0 + col * ((i % 3) as f64 + 0.5), STAGE_PANEL.y0 + 50.0 + 94.0 * (i / 3) as f64);
                vec![
                    Self::knob(S_THRESH, stage_id(slot, S_THRESH.id), "THRESHOLD", at(0)),
                    Self::knob(S_RATIO, stage_id(slot, S_RATIO.id), "RATIO", at(1)),
                    Self::knob(S_KNEE, stage_id(slot, S_KNEE.id), "KNEE", at(2)),
                    Self::knob(S_ATTACK, stage_id(slot, S_ATTACK.id), "ATTACK", at(3)).with_curve(ATTACK_DIAL),
                    Self::knob(S_RELEASE, stage_id(slot, S_RELEASE.id), "RELEASE", at(4)).with_curve(RELEASE_DIAL),
                    Self::knob(S_RANGE, stage_id(slot, S_RANGE.id), "RANGE", at(5)),
                ]
            }
            Panel::Custom => {
                let x0 = WIDE_PANEL.x0 + 16.0 + 170.0 + 16.0;
                let col = (WIDE_PANEL.x1 - 16.0 - 36.0 - 16.0 - x0) / 6.0;
                let y = WIDE_PANEL.center().y + 4.0;
                [C_ATTACK, C_RELEASE, C_DRIVE, C_TAIL, C_PROGRAM, C_BEND]
                    .into_iter()
                    .zip(["ATTACK ×", "RELEASE ×", "DRIVE", "TAIL", "PROGRAM", "BEND"])
                    .enumerate()
                    .map(|(i, (spec, label))| Self::knob(spec, spec.id.to_owned(), label, Point::new(x0 + col * (i as f64 + 0.5), y)))
                    .collect()
            }
            Panel::Detector => {
                let x0 = WIDE_PANEL.x0 + 16.0 + 170.0 + 16.0;
                let col = (WIDE_PANEL.x1 - 16.0 - 36.0 - 16.0 - x0) / 4.0;
                let y = WIDE_PANEL.center().y + 4.0;
                vec![
                    Self::knob(LOOKAHEAD, LOOKAHEAD.id.to_owned(), "LOOKAHEAD", Point::new(x0 + col * 0.5, y)),
                    Self::knob(HOLD, HOLD.id.to_owned(), "HOLD", Point::new(x0 + col * 1.5, y)),
                    Knob::new(MAKEUP, "MAKEUP", KnobKind::Bipolar, Point::new(x0 + col * 2.5, y), 56.0),
                ]
            }
            Panel::Stereo => vec![Self::knob(LINK, LINK.id.to_owned(), "LINK", Point::new(STEREO_POP.x1 - 70.0, STEREO_POP.center().y + 2.0))],
            Panel::Sidechain => Vec::new(),
        };
        self.knobs_for = Some(p);
    }

    fn panel_rect(p: Panel) -> Rect {
        match p {
            Panel::Stage(_) => STAGE_PANEL,
            Panel::Stereo => STEREO_POP,
            _ => WIDE_PANEL,
        }
    }

    fn draw_panel(&mut self, cx: &mut DrawCx<'_>, stages: &[Stage; STAGES]) {
        let Some(p) = self.panel else {
            self.hits = PanelHits::default();
            return;
        };
        self.build_knobs();
        let theme = cx.theme;
        let rect = Self::panel_rect(p);
        let radius = 14.0;
        if theme.shadows {
            cx.scene.draw_blurred_rounded_rect(Affine::translate((0.0, 14.0)), rect, Color::from_rgba8(0, 0, 0, 140), radius, 20.0);
        }
        glass_panel(cx, RoundedRect::from_rect(rect, radius), theme.blur_panel);
        let mut hits = PanelHits { rect, ..PanelHits::default() };
        let hot = |r: Rect| self.pointer.is_some_and(|q| r.contains(q));
        let round = Stroke::new(1.6).with_caps(Cap::Round);
        let close = Rect::new(rect.x1 - 14.0 - 36.0, rect.y0 + 12.0, rect.x1 - 14.0, rect.y0 + 48.0);
        if p != Panel::Stereo {
            let ink = icon_button(cx.scene, theme, close, theme.panel_control, hot(close), false, theme.accent);
            let k = close.center();
            let mut x = BezPath::new();
            x.move_to((k.x - 3.0, k.y - 3.0));
            x.line_to((k.x + 3.0, k.y + 3.0));
            x.move_to((k.x + 3.0, k.y - 3.0));
            x.line_to((k.x - 3.0, k.y + 3.0));
            cx.scene.stroke(&round, Affine::IDENTITY, ink, None, &x);
            hits.close = close;
        }
        match p {
            Panel::Stage(slot) => self.draw_stage_panel(cx, &mut hits, slot, stages),
            Panel::Custom => {
                self.panel_title(cx, rect, glyph::CHARACTERS[CUSTOM], "Custom", theme.accent);
            }
            Panel::Detector => {
                self.panel_title(cx, rect, glyph::SLIDERS, "Detector & output", theme.accent);
                let feedback = Self::index(cx.host, TOPOLOGY.id) == 1;
                for (i, (d, v)) in [(glyph::FF, 0.0), (glyph::FB, 1.0)].into_iter().enumerate() {
                    let r = Rect::new(rect.x0 + 16.0 + i as f64 * 85.0, rect.y0 + 50.0, rect.x0 + 16.0 + i as f64 * 85.0 + 79.0, rect.y0 + 88.0);
                    let on = feedback == (v == 1.0);
                    let ink = icon_button(cx.scene, theme, r, theme.panel_control, hot(r), on, theme.accent);
                    draw_glyph(cx.scene, d, r.center(), 22.0, ink, 1.7);
                    hits.icons.push((r, TOPOLOGY.id, v));
                }
                // Oversampling in the fourth column: 1×, 2×, 4× stacked, the one in force lit.
                let x0 = WIDE_PANEL.x0 + 16.0 + 170.0 + 16.0;
                let col = (WIDE_PANEL.x1 - 16.0 - 36.0 - 16.0 - x0) / 4.0;
                let (cx_os, cy) = (x0 + col * 3.5, WIDE_PANEL.center().y + 4.0);
                let factor = Self::index(cx.host, OVERSAMPLING.id);
                for (i, label) in OVERSAMPLING.choices.iter().enumerate() {
                    let y = cy - 45.0 + i as f64 * 32.0;
                    let r = Rect::new(cx_os - 30.0, y, cx_os + 30.0, y + 26.0);
                    let on = factor == i;
                    let ink = icon_button(cx.scene, theme, r, theme.panel_control, hot(r), on, theme.accent);
                    cx.text.draw(cx.scene, label, r.center().x, r.y0 + 6.0, TextStyle::LABEL.sized(10.0).weighted(800.0), ink, Align::Center);
                    hits.icons.push((r, OVERSAMPLING.id, i as f32));
                }
                // Auto makeup beside the MAKEUP dial.
                if let Some(k) = self.panel_knobs.get(2) {
                    let r = Rect::new(k.center.x + 36.0, k.center.y - 18.0, k.center.x + 72.0, k.center.y + 18.0);
                    let on = Self::on(cx.host, AUTO_MAKEUP.id);
                    let ink = icon_button(cx.scene, theme, r, theme.panel_control, hot(r), on, theme.accent);
                    mite_ui::widgets::footer::draw_toggle_glyph(cx.scene, ToggleGlyph::Auto, r.center(), ink);
                    hits.toggle = Some(r);
                }
            }
            Panel::Sidechain => self.draw_sidechain_panel(cx, &mut hits),
            Panel::Stereo => {
                let ms = Self::index(cx.host, STEREO.id) == 1;
                for (i, (d, v)) in [(glyph::LR, 0.0), (glyph::MS, 1.0)].into_iter().enumerate() {
                    let r = Rect::new(rect.x0 + 12.0, rect.y0 + 12.0 + i as f64 * 46.0, rect.x0 + 64.0, rect.y0 + 50.0 + i as f64 * 46.0);
                    let on = ms == (v == 1.0);
                    let ink = icon_button(cx.scene, theme, r, theme.panel_control, hot(r), on, theme.accent);
                    draw_glyph(cx.scene, d, r.center(), 22.0, ink, 1.7);
                    hits.icons.push((r, STEREO.id, v));
                }
            }
        }
        for k in &self.panel_knobs {
            k.draw(cx);
        }
        self.hits = hits;
    }

    fn panel_title(&self, cx: &mut DrawCx<'_>, rect: Rect, d: &str, title: &str, color: Color) {
        draw_glyph(cx.scene, d, Point::new(rect.x0 + 26.0, rect.y0 + 26.0), 20.0, color, 1.8);
        cx.text.draw(cx.scene, title, rect.x0 + 42.0, rect.y0 + 17.0, TextStyle::TITLE, cx.theme.text_hi, Align::Left);
    }

    fn draw_stage_panel(&self, cx: &mut DrawCx<'_>, hits: &mut PanelHits, slot: usize, stages: &[Stage; STAGES]) {
        let theme = cx.theme;
        let s = stages[slot];
        let rect = STAGE_PANEL;
        let hot = |r: Rect| self.pointer.is_some_and(|q| r.contains(q));
        // The envelope scope: a burst through the stage's real gain computer and ballistics.
        let scope = Rect::new(rect.x0 + 14.0, rect.y0 + 12.0, rect.x0 + 264.0, rect.y0 + 136.0);
        panel(cx.scene, RoundedRect::from_rect(scope, 8.0), theme.well, theme.well_border);
        let style = self.style(cx.host);
        // Long enough to see the recovery: auto release's slow part lets go several times slower.
        let slow = if s.auto_release { AUTO_SLOW_FACTOR } else { 1.0 };
        let window_ms = (260.0 + s.release_ms * style.release * slow * 2.2).clamp(300.0, if s.auto_release { 4000.0 } else { 1600.0 });
        let n = 250usize;
        let dt = window_ms / n as f32;
        // A 260 ms burst, 12 dB into the stage's active side, between rests 24 dB on the other
        // side (a stage acting below its threshold works in the rests).
        let (active, rest) = if s.quadrant.acts_above() { (s.threshold_db + 12.0, s.threshold_db - 24.0) } else { (s.threshold_db - 12.0, s.threshold_db + 24.0) };
        let level = |t: f32| if (3.0..260.0).contains(&t) { active } else { rest };
        let rate = 1000.0 / dt;
        let input: Vec<f32> = (0..=n).map(|i| level(i as f32 * dt).clamp(-60.0, 0.0)).collect();
        let gains = simulate_stage(&s, &style, rate, &input);
        let sx = |i: usize| scope.x0 + i as f64 * scope.width() / n as f64;
        let sy_in = |x: f32| scope.y1 - 6.0 - f64::from(x.clamp(-60.0, 0.0) + 60.0) / 60.0 * (scope.height() - 16.0);
        let sy_g = |g: f32| scope.y0 + (f64::from(g.abs()) * 4.5).min(scope.height() - 8.0);
        let up = s.quadrant.is_upward();
        let gcolor = if up { theme.gain_up } else { theme.gain_reduction };
        let mut fill_in = BezPath::new();
        fill_in.move_to((scope.x0, scope.y1));
        for (i, &x) in input.iter().enumerate() {
            fill_in.line_to((sx(i), sy_in(x)));
        }
        fill_in.line_to((scope.x1, scope.y1));
        fill_in.close_path();
        cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.spectrum_pre.multiply_alpha(0.12), None, &fill_in);
        let pts: Vec<Point> = gains.iter().enumerate().map(|(i, &g)| Point::new(sx(i), sy_g(g))).collect();
        let mut fill_g = polyline(&pts);
        fill_g.line_to((scope.x1, scope.y0));
        fill_g.line_to((scope.x0, scope.y0));
        fill_g.close_path();
        cx.scene.fill(Fill::NonZero, Affine::IDENTITY, gcolor.multiply_alpha(0.28), None, &fill_g);
        glow_stroke(cx, &polyline(&pts), 1.8, gcolor, gcolor, 3.0);
        // Attack (63 % of the way in) and release (63 % of the way back) handles.
        let peak = gains.iter().fold(0.0f32, |m, g| m.max(g.abs()));
        let off = ((260.0 / dt) as usize).min(n);
        let ia = gains.iter().position(|g| g.abs() >= 0.63 * peak).unwrap_or(0);
        let at_off = gains.get(off).map_or(0.0, |g| g.abs());
        let ir = off + gains[off..].iter().position(|g| g.abs() <= 0.37 * at_off).unwrap_or(n - off);
        for (attack, i) in [(true, ia), (false, ir.min(n))] {
            let x = sx(i);
            cx.scene.stroke(&Stroke::new(1.0).with_dashes(0.0, [2.0, 3.0]), Affine::IDENTITY, theme.accent, None, &Line::new((x, scope.y0), (x, scope.y1)));
            let c = Point::new(x, scope.y0 + 18.0 + if attack { 0.0 } else { 26.0 });
            mite_ui::paint::glow_fill(cx, &Circle::new(c, 5.5), theme.accent, 4.0);
            cx.scene.stroke(&Stroke::new(2.0), Affine::IDENTITY, theme.needle, None, &Circle::new(c, 4.5));
            hits.handles.push((attack, c));
        }
        hits.scope = Some(scope);
        // Title row: the stage's dot, number and quadrant glyph.
        let ty = scope.y1 + 22.0;
        mite_ui::paint::glow_fill(cx, &Circle::new(Point::new(scope.x0 + 6.0, ty), 4.5), theme.accent, 4.0);
        let title = format!("Stage {}", Self::number(stages, slot));
        let w = cx.text.draw(cx.scene, &title, scope.x0 + 18.0, ty - 9.0, TextStyle::TITLE, theme.text_hi, Align::Left);
        let gp = Point::new(scope.x0 + 18.0 + f64::from(w) + 16.0, ty);
        draw_glyph(cx.scene, glyph::DIAGONAL, gp, 20.0, theme.grid_zero, 1.2);
        draw_glyph(cx.scene, glyph::quadrant(s.quadrant), gp, 20.0, if up { theme.gain_up } else { theme.accent }, 1.8);
        // Quadrant radio (2 × 2) and the detector dropdown.
        let qx = scope.x1 + 12.0;
        for (i, q) in [Quadrant::Compress, Quadrant::Expand, Quadrant::UpCompress, Quadrant::UpExpand].into_iter().enumerate() {
            let r = Rect::new(qx + (i % 2) as f64 * 61.0, rect.y0 + 12.0 + (i / 2) as f64 * 46.0, qx + (i % 2) as f64 * 61.0 + 55.0, rect.y0 + 52.0 + (i / 2) as f64 * 46.0);
            let on = s.quadrant == q;
            let color = if q.is_upward() { theme.gain_up } else { theme.accent };
            let ink = icon_button(cx.scene, theme, r, theme.panel_control, hot(r), on, color);
            draw_glyph(cx.scene, glyph::DIAGONAL, r.center(), 22.0, theme.grid_zero, 1.0);
            draw_glyph(cx.scene, glyph::quadrant(q), r.center(), 22.0, ink, 1.8);
            hits.quadrants.push(r);
        }
        let det = Rect::new(qx, rect.y0 + 112.0, qx + 116.0, rect.y0 + 148.0);
        let open = matches!(self.menu, Some(Menu { kind: MenuKind::StageDetector(_), .. }));
        let d = s.detector as usize;
        draw_dropdown_button(cx, det, Detector::NAMES[d], open, hot(det), |scene, at| draw_glyph(scene, glyph::DETECTORS[d], at + (8.0, 6.0), 18.0, theme.accent, 1.8));
        hits.detector = Some(det);
        // Auto release under the detector (the same glyph as auto makeup).
        let r = Rect::new(qx, rect.y0 + 158.0, qx + 55.0, rect.y0 + 192.0);
        let ink = icon_button(cx.scene, theme, r, theme.panel_control, hot(r), s.auto_release, theme.accent);
        mite_ui::widgets::footer::draw_toggle_glyph(cx.scene, ToggleGlyph::Auto, r.center(), ink);
        hits.auto_release = Some(r);
        // The bin under the close button.
        let bin = Rect::new(rect.x1 - 14.0 - 36.0, rect.y1 - 12.0 - 36.0, rect.x1 - 14.0, rect.y1 - 12.0);
        let ink = icon_button(cx.scene, theme, bin, theme.panel_control, hot(bin), false, theme.danger);
        draw_bin(cx.scene, bin.center(), ink);
        hits.remove = Some(bin);
    }

    fn draw_sidechain_panel(&self, cx: &mut DrawCx<'_>, hits: &mut PanelHits) {
        let theme = cx.theme;
        let rect = WIDE_PANEL;
        let hot = |r: Rect| self.pointer.is_some_and(|q| r.contains(q));
        self.panel_title(cx, rect, glyph::KEY, "Sidechain", theme.sidechain);
        let external = Self::index(cx.host, SC_SOURCE.id) == 1;
        let src = Rect::new(rect.x0 + 16.0, rect.y0 + 50.0, rect.x0 + 186.0, rect.y0 + 86.0);
        let open = matches!(self.menu, Some(Menu { kind: MenuKind::KeySource, .. }));
        let (name, d) = if external { ("External", glyph::EXTERNAL) } else { ("Internal", glyph::INTERNAL) };
        draw_dropdown_button(cx, src, name, open, hot(src), |scene, at| draw_glyph(scene, d, at + (8.0, 6.0), 18.0, theme.sidechain, 1.8));
        hits.key_source = Some(src);
        let listen = Rect::new(rect.x0 + 16.0, rect.y0 + 96.0, rect.x0 + 186.0, rect.y0 + 134.0);
        let on = Self::on(cx.host, LISTEN.id);
        let ink = icon_button(cx.scene, theme, listen, theme.panel_control, hot(listen), on, theme.sidechain);
        draw_glyph(cx.scene, glyph::LISTEN, listen.center(), 22.0, ink, 1.7);
        hits.listen = Some(listen);
        // The detector EQ: its curve over 20 Hz…20 kHz, ±18 dB, and a node per band.
        let area = Rect::new(rect.x0 + 202.0, rect.y0 + 14.0, rect.x1 - 16.0 - 36.0 - 16.0, rect.y1 - 14.0);
        panel(cx.scene, RoundedRect::from_rect(area, 8.0), theme.well, theme.well_border);
        let fx = |f: f64| area.x0 + area.width() * (f / 20.0).ln() / 1000f64.ln();
        let gy = |g: f64| area.center().y - g * (area.height() / 2.0 - 12.0) / 18.0;
        for f in [100.0, 1000.0, 10_000.0] {
            cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, theme.grid_minor, None, &Line::new((fx(f), area.y0), (fx(f), area.y1)));
        }
        for g in [-12.0, -6.0, 6.0, 12.0] {
            cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, theme.grid_minor, None, &Line::new((area.x0, gy(g)), (area.x1, gy(g))));
        }
        cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, theme.grid_zero, None, &Line::new((area.x0, gy(0.0)), (area.x1, gy(0.0))));
        cx.text.draw(cx.scene, "SIDECHAIN EQ", area.x0 + 10.0, area.y0 + 8.0, TextStyle::LABEL.sized(10.0), theme.text_lo, Align::Left);
        let bands: Vec<BandSettings> = (0..KEY_BANDS).map(|b| Self::key_band(cx.host, b)).collect();
        let pts: Vec<Point> = (0..=160)
            .map(|i| {
                let f = 20.0 * 1000f64.powf(i as f64 / 160.0);
                let g: f64 = bands.iter().filter(|b| b.enabled).map(|b| analog_response_db(b, f)).sum();
                Point::new(fx(f), gy(g.clamp(-20.0, 20.0)))
            })
            .collect();
        cx.scene.push_clip_layer(Fill::NonZero, Affine::IDENTITY, &RoundedRect::from_rect(area, 8.0));
        glow_stroke(cx, &polyline(&pts), 2.0, theme.curve, theme.curve, 4.0);
        cx.scene.pop_layer();
        for (b, band) in bands.iter().enumerate() {
            let p = Self::key_node(area, band);
            let color = theme.band_ramp[0];
            let alpha = if band.enabled { 1.0 } else { 0.35 };
            cx.scene.fill(Fill::NonZero, Affine::IDENTITY, color.multiply_alpha(alpha), None, &Circle::new(p, 7.0));
            glow_stroke(cx, &Circle::new(p, 6.0), 2.0, color.multiply_alpha(alpha), color, if band.enabled { 4.0 } else { 1.0 });
            cx.text.draw(cx.scene, &(b + 1).to_string(), p.x, p.y - 5.5, TextStyle::LABEL.sized(8.0).weighted(800.0), theme.display, Align::Center);
        }
        hits.key_area = Some(area);
    }

    fn key_band(host: &dyn ParamHost, b: usize) -> BandSettings {
        let id = |p: &str| key_id(b, p);
        BandSettings {
            enabled: Self::on(host, &id(K_ON.id)),
            kind: BandType::from_index(Self::index(host, &id(K_TYPE.id))),
            freq: f64::from(Self::plain(host, &id(K_FREQ.id))),
            gain_db: f64::from(Self::plain(host, &id(K_GAIN.id))),
            q: f64::from(Self::plain(host, &id(K_Q.id))),
            ..BandSettings::default()
        }
    }

    fn key_node(area: Rect, band: &BandSettings) -> Point {
        let x = area.x0 + area.width() * (band.freq / 20.0).ln() / 1000f64.ln();
        let g = if band.kind.has_gain() { band.gain_db } else { 0.0 };
        Point::new(x, area.center().y - g * (area.height() / 2.0 - 12.0) / 18.0)
    }

    fn draw_footer(&mut self, cx: &mut DrawCx<'_>) {
        let theme = cx.theme;
        self.footer.draw_frame(cx);
        let top = FOOTER_TOP;
        let character = Self::index(cx.host, CHARACTER.id).min(CUSTOM);
        let smart = Self::index(cx.host, SMART.id).min(3);
        let open = |k: MenuKind| self.menu.as_ref().is_some_and(|m| m.kind == k);
        let char_r = Rect::new(16.0, top + 10.0, 166.0, top + 46.0);
        let smart_r = Rect::new(176.0, top + 10.0, 316.0, top + 46.0);
        let hot = |r: Rect| self.pointer.is_some_and(|p| r.contains(p));
        let lit = |on: bool| if on { theme.accent } else { theme.text_mid };
        draw_dropdown_button(cx, char_r, CHARACTERS[character], open(MenuKind::Character), hot(char_r), |scene, at| draw_glyph(scene, glyph::CHARACTERS[character], at + (8.0, 6.0), 22.0, lit(character != 0), 1.7));
        draw_dropdown_button(cx, smart_r, Smart::NAMES[smart], open(MenuKind::Smart), hot(smart_r), |scene, at| draw_glyph(scene, glyph::SMART[smart], at + (8.0, 6.0), 22.0, lit(smart != 0), 1.7));
        self.foot_hits = [char_r, smart_r];
        let sample_rate = f64::from(self.shared.sample_rate.load()).max(1.0);
        self.footer.draw_latency(cx, 332.0, f64::from(self.shared.latency.load()) / sample_rate * 1000.0);
        self.footer.draw_controls(cx);
    }

    fn draw_menu(&self, cx: &mut DrawCx<'_>) {
        let Some(menu) = &self.menu else { return };
        let theme = cx.theme;
        let kind = menu.kind;
        menu.list.draw(cx, theme.accent, |scene, i, at, ink| {
            let c = at + (11.0, 6.0);
            match kind {
                MenuKind::Character => draw_glyph(scene, glyph::CHARACTERS[i], c, 20.0, ink, 1.6),
                MenuKind::Smart => draw_glyph(scene, glyph::SMART[i], c, 20.0, ink, 1.6),
                MenuKind::StageDetector(_) => draw_glyph(scene, glyph::DETECTORS[i], c, 20.0, ink, 1.6),
                MenuKind::KeySource => draw_glyph(scene, if i == 1 { glyph::EXTERNAL } else { glyph::INTERNAL }, c, 20.0, ink, 1.6),
            }
        });
    }

    // ---- help -------------------------------------------------------------------------------

    fn help_text(&self, host: &dyn ParamHost) -> Help {
        let stages = Self::stages(host);
        let db = |v: f32| format!("{}{:.1} dB", if v < -0.05 { "−" } else if v > 0.05 { "+" } else { "" }, v.abs());
        let ratio = |r: f32| S_RATIO.format(r);
        if let Some(Drag::Create { start, input_db, now }) = self.drag {
            if Self::free_slot(host).is_none() {
                return Help::default().text("Four stages is the limit.");
            }
            if let Some(s) = Self::pulled_stage(host, input_db, start, now) {
                let n = Self::ranked(&stages).iter().filter(|&&i| stages[i].threshold_db < s.threshold_db).count() + 1;
                return Help::default()
                    .slot(format!("New stage {n}"), "New stage 4")
                    .text(" · ")
                    .slot(Quadrant::NAMES[s.quadrant as usize], "Upward compress")
                    .text(" · ")
                    .slot(db(s.threshold_db), "−60.0 dB")
                    .text(" · ")
                    .slot(ratio(s.ratio), "100:1")
                    .text(" — release to create, Esc cancels");
            }
        }
        if let Some(help) = self.session.help() {
            return help;
        }
        if self.pointer.is_some_and(|p| self.range.rect.contains(p)) {
            let how = if self.range.manual().is_some() { "picked" } else { "Auto: fits the music and the stages" };
            return Help::default().text("Level range ").slot(format!("{:.0} dB", self.range_shown), "60 dB").text(format!(" · {how} — click to pick, wheel over the left scale steps, double-click for Auto"));
        }
        if let Some(help) = self.footer.help(host) {
            return help;
        }
        let p = self.pointer.unwrap_or(Point::ZERO);
        if self.foot_hits[0].contains(p) {
            let c = Self::index(host, CHARACTER.id).min(CUSTOM);
            return Help::default().text(format!("Character · {} — changes how the stages move, not the curve", CHARACTERS[c]));
        }
        if self.foot_hits[1].contains(p) {
            return Help::default().text(match Self::smart(host) {
                Smart::Off => "Smart · Off — nothing adapts",
                Smart::Adaptive => "Smart · Adaptive — bends the release to the material",
                Smart::Vocal => "Smart · Vocal — the curve follows the voice to hold the target; dashed lines are where you set it",
                Smart::Master => "Smart · Master — the curve follows the mix gently to hold the target",
            });
        }
        for (i, words) in ["Sidechain — what the detector hears, through its own EQ", "Stereo — L/R or M/S, and how tightly the channels are linked", "Detector & output — topology, lookahead, hold, makeup, oversampling"].into_iter().enumerate() {
            if self.head_hits[i].contains(p) {
                return Help::default().text(words);
            }
        }
        if Self::on(host, LISTEN.id) {
            return Help::default().text("Listening to the detector — the plugin outputs the filtered sidechain, not your track. Click the headphones again to stop.");
        }
        if let Some(panel) = self.panel {
            if self.hits.rect.contains(p) {
                if let Some(k) = self.panel_knobs.iter().find(|k| k.bounds().contains(p)) {
                    let v = Self::plain(host, &k.id);
                    return Help::default().text(format!("{} · ", k.label)).slot(k.spec.format(v), k.spec.format(k.spec.max)).text(" — drag; double-click resets");
                }
                if let Some(&(_, _, v)) = self.hits.icons.iter().find(|(r, id, _)| *id == OVERSAMPLING.id && r.contains(p)) {
                    let factor = 1usize << (v as usize);
                    let latency_ms = Oversampler::latency_for(factor) as f32 / self.shared.sample_rate.load().max(1.0) * 1000.0;
                    return Help::default()
                        .text("Oversampling · ")
                        .slot(OVERSAMPLING.choices[v as usize], "4×")
                        .text(if factor == 1 { " — the compressor runs at the host's rate" } else { " — fast gain changes alias less; adds " })
                        .slot(if factor == 1 { String::new() } else { format!("{latency_ms:.1} ms") }, "0.0 ms")
                        .text(if factor == 1 { "" } else { " of latency" });
                }
                if self.hits.handles.iter().any(|(_, c)| c.distance(p) < 12.0) {
                    return Help::default().text("Drag sideways: attack (first handle) or release (second)");
                }
                if self.hits.auto_release.is_some_and(|r| r.contains(p)) {
                    return Help::default().text("Auto release — a short peak lets go at RELEASE, sustained compression recovers slowly, without pumping. Click to switch");
                }
                if let Panel::Stage(slot) = panel {
                    let s = stages[slot];
                    return Help::default()
                        .slot(format!("Stage {}", Self::number(&stages, slot)), "Stage 4")
                        .text(" · ")
                        .slot(Quadrant::NAMES[s.quadrant as usize], "Upward compress")
                        .text(" · ")
                        .slot(db(s.threshold_db), "−60.0 dB")
                        .text(" · ")
                        .slot(ratio(s.ratio), "100:1");
                }
                return Help::default();
            }
        }
        let last = self.history.back();
        let now = |k: usize| last.map_or(-150.0, |r| r[k].max(r[k + 1]));
        let gr = last.map_or(0.0, |r| r[record::CUT].min(r[record::CUT + 1]));
        if let Some(slot) = Self::card_at(&stages, p) {
            let s = stages[slot];
            return Help::default()
                .slot(format!("Stage {}", Self::number(&stages, slot)), "Stage 4")
                .text(" · ")
                .slot(Quadrant::NAMES[s.quadrant as usize], "Upward compress")
                .text(" · ")
                .slot(db(s.threshold_db), "−60.0 dB")
                .text(" · ")
                .slot(ratio(s.ratio), "100:1")
                .text(" — click to edit it; the bar is its gain now");
        }
        match self.hot {
            Some(Hot::Node(slot)) | Some(Hot::Diamond(slot)) => {
                let s = stages[slot];
                return Help::default()
                    .slot(format!("Stage {}", Self::number(&stages, slot)), "Stage 4")
                    .text(" · ")
                    .slot(Quadrant::NAMES[s.quadrant as usize], "Upward compress")
                    .text(" · ")
                    .slot(db(s.threshold_db), "−60.0 dB")
                    .text(" · ")
                    .slot(ratio(s.ratio), "100:1")
                    .text(" — drag across to move the threshold, away from the line for ratio, Alt-drag for knee, double-click resets");
            }
            Some(Hot::Grip(slot)) => {
                let s = stages[slot];
                return Help::default().slot(format!("Stage {}", Self::number(&stages, slot)), "Stage 4").text(" · threshold ").slot(db(s.threshold_db), "−60.0 dB").text(" — drag up or down; double-click resets");
            }
            Some(Hot::Target) => {
                return Help::default().text("Smart target · ").slot(db(Self::plain(host, TARGET_GR.id)), "24.0 dB").text(" average gain reduction — drag up or down");
            }
            Some(Hot::Square) | Some(Hot::Line) => {
                let (i, o) = sq_db(p);
                let _ = o;
                let out = curve_db(&stages, i, self.offset());
                return Help::default().text("IN ").slot(db(i), "−60.0 dB").text(" → OUT ").slot(db(out), "−60.0 dB").text(" — pull the line below to compress, above to lift; double-click makes a 4:1 stage");
            }
            _ => {}
        }
        if Self::ranked(&stages).is_empty() {
            return Help::default().text("No stages — the curve is on the line, so the plugin passes audio unchanged. Pull it below the line to compress.");
        }
        Help::default()
            .text("IN ")
            .slot(db(now(record::IN)), "−60.0 dB")
            .text(" · GR ")
            .slot(db(gr), "−60.0 dB")
            .text(" · OUT ")
            .slot(db(now(record::OUT)), "−60.0 dB")
            .text(" — pull the curve below the line to compress, above it to lift; drag a threshold grip on the history")
    }

    // ---- events -----------------------------------------------------------------------------

    fn toggle_menu(&mut self, kind: MenuKind, anchor: Rect, host: &dyn ParamHost) {
        if self.menu.as_ref().is_some_and(|m| m.kind == kind) {
            self.close_menu();
            return;
        }
        let (items, current): (&'static [&'static str], usize) = match kind {
            MenuKind::Character => (&CHARACTERS, Self::index(host, CHARACTER.id)),
            MenuKind::Smart => (&Smart::NAMES, Self::index(host, SMART.id)),
            MenuKind::StageDetector(slot) => (&Detector::NAMES, Self::index(host, &stage_id(slot, S_DET.id))),
            MenuKind::KeySource => (SC_SOURCE.choices, Self::index(host, SC_SOURCE.id)),
        };
        let list = Dropdown::new(anchor, items, current, (DISPLAY_TOP, FOOTER_TOP)).with_min_width(200.0);
        self.menu = Some(Menu { kind, list });
    }

    fn close_menu(&mut self) {
        self.menu = None;
        self.shared.audition.store(-1.0);
    }

    fn menu_event(&mut self, event: &UiEvent, host: &dyn ParamHost) -> bool {
        let Some(menu) = &mut self.menu else { return false };
        if let UiEvent::PointerMove { pos, .. } = *event {
            self.pointer = Some(pos);
        }
        let result = menu.list.event(event);
        let kind = menu.kind;
        // Hovering a Character auditions it (never the parameter: no automation, no undo).
        if kind == MenuKind::Character {
            let a = menu.list.hover.map_or(-1.0, |k| menu.list.shown[k] as f32);
            self.shared.audition.store(a);
        }
        match result {
            DropdownEvent::Open => {}
            DropdownEvent::Picked(i) => {
                match kind {
                    MenuKind::Character => {
                        // Custom starts from the Character in force.
                        if i == CUSTOM {
                            let from = Self::index(host, CHARACTER.id).min(CUSTOM);
                            if from < CUSTOM {
                                let st = STYLES[from];
                                Self::set(host, C_ATTACK.id, st.attack);
                                Self::set(host, C_RELEASE.id, st.release);
                                Self::set(host, C_DRIVE.id, st.drive_db);
                                Self::set(host, C_TAIL.id, st.slow_tail * 100.0);
                                Self::set(host, C_PROGRAM.id, st.program * 100.0);
                                Self::set(host, C_BEND.id, st.ratio_bend * 100.0);
                            }
                            self.panel = Some(Panel::Custom);
                            self.knobs_for = None;
                        }
                        Self::set(host, CHARACTER.id, i as f32);
                    }
                    MenuKind::Smart => Self::set(host, SMART.id, i as f32),
                    MenuKind::StageDetector(slot) => Self::set(host, &stage_id(slot, S_DET.id), i as f32),
                    MenuKind::KeySource => Self::set(host, SC_SOURCE.id, i as f32),
                }
                self.close_menu();
            }
            DropdownEvent::Closed => self.close_menu(),
        }
        true
    }

    /// Whether a gesture is under way (a drag becomes one undo step when it ends).
    fn busy(&self) -> bool {
        self.drag.is_some() || self.panel_knobs.iter().any(Knob::is_dragging) || self.footer.busy()
    }

    fn after_session(&mut self, host: &dyn ParamHost) {
        if self.session.take_replaced() {
            if self.selected.is_some_and(|s| !Self::on(host, &stage_id(s, S_ON.id))) {
                self.select(None);
            }
            self.panel_knobs.clear();
            self.knobs_for = None;
        }
    }

    fn key_event(&mut self, key: Key, mods: Modifiers, host: &dyn ParamHost) -> bool {
        if let Some(used) = self.session.key(key, mods, host) {
            return used;
        }
        let stages = Self::stages(host);
        match key {
            Key::Escape => {
                if self.menu.is_some() {
                    self.close_menu();
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
                    self.selected = None;
                    return true;
                }
                false
            }
            Key::Delete | Key::Backspace => match self.selected {
                Some(slot) => {
                    self.delete_stage(host, slot);
                    true
                }
                None => false,
            },
            Key::Up | Key::Down => {
                let Some(slot) = self.selected else { return false };
                let step = if mods.fine() { 0.1 } else { 0.5 } * if key == Key::Up { 1.0 } else { -1.0 };
                Self::set(host, &stage_id(slot, S_THRESH.id), (stages[slot].threshold_db + step).clamp(-60.0, 0.0));
                true
            }
            Key::PageUp | Key::PageDown => {
                let Some(slot) = self.selected else { return false };
                let r = S_RATIO.wheel(stages[slot].ratio, if key == Key::PageUp { 1.0 } else { -1.0 }, mods.fine());
                Self::set(host, &stage_id(slot, S_RATIO.id), r);
                true
            }
            Key::Left | Key::Right => {
                let ranked = Self::ranked(&stages);
                if ranked.is_empty() {
                    return false;
                }
                let at = self.selected.and_then(|s| ranked.iter().position(|&i| i == s));
                let next = match (at, key == Key::Right) {
                    (None, _) => 0,
                    (Some(k), true) => (k + 1).min(ranked.len() - 1),
                    (Some(k), false) => k.saturating_sub(1),
                };
                self.select(Some(ranked[next]));
                true
            }
            Key::Character(c) if !(mods.ctrl || mods.meta) => match (c.to_ascii_lowercase(), self.selected) {
                ('q', Some(slot)) => {
                    let q = (stages[slot].quadrant as usize + 1) % 4;
                    Self::set(host, &stage_id(slot, S_QUAD.id), q as f32);
                    true
                }
                ('s', Some(slot)) => {
                    let on = Self::on(host, &stage_id(slot, S_SOLO.id));
                    for i in 0..STAGES {
                        Self::set(host, &stage_id(i, S_SOLO.id), if i == slot && !on { 1.0 } else { 0.0 });
                    }
                    true
                }
                ('b', Some(slot)) => {
                    // Bypass a stage: ratio 1:1 leaves it inert (and keeps it).
                    Self::set(host, &stage_id(slot, S_RATIO.id), if stages[slot].ratio > 1.001 { 1.0 } else { 4.0 });
                    true
                }
                ('n', _) => {
                    if let Some(slot) = Self::free_slot(host) {
                        let (knee_db, attack_ms, release_ms, range_db, detector) = quadrant_defaults(Quadrant::Compress);
                        Self::write_stage(host, slot, &Stage { on: true, quadrant: Quadrant::Compress, threshold_db: -18.0, ratio: 4.0, knee_db, range_db, attack_ms, release_ms, auto_release: false, detector });
                        self.select(Some(slot));
                    }
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
        if self.range.event(event, self.range_shown) {
            return true;
        }
        if self.footer.event(event, host) {
            return true;
        }
        for k in &mut self.panel_knobs {
            if k.event(event, host) {
                return true;
            }
        }
        let stages = Self::stages(host);
        match *event {
            UiEvent::PointerMove { pos, mods } => {
                self.pointer = Some(pos);
                if let Some(drag) = self.drag {
                    self.drag_to(host, drag, pos, mods, &stages);
                    return true;
                }
                let hot = self.hot_at(host, pos);
                self.hot = hot;
                true
            }
            UiEvent::PointerLeft => {
                self.pointer = None;
                self.hot = None;
                true
            }
            UiEvent::PointerDown { pos, button: MouseButton::Left, mods } => self.press(host, pos, mods, &stages),
            UiEvent::PointerUp { button: MouseButton::Left, .. } => self.release(host),
            UiEvent::DoubleClick { pos, .. } => self.double_click(host, pos, &stages),
            UiEvent::Wheel { pos, lines, mods } => {
                let fine = mods.fine();
                match self.hot_at(host, pos) {
                    Some(Hot::Node(slot)) => {
                        let r = S_RATIO.wheel(stages[slot].ratio, lines, fine);
                        Self::set(host, &stage_id(slot, S_RATIO.id), r);
                        true
                    }
                    Some(Hot::Diamond(slot)) | Some(Hot::Grip(slot)) => {
                        let t = S_THRESH.wheel(stages[slot].threshold_db, lines, fine);
                        Self::set(host, &stage_id(slot, S_THRESH.id), t);
                        true
                    }
                    _ => false,
                }
            }
            _ => false,
        }
    }

    fn press(&mut self, host: &dyn ParamHost, pos: Point, mods: Modifiers, stages: &[Stage; STAGES]) -> bool {
        // Header buttons.
        if pos.y < HEADER {
            for (i, p) in [Panel::Sidechain, Panel::Stereo, Panel::Detector].into_iter().enumerate() {
                if self.head_hits[i].contains(pos) {
                    self.panel = if self.panel == Some(p) { None } else { Some(p) };
                    self.selected = None;
                    self.knobs_for = None;
                    return true;
                }
            }
            return true;
        }
        // Footer dropdowns.
        if pos.y > FOOTER_TOP {
            for (i, kind) in [MenuKind::Character, MenuKind::Smart].into_iter().enumerate() {
                if self.foot_hits[i].contains(pos) {
                    self.toggle_menu(kind, self.foot_hits[i], host);
                    return true;
                }
            }
            return true;
        }
        // The open panel.
        if let Some(p) = self.panel.filter(|_| self.hits.rect.contains(pos)) {
            let hits = self.hits.clone();
            if hits.close.contains(pos) {
                self.panel = None;
                self.selected = None;
                return true;
            }
            if let (Panel::Stage(slot), Some(bin)) = (p, hits.remove) {
                if bin.contains(pos) {
                    self.delete_stage(host, slot);
                    return true;
                }
            }
            if let Panel::Stage(slot) = p {
                if let Some(i) = hits.quadrants.iter().position(|r| r.contains(pos)) {
                    Self::set(host, &stage_id(slot, S_QUAD.id), i as f32);
                    return true;
                }
                if let Some(r) = hits.detector.filter(|r| r.contains(pos)) {
                    self.toggle_menu(MenuKind::StageDetector(slot), r, host);
                    return true;
                }
                if hits.auto_release.is_some_and(|r| r.contains(pos)) {
                    let id = stage_id(slot, S_AUTO_REL.id);
                    Self::set(host, &id, if Self::on(host, &id) { 0.0 } else { 1.0 });
                    return true;
                }
                if let Some(&(attack, _)) = hits.handles.iter().find(|(_, c)| c.distance(pos) < 12.0) {
                    let s = stages[slot];
                    let id = stage_id(slot, if attack { S_ATTACK.id } else { S_RELEASE.id });
                    host.begin_gesture(&id);
                    self.drag = Some(Drag::Scope { slot, attack, start: pos, value: if attack { s.attack_ms } else { s.release_ms } });
                    return true;
                }
            }
            if let Some(&(_, id, v)) = hits.icons.iter().find(|(r, _, _)| r.contains(pos)) {
                Self::set(host, id, v);
                return true;
            }
            if hits.toggle.is_some_and(|r| r.contains(pos)) {
                let on = Self::on(host, AUTO_MAKEUP.id);
                Self::set(host, AUTO_MAKEUP.id, if on { 0.0 } else { 1.0 });
                return true;
            }
            if let Some(r) = hits.key_source.filter(|r| r.contains(pos)) {
                self.toggle_menu(MenuKind::KeySource, r, host);
                return true;
            }
            if hits.listen.is_some_and(|r| r.contains(pos)) {
                let on = Self::on(host, LISTEN.id);
                Self::set(host, LISTEN.id, if on { 0.0 } else { 1.0 });
                return true;
            }
            if let Some(area) = hits.key_area.filter(|r| r.contains(pos)) {
                if let Some(band) = (0..KEY_BANDS).find(|&b| Self::key_node(area, &Self::key_band(host, b)).distance(pos) < 12.0) {
                    for p in [K_ON.id, K_FREQ.id, K_GAIN.id] {
                        host.begin_gesture(&key_id(band, p));
                    }
                    Self::set_raw(host, &key_id(band, K_ON.id), 1.0);
                    self.drag = Some(Drag::Key { band });
                }
                return true;
            }
            return true;
        }
        let _ = mods;
        // A stage's card in the strip selects it (as its node does).
        if let Some(slot) = Self::card_at(stages, pos) {
            self.select(Some(slot));
            return true;
        }
        match self.hot_at(host, pos) {
            Some(Hot::Node(slot)) => {
                self.select(Some(slot));
                let s = stages[slot];
                for p in [S_THRESH.id, S_RATIO.id, S_KNEE.id] {
                    host.begin_gesture(&stage_id(slot, p));
                }
                self.drag = Some(Drag::Node { slot, start: pos, threshold: s.threshold_db, knee: s.knee_db, lock: None });
                true
            }
            Some(Hot::Diamond(slot)) => {
                self.select(Some(slot));
                host.begin_gesture(&stage_id(slot, S_THRESH.id));
                self.drag = Some(Drag::Diamond { slot, start: pos, threshold: stages[slot].threshold_db });
                true
            }
            Some(Hot::Grip(slot)) => {
                self.select(Some(slot));
                host.begin_gesture(&stage_id(slot, S_THRESH.id));
                self.drag = Some(Drag::Grip { slot, start: pos, threshold: stages[slot].threshold_db });
                true
            }
            Some(Hot::Target) => {
                host.begin_gesture(TARGET_GR.id);
                self.drag = Some(Drag::Target { start: pos, target: Self::plain(host, TARGET_GR.id) });
                true
            }
            Some(Hot::Line) | Some(Hot::Square) => {
                // Pulling the line makes a stage; a plain click clears the selection.
                let (input_db, _) = sq_db(pos);
                self.drag = Some(Drag::Create { start: pos, input_db, now: pos });
                true
            }
            Some(Hot::History) => {
                if self.selected.is_some() {
                    self.select(None);
                }
                true
            }
            None => false,
        }
    }

    /// Writes without a gesture (inside one already begun).
    fn set_raw(host: &dyn ParamHost, id: &str, plain: f32) {
        host.set_normalized(id, host.normalize(id, plain));
    }

    fn drag_to(&mut self, host: &dyn ParamHost, drag: Drag, pos: Point, mods: Modifiers, stages: &[Stage; STAGES]) {
        let fine = if mods.fine() { 0.1 } else { 1.0 };
        let px_per_db = SQ / 60.0;
        match drag {
            Drag::Node { slot, start, threshold, knee, lock } => {
                let s = stages[slot];
                if mods.alt {
                    // Knee: 0–24 dB over 120 px, horizontal.
                    let k = (knee + ((pos.x - start.x) / 120.0 * 24.0 * fine) as f32).clamp(0.0, 24.0);
                    Self::set_raw(host, &stage_id(slot, S_KNEE.id), k);
                    return;
                }
                let (dx, dy) = (pos.x - start.x, pos.y - start.y);
                // Along the diagonal moves the threshold; away from it sets the ratio.
                let along = (dx - dy) / 2.0 / px_per_db;
                let lock = if mods.ctrl || mods.meta {
                    lock.or_else(|| (dx.hypot(dy) > 6.0).then(|| (dx - dy).abs() >= (dx + dy).abs()))
                } else {
                    None
                };
                self.drag = Some(Drag::Node { slot, start, threshold, knee, lock });
                if lock != Some(false) {
                    let t = (threshold + (along * fine) as f32).clamp(-60.0, 0.0);
                    Self::set_raw(host, &stage_id(slot, S_THRESH.id), t);
                }
                if lock != Some(true) {
                    // Against what the stages below make of the node's input.
                    let (_, y) = sq_db(pos);
                    let x = Self::node_input(stages, slot, 0.0);
                    let base = level_before(stages, slot, x, 0.0);
                    let depth = (base - level_thresholds(stages)[slot]).abs();
                    let r = Self::ratio_from_gain(s.quadrant, y - base, depth);
                    Self::set_raw(host, &stage_id(slot, S_RATIO.id), r);
                }
            }
            Drag::Diamond { slot, start, threshold } => {
                let along = ((pos.x - start.x) - (pos.y - start.y)) / 2.0 / px_per_db;
                Self::set_raw(host, &stage_id(slot, S_THRESH.id), (threshold + (along * fine) as f32).clamp(-60.0, 0.0));
            }
            Drag::Grip { slot, start, threshold } => {
                let d = lv_db(pos.y) - lv_db(start.y);
                Self::set_raw(host, &stage_id(slot, S_THRESH.id), (threshold + d * fine as f32).clamp(-60.0, 0.0));
            }
            Drag::Target { start, target } => {
                let d = ((pos.y - start.y) / gr_px_per_db() * fine) as f32;
                Self::set_raw(host, TARGET_GR.id, (target + d).clamp(0.0, 24.0));
            }
            Drag::Scope { slot, attack, start, value } => {
                // Time rows: ×2 per 50 px sideways (Shift finer).
                let v = value * 2f32.powf(((pos.x - start.x) / 50.0 * fine) as f32);
                let (id, spec) = if attack { (stage_id(slot, S_ATTACK.id), S_ATTACK) } else { (stage_id(slot, S_RELEASE.id), S_RELEASE) };
                Self::set_raw(host, &id, v.clamp(spec.min, spec.max));
            }
            Drag::Create { start, input_db, .. } => {
                self.drag = Some(Drag::Create { start, input_db, now: pos });
            }
            Drag::Key { band } => {
                let Some(area) = self.hits.key_area else { return };
                let f = 20.0 * 1000f64.powf(((pos.x - area.x0) / area.width()).clamp(0.0, 1.0));
                let g = ((area.center().y - pos.y) / (area.height() / 2.0 - 12.0) * 18.0).clamp(-18.0, 18.0);
                Self::set_raw(host, &key_id(band, K_FREQ.id), f as f32);
                if Self::key_band(host, band).kind.has_gain() {
                    Self::set_raw(host, &key_id(band, K_GAIN.id), g as f32);
                }
            }
        }
    }

    fn release(&mut self, host: &dyn ParamHost) -> bool {
        let Some(drag) = self.drag.take() else { return false };
        match drag {
            Drag::Node { slot, .. } => {
                for p in [S_THRESH.id, S_RATIO.id, S_KNEE.id] {
                    host.end_gesture(&stage_id(slot, p));
                }
            }
            Drag::Diamond { slot, .. } | Drag::Grip { slot, .. } => host.end_gesture(&stage_id(slot, S_THRESH.id)),
            Drag::Target { .. } => host.end_gesture(TARGET_GR.id),
            Drag::Scope { slot, attack, .. } => host.end_gesture(&stage_id(slot, if attack { S_ATTACK.id } else { S_RELEASE.id })),
            Drag::Key { band } => {
                for p in [K_ON.id, K_FREQ.id, K_GAIN.id] {
                    host.end_gesture(&key_id(band, p));
                }
            }
            Drag::Create { start, input_db, now } => match Self::pulled_stage(host, input_db, start, now) {
                Some(s) => {
                    if let Some(slot) = Self::free_slot(host) {
                        Self::write_stage(host, slot, &s);
                        self.select(Some(slot));
                    }
                }
                None => {
                    if self.selected.is_some() {
                        self.select(None);
                    }
                }
            },
        }
        true
    }

    fn double_click(&mut self, host: &dyn ParamHost, pos: Point, stages: &[Stage; STAGES]) -> bool {
        if let Some(p) = self.panel.filter(|_| self.hits.rect.contains(pos)) {
            if p == Panel::Sidechain {
                if let Some(area) = self.hits.key_area {
                    if let Some(band) = (0..KEY_BANDS).find(|&b| Self::key_node(area, &Self::key_band(host, b)).distance(pos) < 12.0) {
                        let on = Self::on(host, &key_id(band, K_ON.id));
                        Self::set(host, &key_id(band, K_ON.id), if on { 0.0 } else { 1.0 });
                    }
                }
            }
            return true;
        }
        match self.hot_at(host, pos) {
            Some(Hot::Node(slot)) => {
                // Ratio 1:1 makes the stage inert; a second double-click deletes it.
                if stages[slot].ratio <= 1.001 {
                    self.delete_stage(host, slot);
                } else {
                    Self::set(host, &stage_id(slot, S_RATIO.id), 1.0);
                }
                true
            }
            Some(Hot::Diamond(slot)) | Some(Hot::Grip(slot)) => {
                Self::set(host, &stage_id(slot, S_THRESH.id), S_THRESH.default);
                true
            }
            Some(Hot::Line) | Some(Hot::Square) => {
                self.drag = None;
                if let Some(slot) = Self::free_slot(host) {
                    let (input_db, _) = sq_db(pos);
                    let (knee_db, attack_ms, release_ms, range_db, detector) = quadrant_defaults(Quadrant::Compress);
                    Self::write_stage(host, slot, &Stage { on: true, quadrant: Quadrant::Compress, threshold_db: input_db.clamp(-60.0, 0.0), ratio: 4.0, knee_db, range_db, attack_ms, release_ms, auto_release: false, detector });
                    self.select(Some(slot));
                }
                true
            }
            _ => false,
        }
    }
}

/// A bin centred on `c`: removes the stage.
fn draw_bin(scene: &mut mite_ui::vello::Scene, c: Point, color: Color) {
    draw_glyph(scene, "M4.5 6.75 H19.5 M9.75 6.75 V4.5 H14.25 V6.75 M6.75 6.75 L7.8 20.25 H16.2 L17.25 6.75 M10.5 10.5 V16.5 M13.5 10.5 V16.5", c, 24.0, color, 1.4);
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

impl View for CompView {
    fn logical_size(&self) -> (f64, f64) {
        (WIDTH, HEIGHT)
    }

    fn draw(&mut self, cx: &mut DrawCx<'_>) {
        set_level_range(self.range_shown);
        let theme = cx.theme;
        let stages = Self::stages(cx.host);
        self.draw_header(cx);
        cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.display, None, &Rect::new(0.0, DISPLAY_TOP, WIDTH, HELP_TOP));
        cx.scene.push_clip_layer(Fill::NonZero, Affine::IDENTITY, &hist_rect());
        self.draw_history(cx, &stages);
        cx.scene.pop_layer();
        cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, theme.line, None, &Line::new((HIST_W + 0.5, DISPLAY_TOP), (HIST_W + 0.5, HELP_TOP)));
        self.range.draw(cx, self.range_shown);
        self.draw_thresholds(cx, &stages);
        self.draw_square(cx, &stages);
        self.draw_strip(cx, &stages);
        self.draw_meters(cx);
        self.draw_states(cx);
        self.help_text(cx.host).draw(cx, Rect::new(0.0, HELP_TOP, WIDTH, FOOTER_TOP));
        self.draw_footer(cx);
        self.draw_panel(cx, &stages);
        self.draw_menu(cx);
        self.range.draw_menu(cx);
        self.session.draw_menu(cx);
    }

    fn event(&mut self, event: &UiEvent, host: &dyn ParamHost) -> bool {
        set_level_range(self.range_shown);
        self.session.begin(host);
        let recording = RecordingHost::new(host);
        let changed = self.handle_event(event, &recording);
        let busy = self.busy();
        self.session.end(host, recording.wrote(), busy);
        self.after_session(host);
        changed
    }

    fn tick(&mut self, host: &dyn ParamHost) -> bool {
        self.ticks = self.ticks.wrapping_add(1);
        let busy = self.busy();
        self.session.follow(host, busy);
        let dt = self.last_tick.elapsed().as_secs_f32().min(0.1);
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
        // The level range: Auto fits the music (its quiet tenth, silence aside) and the deepest
        // stage's line, with a little room; a picked range is kept. It glides.
        let stages = Self::stages(host);
        let deepest = Self::ranked(&stages).iter().map(|&s| -Self::threshold_input(&stages, s, 0.0) + 6.0).fold(0.0f32, f32::max);
        let mut levels: Vec<f32> = self.history.iter().rev().take(1200).map(|r| r[record::IN].max(r[record::IN + 1])).filter(|&l| l > -80.0).collect();
        let content = if levels.len() > 50 {
            levels.sort_by(f32::total_cmp);
            -levels[levels.len() / 10] + 3.0
        } else {
            0.0
        };
        let auto = if deepest == 0.0 && content == 0.0 { 60.0 } else { deepest.max(content) };
        self.range_target = self.range.target(auto);
        let moved = ease_range(&mut self.range_shown, self.range_target, dt);
        set_level_range(self.range_shown);
        // Meters: instant attack, 20 dB/s release (GR returns at the same speed).
        if let Some(r) = self.history.back().copied() {
            let targets = [r[record::IN], r[record::IN + 1], r[record::CUT], r[record::CUT + 1], r[record::OUT], r[record::OUT + 1]];
            for (i, (m, t)) in self.meters.iter_mut().zip(targets).enumerate() {
                let fall = METER_RELEASE_DB_S * dt;
                *m = if i == 2 || i == 3 { if t < *m { t } else { (*m + fall).min(t) } } else if t > *m { t } else { (*m - fall).max(t) };
            }
        }
        // While audio flows the history scrolls every frame; when it stops, the meters fall.
        let flowing = self.last_fresh.elapsed().as_secs_f32() < 0.25;
        fresh || flowing || moved || self.drag.is_some() || self.meters.iter().enumerate().any(|(i, &m)| if i == 2 || i == 3 { m < -0.01 } else { m > -149.0 })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pull_sets_the_ratio_by_its_length() {
        assert_eq!(pull_ratio(3.0), 1.0);
        assert!((pull_ratio(PULL_4) - 4.0).abs() < 0.01);
        assert_eq!(pull_ratio(PULL_INF + 10.0), RATIO_MAX);
    }

    #[test]
    fn the_node_ratio_round_trips_through_its_gain() {
        for q in [Quadrant::Compress, Quadrant::Expand, Quadrant::UpCompress, Quadrant::UpExpand] {
            let s = Stage { on: true, quadrant: q, threshold_db: -30.0, ratio: 3.0, knee_db: 0.0, range_db: 60.0, ..Stage::default() };
            let mut stages = [Stage::default(); STAGES];
            stages[0] = s;
            let x = if q.acts_above() { -30.0 + NODE_OFFSET_DB } else { -30.0 - NODE_OFFSET_DB };
            let gain = curve_db(&stages, x, 0.0) - x;
            assert!((CompView::ratio_from_gain(q, gain, NODE_OFFSET_DB) - 3.0).abs() < 0.01, "{q:?}");
        }
    }
}
