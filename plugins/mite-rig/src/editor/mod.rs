//! The Mite Rig editor (docs/design/rig-brief.md §3), in 1280×760 design units.
//!
//! Top: the board, IN to OUT, its blocks as tiles on wires; a split forks into its lanes and
//! merges them again. Between tiles, insertion points (+) add a block (or, on the main row, a
//! split); tiles drag to another insertion point; a tile's light switches its bypass. Below: the
//! selected block's simple view (its knobs and, for captures and cabinets, its file: open,
//! TONE3000, empty), or a split's lanes (level, pan, how many). TONE3000 opens over everything.

mod board_view;
mod browser;
mod shapes;

use std::path::PathBuf;
use std::sync::mpsc::Receiver;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Instant;

use mite_core::{ParamDisplay, ParamScale, ParamSpec};
use mite_ui::kurbo::{Affine, BezPath, Cap, Circle, Join, Line, Point, Rect, RoundedRect, Stroke};
use mite_ui::paint::{glow_stroke, icon_button, panel};
use mite_ui::peniko::{Color, Fill};
use mite_ui::session::{Session, SessionConfig};
use mite_ui::widgets::brand::{draw_mark, draw_title};
use mite_ui::widgets::{Footer, FooterDial, FooterToggle, Help, Knob, KnobKind, ToggleGlyph};
use mite_ui::{Align, DrawCx, Key, MouseButton, ParamHost, RecordingHost, TextStyle, UiEvent, View};

use crate::blocks::Kind;
use crate::board::{Block, Item, LaneLive, Live, Place};
use crate::params::*;
use crate::presets::{preset_ids, FACTORY, PLUGIN};
use crate::rig::{Gear, SlotFile, Status};
use crate::RigUi;
pub use board_view::{Layout, AREA};
use board_view::{Gap, Tile, CY, IN_X};
use browser::Browser;
pub use browser::PANEL as BROWSER_PANEL;

/// Mite Rig wears its accent (tokens.json `color.accent.rig`, tube glow).
pub const ACCENT: &str = "rig";

const VIEW_W: f64 = 1280.0;
const VIEW_H: f64 = 760.0;
const HEADER: f64 = 56.0;
const DISPLAY_TOP: f64 = HEADER;
const HELP_TOP: f64 = DISPLAY_TOP + 620.0;
const FOOTER_TOP: f64 = HELP_TOP + 28.0;

/// The selected block's view.
const PANEL: Rect = Rect::new(16.0, AREA.y1 + 10.0, VIEW_W - 76.0, HELP_TOP - 12.0);
const KNOB_X0: f64 = PANEL.x0 + 430.0;
const KNOB_STEP: f64 = 112.0;
const KNOB_Y: f64 = PANEL.y0 + 150.0;
const KNOB_D: f64 = 64.0;
const BUTTON: f64 = 34.0;

const WELLS: [f64; 2] = [1210.0, 1240.0];
const METER_TOP: f64 = AREA.y1 + 14.0;
const METER_BOTTOM: f64 = HELP_TOP - 30.0;
const METER_RANGE: f64 = 60.0;
const SILENT: f32 = -150.0;

const POWER_GLYPH: &str = "M12 3.5 V11 M7.2 6.6 A7.4 7.4 0 1 0 16.8 6.6";
const FOLDER_GLYPH: &str = "M3.5 7 L3.5 18 L20.5 18 L20.5 9 L11.5 9 L9.5 7 Z";
const CLEAR_GLYPH: &str = "M7.5 7.5 L16.5 16.5 M16.5 7.5 L7.5 16.5";
const TRASH_GLYPH: &str = "M5 7 L19 7 M9.5 7 L9.5 4.5 L14.5 4.5 L14.5 7 M7 7 L8 20 L16 20 L17 7";

fn draw_glyph(scene: &mut mite_ui::vello::Scene, d: &str, c: Point, size: f64, color: Color, width: f64) {
    let Ok(mut path) = BezPath::from_svg(d) else { return };
    let k = size / 24.0;
    path.apply_affine(Affine::translate((c.x - 12.0 * k, c.y - 12.0 * k)) * Affine::scale(k));
    scene.stroke(&Stroke::new(width).with_caps(Cap::Round).with_join(Join::Round), Affine::IDENTITY, color, None, &path);
}

/// A stand-in host holding every Mite Rig parameter at its default (tests, preview, tools).
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
            _ => spec_for(id).expect("every manifest line has a spec"),
        };
        host = host.with(spec);
    }
    host
}

/// The engine's settings as a host's parameters set them.
pub fn settings(host: &dyn ParamHost) -> crate::engine::RigSettings {
    rig_settings(|id| host.plain(id, host.normalized(id)))
}

// ---- hosts for a block's and a lane's values ----------------------------------------------------

/// A block's values as a `ParamHost`, so the suite's knobs drive them.
struct BlockHost<'a>(&'a Live);

impl BlockHost<'_> {
    fn spec(&self, id: &str) -> Option<(usize, ParamSpec)> {
        self.0.kind.params().iter().enumerate().find(|(_, p)| p.spec.id == id).map(|(i, p)| (i, p.spec))
    }
}

impl ParamHost for BlockHost<'_> {
    fn normalized(&self, id: &str) -> f32 {
        self.spec(id).map_or(0.0, |(i, s)| s.normalize(self.0.value(i)))
    }
    fn plain(&self, id: &str, normalized: f32) -> f32 {
        self.spec(id).map_or(0.0, |(_, s)| s.denormalize(normalized))
    }
    fn normalize(&self, id: &str, plain: f32) -> f32 {
        self.spec(id).map_or(0.0, |(_, s)| s.normalize(plain))
    }
    fn default_normalized(&self, id: &str) -> f32 {
        self.spec(id).map_or(0.0, |(_, s)| s.normalize(s.default))
    }
    fn begin_gesture(&self, _id: &str) {}
    fn set_normalized(&self, id: &str, normalized: f32) {
        if let Some((i, s)) = self.spec(id) {
            self.0.set(i, s.denormalize(normalized));
        }
    }
    fn end_gesture(&self, _id: &str) {}
}

const LANE_LEVEL: ParamSpec = ParamSpec { id: "level", name: "Level", unit: "dB", min: -60.0, max: 12.0, default: 0.0, scale: ParamScale::Linear, smoothing_ms: 20.0, decimals: 1, wheel_step: 0.5, wheel_step_fine: 0.1, display: ParamDisplay::Number };
const LANE_PAN: ParamSpec = ParamSpec { id: "pan", name: "Pan", unit: "", min: -1.0, max: 1.0, default: 0.0, scale: ParamScale::Linear, smoothing_ms: 20.0, decimals: 2, wheel_step: 0.05, wheel_step_fine: 0.01, display: ParamDisplay::Number };

/// A lane's level and pan as a `ParamHost`.
struct LaneHost<'a>(&'a LaneLive);

impl LaneHost<'_> {
    fn spec(id: &str) -> ParamSpec {
        if id == LANE_PAN.id { LANE_PAN } else { LANE_LEVEL }
    }
    fn get(&self, id: &str) -> f32 {
        if id == LANE_PAN.id { self.0.pan.load() } else { self.0.level_db.load() }
    }
}

impl ParamHost for LaneHost<'_> {
    fn normalized(&self, id: &str) -> f32 {
        Self::spec(id).normalize(self.get(id))
    }
    fn plain(&self, id: &str, normalized: f32) -> f32 {
        Self::spec(id).denormalize(normalized)
    }
    fn normalize(&self, id: &str, plain: f32) -> f32 {
        Self::spec(id).normalize(plain)
    }
    fn default_normalized(&self, id: &str) -> f32 {
        Self::spec(id).normalize(Self::spec(id).default)
    }
    fn begin_gesture(&self, _id: &str) {}
    fn set_normalized(&self, id: &str, normalized: f32) {
        let v = Self::spec(id).denormalize(normalized);
        if id == LANE_PAN.id { self.0.pan.store(v) } else { self.0.level_db.store(v) }
    }
    fn end_gesture(&self, _id: &str) {}
}

// ---- the view -----------------------------------------------------------------------------------

/// What is selected: a block, or a split.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Selection {
    Block(u64),
    Split(u64),
}

/// Buttons in the selected block's view.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PanelButton {
    Bypass,
    Open,
    Tone3000,
    Clear,
    Remove,
    Size,
    /// A block's switch or choice parameter (its index), at a place in the button grid (0…3
    /// the first row, 4… the second).
    Switch { param: usize, at: usize },
    /// A split's lane count.
    Lanes(usize),
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Hot {
    Tile(u64),
    Led(u64),
    Gap(Place),
    Fork(u64),
    Menu(usize),
    Panel(PanelButton),
}

/// The add menu, open at an insertion point.
struct Menu {
    place: Place,
    at: Point,
}

impl Menu {
    fn entries(&self) -> Vec<(Option<Kind>, usize, &'static str, &'static str)> {
        let mut e: Vec<_> = Kind::ALL.iter().map(|k| (Some(*k), 0, k.name(), k.words())).collect();
        if matches!(self.place, Place::Main(_)) {
            e.push((None, 2, "Split · 2 lanes", "Fork the chain into two lanes that merge again"));
            e.push((None, 3, "Split · 3 lanes", "Fork the chain into three lanes that merge again"));
        }
        e
    }

    /// Rows per column (the entries run down the first column, then the second).
    fn rows(&self) -> usize {
        self.entries().len().div_ceil(2)
    }

    fn rect(&self) -> Rect {
        let h = self.rows() as f64 * 32.0 + 12.0;
        let w = 2.0 * 300.0 + 12.0;
        let x0 = (self.at.x - 20.0).clamp(12.0, VIEW_W - w - 12.0);
        let y0 = if self.at.y + 18.0 + h < HELP_TOP { self.at.y + 18.0 } else { (self.at.y - 18.0 - h).max(DISPLAY_TOP + 4.0) };
        Rect::new(x0, y0, x0 + w, y0 + h)
    }

    fn row(&self, i: usize) -> Rect {
        let r = self.rect();
        let (col, k) = (i / self.rows(), i % self.rows());
        let x = r.x0 + 6.0 + col as f64 * 300.0;
        Rect::new(x, r.y0 + 6.0 + k as f64 * 32.0, x + 300.0, r.y0 + 6.0 + (k + 1) as f64 * 32.0)
    }
}

/// A tile being dragged: which, from where, and whether it has moved far enough to count.
struct Drag {
    id: u64,
    from: Point,
    at: Point,
    moving: bool,
}

pub struct RigView {
    ui: Arc<RigUi>,
    session: Session,
    footer: Footer,
    selected: Option<Selection>,
    /// The selected block's (or split's lanes') knobs, rebuilt when the selection changes.
    knobs: Vec<Knob>,
    knobs_for: Option<Selection>,
    scroll: f64,
    menu: Option<Menu>,
    drag: Option<Drag>,
    pointer: Option<Point>,
    hot: Option<Hot>,
    meters: [f32; 2],
    last_tick: Instant,
    started: Instant,
    picking: Option<(u64, Receiver<Option<PathBuf>>)>,
    browser: Option<Browser>,
    seen: u64,
    items: Vec<Item>,
}

impl RigView {
    pub fn new(ui: Arc<RigUi>) -> Self {
        let footer = Footer::new(
            FOOTER_TOP,
            (VIEW_W, VIEW_H),
            vec![FooterToggle { id: BYPASS.id, glyph: ToggleGlyph::Bypass, words: "Bypass — the input as it came (in time: the latency stays)" }],
            vec![
                FooterDial { spec: INPUT, kind: KnobKind::Bipolar, name: "INPUT", words: "Input gain, into the board" },
                FooterDial { spec: MIX, kind: KnobKind::Unipolar, name: "MIX", words: "Mix — the board against the dry input (in time)" },
                FooterDial { spec: OUT_GAIN, kind: KnobKind::Bipolar, name: "OUTPUT", words: "Output gain" },
            ],
        );
        let items = ui.board.items();
        let first = items.iter().find_map(|i| if let Item::Block(b) = i { (b.kind() == Kind::Amp).then(|| b.id()) } else { None });
        Self {
            session: Session::new(SessionConfig { plugin: PLUGIN, factory: FACTORY, sound_ids: preset_ids(), snapshot_ids: preset_ids() }, ui.snapshots.clone(), ui.preset.clone()),
            footer,
            selected: first.map(Selection::Block),
            knobs: Vec::new(),
            knobs_for: None,
            scroll: 0.0,
            menu: None,
            drag: None,
            pointer: None,
            hot: None,
            meters: [SILENT, SILENT],
            last_tick: Instant::now(),
            started: Instant::now(),
            picking: None,
            browser: None,
            seen: u64::MAX,
            items,
            ui,
        }
    }

    /// Keeps the user's presets in `dir` instead of the settings folder (tests).
    pub fn with_preset_dir(mut self, dir: PathBuf) -> Self {
        self.session = self.session.with_preset_dir(dir);
        self
    }

    // ---- for tests and tools ----------------------------------------------------------------

    pub fn layout(&self) -> Layout {
        Layout::new(&self.items, self.scroll)
    }

    pub fn selection(&self) -> Option<Selection> {
        self.selected
    }

    pub fn select(&mut self, s: Option<Selection>) {
        self.selected = s;
    }

    pub fn panel_button(&self, b: PanelButton) -> Rect {
        panel_button(b)
    }

    pub fn knob_at(&self, id: &str) -> Option<Point> {
        self.knobs.iter().find(|k| k.id == id).map(|k| k.center)
    }

    pub fn menu_row(&self, i: usize) -> Option<Rect> {
        self.menu.as_ref().map(|m| m.row(i))
    }

    pub fn browser_open(&self) -> bool {
        self.browser.is_some()
    }

    /// Opens TONE3000 for block `id` (tools).
    pub fn open_browser(&mut self, id: u64) {
        if let Some(b) = self.ui.board.block(id) {
            self.browser = Some(Browser::new(id, b.kind(), &self.ui));
        }
    }

    /// Reads the board now (tools render a single frame).
    pub fn settle(&mut self) {
        self.read_board();
        self.rebuild_knobs();
    }

    // ---- reading ----------------------------------------------------------------------------

    fn read_board(&mut self) -> bool {
        let v = self.ui.board.version();
        if v == self.seen {
            return false;
        }
        self.seen = v;
        self.items = self.ui.board.items();
        // A selection that is gone goes too.
        match self.selected {
            Some(Selection::Block(id)) if self.block(id).is_none() => self.selected = None,
            Some(Selection::Split(id)) if self.split_lanes(id).is_none() => self.selected = None,
            _ => {}
        }
        true
    }

    fn block(&self, id: u64) -> Option<Block> {
        self.items.iter().find_map(|i| match i {
            Item::Block(b) if b.id() == id => Some(b.clone()),
            Item::Split(s) => s.lanes.iter().find_map(|l| l.blocks.iter().find(|b| b.id() == id).cloned()),
            _ => None,
        })
    }

    fn split_lanes(&self, id: u64) -> Option<Vec<Arc<LaneLive>>> {
        self.items.iter().find_map(|i| match i {
            Item::Split(s) if s.id == id => Some(s.lanes.iter().map(|l| l.live.clone()).collect()),
            _ => None,
        })
    }

    fn rebuild_knobs(&mut self) {
        if self.knobs_for == self.selected {
            return;
        }
        self.knobs_for = self.selected;
        self.knobs.clear();
        match self.selected {
            Some(Selection::Block(id)) => {
                let Some(b) = self.block(id) else { return };
                let dials: Vec<_> = b.kind().params().iter().filter(|p| !p.toggle && p.choices.is_empty()).collect();
                let (step, d) = if dials.len() > 6 { (92.0, 54.0) } else { (KNOB_STEP, KNOB_D) };
                let mut x = KNOB_X0;
                for p in dials {
                    let kind = if p.spec.min < 0.0 && p.spec.max > 0.0 { KnobKind::Bipolar } else { KnobKind::Unipolar };
                    let label: &'static str = Box::leak(p.spec.name.to_uppercase().into_boxed_str());
                    self.knobs.push(Knob::new(p.spec, label, kind, Point::new(x, KNOB_Y), d));
                    x += step;
                }
            }
            Some(Selection::Split(id)) => {
                let n = self.split_lanes(id).map_or(0, |l| l.len());
                for j in 0..n {
                    let x = KNOB_X0 + j as f64 * 2.0 * KNOB_STEP;
                    self.knobs.push(Knob::new(LANE_LEVEL, "LEVEL", KnobKind::Bipolar, Point::new(x, KNOB_Y), KNOB_D).with_id(format!("{j}:level")));
                    self.knobs.push(Knob::new(LANE_PAN, "PAN", KnobKind::Bipolar, Point::new(x + KNOB_STEP, KNOB_Y), KNOB_D).with_id(format!("{j}:pan")));
                }
            }
            None => {}
        }
    }

    fn label_of(&self, b: &Block) -> (String, String) {
        let file = b.live.file();
        let status = b.live.status();
        let stem = file.path.as_ref().and_then(|p| p.file_stem()).map(|s| s.to_string_lossy().into_owned());
        match b.kind() {
            Kind::Pedal | Kind::Amp | Kind::Cab => {
                let title = file.source.as_ref().map(|s| s.title.clone()).filter(|t| !t.is_empty());
                let name = match &status {
                    Status::Model(info) => title.or(info.name.clone()).or(stem),
                    _ => title.or(stem),
                };
                let gear = file.gear(&status);
                let kind = match (b.kind(), gear) {
                    (Kind::Amp, Gear::FullRig) => "Combo · full rig".to_owned(),
                    (k, _) => k.name().to_owned(),
                };
                (name.unwrap_or_else(|| "Empty".to_owned()), kind)
            }
            k => (k.name().to_owned(), String::new()),
        }
    }

    // ---- drawing ----------------------------------------------------------------------------

    fn draw_header(&mut self, cx: &mut DrawCx<'_>) {
        let theme = cx.theme;
        cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.window, None, &Rect::new(0.0, 0.0, VIEW_W, HEADER));
        cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, theme.line, None, &Line::new((0.0, HEADER - 0.5), (VIEW_W, HEADER - 0.5)));
        draw_mark(cx, 16.0, 15.0, 26.0);
        draw_title(cx, 52.0, 18.0, "Rig");
        self.session.draw(cx);
    }

    fn fit(cx: &mut DrawCx<'_>, text: &str, style: TextStyle, width: f64) -> String {
        if f64::from(cx.text.measure(text, style)) <= width {
            return text.to_owned();
        }
        let mut s = text.to_owned();
        while !s.is_empty() {
            s.pop();
            let t = format!("{}…", s.trim_end());
            if f64::from(cx.text.measure(&t, style)) <= width {
                return t;
            }
        }
        "…".to_owned()
    }

    fn draw_board(&self, cx: &mut DrawCx<'_>) {
        let theme = cx.theme;
        let layout = self.layout();
        cx.begin_clip(&AREA);
        let wire = Stroke::new(2.0).with_caps(Cap::Round);
        let wire_color = theme.text_lo.multiply_alpha(0.7);
        // IN and OUT.
        for (x, t) in [(IN_X, "IN"), (layout.out_x, "OUT")] {
            let r = RoundedRect::new(x - 22.0, CY - 13.0, x + 22.0, CY + 13.0, 13.0);
            cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.panel_control, None, &r);
            cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, theme.panel_control_border, None, &r);
            cx.text.draw(cx.scene, t, x, CY - 7.0, TextStyle::LABEL.sized(9.5), theme.text_mid, Align::Center);
        }
        // The wires: the main row, then each split's fork, lanes and merge.
        for &(x0, x1) in &layout.runs {
            cx.scene.stroke(&wire, Affine::IDENTITY, wire_color, None, &Line::new((x0.max(IN_X + 22.0), CY), (x1.min(layout.out_x - 22.0), CY)));
        }
        for f in &layout.forks {
            let selected = self.selected == Some(Selection::Split(f.id));
            for &(y, x0, x1) in f.rows.iter().take(f.lanes) {
                let mut p = BezPath::new();
                p.move_to(f.fork);
                p.curve_to((f.fork.x + 18.0, f.fork.y), (x0 - 18.0, y), (x0, y));
                p.line_to((x1, y));
                p.curve_to((x1 + 18.0, y), (f.merge.x - 18.0, f.merge.y), (f.merge.x, f.merge.y));
                cx.scene.stroke(&wire, Affine::IDENTITY, if selected { theme.accent.multiply_alpha(0.8) } else { wire_color }, None, &p);
            }
            for pt in [f.fork, f.merge] {
                let hot = self.hot == Some(Hot::Fork(f.id)) || selected;
                cx.scene.fill(Fill::NonZero, Affine::IDENTITY, if hot { theme.accent } else { theme.panel_control }, None, &Circle::new(pt, 7.0));
                cx.scene.stroke(&Stroke::new(1.5), Affine::IDENTITY, if hot { theme.accent } else { wire_color }, None, &Circle::new(pt, 7.0));
            }
        }
        // Insertion points.
        for g in &layout.gaps {
            let hot = self.hot == Some(Hot::Gap(g.place)) || self.menu.as_ref().is_some_and(|m| m.place == g.place);
            let dragging = self.drag.as_ref().is_some_and(|d| d.moving);
            let near = dragging && self.pointer.is_some_and(|p| layout.gap_at(p, 60.0).is_some_and(|n| n.place == g.place));
            let r = if hot || near { 11.0 } else { 8.0 };
            cx.scene.fill(Fill::NonZero, Affine::IDENTITY, if hot || near { theme.accent } else { theme.display }, None, &Circle::new(g.at, r));
            cx.scene.stroke(&Stroke::new(1.3), Affine::IDENTITY, if hot || near { theme.accent } else { theme.text_lo }, None, &Circle::new(g.at, r));
            let ink = if hot || near { theme.display } else { theme.text_mid };
            let s = r * 0.5;
            cx.scene.stroke(&Stroke::new(1.5).with_caps(Cap::Round), Affine::IDENTITY, ink, None, &Line::new((g.at.x - s, g.at.y), (g.at.x + s, g.at.y)));
            cx.scene.stroke(&Stroke::new(1.5).with_caps(Cap::Round), Affine::IDENTITY, ink, None, &Line::new((g.at.x, g.at.y - s), (g.at.x, g.at.y + s)));
        }
        // The tiles.
        for t in &layout.tiles {
            if self.drag.as_ref().is_some_and(|d| d.moving && d.id == t.id) {
                self.draw_tile(cx, t, 0.35);
            } else {
                self.draw_tile(cx, t, 1.0);
            }
        }
        // The tile being dragged follows the pointer.
        if let (Some(d), true) = (&self.drag, self.drag.as_ref().is_some_and(|d| d.moving)) {
            if let Some(t) = layout.tiles.iter().find(|t| t.id == d.id) {
                let moved = Tile { rect: t.rect + (d.at - d.from), ..*t };
                self.draw_tile(cx, &moved, 0.9);
            }
        }
        cx.end_clip();
        // More board than fits: a scroll bar under it (the wheel scrolls).
        if layout.width > VIEW_W {
            let track = Rect::new(24.0, AREA.y1 - 10.0, VIEW_W - 24.0, AREA.y1 - 6.0);
            let frac = VIEW_W / layout.width;
            let x0 = track.x0 + (self.scroll / layout.width) * track.width();
            let thumb = Rect::new(x0, track.y0, (x0 + frac * track.width()).min(track.x1), track.y1);
            cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.well, None, &RoundedRect::from_rect(track, 2.0));
            cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.text_lo, None, &RoundedRect::from_rect(thumb, 2.0));
        }
    }

    fn draw_tile(&self, cx: &mut DrawCx<'_>, t: &Tile, alpha: f32) {
        let theme = cx.theme;
        let Some(b) = self.block(t.id) else { return };
        let bypassed = b.live.bypassed();
        let selected = self.selected == Some(Selection::Block(t.id));
        let hot = self.hot == Some(Hot::Tile(t.id));
        let status = b.live.status();
        let gear = b.live.file().gear(&status);
        let look = shapes::Look::of(b.kind(), gear);
        let area = Rect::new(t.rect.x0, t.rect.y0, t.rect.x1, t.rect.y0 + board_view::LOOK_H);
        let ink = shapes::Ink {
            body: theme.panel_control.multiply_alpha(alpha),
            edge: (if selected { theme.accent } else if hot { theme.accent.multiply_alpha(0.6) } else { theme.text_lo }).multiply_alpha(alpha),
            detail: (if bypassed { theme.text_lo.multiply_alpha(0.6) } else { theme.text_mid }).multiply_alpha(alpha),
            lit: (if bypassed { theme.text_lo } else { theme.accent }).multiply_alpha(alpha),
        };
        let outline = shapes::draw(cx, look, area, &ink);
        if selected {
            glow_stroke(cx, &outline, 1.8, theme.accent, theme.accent.multiply_alpha(0.6), 5.0);
        }
        // The name under it (a file's in two lines), or the block's own and its main value.
        let (name, _) = self.label_of(&b);
        let failed = matches!(status, Status::Failed(_));
        let loading = matches!(status, Status::Loading);
        let ink_text = (if bypassed { theme.text_lo } else { theme.text_strong }).multiply_alpha(alpha);
        let style = TextStyle::BODY.sized(11.5).weighted(700.0);
        let y = area.y1 + 4.0;
        let cxm = t.rect.center().x;
        let w = t.rect.width() - 4.0;
        if b.kind().has_file() {
            let line = if failed { "can't load".to_owned() } else if loading { "loading…".to_owned() } else { name };
            let color = if failed { theme.danger } else if loading { theme.accent } else { ink_text };
            let words: Vec<&str> = line.split_whitespace().collect();
            let mut first = String::new();
            let mut i = 0;
            while i < words.len() {
                let next = if first.is_empty() { words[i].to_owned() } else { format!("{first} {}", words[i]) };
                if f64::from(cx.text.measure(&next, style)) > w && !first.is_empty() {
                    break;
                }
                first = next;
                i += 1;
            }
            let first = Self::fit(cx, &first, style, w);
            cx.text.draw(cx.scene, &first, cxm, y, style, color, Align::Center);
            let rest = words[i..].join(" ");
            if !rest.is_empty() {
                let rest = Self::fit(cx, &rest, style, w);
                cx.text.draw(cx.scene, &rest, cxm, y + 15.0, style, color, Align::Center);
            }
        } else {
            cx.text.draw(cx.scene, b.kind().name(), cxm, y, style, ink_text, Align::Center);
            cx.text.draw(cx.scene, &Self::summary(&b), cxm, y + 15.0, TextStyle::BODY.sized(10.0), theme.text_mid.multiply_alpha(alpha), Align::Center);
        }
        // The on/off light, on the block itself.
        let led = shapes::led(look, area);
        let on = !bypassed;
        let lhot = self.hot == Some(Hot::Led(t.id));
        cx.scene.fill(Fill::NonZero, Affine::IDENTITY, if on { theme.accent } else { theme.well }, None, &Circle::new(led, 4.5));
        cx.scene.stroke(&Stroke::new(if lhot { 2.0 } else { 1.0 }), Affine::IDENTITY, if on || lhot { theme.accent } else { theme.text_lo }, None, &Circle::new(led, 4.5));
        if on && alpha > 0.5 {
            glow_stroke(cx, &Circle::new(led, 4.5), 1.0, theme.accent, theme.accent, 4.0);
        }
    }

    /// Where a tile's on/off light is.
    pub fn led_of(&self, t: &Tile) -> Option<Point> {
        let b = self.block(t.id)?;
        let look = shapes::Look::of(b.kind(), b.live.file().gear(&b.live.status()));
        Some(shapes::led(look, Rect::new(t.rect.x0, t.rect.y0, t.rect.x1, t.rect.y0 + board_view::LOOK_H)))
    }

    /// A tile's line of numbers (its main knob).
    fn summary(b: &Block) -> String {
        let params = b.kind().params();
        match b.kind() {
            Kind::Tone => {
                let v: Vec<String> = (0..3).map(|i| format!("{:+.0}", b.live.value(i))).collect();
                format!("B {} M {} T {}", v[0], v[1], v[2])
            }
            Kind::Tuner => match crate::pedals::note_of(b.live.readout[0].load(), b.live.value(1)) {
                Some((note, cents)) => format!("{note}  {cents:+.0} ¢"),
                None => "—".to_owned(),
            },
            Kind::Comp => format!("{:.1} dB", b.live.readout[0].load().min(0.0)),
            Kind::Delay | Kind::Mod | Kind::Reverb | Kind::Tremolo => {
                let p = params.iter().position(|p| !p.choices.is_empty()).unwrap_or(0);
                params[p].choices.get(b.live.value(p).round().max(0.0) as usize).copied().unwrap_or("").to_owned()
            }
            _ => params.first().map(|p| p.spec.format(b.live.value(0))).unwrap_or_default(),
        }
    }

    fn draw_menu(&self, cx: &mut DrawCx<'_>) {
        let Some(m) = &self.menu else { return };
        let theme = cx.theme;
        let r = m.rect();
        let shape = RoundedRect::from_rect(r, 10.0);
        mite_ui::paint::glass_panel(cx, shape, theme.blur_menu);
        cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.display.multiply_alpha(0.94), None, &shape);
        cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, theme.glass_border, None, &shape);
        for (i, (_, _, name, words)) in m.entries().iter().enumerate() {
            let row = m.row(i);
            if self.hot == Some(Hot::Menu(i)) {
                cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.accent.multiply_alpha(0.14), None, &RoundedRect::from_rect(row, 6.0));
            }
            cx.text.draw(cx.scene, name, row.x0 + 10.0, row.y0 + 3.0, TextStyle::BODY.sized(12.5).weighted(700.0), theme.text_strong, Align::Left);
            let w = Self::fit(cx, words, TextStyle::BODY.sized(9.5), row.width() - 20.0);
            cx.text.draw(cx.scene, &w, row.x0 + 10.0, row.y0 + 18.0, TextStyle::BODY.sized(9.5), theme.text_lo, Align::Left);
        }
    }

    fn panel_buttons(&self) -> Vec<PanelButton> {
        match self.selected {
            Some(Selection::Block(id)) => {
                let Some(b) = self.block(id) else { return Vec::new() };
                let mut v = vec![PanelButton::Bypass, PanelButton::Remove];
                if b.kind().has_file() {
                    v.extend([PanelButton::Open, PanelButton::Tone3000, PanelButton::Clear]);
                }
                if b.kind().is_capture() {
                    v.push(PanelButton::Size);
                }
                // Switches and choices: after the file's buttons (and the size), or on the first row.
                let start = if b.kind().has_file() { 5 } else { 0 };
                v.extend(b.kind().params().iter().enumerate().filter(|(_, p)| p.toggle || !p.choices.is_empty()).enumerate().map(|(k, (i, _))| PanelButton::Switch { param: i, at: start + k }));
                v
            }
            Some(Selection::Split(_)) => vec![PanelButton::Lanes(2), PanelButton::Lanes(3), PanelButton::Remove],
            None => Vec::new(),
        }
    }

    fn draw_panel(&mut self, cx: &mut DrawCx<'_>) {
        let theme = cx.theme;
        let shape = RoundedRect::from_rect(PANEL, 12.0);
        panel(cx.scene, shape, theme.panel_control, theme.panel_control_border);
        let label = TextStyle::LABEL.sized(9.5);
        match self.selected {
            None => {
                cx.text.draw(cx.scene, "Click a block to set it up, or a + to add one (a split forks the chain into parallel lanes).", PANEL.center().x, PANEL.center().y - 8.0, TextStyle::BODY.sized(13.0), theme.text_mid, Align::Center);
                return;
            }
            Some(Selection::Split(id)) => {
                let n = self.split_lanes(id).map_or(0, |l| l.len());
                cx.text.draw(cx.scene, "SPLIT", PANEL.x0 + 22.0, PANEL.y0 + 18.0, label, theme.accent, Align::Left);
                cx.text.draw(cx.scene, &format!("{n} lanes"), PANEL.x0 + 22.0, PANEL.y0 + 36.0, TextStyle::BODY.sized(20.0).weighted(800.0), theme.text_strong, Align::Left);
                cx.text.draw(cx.scene, "Each lane runs its own blocks; they merge again here, in time.", PANEL.x0 + 22.0, PANEL.y0 + 68.0, TextStyle::BODY.sized(11.5), theme.text_mid, Align::Left);
                for j in 0..n {
                    let x = KNOB_X0 + j as f64 * 2.0 * KNOB_STEP + KNOB_STEP / 2.0;
                    cx.text.draw(cx.scene, &format!("LANE {}", j + 1), x, PANEL.y0 + 70.0, label, theme.text_hi, Align::Center);
                }
            }
            Some(Selection::Block(id)) => {
                let Some(b) = self.block(id) else { return };
                let (name, kind) = self.label_of(&b);
                let top = if kind.is_empty() || kind == b.kind().name() { b.kind().name().to_uppercase() } else { format!("{} · {}", b.kind().name().to_uppercase(), kind.to_uppercase()) };
                cx.text.draw(cx.scene, &top, PANEL.x0 + 22.0, PANEL.y0 + 18.0, label, theme.accent, Align::Left);
                let title = Self::fit(cx, &name, TextStyle::BODY.sized(20.0).weighted(800.0), 360.0);
                cx.text.draw(cx.scene, &title, PANEL.x0 + 22.0, PANEL.y0 + 36.0, TextStyle::BODY.sized(20.0).weighted(800.0), theme.text_strong, Align::Left);
                let lines = self.block_words(&b);
                for (k, (l, c)) in lines.iter().enumerate() {
                    let l = Self::fit(cx, l, TextStyle::BODY.sized(11.5), 380.0);
                    let color = match c {
                        1 => theme.danger,
                        2 => theme.text_lo,
                        _ => theme.text_mid,
                    };
                    cx.text.draw(cx.scene, &l, PANEL.x0 + 22.0, PANEL.y0 + 68.0 + k as f64 * 18.0, TextStyle::BODY.sized(11.5), color, Align::Left);
                }
            }
        }
        // Buttons.
        for pb in self.panel_buttons() {
            let r = panel_button(pb);
            let on = self.button_on(cx.host, pb);
            let hot = self.hot == Some(Hot::Panel(pb));
            let ink = icon_button(cx.scene, theme, r, theme.control, hot, on, theme.accent);
            let text = |cx: &mut DrawCx<'_>, t: &str| {
                cx.text.draw(cx.scene, t, r.center().x, r.center().y - 7.5, TextStyle::BODY.sized(12.0).weighted(700.0), ink, Align::Center);
            };
            match pb {
                PanelButton::Bypass => draw_glyph(cx.scene, POWER_GLYPH, r.center(), 22.0, ink, 1.8),
                PanelButton::Open => draw_glyph(cx.scene, FOLDER_GLYPH, r.center(), 22.0, ink, 1.6),
                PanelButton::Clear => draw_glyph(cx.scene, CLEAR_GLYPH, r.center(), 20.0, ink, 1.6),
                PanelButton::Remove => draw_glyph(cx.scene, TRASH_GLYPH, r.center(), 20.0, ink, 1.6),
                PanelButton::Tone3000 => text(cx, "TONE3000"),
                PanelButton::Size => text(cx, if Self::plain(cx.host, SIZE.id) >= 0.5 { "A2 Lite" } else { "A2 Full" }),
                PanelButton::Lanes(n) => text(cx, &format!("{n} lanes")),
                PanelButton::Switch { param, .. } => {
                    let word = self.selected_block().map(|b| {
                        let p = b.kind().params()[param];
                        if p.choices.is_empty() { p.spec.name.to_owned() } else { p.choices.get(b.live.value(param).round().max(0.0) as usize).copied().unwrap_or("").to_owned() }
                    });
                    text(cx, &word.unwrap_or_default());
                }
            }
        }
        // Knobs, against the block's (or the lanes') own values.
        match self.selected {
            Some(Selection::Block(id)) => {
                if let Some(b) = self.block(id) {
                    let host = BlockHost(&b.live);
                    let mut sub = DrawCx { scene: &mut *cx.scene, theme: cx.theme, text: &mut *cx.text, host: &host, size: cx.size, fx: &mut *cx.fx };
                    for k in &self.knobs {
                        k.draw(&mut sub);
                    }
                }
            }
            Some(Selection::Split(id)) => {
                if let Some(lanes) = self.split_lanes(id) {
                    for k in &self.knobs {
                        let (j, rest) = k.id.split_once(':').unwrap_or(("0", "level"));
                        let Some(lane) = lanes.get(j.parse::<usize>().unwrap_or(0)) else { continue };
                        let host = LaneHost(lane);
                        let mut sub = DrawCx { scene: &mut *cx.scene, theme: cx.theme, text: &mut *cx.text, host: &host, size: cx.size, fx: &mut *cx.fx };
                        let _ = rest;
                        k.draw(&mut sub);
                    }
                }
            }
            None => {}
        }
    }

    fn selected_block(&self) -> Option<Block> {
        match self.selected {
            Some(Selection::Block(id)) => self.block(id),
            _ => None,
        }
    }

    fn button_on(&self, host: &dyn ParamHost, pb: PanelButton) -> bool {
        match pb {
            PanelButton::Bypass => self.selected_block().is_some_and(|b| !b.live.bypassed()),
            PanelButton::Size => Self::plain(host, SIZE.id) >= 0.5,
            PanelButton::Switch { param, .. } => self.selected_block().is_some_and(|b| b.kind().params()[param].toggle && b.live.value(param) >= 0.5),
            PanelButton::Lanes(n) => matches!(self.selected, Some(Selection::Split(id)) if self.split_lanes(id).is_some_and(|l| l.len() == n)),
            _ => false,
        }
    }

    /// A block's lines under its name: (text, 0 normal / 1 error / 2 quiet).
    fn block_words(&self, b: &Block) -> Vec<(String, u8)> {
        let file = b.live.file();
        let status = b.live.status();
        let mut out = Vec::new();
        match &status {
            Status::Empty if b.kind() == Kind::Cab => out.push(("Empty: open an IR (WAV, AIFF, FLAC) or find one on TONE3000".to_owned(), 0)),
            Status::Empty if b.kind() == Kind::Pedal => out.push(("Empty: open a pedal's .nam capture or find one on TONE3000".to_owned(), 0)),
            Status::Empty if b.kind() == Kind::Amp => out.push(("Empty: open an amp's (or a combo's) .nam capture or find one on TONE3000".to_owned(), 0)),
            Status::Empty => out.push((b.kind().words().to_owned(), 0)),
            Status::Loading => out.push(("Loading…".to_owned(), 0)),
            Status::Failed(e) => out.push((format!("Could not load it: {e}"), 1)),
            Status::Model(info) => {
                let g = if info.generation == mite_nam::Generation::A2 { "A2" } else { "A1" };
                let mut parts = vec![format!("{g} {}", info.architecture)];
                if info.sizes.len() > 1 {
                    parts.push(if info.size_index + 1 == info.sizes.len() { "Full".to_owned() } else { "Lite".to_owned() });
                }
                parts.push(format!("{} kHz", info.sample_rate.map_or(48.0, |r| r / 1000.0)));
                if info.loudness.is_none() {
                    parts.push("no loudness".to_owned());
                }
                out.push((parts.join(" · "), 0));
                if file.gear(&status) == Gear::FullRig {
                    out.push(("A full rig: amp and cabinet in one capture (no Cab needed after it)".to_owned(), 2));
                }
            }
            Status::Cab(info) => {
                let mut t = format!("IR · {:.0} ms · {} kHz", info.seconds * 1000.0, info.file_rate / 1000.0);
                if info.truncated {
                    t.push_str(" · cut to 500 ms");
                }
                out.push((t, 0));
            }
        }
        if let Some(s) = &file.source {
            out.push((format!("TONE3000 · {} · {}", s.creator, if s.license.is_empty() { "license on the site".to_owned() } else { s.license.to_uppercase() }), 2));
        }
        // What the block hears now.
        match b.kind() {
            Kind::Tuner => out.push((format!("Hearing: {}", Self::summary(b)), 0)),
            Kind::Comp => out.push((format!("Reduction now: {}", Self::summary(b)), 2)),
            _ => {}
        }
        out
    }

    fn draw_meters(&self, cx: &mut DrawCx<'_>) {
        let theme = cx.theme;
        let seg = Stroke::new(8.0).with_dashes(0.0, [2.0, 2.0]);
        let (top, bottom) = (METER_TOP + 4.0, METER_BOTTOM - 4.0);
        let y = |db: f32| bottom - ((f64::from(db) + METER_RANGE).clamp(0.0, METER_RANGE) / METER_RANGE * (bottom - top) / 4.0).round() * 4.0;
        for (i, x) in WELLS.iter().enumerate() {
            panel(cx.scene, RoundedRect::new(*x, METER_TOP, x + 20.0, METER_BOTTOM, 3.0), theme.meter_well, theme.well_border);
            let cx0 = x + 10.0;
            cx.scene.stroke(&seg, Affine::IDENTITY, theme.meter_off, None, &Line::new((cx0, top), (cx0, bottom)));
            let v = self.meters[i];
            let line = Line::new((cx0, bottom), (cx0, y(v)));
            if (line.p1.y - line.p0.y).abs() >= 2.0 {
                let color = if v > -0.1 { theme.danger } else if i == 0 { theme.spectrum_pre } else { theme.curve };
                cx.scene.stroke(&seg, Affine::IDENTITY, color, None, &line);
            }
        }
        for (x, t) in [(WELLS[0], "IN"), (WELLS[1], "OUT")] {
            cx.text.draw(cx.scene, t, x + 10.0, METER_BOTTOM + 4.0, TextStyle::LABEL.sized(9.0), theme.text_lo, Align::Center);
        }
    }

    // ---- help -------------------------------------------------------------------------------

    fn help_text(&self, host: &dyn ParamHost) -> Help {
        if let Some(b) = &self.browser {
            return Help::default().text(b.help());
        }
        if self.drag.as_ref().is_some_and(|d| d.moving) {
            return Help::default().text("Drop it on a + to move it there (into a lane, too)");
        }
        match self.hot {
            Some(Hot::Tile(_)) => return Help::default().text("Click to set it up; drag it to another + to move it"),
            Some(Hot::Led(_)) => return Help::default().text("On and off (off: the signal passes it untouched, in time)"),
            Some(Hot::Gap(Place::Main(_))) => return Help::default().text("Add a block here, or a split (two or three parallel lanes)"),
            Some(Hot::Gap(_)) => return Help::default().text("Add a block to this lane"),
            Some(Hot::Fork(_)) => return Help::default().text("The split: click to set its lanes' levels and pans, or how many"),
            Some(Hot::Menu(_)) => return Help::default().text("Click to add it; Esc closes"),
            Some(Hot::Panel(pb)) => {
                return Help::default().text(match pb {
                    PanelButton::Bypass => "On and off",
                    PanelButton::Open => "Open a file from this computer",
                    PanelButton::Tone3000 => "Find one on TONE3000 and load it here",
                    PanelButton::Clear => "Empty the block",
                    PanelButton::Remove => "Remove it from the board",
                    PanelButton::Size => "Captures that come in two sizes (A2 Full and Lite) play this one, everywhere on the board: Lite costs a fraction of the CPU",
                    PanelButton::Switch { .. } => "Click to switch it (a type: the next one)",
                    PanelButton::Lanes(_) => "How many lanes the split has (the last one's blocks go when it is removed)",
                });
            }
            None => {}
        }
        if let Some(h) = self.footer.help(host) {
            return h;
        }
        if let Some(k) = self.knobs.iter().find(|k| self.pointer.is_some_and(|p| k.bounds().contains(p))) {
            return Help::default().text(format!("{} — drag, wheel, double-click for its default", k.spec.name));
        }
        if let Some(h) = self.session.help() {
            return h;
        }
        Help::default().text("IN → OUT. + adds a block (or a split into parallel lanes); drag a block to move it; its light switches it.")
    }

    // ---- events -----------------------------------------------------------------------------

    fn plain(host: &dyn ParamHost, id: &str) -> f32 {
        host.plain(id, host.normalized(id))
    }

    fn busy(&self) -> bool {
        self.drag.is_some() || self.knobs.iter().any(Knob::is_dragging) || self.footer.busy()
    }

    fn hot_at(&self, p: Point) -> Option<Hot> {
        if let Some(m) = &self.menu {
            return (0..m.entries().len()).find(|&i| m.row(i).contains(p)).map(Hot::Menu);
        }
        if AREA.contains(p) {
            let layout = self.layout();
            for t in &layout.tiles {
                if self.led_of(t).is_some_and(|l| l.distance(p) <= 9.0) {
                    return Some(Hot::Led(t.id));
                }
            }
            if let Some(t) = layout.tile_at(p) {
                return Some(Hot::Tile(t.id));
            }
            if let Some(f) = layout.fork_at(p) {
                return Some(Hot::Fork(f.id));
            }
            if let Some(g) = layout.gap_at(p, 13.0) {
                return Some(Hot::Gap(g.place));
            }
            return None;
        }
        self.panel_buttons().into_iter().find(|b| panel_button(*b).contains(p)).map(Hot::Panel)
    }

    fn click_panel(&mut self, pb: PanelButton, host: &dyn ParamHost) {
        let board = self.ui.board.clone();
        match (pb, self.selected) {
            (PanelButton::Remove, Some(Selection::Block(id) | Selection::Split(id))) => {
                board.remove(id);
                self.selected = None;
            }
            (PanelButton::Lanes(n), Some(Selection::Split(id))) => {
                board.set_lanes(id, n);
                self.knobs_for = None;
            }
            (PanelButton::Size, _) => {
                let v = if Self::plain(host, SIZE.id) >= 0.5 { 0.0 } else { 1.0 };
                host.begin_gesture(SIZE.id);
                host.set_normalized(SIZE.id, host.normalize(SIZE.id, v));
                host.end_gesture(SIZE.id);
                board.poke();
            }
            (_, Some(Selection::Block(id))) => {
                let Some(b) = self.block(id) else { return };
                match pb {
                    PanelButton::Bypass => {
                        b.live.bypass.store(!b.live.bypassed(), Ordering::Relaxed);
                        board.touch();
                    }
                    PanelButton::Switch { param, .. } => {
                        let p = b.kind().params()[param];
                        let v = b.live.value(param);
                        // A switch flips; a choice steps to the next (round).
                        let next = if p.choices.is_empty() { if v >= 0.5 { 0.0 } else { 1.0 } } else { ((v.round() as usize + 1) % p.choices.len()) as f32 };
                        b.live.set(param, next);
                        board.touch();
                    }
                    PanelButton::Clear => board.set_file(id, SlotFile::default()),
                    PanelButton::Tone3000 => self.browser = Some(Browser::new(id, b.kind(), &self.ui)),
                    PanelButton::Open if self.picking.is_none() => {
                        {
                            let rx = if b.kind() == Kind::Cab {
                                mite_ui::dialog::pick_file("Open an impulse response", &[("Impulse responses", crate::ir::EXTENSIONS)])
                            } else {
                                mite_ui::dialog::pick_file("Open a NAM capture", &[("NAM captures", &["nam"])])
                            };
                            self.picking = Some((id, rx));
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }

    /// The drop target for a dragged block, adjusted for the block leaving its own place.
    fn drop_place(&self, id: u64, gap: &Gap) -> Place {
        let from = self.ui.board.place_of(id);
        match (from, gap.place) {
            (Some(Place::Main(a)), Place::Main(b)) if b > a => Place::Main(b - 1),
            (Some(Place::Lane { split: s, lane: l, index: a }), Place::Lane { split, lane, index }) if s == split && l == lane && index > a => Place::Lane { split, lane, index: index - 1 },
            // Leaving the main row shifts the splits after it one to the left.
            (Some(Place::Main(a)), Place::Lane { split, lane, index }) if split > a => Place::Lane { split: split - 1, lane, index },
            (_, p) => p,
        }
    }

    fn handle_event(&mut self, event: &UiEvent, host: &dyn ParamHost) -> bool {
        // TONE3000 open: it has every event over the display (the header and footer still work).
        if let Some(b) = &mut self.browser {
            let over = match *event {
                UiEvent::PointerMove { pos, .. } | UiEvent::PointerDown { pos, .. } | UiEvent::PointerUp { pos, .. } | UiEvent::Wheel { pos, .. } | UiEvent::DoubleClick { pos, .. } => pos.y > DISPLAY_TOP && pos.y < HELP_TOP,
                _ => true,
            };
            if over {
                let (redraw, close) = b.event(event, &self.ui);
                if close {
                    self.browser = None;
                    self.ui.tone3000.close_tone();
                }
                return redraw || close;
            }
        }
        if let UiEvent::KeyDown { key, mods } = *event {
            if key == Key::Escape && self.menu.is_some() {
                self.menu = None;
                return true;
            }
            if matches!(key, Key::Delete | Key::Backspace) && self.menu.is_none() {
                if let Some(Selection::Block(id) | Selection::Split(id)) = self.selected {
                    self.ui.board.remove(id);
                    self.selected = None;
                    return true;
                }
            }
            return self.session.key(key, mods, host).unwrap_or(false);
        }
        if self.menu.is_none() {
            if let Some(answer) = self.session.event(event, host) {
                return answer;
            }
            if self.footer.event(event, host) {
                return true;
            }
            // The panel's knobs, against the block's or lanes' own values.
            match self.selected {
                Some(Selection::Block(id)) => {
                    if let Some(b) = self.block(id) {
                        let bh = BlockHost(&b.live);
                        for k in &mut self.knobs {
                            if k.event(event, &bh) {
                                return true;
                            }
                        }
                    }
                }
                Some(Selection::Split(id)) => {
                    if let Some(lanes) = self.split_lanes(id) {
                        for k in &mut self.knobs {
                            let j: usize = k.id.split(':').next().and_then(|s| s.parse().ok()).unwrap_or(0);
                            if let Some(lane) = lanes.get(j) {
                                if k.event(event, &LaneHost(lane)) {
                                    return true;
                                }
                            }
                        }
                    }
                }
                None => {}
            }
        }
        match *event {
            UiEvent::PointerMove { pos, .. } => {
                self.pointer = Some(pos);
                if let Some(d) = &mut self.drag {
                    d.at = pos;
                    if !d.moving && d.at.distance(d.from) > 6.0 {
                        d.moving = true;
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
            UiEvent::PointerDown { pos, button: MouseButton::Left, .. } => {
                if let Some(m) = &self.menu {
                    let entries = m.entries();
                    let place = m.place;
                    let hit = (0..entries.len()).find(|&i| m.row(i).contains(pos));
                    self.menu = None;
                    if let Some(i) = hit {
                        let (kind, lanes, _, _) = entries[i];
                        match (kind, place) {
                            (Some(k), _) => {
                                if let Some(id) = self.ui.board.insert(k, place) {
                                    self.selected = Some(Selection::Block(id));
                                }
                            }
                            (None, Place::Main(at)) => {
                                let id = self.ui.board.insert_split(at, lanes);
                                self.selected = Some(Selection::Split(id));
                            }
                            _ => {}
                        }
                        self.read_board();
                    }
                    return true;
                }
                match self.hot_at(pos) {
                    Some(Hot::Led(id)) => {
                        if let Some(b) = self.block(id) {
                            b.live.bypass.store(!b.live.bypassed(), Ordering::Relaxed);
                            self.ui.board.touch();
                        }
                        true
                    }
                    Some(Hot::Tile(id)) => {
                        self.selected = Some(Selection::Block(id));
                        self.drag = Some(Drag { id, from: pos, at: pos, moving: false });
                        true
                    }
                    Some(Hot::Fork(id)) => {
                        self.selected = Some(Selection::Split(id));
                        true
                    }
                    Some(Hot::Gap(place)) => {
                        let at = self.layout().gaps.iter().find(|g| g.place == place).map_or(pos, |g| g.at);
                        self.menu = Some(Menu { place, at });
                        true
                    }
                    Some(Hot::Panel(pb)) => {
                        self.click_panel(pb, host);
                        true
                    }
                    _ => false,
                }
            }
            UiEvent::PointerUp { pos, button: MouseButton::Left, .. } => {
                if let Some(d) = self.drag.take() {
                    if d.moving {
                        let layout = self.layout();
                        if let Some(g) = layout.gap_at(pos, 60.0) {
                            let place = self.drop_place(d.id, g);
                            self.ui.board.move_block(d.id, place);
                            self.read_board();
                        }
                    }
                    return true;
                }
                false
            }
            UiEvent::Wheel { pos, lines, .. } => {
                if AREA.contains(pos) {
                    let width = self.layout().width;
                    let max = (width - VIEW_W).max(0.0);
                    self.scroll = (self.scroll - f64::from(lines) * 60.0).clamp(0.0, max);
                    self.hot = self.hot_at(pos);
                    return true;
                }
                false
            }
            _ => false,
        }
    }
}

/// The selected block's buttons: bypass, remove (top right); file (under the name); size and
/// switches (under the file buttons); a split's lane counts.
fn panel_button(b: PanelButton) -> Rect {
    let row = PANEL.y0 + 150.0;
    let at = |x: f64, w: f64, y: f64| Rect::new(x, y, x + w, y + BUTTON);
    match b {
        PanelButton::Bypass => at(PANEL.x1 - 2.0 * (BUTTON + 10.0) - 6.0, BUTTON, PANEL.y0 + 14.0),
        PanelButton::Remove => at(PANEL.x1 - (BUTTON + 10.0) - 6.0, BUTTON, PANEL.y0 + 14.0),
        PanelButton::Open => at(PANEL.x0 + 22.0, BUTTON, row),
        PanelButton::Tone3000 => at(PANEL.x0 + 22.0 + BUTTON + 10.0, 104.0, row),
        PanelButton::Clear => at(PANEL.x0 + 22.0 + BUTTON + 124.0, BUTTON, row),
        PanelButton::Size => at(PANEL.x0 + 22.0, 104.0, row + BUTTON + 12.0),
        PanelButton::Switch { at: k, .. } => at(PANEL.x0 + 22.0 + (k % 4) as f64 * 114.0, 104.0, row + if k >= 4 { BUTTON + 12.0 } else { 0.0 }),
        PanelButton::Lanes(n) => at(PANEL.x0 + 22.0 + (n as f64 - 2.0) * 96.0, 86.0, row),
    }
}

impl View for RigView {
    fn logical_size(&self) -> (f64, f64) {
        (VIEW_W, VIEW_H)
    }

    fn draw(&mut self, cx: &mut DrawCx<'_>) {
        let theme = cx.theme;
        self.draw_header(cx);
        cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.display, None, &Rect::new(0.0, DISPLAY_TOP, VIEW_W, HELP_TOP));
        self.draw_board(cx);
        self.draw_panel(cx);
        self.draw_meters(cx);
        self.draw_menu(cx);
        if let Some(b) = &mut self.browser {
            b.draw(cx, &self.ui);
        }
        self.help_text(cx.host).draw(cx, Rect::new(0.0, HELP_TOP, VIEW_W, FOOTER_TOP));
        self.footer.draw_frame(cx);
        let fs = f64::from(self.ui.sample_rate.load()).max(1.0);
        self.footer.draw_latency(cx, 24.0, f64::from(self.ui.latency.load()) / fs * 1000.0);
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
        self.rebuild_knobs();
        changed
    }

    fn tick(&mut self, host: &dyn ParamHost) -> bool {
        let busy = self.busy();
        self.session.follow(host, busy);
        let dt = self.last_tick.elapsed().as_secs_f64().min(0.1);
        self.last_tick = Instant::now();
        let mut fresh = self.read_board();
        self.rebuild_knobs();
        if let Some((id, rx)) = &self.picking {
            match rx.try_recv() {
                Ok(answer) => {
                    if let Some(path) = answer {
                        self.ui.board.set_file(*id, SlotFile { path: Some(path), source: None });
                    }
                    self.picking = None;
                    fresh = true;
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => self.picking = None,
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
            }
        }
        if let Some(b) = &mut self.browser {
            fresh |= b.tick(&self.ui);
        }
        let (pin, pout) = self.ui.engine.take_peaks();
        let fall_db = (20.0 * dt) as f32;
        for (m, t) in self.meters.iter_mut().zip([pin, pout]) {
            *m = if t > *m { t } else { (*m - fall_db).max(t).max(SILENT) };
        }
        let loading = self.items.iter().any(|i| match i {
            Item::Block(b) => matches!(b.live.status(), Status::Loading),
            Item::Split(s) => s.lanes.iter().any(|l| l.blocks.iter().any(|b| matches!(b.live.status(), Status::Loading))),
        });
        let _ = self.started;
        fresh || busy || loading || self.picking.is_some() || self.menu.is_some() || self.meters.iter().any(|&m| m > SILENT + 1.0)
    }
}

