//! The Mite Gate editor (docs/design/gate-brief.md §3), in 1280×760 design units: Comp's frame.
//!
//! The history on the left (`mite_ui::widgets::history`: honest peaks, the input always in view)
//! with the threshold and the close threshold (threshold − hysteresis) as lines with grips, the
//! band between them shaded, and the detector's level dashed whenever it is not the input's peak;
//! the transfer square on the right, its curve from `mite_dsp::gate::transfer_db` (both branches
//! of the hysteresis loop for the Gate and the Ducker); the strip under it with the modes, the
//! detector and the dials; the meters.

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Instant;

use mite_core::{ParamDisplay, ParamScale, ParamSpec};
use mite_dsp::gate::{transfer_db, Detector, GateSettings, Mode, KEY_BANDS};
use mite_ui::kurbo::{Affine, BezPath, Cap, Circle, Join, Line, Point, Rect, RoundedRect, Stroke};
use mite_ui::paint::{glow_stroke, icon_button, panel, raised};
use mite_ui::peniko::{Color, Fill};
use mite_ui::session::{Session, SessionConfig};
use mite_ui::widgets::brand::{draw_mark, draw_title};
use mite_ui::widgets::footer::draw_dropdown_button;
use mite_ui::widgets::history::{self, HistoryLane};
use mite_ui::widgets::{ease_range, Dropdown, DropdownEvent, Footer, FooterDial, FooterToggle, Help, KeyEq, Knob, KnobKind, RangePicker, ToggleGlyph};
use mite_ui::{write_if_changed, Align, DrawCx, Key, Modifiers, MouseButton, ParamHost, RecordingHost, TextStyle, UiEvent, View};

mod midi;
pub use midi::MidiPanel;

use crate::dsp::{record, HISTORY_RECORDS, HISTORY_SLICE_MS};
use midi::MIDI_GLYPH;
use crate::params::*;
use crate::presets::{preset_ids, FACTORY, PLUGIN};
use crate::GateShared;

/// Mite Gate wears the Gate accent (tokens.json `color.accent.gate`, pale green).
pub const ACCENT: &str = "gate";

const WIDTH: f64 = 1280.0;
const HEIGHT: f64 = 760.0;
const HEADER: f64 = 56.0;
const DISPLAY_TOP: f64 = HEADER;
const DISPLAY_H: f64 = 620.0;
const HELP_TOP: f64 = DISPLAY_TOP + DISPLAY_H;
const FOOTER_TOP: f64 = HELP_TOP + 28.0;

const HIST_W: f64 = 776.0;
const HIST_SECONDS: f64 = 6.0;
const LEVEL_SPAN_PX: f64 = DISPLAY_H - 48.0;
const LEVEL_RANGES: &[f32] = &[24.0, 36.0, 48.0, 60.0, 84.0];
const LEVEL_RANGE_LABELS: &[&str] = &["Auto", "24 dB", "36 dB", "48 dB", "60 dB", "84 dB"];
const GRIP_X: f64 = HIST_W - 38.0;
const GRIP_SIZE: (f64, f64) = (34.0, 22.0);

/// The transfer square: the key's level across, the output up, over the history's range.
const SQ_X: f64 = 796.0;
const SQ_Y: f64 = DISPLAY_TOP + 16.0;
const SQ: f64 = 336.0;
const NODE_HIT: f64 = 16.0;

/// The strip under the square.
/// Buttons and dials sized for the hand (user, 2026-11-03: "buttons are too small").
const STRIP_TOP: f64 = SQ_Y + SQ + 14.0;
const BUTTON: f64 = 52.0;
const DIAL_D: f64 = 52.0;
const DIAL_ROWS: [f64; 2] = [STRIP_TOP + 104.0, STRIP_TOP + 190.0];
const DIAL_XS: [f64; 4] = [SQ_X + 42.0, SQ_X + 125.0, SQ_X + 208.0, SQ_X + 292.0];

const WELLS: [f64; 3] = [1194.0, 1220.0, 1246.0];
const METER_TOP: f64 = DISPLAY_TOP + 20.0;
const METER_BOTTOM: f64 = DISPLAY_TOP + 592.0;
const METER_RELEASE_DB_S: f32 = 20.0;
const SILENT: f32 = -150.0;

/// Glyphs (24×24, stroked): each mode a drawing of its curve, the two detectors' envelopes.
const MODE_GLYPHS: [&str; 3] = [
    "M3 20 H8.5 C9.8 20 9.6 9.6 11 9 L21 3",
    "M4.5 21 C7.5 18.5 9 13.5 12 11 L21 3",
    "M3 13 L9.5 9.2 C11 8.4 10.8 16.5 12.4 15.8 L21 11",
];
const DETECTOR_GLYPHS: [&str; 2] = ["M3 18 H9 C10.5 18 11 4 12 4 C13 4 13.5 18 15 18 H21", "M3 18 C7 18 8.5 9 12 9 C15.5 9 17 18 21 18"];
const KEY_GLYPH: &str = "M7.5 15.5 A3.5 3.5 0 1 1 7.51 15.5 M10.5 13.5 L20 7 M17 9 L19 12 M14.5 10.7 L16 13";

thread_local! {
    static LEVEL_RANGE: std::cell::Cell<f64> = const { std::cell::Cell::new(60.0) };
    static HEADROOM: std::cell::Cell<f64> = const { std::cell::Cell::new(0.0) };
}

fn set_scale(range: f32, headroom: f32) {
    LEVEL_RANGE.with(|r| r.set(f64::from(range)));
    HEADROOM.with(|h| h.set(f64::from(headroom)));
}

fn level_range() -> f64 {
    LEVEL_RANGE.with(std::cell::Cell::get)
}

fn headroom() -> f64 {
    HEADROOM.with(std::cell::Cell::get)
}

fn history_lane() -> HistoryLane {
    HistoryLane { x0: 0.0, width: HIST_W, top: DISPLAY_TOP, bottom: HELP_TOP, seconds: HIST_SECONDS, slice_ms: HISTORY_SLICE_MS }
}

/// A level's y on the history: the top line is the headroom (0 dB unless something goes over).
fn lv_y(db: f32) -> f64 {
    let (range, head) = (level_range(), headroom());
    DISPLAY_TOP + 24.0 + (head - f64::from(db)).clamp(0.0, range + head) / (range + head) * LEVEL_SPAN_PX
}

fn px_per_db() -> f64 {
    LEVEL_SPAN_PX / (level_range() + headroom())
}

/// The square over the history's range (−range…0 dB both ways).
fn sq_x(db: f32) -> f64 {
    let r = level_range();
    SQ_X + (f64::from(db) + r).clamp(0.0, r) / r * SQ
}

fn sq_y(db: f32) -> f64 {
    let r = level_range();
    SQ_Y + SQ - (f64::from(db) + r).clamp(0.0, r) / r * SQ
}

/// `sq_y` without the clamp: curves run off the square and are clipped, never flattened onto
/// its edge (that would draw a corner the DSP does not have).
fn sq_y_free(db: f32) -> f64 {
    let r = level_range();
    SQ_Y + SQ - (f64::from(db) + r) / r * SQ
}

fn sq_rect() -> Rect {
    Rect::new(SQ_X, SQ_Y, SQ_X + SQ, SQ_Y + SQ)
}

fn draw_glyph(scene: &mut mite_ui::vello::Scene, d: &str, c: Point, size: f64, color: Color, width: f64) {
    let Ok(mut path) = BezPath::from_svg(d) else { return };
    let k = size / 24.0;
    path.apply_affine(Affine::translate((c.x - 12.0 * k, c.y - 12.0 * k)) * Affine::scale(k));
    scene.stroke(&Stroke::new(width).with_caps(Cap::Round).with_join(Join::Round), Affine::IDENTITY, color, None, &path);
}

fn fmt_db(v: f32) -> String {
    format!("{}{:.1}", if v < -0.05 { "−" } else { "" }, v.abs())
}

/// A stand-in host holding every Mite Gate parameter at its default (tests, preview, tools).
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
                let base = spec_for(id).expect("every manifest line has a spec");
                ParamSpec { id, default: f[4].parse().unwrap_or(base.default), ..base }
            }
        };
        host = host.with(spec);
    }
    host
}

/// The gate's settings as a host's parameters set them (as `GateParams::settings` does).
pub fn settings(host: &dyn ParamHost) -> GateSettings {
    let p = |id: &str| host.plain(id, host.normalized(id));
    let key = |b: usize, param: &str| p(&key_id(b, param));
    GateSettings {
        mode: Mode::from_index(p(MODE.id).round().max(0.0) as usize),
        threshold_db: p(THRESHOLD.id),
        range_db: p(RANGE.id),
        ratio: p(RATIO.id),
        hysteresis_db: p(HYSTERESIS.id),
        attack_ms: p(ATTACK.id),
        hold_ms: p(HOLD.id),
        release_ms: p(RELEASE.id),
        lookahead_ms: p(LOOKAHEAD.id),
        detector: Detector::from_index(p(DETECTOR.id).round().max(0.0) as usize),
        link: p(LINK.id) / 100.0,
        external_key: p(SC_SOURCE.id) >= 0.5,
        key_bands: std::array::from_fn(|b| mite_dsp::eq::BandSettings {
            enabled: key(b, K_ON.id) >= 0.5,
            kind: mite_dsp::eq::BandType::from_index(key(b, K_TYPE.id).round().max(0.0) as usize),
            freq: f64::from(key(b, K_FREQ.id)),
            gain_db: f64::from(key(b, K_GAIN.id)),
            q: f64::from(key(b, K_Q.id)),
            slope: key(b, K_SLOPE.id).round().max(0.0) as usize,
            ..mite_dsp::eq::BandSettings::default()
        }),
        listen: p(LISTEN.id) >= 0.5,
    }
}

/// The MIDI trigger as a host's parameters set it (as `GateParams::midi` does).
pub fn midi(host: &dyn ParamHost) -> crate::dsp::MidiSettings {
    let p = |id: &str| host.plain(id, host.normalized(id));
    crate::dsp::MidiSettings {
        on: p(MIDI_ON.id) >= 0.5,
        note: p(MIDI_NOTE.id).round().clamp(0.0, 127.0) as u8,
        channel: (p(MIDI_CHANNEL.id).round().clamp(1.0, 16.0) - 1.0) as u8,
        from_level: p(MIDI_VELOCITY.id).round() as i32 == 1,
        fixed: p(MIDI_VEL_FIXED.id).round().clamp(1.0, 127.0) as u8,
    }
}

/// The output stage as a host's parameters set it.
pub fn output(host: &dyn ParamHost) -> crate::dsp::Output {
    let p = |id: &str| host.plain(id, host.normalized(id));
    crate::dsp::Output { mix: p(MIX.id) / 100.0, gain_db: p(OUT_GAIN.id), delta: p(DELTA.id) >= 0.5, bypass: p(BYPASS.id) >= 0.5 }
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Drag {
    /// The threshold's grip on the history (or its node in the square).
    Threshold { start: Point, threshold: f32, square: bool },
    /// The close threshold's grip: the hysteresis.
    Close { start: Point, hysteresis: f32 },
    /// Up and down in the square: the range.
    Range { start: Point, range: f32 },
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Hot {
    Threshold,
    Close,
    Node,
    Square,
    Mode(usize),
    Detector(usize),
    Source,
    EqButton,
    EqPanel,
    MidiButton,
    MidiPanel,
}

pub struct GateView {
    shared: Arc<GateShared>,
    session: Session,
    footer: Footer,
    range: RangePicker,
    range_shown: f32,
    range_target: f32,
    headroom: f32,
    knobs: Vec<Knob>,
    knobs_mode: Option<Mode>,
    pointer: Option<Point>,
    hot: Option<Hot>,
    drag: Option<Drag>,
    source_menu: Option<Dropdown>,
    /// The sidechain EQ (its panel open or not).
    key_eq: KeyEq,
    key_spectrum: mite_ui::widgets::key_eq::KeySpectrum,
    eq_open: bool,
    midi: MidiPanel,
    midi_open: bool,
    history: VecDeque<[f32; record::WIDTH]>,
    seen: usize,
    /// IN L/R, GAIN L/R, OUT L/R as drawn.
    meters: [f32; 6],
    last_fresh: Instant,
    last_tick: Instant,
}

const SOURCE_BUTTON: Rect = Rect::new(16.0, FOOTER_TOP + 9.0, 156.0, FOOTER_TOP + 47.0);
/// The sidechain EQ's button (beside the source) and its panel, docked over the history's
/// bottom as Comp's.
const EQ_BUTTON: Rect = Rect::new(164.0, FOOTER_TOP + 9.0, 204.0, FOOTER_TOP + 47.0);
/// It stops short of the grips' column, so both thresholds stay in reach. Opaque: a glass panel
/// blurred the threshold lines behind it into a green haze (user, 2026-11-03).
const EQ_PANEL: Rect = Rect::new(12.0, DISPLAY_TOP + 300.0, GRIP_X - 10.0, DISPLAY_TOP + 610.0);
/// The MIDI trigger's button (beside the EQ's) and its panel, docked as the EQ's.
const MIDI_BUTTON: Rect = Rect::new(212.0, FOOTER_TOP + 9.0, 252.0, FOOTER_TOP + 47.0);
const MIDI_PANEL: Rect = Rect::new(12.0, DISPLAY_TOP + 400.0, GRIP_X - 10.0, DISPLAY_TOP + 610.0);
/// How long a sent note lights its key, s.
const NOTE_FLASH_S: f64 = 0.25;
/// The sidechain EQ's sliders glyph.
const EQ_GLYPH: &str = "M4 7 H20 M4 12 H20 M4 17 H20 M8.5 5 V9 M15 10 V14 M10.5 15 V19";

impl GateView {
    pub fn new(shared: Arc<GateShared>) -> Self {
        let footer = Footer::new(
            FOOTER_TOP,
            (WIDTH, HEIGHT),
            vec![
                FooterToggle { id: LISTEN.id, glyph: ToggleGlyph::Listen, words: "Listen — hear the key as the detector hears it (after the sidechain EQ)" },
                FooterToggle { id: DELTA.id, glyph: ToggleGlyph::Delta, words: "Delta — hear only what the gate takes away" },
                FooterToggle { id: BYPASS.id, glyph: ToggleGlyph::Bypass, words: "Bypass — the input, latency kept" },
            ],
            vec![
                FooterDial { spec: MIX, kind: KnobKind::Unipolar, name: "MIX", words: "Mix — the gated signal against the dry one, delay-compensated" },
                FooterDial { spec: OUT_GAIN, kind: KnobKind::Bipolar, name: "OUTPUT", words: "Output gain" },
            ],
        );
        let range = RangePicker::new(Rect::new(8.0, DISPLAY_TOP + 4.0, 54.0, DISPLAY_TOP + 22.0), Rect::new(0.0, DISPLAY_TOP + 24.0, 36.0, HELP_TOP), LEVEL_RANGES, LEVEL_RANGE_LABELS, shared.display_range.clone(), (DISPLAY_TOP, HELP_TOP)).with_prefix("−");
        Self {
            session: Session::new(SessionConfig { plugin: PLUGIN, factory: FACTORY, sound_ids: preset_ids(), snapshot_ids: preset_ids() }, shared.snapshots.clone(), shared.preset.clone()),
            footer,
            range,
            range_shown: 60.0,
            range_target: 60.0,
            headroom: 0.0,
            knobs: Vec::new(),
            knobs_mode: None,
            pointer: None,
            hot: None,
            drag: None,
            source_menu: None,
            key_eq: {
                let mut k = KeyEq::new(Rect::new(EQ_PANEL.x0 + 16.0, EQ_PANEL.y0 + 44.0, EQ_PANEL.x1 - 16.0, EQ_PANEL.y1 - 14.0), KEY_BANDS).with_slopes();
                k.menu_bounds = (DISPLAY_TOP, FOOTER_TOP);
                k
            },
            key_spectrum: mite_ui::widgets::key_eq::KeySpectrum::new(),
            eq_open: false,
            midi: MidiPanel::new(MIDI_PANEL),
            midi_open: false,
            history: VecDeque::with_capacity(HISTORY_RECORDS),
            seen: 0,
            meters: [SILENT, SILENT, 0.0, 0.0, SILENT, SILENT],
            last_fresh: Instant::now(),
            last_tick: Instant::now(),
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

    /// Lets the range settle at once (tools render a single frame).
    pub fn settle(&mut self) {
        self.range_shown = self.range_target;
        set_scale(self.range_shown, self.headroom);
    }

    // ---- geometry for tests and tools -------------------------------------------------------

    pub fn level_y(&self, db: f32) -> f64 {
        set_scale(self.range_shown, self.headroom);
        lv_y(db)
    }

    pub fn threshold_grip(&self, host: &dyn ParamHost) -> Rect {
        set_scale(self.range_shown, self.headroom);
        Self::grip(lv_y(Self::plain(host, THRESHOLD.id)))
    }

    pub fn close_grip(&self, host: &dyn ParamHost) -> Rect {
        set_scale(self.range_shown, self.headroom);
        Self::grip(lv_y(Self::close(host))).inflate(-3.0, -2.0)
    }

    pub fn threshold_node(&self, host: &dyn ParamHost) -> Point {
        set_scale(self.range_shown, self.headroom);
        let t = Self::plain(host, THRESHOLD.id);
        Point::new(sq_x(t), sq_y(t))
    }

    pub fn square(&self) -> Rect {
        sq_rect()
    }

    pub fn mode_button(&self, i: usize) -> Rect {
        Self::mode_rect(i)
    }

    pub fn detector_button(&self, i: usize) -> Rect {
        Self::detector_rect(i)
    }

    pub fn dial(&self, id: &str) -> Option<Point> {
        self.knobs.iter().find(|k| k.id == id).map(|k| k.center)
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

    /// The sidechain EQ's well while its panel is open.
    pub fn eq_area(&self) -> Option<Rect> {
        self.eq_open.then_some(self.key_eq.area)
    }

    /// Where key band `b`'s node is drawn (tests and tools).
    pub fn eq_node(&self, host: &dyn ParamHost, b: usize) -> Point {
        mite_ui::widgets::key_eq::node(self.key_eq.area, &mite_ui::widgets::key_eq::band_with(host, b, true))
    }

    /// The shape menu's rows while it is open (tests and tools).
    pub fn eq_menu_rows(&self) -> Vec<Rect> {
        self.key_eq.menu_rows()
    }

    pub fn midi_button(&self) -> Rect {
        MIDI_BUTTON
    }

    pub fn midi_panel(&self) -> Option<&MidiPanel> {
        self.midi_open.then_some(&self.midi)
    }

    pub fn open_midi(&mut self) {
        self.midi_open = true;
        self.eq_open = false;
    }

    pub fn open_eq(&mut self) {
        self.eq_open = true;
    }

    /// Reads the key's spectrum at once (tools render a single frame).
    pub fn settle_spectrum(&mut self) {
        let fs = f64::from(self.shared.sample_rate.load());
        self.key_spectrum.settle(&self.shared.key_spectrum, fs);
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

    fn mode(host: &dyn ParamHost) -> Mode {
        Mode::from_index(Self::plain(host, MODE.id).round().max(0.0) as usize)
    }

    /// The close threshold, dB: threshold − hysteresis.
    fn close(host: &dyn ParamHost) -> f32 {
        Self::plain(host, THRESHOLD.id) - Self::plain(host, HYSTERESIS.id)
    }

    /// Whether the detector is not simply the input's peak (RMS, a filtered or external key):
    /// then its level is drawn, since that is what the thresholds are compared with.
    fn detector_differs(host: &dyn ParamHost) -> bool {
        Self::plain(host, DETECTOR.id).round() as usize == 1 || Self::on(host, SC_SOURCE.id) || (0..KEY_BANDS).any(|b| Self::on(host, &key_id(b, K_ON.id)))
    }

    fn grip(y: f64) -> Rect {
        Rect::new(GRIP_X, y - GRIP_SIZE.1 / 2.0, GRIP_X + GRIP_SIZE.0, y + GRIP_SIZE.1 / 2.0)
    }

    fn mode_rect(i: usize) -> Rect {
        let x = SQ_X + i as f64 * (BUTTON + 10.0);
        Rect::new(x, STRIP_TOP, x + BUTTON, STRIP_TOP + 44.0)
    }

    fn detector_rect(i: usize) -> Rect {
        let x = SQ_X + SQ - (2 - i) as f64 * (BUTTON + 10.0) + 10.0;
        Rect::new(x, STRIP_TOP, x + BUTTON, STRIP_TOP + 44.0)
    }

    fn since_fresh(&self) -> f64 {
        (self.last_fresh.elapsed().as_secs_f64() * 1000.0 / f64::from(HISTORY_SLICE_MS)).min(4.0)
    }

    // ---- drawing ----------------------------------------------------------------------------

    fn draw_header(&mut self, cx: &mut DrawCx<'_>) {
        let theme = cx.theme;
        cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.window, None, &Rect::new(0.0, 0.0, WIDTH, HEADER));
        cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, theme.line, None, &Line::new((0.0, HEADER - 0.5), (WIDTH, HEADER - 0.5)));
        draw_mark(cx, 16.0, 15.0, 26.0);
        draw_title(cx, 52.0, 18.0, "Gate");
        self.session.draw(cx);
    }

    fn draw_history(&self, cx: &mut DrawCx<'_>) {
        let theme = cx.theme;
        let lane = history_lane();
        cx.begin_clip(&lane.rect());
        lane.draw_grid(cx, self.range_target, self.headroom, lv_y);
        // The hysteresis band, between the close threshold and the threshold.
        let (t, close) = (Self::plain(cx.host, THRESHOLD.id), Self::close(cx.host));
        cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.accent.multiply_alpha(0.07), None, &Rect::new(0.0, lv_y(t), GRIP_X - 2.0, lv_y(close)));
        let cols = lane.columns(&self.history, self.seen, self.since_fresh());
        if cols.len() >= 2 {
            type Col = history::Column<{ record::WIDTH }>;
            let delta = Self::on(cx.host, DELTA.id);
            let listen = Self::on(cx.host, LISTEN.id);
            let level = |c: &Col, k: usize| c.high[k].max(c.high[k + 1]);
            let line = |f: &dyn Fn(&Col) -> f64| history::column_line(&cols, f);
            let input = line(&|c| lv_y(level(c, record::IN)));
            cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.spectrum_pre.multiply_alpha(0.16), None, &history::close_to(input.clone(), &cols, HELP_TOP));
            let out_color = if listen { theme.sidechain } else { theme.curve };
            let output = if delta {
                line(&|c| {
                    let (i, o) = (mite_dsp::db_to_gain(level(c, record::IN)), mite_dsp::db_to_gain(level(c, record::OUT)));
                    lv_y(mite_dsp::gain_to_db((i - o).abs().max(1e-6)))
                })
            } else {
                line(&|c| lv_y(level(c, record::OUT)))
            };
            let a = if delta { 0.3 } else { 0.12 };
            let look = history::LevelLook { fill: [out_color.multiply_alpha(a), out_color.multiply_alpha(0.01)], span: (DISPLAY_TOP, HELP_TOP), edge: out_color, width: 1.8, glow: 2.4 };
            history::draw_level(cx, &cols, &output, HELP_TOP, look);
            if cols.iter().any(|c| c.low[record::GAIN].min(c.low[record::GAIN + 1]) < -0.05) {
                let edge = line(&|c| DISPLAY_TOP + (f64::from(-c.low[record::GAIN].min(c.low[record::GAIN + 1])) * px_per_db()).clamp(0.0, HELP_TOP - DISPLAY_TOP));
                history::draw_reduction(cx, &cols, &edge, DISPLAY_TOP, 0.5 * LEVEL_SPAN_PX);
            }
            if Self::detector_differs(cx.host) {
                let det = line(&|c| lv_y(c.high[record::KEY]));
                cx.scene.stroke(&Stroke::new(1.4).with_dashes(0.0, [6.0, 3.0]), Affine::IDENTITY, theme.text_hi.multiply_alpha(0.75), None, &det);
            }
            cx.scene.stroke(&Stroke::new(1.2), Affine::IDENTITY, theme.spectrum_pre.multiply_alpha(0.85), None, &input);
            // Each MIDI note sent: a tick down from the top, as long as its velocity.
            if Self::on(cx.host, MIDI_ON.id) {
                for c in cols.iter().filter(|c| c.high[record::NOTE] > 0.0) {
                    let len = 6.0 + 30.0 * f64::from(c.high[record::NOTE]) / 127.0;
                    let x = c.x;
                    cx.scene.stroke(&Stroke::new(2.0).with_caps(Cap::Round), Affine::IDENTITY, theme.accent.multiply_alpha(0.85), None, &Line::new((x, DISPLAY_TOP + 4.0), (x, DISPLAY_TOP + 4.0 + len)));
                }
            }
        }
        cx.end_clip();
        self.draw_thresholds(cx);
        self.range.draw(cx, self.range_shown);
    }

    fn draw_thresholds(&self, cx: &mut DrawCx<'_>) {
        let theme = cx.theme;
        let (t, close) = (Self::plain(cx.host, THRESHOLD.id), Self::close(cx.host));
        // The close threshold: dashed, its grip smaller.
        let hot_c = self.hot == Some(Hot::Close) || matches!(self.drag, Some(Drag::Close { .. }));
        let yc = lv_y(close);
        cx.scene.stroke(&Stroke::new(1.3).with_dashes(0.0, [5.0, 4.0]), Affine::IDENTITY, theme.accent.multiply_alpha(0.8), None, &Line::new((0.0, yc), (GRIP_X - 2.0, yc)));
        let rc = Self::grip(yc).inflate(-3.0, -2.0);
        let shape = RoundedRect::from_rect(rc, 9.0);
        cx.scene.fill(Fill::NonZero, Affine::IDENTITY, if hot_c { theme.accent } else { theme.display }, None, &shape);
        cx.scene.stroke(&Stroke::new(1.3).with_dashes(0.0, [3.0, 2.0]), Affine::IDENTITY, theme.accent, None, &shape);
        draw_chevrons(cx, rc.center(), if hot_c { theme.display } else { theme.accent });
        // The threshold: solid, lit.
        let hot = self.hot == Some(Hot::Threshold) || matches!(self.drag, Some(Drag::Threshold { square: false, .. }));
        let y = lv_y(t);
        glow_stroke(cx, &Line::new((0.0, y), (GRIP_X - 2.0, y)), 1.5, theme.accent, theme.accent, if hot { 4.0 } else { 2.5 });
        let r = Self::grip(y);
        let shape = RoundedRect::from_rect(r, 11.0);
        cx.scene.fill(Fill::NonZero, Affine::IDENTITY, if hot { theme.accent } else { theme.display }, None, &shape);
        glow_stroke(cx, &shape, 1.5, theme.accent, theme.accent, if hot { 4.0 } else { 2.0 });
        draw_chevrons(cx, r.center(), if hot { theme.display } else { theme.accent });
    }

    fn draw_square(&self, cx: &mut DrawCx<'_>) {
        let theme = cx.theme;
        let host = cx.host;
        let r = sq_rect();
        panel(cx.scene, RoundedRect::from_rect(r, 10.0), theme.well, theme.well_border);
        let range = level_range() as f32;
        let step = if range <= 24.5 { 6.0 } else { 12.0 };
        for db in (1..).map(|k| -step * k as f32).take_while(|&db| db > -range + 0.5) {
            cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, theme.grid_minor, None, &Line::new((sq_x(db), r.y0), (sq_x(db), r.y1)));
            cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, theme.grid_minor, None, &Line::new((r.x0, sq_y(db)), (r.x1, sq_y(db))));
        }
        cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, theme.grid_major, None, &Line::new((sq_x(-range), sq_y(-range)), (sq_x(0.0), sq_y(0.0))));
        let s = settings(host);
        let (t, close) = (s.threshold_db, s.threshold_db - s.hysteresis_db);
        // The hysteresis band.
        if s.mode != Mode::Expander {
            cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.accent.multiply_alpha(0.09), None, &Rect::new(sq_x(close), r.y0, sq_x(t), r.y1));
        }
        // Scene and glow both: the curves run off the square's edges.
        cx.begin_clip(&RoundedRect::from_rect(r, 10.0));
        let curve = |from: f32, to: f32, open: bool| -> BezPath {
            let mut p = BezPath::new();
            let n = 160;
            for i in 0..=n {
                let l = from + (to - from) * i as f32 / n as f32;
                let pt = Point::new(sq_x(l), sq_y_free(transfer_db(&s, l, open)));
                if i == 0 { p.move_to(pt) } else { p.line_to(pt) }
            }
            p
        };
        let accent = theme.accent;
        match s.mode {
            Mode::Expander => glow_stroke(cx, &curve(-range, 0.0, true), 2.0, accent, accent, 4.0),
            Mode::Gate | Mode::Ducker => {
                // Both branches of the loop: the open one from the close threshold up, the closed
                // one up to the threshold, and dashed jumps where the state changes.
                let (passing, held) = if s.mode == Mode::Gate { (true, false) } else { (false, true) };
                let upper_from = if s.mode == Mode::Gate { close } else { -range };
                let upper_to = if s.mode == Mode::Gate { 0.0 } else { t };
                let lower_from = if s.mode == Mode::Gate { -range } else { close };
                let lower_to = if s.mode == Mode::Gate { t } else { 0.0 };
                glow_stroke(cx, &curve(upper_from, upper_to, passing), 2.0, accent, accent, 4.0);
                glow_stroke(cx, &curve(lower_from, lower_to, held), 2.0, accent.multiply_alpha(0.85), accent, 3.0);
                let dash = Stroke::new(1.2).with_dashes(0.0, [3.0, 3.0]);
                for (at, a, b) in [(t, transfer_db(&s, t, false), transfer_db(&s, t, true)), (close, transfer_db(&s, close, true), transfer_db(&s, close, false))] {
                    cx.scene.stroke(&dash, Affine::IDENTITY, accent.multiply_alpha(0.7), None, &Line::new((sq_x(at), sq_y_free(a)), (sq_x(at), sq_y_free(b))));
                }
            }
        }
        // The live dot: the key's level, on the branch the gate is on.
        if let Some(last) = self.history.back().filter(|r| r[record::KEY] > -range) {
            let l = last[record::KEY];
            let open = last[record::ACTIVE] >= 0.5;
            let p = Point::new(sq_x(l), sq_y(transfer_db(&s, l, if s.mode == Mode::Ducker { open } else { open || s.mode == Mode::Expander })));
            cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.text_strong, None, &Circle::new(p, 5.0));
            cx.glow().fill(Fill::NonZero, Affine::IDENTITY, theme.text_strong.multiply_alpha(0.5), None, &Circle::new(p, 7.0));
        }
        cx.end_clip();
        // The threshold node.
        let hot = matches!(self.hot, Some(Hot::Node)) || matches!(self.drag, Some(Drag::Threshold { square: true, .. }));
        let node = Point::new(sq_x(t), sq_y(t));
        cx.scene.fill(Fill::NonZero, Affine::IDENTITY, if hot { accent } else { theme.display }, None, &Circle::new(node, 7.0));
        glow_stroke(cx, &Circle::new(node, 7.0), 2.0, accent, accent, if hot { 5.0 } else { 3.0 });
    }

    fn draw_strip(&self, cx: &mut DrawCx<'_>) {
        let theme = cx.theme;
        let mode = Self::mode(cx.host);
        for (i, g) in MODE_GLYPHS.iter().enumerate() {
            let r = Self::mode_rect(i);
            let ink = icon_button(cx.scene, theme, r, theme.panel_control, self.hot == Some(Hot::Mode(i)), Mode::ALL[i] == mode, theme.accent);
            draw_glyph(cx.scene, g, r.center(), 28.0, ink, 1.7);
        }
        let det = Self::plain(cx.host, DETECTOR.id).round() as usize;
        for (i, g) in DETECTOR_GLYPHS.iter().enumerate() {
            let r = Self::detector_rect(i);
            let ink = icon_button(cx.scene, theme, r, theme.panel_control, self.hot == Some(Hot::Detector(i)), i == det, theme.accent);
            draw_glyph(cx.scene, g, r.center(), 26.0, ink, 1.7);
        }
        for k in &self.knobs {
            k.draw(cx);
        }
    }

    fn draw_meters(&self, cx: &mut DrawCx<'_>) {
        let theme = cx.theme;
        cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, theme.line, None, &Line::new((HIST_W + 0.5, DISPLAY_TOP), (HIST_W + 0.5, HELP_TOP)));
        for x in WELLS {
            panel(cx.scene, RoundedRect::new(x, METER_TOP, x + 20.0, METER_BOTTOM, 3.0), theme.meter_well, theme.well_border);
        }
        let seg = Stroke::new(8.0).with_dashes(0.0, [2.0, 2.0]);
        let (top, bottom) = (METER_TOP + 4.0, METER_BOTTOM - 4.0);
        let span = bottom - top;
        let (range, head) = (level_range(), headroom());
        let level_y = |db: f32| bottom - ((f64::from(db) + range).clamp(0.0, range + head) / (range + head) * span / 4.0).round() * 4.0;
        let gr_y = |g: f32| top + (f64::from(-g).clamp(0.0, range + head) / (range + head) * span).min(span);
        for (i, x) in WELLS.iter().flat_map(|&w| [w + 5.0, w + 15.0]).enumerate() {
            cx.scene.stroke(&seg, Affine::IDENTITY, theme.meter_off, None, &Line::new((x, top), (x, bottom)));
            let v = self.meters[i];
            let (line, color) = match i {
                0 | 1 => (Line::new((x, bottom), (x, level_y(v))), theme.spectrum_pre),
                2 | 3 => (Line::new((x, top), (x, gr_y(v))), theme.gain_reduction),
                _ => (Line::new((x, bottom), (x, level_y(v))), theme.curve),
            };
            if (line.p1.y - line.p0.y).abs() >= 2.0 {
                cx.scene.stroke(&seg, Affine::IDENTITY, color, None, &line);
                if i >= 2 {
                    cx.glow().stroke(&Stroke::new(8.0), Affine::IDENTITY, color.multiply_alpha(0.3), None, &line);
                }
            }
        }
        // The thresholds across the IN meter.
        for (db, w) in [(Self::plain(cx.host, THRESHOLD.id), 2.0), (Self::close(cx.host), 1.0)] {
            let y = level_y(db);
            cx.scene.stroke(&Stroke::new(w), Affine::IDENTITY, theme.accent, None, &Line::new((WELLS[0] - 2.0, y), (WELLS[0] + 22.0, y)));
        }
        for (x, t) in [(WELLS[0], "IN"), (WELLS[1], "GR"), (WELLS[2], "OUT")] {
            cx.text.draw(cx.scene, t, x + 10.0, DISPLAY_TOP + 596.0, TextStyle::LABEL.sized(9.0), theme.text_lo, Align::Center);
        }
    }

    fn draw_footer(&mut self, cx: &mut DrawCx<'_>) {
        let theme = cx.theme;
        self.footer.draw_frame(cx);
        let external = Self::on(cx.host, SC_SOURCE.id);
        let hot = self.hot == Some(Hot::Source);
        draw_dropdown_button(cx, SOURCE_BUTTON, if external { "External" } else { "Internal" }, self.source_menu.is_some(), hot, |scene, at| draw_glyph(scene, KEY_GLYPH, at + (8.0, 6.0), 20.0, theme.sidechain, 1.7));
        let sample_rate = f64::from(self.shared.sample_rate.load()).max(1.0);
        let eq_on = (0..KEY_BANDS).any(|b| Self::on(cx.host, &key_id(b, K_ON.id)));
        let ink = icon_button(cx.scene, theme, EQ_BUTTON, theme.panel_control, self.hot == Some(Hot::EqButton), self.eq_open || eq_on, theme.sidechain);
        draw_glyph(cx.scene, EQ_GLYPH, EQ_BUTTON.center(), 22.0, ink, 1.6);
        let midi_on = Self::on(cx.host, MIDI_ON.id);
        let ink = icon_button(cx.scene, theme, MIDI_BUTTON, theme.panel_control, self.hot == Some(Hot::MidiButton), self.midi_open || midi_on, theme.accent);
        draw_glyph(cx.scene, MIDI_GLYPH, MIDI_BUTTON.center(), 22.0, ink, 1.6);
        self.footer.draw_latency(cx, 268.0, f64::from(self.shared.latency.load()) / sample_rate * 1000.0);
        self.footer.draw_controls(cx);
        if let Some(list) = &self.source_menu {
            list.draw(cx, theme.accent, |_, _, _, _| {});
        }
    }

    fn draw_eq_panel(&self, cx: &mut DrawCx<'_>) {
        if !self.eq_open {
            return;
        }
        let theme = cx.theme;
        raised(cx.scene, RoundedRect::from_rect(EQ_PANEL, 12.0), theme.window, theme.panel_control_border, theme.shadows);
        draw_glyph(cx.scene, KEY_GLYPH, Point::new(EQ_PANEL.x0 + 28.0, EQ_PANEL.y0 + 22.0), 20.0, theme.sidechain, 1.7);
        cx.text.draw(cx.scene, "Sidechain", EQ_PANEL.x0 + 46.0, EQ_PANEL.y0 + 15.0, TextStyle::BODY.weighted(700.0), theme.text_hi, Align::Left);
        self.key_eq.draw_with(cx, Some(&self.key_spectrum));
    }

    /// The last note sent within [`NOTE_FLASH_S`]: its velocity and what is left of its flash.
    fn note_flash(&self) -> Option<(f32, f32)> {
        let slices = (NOTE_FLASH_S * 1000.0 / f64::from(HISTORY_SLICE_MS)) as usize;
        self.history.iter().rev().take(slices).enumerate().find(|(_, r)| r[record::NOTE] > 0.0).map(|(age, r)| {
            let age = (age as f64 + self.since_fresh()) / slices as f64;
            (r[record::NOTE], (1.0 - age.min(1.0)) as f32)
        })
    }

    fn draw_midi_panel(&self, cx: &mut DrawCx<'_>) {
        if self.midi_open {
            self.midi.draw(cx, self.note_flash());
        }
    }

    fn draw_states(&self, cx: &mut DrawCx<'_>) {
        let theme = cx.theme;
        for (on, glyph, x) in [(Self::on(cx.host, DELTA.id), ToggleGlyph::Delta, HIST_W - 56.0), (Self::on(cx.host, LISTEN.id), ToggleGlyph::Listen, HIST_W - 100.0)] {
            if on {
                let r = RoundedRect::from_rect(Rect::new(x, DISPLAY_TOP + 10.0, x + 36.0, DISPLAY_TOP + 46.0), 10.0);
                cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.accent.multiply_alpha(0.18), None, &r);
                glow_stroke(cx, &r, 1.5, theme.accent, theme.accent, 9.0);
                mite_ui::widgets::footer::draw_toggle_glyph(cx.scene, glyph, r.center(), theme.accent);
            }
        }
    }

    // ---- help -------------------------------------------------------------------------------

    fn help_text(&self, host: &dyn ParamHost) -> Help {
        let t = Self::plain(host, THRESHOLD.id);
        let h = Self::plain(host, HYSTERESIS.id);
        let range = Self::plain(host, RANGE.id);
        let closes = match Self::mode(host) {
            Mode::Ducker => "lets go",
            Mode::Expander => "is fully open",
            Mode::Gate => "closes",
        };
        match self.drag {
            Some(Drag::Threshold { .. }) => return Help::default().text("Threshold ").slot(format!("{} dB", fmt_db(t)), "−80.0 dB").text(" — where the key reaches this line the gate acts. Shift is fine."),
            Some(Drag::Close { .. }) => return Help::default().text("Hysteresis ").slot(format!("{h:.1} dB"), "24.0 dB").text(format!(" — under the dashed line it {closes}; between the lines it keeps its state.")),
            Some(Drag::Range { .. }) => return Help::default().text("Range ").slot(format!("{} dB", fmt_db(range)), "−80.0 dB").text(" — how far it turns the audio down."),
            None => {}
        }
        if self.key_eq.menu_open() {
            if let Some(h) = self.key_eq.help(host) {
                return h;
            }
        }
        if let Some(list) = &self.source_menu {
            if let Some(i) = self.pointer.and_then(|p| list.item_rects().iter().position(|r| r.contains(p))) {
                return Help::default().text(if i == 0 { "Internal — the input itself keys the gate" } else { "External — the sidechain input keys the gate (route a track to the plugin's second input)" });
            }
        }
        match self.hot {
            Some(Hot::Threshold) | Some(Hot::Node) => return Help::default().text("Threshold ").slot(format!("{} dB", fmt_db(t)), "−80.0 dB").text(" — drag it; wheel steps; double-click resets."),
            Some(Hot::Close) => return Help::default().text("Close threshold ").slot(format!("{} dB", fmt_db(t - h)), "−80.0 dB").text(format!(" — under it the gate {closes} (after the hold); drag for the hysteresis.")),
            Some(Hot::Square) => return Help::default().text("Range ").slot(format!("{} dB", fmt_db(range)), "−80.0 dB").text(" — drag up or down; the node on the diagonal is the threshold."),
            Some(Hot::Mode(i)) => return Help::default().text(format!("{} — {}", Mode::NAMES[i], mode_words(i))),
            Some(Hot::Detector(i)) => return Help::default().text(if i == 0 { "Peak — instant attack, 10 ms release: catches every hit" } else { "RMS — the power over 10 ms: steadier on sustained sounds" }),
            Some(Hot::Source) => return Help::default().text("Sidechain — what keys the gate: the input, or the plugin's sidechain input."),
            Some(Hot::EqButton) => return Help::default().text(if self.eq_open { "Sidechain EQ — click to close" } else { "Sidechain EQ — shape what the detector hears (a low cut keeps a kick's bleed out of a snare's gate)" }),
            Some(Hot::EqPanel) => return self.key_eq.help(host).unwrap_or_default(),
            Some(Hot::MidiButton) => return Help::default().text(if self.midi_open { "MIDI trigger — click to close" } else { "MIDI trigger — send a note at each opening (drum replacement, a sampler keyed by the gate)" }),
            Some(Hot::MidiPanel) => return self.midi.help(host).or_else(|| self.midi.knob_help(host, self.pointer)).unwrap_or_default(),
            None => {}
        }
        if self.range.menu_open() {
            return Help::default().text("Level range — how many dB under full scale the history and the square show");
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
        if Self::detector_differs(host) {
            Help::default().text("Dashed white: the key as the detector hears it. Where it crosses the solid line the gate opens; under the dashed line it closes.")
        } else {
            Help::default().text("Where the input crosses the solid line the gate opens; under the dashed line it closes. Drag either grip.")
        }
    }

    // ---- events -----------------------------------------------------------------------------

    fn sync_knobs(&mut self, host: &dyn ParamHost) {
        let mode = Self::mode(host);
        if self.knobs_mode == Some(mode) || self.knobs.iter().any(Knob::is_dragging) {
            return;
        }
        let ratio_label = if mode == Mode::Expander { "RATIO" } else { "" };
        let dials: [(ParamSpec, &'static str, Option<mite_core::DialCurve>); 8] = [
            (RANGE, "RANGE", None),
            (RATIO, ratio_label, None),
            (HYSTERESIS, "HYSTERESIS", None),
            (LINK, "LINK", None),
            (ATTACK, "ATTACK", Some(ATTACK_DIAL)),
            (HOLD, "HOLD", Some(HOLD_DIAL)),
            (RELEASE, "RELEASE", Some(RELEASE_DIAL)),
            (LOOKAHEAD, "LOOKAHEAD", None),
        ];
        self.knobs = dials
            .into_iter()
            .enumerate()
            // The ratio only shapes the expander.
            .filter(|(i, _)| *i != 1 || mode == Mode::Expander)
            .map(|(i, (spec, label, curve))| {
                let k = Knob::new(spec, label, KnobKind::Unipolar, Point::new(DIAL_XS[i % 4], DIAL_ROWS[i / 4]), DIAL_D);
                match curve {
                    Some(c) => k.with_curve(c),
                    None => k,
                }
            })
            .collect();
        self.knobs_mode = Some(mode);
    }

    fn busy(&self) -> bool {
        self.drag.is_some() || self.key_eq.busy() || self.key_eq.menu_open() || self.knobs.iter().any(Knob::is_dragging) || self.footer.busy() || self.midi.busy()
    }

    fn hot_at(&self, host: &dyn ParamHost, p: Point) -> Option<Hot> {
        if self.eq_open && EQ_PANEL.contains(p) {
            return Some(Hot::EqPanel);
        }
        if EQ_BUTTON.contains(p) {
            return Some(Hot::EqButton);
        }
        if self.midi_open && MIDI_PANEL.contains(p) {
            return Some(Hot::MidiPanel);
        }
        if MIDI_BUTTON.contains(p) {
            return Some(Hot::MidiButton);
        }
        if Self::grip(lv_y(Self::plain(host, THRESHOLD.id))).inflate(3.0, 2.0).contains(p) {
            return Some(Hot::Threshold);
        }
        if Self::grip(lv_y(Self::close(host))).inflate(0.0, 1.0).contains(p) {
            return Some(Hot::Close);
        }
        if let Some(i) = (0..3).find(|&i| Self::mode_rect(i).contains(p)) {
            return Some(Hot::Mode(i));
        }
        if let Some(i) = (0..2).find(|&i| Self::detector_rect(i).contains(p)) {
            return Some(Hot::Detector(i));
        }
        if SOURCE_BUTTON.contains(p) {
            return Some(Hot::Source);
        }
        if sq_rect().contains(p) {
            let t = Self::plain(host, THRESHOLD.id);
            if Point::new(sq_x(t), sq_y(t)).distance(p) < NODE_HIT {
                return Some(Hot::Node);
            }
            return Some(Hot::Square);
        }
        None
    }

    fn key_event(&mut self, key: Key, mods: Modifiers, host: &dyn ParamHost) -> bool {
        if let Some(used) = self.session.key(key, mods, host) {
            return used;
        }
        match key {
            Key::Escape if self.eq_open || self.midi_open => {
                self.eq_open = false;
                self.midi_open = false;
                true
            }
            Key::Up | Key::Down => {
                let step = if mods.fine() { 0.1 } else { 1.0 } * if key == Key::Up { 1.0 } else { -1.0 };
                Self::set(host, THRESHOLD.id, (Self::plain(host, THRESHOLD.id) + step).clamp(THRESHOLD.min, THRESHOLD.max));
                true
            }
            _ => false,
        }
    }

    fn press(&mut self, host: &dyn ParamHost, pos: Point) -> bool {
        match self.hot_at(host, pos) {
            Some(Hot::Threshold) | Some(Hot::Node) => {
                let square = self.hot_at(host, pos) == Some(Hot::Node);
                host.begin_gesture(THRESHOLD.id);
                self.drag = Some(Drag::Threshold { start: pos, threshold: Self::plain(host, THRESHOLD.id), square });
                true
            }
            Some(Hot::Close) => {
                host.begin_gesture(HYSTERESIS.id);
                self.drag = Some(Drag::Close { start: pos, hysteresis: Self::plain(host, HYSTERESIS.id) });
                true
            }
            Some(Hot::Square) => {
                host.begin_gesture(RANGE.id);
                self.drag = Some(Drag::Range { start: pos, range: Self::plain(host, RANGE.id) });
                true
            }
            Some(Hot::Mode(i)) => {
                Self::set(host, MODE.id, i as f32);
                true
            }
            Some(Hot::Detector(i)) => {
                Self::set(host, DETECTOR.id, i as f32);
                true
            }
            Some(Hot::EqButton) => {
                self.eq_open = !self.eq_open;
                self.midi_open = false;
                true
            }
            Some(Hot::EqPanel) => true,
            Some(Hot::MidiButton) => {
                self.midi_open = !self.midi_open;
                self.eq_open = false;
                true
            }
            Some(Hot::MidiPanel) => true,
            Some(Hot::Source) => {
                let current = usize::from(Self::on(host, SC_SOURCE.id));
                self.source_menu = Some(Dropdown::new(SOURCE_BUTTON, SC_SOURCE.choices, current, (DISPLAY_TOP, FOOTER_TOP)).with_min_width(170.0));
                true
            }
            None => false,
        }
    }

    fn end_drag(&mut self, host: &dyn ParamHost) {
        match self.drag.take() {
            Some(Drag::Threshold { .. }) => host.end_gesture(THRESHOLD.id),
            Some(Drag::Close { .. }) => host.end_gesture(HYSTERESIS.id),
            Some(Drag::Range { .. }) => host.end_gesture(RANGE.id),
            None => {}
        }
    }

    fn handle_event(&mut self, event: &UiEvent, host: &dyn ParamHost) -> bool {
        if let UiEvent::KeyDown { key, mods } = *event {
            return self.key_event(key, mods, host);
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
            // The rest of the panel is the panel's own: nothing under it reacts.
            if let UiEvent::PointerDown { pos, .. } | UiEvent::DoubleClick { pos, .. } | UiEvent::Wheel { pos, .. } = *event {
                if in_panel(pos) {
                    return true;
                }
            }
        }
        if self.midi_open {
            let in_panel = |p: Point| MIDI_PANEL.contains(p);
            let ours = match *event {
                UiEvent::PointerDown { pos, .. } | UiEvent::DoubleClick { pos, .. } | UiEvent::Wheel { pos, .. } => in_panel(pos),
                UiEvent::PointerMove { pos, .. } => in_panel(pos) || self.midi.busy(),
                _ => true,
            };
            if ours && self.midi.event(event, host) {
                if let UiEvent::PointerMove { pos, .. } = *event {
                    self.pointer = Some(pos);
                    self.hot = Some(Hot::MidiPanel);
                }
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
        if self.range.event(event, self.range_shown) {
            return true;
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
                let k = if mods.fine() { 0.2 } else { 1.0 };
                match self.drag {
                    Some(Drag::Threshold { start, threshold, square }) => {
                        // On the history up and down; in the square along the diagonal (across).
                        let d = if square { ((pos.x - start.x) / SQ * level_range()) as f32 } else { ((start.y - pos.y) / px_per_db()) as f32 };
                        Self::set_raw(host, THRESHOLD.id, THRESHOLD.snap((threshold + d * k).clamp(THRESHOLD.min, THRESHOLD.max)));
                        return true;
                    }
                    Some(Drag::Close { start, hysteresis }) => {
                        let d = ((pos.y - start.y) / px_per_db()) as f32;
                        Self::set_raw(host, HYSTERESIS.id, HYSTERESIS.snap((hysteresis + d * k).clamp(HYSTERESIS.min, HYSTERESIS.max)));
                        return true;
                    }
                    Some(Drag::Range { start, range }) => {
                        let d = ((start.y - pos.y) / SQ * level_range()) as f32;
                        Self::set_raw(host, RANGE.id, RANGE.snap((range + d * k).clamp(RANGE.min, RANGE.max)));
                        return true;
                    }
                    None => {}
                }
                self.hot = self.hot_at(host, pos);
                true
            }
            UiEvent::PointerLeft => {
                self.pointer = None;
                self.hot = None;
                true
            }
            UiEvent::PointerDown { pos, button: MouseButton::Left, .. } => self.press(host, pos),
            UiEvent::PointerUp { button: MouseButton::Left, .. } => {
                let was = self.drag.is_some();
                self.end_drag(host);
                was
            }
            UiEvent::DoubleClick { pos, .. } => {
                let id = match self.hot_at(host, pos) {
                    Some(Hot::Threshold) | Some(Hot::Node) => Some((THRESHOLD.id, THRESHOLD.default)),
                    Some(Hot::Close) => Some((HYSTERESIS.id, HYSTERESIS.default)),
                    Some(Hot::Square) => Some((RANGE.id, RANGE.default)),
                    _ => None,
                };
                match id {
                    Some((id, v)) => {
                        self.end_drag(host);
                        Self::set(host, id, v);
                        true
                    }
                    None => false,
                }
            }
            UiEvent::Wheel { pos, lines, mods } => {
                let spec = match self.hot_at(host, pos) {
                    Some(Hot::Threshold) | Some(Hot::Node) => Some(THRESHOLD),
                    Some(Hot::Close) => Some(HYSTERESIS),
                    Some(Hot::Square) => Some(RANGE),
                    _ => None,
                };
                match spec {
                    Some(spec) => {
                        Self::set(host, spec.id, spec.wheel(Self::plain(host, spec.id), lines, mods.fine()));
                        true
                    }
                    None => false,
                }
            }
            _ => false,
        }
    }

    fn after_session(&mut self, _host: &dyn ParamHost) {
        if self.session.take_replaced() {
            self.knobs_mode = None;
        }
    }
}

fn draw_chevrons(cx: &mut DrawCx<'_>, c: Point, ink: Color) {
    let mut chev = BezPath::new();
    chev.move_to((c.x - 3.0, c.y - 2.0));
    chev.line_to((c.x, c.y - 5.0));
    chev.line_to((c.x + 3.0, c.y - 2.0));
    chev.move_to((c.x - 3.0, c.y + 2.0));
    chev.line_to((c.x, c.y + 5.0));
    chev.line_to((c.x + 3.0, c.y + 2.0));
    cx.scene.stroke(&Stroke::new(1.3).with_caps(Cap::Round).with_join(Join::Round), Affine::IDENTITY, ink, None, &chev);
}

fn mode_words(i: usize) -> &'static str {
    match i {
        0 => "opens over the threshold, closes under the dashed line; closed, the audio is turned down by the range",
        1 => "turns quieter parts down gradually (by the ratio), as far as the range",
        _ => "turns the audio down while the key is over the threshold: usually keyed from the sidechain",
    }
}

fn knob_words(id: &str) -> &'static str {
    match id {
        "range" => " — how far a closed gate turns the audio down (0 dB: nothing)",
        "ratio" => " — how steeply the expander turns quieter parts down",
        "hysteresis" => " — the gap between opening and closing, so a level hovering at the threshold does not chatter",
        "link" => " — how far the channels follow the louder one (100 %: they open together)",
        "attack" => " — how fast it opens",
        "hold" => " — how long it stays open after the key drops under the dashed line",
        "release" => " — how fast it closes",
        "lookahead" => " — opens this much before a transient (it is the latency)",
        _ => "",
    }
}

impl View for GateView {
    fn logical_size(&self) -> (f64, f64) {
        (WIDTH, HEIGHT)
    }

    fn draw(&mut self, cx: &mut DrawCx<'_>) {
        set_scale(self.range_shown, self.headroom);
        self.sync_knobs(cx.host);
        let theme = cx.theme;
        self.draw_header(cx);
        cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.display, None, &Rect::new(0.0, DISPLAY_TOP, WIDTH, HELP_TOP));
        // Nothing under the EQ panel glows through it.
        let hole = if self.eq_open { Some(EQ_PANEL) } else if self.midi_open { Some(MIDI_PANEL) } else { None };
        if let Some(r) = hole {
            cx.begin_glow_hole(RoundedRect::from_rect(r, 12.0));
        }
        self.draw_history(cx);
        if hole.is_some() {
            cx.end_glow_hole();
        }
        self.draw_square(cx);
        self.draw_strip(cx);
        self.draw_meters(cx);
        self.draw_states(cx);
        self.draw_eq_panel(cx);
        self.draw_midi_panel(cx);
        self.help_text(cx.host).draw(cx, Rect::new(0.0, HELP_TOP, WIDTH, FOOTER_TOP));
        self.draw_footer(cx);
        self.range.draw_menu(cx);
        self.key_eq.draw_menu(cx);
        self.session.draw_menu(cx);
    }

    fn event(&mut self, event: &UiEvent, host: &dyn ParamHost) -> bool {
        set_scale(self.range_shown, self.headroom);
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
        let dt = self.last_tick.elapsed().as_secs_f32().min(0.1);
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
        // The range: Auto fits the music (its quiet tenth) and the close threshold and range
        // with room; a picked range is kept, never narrower than the close threshold needs.
        let need = -Self::close(host) + 6.0;
        let mut levels: Vec<f32> = self.history.iter().rev().take(1200).map(|r| r[record::IN].max(r[record::IN + 1])).filter(|&l| l > -100.0).collect();
        let content = if levels.len() > 50 {
            levels.sort_by(f32::total_cmp);
            -levels[levels.len() / 10] + 3.0
        } else {
            60.0
        };
        let auto = content.max(need).clamp(24.0, 84.0);
        let fit = LEVEL_RANGES.iter().copied().find(|&v| v >= need).unwrap_or(LEVEL_RANGES[LEVEL_RANGES.len() - 1]);
        self.range_target = self.range.target(auto).max(fit);
        let moved = ease_range(&mut self.range_shown, self.range_target, dt);
        let head = history::headroom_db(&self.history, 1200, &[record::IN, record::IN + 1, record::OUT, record::OUT + 1]);
        let before = self.headroom;
        self.headroom += (head - self.headroom) * (1.0 - (-dt / 0.12).exp());
        if (self.headroom - head).abs() < 0.01 {
            self.headroom = head;
        }
        set_scale(self.range_shown, self.headroom);
        if let Some(r) = self.history.back().copied() {
            let targets = [r[record::IN], r[record::IN + 1], r[record::GAIN], r[record::GAIN + 1], r[record::OUT], r[record::OUT + 1]];
            for (i, (m, t)) in self.meters.iter_mut().zip(targets).enumerate() {
                let fall = METER_RELEASE_DB_S * dt;
                *m = if i == 2 || i == 3 { if t < *m { t } else { (*m + fall).min(t) } } else if t > *m { t } else { (*m - fall).max(t) };
            }
        }
        let spectrum = self.eq_open && self.key_spectrum.update(&self.shared.key_spectrum, f64::from(self.shared.sample_rate.load()), f64::from(dt));
        let flowing = self.last_fresh.elapsed().as_secs_f32() < 0.25 || self.midi_open && self.note_flash().is_some_and(|(_, f)| f > 0.0);
        fresh || flowing || spectrum || moved || busy || self.headroom != before || self.meters.iter().enumerate().any(|(i, &m)| if i == 2 || i == 3 { m < -0.01 } else { m > SILENT + 1.0 })
    }
}
