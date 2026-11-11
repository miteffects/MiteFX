//! The Mite Amp editor (docs/design/amp-brief.md §4), in 1280×760 design units.
//!
//! Top: the rig, three cards in signal order (Pedal → Amp → Cab), each with its switch, what is
//! loaded (name, generation, size, rate; who made it, from where) and its three ways in: a file,
//! TONE3000, or nothing. Middle: what the tone stack and the cabinet do, as heard: the cabinet's
//! measured response (dim), the tone curve (accent) with its grips (bass, middle, treble drag up
//! and down; the cuts left and right), and the two together (lit). Under it the input's switches
//! (gate, normalise, size, calibrate) and dials; IN and OUT meters on the right.
//!
//! TONE3000 opens over the display: sign in (a code to enter on the site), the user's lists,
//! a tone's files, a click puts one in the slot.

use std::path::PathBuf;
use std::sync::mpsc::Receiver;
use std::sync::Arc;
use std::time::Instant;

use mite_core::{ParamDisplay, ParamScale, ParamSpec};
use mite_dsp::eq;
use mite_ui::kurbo::{Affine, BezPath, Cap, Circle, Join, Line, Point, Rect, RoundedRect, Stroke};
use mite_ui::paint::{glow_stroke, icon_button, panel};
use mite_ui::peniko::{Color, Fill};
use mite_ui::session::{Session, SessionConfig};
use mite_ui::widgets::brand::{draw_mark, draw_title};
use mite_ui::widgets::history::smooth_path;
use mite_ui::widgets::{Footer, FooterDial, FooterToggle, Help, Knob, KnobKind, ToggleGlyph};
use mite_ui::{write_if_changed, Align, DrawCx, MouseButton, ParamHost, RecordingHost, TextStyle, UiEvent, View};

use crate::dsp::{tone_bands, AmpSettings};
use crate::ir::{response_freq, RESPONSE_POINTS};
use crate::params::*;
use crate::presets::{preset_ids, FACTORY, PLUGIN};
use crate::rig::{Slot, SlotFile, Status};
use crate::AmpUi;

/// Mite Amp wears its accent (tokens.json `color.accent.amp`, tube glow).
pub const ACCENT: &str = "amp";

mod browser;
use browser::Browser;
pub use browser::PANEL;

const VIEW_W: f64 = 1280.0;
const VIEW_H: f64 = 760.0;
const HEADER: f64 = 56.0;
const DISPLAY_TOP: f64 = HEADER;
const HELP_TOP: f64 = DISPLAY_TOP + 620.0;
const FOOTER_TOP: f64 = HELP_TOP + 28.0;

/// The rig's cards.
const CARD_TOP: f64 = DISPLAY_TOP + 14.0;
const CARD_H: f64 = 162.0;
const CARD_W: f64 = 372.0;
const CARD_X: [f64; 3] = [24.0, 416.0, 808.0];
const BUTTON: f64 = 34.0;

/// The response plot: 20 Hz…20 kHz across, dB up.
const PLOT: Rect = Rect::new(0.0, CARD_TOP + CARD_H + 14.0, 1180.0, DISPLAY_TOP + 466.0);
const PLOT_X0: f64 = 44.0;
const PLOT_W: f64 = 1112.0;
const TOP_DB: f64 = 18.0;
const BOTTOM_DB: f64 = -30.0;
const GRIP: (f64, f64) = (26.0, 20.0);

/// The strip under the plot.
const STRIP_TOP: f64 = PLOT.y1 + 8.0;
const TOGGLE_Y: f64 = STRIP_TOP + 34.0;
const TOGGLE_S: f64 = 60.0;
const TOGGLE_X: [f64; 4] = [24.0, 96.0, 168.0, 240.0];
/// The strip's dials: INPUT and GATE, the selected slot's own, INTERFACE.
const DIAL_GLOBAL_X: [f64; 2] = [372.0, 462.0];
const DIAL_SLOT_X0: f64 = 590.0;
const DIAL_SLOT_STEP: f64 = 94.0;
const DIAL_INTERFACE_X: f64 = 1112.0;
const DIAL_Y: f64 = STRIP_TOP + 68.0;
const DIAL_D: f64 = 52.0;

const WELLS: [f64; 2] = [1206.0, 1238.0];
const METER_TOP: f64 = PLOT.y0 + 4.0;
const METER_BOTTOM: f64 = DISPLAY_TOP + 592.0;
const METER_RANGE: f64 = 60.0;
const SILENT: f32 = -150.0;

const POWER_GLYPH: &str = "M12 3.5 V11 M7.2 6.6 A7.4 7.4 0 1 0 16.8 6.6";
const FOLDER_GLYPH: &str = "M3.5 7 L3.5 18 L20.5 18 L20.5 9 L11.5 9 L9.5 7 Z";
const CLEAR_GLYPH: &str = "M7.5 7.5 L16.5 16.5 M16.5 7.5 L7.5 16.5";
const GATE_GLYPH: &str = "M2.5 14 L6 14 L6 7 L18 7 L18 14 L21.5 14 M9 10.5 L15 10.5";
const NORMALIZE_GLYPH: &str = "M4 18 L4 9 M9 18 L9 5 M14 18 L14 12 M19 18 L19 8 M2.5 10 L21.5 10";
const CALIBRATE_GLYPH: &str = "M4 17 A9 9 0 0 1 20 17 M12 17 L16 10 M8 6.5 L8.6 7.6 M12 5 L12 6.4 M16 6.5 L15.4 7.6";

pub fn freq_x(f: f64) -> f64 {
    PLOT_X0 + PLOT_W * (f / 20.0).ln() / 1000f64.ln()
}

fn x_freq(x: f64) -> f64 {
    20.0 * 1000f64.powf(((x - PLOT_X0) / PLOT_W).clamp(0.0, 1.0))
}

fn db_y(db: f64) -> f64 {
    PLOT.y0 + 18.0 + (TOP_DB - db.clamp(BOTTOM_DB, TOP_DB)) / (TOP_DB - BOTTOM_DB) * (PLOT.height() - 36.0)
}

fn px_per_db() -> f64 {
    (PLOT.height() - 36.0) / (TOP_DB - BOTTOM_DB)
}

fn draw_glyph(scene: &mut mite_ui::vello::Scene, d: &str, c: Point, size: f64, color: Color, width: f64) {
    let Ok(mut path) = BezPath::from_svg(d) else { return };
    let k = size / 24.0;
    path.apply_affine(Affine::translate((c.x - 12.0 * k, c.y - 12.0 * k)) * Affine::scale(k));
    scene.stroke(&Stroke::new(width).with_caps(Cap::Round).with_join(Join::Round), Affine::IDENTITY, color, None, &path);
}

/// A stand-in host holding every Mite Amp parameter at its default (tests, preview, tools).
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
pub fn settings(host: &dyn ParamHost) -> AmpSettings {
    amp_settings(|id| host.plain(id, host.normalized(id)))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Grip {
    Bass,
    Middle,
    Treble,
    LowCut,
    HighCut,
}

impl Grip {
    const ALL: [Grip; 5] = [Grip::LowCut, Grip::Bass, Grip::Middle, Grip::Treble, Grip::HighCut];

    /// What it moves: the bands are the selected slot's (the pedal's own, or the amp's).
    fn spec(self, selected: Slot) -> ParamSpec {
        let pedal = selected == Slot::Pedal;
        match self {
            Grip::Bass if pedal => PEDAL_BASS,
            Grip::Middle if pedal => PEDAL_MIDDLE,
            Grip::Treble if pedal => PEDAL_TREBLE,
            Grip::Bass => BASS,
            Grip::Middle => MIDDLE,
            Grip::Treble => TREBLE,
            Grip::LowCut => LOW_CUT,
            Grip::HighCut => HIGH_CUT,
        }
    }

    fn is_cut(self) -> bool {
        matches!(self, Grip::LowCut | Grip::HighCut)
    }
}

/// The strip's switches.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Toggle {
    Gate,
    Normalize,
    Size,
    Calibrate,
}

impl Toggle {
    const ALL: [Toggle; 4] = [Toggle::Gate, Toggle::Normalize, Toggle::Size, Toggle::Calibrate];

    fn rect(self) -> Rect {
        let i = Self::ALL.iter().position(|t| *t == self).unwrap_or(0);
        Rect::new(TOGGLE_X[i], TOGGLE_Y, TOGGLE_X[i] + TOGGLE_S, TOGGLE_Y + TOGGLE_S)
    }

    fn id(self) -> &'static str {
        match self {
            Toggle::Gate => GATE.id,
            Toggle::Normalize => NORMALIZE.id,
            Toggle::Size => SIZE.id,
            Toggle::Calibrate => CALIBRATE.id,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Toggle::Gate => "GATE",
            Toggle::Normalize => "NORMALIZE",
            Toggle::Size => "SIZE",
            Toggle::Calibrate => "CALIBRATE",
        }
    }
}

/// A card's controls.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CardButton {
    Power,
    Open,
    Tone3000,
    Clear,
    /// The Cab card, while the Amp is a full rig: stack the IR on it, or let it rest.
    Stack,
}

fn card_rect(slot: Slot) -> Rect {
    let x = CARD_X[slot.index()];
    Rect::new(x, CARD_TOP, x + CARD_W, CARD_TOP + CARD_H)
}

fn card_button(slot: Slot, b: CardButton) -> Rect {
    let r = card_rect(slot);
    match b {
        CardButton::Power => Rect::new(r.x1 - 14.0 - BUTTON, r.y0 + 12.0, r.x1 - 14.0, r.y0 + 12.0 + BUTTON),
        CardButton::Stack => Rect::new(r.x1 - 14.0 - 112.0, r.y1 - 14.0 - BUTTON, r.x1 - 14.0, r.y1 - 14.0),
        _ => {
            let i = match b {
                CardButton::Open => 0.0,
                CardButton::Tone3000 => 1.0,
                _ => 2.0,
            };
            let x = r.x0 + 16.0 + i * (BUTTON + 10.0);
            let w = if b == CardButton::Tone3000 { BUTTON + 26.0 } else { BUTTON };
            let x = if i == 2.0 { x + 26.0 } else { x };
            Rect::new(x, r.y1 - 14.0 - BUTTON, x + w, r.y1 - 14.0)
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Hot {
    Card(Slot, CardButton),
    /// A card's body: selects it.
    Select(Slot),
    Grip(Grip),
    Toggle(Toggle),
}

pub struct AmpView {
    ui: Arc<AmpUi>,
    session: Session,
    footer: Footer,
    knobs: Vec<Knob>,
    /// Which slot each knob belongs to (none: always shown).
    knob_slot: Vec<Option<Slot>>,
    /// The slot whose controls the strip and the curve show.
    selected: Slot,
    last_tick: Instant,
    started: Instant,
    pointer: Option<Point>,
    hot: Option<Hot>,
    /// A grip drag: which, where it was grabbed, its value then.
    drag: Option<(Grip, Point, f32)>,
    meters: [f32; 2],
    picking: Option<(Slot, Receiver<Option<PathBuf>>)>,
    browser: Option<Browser>,
    seen: (u64, u64),
    files: [SlotFile; 3],
    status: [Status; 3],
}

fn knob_words(id: &str) -> &'static str {
    match id {
        "input" => " — into the captures: how hard you hit the pedal or amp",
        "gate_threshold" => " — below this the gate closes (on the captures' output, so their hiss goes too)",
        "bass" => " — a shelf at 150 Hz (or drag its grip on the curve)",
        "middle" => " — a bell at 425 Hz",
        "treble" => " — a shelf at 1.8 kHz",
        "low_cut" => " — takes the boom out under it (all the way down: off)",
        "high_cut" => " — takes the fizz off over it (all the way up: off)",
        "interface_level" => " — the level your interface's full scale stands for (its manual says, in dBu); with CALIBRATE, captures that know their input level are hit as hard as the real amp was",
        _ => "",
    }
}

impl AmpView {
    pub fn new(ui: Arc<AmpUi>) -> Self {
        let footer = Footer::new(
            FOOTER_TOP,
            (VIEW_W, VIEW_H),
            vec![FooterToggle { id: BYPASS.id, glyph: ToggleGlyph::Bypass, words: "Bypass — the input as it came (in time: the latency stays)" }],
            vec![FooterDial { spec: MIX, kind: KnobKind::Unipolar, name: "MIX", words: "Mix — the rig against the dry input (in time)" }, FooterDial { spec: OUT_GAIN, kind: KnobKind::Bipolar, name: "OUTPUT", words: "Output gain" }],
        );
        let mut knobs = Vec::new();
        let mut knob_slot = Vec::new();
        let mut add = |spec: ParamSpec, label: &'static str, kind: KnobKind, x: f64, slot: Option<Slot>| {
            knobs.push(Knob::new(spec, label, kind, Point::new(x, DIAL_Y), DIAL_D));
            knob_slot.push(slot);
        };
        add(INPUT, "INPUT", KnobKind::Bipolar, DIAL_GLOBAL_X[0], None);
        add(GATE_THRESHOLD, "GATE", KnobKind::Unipolar, DIAL_GLOBAL_X[1], None);
        add(INTERFACE_LEVEL, "INTERFACE", KnobKind::Unipolar, DIAL_INTERFACE_X, None);
        type Dials = Vec<(ParamSpec, &'static str, KnobKind)>;
        let sets: [(Slot, Dials); 3] = [
            (Slot::Pedal, vec![(PEDAL_GAIN, "GAIN", KnobKind::Bipolar), (PEDAL_LEVEL, "LEVEL", KnobKind::Bipolar), (PEDAL_BASS, "BASS", KnobKind::Bipolar), (PEDAL_MIDDLE, "MIDDLE", KnobKind::Bipolar), (PEDAL_TREBLE, "TREBLE", KnobKind::Bipolar)]),
            (Slot::Amp, vec![(AMP_GAIN, "GAIN", KnobKind::Bipolar), (AMP_LEVEL, "LEVEL", KnobKind::Bipolar), (BASS, "BASS", KnobKind::Bipolar), (MIDDLE, "MIDDLE", KnobKind::Bipolar), (TREBLE, "TREBLE", KnobKind::Bipolar)]),
            (Slot::Cab, vec![(CAB_LEVEL, "LEVEL", KnobKind::Bipolar), (LOW_CUT, "LOW CUT", KnobKind::Unipolar), (HIGH_CUT, "HIGH CUT", KnobKind::Unipolar)]),
        ];
        for (slot, dials) in sets {
            for (i, (spec, label, kind)) in dials.into_iter().enumerate() {
                add(spec, label, kind, DIAL_SLOT_X0 + i as f64 * DIAL_SLOT_STEP, Some(slot));
            }
        }
        let files = ui.rig.files();
        let status = Slot::ALL.map(|s| ui.rig.status(s));
        Self {
            session: Session::new(SessionConfig { plugin: PLUGIN, factory: FACTORY, sound_ids: preset_ids(), snapshot_ids: preset_ids() }, ui.snapshots.clone(), ui.preset.clone()),
            footer,
            knobs,
            knob_slot,
            selected: Slot::Amp,
            last_tick: Instant::now(),
            started: Instant::now(),
            pointer: None,
            hot: None,
            drag: None,
            meters: [SILENT, SILENT],
            picking: None,
            browser: None,
            seen: (u64::MAX, u64::MAX),
            files,
            status,
            ui,
        }
    }

    /// Keeps the user's presets in `dir` instead of the settings folder (tests).
    pub fn with_preset_dir(mut self, dir: PathBuf) -> Self {
        self.session = self.session.with_preset_dir(dir);
        self
    }

    // ---- for tests and tools ----------------------------------------------------------------

    pub fn button(&self, slot: Slot, b: CardButton) -> Rect {
        card_button(slot, b)
    }

    pub fn toggle(&self, t: Toggle) -> Rect {
        t.rect()
    }

    pub fn dial(&self, id: &str) -> Option<Point> {
        self.knobs.iter().zip(&self.knob_slot).find(|(k, s)| k.id == id && s.is_none_or(|s| s == self.selected)).map(|(k, _)| k.center)
    }

    pub fn grip_at(&self, host: &dyn ParamHost, g: Grip) -> Point {
        self.grip(host, g)
    }

    pub fn browser_open(&self) -> Option<Slot> {
        self.browser.as_ref().map(|b| b.slot)
    }

    pub fn open_browser(&mut self, slot: Slot) {
        self.browser = Some(Browser::new(slot, &self.ui));
    }

    /// Reads the rig and TONE3000 now (tools render a single frame).
    pub fn settle(&mut self) {
        self.read_shared();
    }

    // ---- reading ----------------------------------------------------------------------------

    fn plain(host: &dyn ParamHost, id: &str) -> f32 {
        host.plain(id, host.normalized(id))
    }

    fn set_raw(host: &dyn ParamHost, id: &str, v: f32) {
        host.set_normalized(id, host.normalize(id, v));
    }

    fn gesture(host: &dyn ParamHost, id: &str, v: f32) {
        host.begin_gesture(id);
        Self::set_raw(host, id, v);
        host.end_gesture(id);
    }

    fn on(host: &dyn ParamHost, id: &str) -> bool {
        Self::plain(host, id) >= 0.5
    }

    fn read_shared(&mut self) -> bool {
        let v = (self.ui.rig.status_version(), 0);
        if v == self.seen {
            return false;
        }
        self.seen = v;
        self.files = self.ui.rig.files();
        self.status = Slot::ALL.map(|s| self.ui.rig.status(s));
        true
    }

    fn fs(&self) -> f64 {
        f64::from(self.ui.sample_rate.load()).max(1.0)
    }

    /// Bands' response at each of `freqs`, dB (each band designed once).
    fn curve(&self, bands: &[eq::BandSettings], freqs: &[f64]) -> Vec<f64> {
        let fs = self.fs();
        let designs: Vec<eq::Sections> = bands.iter().filter(|b| b.enabled).map(|b| eq::design(b, fs)).collect();
        freqs.iter().map(|f| designs.iter().map(|d| d.response_db(std::f64::consts::TAU * f / fs)).sum()).collect()
    }

    /// The selected slot's tone: the pedal's three bands, or the amp's with the cuts.
    fn tone_curve(&self, host: &dyn ParamHost, freqs: &[f64]) -> Vec<f64> {
        let s = settings(host);
        if self.selected == Slot::Pedal {
            self.curve(&crate::dsp::pedal_bands(&AmpSettings { pedal: true, ..s }), freqs)
        } else {
            self.curve(&tone_bands(&s), freqs)
        }
    }

    fn tone_db(&self, host: &dyn ParamHost, f: f64) -> f64 {
        self.tone_curve(host, &[f])[0]
    }

    /// The grips the selected slot has: the pedal its bands, the amp its bands and the cuts, the
    /// cabinet the cuts.
    fn grips(&self) -> &'static [Grip] {
        match self.selected {
            Slot::Pedal => &[Grip::Bass, Grip::Middle, Grip::Treble],
            Slot::Amp => &Grip::ALL,
            Slot::Cab => &[Grip::LowCut, Grip::HighCut],
        }
    }

    /// Whether the amp slot plays a full rig (its own cabinet: the Cab steps aside).
    fn amp_is_full_rig(&self) -> bool {
        let file = &self.files[Slot::Amp.index()];
        match &self.status[Slot::Amp.index()] {
            Status::Model(info) => match file.source.as_ref().filter(|s| !s.gear.is_empty()) {
                Some(src) => crate::rig::Gear::of_tone3000(&src.gear, "nam") == crate::rig::Gear::FullRig,
                None => crate::rig::Gear::of_nam(info.gear_type.as_deref()) == crate::rig::Gear::FullRig,
            },
            _ => false,
        }
    }

    fn cab_db(&self) -> Option<&[f32]> {
        match &self.status[Slot::Cab.index()] {
            Status::Cab(info) if info.response_db.len() == RESPONSE_POINTS => Some(&info.response_db),
            _ => None,
        }
    }

    fn grip(&self, host: &dyn ParamHost, g: Grip) -> Point {
        let v = f64::from(Self::plain(host, g.spec(self.selected).id));
        let f = match g {
            Grip::Bass => crate::dsp::BASS_HZ,
            Grip::Middle => crate::dsp::MIDDLE_HZ,
            Grip::Treble => crate::dsp::TREBLE_HZ,
            Grip::LowCut | Grip::HighCut => v,
        };
        // A cut sits on the curve's −3 dB point at its corner; a band on its own gain.
        let db = if g.is_cut() { self.tone_db(host, f) } else { v };
        Point::new(freq_x(f), db_y(db))
    }

    // ---- drawing ----------------------------------------------------------------------------

    fn draw_header(&mut self, cx: &mut DrawCx<'_>) {
        let theme = cx.theme;
        cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.window, None, &Rect::new(0.0, 0.0, VIEW_W, HEADER));
        cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, theme.line, None, &Line::new((0.0, HEADER - 0.5), (VIEW_W, HEADER - 0.5)));
        draw_mark(cx, 16.0, 15.0, 26.0);
        draw_title(cx, 52.0, 18.0, "Amp");
        self.session.draw(cx);
    }

    /// Cuts `text` to `width` (with an ellipsis).
    fn fit(cx: &mut DrawCx<'_>, text: &str, style: TextStyle, width: f64) -> String {
        if f64::from(cx.text.measure(text, style)) <= width {
            return text.to_owned();
        }
        let mut s: String = text.to_owned();
        while !s.is_empty() {
            s.pop();
            let t = format!("{}…", s.trim_end());
            if f64::from(cx.text.measure(&t, style)) <= width {
                return t;
            }
        }
        "…".to_owned()
    }

    fn slot_words(&self, slot: Slot) -> (String, String, String) {
        let file = &self.files[slot.index()];
        let stem = file.path.as_ref().and_then(|p| p.file_stem()).map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        let source = file.source.as_ref().map(|s| format!("TONE3000 · {} · {}", s.creator, if s.license.is_empty() { "license on the site".to_owned() } else { s.license.to_uppercase() }));
        match &self.status[slot.index()] {
            Status::Empty => (
                "Empty".to_owned(),
                if slot == Slot::Cab { "No cabinet: the amp's own sound".to_owned() } else { "Nothing here: the signal passes".to_owned() },
                if slot == Slot::Cab { "Open an IR (WAV, AIFF, FLAC) or find one on TONE3000".to_owned() } else { "Open a .nam capture or find one on TONE3000".to_owned() },
            ),
            Status::Loading => (stem, "Loading…".to_owned(), String::new()),
            Status::Failed(e) => (stem, "Could not load it".to_owned(), e.clone()),
            Status::Model(info) => {
                let title = source.as_ref().and(file.source.as_ref().map(|s| s.title.clone())).filter(|t| !t.is_empty()).or_else(|| info.name.clone()).unwrap_or(stem);
                let generation = if info.generation == mite_nam::Generation::A2 { "A2" } else { "A1" };
                let mut parts = vec![format!("{generation} {}", info.architecture)];
                if info.sizes.len() > 1 {
                    parts.push(if info.size_index + 1 == info.sizes.len() { "Full".to_owned() } else { "Lite".to_owned() });
                }
                parts.push(format!("{} kHz", info.sample_rate.map_or("48".to_owned(), |r| format!("{}", r / 1000.0))));
                if info.loudness.is_none() {
                    parts.push("no loudness".to_owned());
                }
                if info.input_level_dbu.is_some() {
                    parts.push("calibrated".to_owned());
                }
                let gear = [info.gear_make.clone(), info.gear_model.clone()].into_iter().flatten().collect::<Vec<_>>().join(" ");
                let who = source.unwrap_or_else(|| match (gear.is_empty(), &info.modeled_by) {
                    (false, Some(by)) => format!("{gear} · by {by}"),
                    (false, None) => gear,
                    (true, Some(by)) => format!("by {by}"),
                    (true, None) => String::new(),
                });
                (title, parts.join(" · "), who)
            }
            Status::Cab(info) => {
                let title = file.source.as_ref().map(|s| s.title.clone()).filter(|t| !t.is_empty()).unwrap_or(stem);
                let mut parts = vec![format!("IR · {:.0} ms · {} kHz", info.seconds * 1000.0, info.file_rate / 1000.0)];
                if info.channels > 1 {
                    parts.push("stereo summed".to_owned());
                }
                if info.truncated {
                    parts.push("cut to 500 ms".to_owned());
                }
                (title, parts.join(" · "), source.unwrap_or_default())
            }
        }
    }

    fn draw_cards(&self, cx: &mut DrawCx<'_>) {
        let theme = cx.theme;
        for slot in Slot::ALL {
            let r = card_rect(slot);
            let on = Self::on(cx.host, [PEDAL.id, AMP.id, CAB.id][slot.index()]);
            let loaded = matches!(self.status[slot.index()], Status::Model(_) | Status::Cab(_));
            let full_rig = self.amp_is_full_rig();
            // The Cab under a full rig rests (unless stacked): drawn as resting, not as playing.
            let stacked = Self::on(cx.host, CAB_ON_RIG.id);
            let resting = slot == Slot::Cab && full_rig && !stacked;
            let lit = on && loaded && !resting;
            let selected = slot == self.selected;
            let shape = RoundedRect::from_rect(r, 12.0);
            panel(cx.scene, shape, theme.panel_control, if lit { theme.accent.multiply_alpha(0.55) } else { theme.panel_control_border });
            if lit {
                glow_stroke(cx, &shape, 1.0, theme.accent.multiply_alpha(0.5), theme.accent.multiply_alpha(0.35), 6.0);
            }
            if selected {
                cx.scene.stroke(&Stroke::new(2.0), Affine::IDENTITY, theme.accent, None, &RoundedRect::from_rect(r.inflate(3.0, 3.0), 14.0));
            } else if self.hot == Some(Hot::Select(slot)) {
                cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, theme.accent.multiply_alpha(0.5), None, &RoundedRect::from_rect(r.inflate(3.0, 3.0), 14.0));
            }
            let label = TextStyle::LABEL.sized(10.0);
            cx.text.draw(cx.scene, &slot.name().to_uppercase(), r.x0 + 18.0, r.y0 + 16.0, label, if lit { theme.accent } else { theme.text_lo }, Align::Left);
            let (title, mut line2, mut line3) = self.slot_words(slot);
            if slot == Slot::Cab && full_rig {
                (line2, line3) = if stacked {
                    ("Stacked on the full rig's own cabinet".to_owned(), "Two cabinets in a row: darker, narrower".to_owned())
                } else if loaded {
                    ("Resting: the Amp is a full rig with its own cabinet".to_owned(), "That cabinet is part of the capture and can't be swapped".to_owned())
                } else {
                    ("The Amp is a full rig: its own cabinet plays".to_owned(), "For your own IR, load an amp-only capture".to_owned())
                };
            }
            // A full rig says so on its card.
            if slot == Slot::Amp && full_rig {
                let style = TextStyle::LABEL.sized(9.0);
                let text = "FULL RIG · CAB INCLUDED";
                let w = f64::from(cx.text.measure(text, style)) + 14.0;
                let x = r.x0 + 58.0;
                cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.accent.multiply_alpha(0.16), None, &RoundedRect::new(x, r.y0 + 13.0, x + w, r.y0 + 30.0, 8.5));
                cx.text.draw(cx.scene, text, x + 7.0, r.y0 + 16.5, style, theme.accent, Align::Left);
            }
            let big = TextStyle::BODY.sized(19.0).weighted(700.0);
            let width = r.width() - 36.0 - BUTTON - 10.0;
            let title = Self::fit(cx, &title, big, width);
            let ink = if lit { theme.text_strong } else if loaded { theme.text_mid } else { theme.text_lo };
            cx.text.draw(cx.scene, &title, r.x0 + 18.0, r.y0 + 36.0, big, ink, Align::Left);
            let small = TextStyle::BODY.sized(11.5);
            let failed = matches!(self.status[slot.index()], Status::Failed(_));
            let l2 = Self::fit(cx, &line2, small, r.width() - 36.0);
            cx.text.draw(cx.scene, &l2, r.x0 + 18.0, r.y0 + 66.0, small, if failed { theme.danger } else { theme.text_mid }, Align::Left);
            let l3 = Self::fit(cx, &line3, small, r.width() - 36.0);
            cx.text.draw(cx.scene, &l3, r.x0 + 18.0, r.y0 + 84.0, small, if failed { theme.danger.multiply_alpha(0.8) } else { theme.text_lo }, Align::Left);
            if matches!(self.status[slot.index()], Status::Loading) {
                let t = self.started.elapsed().as_secs_f64();
                let x = r.x0 + 18.0 + (r.width() - 36.0) * (0.5 + 0.5 * (t * 2.5).sin());
                cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.accent, None, &Circle::new((x, r.y0 + 106.0), 2.5));
            }
            let mut buttons = vec![CardButton::Power, CardButton::Open, CardButton::Tone3000, CardButton::Clear];
            if slot == Slot::Cab && full_rig && loaded {
                buttons.push(CardButton::Stack);
            }
            for b in buttons {
                let br = card_button(slot, b);
                let hot = self.hot == Some(Hot::Card(slot, b));
                let pressed = (b == CardButton::Power && on) || (b == CardButton::Stack && stacked);
                let ink = icon_button(cx.scene, theme, br, theme.control, hot, pressed, theme.accent);
                match b {
                    CardButton::Power => draw_glyph(cx.scene, POWER_GLYPH, br.center(), 22.0, ink, 1.8),
                    CardButton::Open => draw_glyph(cx.scene, FOLDER_GLYPH, br.center(), 22.0, ink, 1.6),
                    CardButton::Clear => draw_glyph(cx.scene, CLEAR_GLYPH, br.center(), 20.0, if self.files[slot.index()].path.is_some() { ink } else { theme.text_disabled }, 1.6),
                    CardButton::Tone3000 => {
                        cx.text.draw(cx.scene, "T3K", br.center().x, br.center().y - 7.0, TextStyle::BODY.sized(12.5).weighted(800.0), ink, Align::Center);
                    }
                    CardButton::Stack => {
                        let word = if stacked { "Stacked" } else { "Use anyway" };
                        cx.text.draw(cx.scene, word, br.center().x, br.center().y - 7.5, TextStyle::BODY.sized(12.0).weighted(700.0), ink, Align::Center);
                    }
                }
            }
            // The signal flows on to the next card (not into a resting Cab).
            if slot != Slot::Cab && !(slot == Slot::Amp && full_rig && !stacked) {
                let y = r.center().y;
                let x = r.x1 + 10.0;
                let mut a = BezPath::new();
                a.move_to((x - 2.0, y - 6.0));
                a.line_to((x + 4.0, y));
                a.line_to((x - 2.0, y + 6.0));
                cx.scene.stroke(&Stroke::new(1.6).with_caps(Cap::Round).with_join(Join::Round), Affine::IDENTITY, theme.text_lo, None, &a);
            }
        }
    }

    fn draw_plot(&self, cx: &mut DrawCx<'_>) {
        let theme = cx.theme;
        let host = cx.host;
        for f in [100.0, 1000.0, 10_000.0] {
            cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, theme.grid_minor, None, &Line::new((freq_x(f), PLOT.y0), (freq_x(f), PLOT.y1)));
        }
        for f in [50.0, 200.0, 500.0, 2000.0, 5000.0] {
            cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, theme.grid_minor.multiply_alpha(0.5), None, &Line::new((freq_x(f), PLOT.y0), (freq_x(f), PLOT.y1)));
        }
        let label = TextStyle { tabular: true, ..TextStyle::BODY.sized(10.0).weighted(600.0) };
        for (f, s) in [(100.0, "100"), (1000.0, "1k"), (10_000.0, "10k")] {
            cx.text.draw(cx.scene, s, freq_x(f) + 4.0, PLOT.y1 - 16.0, label, theme.text_lo, Align::Left);
        }
        for db in [-24, -12, 0, 12] {
            let y = db_y(f64::from(db));
            let color = if db == 0 { theme.grid_zero } else { theme.grid_minor.multiply_alpha(0.5) };
            cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, color, None, &Line::new((PLOT_X0, y), (PLOT_X0 + PLOT_W, y)));
            let t = if db > 0 { format!("+{db}") } else if db < 0 { format!("−{}", -db) } else { "0".to_owned() };
            cx.text.draw(cx.scene, &t, 8.0, y - 6.0, label, theme.text_lo, Align::Left);
        }
        cx.begin_clip(&Rect::new(PLOT_X0, PLOT.y0, PLOT_X0 + PLOT_W, PLOT.y1));
        let cab_on = Self::on(host, CAB.id);
        let pts = |f: &dyn Fn(f64, usize) -> f64| -> Vec<Point> { (0..RESPONSE_POINTS).map(|i| Point::new(freq_x(response_freq(i)), db_y(f(response_freq(i), i)))).collect() };
        let freqs: Vec<f64> = (0..RESPONSE_POINTS).map(response_freq).collect();
        let tone = self.tone_curve(host, &freqs);
        // What is heard after the amp: its tone and the cuts, whatever slot is shown.
        let post = self.curve(&tone_bands(&settings(host)), &freqs);
        let cab_level = f64::from(Self::plain(host, CAB_LEVEL.id));
        // The cabinet's own response: dim, filled down to the floor.
        if let Some(cab) = self.cab_db() {
            let p = pts(&|_, i| f64::from(cab[i]));
            let curve = smooth_path(&p);
            let mut fill = curve.clone();
            fill.line_to((p[p.len() - 1].x, PLOT.y1));
            fill.line_to((p[0].x, PLOT.y1));
            fill.close_path();
            let resting = self.amp_is_full_rig() && !Self::on(host, CAB_ON_RIG.id);
            let a = if cab_on && !resting { 1.0 } else { 0.4 };
            cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.spectrum_pre.multiply_alpha(0.10 * a), None, &fill);
            cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, theme.spectrum_pre.multiply_alpha(0.5 * a), None, &curve);
        }
        // The tone curve: the accent.
        let hot = matches!(self.hot, Some(Hot::Grip(_))) || self.drag.is_some();
        let tc = smooth_path(&pts(&|_, i| tone[i]));
        cx.scene.stroke(&Stroke::new(if hot { 1.8 } else { 1.4 }), Affine::IDENTITY, theme.accent.multiply_alpha(0.85), None, &tc);
        // What is heard: tone and cabinet together, lit.
        if let (Some(cab), true) = (self.cab_db(), cab_on && (!self.amp_is_full_rig() || Self::on(host, CAB_ON_RIG.id))) {
            let both = smooth_path(&pts(&|_, i| post[i] + f64::from(cab[i]) + cab_level));
            glow_stroke(cx, &both, 1.6, theme.curve.multiply_alpha(0.85), theme.curve.multiply_alpha(0.6), 1.6);
        }
        cx.end_clip();
        for &g in self.grips() {
            let p = self.grip(host, g);
            let hot = self.hot == Some(Hot::Grip(g)) || self.drag.is_some_and(|d| d.0 == g);
            let r = RoundedRect::from_rect(Rect::from_center_size(p, GRIP), 10.0);
            cx.scene.fill(Fill::NonZero, Affine::IDENTITY, if hot { theme.accent } else { theme.display }, None, &r);
            glow_stroke(cx, &r, 1.4, theme.accent, theme.accent, if hot { 4.0 } else { 2.0 });
            let ink = if hot { theme.display } else { theme.accent };
            let mut chev = BezPath::new();
            if g.is_cut() {
                chev.move_to((p.x - 2.0, p.y - 3.5));
                chev.line_to((p.x - 5.5, p.y));
                chev.line_to((p.x - 2.0, p.y + 3.5));
                chev.move_to((p.x + 2.0, p.y - 3.5));
                chev.line_to((p.x + 5.5, p.y));
                chev.line_to((p.x + 2.0, p.y + 3.5));
            } else {
                chev.move_to((p.x - 3.5, p.y - 2.0));
                chev.line_to((p.x, p.y - 5.5));
                chev.line_to((p.x + 3.5, p.y - 2.0));
                chev.move_to((p.x - 3.5, p.y + 2.0));
                chev.line_to((p.x, p.y + 5.5));
                chev.line_to((p.x + 3.5, p.y + 2.0));
            }
            cx.scene.stroke(&Stroke::new(1.4).with_caps(Cap::Round).with_join(Join::Round), Affine::IDENTITY, ink, None, &chev);
        }
    }

    fn draw_strip(&self, cx: &mut DrawCx<'_>) {
        let theme = cx.theme;
        cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, theme.line, None, &Line::new((0.0, PLOT.y1 + 0.5), (PLOT.x1, PLOT.y1 + 0.5)));
        let label = TextStyle::LABEL.sized(9.0);
        for t in Toggle::ALL {
            let r = t.rect();
            let on = match t {
                Toggle::Size => Self::plain(cx.host, SIZE.id) >= 0.5,
                _ => Self::on(cx.host, t.id()),
            };
            let ink = icon_button(cx.scene, theme, r, theme.panel_control, self.hot == Some(Hot::Toggle(t)), on, theme.accent);
            match t {
                Toggle::Gate => draw_glyph(cx.scene, GATE_GLYPH, r.center(), 36.0, ink, 1.8),
                Toggle::Normalize => draw_glyph(cx.scene, NORMALIZE_GLYPH, r.center(), 36.0, ink, 1.8),
                Toggle::Calibrate => draw_glyph(cx.scene, CALIBRATE_GLYPH, r.center(), 36.0, ink, 1.8),
                Toggle::Size => {
                    let word = if on { "LITE" } else { "FULL" };
                    cx.text.draw(cx.scene, word, r.center().x, r.center().y - 8.0, TextStyle::BODY.sized(13.0).weighted(800.0), ink, Align::Center);
                }
            }
            cx.text.draw(cx.scene, t.label(), r.center().x, r.y1 + 8.0, label, theme.text_lo, Align::Center);
        }
        for (k, slot) in self.knobs.iter().zip(&self.knob_slot) {
            if slot.is_none_or(|s| s == self.selected) {
                k.draw(cx);
            }
        }
        // The selected slot's section, named over its dials.
        let x1 = DIAL_SLOT_X0 + if self.selected == Slot::Cab { 2.0 } else { 4.0 } * DIAL_SLOT_STEP;
        let name = format!("{} — click a card to switch", self.selected.name().to_uppercase());
        cx.text.draw(cx.scene, &name, (DIAL_SLOT_X0 + x1) / 2.0, STRIP_TOP + 10.0, TextStyle::LABEL.sized(9.0), theme.accent, Align::Center);
        cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, theme.accent.multiply_alpha(0.35), None, &Line::new((DIAL_SLOT_X0 - 40.0, STRIP_TOP + 26.0), (x1 + 40.0, STRIP_TOP + 26.0)));
    }

    fn draw_meters(&self, cx: &mut DrawCx<'_>) {
        let theme = cx.theme;
        cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, theme.line, None, &Line::new((PLOT.x1 + 0.5, PLOT.y0), (PLOT.x1 + 0.5, HELP_TOP)));
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
        let slot_help = |spec: &ParamSpec| {
            let v = Self::plain(host, spec.id);
            Help::default().text(format!("{} ", spec.name)).slot(spec.format(v), spec.format(spec.max))
        };
        if let Some(b) = &self.browser {
            return Help::default().text(b.help());
        }
        if let Some((g, _, _)) = self.drag {
            return slot_help(&g.spec(self.selected)).text(if g.is_cut() { " — drag left and right. Shift is fine." } else { " — drag up and down. Shift is fine." });
        }
        match self.hot {
            Some(Hot::Grip(g)) => return slot_help(&g.spec(self.selected)).text(if g.is_cut() { " — drag sideways (to the end: off); wheel; double-click: off" } else { " — drag up and down; wheel; double-click: flat" }),
            Some(Hot::Card(slot, b)) => {
                return Help::default().text(match (slot, b) {
                    (Slot::Cab, CardButton::Power) => "Cab on and off (off: the amp's sound without a cabinet, for a real one or another IR loader)",
                    (_, CardButton::Power) => "On and off (off: the signal passes this slot untouched, still in time)",
                    (Slot::Cab, CardButton::Open) => "Open an impulse response (WAV, AIFF or FLAC; stereo is summed; up to 500 ms)",
                    (_, CardButton::Open) => "Open a NAM capture (.nam: A1 or A2, any size)",
                    (_, CardButton::Tone3000) => "Find one on TONE3000",
                    (_, CardButton::Clear) => "Empty the slot",
                    (_, CardButton::Stack) => "A full rig's cabinet is part of its capture and can't be taken out. Use anyway puts this IR after it too (two cabinets: darker); off, the IR rests until the Amp is an amp-only capture",
                });
            }
            Some(Hot::Toggle(t)) => {
                return Help::default().text(match t {
                    Toggle::Gate => "Gate — silence between notes: it listens to the input and closes on the captures' output (its threshold is the GATE dial)",
                    Toggle::Normalize => "Normalize — every capture plays at the same loudness (−18 dB), so swapping amps does not jump in level",
                    Toggle::Size => "Size — captures that come in two sizes (A2 Full and Lite) play this one: Lite costs a fraction of the CPU",
                    Toggle::Calibrate => "Calibrate — captures that know their input level are hit as hard as the real amp was (set INTERFACE to your interface's full-scale level)",
                });
            }
            _ => {}
        }
        if let Some(h) = self.footer.help(host) {
            return h;
        }
        if let Some((k, _)) = self.knobs.iter().zip(&self.knob_slot).find(|(k, s)| s.is_none_or(|s| s == self.selected) && self.pointer.is_some_and(|p| k.bounds().contains(p))) {
            let v = Self::plain(host, &k.id);
            return Help::default().text(format!("{} ", k.spec.name)).slot(k.spec.format(v), k.spec.format(k.spec.max)).text(knob_words(k.spec.id));
        }
        if let Some(h) = self.session.help() {
            return h;
        }
        Help::default().text("Pedal → Amp → Cab. On the curve: the cabinet (dim), the tone (accent, drag its grips), the two together (lit).")
    }

    // ---- events -----------------------------------------------------------------------------

    fn busy(&self) -> bool {
        self.drag.is_some() || self.knobs.iter().any(Knob::is_dragging) || self.footer.busy()
    }

    fn hot_at(&self, host: &dyn ParamHost, p: Point) -> Option<Hot> {
        let stack = self.amp_is_full_rig() && matches!(self.status[Slot::Cab.index()], Status::Cab(_));
        for slot in Slot::ALL {
            let extra: &[CardButton] = if slot == Slot::Cab && stack { &[CardButton::Stack] } else { &[] };
            for &b in [CardButton::Power, CardButton::Open, CardButton::Tone3000, CardButton::Clear].iter().chain(extra) {
                if card_button(slot, b).contains(p) {
                    return Some(Hot::Card(slot, b));
                }
            }
        }
        for &g in self.grips() {
            if Rect::from_center_size(self.grip(host, g), GRIP).inflate(4.0, 4.0).contains(p) {
                return Some(Hot::Grip(g));
            }
        }
        if let Some(slot) = Slot::ALL.into_iter().find(|s| card_rect(*s).contains(p)) {
            return Some(Hot::Select(slot));
        }
        Toggle::ALL.into_iter().find(|t| t.rect().contains(p)).map(Hot::Toggle)
    }

    fn click_card(&mut self, slot: Slot, b: CardButton, host: &dyn ParamHost) {
        match b {
            CardButton::Power => {
                let id = [PEDAL.id, AMP.id, CAB.id][slot.index()];
                Self::gesture(host, id, if Self::on(host, id) { 0.0 } else { 1.0 });
            }
            CardButton::Open => {
                if self.picking.is_none() {
                    let rx = if slot == Slot::Cab {
                        mite_ui::dialog::pick_file("Open an impulse response", &[("Impulse responses", crate::ir::EXTENSIONS)])
                    } else {
                        mite_ui::dialog::pick_file("Open a NAM capture", &[("NAM captures", &["nam"])])
                    };
                    self.picking = Some((slot, rx));
                }
            }
            CardButton::Tone3000 => self.open_browser(slot),
            CardButton::Clear => self.ui.rig.set_file(slot, SlotFile::default()),
            CardButton::Stack => Self::gesture(host, CAB_ON_RIG.id, if Self::on(host, CAB_ON_RIG.id) { 0.0 } else { 1.0 }),
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
            return self.session.key(key, mods, host).unwrap_or(false);
        }
        if let Some(answer) = self.session.event(event, host) {
            return answer;
        }
        if self.footer.event(event, host) {
            return true;
        }
        if self.browser.is_none() {
            for (k, slot) in self.knobs.iter_mut().zip(&self.knob_slot) {
                if slot.is_none_or(|s| s == self.selected) && k.event(event, host) {
                    return true;
                }
            }
        }
        match *event {
            UiEvent::PointerMove { pos, mods } => {
                self.pointer = Some(pos);
                if let Some((g, start, v0)) = self.drag {
                    let spec = g.spec(self.selected);
                    let k = if mods.fine() { 0.2 } else { 1.0 };
                    let v = if g.is_cut() {
                        let f0 = f64::from(v0);
                        let x = freq_x(f0) + (pos.x - start.x) * k;
                        x_freq(x) as f32
                    } else {
                        v0 - ((pos.y - start.y) / px_per_db()) as f32 * k as f32
                    };
                    Self::set_raw(host, spec.id, spec.snap(v.clamp(spec.min, spec.max)));
                    return true;
                }
                self.hot = self.hot_at(host, pos);
                true
            }
            UiEvent::PointerLeft => {
                self.pointer = None;
                self.hot = None;
                true
            }
            UiEvent::PointerDown { pos, button: MouseButton::Left, .. } => match self.hot_at(host, pos) {
                Some(Hot::Card(slot, b)) => {
                    self.selected = slot;
                    self.click_card(slot, b, host);
                    true
                }
                Some(Hot::Select(slot)) => {
                    self.selected = slot;
                    true
                }
                Some(Hot::Grip(g)) => {
                    let id = g.spec(self.selected).id;
                    host.begin_gesture(id);
                    self.drag = Some((g, pos, Self::plain(host, id)));
                    true
                }
                Some(Hot::Toggle(t)) => {
                    let id = t.id();
                    Self::gesture(host, id, if Self::plain(host, id) >= 0.5 { 0.0 } else { 1.0 });
                    true
                }
                None => false,
            },
            UiEvent::PointerUp { button: MouseButton::Left, .. } => {
                if let Some((g, _, _)) = self.drag.take() {
                    host.end_gesture(g.spec(self.selected).id);
                    return true;
                }
                false
            }
            UiEvent::DoubleClick { pos, .. } => match self.hot_at(host, pos) {
                Some(Hot::Grip(g)) => {
                    self.drag = None;
                    let spec = g.spec(self.selected);
                    Self::gesture(host, spec.id, spec.default);
                    true
                }
                _ => false,
            },
            UiEvent::Wheel { pos, lines, mods } => {
                match self.hot_at(host, pos) {
                    Some(Hot::Grip(g)) => {
                        let spec = g.spec(self.selected);
                        let v = spec.wheel(Self::plain(host, spec.id), lines, mods.fine());
                        write_if_changed(host, spec.id, host.normalize(spec.id, v));
                        true
                    }
                    _ => false,
                }
            }
            _ => false,
        }
    }
}

impl View for AmpView {
    fn logical_size(&self) -> (f64, f64) {
        (VIEW_W, VIEW_H)
    }

    fn draw(&mut self, cx: &mut DrawCx<'_>) {
        let theme = cx.theme;
        self.draw_header(cx);
        cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.display, None, &Rect::new(0.0, DISPLAY_TOP, VIEW_W, HELP_TOP));
        self.draw_cards(cx);
        self.draw_plot(cx);
        self.draw_strip(cx);
        self.draw_meters(cx);
        if let Some(b) = &mut self.browser {
            b.draw(cx, &self.ui);
        }
        self.help_text(cx.host).draw(cx, Rect::new(0.0, HELP_TOP, VIEW_W, FOOTER_TOP));
        self.footer.draw_frame(cx);
        let fs = self.fs();
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
        changed
    }

    fn tick(&mut self, host: &dyn ParamHost) -> bool {
        let busy = self.busy();
        self.session.follow(host, busy);
        let dt = self.last_tick.elapsed().as_secs_f64().min(0.1);
        self.last_tick = Instant::now();
        let mut fresh = self.read_shared();
        // A file picked in the dialog.
        if let Some((slot, rx)) = &self.picking {
            match rx.try_recv() {
                Ok(answer) => {
                    if let Some(path) = answer {
                        self.ui.rig.set_file(*slot, SlotFile { path: Some(path), source: None });
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
        let loading = self.status.iter().any(|s| matches!(s, Status::Loading));
        fresh || busy || loading || self.picking.is_some() || self.meters.iter().any(|&m| m > SILENT + 1.0)
    }
}
