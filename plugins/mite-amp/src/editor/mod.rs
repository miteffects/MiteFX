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
use mite_ui::paint::{glass_panel, glow_stroke, icon_button, panel};
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
use crate::tone3000::{self, List, Phase};
use crate::AmpUi;

/// Mite Amp wears its accent (tokens.json `color.accent.amp`, tube glow).
pub const ACCENT: &str = "amp";

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
const DIAL_X0: f64 = 380.0;
const DIAL_STEP: f64 = 104.0;
const DIAL_Y: f64 = STRIP_TOP + 68.0;
const DIAL_D: f64 = 52.0;

const WELLS: [f64; 2] = [1206.0, 1238.0];
const METER_TOP: f64 = PLOT.y0 + 4.0;
const METER_BOTTOM: f64 = DISPLAY_TOP + 592.0;
const METER_RANGE: f64 = 60.0;
const SILENT: f32 = -150.0;

/// The TONE3000 panel.
const PANEL: Rect = Rect::new(16.0, DISPLAY_TOP + 8.0, VIEW_W - 16.0, HELP_TOP - 8.0);
const ROW_H: f64 = 46.0;
const BODY_TOP: f64 = PANEL.y0 + 104.0;

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

    fn spec(self) -> ParamSpec {
        match self {
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
}

fn card_rect(slot: Slot) -> Rect {
    let x = CARD_X[slot.index()];
    Rect::new(x, CARD_TOP, x + CARD_W, CARD_TOP + CARD_H)
}

fn card_button(slot: Slot, b: CardButton) -> Rect {
    let r = card_rect(slot);
    match b {
        CardButton::Power => Rect::new(r.x1 - 14.0 - BUTTON, r.y0 + 12.0, r.x1 - 14.0, r.y0 + 12.0 + BUTTON),
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

/// Places in the TONE3000 panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PanelHot {
    Close,
    Site,
    SignIn,
    SignOut,
    OpenCode,
    Tab(List),
    Back,
    Row(usize),
}

struct Browser {
    slot: Slot,
    scroll: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Hot {
    Card(Slot, CardButton),
    Grip(Grip),
    Toggle(Toggle),
    Panel(PanelHot),
}

pub struct AmpView {
    ui: Arc<AmpUi>,
    session: Session,
    footer: Footer,
    knobs: Vec<Knob>,
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
    t3k: tone3000::View,
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
        let dials: [(ParamSpec, &str, KnobKind); 8] = [
            (INPUT, "INPUT", KnobKind::Bipolar),
            (GATE_THRESHOLD, "GATE", KnobKind::Unipolar),
            (BASS, "BASS", KnobKind::Bipolar),
            (MIDDLE, "MIDDLE", KnobKind::Bipolar),
            (TREBLE, "TREBLE", KnobKind::Bipolar),
            (LOW_CUT, "LOW CUT", KnobKind::Unipolar),
            (HIGH_CUT, "HIGH CUT", KnobKind::Unipolar),
            (INTERFACE_LEVEL, "INTERFACE", KnobKind::Unipolar),
        ];
        let knobs = dials.into_iter().enumerate().map(|(i, (spec, label, kind))| Knob::new(spec, label, kind, Point::new(DIAL_X0 + i as f64 * DIAL_STEP, DIAL_Y), DIAL_D)).collect();
        let files = ui.rig.files();
        let status = Slot::ALL.map(|s| ui.rig.status(s));
        let t3k = ui.tone3000.view();
        Self {
            session: Session::new(SessionConfig { plugin: PLUGIN, factory: FACTORY, sound_ids: preset_ids(), snapshot_ids: preset_ids() }, ui.snapshots.clone(), ui.preset.clone()),
            footer,
            knobs,
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
            t3k,
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
        self.knobs.iter().find(|k| k.id == id).map(|k| k.center)
    }

    pub fn grip_at(&self, host: &dyn ParamHost, g: Grip) -> Point {
        self.grip(host, g)
    }

    pub fn browser_open(&self) -> Option<Slot> {
        self.browser.as_ref().map(|b| b.slot)
    }

    pub fn open_browser(&mut self, slot: Slot) {
        self.browser = Some(Browser { slot, scroll: 0.0 });
        let v = self.ui.tone3000.view();
        if v.phase == Phase::SignedIn && v.list.is_none() {
            self.ui.tone3000.open_list(List::Favorites);
        }
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
        let v = (self.ui.rig.status_version(), self.ui.tone3000.version());
        if v == self.seen {
            return false;
        }
        self.seen = v;
        self.files = self.ui.rig.files();
        self.status = Slot::ALL.map(|s| self.ui.rig.status(s));
        self.t3k = self.ui.tone3000.view();
        true
    }

    fn fs(&self) -> f64 {
        f64::from(self.ui.sample_rate.load()).max(1.0)
    }

    /// The tone stack's response at each of `freqs`, dB (each band designed once).
    fn tone_curve(&self, host: &dyn ParamHost, freqs: &[f64]) -> Vec<f64> {
        let fs = self.fs();
        let designs: Vec<eq::Sections> = tone_bands(&settings(host)).iter().filter(|b| b.enabled).map(|b| eq::design(b, fs)).collect();
        freqs.iter().map(|f| designs.iter().map(|d| d.response_db(std::f64::consts::TAU * f / fs)).sum()).collect()
    }

    fn tone_db(&self, host: &dyn ParamHost, f: f64) -> f64 {
        self.tone_curve(host, &[f])[0]
    }

    fn cab_db(&self) -> Option<&[f32]> {
        match &self.status[Slot::Cab.index()] {
            Status::Cab(info) if info.response_db.len() == RESPONSE_POINTS => Some(&info.response_db),
            _ => None,
        }
    }

    fn grip(&self, host: &dyn ParamHost, g: Grip) -> Point {
        let v = f64::from(Self::plain(host, g.spec().id));
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
            let lit = on && loaded;
            let shape = RoundedRect::from_rect(r, 12.0);
            panel(cx.scene, shape, theme.panel_control, if lit { theme.accent.multiply_alpha(0.55) } else { theme.panel_control_border });
            if lit {
                glow_stroke(cx, &shape, 1.0, theme.accent.multiply_alpha(0.5), theme.accent.multiply_alpha(0.35), 6.0);
            }
            let label = TextStyle::LABEL.sized(10.0);
            cx.text.draw(cx.scene, &slot.name().to_uppercase(), r.x0 + 18.0, r.y0 + 16.0, label, if lit { theme.accent } else { theme.text_lo }, Align::Left);
            let (title, line2, line3) = self.slot_words(slot);
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
            for b in [CardButton::Power, CardButton::Open, CardButton::Tone3000, CardButton::Clear] {
                let br = card_button(slot, b);
                let hot = self.hot == Some(Hot::Card(slot, b));
                let pressed = b == CardButton::Power && on;
                let ink = icon_button(cx.scene, theme, br, theme.control, hot, pressed, theme.accent);
                match b {
                    CardButton::Power => draw_glyph(cx.scene, POWER_GLYPH, br.center(), 22.0, ink, 1.8),
                    CardButton::Open => draw_glyph(cx.scene, FOLDER_GLYPH, br.center(), 22.0, ink, 1.6),
                    CardButton::Clear => draw_glyph(cx.scene, CLEAR_GLYPH, br.center(), 20.0, if self.files[slot.index()].path.is_some() { ink } else { theme.text_disabled }, 1.6),
                    CardButton::Tone3000 => {
                        cx.text.draw(cx.scene, "T3K", br.center().x, br.center().y - 7.0, TextStyle::BODY.sized(12.5).weighted(800.0), ink, Align::Center);
                    }
                }
            }
            // The signal flows on to the next card.
            if slot != Slot::Cab {
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
        // The cabinet's own response: dim, filled down to the floor.
        if let Some(cab) = self.cab_db() {
            let p = pts(&|_, i| f64::from(cab[i]));
            let curve = smooth_path(&p);
            let mut fill = curve.clone();
            fill.line_to((p[p.len() - 1].x, PLOT.y1));
            fill.line_to((p[0].x, PLOT.y1));
            fill.close_path();
            let a = if cab_on { 1.0 } else { 0.4 };
            cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.spectrum_pre.multiply_alpha(0.10 * a), None, &fill);
            cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, theme.spectrum_pre.multiply_alpha(0.5 * a), None, &curve);
        }
        // The tone curve: the accent.
        let hot = matches!(self.hot, Some(Hot::Grip(_))) || self.drag.is_some();
        let tc = smooth_path(&pts(&|_, i| tone[i]));
        cx.scene.stroke(&Stroke::new(if hot { 1.8 } else { 1.4 }), Affine::IDENTITY, theme.accent.multiply_alpha(0.85), None, &tc);
        // What is heard: tone and cabinet together, lit.
        if let (Some(cab), true) = (self.cab_db(), cab_on) {
            let both = smooth_path(&pts(&|_, i| tone[i] + f64::from(cab[i])));
            glow_stroke(cx, &both, 1.6, theme.curve.multiply_alpha(0.85), theme.curve.multiply_alpha(0.6), 1.6);
        }
        cx.end_clip();
        for g in Grip::ALL {
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
        for k in &self.knobs {
            k.draw(cx);
        }
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

    // ---- TONE3000 ---------------------------------------------------------------------------

    fn tab_rect(list: List) -> Rect {
        let i = List::ALL.iter().position(|l| *l == list).unwrap_or(0) as f64;
        Rect::new(PANEL.x0 + 24.0 + i * 128.0, PANEL.y0 + 58.0, PANEL.x0 + 24.0 + i * 128.0 + 118.0, PANEL.y0 + 88.0)
    }

    fn panel_button(h: PanelHot) -> Rect {
        let top = PANEL.y0 + 14.0;
        match h {
            PanelHot::Close => Rect::new(PANEL.x1 - 46.0, top, PANEL.x1 - 14.0, top + 32.0),
            PanelHot::Site => Rect::new(PANEL.x1 - 230.0, top, PANEL.x1 - 58.0, top + 32.0),
            PanelHot::SignOut => Rect::new(PANEL.x1 - 336.0, top, PANEL.x1 - 242.0, top + 32.0),
            PanelHot::SignIn | PanelHot::OpenCode => Rect::new(PANEL.center().x - 110.0, BODY_TOP + 150.0, PANEL.center().x + 110.0, BODY_TOP + 192.0),
            PanelHot::Back => Rect::new(PANEL.x0 + 24.0, BODY_TOP - 2.0, PANEL.x0 + 104.0, BODY_TOP + 28.0),
            PanelHot::Tab(l) => Self::tab_rect(l),
            PanelHot::Row(i) => Rect::new(PANEL.x0 + 16.0, 0.0, PANEL.x1 - 16.0, 0.0 + i as f64),
        }
    }

    /// The rows shown: tones of the list, or the open tone's files that fit the slot.
    fn rows(&self, slot: Slot) -> Vec<(String, String, String)> {
        let v = &self.t3k;
        if v.tone.is_some() {
            return self.fitting_models(slot).iter().map(|m| (m.name.clone(), m.kind.clone(), m.size.clone())).collect();
        }
        let Some(list) = v.list else { return Vec::new() };
        let Some((_, lv)) = v.lists.iter().find(|(l, _)| *l == list) else { return Vec::new() };
        lv.tones
            .iter()
            .map(|t| {
                let counts = if t.format == "ir" { format!("{} IR", t.irs.max(t.models)) } else if t.a2_models > 0 { format!("{} A2 · {} files", t.a2_models, t.models) } else { format!("{} files", t.models) };
                let mut sub = vec![format!("by {}", t.creator)];
                if !t.gear.is_empty() {
                    sub.push(t.gear.clone());
                }
                sub.push(t.format.to_uppercase());
                if !t.license.is_empty() {
                    sub.push(t.license.to_uppercase());
                }
                (t.title.clone(), sub.join(" · "), counts)
            })
            .collect()
    }

    fn fitting_models(&self, slot: Slot) -> Vec<tone3000::Model> {
        let models = self.t3k.models.clone().unwrap_or_default();
        models.into_iter().filter(|m| (m.kind == "IR") == (slot == Slot::Cab)).collect()
    }

    fn row_top(&self, i: usize) -> f64 {
        let first = if self.t3k.tone.is_some() { BODY_TOP + 40.0 } else { BODY_TOP };
        first + i as f64 * ROW_H - self.browser.as_ref().map_or(0.0, |b| b.scroll)
    }

    fn draw_browser(&self, cx: &mut DrawCx<'_>) {
        let Some(b) = &self.browser else { return };
        let theme = cx.theme;
        let shape = RoundedRect::from_rect(PANEL, 14.0);
        glass_panel(cx, shape, theme.blur_panel);
        cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.display.multiply_alpha(0.9), None, &shape);
        cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, theme.glass_border, None, &shape);
        let v = &self.t3k;
        // The header: TONE3000's name (its terms ask for it), where this goes, the site, close.
        cx.text.draw(cx.scene, "TONE3000", PANEL.x0 + 24.0, PANEL.y0 + 18.0, TextStyle::BODY.sized(20.0).weighted(800.0), theme.text_strong, Align::Left);
        let to = format!("Powered by TONE3000 · into the {}", b.slot.name());
        cx.text.draw(cx.scene, &to, PANEL.x0 + 146.0, PANEL.y0 + 24.0, TextStyle::BODY.sized(12.0), theme.text_mid, Align::Left);
        let mut buttons = vec![(PanelHot::Close, "×"), (PanelHot::Site, "Browse TONE3000")];
        if v.phase == Phase::SignedIn {
            buttons.push((PanelHot::SignOut, "Sign out"));
        }
        for (h, t) in buttons {
            let r = Self::panel_button(h);
            let ink = icon_button(cx.scene, theme, r, theme.control, self.hot == Some(Hot::Panel(h)), false, theme.accent);
            let size = if h == PanelHot::Close { 18.0 } else { 12.0 };
            cx.text.draw(cx.scene, t, r.center().x, r.center().y - size * 0.6, TextStyle::BODY.sized(size as f32).weighted(700.0), ink, Align::Center);
        }
        if let Some(user) = &v.user {
            cx.text.draw(cx.scene, user, Self::panel_button(PanelHot::SignOut).x0 - 12.0, PANEL.y0 + 24.0, TextStyle::BODY.sized(12.0).weighted(700.0), theme.text_hi, Align::Right);
        }
        let body = TextStyle::BODY.sized(13.0);
        let center = PANEL.center().x;
        match &v.phase {
            Phase::NoKey => {
                let lines = [
                    "TONE3000 asks every app for its own key.".to_owned(),
                    "Make a publishable key in your TONE3000 settings (Settings → API), then save it as".to_owned(),
                    format!("{}/tone3000.json  as  {{\"client_id\": \"your key\"}}", tone3000::settings_dir().map_or("the Mite Amp settings folder".to_owned(), |d| d.display().to_string())),
                    "and open the editor again.".to_owned(),
                ];
                for (i, l) in lines.iter().enumerate() {
                    cx.text.draw(cx.scene, l, center, BODY_TOP + 30.0 + i as f64 * 24.0, body, if i == 2 { theme.text_strong } else { theme.text_mid }, Align::Center);
                }
                return;
            }
            Phase::SignedOut | Phase::Failed(_) | Phase::Starting => {
                let msg = match &v.phase {
                    Phase::Failed(e) => e.clone(),
                    Phase::Starting => "Asking TONE3000 for a code…".to_owned(),
                    _ => "Sign in to reach your favourites, downloads and tones.".to_owned(),
                };
                cx.text.draw(cx.scene, &msg, center, BODY_TOP + 80.0, body, if matches!(v.phase, Phase::Failed(_)) { theme.danger } else { theme.text_mid }, Align::Center);
                if v.phase != Phase::Starting {
                    let r = Self::panel_button(PanelHot::SignIn);
                    let ink = icon_button(cx.scene, theme, r, theme.control, self.hot == Some(Hot::Panel(PanelHot::SignIn)), true, theme.accent);
                    cx.text.draw(cx.scene, "Sign in to TONE3000", r.center().x, r.center().y - 8.0, TextStyle::BODY.sized(13.5).weighted(700.0), ink, Align::Center);
                }
                return;
            }
            Phase::Code { code, .. } => {
                cx.text.draw(cx.scene, "Enter this code at tone3000.com/activate (the page opened in your browser):", center, BODY_TOP + 30.0, body, theme.text_mid, Align::Center);
                cx.text.draw(cx.scene, code, center, BODY_TOP + 66.0, TextStyle { tabular: true, ..TextStyle::BODY.sized(40.0).weighted(800.0) }, theme.accent, Align::Center);
                let r = Self::panel_button(PanelHot::OpenCode);
                let ink = icon_button(cx.scene, theme, r, theme.control, self.hot == Some(Hot::Panel(PanelHot::OpenCode)), false, theme.accent);
                cx.text.draw(cx.scene, "Open the page again", r.center().x, r.center().y - 8.0, TextStyle::BODY.sized(13.0).weighted(700.0), ink, Align::Center);
                return;
            }
            Phase::SignedIn => {}
        }
        for l in List::ALL {
            let r = Self::tab_rect(l);
            let on = v.list == Some(l);
            let ink = icon_button(cx.scene, theme, r, theme.control, self.hot == Some(Hot::Panel(PanelHot::Tab(l))), on, theme.accent);
            cx.text.draw(cx.scene, l.name(), r.center().x, r.center().y - 7.5, TextStyle::BODY.sized(12.0).weighted(700.0), ink, Align::Center);
        }
        let clip = Rect::new(PANEL.x0, BODY_TOP - 6.0, PANEL.x1, PANEL.y1 - 10.0);
        if let Some(tone) = &v.tone {
            let r = Self::panel_button(PanelHot::Back);
            let ink = icon_button(cx.scene, theme, r, theme.control, self.hot == Some(Hot::Panel(PanelHot::Back)), false, theme.accent);
            cx.text.draw(cx.scene, "‹ Back", r.center().x, r.center().y - 7.5, TextStyle::BODY.sized(12.0).weighted(700.0), ink, Align::Center);
            let head = format!("{} — by {}{}", tone.title, tone.creator, if tone.license.is_empty() { String::new() } else { format!(" · {}", tone.license.to_uppercase()) });
            let head = Self::fit(cx, &head, TextStyle::BODY.sized(14.0).weighted(700.0), PANEL.width() - 160.0);
            cx.text.draw(cx.scene, &head, PANEL.x0 + 120.0, BODY_TOP + 4.0, TextStyle::BODY.sized(14.0).weighted(700.0), theme.text_strong, Align::Left);
        }
        let rows = self.rows(b.slot);
        let loading = match (&v.tone, v.list) {
            (Some(_), _) => v.models.is_none(),
            (None, Some(list)) => v.lists.iter().find(|(l, _)| *l == list).is_some_and(|(_, lv)| lv.loading),
            _ => false,
        };
        let error = match (&v.tone, v.list) {
            (Some(_), _) => v.models_error.clone().or_else(|| v.download_error.clone()),
            (None, Some(list)) => v.lists.iter().find(|(l, _)| *l == list).and_then(|(_, lv)| lv.error.clone()),
            _ => None,
        };
        let empty_word = if v.tone.is_some() {
            if b.slot == Slot::Cab { "This tone has no IRs for the Cab." } else { "This tone has no captures for this slot." }
        } else {
            "Nothing here yet."
        };
        if let Some(e) = &error {
            cx.text.draw(cx.scene, e, center, PANEL.y1 - 40.0, body, theme.danger, Align::Center);
        }
        if loading {
            cx.text.draw(cx.scene, "Loading…", center, BODY_TOP + 60.0, body, theme.text_mid, Align::Center);
            return;
        }
        if rows.is_empty() {
            cx.text.draw(cx.scene, empty_word, center, BODY_TOP + 60.0, body, theme.text_lo, Align::Center);
            return;
        }
        cx.begin_clip(&clip);
        let models = self.fitting_models(b.slot);
        for (i, (title, sub, right)) in rows.iter().enumerate() {
            let y = self.row_top(i);
            if y + ROW_H < clip.y0 || y > clip.y1 {
                continue;
            }
            let r = Rect::new(PANEL.x0 + 16.0, y, PANEL.x1 - 16.0, y + ROW_H - 4.0);
            let hot = self.hot == Some(Hot::Panel(PanelHot::Row(i)));
            let downloading = v.tone.is_some() && models.get(i).is_some_and(|m| v.downloading == Some(m.id));
            if hot || downloading {
                cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.accent.multiply_alpha(if downloading { 0.18 } else { 0.10 }), None, &RoundedRect::from_rect(r, 8.0));
            }
            let t = Self::fit(cx, title, TextStyle::BODY.sized(13.5).weighted(700.0), r.width() * 0.62);
            cx.text.draw(cx.scene, &t, r.x0 + 14.0, r.y0 + 6.0, TextStyle::BODY.sized(13.5).weighted(700.0), theme.text_strong, Align::Left);
            let s = Self::fit(cx, sub, TextStyle::BODY.sized(11.0), r.width() * 0.62);
            cx.text.draw(cx.scene, &s, r.x0 + 14.0, r.y0 + 24.0, TextStyle::BODY.sized(11.0), theme.text_lo, Align::Left);
            let right = if downloading { "Downloading…".to_owned() } else { right.clone() };
            cx.text.draw(cx.scene, &right, r.x1 - 14.0, r.y0 + 13.0, TextStyle::BODY.sized(12.0).weighted(600.0), if downloading { theme.accent } else { theme.text_mid }, Align::Right);
        }
        cx.end_clip();
    }

    fn panel_hot(&self, p: Point) -> Option<PanelHot> {
        let b = self.browser.as_ref()?;
        let v = &self.t3k;
        let mut fixed = vec![PanelHot::Close, PanelHot::Site];
        match v.phase {
            Phase::SignedIn => {
                fixed.push(PanelHot::SignOut);
                fixed.extend(List::ALL.map(PanelHot::Tab));
                if v.tone.is_some() {
                    fixed.push(PanelHot::Back);
                }
            }
            Phase::SignedOut | Phase::Failed(_) => fixed.push(PanelHot::SignIn),
            Phase::Code { .. } => fixed.push(PanelHot::OpenCode),
            _ => {}
        }
        if let Some(h) = fixed.into_iter().find(|h| Self::panel_button(*h).contains(p)) {
            return Some(h);
        }
        if v.phase == Phase::SignedIn && p.y >= BODY_TOP - 6.0 && p.y <= PANEL.y1 - 10.0 && p.x > PANEL.x0 + 16.0 && p.x < PANEL.x1 - 16.0 {
            let n = self.rows(b.slot).len();
            for i in 0..n {
                let y = self.row_top(i);
                if p.y >= y && p.y < y + ROW_H - 4.0 {
                    return Some(PanelHot::Row(i));
                }
            }
        }
        None
    }

    fn panel_click(&mut self, h: PanelHot) {
        let Some(slot) = self.browser.as_ref().map(|b| b.slot) else { return };
        let t3k = self.ui.tone3000.clone();
        match h {
            PanelHot::Close => self.browser = None,
            PanelHot::Site => tone3000::open_in_browser(&format!("{}/search", tone3000::SITE)),
            PanelHot::SignIn => t3k.sign_in(),
            PanelHot::SignOut => t3k.sign_out(),
            PanelHot::OpenCode => {
                if let Phase::Code { uri, .. } = &self.t3k.phase {
                    tone3000::open_in_browser(uri);
                }
            }
            PanelHot::Tab(l) => {
                if let Some(b) = &mut self.browser {
                    b.scroll = 0.0;
                }
                t3k.open_list(l);
            }
            PanelHot::Back => {
                if let Some(b) = &mut self.browser {
                    b.scroll = 0.0;
                }
                t3k.close_tone();
            }
            PanelHot::Row(i) => {
                if self.t3k.tone.is_some() {
                    let models = self.fitting_models(slot);
                    if let (Some(tone), Some(m)) = (self.t3k.tone.clone(), models.get(i).cloned()) {
                        if self.t3k.downloading.is_none() {
                            t3k.download(tone, m, slot, self.ui.rig.clone());
                        }
                    }
                } else if let Some(list) = self.t3k.list {
                    if let Some(tone) = self.t3k.lists.iter().find(|(l, _)| *l == list).and_then(|(_, lv)| lv.tones.get(i).cloned()) {
                        if let Some(b) = &mut self.browser {
                            b.scroll = 0.0;
                        }
                        t3k.open_tone(tone);
                    }
                }
            }
        }
    }

    // ---- help -------------------------------------------------------------------------------

    fn help_text(&self, host: &dyn ParamHost) -> Help {
        let slot_help = |spec: &ParamSpec| {
            let v = Self::plain(host, spec.id);
            Help::default().text(format!("{} ", spec.name)).slot(spec.format(v), spec.format(spec.max))
        };
        if self.browser.is_some() {
            return Help::default().text(match self.hot {
                Some(Hot::Panel(PanelHot::Row(_))) if self.t3k.tone.is_some() => "Click to download it into the slot (it stays in the settings folder's TONE3000 folder, with its creator and license)",
                Some(Hot::Panel(PanelHot::Row(_))) => "Click to see the tone's files",
                Some(Hot::Panel(PanelHot::Site)) => "Opens TONE3000 in your browser: favourite a tone there and it shows here under Favorites",
                Some(Hot::Panel(PanelHot::SignOut)) => "Signs out (the session is forgotten on this computer)",
                _ => "TONE3000: your favourites, downloads and tones, and what is trending. Esc or × closes.",
            });
        }
        if let Some((g, _, _)) = self.drag {
            return slot_help(&g.spec()).text(if g.is_cut() { " — drag left and right. Shift is fine." } else { " — drag up and down. Shift is fine." });
        }
        match self.hot {
            Some(Hot::Grip(g)) => return slot_help(&g.spec()).text(if g.is_cut() { " — drag sideways (to the end: off); wheel; double-click: off" } else { " — drag up and down; wheel; double-click: flat" }),
            Some(Hot::Card(slot, b)) => {
                return Help::default().text(match (slot, b) {
                    (Slot::Cab, CardButton::Power) => "Cab on and off (off: the amp's sound without a cabinet, for a real one or another IR loader)",
                    (_, CardButton::Power) => "On and off (off: the signal passes this slot untouched, still in time)",
                    (Slot::Cab, CardButton::Open) => "Open an impulse response (WAV, AIFF or FLAC; stereo is summed; up to 500 ms)",
                    (_, CardButton::Open) => "Open a NAM capture (.nam: A1 or A2, any size)",
                    (_, CardButton::Tone3000) => "Find one on TONE3000",
                    (_, CardButton::Clear) => "Empty the slot",
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
        if let Some(k) = self.knobs.iter().find(|k| self.pointer.is_some_and(|p| k.bounds().contains(p))) {
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
        if self.browser.is_some() {
            return self.panel_hot(p).map(Hot::Panel);
        }
        for slot in Slot::ALL {
            for b in [CardButton::Power, CardButton::Open, CardButton::Tone3000, CardButton::Clear] {
                if card_button(slot, b).contains(p) {
                    return Some(Hot::Card(slot, b));
                }
            }
        }
        for g in Grip::ALL {
            if Rect::from_center_size(self.grip(host, g), GRIP).inflate(4.0, 4.0).contains(p) {
                return Some(Hot::Grip(g));
            }
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
        }
    }

    fn handle_event(&mut self, event: &UiEvent, host: &dyn ParamHost) -> bool {
        if let UiEvent::KeyDown { key, mods } = *event {
            if self.browser.is_some() && key == mite_ui::Key::Escape {
                self.browser = None;
                return true;
            }
            return self.session.key(key, mods, host).unwrap_or(false);
        }
        if self.browser.is_none() {
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
        }
        match *event {
            UiEvent::PointerMove { pos, mods } => {
                self.pointer = Some(pos);
                if let Some((g, start, v0)) = self.drag {
                    let spec = g.spec();
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
                Some(Hot::Panel(h)) => {
                    self.panel_click(h);
                    true
                }
                Some(Hot::Card(slot, b)) => {
                    self.click_card(slot, b, host);
                    true
                }
                Some(Hot::Grip(g)) => {
                    let id = g.spec().id;
                    host.begin_gesture(id);
                    self.drag = Some((g, pos, Self::plain(host, id)));
                    true
                }
                Some(Hot::Toggle(t)) => {
                    let id = t.id();
                    Self::gesture(host, id, if Self::plain(host, id) >= 0.5 { 0.0 } else { 1.0 });
                    true
                }
                None => {
                    // A click outside the panel closes it.
                    if self.browser.is_some() && !PANEL.contains(pos) {
                        self.browser = None;
                        return true;
                    }
                    false
                }
            },
            UiEvent::PointerUp { button: MouseButton::Left, .. } => {
                if let Some((g, _, _)) = self.drag.take() {
                    host.end_gesture(g.spec().id);
                    return true;
                }
                false
            }
            UiEvent::DoubleClick { pos, .. } => match self.hot_at(host, pos) {
                Some(Hot::Grip(g)) => {
                    self.drag = None;
                    let spec = g.spec();
                    Self::gesture(host, spec.id, spec.default);
                    true
                }
                _ => false,
            },
            UiEvent::Wheel { pos, lines, mods } => {
                if let Some(slot) = self.browser.as_ref().map(|b| b.slot) {
                    let n = self.rows(slot).len() as f64;
                    let max = (n * ROW_H - (PANEL.y1 - BODY_TOP - 60.0)).max(0.0);
                    if let Some(b) = &mut self.browser {
                        b.scroll = (b.scroll - f64::from(lines) * ROW_H).clamp(0.0, max);
                    }
                    return true;
                }
                match self.hot_at(host, pos) {
                    Some(Hot::Grip(g)) => {
                        let spec = g.spec();
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
        self.draw_browser(cx);
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
        // A download landed in the browser's slot: close it.
        if let Some(b) = &self.browser {
            let f = &self.files[b.slot.index()];
            if self.t3k.downloading.is_none() && f.source.as_ref().is_some_and(|s| self.t3k.models.as_ref().is_some_and(|m| m.iter().any(|x| x.id == s.model_id))) && matches!(self.status[b.slot.index()], Status::Loading | Status::Model(_) | Status::Cab(_)) && self.t3k.tone.as_ref().is_some_and(|t| f.source.as_ref().is_some_and(|s| s.tone_id == t.id)) {
                self.browser = None;
                self.ui.tone3000.close_tone();
                fresh = true;
            }
        }
        let (pin, pout) = self.ui.engine.take_peaks();
        let fall_db = (20.0 * dt) as f32;
        for (m, t) in self.meters.iter_mut().zip([pin, pout]) {
            *m = if t > *m { t } else { (*m - fall_db).max(t).max(SILENT) };
        }
        let loading = self.status.iter().any(|s| matches!(s, Status::Loading)) || self.t3k.downloading.is_some();
        fresh || busy || loading || self.picking.is_some() || self.meters.iter().any(|&m| m > SILENT + 1.0)
    }
}
