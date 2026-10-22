//! The Mite EQ editor (docs/design/mockups/eq-main.dc.html), laid out in the mockup's 1280×760
//! design units and shown at [`ZOOM`].
//!
//! Gestures follow docs/design/interaction.md, "Main display gestures".

use std::f64::consts::LN_2;
use std::sync::Arc;
use std::time::Instant;

use mite_core::{ParamDisplay, ParamScale, ParamSpec, Registry};
use mite_dsp::analyzer::{spectrum_point_freq, spectrum_point_oct, Analyzer, LogSpectrum, SPECTRUM_POINTS};
use mite_dsp::matching::{self, Collision, LongTermSpectrum};
use mite_dsp::eq::{analog_response_db, design, BandSettings, BandType, DetectSource, Dynamics, DynamicsMode, Placement, ShapeDetector, GAIN_LIMIT_DB, Sections, FREQ_MAX, FREQ_MIN};
use mite_ui::kurbo::{Affine, BezPath, Circle, Line, Point, Rect, RoundedRect, Stroke};
use mite_ui::paint::{glass_panel, glow_stroke, icon_button, panel, raised};
use mite_ui::peniko::{Color, Fill, Gradient};
use mite_ui::widgets::brand::{draw_mark, draw_title};
use mite_ui::session::{Session, SessionConfig};
use mite_ui::widgets::footer::draw_dropdown_button;
use mite_ui::widgets::{Dropdown, DropdownEvent, Footer, FooterDial, FooterToggle, Help, Knob, KnobKind, LevelMeter, ToggleGlyph};
use mite_ui::{Align, DrawCx, Key, MouseButton, ParamHost, RecordingHost, TextStyle, UiEvent, View};

use crate::dsp::{PhaseMode, LP_RESOLUTIONS};
use crate::params::{
    band_id, ATTACK, ATTACK_DIAL, AUTO_GAIN, BANDS, BYPASS, DELTA, DETECT_NAMES, DMODE, DSRC, DYN, FREQ, GAIN, LP_RES, MIX, MODE, ON, OUT_GAIN, OUT_PAN, Q, RANGE, RATIO, RELEASE,
    RELEASE_DIAL, RTHRESH, SLOPE, SOLO, SPECTRAL, SREL, SSMOOTH, STEREO, SUSTAIN, THRESH, TRANSIENT, TRANSITION, TTIME, TYPE,
};
use crate::params::snapshot_ids;
use crate::presets::{preset_ids, FACTORY, PLUGIN};
use crate::{EqShared, INSTANCES, SPECTRAL_POINTS};

/// A stand-in host holding every Mite EQ parameter at its default, for tests, the standalone
/// preview and benchmarks.
pub fn memory_host() -> mite_ui::MemoryParamHost {
    use mite_core::{BoolSpec, ChoiceSpec, ParamDisplay, ParamScale, ParamSpec};
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
    let choice = |c: &ChoiceSpec| index(c.id, c.choices.len(), c.default);
    let boolean = |b: &BoolSpec| index(b.id, 2, usize::from(b.default));
    let mut host = mite_ui::MemoryParamHost::new()
        .with(OUT_GAIN)
        .with(choice(&MODE))
        .with(choice(&LP_RES))
        .with(OUT_PAN)
        .with(MIX)
        .with(boolean(&AUTO_GAIN))
        .with(boolean(&DELTA))
        .with(boolean(&BYPASS));
    for b in 0..BANDS {
        let id = |param: &str| leak(band_id(b, param));
        host = host
            .with_id(id(ON.id), boolean(&ON))
            .with_id(id(TYPE.id), choice(&TYPE))
            .with_id(id(FREQ.id), FREQ)
            .with_id(id(GAIN.id), GAIN)
            .with_id(id(Q.id), Q)
            .with_id(id(SLOPE.id), choice(&SLOPE))
            .with_id(id(STEREO.id), choice(&STEREO))
            .with_id(id(DYN.id), boolean(&DYN))
            .with_id(id(RANGE.id), RANGE)
            .with_id(id(THRESH.id), THRESH)
            .with_id(id(ATTACK.id), ATTACK)
            .with_id(id(RELEASE.id), RELEASE)
            .with_id(id(RATIO.id), RATIO)
            .with_id(id(DMODE.id), choice(&DMODE))
            .with_id(id(SUSTAIN.id), SUSTAIN)
            .with_id(id(TRANSIENT.id), TRANSIENT)
            .with_id(id(TTIME.id), TTIME)
            .with_id(id(TRANSITION.id), TRANSITION)
            .with_id(id(SPECTRAL.id), boolean(&SPECTRAL))
            .with_id(id(SSMOOTH.id), SSMOOTH)
            .with_id(id(SREL.id), boolean(&SREL))
            .with_id(id(RTHRESH.id), RTHRESH)
            .with_id(id(SOLO.id), boolean(&SOLO))
            .with_id(id(DSRC.id), choice(&DSRC));
    }
    host
}

/// Mite EQ wears the EQ accent (tokens.json `color.accent.eq`).
pub const ACCENT: &str = "eq";
pub use mite_dsp::analyzer::ANALYZER_SIZE;
/// Default UI scale. The mockup is 1280×760; 75 % suits most screens.
pub const ZOOM: f64 = 1.0;

const WIDTH: f64 = 1280.0;
const HEIGHT: f64 = 760.0;
const HEADER: f64 = 56.0;
const DISPLAY_TOP: f64 = HEADER;
const DISPLAY_H: f64 = 620.0;
const HELP_TOP: f64 = DISPLAY_TOP + DISPLAY_H;
const FOOTER_TOP: f64 = HELP_TOP + 28.0;

/// Plot geometry inside the display (mockup: W 1200, TOP 24, BOT 588, MID 306).
const PLOT_W: f64 = 1200.0;
const PLOT_TOP: f64 = DISPLAY_TOP + 24.0;
const PLOT_BOT: f64 = DISPLAY_TOP + 588.0;
const MID: f64 = DISPLAY_TOP + 306.0;
const PX_PER_DB: f64 = 264.0 / 18.0;
/// Curve and spectrum resolution: one point every 4 design px (the analyzer's points).
const POINTS: usize = SPECTRUM_POINTS;

const NODE_HIT: f64 = 16.0;
const PANEL_W: f64 = 700.0;
const PANEL_H: f64 = 104.0;
/// Extra height of the dynamics row (dynamic bands only).
const PANEL_DYN_H: f64 = 104.0;
/// Panel dials: diameter, and the x of the rightmost dial's centre relative to the panel (clear
/// of the close and remove buttons).
const PANEL_DIAL: f64 = 56.0;
const PANEL_LAST_DIAL_X: f64 = 596.0;
const PANEL_DIAL_STEP: f64 = 84.0;
/// The header row the controls sit in (the session's presets, undo and snapshots take the left
/// part; see `mite_ui::session::HeaderLayout`).
const HEADER_ROW: (f64, f64) = (12.0, 44.0);

/// The gain dial's dynamics ring: its radius, and the radial band that grabs it.
const RING_R: f64 = PANEL_DIAL / 2.0 + 7.0;
const RING_HIT: (f64, f64) = (PANEL_DIAL / 2.0 + 1.0, PANEL_DIAL / 2.0 + 14.0);
/// dB of dynamic range per px dragged on the ring or the dynamics scope.
const RING_DB_PER_PX: f64 = 0.1;
/// Below this |range| a band is static: pulling the range back to the node turns dynamics off.
const RANGE_OFF_DB: f64 = 0.5;
/// Distance of the "make dynamic" grips from a static node, and their hit radius.
const GRIP_OFFSET: f64 = 28.0;
const GRIP_HIT: f64 = 11.0;
/// The dynamics scope in the panel: size, and the quietest level it shows (dBFS).
const SCOPE_W: f64 = 200.0;
const SCOPE_H: f64 = 86.0;
const SCOPE_FLOOR_DB: f64 = -60.0;
/// Transient scope: the simulated hit's silence before the onset and its length, s, and the inner
/// padding. Time runs on a square-root axis (see [`scope_x`]).
const SCOPE_PRE_S: f64 = 0.005;
const SCOPE_LEN_S: f64 = 0.6;
const SCOPE_PAD: f64 = 8.0;

/// x in the transient scope `r` of time `t` s after the hit starts. Square-root time: the first
/// tens of milliseconds, where the attack acts, get room to show their shape.
fn scope_x(r: Rect, t: f64) -> f64 {
    r.x0 + SCOPE_PAD + ((t + SCOPE_PRE_S) / SCOPE_LEN_S).clamp(0.0, 1.0).sqrt() * (r.width() - 2.0 * SCOPE_PAD)
}

/// The time after the hit starts, s, at x in the transient scope `r` (the inverse of [`scope_x`]).
fn scope_time(r: Rect, x: f64) -> f64 {
    let u = ((x - r.x0 - SCOPE_PAD) / (r.width() - 2.0 * SCOPE_PAD)).clamp(0.0, 1.0);
    u * u * SCOPE_LEN_S - SCOPE_PRE_S
}
/// dB per px dragged on the transient scope.
const SCOPE_DB_PER_PX: f64 = 0.15;
/// Gap between the band panel and the plot's edges.
const PANEL_MARGIN: f64 = 14.0;

fn fx(f: f64) -> f64 {
    PLOT_W * (f / FREQ_MIN).ln() / (FREQ_MAX / FREQ_MIN).ln()
}

fn xf(x: f64) -> f64 {
    FREQ_MIN * ((x / PLOT_W) * (FREQ_MAX / FREQ_MIN).ln()).exp()
}

fn gy(db: f64) -> f64 {
    MID - db * PX_PER_DB
}

fn yg(y: f64) -> f64 {
    (MID - y) / PX_PER_DB
}

/// Spectrum: 0 dBFS at the top of the plot, −90 at the bottom.
fn sy(db: f64) -> f64 {
    PLOT_TOP + (-db / 90.0) * (PLOT_BOT - PLOT_TOP)
}

fn note_name(freq: f64) -> String {
    const NAMES: [&str; 12] = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];
    let midi = 69.0 + 12.0 * (freq / 440.0).log2();
    let n = midi.round();
    let cents = ((midi - n) * 100.0).round() as i32;
    let idx = (n as i32).rem_euclid(12) as usize;
    let octave = (n as i32).div_euclid(12) - 1;
    format!("{}{} {}{} ct", NAMES[idx], octave, if cents < 0 { "−" } else { "+" }, cents.abs())
}

/// Host ids for one band, built once.
struct BandIds {
    on: String,
    kind: String,
    freq: String,
    gain: String,
    q: String,
    slope: String,
    stereo: String,
    dyn_on: String,
    range: String,
    thresh: String,
    attack: String,
    release: String,
    ratio: String,
    dmode: String,
    sustain: String,
    transient: String,
    ttime: String,
    transition: String,
    spectral: String,
    srel: String,
    rthresh: String,
    ssmooth: String,
    solo: String,
    dsrc: String,
}

impl BandIds {
    fn new(b: usize) -> Self {
        Self {
            on: band_id(b, ON.id),
            kind: band_id(b, TYPE.id),
            freq: band_id(b, FREQ.id),
            gain: band_id(b, GAIN.id),
            q: band_id(b, Q.id),
            slope: band_id(b, SLOPE.id),
            stereo: band_id(b, STEREO.id),
            dyn_on: band_id(b, DYN.id),
            range: band_id(b, RANGE.id),
            thresh: band_id(b, THRESH.id),
            attack: band_id(b, ATTACK.id),
            release: band_id(b, RELEASE.id),
            ratio: band_id(b, RATIO.id),
            dmode: band_id(b, DMODE.id),
            sustain: band_id(b, SUSTAIN.id),
            transient: band_id(b, TRANSIENT.id),
            ttime: band_id(b, TTIME.id),
            transition: band_id(b, TRANSITION.id),
            spectral: band_id(b, SPECTRAL.id),
            srel: band_id(b, SREL.id),
            rthresh: band_id(b, RTHRESH.id),
            ssmooth: band_id(b, SSMOOTH.id),
            solo: band_id(b, SOLO.id),
            dsrc: band_id(b, DSRC.id),
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum DragMode {
    Move,
    Width,
    /// A plot grip: the range follows the pointer's height above or below the node.
    Range,
    /// The gain dial's ring: right lifts the band when loud, left ducks it.
    Ring,
    /// The compressor scope: sideways sets the threshold, up and down the range.
    Scope,
    /// The transient scope's attack: up and down set the transient amount.
    TransientAttack,
    /// The transient scope's tail: up and down set the sustain.
    Sustain,
    /// The transient scope's handover line: sideways sets the transient time.
    Split,
    /// The spectral compressor's scope: the threshold line follows the pointer up and down.
    SpectralThreshold,
}

struct NodeDrag {
    band: usize,
    mode: DragMode,
    last: Point,
    log_freq: f64,
    gain: f64,
    log_q: f64,
    range: f64,
    thresh: f64,
    sustain: f64,
    transient: f64,
    /// Relative spectral threshold, dB over the surroundings.
    relative: f64,
}

/// Live dynamic gains and levels on the plot follow the engine through two eased stages of this
/// time constant each, s. One stage alone turns block-rate telemetry into a ramp that restarts at
/// every block (speed jumps); two make the speed change smoothly as well.
const LIVE_EASE_S: f64 = 0.012;

/// Spectrum grab (interaction.md, "Spectrum peak"): a peak counts when it stands this far above
/// the lowest point within a third of an octave on both sides, dB; the pointer finds it within
/// this distance, px; a grabbed or double-clicked peak becomes a Bell of this Q, and a
/// double-click cuts it by this much, dB. The marker fades with this time constant, s.
const PEAK_PROMINENCE_DB: f64 = 3.0;
const PEAK_REACH: f64 = 18.0;
/// How far above the spectrum line still counts as under it, px (the line's own width and a
/// little give).
const SPECTRUM_EDGE: f64 = 4.0;
const PEAK_Q: f64 = 3.0;
const PEAK_CUT_DB: f64 = -3.0;
const PEAK_FADE_S: f64 = 0.05;
/// Peak hold (Pro-Q's spectrum grab): after the pointer has rested under the spectrum this long, s,
/// the spectrum's maxima accumulate into a held curve, which fades in and out with this time
/// constant, s.
const HOLD_AFTER_S: f64 = 0.3;
const HOLD_FADE_S: f64 = 0.08;

/// A spectrum peak: its frequency and level (dB on the analyzer's scale).
#[derive(Clone, Copy, PartialEq, Debug)]
struct Peak {
    freq: f64,
    db: f64,
}

/// A peak being dragged: the Bell it will become (a ghost until release). It stays on the
/// peak's frequency; the drag sets only its gain.
struct Grab {
    freq: f64,
    gain: f64,
    start: Point,
    last: Point,
    moved: bool,
}

/// Masking (BLUEPRINT §8.3): the averages of this track and the sidechain it compares, s.
const MASK_AVERAGE_S: f64 = 3.0;
/// Where a spectrum counts as heard, dB as drawn.
const HEARD_DB: f64 = -80.0;
/// How often collisions are looked for, s, and how fast they fade in and out.
const MASK_UPDATE_S: f64 = 0.1;
const MASK_FADE_S: f64 = 0.25;
/// The ring a carve suggestion is drawn as, and its reach; the ring that carves on the other
/// instance sits this far to its right.
const CARVE_RING: f64 = 10.0;
const CARVE_THERE_DX: f64 = 2.0 * CARVE_RING + 12.0;
/// Other instances are listed this often, and analysed every this many of this track's
/// analyses (their 3 s average needs no more).
const PEERS_EVERY_S: f64 = 1.0;
const PEER_ANALYSIS_EVERY: u32 = 4;

/// What this track's masking is checked against.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Other {
    Sidechain,
    /// Another Mite EQ, by its number in the registry.
    Instance(u32),
}

impl Other {
    fn name(self) -> String {
        match self {
            Self::Sidechain => "the sidechain".to_owned(),
            Self::Instance(n) => format!("EQ {n}"),
        }
    }
}

/// One thing masking compares this track with: its latest analysis, whether that was heard,
/// its 3 s average, and the score against this track.
struct MaskSource {
    other: Other,
    /// The other instance (`None` for the sidechain); weak, so it can go away.
    shared: Option<std::sync::Weak<EqShared>>,
    latest: Vec<f32>,
    heard: bool,
    /// Whether `latest` is new since it was last averaged.
    fresh: bool,
    average: LongTermSpectrum,
    score: Vec<f64>,
}

impl MaskSource {
    fn new(other: Other, shared: Option<std::sync::Weak<EqShared>>) -> Self {
        Self {
            other,
            shared,
            latest: vec![-150.0; POINTS],
            heard: false,
            fresh: false,
            average: LongTermSpectrum::new(POINTS, MASK_AVERAGE_S),
            score: vec![f64::NEG_INFINITY; POINTS],
        }
    }
}

/// EQ Match: the header button, how often a learning fit is refreshed (s), and how much audio
/// fills the learning ring (s).
const MATCH_X: (f64, f64) = (790.0, 822.0);
const MATCH_REFIT_S: f64 = 0.5;
const MATCH_FULL_S: f64 = 10.0;

/// EQ Match's dials (the editor's own, not plugin parameters): how far to go towards the
/// reference, how many bands it may add, and how finely it follows the difference (octaves).
const MATCH_AMOUNT: ParamSpec = ParamSpec {
    id: "match_amount",
    name: "Amount",
    unit: "%",
    min: 0.0,
    max: 100.0,
    default: 100.0,
    scale: ParamScale::Linear,
    smoothing_ms: 0.0,
    decimals: 0,
    wheel_step: 5.0,
    wheel_step_fine: 1.0,
    display: ParamDisplay::Number,
};
const MATCH_BANDS: ParamSpec =
    ParamSpec { id: "match_bands", name: "Bands", unit: "", min: 1.0, max: BANDS as f32, default: 8.0, wheel_step: 1.0, wheel_step_fine: 1.0, ..MATCH_AMOUNT };
const MATCH_SMOOTH: ParamSpec = ParamSpec {
    id: "match_smooth",
    name: "Smooth",
    unit: " oct",
    min: 0.08,
    max: 1.0,
    default: 1.0 / 3.0,
    scale: ParamScale::Log,
    decimals: 2,
    wheel_step: 0.05,
    wheel_step_fine: 0.01,
    ..MATCH_AMOUNT
};

/// The Match panel's buttons, set while drawing.
#[derive(Clone, Copy)]
struct MatchHits {
    panel: Rect,
    learn: Rect,
    restart: Rect,
    file: Rect,
    /// The × after a reference file's name.
    drop: Option<Rect>,
    apply: Rect,
    close: Rect,
}

/// A reference file being read on a worker: its name, and its spectrum or why not.
type PendingFile = (String, std::sync::mpsc::Receiver<Result<Vec<f64>, String>>);

/// A fit running on a worker, and the aim it was given.
type PendingFit = (std::sync::mpsc::Receiver<Vec<BandSettings>>, Vec<Option<f64>>);

/// EQ Match (BLUEPRINT §8.3): this track before the EQ and the reference (the sidechain),
/// averaged while learning, and the bands fitted to what the current bands leave.
struct Match {
    source: LongTermSpectrum,
    reference: LongTermSpectrum,
    learning: bool,
    controls: mite_ui::MemoryParamHost,
    knobs: Vec<Knob>,
    /// The whole EQ the fit aims for (dB per grid point, only where the match listens), and the
    /// fitted bands' curve (empty before the first fit).
    aim: Vec<Option<f64>>,
    ghost: Vec<f64>,
    fitted: Vec<BandSettings>,
    /// A fit running on a worker, the aim it was given, and whether the inputs changed since.
    pending: Option<PendingFit>,
    stale: bool,
    last_fit: Instant,
    /// The dials' values the last fit used.
    inputs: [f32; 3],
    hits: Option<MatchHits>,
    /// A reference file instead of the sidechain (its name and long-term spectrum), a file
    /// being picked or read, and why the last one could not be read.
    file: Option<(String, Vec<f64>)>,
    picking: Option<std::sync::mpsc::Receiver<Option<std::path::PathBuf>>>,
    loading: Option<PendingFile>,
    error: Option<String>,
}

impl Match {
    fn new() -> Self {
        let controls = mite_ui::MemoryParamHost::new().with(MATCH_AMOUNT).with(MATCH_BANDS).with(MATCH_SMOOTH);
        let o = Self::rect().origin();
        let first = o.x + PANEL_LAST_DIAL_X - 2.0 * PANEL_DIAL_STEP;
        let knobs = [(MATCH_AMOUNT, "AMOUNT"), (MATCH_BANDS, "BANDS"), (MATCH_SMOOTH, "SMOOTH")]
            .into_iter()
            .enumerate()
            .map(|(i, (spec, label))| Knob::new(spec, label, KnobKind::Unipolar, Point::new(first + i as f64 * PANEL_DIAL_STEP, o.y + 44.0), PANEL_DIAL))
            .collect();
        Self {
            source: LongTermSpectrum::new(POINTS, f64::INFINITY),
            reference: LongTermSpectrum::new(POINTS, f64::INFINITY),
            learning: true,
            controls,
            knobs,
            aim: Vec::new(),
            ghost: Vec::new(),
            fitted: Vec::new(),
            pending: None,
            stale: false,
            last_fit: Instant::now(),
            inputs: [f32::NAN; 3],
            hits: None,
            file: None,
            picking: None,
            loading: None,
            error: None,
        }
    }

    /// Seconds learned: of this track against a file, of both against the sidechain.
    fn learned_s(&self) -> f64 {
        if self.file.is_some() { self.source.seconds() } else { self.reference.seconds() }
    }

    /// Starts reading `path` as the reference, on a worker.
    fn load(&mut self, path: std::path::PathBuf) {
        let name = path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned());
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(crate::reference::analyze(&path));
        });
        self.loading = Some((name, rx));
        self.error = None;
    }

    /// The fit starts again from nothing (the reference changed).
    fn refit(&mut self) {
        self.fitted.clear();
        self.aim.clear();
        self.ghost.clear();
        self.inputs = [f32::NAN; 3];
    }

    /// Where the panel sits: the band panel's place, docked at the bottom centre of the plot.
    fn rect() -> Rect {
        let x = (PLOT_W - PANEL_W) / 2.0;
        Rect::new(x, HELP_TOP - PANEL_H - 10.0, x + PANEL_W, HELP_TOP - 10.0)
    }

    fn inputs(&self) -> [f32; 3] {
        [MATCH_AMOUNT, MATCH_BANDS, MATCH_SMOOTH].map(|spec| self.controls.plain(spec.id, self.controls.normalized(spec.id)))
    }

    fn restart(&mut self) {
        self.source.clear();
        self.reference.clear();
        self.aim.clear();
        self.ghost.clear();
        self.fitted.clear();
        self.learning = true;
        self.stale = false;
    }
}

/// The analyzer's points, Hz (the plot's grid points sit on them).
fn grid_freqs() -> Vec<f64> {
    (0..POINTS).map(spectrum_point_freq).collect()
}

/// Analyzer motion: time constants of the rise and of the fall, s.
const SPECTRUM_RISE_S: f64 = 0.06;
const SPECTRUM_FALL_S: f64 = 0.3;

struct BandResponse {
    settings: BandSettings,
    fs: f64,
    linear: bool,
    sections: Sections,
    grid: Vec<f64>,
}

/// x of display grid point `i`.
fn grid_x(i: usize) -> f64 {
    i as f64 * PLOT_W / (POINTS - 1) as f64
}

/// A band's response in dB at `f`, as drawn: its analog prototype, which is what every mode
/// plays (Zero Latency through its minimum-phase correction, Natural Phase through its complex
/// one, Linear Phase by design), above half Nyquist included; `linear`: as Linear Phase plays it
/// (a Brickwall edge). The sections stay in the signature for the cache.
fn band_db(_sections: &Sections, s: &BandSettings, f: f64, _fs: f64, linear: bool) -> f64 {
    // A Brickwall cut is a vertical edge only in Linear Phase; the IIR modes play 96 dB/oct.
    let db = if linear { mite_dsp::eq::linear_phase_response_db(s, f) } else { analog_response_db(s, f) };
    if db.is_nan() { -300.0 } else { db.max(-300.0) }
}

/// Draws a band type's response shape as a 22 × 12 icon with its top-left at `at`.
fn draw_type_icon(scene: &mut mite_ui::vello::Scene, kind: BandType, at: Point, color: Color) {
    let mut p = BezPath::new();
    let pt = |x: f64, y: f64| (at.x + x, at.y + y);
    match kind {
        BandType::Bell => {
            p.move_to(pt(0.0, 9.0));
            p.line_to(pt(5.0, 9.0));
            p.curve_to(pt(8.0, 9.0), pt(9.0, 2.0), pt(11.0, 2.0));
            p.curve_to(pt(13.0, 2.0), pt(14.0, 9.0), pt(17.0, 9.0));
            p.line_to(pt(22.0, 9.0));
        }
        BandType::LowShelf => {
            p.move_to(pt(0.0, 3.0));
            p.line_to(pt(6.0, 3.0));
            p.curve_to(pt(10.0, 3.0), pt(12.0, 9.0), pt(16.0, 9.0));
            p.line_to(pt(22.0, 9.0));
        }
        BandType::HighShelf => {
            p.move_to(pt(0.0, 9.0));
            p.line_to(pt(6.0, 9.0));
            p.curve_to(pt(10.0, 9.0), pt(12.0, 3.0), pt(16.0, 3.0));
            p.line_to(pt(22.0, 3.0));
        }
        BandType::LowCut => {
            p.move_to(pt(2.0, 12.0));
            p.curve_to(pt(5.0, 4.0), pt(7.0, 3.0), pt(12.0, 3.0));
            p.line_to(pt(22.0, 3.0));
        }
        BandType::HighCut => {
            p.move_to(pt(0.0, 3.0));
            p.line_to(pt(10.0, 3.0));
            p.curve_to(pt(15.0, 3.0), pt(17.0, 4.0), pt(20.0, 12.0));
        }
        BandType::Notch => {
            p.move_to(pt(0.0, 3.0));
            p.line_to(pt(7.0, 3.0));
            p.curve_to(pt(10.0, 3.0), pt(10.5, 12.0), pt(11.0, 12.0));
            p.curve_to(pt(11.5, 12.0), pt(12.0, 3.0), pt(15.0, 3.0));
            p.line_to(pt(22.0, 3.0));
        }
        BandType::BandPass => {
            p.move_to(pt(1.0, 12.0));
            p.curve_to(pt(6.0, 12.0), pt(8.0, 2.0), pt(11.0, 2.0));
            p.curve_to(pt(14.0, 2.0), pt(16.0, 12.0), pt(21.0, 12.0));
        }
        BandType::TiltShelf => {
            p.move_to(pt(0.0, 10.0));
            p.curve_to(pt(9.0, 10.0), pt(13.0, 2.0), pt(22.0, 2.0));
        }
        BandType::FlatTilt => {
            p.move_to(pt(0.0, 11.0));
            p.line_to(pt(22.0, 1.0));
        }
        BandType::AllPass => {
            // A flat level, with the phase turning: a circular arrow over it.
            p.move_to(pt(0.0, 11.0));
            p.line_to(pt(22.0, 11.0));
            let (c, r, start, sweep) = (Point::new(at.x + 11.0, at.y + 4.5), 4.0, 0.5f64, 4.6f64);
            p.move_to(c + (r * start.cos(), r * start.sin()));
            p.extend(mite_ui::kurbo::Arc::new(c, (r, r), start, sweep, 0.0).append_iter(0.05));
            // The arrowhead follows the arc's direction at its end.
            let e = start + sweep;
            let tip = c + (r * e.cos(), r * e.sin());
            let dir = Point::new(-e.sin(), e.cos()).to_vec2();
            let back = |turn: f64| {
                let (sn, cs) = turn.sin_cos();
                tip - mite_ui::kurbo::Vec2::new(dir.x * cs - dir.y * sn, dir.x * sn + dir.y * cs) * 2.8
            };
            p.move_to(back(0.6));
            p.line_to(tip);
            p.line_to(back(-0.6));
        }
    }
    let stroke = Stroke::new(1.6).with_caps(mite_ui::kurbo::Cap::Round).with_join(mite_ui::kurbo::Join::Round);
    scene.stroke(&stroke, Affine::IDENTITY, color, None, &p);
}

/// Draws a cut's steepness for slope choice `i` of `n` as a 22 × 12 icon: steeper is more upright.
fn draw_slope_icon(scene: &mut mite_ui::vello::Scene, kind: BandType, i: usize, n: usize, at: Point, color: Color) {
    let t = i as f64 / (n.max(2) - 1) as f64;
    let run = 12.0 - 11.0 * t.sqrt();
    // Drawn as a low cut; a high cut is its mirror image.
    let mirror = kind == BandType::HighCut;
    let pt = |x: f64, y: f64| (at.x + if mirror { 22.0 - x } else { x }, at.y + y);
    let knee = 13.0;
    let mut p = BezPath::new();
    p.move_to(pt(knee - run, 12.0));
    p.quad_to(pt(knee - 0.1 * run, 3.0), pt(knee + 2.0, 3.0));
    p.line_to(pt(22.0, 3.0));
    let stroke = Stroke::new(1.6).with_caps(mite_ui::kurbo::Cap::Round).with_join(mite_ui::kurbo::Join::Round);
    scene.stroke(&stroke, Affine::IDENTITY, color, None, &p);
}

/// Draws a stereo placement as a 22 × 12 icon: two overlapping circles for the left and right
/// channels, filled where the band works (both, one, their shared middle or their sides).
fn draw_placement_icon(scene: &mut mite_ui::vello::Scene, placement: Placement, at: Point, color: Color) {
    let (l, r) = (Circle::new((at.x + 7.5, at.y + 6.0), 5.0), Circle::new((at.x + 14.5, at.y + 6.0), 5.0));
    let fill = color.multiply_alpha(0.9);
    let both = |rule| {
        let mut p = BezPath::new();
        p.extend(mite_ui::kurbo::Shape::path_elements(&l, 0.1));
        p.extend(mite_ui::kurbo::Shape::path_elements(&r, 0.1));
        (rule, p)
    };
    match placement {
        Placement::Stereo => {
            let (rule, p) = both(Fill::NonZero);
            scene.fill(rule, Affine::IDENTITY, fill, None, &p);
        }
        Placement::Left => scene.fill(Fill::NonZero, Affine::IDENTITY, fill, None, &l),
        Placement::Right => scene.fill(Fill::NonZero, Affine::IDENTITY, fill, None, &r),
        Placement::Mid => {
            scene.push_clip_layer(Fill::NonZero, Affine::IDENTITY, &l);
            scene.fill(Fill::NonZero, Affine::IDENTITY, fill, None, &r);
            scene.pop_layer();
        }
        Placement::Side => {
            let (rule, p) = both(Fill::EvenOdd);
            scene.fill(rule, Affine::IDENTITY, fill, None, &p);
        }
    }
    let stroke = Stroke::new(1.2);
    scene.stroke(&stroke, Affine::IDENTITY, color.multiply_alpha(0.7), None, &l);
    scene.stroke(&stroke, Affine::IDENTITY, color.multiply_alpha(0.7), None, &r);
}

/// A dropdown's icon.
#[derive(Clone, Copy)]
enum Icon {
    Type(BandType),
    /// A cut type and its slope index.
    Slope(BandType, usize),
    Stereo(Placement),
    /// A linear-phase resolution: ascending bars, as many lit as the level.
    Resolution(usize),
    /// A detection source.
    Source(DetectSource),
}

/// EQ Match centred on `c`: a curve and the dashed one it follows.
fn draw_match_glyph(scene: &mut mite_ui::vello::Scene, c: Point, color: Color) {
    let wave = |dy: f64| {
        let mut p = BezPath::new();
        p.move_to((c.x - 8.0, c.y + dy));
        p.curve_to((c.x - 3.0, c.y + dy - 7.0), (c.x + 3.0, c.y + dy + 7.0), (c.x + 8.0, c.y + dy));
        p
    };
    let round = Stroke::new(1.5).with_caps(mite_ui::kurbo::Cap::Round);
    scene.stroke(&round.clone().with_dashes(0.0, [2.0, 2.5]), Affine::IDENTITY, color.multiply_alpha(0.7), None, &wave(-2.5));
    scene.stroke(&round, Affine::IDENTITY, color, None, &wave(2.5));
}

/// A bin centred on `c`: removes the band.
fn draw_bin(scene: &mut mite_ui::vello::Scene, c: Point, color: Color) {
    let round = Stroke::new(1.4).with_caps(mite_ui::kurbo::Cap::Round).with_join(mite_ui::kurbo::Join::Round);
    let mut bin = BezPath::new();
    // Lid and handle.
    bin.move_to((c.x - 6.5, c.y - 4.0));
    bin.line_to((c.x + 6.5, c.y - 4.0));
    bin.move_to((c.x - 2.0, c.y - 4.0));
    bin.line_to((c.x - 2.0, c.y - 6.0));
    bin.line_to((c.x + 2.0, c.y - 6.0));
    bin.line_to((c.x + 2.0, c.y - 4.0));
    // Body, narrowing to the bottom, with two ribs.
    bin.move_to((c.x - 5.0, c.y - 2.0));
    bin.line_to((c.x - 4.0, c.y + 6.5));
    bin.line_to((c.x + 4.0, c.y + 6.5));
    bin.line_to((c.x + 5.0, c.y - 2.0));
    bin.move_to((c.x - 1.5, c.y));
    bin.line_to((c.x - 1.3, c.y + 4.0));
    bin.move_to((c.x + 1.5, c.y));
    bin.line_to((c.x + 1.3, c.y + 4.0));
    scene.stroke(&round, Affine::IDENTITY, color, None, &bin);
}

/// A detection source as a 22 × 12 icon at `at`: the band's own bump, a sidechain plug coming
/// in, or another band (a ring).
fn draw_source_icon(scene: &mut mite_ui::vello::Scene, source: DetectSource, at: Point, color: Color) {
    let round = Stroke::new(1.4).with_caps(mite_ui::kurbo::Cap::Round).with_join(mite_ui::kurbo::Join::Round);
    match source {
        DetectSource::Own => draw_type_icon(scene, BandType::Bell, at, color),
        DetectSource::Sidechain => {
            let mut plug = BezPath::new();
            plug.move_to((at.x + 1.0, at.y + 6.0));
            plug.line_to((at.x + 12.0, at.y + 6.0));
            plug.move_to((at.x + 8.0, at.y + 2.5));
            plug.line_to((at.x + 12.0, at.y + 6.0));
            plug.line_to((at.x + 8.0, at.y + 9.5));
            scene.stroke(&round, Affine::IDENTITY, color, None, &plug);
            scene.stroke(&round, Affine::IDENTITY, color, None, &Line::new((at.x + 16.0, at.y + 1.0), (at.x + 16.0, at.y + 11.0)));
        }
        DetectSource::Band(_) => {
            scene.stroke(&round, Affine::IDENTITY, color, None, &Circle::new((at.x + 11.0, at.y + 6.0), 4.5));
            scene.fill(Fill::NonZero, Affine::IDENTITY, color, None, &Circle::new((at.x + 11.0, at.y + 6.0), 1.8));
        }
    }
}

/// Headphones centred on `c`: the solo glyph.
fn draw_headphones(scene: &mut mite_ui::vello::Scene, c: Point, color: Color) {
    let round = Stroke::new(1.5).with_caps(mite_ui::kurbo::Cap::Round);
    let band = mite_ui::kurbo::Arc::new(c + (0.0, 1.5), (6.0, 6.0), std::f64::consts::PI, std::f64::consts::PI, 0.0);
    scene.stroke(&round, Affine::IDENTITY, color, None, &band);
    for dx in [-6.0, 6.0] {
        let cup = RoundedRect::new(c.x + dx - 2.0, c.y + 1.0, c.x + dx + 2.0, c.y + 7.0, 1.5);
        scene.fill(Fill::NonZero, Affine::IDENTITY, color, None, &cup);
    }
}

fn draw_icon(scene: &mut mite_ui::vello::Scene, icon: Icon, at: Point, color: Color) {
    match icon {
        Icon::Type(kind) => draw_type_icon(scene, kind, at, color),
        Icon::Slope(kind, i) => draw_slope_icon(scene, kind, i, SLOPE.choices.len(), at, color),
        Icon::Stereo(placement) => draw_placement_icon(scene, placement, at, color),
        Icon::Source(source) => draw_source_icon(scene, source, at, color),
        Icon::Resolution(level) => {
            for i in 0..LP_RESOLUTIONS.len() {
                let h = 4.0 + 2.5 * i as f64;
                let bar = RoundedRect::new(at.x + 3.0 + 4.5 * i as f64, at.y + 12.0 - h, at.x + 6.0 + 4.5 * i as f64, at.y + 12.0, 1.0);
                let lit = if i <= level { color } else { color.multiply_alpha(0.3) };
                scene.fill(Fill::NonZero, Affine::IDENTITY, lit, None, &bar);
            }
        }
    }
}

/// Which band setting an open dropdown chooses.
#[derive(Clone, Copy, PartialEq)]
enum MenuKind {
    Type,
    Slope,
    Stereo,
    /// The footer's linear-phase resolution (global, not a band setting).
    Resolution,
    /// What a dynamic band's detector listens to.
    Source,
}

/// An open dropdown and what it chooses.
struct Menu {
    kind: MenuKind,
    band: usize,
    /// The band's type when the menu opened (slope icons are drawn for it).
    band_kind: BandType,
    list: Dropdown,
}

impl Menu {
    fn icon(&self, i: usize) -> Icon {
        match self.kind {
            MenuKind::Type => Icon::Type(BandType::from_index(i)),
            MenuKind::Slope => Icon::Slope(self.band_kind, i),
            MenuKind::Stereo => Icon::Stereo(Placement::from_index(i)),
            MenuKind::Resolution => Icon::Resolution(i),
            MenuKind::Source => Icon::Source(DetectSource::from_index(i)),
        }
    }
}

/// Clickable rectangles of the open band panel, set while drawing.
#[derive(Default, Clone, Copy)]
struct PanelHits {
    panel: Rect,
    kind: Rect,
    slope: Option<Rect>,
    stereo: Rect,
    /// Centre of the gain dial, whose outer ring sets the dynamic range (types with gain only).
    ring: Option<Point>,
    /// The dynamics scope and the button that removes dynamics (dynamic bands only).
    scope: Option<Rect>,
    modes: Option<[Rect; 3]>,
    remove_dynamics: Option<Rect>,
    /// The spectral compressor's fixed/relative threshold toggle.
    relative: Option<Rect>,
    /// The compressor's source dropdown.
    source: Option<Rect>,
    /// Listen to the band alone (not for a tilt, which reaches everything).
    solo: Option<Rect>,
    /// Removes the band.
    remove: Rect,
    close: Rect,
}

/// The dynamics control under the pointer.
#[derive(Clone, Copy, PartialEq)]
enum Hot {
    /// A grip on the plot: the end of a dynamic band's range, or one of the two grips beside a
    /// static node that make it dynamic.
    Grip(Point),
    Ring,
    Scope,
    /// The transient scope's line where the attack hands over to the sustain.
    Split,
    /// A dynamics mode button: compressor, spectral compressor, transient shaper.
    Mode(usize),
    RemoveDynamics,
    /// The spectral compressor's fixed/relative threshold toggle.
    Relative,
}

pub struct EqView {
    shared: Arc<EqShared>,
    ids: Vec<BandIds>,
    selected: Option<usize>,
    hover: Option<usize>,
    pointer: Option<Point>,
    drag: Option<NodeDrag>,
    panel_knobs: Vec<Knob>,
    panel_hits: Option<PanelHits>,
    menu: Option<Menu>,
    /// Footer: auto gain, delta and bypass; mix, balance and output.
    footer: Footer,
    /// The band being listened to alone, as last drawn.
    soloed: Option<usize>,
    /// Whether the EQ runs Linear Phase, as last drawn (a Brickwall is drawn as it plays).
    linear: bool,
    /// The header's presets, undo and snapshots A–D.
    session: Session,
    meter: LevelMeter,
    analyzer: Analyzer,
    samples: Vec<f32>,
    power: Vec<f32>,
    spectrum: LogSpectrum,
    pre: Vec<f64>,
    post: Vec<f64>,
    last_spectrum: Instant,
    last_analysis: Instant,
    pre_target: Vec<f32>,
    post_target: Vec<f32>,
    responses: Vec<Option<BandResponse>>,
    signature: Vec<f32>,
    last_signature_check: Instant,
    /// Wheel movement not yet turned into whole slope steps (smooth-scrolling devices send
    /// fractions of a line).
    slope_wheel: f32,
    /// Dynamic gains last drawn, dB.
    last_dynamic: [f32; BANDS],
    /// The first easing stage of the live gains and the selected band's level.
    mid_dynamic: [f32; BANDS],
    mid_spectral: [f32; SPECTRAL_POINTS],
    mid_level: f32,
    /// The selected band's detected level last drawn, dBFS.
    last_level: f32,
    /// The spectral stage's gain curve last drawn, dB, and the selected band's per-bin level
    /// (dBFS) and gain (dB).
    last_spectral: [f32; SPECTRAL_POINTS],
    focus_level: [f32; SPECTRAL_POINTS],
    focus_threshold: [f32; SPECTRAL_POINTS],
    focus_gain: [f32; SPECTRAL_POINTS],
    /// The spectral scope as drawn: the focus telemetry smoothed across frequency and eased in
    /// time like the analyzer (level before and after the band, dBFS, and the threshold).
    scope_level: [f32; SPECTRAL_POINTS],
    scope_out: [f32; SPECTRAL_POINTS],
    scope_threshold: [f32; SPECTRAL_POINTS],
    last_scope: Instant,
    /// When the live gains were last eased.
    last_live: Instant,
    /// Spectrum grab: the peak under the pointer, the marker as drawn (eased position and
    /// opacity), and a peak being dragged.
    peak: Option<Peak>,
    peak_marker: (Point, f32),
    grab: Option<Grab>,
    /// Peak hold: the accumulated maxima of the spectrum (dB per grid point), since when the
    /// pointer has rested over the plot, whether it is collecting, and its opacity as drawn.
    held: Vec<f64>,
    hold_since: Option<Instant>,
    holding: bool,
    hold_alpha: f32,
    /// The sidechain spectrum as drawn (dB per grid point), its latest analysis, and whether it
    /// carries signal.
    sc: Vec<f64>,
    sc_target: Vec<f32>,
    sc_heard: bool,
    /// Masking: the slow average of this track (post); what it is compared with (the sidechain
    /// first, then the other instances); the collisions found, their opacity as drawn, the one
    /// under the pointer, and the frequency ranges dismissed per source (a collision whose peak
    /// falls in one stays hidden).
    mask_post: LongTermSpectrum,
    sources: Vec<MaskSource>,
    collisions: Vec<(Other, Collision)>,
    last_mask: Instant,
    mask_alpha: f32,
    collision_hot: Option<usize>,
    ignored: Vec<(Other, f64, f64)>,
    /// The instances this one can see, when they were last listed, and analyses so far.
    registry: &'static Registry<EqShared>,
    last_peers: Option<Instant>,
    analyses: u32,
    /// EQ Match, while its panel is open.
    matcher: Option<Match>,
    hot: Option<Hot>,
    /// Origin the panel dials were laid out for, and whether the dynamics row was included.
    knobs_layout: Option<(Point, Option<f64>)>,
    /// The panel's dock: x, and whether it hangs from the bottom of the plot.
    panel_dock: Option<(f64, bool)>,
    /// Footer: the processing-mode buttons and the resolution dropdown (Linear Phase only), set
    /// while drawing; the latency last drawn, samples.
    mode_hits: [Rect; 3],
    resolution_hit: Option<Rect>,
    last_latency: f32,
    ticks: u32,
}

impl EqView {
    pub fn new(shared: Arc<EqShared>) -> Self {
        let analyzer = Analyzer::new(ANALYZER_SIZE);
        let bins = analyzer.bins();
        let footer = Footer::new(
            FOOTER_TOP,
            (WIDTH, HEIGHT),
            vec![
                FooterToggle { id: AUTO_GAIN.id, glyph: ToggleGlyph::Auto, words: "Auto gain — keeps the loudness as the curve changes (worked out from the curve, so it never pumps)" },
                FooterToggle { id: DELTA.id, glyph: ToggleGlyph::Delta, words: "Delta — hear only what the EQ changes" },
                FooterToggle { id: BYPASS.id, glyph: ToggleGlyph::Bypass, words: "Bypass — the whole EQ, latency kept" },
            ],
            vec![
                FooterDial { spec: MIX, kind: KnobKind::Unipolar, name: "MIX", words: "Mix — the EQ against the dry signal, delay-compensated" },
                FooterDial { spec: OUT_PAN, kind: KnobKind::Bipolar, name: "PAN", words: "Output balance" },
                FooterDial { spec: OUT_GAIN, kind: KnobKind::Bipolar, name: "OUTPUT", words: "Output gain" },
            ],
        );
        Self {
            ids: (0..BANDS).map(BandIds::new).collect(),
            selected: None,
            hover: None,
            pointer: None,
            drag: None,
            panel_knobs: Vec::new(),
            panel_hits: None,
            menu: None,
            footer,
            soloed: None,
            linear: false,
            session: Session::new(
                SessionConfig { plugin: PLUGIN, factory: FACTORY, sound_ids: preset_ids(), snapshot_ids: snapshot_ids() },
                shared.snapshots.clone(),
                shared.preset.clone(),
            ),
            meter: LevelMeter::new(Rect::new(1233.5, DISPLAY_TOP + 19.5, 1266.5, DISPLAY_TOP + 592.5)),
            analyzer,
            samples: vec![0.0; ANALYZER_SIZE],
            power: vec![0.0; bins],
            spectrum: LogSpectrum::new(ANALYZER_SIZE, 48_000.0),
            pre: vec![-150.0; POINTS],
            post: vec![-150.0; POINTS],
            last_spectrum: Instant::now(),
            last_analysis: Instant::now(),
            pre_target: vec![-150.0; POINTS],
            post_target: vec![-150.0; POINTS],
            responses: (0..BANDS).map(|_| None).collect(),
            signature: Vec::new(),
            last_signature_check: Instant::now(),
            slope_wheel: 0.0,
            last_dynamic: [0.0; BANDS],
            mid_dynamic: [0.0; BANDS],
            mid_spectral: [0.0; SPECTRAL_POINTS],
            mid_level: -120.0,
            last_level: -120.0,
            last_spectral: [0.0; SPECTRAL_POINTS],
            focus_level: [-150.0; SPECTRAL_POINTS],
            focus_threshold: [-150.0; SPECTRAL_POINTS],
            focus_gain: [0.0; SPECTRAL_POINTS],
            scope_level: [-150.0; SPECTRAL_POINTS],
            scope_out: [-150.0; SPECTRAL_POINTS],
            scope_threshold: [-150.0; SPECTRAL_POINTS],
            last_scope: Instant::now(),
            last_live: Instant::now(),
            peak: None,
            peak_marker: (Point::ZERO, 0.0),
            grab: None,
            held: vec![-150.0; POINTS],
            hold_since: None,
            holding: false,
            hold_alpha: 0.0,
            sc: vec![-150.0; POINTS],
            sc_target: vec![-150.0; POINTS],
            sc_heard: false,
            mask_post: LongTermSpectrum::new(POINTS, MASK_AVERAGE_S),
            sources: vec![MaskSource::new(Other::Sidechain, None)],
            collisions: Vec::new(),
            registry: &INSTANCES,
            last_peers: None,
            analyses: 0,
            last_mask: Instant::now(),
            mask_alpha: 0.0,
            collision_hot: None,
            ignored: Vec::new(),
            matcher: None,
            hot: None,
            knobs_layout: None,
            panel_dock: None,
            mode_hits: [Rect::ZERO; 3],
            resolution_hit: None,
            last_latency: 0.0,
            ticks: 0,
            shared,
        }
    }

    fn plain(host: &dyn ParamHost, id: &str) -> f32 {
        host.plain(id, host.normalized(id))
    }

    /// The band soloed now: the first enabled band with solo on.
    fn soloed_band(&self, host: &dyn ParamHost) -> Option<usize> {
        (0..BANDS).find(|&b| Self::plain(host, &self.ids[b].solo) >= 0.5 && Self::plain(host, &self.ids[b].on) >= 0.5)
    }

    fn band(&self, host: &dyn ParamHost, b: usize) -> BandSettings {
        let ids = &self.ids[b];
        BandSettings {
            enabled: Self::plain(host, &ids.on) >= 0.5,
            kind: BandType::from_index(Self::plain(host, &ids.kind).round().max(0.0) as usize),
            freq: f64::from(Self::plain(host, &ids.freq)),
            gain_db: f64::from(Self::plain(host, &ids.gain)),
            q: f64::from(Self::plain(host, &ids.q)),
            slope: Self::plain(host, &ids.slope).round().max(0.0) as usize,
            placement: Placement::from_index(Self::plain(host, &ids.stereo).round().max(0.0) as usize),
            dynamics: Dynamics {
                enabled: Self::plain(host, &ids.dyn_on) >= 0.5,
                mode: DynamicsMode::from_index(Self::plain(host, &ids.dmode).round().max(0.0) as usize),
                range_db: f64::from(Self::plain(host, &ids.range)),
                transient_db: f64::from(Self::plain(host, &ids.transient)),
                sustain_db: f64::from(Self::plain(host, &ids.sustain)),
                transient_ms: f64::from(Self::plain(host, &ids.ttime)),
                transition: f64::from(Self::plain(host, &ids.transition)) / 100.0,
                threshold_db: f64::from(Self::plain(host, &ids.thresh)),
                attack_ms: f64::from(Self::plain(host, &ids.attack)),
                release_ms: f64::from(Self::plain(host, &ids.release)),
                ratio: f64::from(Self::plain(host, &ids.ratio)),
                spectral: Self::plain(host, &ids.spectral) >= 0.5,
                smoothing: f64::from(Self::plain(host, &ids.ssmooth)) / 100.0,
                relative: Self::plain(host, &ids.srel) >= 0.5,
                relative_db: f64::from(Self::plain(host, &ids.rthresh)),
                source: DetectSource::from_index(Self::plain(host, &ids.dsrc).round().max(0.0) as usize),
            },
        }
    }

    fn set(host: &dyn ParamHost, id: &str, plain: f32) {
        host.begin_gesture(id);
        host.set_normalized(id, host.normalize(id, plain));
        host.end_gesture(id);
    }

    fn node_pos(s: &BandSettings) -> Point {
        let y = if s.kind.has_gain() { gy(s.gain_db.clamp(-18.0, 18.0)) } else { MID };
        Point::new(fx(s.freq), y)
    }

    /// One colour per stereo placement: all stereo bands share a colour, all left bands
    /// another, and so on, so the colour says what the band processes.
    fn band_color(theme: &mite_ui::Theme, s: &BandSettings) -> Color {
        theme.band_ramp[s.placement as usize % theme.band_ramp.len()]
    }

    fn node_at(&self, host: &dyn ParamHost, p: Point) -> Option<usize> {
        // Topmost first: the selected band, then the highest band number.
        let mut order: Vec<usize> = (0..BANDS).rev().collect();
        if let Some(sel) = self.selected {
            order.retain(|&b| b != sel);
            order.insert(0, sel);
        }
        order.into_iter().find(|&b| {
            let s = self.band(host, b);
            s.enabled && Self::node_pos(&s).distance(p) <= NODE_HIT
        })
    }

    fn select(&mut self, band: Option<usize>) {
        if band.is_some() {
            self.matcher = None;
        }
        self.selected = band;
        self.slope_wheel = 0.0;
        self.menu = None;
        self.panel_knobs.clear();
        self.panel_hits = None;
        self.panel_dock = None;
    }

    fn sample_rate(&self) -> f64 {
        f64::from(self.shared.sample_rate.load()).max(8000.0)
    }

    /// Band `b`'s digital sections and its response in dB at every grid point, cached until its
    /// settings or the sample rate change.
    fn response(&mut self, b: usize, s: &BandSettings) -> &BandResponse {
        let fs = self.sample_rate();
        let linear = self.linear;
        let stale = !matches!(&self.responses[b], Some(r) if r.settings == *s && r.fs == fs && r.linear == linear);
        if stale {
            let sections = design(s, fs);
            let grid = (0..POINTS).map(|i| band_db(&sections, s, xf(grid_x(i)), fs, linear)).collect();
            self.responses[b] = Some(BandResponse { settings: *s, fs, linear, sections, grid });
        }
        self.responses[b].as_ref().expect("just filled")
    }

    /// Called on every animation frame (~2× the display refresh). The FFT runs at most once
    /// per display refresh; every call advances the ballistics towards the latest analysis, so
    /// in-between frames still move smoothly.
    fn update_spectrum(&mut self) -> bool {
        let dt = self.last_spectrum.elapsed().as_secs_f64().min(0.1);
        self.last_spectrum = Instant::now();
        let fs = self.sample_rate();
        if fs != self.spectrum.sample_rate() {
            self.spectrum = LogSpectrum::new(ANALYZER_SIZE, fs);
        }
        if self.last_analysis.elapsed().as_secs_f64() >= 0.0155 {
            let frame_s = self.last_analysis.elapsed().as_secs_f64().min(0.1);
            self.last_analysis = Instant::now();
            for which in 0..3 {
                let (tap, target) = match which {
                    0 => (&self.shared.pre, &mut self.pre_target),
                    1 => (&self.shared.post, &mut self.post_target),
                    _ => (&self.shared.sidechain, &mut self.sc_target),
                };
                tap.read_latest(&mut self.samples);
                // A silent sidechain (unconnected, or nothing playing) needs no FFT.
                if which == 2 && self.samples.iter().all(|s| s.abs() < 1e-7) {
                    target.iter_mut().for_each(|t| *t = -150.0);
                    continue;
                }
                self.analyzer.analyze_power(&self.samples, &mut self.power);
                self.spectrum.read(&self.power, target);
            }
            self.analyses = self.analyses.wrapping_add(1);
            if self.analyses % PEER_ANALYSIS_EVERY == 0 {
                self.analyze_peers();
            }
            self.learn(frame_s);
        }
        let rise = (1.0 - (-dt / SPECTRUM_RISE_S).exp()) as f32;
        let fall = (1.0 - (-dt / SPECTRUM_FALL_S).exp()) as f32;
        let mut changed = false;
        for (curve, target) in [(&mut self.pre, &self.pre_target), (&mut self.post, &self.post_target), (&mut self.sc, &self.sc_target)] {
            for (value, &db) in curve.iter_mut().zip(target.iter()) {
                // Ballistics: both ways eased, so frames glide into each other instead of jumping.
                let current = *value as f32;
                let next = current + (db - current) * if db > current { rise } else { fall };
                if (next - current).abs() > 0.02 {
                    changed = true;
                }
                *value = f64::from(next);
            }
        }
        self.sc_heard = self.sc.iter().any(|&v| v > -88.0);
        changed
    }

    /// Lists the other instances (every [`PEERS_EVERY_S`]; an instance that is not registered
    /// itself has none) and reads each one's post spectrum, if it runs at this sample rate.
    fn analyze_peers(&mut self) {
        if self.last_peers.is_none_or(|t| t.elapsed().as_secs_f64() >= PEERS_EVERY_S) {
            self.last_peers = Some(Instant::now());
            let others = if self.registry.number(&self.shared).is_some() { self.registry.others(&self.shared) } else { Vec::new() };
            let before: Vec<Other> = self.sources.iter().map(|src| src.other).collect();
            self.sources.retain(|src| match src.other {
                Other::Sidechain => true,
                Other::Instance(n) => others.iter().any(|(m, a)| *m == n && src.shared.as_ref().is_some_and(|w| std::ptr::eq(w.as_ptr(), Arc::as_ptr(a)))),
            });
            for (n, a) in &others {
                if !self.sources.iter().any(|src| src.other == Other::Instance(*n)) {
                    self.sources.push(MaskSource::new(Other::Instance(*n), Some(Arc::downgrade(a))));
                }
            }
            if self.sources.iter().map(|src| src.other).ne(before) {
                self.collisions.retain(|(other, _)| self.sources.iter().any(|src| src.other == *other));
                self.collision_hot = None;
            }
        }
        let fs = self.sample_rate();
        for src in self.sources.iter_mut().skip(1) {
            let Some(peer) = src.shared.as_ref().and_then(std::sync::Weak::upgrade) else { continue };
            src.heard = false;
            if f64::from(peer.sample_rate.load()) != fs {
                continue;
            }
            peer.post.read_latest(&mut self.samples);
            if self.samples.iter().all(|s| s.abs() < 1e-7) {
                continue;
            }
            self.analyzer.analyze_power(&self.samples, &mut self.power);
            self.spectrum.read(&self.power, &mut src.latest);
            src.heard = src.latest.iter().any(|&d| f64::from(d) > HEARD_DB);
            src.fresh = true;
        }
    }

    /// Feeds the latest analysis to the masking averages and, while EQ Match learns, to its
    /// spectra. A frame counts only while both sides are heard, so pauses do not dilute them.
    fn learn(&mut self, dt: f64) {
        let heard = |v: &[f32]| v.iter().any(|&d| f64::from(d) > HEARD_DB);
        let sidechain = &mut self.sources[0];
        sidechain.latest.copy_from_slice(&self.sc_target);
        sidechain.heard = heard(&self.sc_target);
        sidechain.fresh = true;
        if heard(&self.post_target) {
            self.mask_post.add(&self.post_target, dt);
            for src in &mut self.sources {
                if src.fresh && src.heard {
                    let span = if src.other == Other::Sidechain { dt } else { dt * f64::from(PEER_ANALYSIS_EVERY) };
                    src.average.add(&src.latest, span);
                }
                src.fresh = false;
            }
        }
        let (sidechain, track) = (heard(&self.sc_target), heard(&self.pre_target));
        if let Some(m) = self.matcher.as_mut().filter(|m| m.learning) {
            // Against a file only this track needs to play.
            if track && (sidechain || m.file.is_some()) {
                m.source.add(&self.pre_target, dt);
                if m.file.is_none() {
                    m.reference.add(&self.sc_target, dt);
                }
                m.stale = true;
            }
        }
    }

    /// The bands' static curve (dB per grid point), as drawn without their live movement.
    fn static_total(&mut self, host: &dyn ParamHost) -> Vec<f64> {
        let mut total = vec![0.0; POINTS];
        for b in 0..BANDS {
            let s = self.band(host, b);
            if s.enabled {
                for (t, v) in total.iter_mut().zip(&self.response(b, &s).grid) {
                    *t += v;
                }
            }
        }
        total
    }

    /// Looks for collisions between this track and each source that plays (while Match is
    /// closed) and fades them in or out. Returns whether anything drawn changed.
    fn update_masking(&mut self, dt: f64) -> bool {
        let playing = |src: &MaskSource| src.heard && src.average.seconds() >= 1.0;
        let show = self.matcher.is_none() && self.sources.iter().any(playing);
        let mut changed = false;
        if show && self.last_mask.elapsed().as_secs_f64() >= MASK_UPDATE_S {
            self.last_mask = Instant::now();
            let (mut a, mut b) = (vec![0.0; POINTS], vec![0.0; POINTS]);
            self.mask_post.read_db(&mut a);
            let freqs = grid_freqs();
            let mut found = Vec::new();
            for src in &mut self.sources {
                if !playing(src) {
                    continue;
                }
                src.average.read_db(&mut b);
                let weight = matching::match_weight(&freqs, &a, &b, HEARD_DB);
                src.score = matching::masking_score(&a, &b, &weight, spectrum_point_oct());
                let other = src.other;
                found.extend(
                    matching::collisions(&src.score, spectrum_point_oct())
                        .into_iter()
                        .filter(|c| !self.ignored.iter().any(|&(o, lo, hi)| o == other && (lo..=hi).contains(&freqs[c.peak])))
                        .map(|c| (other, c)),
                );
            }
            if found != self.collisions {
                changed = true;
                // The averages drift, so the regions do: a suggestion being looked at stays up
                // while the pointer is still in its (moved) region.
                let hot = self.collision_hot.and_then(|i| self.collisions.get(i)).map(|(other, _)| *other);
                self.collisions = found;
                self.collision_hot = hot.and_then(|other| {
                    let p = self.pointer?;
                    self.collision_at(p).filter(|&i| self.collisions[i].0 == other)
                });
            } else {
                self.collisions = found;
            }
        }
        let k = (1.0 - (-dt / MASK_FADE_S).exp()) as f32;
        let target = if show && !self.collisions.is_empty() { 1.0 } else { 0.0 };
        let next = self.mask_alpha + (target - self.mask_alpha) * k;
        let next = if (next - target).abs() < 0.005 { target } else { next };
        changed |= (next - self.mask_alpha).abs() > 0.001;
        self.mask_alpha = next;
        if next == 0.0 && target == 0.0 {
            self.collisions.clear();
            self.collision_hot = None;
        }
        changed
    }

    fn source(&self, other: Other) -> Option<&MaskSource> {
        self.sources.iter().find(|src| src.other == other)
    }

    /// The carve collision `i` suggests.
    fn carve(&self, i: usize) -> BandSettings {
        let (other, c) = &self.collisions[i];
        let score = self.source(*other).map_or(&[][..], |src| &src.score[..]);
        if score.len() < POINTS {
            return BandSettings::default();
        }
        matching::carve(c, &grid_freqs(), score)
    }

    /// The other instance collision `i` is with, and its parameters if they can be written.
    fn collision_peer(&self, i: usize) -> Option<(Arc<EqShared>, Option<mite_plugin::SharedParamHost>)> {
        let src = self.source(self.collisions[i].0)?;
        let peer = src.shared.as_ref()?.upgrade()?;
        let host = peer.remote.lock().unwrap_or_else(|e| e.into_inner()).clone();
        Some((peer, host))
    }

    /// Where collision `i`'s second ring sits, the one that carves on the other instance (none
    /// for the sidechain).
    fn carve_there_pos(&self, i: usize) -> Option<Point> {
        matches!(self.collisions[i].0, Other::Instance(_)).then(|| self.carve_pos(i) + (CARVE_THERE_DX, 0.0))
    }

    /// Carves collision `i` on the other instance: the same cut, in its first free band.
    /// Returns whether it could.
    fn carve_there(&self, i: usize) -> bool {
        let Some((_, Some(remote))) = self.collision_peer(i) else { return false };
        let s = self.carve(i);
        let Some(b) = (0..BANDS).find(|&b| remote.normalized(&self.ids[b].on) < 0.5) else { return false };
        self.place_band(&*remote, b, &s);
        true
    }

    /// The collision whose range holds `p` (on the plot), if collisions are showing.
    fn collision_at(&self, p: Point) -> Option<usize> {
        if self.mask_alpha < 0.5 || p.x < 0.0 || p.x >= PLOT_W || p.y < PLOT_TOP || p.y > PLOT_BOT {
            return None;
        }
        self.collisions.iter().position(|(_, c)| p.x >= fx(spectrum_point_freq(c.start)) && p.x <= fx(spectrum_point_freq(c.end)))
    }

    /// Where the carve ring under the pointer is drawn, if one is (for tests and tools).
    pub fn carve_ring(&self) -> Option<Point> {
        self.collision_hot.filter(|&i| i < self.collisions.len()).map(|i| self.carve_pos(i))
    }

    /// Where the ring that carves on the other instance is drawn, if one is (for tests and tools).
    pub fn carve_there_ring(&self) -> Option<Point> {
        self.collision_hot.filter(|&i| i < self.collisions.len()).and_then(|i| self.carve_there_pos(i))
    }

    /// Where collision `i`'s carve ring sits.
    fn carve_pos(&self, i: usize) -> Point {
        let s = self.carve(i);
        Point::new(fx(s.freq), gy(s.gain_db))
    }

    /// Writes `s` into band `b` (a free one) and turns it on: no dynamics, both channels.
    fn place_band(&self, host: &dyn ParamHost, b: usize, s: &BandSettings) {
        let ids = &self.ids[b];
        Self::set(host, &ids.kind, s.kind as usize as f32);
        Self::set(host, &ids.freq, s.freq.clamp(FREQ_MIN, FREQ_MAX) as f32);
        Self::set(host, &ids.gain, s.gain_db.clamp(-30.0, 30.0) as f32);
        Self::set(host, &ids.q, s.q as f32);
        Self::set(host, &ids.stereo, 0.0);
        Self::set(host, &ids.dyn_on, 0.0);
        Self::set(host, &ids.on, 1.0);
    }

    /// Opens or closes EQ Match. Opening starts learning afresh and puts the band panel away.
    fn toggle_match(&mut self) {
        if self.matcher.take().is_none() {
            self.select(None);
            self.matcher = Some(Match::new());
            self.collision_hot = None;
        }
    }

    /// Matches to the audio file at `path` (opening Match if it is closed); what the file button
    /// does once a file is picked.
    pub fn load_match_reference(&mut self, path: std::path::PathBuf) {
        if self.matcher.is_none() {
            self.toggle_match();
        }
        if let Some(m) = &mut self.matcher {
            m.load(path);
        }
    }

    /// Adds the fitted bands in the free slots (one undo step: one event) and closes Match.
    fn apply_match(&mut self, host: &dyn ParamHost) {
        let Some(m) = self.matcher.take() else { return };
        let mut free = (0..BANDS).filter(|&b| !self.band(host, b).enabled);
        for s in &m.fitted {
            let Some(b) = free.next() else { break };
            self.place_band(host, b, s);
        }
    }

    /// Follows the fit on its worker and starts a new one when the learned spectra or the dials
    /// moved (at most every [`MATCH_REFIT_S`] while learning). Returns whether anything drawn
    /// changed.
    fn update_match(&mut self, host: &dyn ParamHost) -> bool {
        let Some(m) = &mut self.matcher else { return false };
        let mut changed = false;
        if let Some(rx) = &m.picking {
            match rx.try_recv() {
                Ok(path) => {
                    m.picking = None;
                    if let Some(path) = path {
                        m.load(path);
                        changed = true;
                    }
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => m.picking = None,
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
            }
        }
        if let Some((name, rx)) = &m.loading {
            match rx.try_recv() {
                Ok(result) => {
                    let name = name.clone();
                    m.loading = None;
                    match result {
                        Ok(db) => {
                            m.file = Some((name, db));
                            m.refit();
                        }
                        Err(e) => m.error = Some(format!("{name}: {e}")),
                    }
                    changed = true;
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => m.loading = None,
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
            }
        }
        if let Some((rx, _)) = &m.pending {
            match rx.try_recv() {
                Ok(bands) => {
                    let (_, aim) = m.pending.take().expect("pending");
                    m.ghost = grid_freqs().iter().map(|&f| bands.iter().map(|s| analog_response_db(s, f)).sum()).collect();
                    m.fitted = bands;
                    m.aim = aim;
                    changed = true;
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => m.pending = None,
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
            }
        }
        let inputs = m.inputs();
        let moved = inputs != m.inputs;
        let due = moved || (m.stale && m.last_fit.elapsed().as_secs_f64() >= MATCH_REFIT_S);
        if !due || m.pending.is_some() || m.learned_s() < 0.5 {
            return changed;
        }
        m.inputs = inputs;
        m.stale = false;
        m.last_fit = Instant::now();
        let [amount, bands, smooth] = inputs;
        let (mut reference, mut source) = (vec![0.0; POINTS], vec![0.0; POINTS]);
        match &m.file {
            Some((_, db)) => reference.copy_from_slice(db),
            None => m.reference.read_db(&mut reference),
        }
        m.source.read_db(&mut source);
        let current = self.static_total(host);
        let free = (0..BANDS).filter(|&b| !self.band(host, b).enabled).count();
        let freqs = grid_freqs();
        // The track as the current bands already shape it: the fit adds what they leave.
        let shaped: Vec<f64> = source.iter().zip(&current).map(|(s, c)| s + c).collect();
        // Past a cut's corner the match does not listen (fading in over the half octave inside
        // it): it would undo what was removed on purpose.
        let cuts: Vec<BandSettings> = (0..BANDS).map(|b| self.band(host, b)).filter(|s| s.enabled && matches!(s.kind, BandType::LowCut | BandType::HighCut)).collect();
        let weight: Vec<f64> = matching::match_weight(&freqs, &reference, &source, HEARD_DB)
            .iter()
            .zip(&freqs)
            .map(|(&w, &f)| {
                cuts.iter().fold(w, |w, c| {
                    let inside = if c.kind == BandType::LowCut { (f / c.freq).log2() } else { (c.freq / f).log2() };
                    w * (2.0 * inside).clamp(0.0, 1.0)
                })
            })
            .collect();
        let residual = matching::match_target(&reference, &shaped, &weight, spectrum_point_oct(), f64::from(smooth), f64::from(amount) / 100.0);
        let aim: Vec<Option<f64>> = (0..POINTS).map(|i| (weight[i] > 0.5).then(|| current[i] + residual[i])).collect();
        let max = (bands.round() as usize).min(free);
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(matching::fit(&freqs, &residual, &weight, max));
        });
        let m = self.matcher.as_mut().expect("open");
        m.pending = Some((rx, aim));
        changed
    }

    /// Eases the spectral scope towards the latest focus telemetry with the analyzer's rise and
    /// fall. The telemetry is already the analyzer's points (the detector reads the spectrum as
    /// the analyzer does), so the scope moves like the main spectrum. Returns whether it moved
    /// visibly.
    fn update_scope(&mut self) -> bool {
        let dt = self.last_scope.elapsed().as_secs_f64().min(0.1);
        self.last_scope = Instant::now();
        let rise = (1.0 - (-dt / SPECTRUM_RISE_S).exp()) as f32;
        let fall = (1.0 - (-dt / SPECTRUM_FALL_S).exp()) as f32;
        let ease = |value: &mut f32, target: f32| {
            let next = if *value <= -149.0 { target } else { *value + (target - *value) * if target > *value { rise } else { fall } };
            let moved = (next - *value).abs() > 0.02;
            *value = next;
            moved
        };
        let mut moved = false;
        for i in 0..SPECTRAL_POINTS {
            let level = self.focus_level[i];
            moved |= ease(&mut self.scope_level[i], level);
            moved |= ease(&mut self.scope_out[i], (level + self.focus_gain[i]).max(-150.0));
            moved |= ease(&mut self.scope_threshold[i], self.focus_threshold[i]);
        }
        moved
    }

    fn help_text(&self, host: &dyn ParamHost) -> Help {
        let band_line = |b: usize| {
            let s = self.band(host, b);
            let mut h = Help::default()
                .slot(format!("Band {}", b + 1), "Band 24")
                .text(" · ")
                .slot(BandType::NAMES[s.kind as usize], "High Shelf")
                .text(" · ")
                .slot(FREQ.format(s.freq as f32), "20.00 kHz");
            if s.kind.has_gain() {
                h = h.text(" · ").slot(GAIN.format(s.gain_db as f32), "−30.0 dB");
            }
            if s.is_dynamic() {
                h = Self::dynamics_words(h.text(" · "), &s);
            }
            if mite_dsp::eq::is_brickwall(&s) && !self.linear {
                h = h.text(" · 96 dB/oct here (a vertical edge in Linear Phase)");
            }
            if s.kind == BandType::AllPass {
                h = h.text(if self.linear { " · phase only: no effect in Linear Phase" } else { " · phase only" });
            }
            let wheel = if s.kind.has_slope() { "wheel for slope" } else { "Alt-drag or wheel for Q" };
            h.text(" — drag to move, Shift for fine, ").text(wheel).text(", double-click resets, right-click deletes")
        };
        if let Some(m) = &self.matcher {
            if let Some(e) = &m.error {
                return Help::default().text(format!("EQ Match — could not read {e}"));
            }
            if let Some((name, _)) = &m.loading {
                return Help::default().text(format!("EQ Match — reading {name} …"));
            }
            if let Some((name, _)) = &m.file {
                let state = if m.learning { "learning this track while it plays" } else { "paused" };
                return Help::default().text(format!("EQ Match — matching to {name} · {state} · the tick adds the bands, × goes back to the sidechain, Esc closes"));
            }
            let text = if m.reference.seconds() == 0.0 && !self.sc_heard {
                "EQ Match — play the reference into the sidechain, or pick a file (the page button); it learns while both play"
            } else if m.fitted.is_empty() && m.reference.seconds() > 0.0 && !(0..BANDS).any(|b| !self.band(host, b).enabled) {
                "EQ Match — every band is in use"
            } else if m.learning {
                "EQ Match — learning while both play · amount, bands and smoothing shape the fit · the tick adds the bands, Esc closes"
            } else {
                "EQ Match — paused · the tick adds the bands, the arrow starts over, Esc closes"
            };
            return Help::default().text(text);
        }
        if let Some(i) = self.collision_hot.filter(|&i| i < self.collisions.len()) {
            let s = self.carve(i);
            let other = self.collisions[i].0;
            let h = Help::default()
                .text(format!("Masking with {} — both peak here · the ring carves ", other.name()))
                .slot(FREQ.format(s.freq as f32), "20.00 kHz")
                .text(" ")
                .slot(GAIN.format(s.gain_db as f32), "−30.0 dB");
            let h = match self.collision_peer(i) {
                Some((_, Some(_))) => h.text(format!(" here, the arrow on {}", other.name())),
                Some((_, None)) => h.text(format!(" here (open {}'s window once to carve there too)", other.name())),
                None => h,
            };
            return h.text(", right-click ignores");
        }
        if let Some(help) = self.dynamics_help(host) {
            return help;
        }
        if let Some(p) = self.pointer {
            if let Some(help) = self.session.help() {
                return help;
            }
            if let Some(help) = self.footer.help(host) {
                return help;
            }
            if let (Some(b), Some(h)) = (self.selected, self.panel_hits) {
                if h.solo.is_some_and(|r| r.contains(p)) {
                    return Help::default().text(format!("Band {} solo — listen to its region alone, until the panel closes", b + 1));
                }
                if h.remove.contains(p) {
                    return Help::default().text(format!("Band {} — remove", b + 1));
                }
                if h.source.is_some_and(|r| r.contains(p)) {
                    let heard = match self.band(host, b).dynamics.source {
                        DetectSource::Own => "its own region".to_owned(),
                        DetectSource::Sidechain => "its region of the sidechain".to_owned(),
                        DetectSource::Band(n) => format!("band {}'s region", n + 1),
                    };
                    return Help::default().text(format!("Band {} listens to {heard} — click to choose", b + 1));
                }
                if h.close.contains(p) {
                    return Help::default().text("Close the panel");
                }
            }
        }
        if let Some(g) = &self.grab {
            return Help::default()
                .text("New bell ")
                .slot(FREQ.format(g.freq as f32), "20.00 kHz")
                .text(" · ")
                .slot(GAIN.format(g.gain as f32), "−30.0 dB")
                .text(" — release to add it, Esc cancels");
        }
        if let Some(d) = &self.drag {
            return band_line(d.band);
        }
        if let Some(b) = self.hover {
            return band_line(b);
        }
        if let Some(peak) = self.peak {
            return Help::default()
                .text("Peak ")
                .slot(FREQ.format(peak.freq as f32), "20.00 kHz")
                .text(" · ")
                .slot(note_name(peak.freq), "C#10 −50 ct")
                .text(" · ")
                .slot(format!("{:.1} dB", peak.db).replace('-', "−"), "−90.0 dB")
                .text(" — drag up or down to shape a bell on it; double-click cuts it 3 dB");
        }
        if let Some(p) = self.pointer.filter(|p| p.x < PLOT_W && p.y > PLOT_TOP && p.y < PLOT_BOT) {
            let f = xf(p.x);
            return Help::default()
                .slot(FREQ.format(f as f32), "20.00 kHz")
                .text(" · ")
                .slot(note_name(f), "C#10 −50 ct")
                .text(" · ")
                .slot(format!("{:+.1} dB", yg(p.y)).replace('-', "−"), "+30.0 dB")
                .text(" — double-click to add a band");
        }
        if let Some(b) = self.selected {
            return band_line(b);
        }
        if (0..BANDS).any(|b| self.band(host, b).enabled) {
            Help::default().text("Click a band to edit it. Double-click empty space to add one.")
        } else {
            Help::default().text("No bands — the audio passes unchanged. Double-click the display to add a band.")
        }
    }

    /// Appends "dynamic −6.0 dB", "spectral −6.0 dB", "attack +6.0 dB · sustain −3.0 dB", or
    /// "static", with the numbers in fixed slots.
    fn dynamics_words(h: Help, s: &BandSettings) -> Help {
        let d = &s.dynamics;
        match (s.is_dynamic(), d.mode) {
            (false, _) => h.text("static"),
            (true, DynamicsMode::Compressor) if d.spectral => h.text("spectral ").slot(RANGE.format(d.range_db as f32), "−30.0 dB"),
            (true, DynamicsMode::Compressor) => h.text("dynamic ").slot(RANGE.format(d.range_db as f32), "−30.0 dB"),
            (true, DynamicsMode::Transient) => h
                .text("attack ")
                .slot(TRANSIENT.format(d.transient_db as f32), "−12.0 dB")
                .text(" · sustain ")
                .slot(SUSTAIN.format(d.sustain_db as f32), "−12.0 dB"),
        }
    }

    /// Help for the dynamics control being dragged or pointed at.
    fn dynamics_help(&self, host: &dyn ParamHost) -> Option<Help> {
        let hot = match self.drag.as_ref().map(|d| d.mode) {
            Some(DragMode::Range) => Hot::Grip(Point::ZERO),
            Some(DragMode::TransientAttack | DragMode::Sustain) => Hot::Scope,
            Some(DragMode::Split) => Hot::Split,
            Some(DragMode::Ring) => Hot::Ring,
            Some(DragMode::Scope | DragMode::SpectralThreshold) => Hot::Scope,
            Some(_) => return None,
            None => self.hot?,
        };
        let b = self.selected?;
        let s = self.band(host, b);
        let band = || Help::default().slot(format!("Band {}", b + 1), "Band 24").text(" ");
        let words = || Self::dynamics_words(band(), &s);
        Some(match hot {
            Hot::Grip(_) if s.is_dynamic() => words().text(" — drag; double-click removes"),
            Hot::Grip(_) => band().text("— drag to make dynamic"),
            Hot::Ring => words().text(" — drag the ring; double-click removes"),
            Hot::Scope if s.dynamics.mode == DynamicsMode::Transient => words().text(" — drag"),
            Hot::Scope if s.is_spectral() && s.dynamics.relative => words()
                .text(" · threshold ")
                .slot(RTHRESH.format(s.dynamics.relative_db as f32), "24.0 dB")
                .text(" over the surroundings — what sticks out above the line is compressed; drag the line"),
            Hot::Scope if s.is_spectral() => words()
                .text(" · threshold ")
                .slot(THRESH.format(s.dynamics.threshold_db as f32), "−80.0 dB")
                .text(" — each frequency above the line is compressed on its own; drag the line"),
            Hot::Relative if s.dynamics.relative => {
                Help::default().text("Threshold follows the surroundings: catches what sticks out, however loud — click for a fixed level")
            }
            Hot::Relative => Help::default().text("Fixed threshold level — click to follow the surroundings instead (catches what sticks out, however loud)"),
            Hot::Scope => words().text(" · threshold ").slot(THRESH.format(s.dynamics.threshold_db as f32), "−80.0 dB").text(" — drag"),
            Hot::Split => band().text("transient time ").slot(TTIME.format(s.dynamics.transient_ms as f32), "200 ms").text(" — drag; double-click resets"),
            Hot::Mode(0) => Help::default().text("Compressor"),
            Hot::Mode(1) => Help::default().text("Spectral compressor: only the loud frequencies inside the band move"),
            Hot::Mode(_) => Help::default().text("Transient shaper"),
            Hot::RemoveDynamics => band().text("— make static"),
        })
    }

    /// Whether a gesture is still under way (a drag becomes one undo step when it ends).
    fn busy(&self) -> bool {
        self.drag.is_some()
            || self.grab.is_some()
            || self.panel_knobs.iter().any(Knob::is_dragging)
            || self.footer.busy()
    }

    /// Looks for other instances in `registry` instead of the plugin's own (tests).
    pub fn with_registry(mut self, registry: &'static Registry<EqShared>) -> Self {
        self.registry = registry;
        self
    }

    /// Keeps the user's presets in `dir` instead of the settings folder (tests, a portable setup).
    pub fn with_preset_dir(mut self, dir: std::path::PathBuf) -> Self {
        self.session = self.session.with_preset_dir(dir);
        self
    }

    /// The header's session (presets, undo, snapshots), for tests and tools.
    pub fn session(&self) -> &Session {
        &self.session
    }

    /// Keys: the session's (a preset name, undo, redo), then Esc. Others go to the host.
    fn key_event(&mut self, key: Key, mods: mite_ui::Modifiers, host: &dyn ParamHost) -> bool {
        if let Some(used) = self.session.key(key, mods, host) {
            return used;
        }
        match key {
            Key::Escape => self.menu.take().is_some() || self.grab.take().is_some() || self.matcher.take().is_some(),
            _ => false,
        }
    }

    /// After the session replaced the parameters (a preset, a snapshot, undo): drop what was
    /// derived from them.
    fn after_session(&mut self, host: &dyn ParamHost) {
        if self.session.take_replaced() {
            if self.selected.is_some_and(|b| !self.band(host, b).enabled) {
                self.select(None);
            }
            self.panel_knobs.clear();
        }
    }

    fn add_band(&mut self, host: &dyn ParamHost, p: Point) {
        self.add_bell(host, xf(p.x), yg(p.y).clamp(-18.0, 18.0), 1.0);
    }

    /// Adds a Bell in the first free band and selects it.
    fn add_bell(&mut self, host: &dyn ParamHost, freq: f64, gain: f64, q: f64) {
        let Some(b) = (0..BANDS).find(|&b| !self.band(host, b).enabled) else { return };
        let ids = &self.ids[b];
        Self::set(host, &ids.kind, 0.0);
        Self::set(host, &ids.freq, freq.clamp(FREQ_MIN, FREQ_MAX) as f32);
        Self::set(host, &ids.gain, gain.clamp(-30.0, 30.0) as f32);
        Self::set(host, &ids.q, q as f32);
        Self::set(host, &ids.on, 1.0);
        self.select(Some(b));
    }

    /// The colour the next added band will wear (a stereo band).
    fn new_band_color(theme: &mite_ui::Theme) -> Color {
        theme.band_ramp[Placement::Stereo as usize % theme.band_ramp.len()]
    }

    /// The spectrum peaks are found on: the held curve while peak hold is on, otherwise the
    /// spectrum as drawn (post).
    fn grab_curve(&self) -> &[f64] {
        if self.holding { &self.held } else { &self.post }
    }

    /// Curve `v` (dB per grid point) at `x` as [`Self::curve_path`] draws it: the same uniform
    /// Catmull-Rom spline, so what the pointer meets is the line on screen.
    fn drawn_at(v: &[f64], x: f64) -> f64 {
        let last = v.len() - 1;
        let pos = (x / grid_x(1)).clamp(0.0, last as f64);
        let k = (pos as usize).min(last - 1);
        let t = pos - k as f64;
        let (p0, p1, p2, p3) = (v[k.saturating_sub(1)], v[k], v[k + 1], v[(k + 2).min(last)]);
        0.5 * (2.0 * p1 + (p2 - p0) * t + (2.0 * p0 - 5.0 * p1 + 4.0 * p2 - p3) * t * t + (3.0 * p1 - p0 - 3.0 * p2 + p3) * t * t * t)
    }

    /// The peaks of curve `v` (dB per grid point): local maxima standing out by
    /// [`PEAK_PROMINENCE_DB`], each placed between grid points by a parabola through its
    /// neighbours, at the level the drawn line has there (so its marker sits on the line).
    fn peaks(v: &[f64]) -> Vec<Peak> {
        let last = v.len() - 1;
        // A third of an octave in grid points.
        let third = ((last as f64) / (FREQ_MAX / FREQ_MIN).log2() / 3.0).round() as usize;
        let mut out = Vec::new();
        for i in 1..last {
            if v[i] < v[i - 1] || v[i] < v[i + 1] || v[i] < -80.0 {
                continue;
            }
            let left = v[i.saturating_sub(third)..i].iter().cloned().fold(f64::INFINITY, f64::min);
            let right = v[i + 1..(i + 1 + third).min(last + 1)].iter().cloned().fold(f64::INFINITY, f64::min);
            if v[i] - left.max(right) < PEAK_PROMINENCE_DB {
                continue;
            }
            let (a, b, c) = (v[i - 1], v[i], v[i + 1]);
            let curve = a - 2.0 * b + c;
            let d = if curve < 0.0 { (0.5 * (a - c) / curve).clamp(-0.5, 0.5) } else { 0.0 };
            let x = grid_x(i) + d * grid_x(1);
            out.push(Peak { freq: xf(x), db: Self::drawn_at(v, x) });
        }
        out
    }

    /// Whether `p` is under the spectrum: inside its filled area, below the line (the held line
    /// while peak hold is on, so the pointer can sit between it and the live one). Only there
    /// does the spectrum answer the pointer (peak hold, peak markers, grabbing).
    fn under_spectrum(&self, p: Point) -> bool {
        if p.x < 0.0 || p.x >= PLOT_W || p.y < PLOT_TOP || p.y >= HELP_TOP {
            return false;
        }
        let db = Self::drawn_at(self.grab_curve(), p.x);
        // Silence draws no spectrum, so there is nothing to be under.
        db > -88.0 && p.y >= sy(db.clamp(-90.0, 6.0)) - SPECTRUM_EDGE
    }

    /// The peak nearest `p`, within [`PEAK_REACH`], while `p` is under the spectrum.
    fn find_peak(&self, p: Point) -> Option<Peak> {
        if !self.under_spectrum(p) {
            return None;
        }
        Self::peaks(self.grab_curve())
            .into_iter()
            .map(|peak| (Point::new(fx(peak.freq), sy(peak.db)).distance(p), peak))
            .filter(|(d, _)| *d <= PEAK_REACH)
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .map(|(_, peak)| peak)
    }

    /// Lays out the panel dials: the core row at `origin`, the dynamics row (if shown) from `dyn_top`.
    fn build_panel_knobs(&mut self, host: &dyn ParamHost, origin: Point, dyn_top: Option<f64>) {
        let Some(b) = self.selected else { return };
        let s = self.band(host, b);
        let ids = &self.ids[b];
        let mut col = 0.0;
        // Dials in a row on the right of the panel, clear of the buttons.
        let count = if s.kind.has_gain() { 3.0 } else { 2.0 };
        let first = origin.x + PANEL_LAST_DIAL_X - (count - 1.0) * PANEL_DIAL_STEP;
        let mut knob = |spec, label, kind, id: &str| {
            let k = Knob::new(spec, label, kind, Point::new(first + col * PANEL_DIAL_STEP, origin.y + 44.0), PANEL_DIAL)
                .with_id(id);
            col += 1.0;
            k
        };
        let mut knobs = vec![knob(FREQ, "FREQ", KnobKind::Unipolar, &ids.freq)];
        if s.kind.has_gain() {
            knobs.push(knob(GAIN, "GAIN", KnobKind::Bipolar, &ids.gain));
        }
        knobs.push(knob(Q, "Q", KnobKind::Unipolar, &ids.q));
        if let Some(dyn_top) = dyn_top {
            // Dynamics row: the scope on the left, then smaller dials. The range has no dial: it
            // is the gain dial's ring, the scope and the grip on the plot.
            let mut col = 0.0;
            let x0 = origin.x + 14.0 + SCOPE_W + 12.0 + 30.0 + 52.0;
            // Five dials (spectral compressor) sit closer together than four.
            let step = if s.dynamics.mode == DynamicsMode::Compressor { 70.0 } else { 82.0 };
            let mut small = |spec, label, kind, id: &str| {
                let k = Knob::new(spec, label, kind, Point::new(x0 + col * step, dyn_top + 38.0), 48.0)
                    .with_id(id);
                col += 1.0;
                k
            };
            if s.dynamics.mode == DynamicsMode::Transient {
                knobs.push(small(TRANSIENT, "ATTACK", KnobKind::Bipolar, &ids.transient));
                knobs.push(small(SUSTAIN, "SUSTAIN", KnobKind::Bipolar, &ids.sustain));
                knobs.push(small(TTIME, "TIME", KnobKind::Unipolar, &ids.ttime));
                knobs.push(small(TRANSITION, "TRANSITION", KnobKind::Unipolar, &ids.transition));
            } else {
                // Relative: the threshold is dB over the surroundings, its own parameter.
                if s.is_spectral() && s.dynamics.relative {
                    knobs.push(small(RTHRESH, "THRESHOLD", KnobKind::Unipolar, &ids.rthresh));
                } else {
                    knobs.push(small(THRESH, "THRESHOLD", KnobKind::Unipolar, &ids.thresh));
                }
                knobs.push(small(ATTACK, "ATTACK", KnobKind::Unipolar, &ids.attack).with_curve(ATTACK_DIAL));
                knobs.push(small(RELEASE, "RELEASE", KnobKind::Unipolar, &ids.release).with_curve(RELEASE_DIAL));
                knobs.push(small(RATIO, "RATIO", KnobKind::Unipolar, &ids.ratio));
                if s.is_spectral() {
                    knobs.push(small(SSMOOTH, "SMOOTHING", KnobKind::Unipolar, &ids.ssmooth));
                }
            }
        }
        self.panel_knobs = knobs;
    }

    /// Dials the panel shows for `s`.
    fn panel_dial_count(s: &BandSettings, show_dynamics: bool) -> usize {
        // Every dynamics row has four dials; the spectral compressor adds smoothing.
        let dynamics = if s.is_spectral() { 5 } else { 4 };
        (if s.kind.has_gain() { 3 } else { 2 }) + if show_dynamics { dynamics } else { 0 }
    }

    fn panel_height(s: &BandSettings) -> f64 {
        PANEL_H + if s.is_dynamic() { PANEL_DYN_H } else { 0.0 }
    }

    /// The dynamics mode button that is on: compressor, spectral compressor or transient shaper.
    fn mode_index(s: &BandSettings) -> usize {
        match s.dynamics.mode {
            DynamicsMode::Transient => 2,
            DynamicsMode::Compressor if s.dynamics.spectral => 1,
            DynamicsMode::Compressor => 0,
        }
    }

    /// The panel at its dock (x, and whether it hangs from the bottom of the plot) for `s`.
    fn docked_rect(s: &BandSettings, x: f64, bottom: bool) -> Rect {
        let height = Self::panel_height(s);
        let y = if bottom { HELP_TOP - height - 10.0 } else { DISPLAY_TOP + 10.0 };
        Rect::new(x, y, x + PANEL_W, y + height)
    }

    /// Where the band panel goes: docked at the bottom centre of the plot, unless that would hide
    /// the band's node or range; then to the bottom corner away from it, and failing
    /// that the top.
    fn panel_rect(s: &BandSettings) -> Rect {
        let height = Self::panel_height(s);
        let p = Self::node_pos(s);
        let (mut top, mut bottom) = (p.y, p.y);
        for g in Self::grips(s) {
            top = top.min(g.y);
            bottom = bottom.max(g.y);
        }
        let keep_out = [Rect::new(p.x - 30.0, top - 10.0, p.x + 30.0, bottom + 10.0)];
        let (left, right) = (PANEL_MARGIN, PLOT_W - PANEL_MARGIN - PANEL_W);
        let away = if p.x > PLOT_W / 2.0 { [left, right] } else { [right, left] };
        let xs = [(PLOT_W - PANEL_W) / 2.0, away[0], away[1]];
        for y in [HELP_TOP - height - 10.0, DISPLAY_TOP + 10.0] {
            for x in xs {
                let r = Rect::new(x, y, x + PANEL_W, y + height);
                if keep_out.iter().all(|k| r.intersect(*k).area() <= 0.0) {
                    return r;
                }
            }
        }
        let y = HELP_TOP - height - 10.0;
        Rect::new(xs[0], y, xs[0] + PANEL_W, y + height)
    }

    /// The band's live dynamic gain, dB (0 when not dynamic). For a spectral compressor, its
    /// strongest per-bin gain.
    fn dynamic_now(&self, b: usize, s: &BandSettings) -> f64 {
        if s.is_dynamic() || s.uses_spectrum() { f64::from(self.last_dynamic[b]) } else { 0.0 }
    }

    /// The spectral stage's live gain at `f`, dB: Catmull-Rom through the engine's log-spaced points.
    fn spectral_at(&self, f: f64) -> f64 {
        Self::points_at(&self.last_spectral, f)
    }

    /// Catmull-Rom through values at the [`crate::spectral_point_freq`] points, held between
    /// neighbours.
    fn points_at(points: &[f32; SPECTRAL_POINTS], f: f64) -> f64 {
        let last = SPECTRAL_POINTS - 1;
        let pos = ((f / FREQ_MIN).ln() / (FREQ_MAX / FREQ_MIN).ln() * last as f64).clamp(0.0, last as f64);
        let k = (pos as usize).min(last - 1);
        let t = pos - k as f64;
        let v = |i: usize| f64::from(points[i.min(last)]);
        let (p0, p1, p2, p3) = (v(k.saturating_sub(1)), v(k), v(k + 1), v(k + 2));
        let v = 0.5 * ((2.0 * p1) + (p2 - p0) * t + (2.0 * p0 - 5.0 * p1 + 4.0 * p2 - p3) * t * t + (3.0 * p1 - p0 - 3.0 * p2 + p3) * t * t * t);
        // Held between its neighbours: no overshoot the engine does not apply.
        v.clamp(p1.min(p2), p1.max(p2))
    }

    /// The plot grips that set a band's dynamic range: the end of the range when the band is
    /// dynamic, otherwise one above the node (lift when loud) and one below it (duck). A transient
    /// band has none: its movement follows the sound's shape, set in the panel.
    fn grips(s: &BandSettings) -> Vec<Point> {
        if !s.enabled || !s.kind.has_gain() || Self::is_transient(s) {
            return Vec::new();
        }
        let p = Self::node_pos(s);
        if s.is_dynamic() {
            return vec![Point::new(p.x, gy((s.gain_db + s.dynamics.range_db).clamp(-18.0, 18.0)))];
        }
        vec![
            Point::new(p.x, (p.y - GRIP_OFFSET).max(PLOT_TOP + 6.0)),
            Point::new(p.x, (p.y + GRIP_OFFSET).min(PLOT_BOT - 6.0)),
        ]
    }

    /// Whether `s` is a dynamic band in transient mode.
    fn is_transient(s: &BandSettings) -> bool {
        s.is_dynamic() && s.dynamics.mode == DynamicsMode::Transient
    }

    /// The dynamics control at `p`, for the selected band.
    fn hot_at(&self, host: &dyn ParamHost, p: Point) -> Option<Hot> {
        let s = self.band(host, self.selected?);
        if !s.enabled {
            return None;
        }
        if let Some(h) = self.panel_hits.filter(|h| h.panel.contains(p)) {
            if !Self::is_transient(&s) && h.ring.is_some_and(|c| (RING_HIT.0..=RING_HIT.1).contains(&c.distance(p))) {
                return Some(Hot::Ring);
            }
            if let Some(r) = h.scope.filter(|r| r.contains(p)) {
                if Self::is_transient(&s) && (p.x - scope_x(r, s.dynamics.transient_ms / 1000.0)).abs() <= 6.0 {
                    return Some(Hot::Split);
                }
                return Some(Hot::Scope);
            }
            if let Some(i) = h.modes.and_then(|m| m.iter().position(|r| r.contains(p))) {
                return Some(Hot::Mode(i));
            }
            if h.remove_dynamics.is_some_and(|r| r.contains(p)) {
                return Some(Hot::RemoveDynamics);
            }
            if h.relative.is_some_and(|r| r.contains(p)) {
                return Some(Hot::Relative);
            }
            return None;
        }
        // A grip wins over the node only when the pointer is nearer to it.
        let node = Self::node_pos(&s).distance(p);
        if let Some(g) = Self::grips(&s).into_iter().find(|g| g.distance(p) <= GRIP_HIT && g.distance(p) < node) {
            return Some(Hot::Grip(g));
        }
        None
    }

    /// Host ids a drag of `mode` changes, for its gestures.
    fn drag_ids(ids: &BandIds, mode: DragMode, has_gain: bool) -> Vec<&str> {
        match mode {
            DragMode::Move if has_gain => vec![&ids.freq, &ids.gain],
            DragMode::Move => vec![&ids.freq],
            DragMode::Width => vec![&ids.q],
            DragMode::Range | DragMode::Ring => vec![&ids.range, &ids.dyn_on],
            DragMode::Scope => vec![&ids.thresh, &ids.range, &ids.dyn_on],
            DragMode::TransientAttack => vec![&ids.transient, &ids.dyn_on],
            DragMode::Sustain => vec![&ids.sustain, &ids.dyn_on],
            DragMode::Split => vec![&ids.ttime],
            DragMode::SpectralThreshold => vec![&ids.thresh, &ids.rthresh],
        }
    }

    /// Starts a drag of band `b`; `range` is where a range drag starts from.
    fn begin_drag(&mut self, host: &dyn ParamHost, b: usize, mode: DragMode, pos: Point, range: f64) {
        let s = self.band(host, b);
        if matches!(mode, DragMode::Range | DragMode::Ring) && !s.is_dynamic() {
            // The ring and the grips are compressor gestures: a band made dynamic with them is one.
            Self::set(host, &self.ids[b].dmode, 0.0);
        }
        for id in Self::drag_ids(&self.ids[b], mode, s.kind.has_gain()) {
            host.begin_gesture(id);
        }
        self.drag = Some(NodeDrag {
            band: b,
            mode,
            last: pos,
            log_freq: s.freq.ln(),
            gain: s.gain_db,
            log_q: s.q.ln(),
            range,
            thresh: s.dynamics.threshold_db,
            sustain: s.dynamics.sustain_db,
            transient: s.dynamics.transient_db,
            relative: s.dynamics.relative_db,
        });
    }

    /// The range a band can take: its gain plus the range stays on the gain dial.
    fn range_limits(host: &dyn ParamHost, ids: &BandIds) -> (f64, f64) {
        let gain = f64::from(Self::plain(host, &ids.gain));
        ((-GAIN_LIMIT_DB - gain).max(f64::from(RANGE.min)), (GAIN_LIMIT_DB - gain).min(f64::from(RANGE.max)))
    }

    /// Sets a band's dynamic range (within [`Self::range_limits`]) and switches dynamics to match.
    fn apply_range(host: &dyn ParamHost, ids: &BandIds, range: f64) {
        let (lo, hi) = Self::range_limits(host, ids);
        host.set_normalized(&ids.range, host.normalize(&ids.range, range.clamp(lo, hi) as f32));
        Self::update_dynamics_on(host, ids);
    }

    /// Dynamics are on while the band has somewhere to move: a range, or in transient mode an
    /// attack or sustain amount, away from zero.
    fn update_dynamics_on(host: &dyn ParamHost, ids: &BandIds) {
        let transient = Self::plain(host, &ids.dmode).round() >= 1.0;
        let away = |id: &str| f64::from(Self::plain(host, id)).abs() >= RANGE_OFF_DB;
        let on = if transient { away(&ids.transient) || away(&ids.sustain) } else { away(&ids.range) };
        if (Self::plain(host, &ids.dyn_on) >= 0.5) != on {
            host.set_normalized(&ids.dyn_on, host.normalize(&ids.dyn_on, if on { 1.0 } else { 0.0 }));
        }
    }

    /// Makes band `b` static.
    fn remove_dynamics(&mut self, host: &dyn ParamHost, b: usize) {
        Self::set(host, &self.ids[b].dyn_on, 0.0);
        self.panel_knobs.clear();
        self.hot = None;
    }

    // ---- drawing ------------------------------------------------------------------------

    fn draw_grid(&self, cx: &mut DrawCx<'_>) {
        let theme = cx.theme;
        let (mut minor, mut major) = (BezPath::new(), BezPath::new());
        for dec in [10.0, 100.0, 1000.0, 10000.0] {
            for k in 1..=9 {
                let f = dec * f64::from(k);
                if f > FREQ_MAX {
                    break;
                }
                let x = fx(f).round() + 0.5;
                let path = if k == 1 { &mut major } else { &mut minor };
                path.move_to((x, DISPLAY_TOP));
                path.line_to((x, HELP_TOP));
            }
        }
        for d in [-15.0, -9.0, -3.0, 3.0, 9.0, 15.0] {
            minor.move_to((0.0, gy(d).round() + 0.5));
            minor.line_to((PLOT_W, gy(d).round() + 0.5));
        }
        for d in [-12.0, -6.0, 6.0, 12.0] {
            major.move_to((0.0, gy(d).round() + 0.5));
            major.line_to((PLOT_W, gy(d).round() + 0.5));
        }
        let hair = Stroke::new(1.0);
        cx.scene.stroke(&hair, Affine::IDENTITY, theme.grid_minor, None, &minor);
        cx.scene.stroke(&hair, Affine::IDENTITY, theme.grid_major, None, &major);
        cx.scene.stroke(&hair, Affine::IDENTITY, theme.grid_zero, None, &Line::new((0.0, MID + 0.5), (PLOT_W, MID + 0.5)));
        cx.scene.stroke(&hair, Affine::IDENTITY, theme.line, None, &Line::new((PLOT_W + 0.5, DISPLAY_TOP), (PLOT_W + 0.5, HELP_TOP)));

        let label = TextStyle::READOUT.sized(10.0);
        for (f, t) in [(20.0, "20"), (50.0, "50"), (100.0, "100"), (200.0, "200"), (500.0, "500"), (1000.0, "1k"), (2000.0, "2k"), (5000.0, "5k"), (10000.0, "10k"), (20000.0, "20k")] {
            cx.text.draw(cx.scene, t, fx(f), DISPLAY_TOP + 596.0, label, theme.text_lo, Align::Center);
        }
        for d in [12, 6, 0, -6, -12] {
            let t = match d.cmp(&0) {
                std::cmp::Ordering::Greater => format!("+{d}"),
                std::cmp::Ordering::Less => format!("−{}", -d),
                std::cmp::Ordering::Equal => "0".into(),
            };
            let color = if d == 0 { theme.text_mid } else { theme.text_lo };
            cx.text.draw(cx.scene, &t, 1206.0, gy(f64::from(d)) - 7.0, label, color, Align::Left);
        }
    }

    /// A smooth curve through the grid values (Catmull-Rom as cubic Béziers): no corners at the
    /// points, so the analyzer reads as a line, not a polyline.
    fn curve_path(values: &[f64], map: impl Fn(f64) -> f64) -> BezPath {
        let pts: Vec<Point> =
            values.iter().enumerate().filter(|(_, v)| v.is_finite()).map(|(i, &v)| Point::new(grid_x(i), map(v))).collect();
        let mut path = BezPath::new();
        let Some(&first) = pts.first() else { return path };
        path.move_to(first);
        let n = pts.len();
        for i in 0..n.saturating_sub(1) {
            let (p0, p1, p2, p3) = (pts[i.saturating_sub(1)], pts[i], pts[i + 1], pts[(i + 2).min(n - 1)]);
            path.curve_to(p1 + (p2 - p0) / 6.0, p2 - (p3 - p1) / 6.0, p2);
        }
        path
    }

    /// A smooth curve through `pts` (x increasing): a monotone cubic (Fritsch–Butland tangents),
    /// so it bends through every point without corners and never overshoots past its neighbours
    /// (it shows no boost or cut that is not there). Appended to `path`, starting with a move or
    /// a line to the first point.
    fn smooth_through(path: &mut BezPath, pts: &[Point], start_with_line: bool) {
        let mut kept: Vec<Point> = Vec::with_capacity(pts.len());
        for &p in pts {
            if kept.last().is_none_or(|q| (p.x - q.x).abs() > 1e-6) {
                kept.push(p);
            }
        }
        let Some(&first) = kept.first() else { return };
        if start_with_line {
            path.line_to(first);
        } else {
            path.move_to(first);
        }
        let n = kept.len();
        let slope = |i: usize| (kept[i + 1].y - kept[i].y) / (kept[i + 1].x - kept[i].x);
        let tangent = |i: usize| {
            if n < 2 {
                0.0
            } else if i == 0 {
                slope(0)
            } else if i == n - 1 {
                slope(n - 2)
            } else {
                let (a, b) = (slope(i - 1), slope(i));
                if a * b <= 0.0 { 0.0 } else { 2.0 / (1.0 / a + 1.0 / b) }
            }
        };
        for i in 0..n.saturating_sub(1) {
            let (p, q) = (kept[i], kept[i + 1]);
            let h = (q.x - p.x) / 3.0;
            path.curve_to((p.x + h, p.y + tangent(i) * h), (q.x - h, q.y - tangent(i + 1) * h), (q.x, q.y));
        }
    }

    fn xy_path(xs: &[(f64, Option<usize>)], values: &[f64], map: impl Fn(f64) -> f64) -> BezPath {
        let mut path = BezPath::new();
        for (i, (&(x, _), &v)) in xs.iter().zip(values).enumerate() {
            if i == 0 {
                path.move_to((x, map(v)));
            } else {
                path.line_to((x, map(v)));
            }
        }
        path
    }

    fn draw_spectrum(&self, cx: &mut DrawCx<'_>) {
        let theme = cx.theme;
        // Silence (or no audio yet): draw nothing rather than a line along the floor.
        let clamp = |db: f64| sy(db.clamp(-90.0, 6.0));
        // The live sidechain (Match shows what it learned instead).
        if self.sc_heard && self.matcher.is_none() {
            let sc = Self::curve_path(&self.sc, clamp);
            cx.scene.stroke(&Stroke::new(1.2), Affine::IDENTITY, theme.sidechain.multiply_alpha(0.7), None, &sc);
        }
        if self.pre.iter().chain(&self.post).all(|&v| v < -88.0) {
            return;
        }
        let pre = Self::curve_path(&self.pre, clamp);
        cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, theme.spectrum_pre.multiply_alpha(0.5), None, &pre);

        let post = Self::curve_path(&self.post, clamp);
        let mut fill = post.clone();
        fill.line_to((PLOT_W, HELP_TOP));
        fill.line_to((0.0, HELP_TOP));
        fill.close_path();
        let gradient = Gradient::new_linear((0.0, PLOT_TOP), (0.0, HELP_TOP)).with_stops([
            (0.0, theme.accent.multiply_alpha(0.34)),
            (0.55, theme.spectrum_post_fill.multiply_alpha(0.12)),
            (1.0, theme.spectrum_post_fill.multiply_alpha(0.0)),
        ]);
        cx.scene.fill(Fill::NonZero, Affine::IDENTITY, &gradient, None, &fill);
        cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, theme.accent.multiply_alpha(0.55), None, &post);
    }

    fn draw_curves(&mut self, cx: &mut DrawCx<'_>, bands: &[BandSettings]) {
        let theme = cx.theme;
        let fs = self.sample_rate();
        // The fixed grid plus every band's own frequency: a notch's zero (or a narrow bell's peak)
        // falls between grid points, so without it the drawn depth changed as the band moved.
        let mut xs: Vec<(f64, Option<usize>)> = (0..POINTS).map(|i| (grid_x(i), Some(i))).collect();
        for s in bands.iter().filter(|s| s.enabled) {
            let x = fx(s.freq);
            if x > 0.0 && x < PLOT_W {
                xs.push((x, None));
            }
        }
        xs.sort_by(|a, b| a.0.total_cmp(&b.0));
        let mut total = vec![0.0_f64; xs.len()];
        let mut any = false;
        // Curves may leave the plot (the display is clipped); clamping only keeps paths finite.
        let clamp = |db: f64| gy(if db.is_nan() { -40.0 } else { db.clamp(-40.0, 40.0) });
        let mut values = vec![0.0_f64; xs.len()];
        for (b, s) in bands.iter().enumerate() {
            if !s.enabled {
                continue;
            }
            any = true;
            // While a band is soloed the others fade back.
            let dim = if self.soloed.is_some_and(|o| o != b) { 0.35 } else { 1.0 };
            let color = Self::band_color(theme, s).multiply_alpha(dim);
            let selected = self.selected == Some(b);
            if s.is_dynamic() && !Self::is_transient(s) {
                // Where the dynamics can take the band: from the static curve to static + range.
                let static_curve = self.curve_at(s, &xs, fs);
                let full = self.curve_at(&BandSettings { gain_db: s.gain_db + s.dynamics.range_db, ..*s }, &xs, fs);
                let mut region = Self::xy_path(&xs, &full, clamp);
                for (&(x, _), &v) in xs.iter().zip(&static_curve).rev() {
                    region.line_to((x, clamp(v)));
                }
                region.close_path();
                cx.scene.fill(Fill::NonZero, Affine::IDENTITY, color.multiply_alpha(0.14), None, &region);
                // The static curve as the region's edge. Solid: Vello mis-renders dashes along
                // long many-segment paths (a thick bar appeared along the line).
                cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, color.multiply_alpha(0.5), None, &Self::xy_path(&xs, &static_curve, clamp));
            }
            // The band as it is right now, including its live dynamic gain (a spectral band's live
            // movement is per bin, added below for all of them at once).
            let moving = if s.uses_spectrum() { 0.0 } else { self.dynamic_now(b, s) };
            let live = BandSettings { gain_db: s.gain_db + moving, ..*s };
            let linear = self.linear;
            let r = self.response(b, &live);
            for (v, &(x, grid)) in values.iter_mut().zip(&xs) {
                *v = match grid {
                    Some(i) => r.grid[i],
                    None => band_db(&r.sections, &live, xf(x), fs, linear),
                };
            }
            for (t, v) in total.iter_mut().zip(&values) {
                *t += v;
            }
            let mut fill = Self::xy_path(&xs, &values, clamp);
            fill.line_to((PLOT_W, MID));
            fill.line_to((0.0, MID));
            fill.close_path();
            cx.scene.fill(Fill::NonZero, Affine::IDENTITY, color.multiply_alpha(if selected { 0.22 } else { 0.08 }), None, &fill);
        }
        // The spectral stage's live per-bin gain: part of the curve, and lit between the curve
        // without it and with it, so what it takes out (or adds) shows where it happens.
        if self.last_spectral.iter().any(|v| v.abs() > 0.05) {
            let without = total.clone();
            for (t, &(x, _)) in total.iter_mut().zip(&xs) {
                *t += self.spectral_at(xf(x));
            }
            let forward: Vec<Point> = xs.iter().zip(&total).map(|(&(x, _), &v)| Point::new(x, clamp(v))).collect();
            let back: Vec<Point> = xs.iter().zip(&without).rev().map(|(&(x, _), &v)| Point::new(-x, clamp(v))).collect();
            let mut lit = BezPath::new();
            Self::smooth_through(&mut lit, &forward, false);
            // The way back, mirrored so x increases, then mirrored again into place.
            let mut tail = BezPath::new();
            Self::smooth_through(&mut tail, &back, false);
            for el in (Affine::FLIP_X * tail).elements() {
                lit.push(match *el {
                    mite_ui::kurbo::PathEl::MoveTo(p) => mite_ui::kurbo::PathEl::LineTo(p),
                    other => other,
                });
            }
            lit.close_path();
            cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.curve.multiply_alpha(0.28), None, &lit);
        }
        let curve = if any {
            let pts: Vec<Point> = xs.iter().zip(&total).map(|(&(x, _), &v)| Point::new(x, clamp(v))).collect();
            let mut path = BezPath::new();
            Self::smooth_through(&mut path, &pts, false);
            path
        } else {
            let mut flat = BezPath::new();
            flat.move_to((0.0, MID));
            flat.line_to((PLOT_W, MID));
            flat
        };
        glow_stroke(cx, &curve, 2.2, theme.curve, theme.curve, 5.0);
    }

    /// Spectrum grab: the marker on the peak under the pointer, and the ghost Bell being dragged
    /// out of one.
    fn draw_grab(&self, cx: &mut DrawCx<'_>) {
        let theme = cx.theme;
        // Peak hold: the accumulated maxima, a faint dot on each peak.
        let a = f64::from(self.hold_alpha);
        if a > 0.01 {
            let held = Self::curve_path(&self.held, |db: f64| sy(db.clamp(-90.0, 6.0)));
            cx.scene.stroke(&Stroke::new(1.2), Affine::IDENTITY, theme.needle.multiply_alpha((0.45 * a) as f32), None, &held);
            for peak in Self::peaks(&self.held) {
                let dot = Circle::new((fx(peak.freq), sy(peak.db)), 2.5);
                cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.needle.multiply_alpha((0.55 * a) as f32), None, &dot);
            }
        }
        let (p, alpha) = self.peak_marker;
        if alpha > 0.01 {
            glow_stroke(cx, &Circle::new(p, 5.0), 1.5, theme.needle.multiply_alpha(alpha), theme.accent.multiply_alpha(alpha), 6.0);
        }
        let Some(g) = &self.grab else { return };
        let s = BandSettings { enabled: true, kind: BandType::Bell, freq: g.freq, gain_db: g.gain, q: PEAK_Q, ..Default::default() };
        let color = Self::new_band_color(theme);
        let xs: Vec<(f64, Option<usize>)> = (0..POINTS).map(|i| (grid_x(i), None)).collect();
        let values = self.curve_at(&s, &xs, self.sample_rate());
        let clamp = |db: f64| gy(if db.is_nan() { 0.0 } else { db.clamp(-40.0, 40.0) });
        let curve = Self::xy_path(&xs, &values, clamp);
        let mut fill = curve.clone();
        fill.line_to((PLOT_W, MID));
        fill.line_to((0.0, MID));
        fill.close_path();
        cx.scene.fill(Fill::NonZero, Affine::IDENTITY, color.multiply_alpha(0.12), None, &fill);
        cx.scene.stroke(&Stroke::new(1.5), Affine::IDENTITY, color.multiply_alpha(0.7), None, &curve);
        let node = Point::new(fx(s.freq), gy(g.gain.clamp(-18.0, 18.0)));
        cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.display.multiply_alpha(0.6), None, &Circle::new(node, 7.0));
        glow_stroke(cx, &Circle::new(node, 7.0), 1.6, color.multiply_alpha(0.8), color, 4.0);
    }

    /// A band's drawn response at `xs`, designed fresh (not cached).
    fn curve_at(&self, s: &BandSettings, xs: &[(f64, Option<usize>)], fs: f64) -> Vec<f64> {
        let sections = design(s, fs);
        xs.iter().map(|&(x, _)| band_db(&sections, s, xf(x), fs, self.linear)).collect()
    }

    fn draw_nodes(&self, cx: &mut DrawCx<'_>, bands: &[BandSettings]) {
        let theme = cx.theme;
        let mut order: Vec<usize> = (0..BANDS).collect();
        if let Some(sel) = self.selected {
            order.retain(|&b| b != sel);
            order.push(sel);
        }
        for b in order {
            let s = &bands[b];
            if !s.enabled {
                continue;
            }
            let dim = if self.soloed.is_some_and(|o| o != b) { 0.35 } else { 1.0 };
            let color = Self::band_color(theme, s).multiply_alpha(dim);
            let p = Self::node_pos(s);
            let selected = self.selected == Some(b);
            let hovered = self.hover == Some(b);
            if self.soloed == Some(b) {
                // The soloed band wears a white ring.
                glow_stroke(cx, &Circle::new(p, 13.0), 1.6, theme.needle, theme.needle, 5.0);
            }
            let r = if selected { 9.0 } else if hovered { 8.0 } else { 7.0 };
            if s.is_dynamic() {
                // A track from the node to where the range ends, filled as far as the band has
                // moved right now: a meter living on the curve.
                let transient = Self::is_transient(s);
                let end = gy((s.gain_db + s.dynamics.range_db).clamp(-18.0, 18.0));
                let now = gy((s.gain_db + self.dynamic_now(b, s)).clamp(-18.0, 18.0));
                let bar = |a: f64, z: f64| RoundedRect::new(p.x - 3.0, a.min(z), p.x + 3.0, a.max(z), 3.0);
                // A transient band has no range to show, only its live motion.
                if !transient {
                    cx.scene.fill(Fill::NonZero, Affine::IDENTITY, color.multiply_alpha(0.2), None, &bar(p.y, end));
                }
                if (now - p.y).abs() > 0.5 {
                    cx.scene.fill(Fill::NonZero, Affine::IDENTITY, color.multiply_alpha(0.85), None, &bar(p.y, now));
                    glow_stroke(cx, &Circle::new((p.x, now), 3.5), 1.5, theme.needle, color, 5.0);
                }
                if !selected && !transient {
                    cx.scene.fill(Fill::NonZero, Affine::IDENTITY, color.multiply_alpha(0.8), None, &Circle::new((p.x, end), 3.0));
                }
            }
            if selected {
                cx.scene.fill(Fill::NonZero, Affine::IDENTITY, color.multiply_alpha(0.2), None, &Circle::new(p, r + 5.0));
                glow_stroke(cx, &Circle::new(p, r - 1.0), 2.0, theme.needle, color, 6.0);
                cx.scene.fill(Fill::NonZero, Affine::IDENTITY, color, None, &Circle::new(p, r - 2.0));
                let num = format!("{}", b + 1);
                cx.text.draw(cx.scene, &num, p.x, p.y - 6.5, TextStyle::LABEL.sized(9.0).weighted(800.0), Color::from_rgba8(14, 17, 22, 255), Align::Center);
            } else {
                if hovered {
                    cx.scene.fill(Fill::NonZero, Affine::IDENTITY, color.multiply_alpha(0.13), None, &Circle::new(p, r + 6.0));
                }
                cx.scene.fill(Fill::NonZero, Affine::IDENTITY, color.multiply_alpha(if hovered { 0.33 } else { 0.2 }), None, &Circle::new(p, r - 1.0));
                glow_stroke(cx, &Circle::new(p, r - 1.0), 2.0, color, color, 3.0);
            }
        }
        self.draw_grips(cx, bands);
    }

    /// The selected band's range grips (see [`Self::grips`]).
    fn draw_grips(&self, cx: &mut DrawCx<'_>, bands: &[BandSettings]) {
        let Some(b) = self.selected else { return };
        let s = &bands[b];
        if self.drag.as_ref().is_some_and(|d| matches!(d.mode, DragMode::Move | DragMode::Width)) {
            return;
        }
        let theme = cx.theme;
        let color = Self::band_color(theme, s);
        let p = Self::node_pos(s);
        let dragging = self.drag.as_ref().is_some_and(|d| d.mode == DragMode::Range);
        let round = Stroke::new(1.4).with_caps(mite_ui::kurbo::Cap::Round).with_join(mite_ui::kurbo::Join::Round);
        for g in Self::grips(s) {
            let hot = dragging || self.hot == Some(Hot::Grip(g));
            if s.is_dynamic() {
                // A pill handle at the end of the range, ridged so it reads as grabbable.
                let pill = RoundedRect::new(g.x - 10.0, g.y - 5.5, g.x + 10.0, g.y + 5.5, 5.5);
                cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.display, None, &pill);
                glow_stroke(cx, &pill, 1.6, color, color, if hot { 6.0 } else { 2.5 });
                let mut ridges = BezPath::new();
                for dx in [-3.5, 0.0, 3.5] {
                    ridges.move_to((g.x + dx, g.y - 2.0));
                    ridges.line_to((g.x + dx, g.y + 2.0));
                }
                cx.scene.stroke(&Stroke::new(1.2), Affine::IDENTITY, if hot { theme.needle } else { color }, None, &ridges);
            } else {
                // A ghost grip on a thread from the node, its arrow pointing the way it moves the band.
                let up = g.y < p.y;
                let dir = if up { -1.0 } else { 1.0 };
                let alpha = if hot { 1.0 } else { 0.55 };
                cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, color.multiply_alpha(0.3 * alpha), None, &Line::new((p.x, p.y + dir * 11.0), (g.x, g.y - dir * 7.0)));
                cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.display.multiply_alpha(0.85), None, &Circle::new(g, 7.0));
                if hot {
                    glow_stroke(cx, &Circle::new(g, 7.0), 1.4, color, color, 5.0);
                } else {
                    cx.scene.stroke(&Stroke::new(1.2), Affine::IDENTITY, color.multiply_alpha(alpha), None, &Circle::new(g, 7.0));
                }
                let mut arrow = BezPath::new();
                arrow.move_to((g.x - 3.0, g.y - dir * 1.5));
                arrow.line_to((g.x, g.y + dir * 1.8));
                arrow.line_to((g.x + 3.0, g.y - dir * 1.5));
                cx.scene.stroke(&round, Affine::IDENTITY, color.multiply_alpha(alpha), None, &arrow);
            }
        }
    }

    /// The gain dial's dynamics ring: the range as an arc from the gain to where the dynamics can
    /// take it, filled live as far as the band has moved. On a static band it stays hidden until
    /// the pointer comes near, then shows arrows inviting a pull either way.
    fn draw_ring(&self, cx: &mut DrawCx<'_>, c: Point, b: usize, s: &BandSettings, color: Color) {
        let theme = cx.theme;
        let id = &self.ids[b].gain;
        let to_deg = |db: f64| -135.0 + 270.0 * f64::from(cx.host.normalize(id, db as f32));
        let arc = |from: f64, to: f64| mite_ui::kurbo::Arc::new(c, (RING_R, RING_R), (from - 90.0).to_radians(), (to - from).to_radians(), 0.0);
        let polar = |deg: f64| {
            let a = (deg - 90.0).to_radians();
            Point::new(c.x + RING_R * a.cos(), c.y + RING_R * a.sin())
        };
        let dragging = self.drag.as_ref().is_some_and(|d| d.mode == DragMode::Ring);
        let hot = dragging || self.hot == Some(Hot::Ring);
        let near = self.pointer.is_some_and(|p| p.distance(c) <= RING_HIT.1);
        let gain = to_deg(s.gain_db);
        let round = Stroke::new(3.0).with_caps(mite_ui::kurbo::Cap::Round);
        if Self::is_transient(s) {
            // Not a control in transient mode: just the band's live motion.
            let now = to_deg(s.gain_db + self.dynamic_now(b, s));
            if (now - gain).abs() > 0.5 {
                glow_stroke(cx, &arc(gain, now), 3.0, color, color, 3.0);
            }
        } else if s.is_dynamic() {
            let end = to_deg(s.gain_db + s.dynamics.range_db);
            let now = to_deg(s.gain_db + self.dynamic_now(b, s));
            cx.scene.stroke(&round, Affine::IDENTITY, theme.tick_off.multiply_alpha(0.5), None, &arc(-135.0, 135.0));
            cx.scene.stroke(&round, Affine::IDENTITY, color.multiply_alpha(0.35), None, &arc(gain, end));
            if (now - gain).abs() > 0.5 {
                glow_stroke(cx, &arc(gain, now), 3.0, color, color, 3.0);
            }
            let grip = polar(end);
            cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.needle, None, &Circle::new(grip, if hot { 5.0 } else { 4.0 }));
            glow_stroke(cx, &Circle::new(grip, if hot { 5.0 } else { 4.0 }), 1.5, color, color, if hot { 5.0 } else { 2.0 });
        } else if near || hot {
            let alpha = if hot { 1.0 } else { 0.6 };
            cx.scene.stroke(&Stroke::new(2.0).with_caps(mite_ui::kurbo::Cap::Round), Affine::IDENTITY, color.multiply_alpha(0.25 * alpha), None, &arc(-135.0, 135.0));
            cx.scene.fill(Fill::NonZero, Affine::IDENTITY, color.multiply_alpha(alpha), None, &Circle::new(polar(gain), 3.5));
            // Arrows along the ring either side of the grip.
            let mut arrows = BezPath::new();
            for side in [-1.0, 1.0] {
                let a = (gain + side * 16.0 - 90.0).to_radians();
                let q = polar(gain + side * 16.0);
                let (tx, ty) = (-a.sin() * side, a.cos() * side);
                let (nx, ny) = (a.cos(), a.sin());
                arrows.move_to((q.x - tx * 1.5 + nx * 3.0, q.y - ty * 1.5 + ny * 3.0));
                arrows.line_to((q.x + tx * 1.5, q.y + ty * 1.5));
                arrows.line_to((q.x - tx * 1.5 - nx * 3.0, q.y - ty * 1.5 - ny * 3.0));
            }
            let stroke = Stroke::new(1.4).with_caps(mite_ui::kurbo::Cap::Round).with_join(mite_ui::kurbo::Join::Round);
            cx.scene.stroke(&stroke, Affine::IDENTITY, color.multiply_alpha(alpha), None, &arrows);
        }
    }

    /// The dynamics scope, framed; its contents are clipped to the frame, glow included.
    fn draw_scope(&self, cx: &mut DrawCx<'_>, r: Rect, b: usize, s: &BandSettings, color: Color) {
        let theme = cx.theme;
        panel(cx.scene, RoundedRect::from_rect(r, 8.0), theme.well, theme.well_border);
        cx.scene.push_clip_layer(Fill::NonZero, Affine::IDENTITY, &RoundedRect::from_rect(r.inset(-1.0), 7.0));
        match s.dynamics.mode {
            _ if s.is_spectral() => self.draw_spectral_scope(cx, r, s, color),
            DynamicsMode::Compressor => self.draw_compressor_scope(cx, r, b, s, color),
            DynamicsMode::Transient => self.draw_transient_scope(cx, r, s, color),
        }
        cx.scene.pop_layer();
    }

    /// Compressor scope: how far the band moves (up or down from the middle line) as its level
    /// rises from quiet (left) to loud (right), with a dot riding the curve live. The vertical
    /// scale is fixed (square-root, ±30 dB at the edges), so a bigger range looks bigger.
    fn draw_compressor_scope(&self, cx: &mut DrawCx<'_>, r: Rect, b: usize, s: &BandSettings, color: Color) {
        let theme = cx.theme;
        let range = if s.is_dynamic() { s.dynamics.range_db } else { 0.0 };
        let (mid, half) = (r.center().y, 0.5 * r.height() - 6.0);
        let x_of = |db: f64| r.x0 + (db.clamp(SCOPE_FLOOR_DB, 0.0) - SCOPE_FLOOR_DB) / -SCOPE_FLOOR_DB * r.width();
        let y_of = |g: f64| mid - g.signum() * (g.abs() / GAIN_LIMIT_DB).min(1.0).sqrt() * half;
        let mut grid = BezPath::new();
        for g in [-24.0, -12.0, -6.0, 6.0, 12.0, 24.0] {
            grid.move_to((r.x0, y_of(g)));
            grid.line_to((r.x1, y_of(g)));
        }
        cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, theme.grid_minor, None, &grid);
        cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, theme.grid_major, None, &Line::new((r.x0, mid), (r.x1, mid)));
        let tx = x_of(s.dynamics.threshold_db);
        cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, color.multiply_alpha(0.3), None, &Line::new((tx, r.y0), (tx, r.y1)));
        let dynamics = Dynamics { range_db: range, ..s.dynamics };
        let mut curve = BezPath::new();
        for i in 0..=64 {
            let db = SCOPE_FLOOR_DB * (1.0 - f64::from(i) / 64.0);
            let pt = (x_of(db), y_of(dynamics.gain_db(db)));
            if i == 0 {
                curve.move_to(pt);
            } else {
                curve.line_to(pt);
            }
        }
        let mut fill = curve.clone();
        fill.line_to((r.x1, mid));
        fill.line_to((r.x0, mid));
        fill.close_path();
        cx.scene.fill(Fill::NonZero, Affine::IDENTITY, color.multiply_alpha(0.18), None, &fill);
        glow_stroke(cx, &curve, 1.8, color, color, 3.0);
        let level = f64::from(self.last_level);
        if s.is_dynamic() && level > SCOPE_FLOOR_DB {
            let lx = x_of(level);
            cx.scene.fill(Fill::NonZero, Affine::IDENTITY, color.multiply_alpha(0.5), None, &RoundedRect::new(r.x0 + 3.0, r.y1 - 5.0, lx, r.y1 - 3.0, 1.0));
            let dot = Point::new(lx, y_of(self.dynamic_now(b, s)));
            cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.needle, None, &Circle::new(dot, 4.0));
            glow_stroke(cx, &Circle::new(dot, 4.0), 1.5, color, color, 5.0);
        }
    }

    /// Transient scope: one hit's envelope over time, as it comes in (faint) and as the band shapes
    /// it (lit). The hit runs through the band's own detector ([`ShapeDetector`]), so the curve is
    /// what the band does: a softer attack rounds the onset, more sustain slows the decay. The
    /// part under the pointer (attack or tail) is tinted, since that is what a drag there changes.
    fn draw_transient_scope(&self, cx: &mut DrawCx<'_>, r: Rect, s: &BandSettings, color: Color) {
        let theme = cx.theme;
        let (attack, sustain) = if s.is_dynamic() { (s.dynamics.transient_db, s.dynamics.sustain_db) } else { (0.0, 0.0) };
        // A short silence, then a 3 ms rise into a decay of 60 dB/s, peaking at −6 dBFS.
        const RATE: f64 = 2000.0;
        let hit = |t: f64| if t < 0.0 { 0.0 } else { (1.0 - (-t / 0.003).exp()) * (-t / 0.145).exp() };
        let steps = (SCOPE_LEN_S * RATE) as usize;
        let time = |i: usize| i as f64 / RATE - SCOPE_PRE_S;
        let peak = (0..steps).map(|i| hit(time(i))).fold(0.0, f64::max);
        let mut detector = ShapeDetector::new(RATE, s.dynamics.transient_ms, s.dynamics.transition);
        let (mut input, mut output) = (Vec::with_capacity(steps), Vec::with_capacity(steps));
        for i in 0..steps {
            let level = hit(time(i)) / peak;
            detector.process(0.5 * level);
            let db = 20.0 * level.max(1e-4).log10();
            input.push(db);
            output.push(db + detector.gain_db(attack, sustain));
        }

        let pad = SCOPE_PAD;
        let x_of = |i: usize| scope_x(r, time(i));
        // +12 dB at the top and −40 dB at the bottom, widened to fit the shaped hit (a big attack
        // boost, a deep tail cut) with 3 dB to spare, so its top is never cut off.
        let high = output.iter().copied().fold(12.0, f64::max).min(GAIN_LIMIT_DB) + 3.0;
        let low = output.iter().copied().fold(-40.0, f64::min).max(-80.0);
        let y_of = |db: f64| r.y0 + pad + (high - db.clamp(low, high)) / (high - low) * (r.height() - 2.0 * pad);
        let path = |values: &[f64]| {
            let mut p = BezPath::new();
            for (i, &v) in values.iter().enumerate() {
                let pt = (x_of(i), y_of(v));
                if i == 0 {
                    p.move_to(pt);
                } else {
                    p.line_to(pt);
                }
            }
            p
        };
        let split = scope_x(r, s.dynamics.transient_ms / 1000.0);
        if let Some(p) = self.pointer.filter(|p| r.contains(*p) && self.hot != Some(Hot::Split)) {
            let part = if p.x < split { Rect::new(r.x0, r.y0, split, r.y1) } else { Rect::new(split, r.y0, r.x1, r.y1) };
            cx.scene.fill(Fill::NonZero, Affine::IDENTITY, color.multiply_alpha(0.07), None, &part);
        }
        cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, theme.grid_major, None, &Line::new((r.x0, y_of(0.0)), (r.x1, y_of(0.0))));
        let (out, inp) = (path(&output), path(&input));
        let mut fill = out.clone();
        fill.line_to((x_of(steps - 1), r.y1));
        fill.line_to((x_of(0), r.y1));
        fill.close_path();
        cx.scene.fill(Fill::NonZero, Affine::IDENTITY, color.multiply_alpha(0.12), None, &fill);
        // What the band changes: the area between the hit as it comes in and as it leaves.
        let mut change = out.clone();
        for i in (0..steps).rev() {
            change.line_to((x_of(i), y_of(input[i])));
        }
        change.close_path();
        cx.scene.fill(Fill::EvenOdd, Affine::IDENTITY, color.multiply_alpha(0.35), None, &change);
        cx.scene.stroke(&Stroke::new(1.2), Affine::IDENTITY, theme.text_lo, None, &inp);
        glow_stroke(cx, &out, 1.8, color, color, 3.0);
        // The crossfade from attack to sustain, shaded around the line.
        let width = (s.dynamics.transition * s.dynamics.transient_ms).max(1.5) / 1000.0;
        let (z0, z1) = (scope_x(r, s.dynamics.transient_ms / 1000.0 - 0.5 * width), scope_x(r, s.dynamics.transient_ms / 1000.0 + 0.5 * width));
        cx.scene.fill(Fill::NonZero, Affine::IDENTITY, color.multiply_alpha(0.1), None, &Rect::new(z0, r.y0, z1, r.y1));
        // Where the attack hands over to the sustain: a line with a grip at its top.
        let hot = self.hot == Some(Hot::Split) || self.drag.as_ref().is_some_and(|d| d.mode == DragMode::Split);
        let line = color.multiply_alpha(if hot { 0.8 } else { 0.35 });
        cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, line, None, &Line::new((split, r.y0 + 14.0), (split, r.y1)));
        let grip = RoundedRect::new(split - 3.0, r.y0 + 4.0, split + 3.0, r.y0 + 14.0, 3.0);
        cx.scene.fill(Fill::NonZero, Affine::IDENTITY, if hot { color } else { color.multiply_alpha(0.6) }, None, &grip);
        if hot {
            let text = TTIME.format(s.dynamics.transient_ms as f32);
            let (x, align) = if split > r.center().x { (split - 7.0, Align::Right) } else { (split + 7.0, Align::Left) };
            cx.text.draw(cx.scene, &text, x, r.y0 + 3.0, TextStyle::READOUT.sized(10.0), theme.text_hi, align);
        }
    }

    /// Spectral compressor scope: the band's frequencies (three octaves either side of it, left to
    /// right) and how loud each is right now (bottom −60 dBFS, top 0), against the threshold
    /// line. Each frequency is compressed on its own: where the level (faint) rises over the line,
    /// the band pulls just that part down, and the level after it (lit) shows by how much. The
    /// levels are the engine's own per-bin detector; drag the line to set the threshold.
    fn draw_spectral_scope(&self, cx: &mut DrawCx<'_>, r: Rect, s: &BandSettings, color: Color) {
        let theme = cx.theme;
        let pad = SCOPE_PAD;
        let (f0, f1) = ((s.freq / 8.0).max(20.0), (s.freq * 8.0).min(20_000.0));
        let x_of = |f: f64| r.x0 + (f / f0).ln() / (f1 / f0).ln() * r.width();
        let y_of = |db: f64| r.y0 + pad + (db.clamp(SCOPE_FLOOR_DB, 0.0) / SCOPE_FLOOR_DB) * (r.height() - 2.0 * pad);
        let mut grid = BezPath::new();
        for db in [-12.0, -24.0, -36.0, -48.0] {
            grid.move_to((r.x0, y_of(db)));
            grid.line_to((r.x1, y_of(db)));
        }
        cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, theme.grid_minor, None, &grid);
        // The band's centre, and how much of each frequency it works on (its region), as a floor glow.
        let cx_line = x_of(s.freq);
        cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, color.multiply_alpha(0.2), None, &Line::new((cx_line, r.y0), (cx_line, r.y1)));
        const N: usize = 72;
        let freq = |i: usize| f0 * (f1 / f0).powf(i as f64 / (N - 1) as f64);
        let input: Vec<Point> = (0..N).map(|i| Point::new(x_of(freq(i)), y_of(Self::points_at(&self.scope_level, freq(i))))).collect();
        let output: Vec<Point> = (0..N).map(|i| Point::new(x_of(freq(i)), y_of(Self::points_at(&self.scope_out, freq(i))))).collect();
        let heard = self.scope_level.iter().any(|&l| f64::from(l) > SCOPE_FLOOR_DB);
        if heard {
            let mut inp = BezPath::new();
            Self::smooth_through(&mut inp, &input, false);
            let mut out = BezPath::new();
            Self::smooth_through(&mut out, &output, false);
            // Under the level after the band: a quiet fill down to the floor.
            let mut fill = out.clone();
            fill.line_to((r.x1, r.y1));
            fill.line_to((r.x0, r.y1));
            fill.close_path();
            cx.scene.fill(Fill::NonZero, Affine::IDENTITY, color.multiply_alpha(0.12), None, &fill);
            // What the band takes off (or adds): between the level before and after it.
            let mut change = out.clone();
            let back: Vec<Point> = input.iter().rev().map(|p| Point::new(-p.x, p.y)).collect();
            let mut tail = BezPath::new();
            Self::smooth_through(&mut tail, &back, false);
            for el in (Affine::FLIP_X * tail).elements() {
                change.push(match *el {
                    mite_ui::kurbo::PathEl::MoveTo(p) => mite_ui::kurbo::PathEl::LineTo(p),
                    other => other,
                });
            }
            change.close_path();
            cx.scene.fill(Fill::EvenOdd, Affine::IDENTITY, color.multiply_alpha(0.4), None, &change);
            cx.scene.stroke(&Stroke::new(1.2), Affine::IDENTITY, theme.text_lo, None, &inp);
            glow_stroke(cx, &out, 1.6, color, color, 3.0);
        }
        // The threshold, with a grip on its right end: a line across at a fixed level, or
        // (relative) a curve riding the surroundings, the engine's own per-bin thresholds.
        let hot = self.hot == Some(Hot::Scope) || self.drag.as_ref().is_some_and(|d| d.mode == DragMode::SpectralThreshold);
        let line_color = color.multiply_alpha(if hot { 0.9 } else { 0.55 });
        let ty = if s.dynamics.relative {
            let edge = x_of(freq(N - 1)) - 14.0;
            let pts: Vec<Point> = (0..N)
                .map(|i| Point::new(x_of(freq(i)), y_of(Self::points_at(&self.scope_threshold, freq(i)))))
                .filter(|p| p.x <= edge)
                .collect();
            if heard && pts.len() > 1 {
                let mut path = BezPath::new();
                Self::smooth_through(&mut path, &pts, false);
                cx.scene.stroke(&Stroke::new(1.2), Affine::IDENTITY, line_color, None, &path);
                pts[pts.len() - 1].y
            } else {
                // Nothing heard yet: the threshold over silence, flat.
                let y = y_of(SCOPE_FLOOR_DB + s.dynamics.relative_db);
                cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, line_color, None, &Line::new((r.x0, y), (r.x1 - 14.0, y)));
                y
            }
        } else {
            let y = y_of(s.dynamics.threshold_db);
            cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, line_color, None, &Line::new((r.x0, y), (r.x1 - 14.0, y)));
            y
        };
        let grip = RoundedRect::new(r.x1 - 14.0, ty - 3.0, r.x1 - 4.0, ty + 3.0, 3.0);
        cx.scene.fill(Fill::NonZero, Affine::IDENTITY, if hot { color } else { color.multiply_alpha(0.7) }, None, &grip);
    }

    /// Icons for the dynamics modes, 16 × 16 at `at`: a compressor's bent transfer line, the same
    /// over a row of frequency bins (spectral), and a hit's spike and decay.
    fn draw_mode_icon(scene: &mut mite_ui::vello::Scene, mode: usize, at: Point, color: Color) {
        let pt = |x: f64, y: f64| (at.x + x, at.y + y);
        let mut p = BezPath::new();
        if mode == 0 {
            p.move_to(pt(1.0, 15.0));
            p.line_to(pt(8.0, 8.0));
            p.quad_to(pt(10.0, 6.0), pt(15.0, 5.0));
        } else if mode == 1 {
            p.move_to(pt(1.0, 11.0));
            p.line_to(pt(6.0, 6.0));
            p.quad_to(pt(8.0, 4.0), pt(15.0, 3.0));
            for (x, top) in [(2.5, 13.0), (6.0, 10.0), (9.5, 12.0), (13.0, 9.5)] {
                p.move_to(pt(x, 15.5));
                p.line_to(pt(x, top));
            }
        } else {
            p.move_to(pt(0.0, 14.0));
            p.line_to(pt(3.0, 14.0));
            p.line_to(pt(5.0, 2.0));
            p.curve_to(pt(7.0, 9.0), pt(10.0, 12.0), pt(16.0, 13.0));
        }
        let stroke = Stroke::new(1.6).with_caps(mite_ui::kurbo::Cap::Round).with_join(mite_ui::kurbo::Join::Round);
        scene.stroke(&stroke, Affine::IDENTITY, color, None, &p);
    }

    fn draw_panel(&mut self, cx: &mut DrawCx<'_>, bands: &[BandSettings]) {
        let Some(b) = self.selected else { return };
        let s = bands[b];
        if !s.enabled {
            return;
        }
        let theme = cx.theme;
        // The panel holds still while anything is dragged, so nothing moves under the pointer.
        let locked = self.drag.is_some() || self.panel_knobs.iter().any(Knob::is_dragging);
        // The dock is chosen when a band is selected or moved on the plot, never by edits made in
        // the panel itself.
        let (x, bottom) = *self.panel_dock.get_or_insert_with(|| {
            let r = Self::panel_rect(&s);
            (r.x0, r.y1 >= HELP_TOP - 11.0)
        });
        let rect = match self.panel_hits {
            Some(h) if locked => h.panel,
            _ => Self::docked_rect(&s, x, bottom),
        };
        let show_dynamics = rect.height() > PANEL_H;
        // The core row stays beside the edge the panel is docked to and the dynamics row grows
        // into the plot, so turning dynamics on never moves the controls under the pointer.
        let docked_bottom = bottom;
        let (origin, dyn_top, divider) = if !show_dynamics {
            (rect.origin(), None, 0.0)
        } else if docked_bottom {
            (Point::new(rect.x0, rect.y0 + PANEL_DYN_H), Some(rect.y0 + 4.0), rect.y0 + PANEL_DYN_H)
        } else {
            (rect.origin(), Some(rect.y0 + PANEL_H - 2.0), rect.y0 + PANEL_H - 4.0)
        };
        if self.panel_knobs.len() != Self::panel_dial_count(&s, show_dynamics) || self.knobs_layout != Some((origin, dyn_top)) {
            self.build_panel_knobs(cx.host, origin, dyn_top);
            self.knobs_layout = Some((origin, dyn_top));
        }
        let shape = RoundedRect::from_rect(rect, 14.0);
        if theme.shadows {
            cx.scene.draw_blurred_rounded_rect(Affine::translate((0.0, 14.0)), rect, Color::from_rgba8(0, 0, 0, 140), 14.0, 20.0);
        }
        glass_panel(cx, shape, theme.blur_panel);

        // Left: band title over a row of dropdown buttons.
        let color = Self::band_color(theme, &s);
        let dot = Point::new(origin.x + 22.0, origin.y + 24.0);
        glow_stroke(cx, &Circle::new(dot, 4.0), 2.0, color, color, 4.0);
        cx.scene.fill(Fill::NonZero, Affine::IDENTITY, color, None, &Circle::new(dot, 5.0));
        let title = format!("Band {}", b + 1);
        cx.text.draw(cx.scene, &title, origin.x + 34.0, origin.y + 16.0, TextStyle::TITLE.sized(13.0), theme.text_hi, Align::Left);

        let row_y = origin.y + 54.0;
        let open = self.menu.as_ref().map(|m| m.kind);
        let pointer = self.pointer;
        let dropdown = |cx: &mut DrawCx<'_>, x: f64, w: f64, text: &str, icon: Icon, active: bool| -> Rect {
            let r = Rect::new(x, row_y, x + w, row_y + 32.0);
            let border = if active || pointer.is_some_and(|p| r.contains(p)) { color } else { cx.theme.panel_control_border };
            raised(cx.scene, RoundedRect::from_rect(r, 6.0), cx.theme.panel_control, border, cx.theme.shadows);
            draw_icon(cx.scene, icon, Point::new(x + 10.0, row_y + 10.0), color);
            cx.text.draw(cx.scene, text, x + 40.0, row_y + 9.0, TextStyle::BODY.weighted(600.0), cx.theme.text_hi, Align::Left);
            let mut chevron = BezPath::new();
            let (cxr, cy) = (r.x1 - 14.0, r.center().y);
            chevron.move_to((cxr - 4.0, cy - 2.0));
            chevron.line_to((cxr, cy + 2.0));
            chevron.line_to((cxr + 4.0, cy - 2.0));
            let round = Stroke::new(1.5).with_caps(mite_ui::kurbo::Cap::Round).with_join(mite_ui::kurbo::Join::Round);
            cx.scene.stroke(&round, Affine::IDENTITY, cx.theme.text_mid, None, &chevron);
            r
        };
        let mut x = origin.x + 14.0;
        let kind = dropdown(cx, x, 150.0, BandType::NAMES[s.kind as usize], Icon::Type(s.kind), open == Some(MenuKind::Type));
        x += 158.0;
        let slope = s.kind.has_slope().then(|| {
            let i = s.slope.min(SLOPE.choices.len() - 1);
            let r = dropdown(cx, x, 124.0, SLOPE.choices[i], Icon::Slope(s.kind, i), open == Some(MenuKind::Slope));
            x += 132.0;
            r
        });
        let stereo = dropdown(cx, x, 118.0, Placement::NAMES[s.placement as usize], Icon::Stereo(s.placement), open == Some(MenuKind::Stereo));
        // Solo: listen to the band's region alone (a tilt reaches everything, so has none).
        let solo = (!matches!(s.kind, BandType::TiltShelf | BandType::FlatTilt)).then(|| {
            let r = Rect::new(stereo.x1 + 8.0, row_y, stereo.x1 + 40.0, row_y + 32.0);
            let on = self.soloed == Some(b);
            let hot = self.pointer.is_some_and(|p| r.contains(p));
            // Lit white while on (the soloed node's ring is white too).
            let ink = icon_button(cx.scene, theme, r, theme.panel_control, hot, on, if on { theme.needle } else { color });
            draw_headphones(cx.scene, r.center(), ink);
            r
        });

        let (scope, modes, remove_dynamics, relative, source) = if let Some(top) = dyn_top {
            cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, theme.line, None, &Line::new((origin.x + 14.0, divider), (origin.x + PANEL_W - 14.0, divider)));
            let scope = Rect::new(origin.x + 14.0, top + 8.0, origin.x + 14.0 + SCOPE_W, top + 8.0 + SCOPE_H);
            self.draw_scope(cx, scope, b, &s, color);
            // Mode buttons, stacked beside the scope: compressor, spectral compressor, transient shaper.
            let mx = scope.x1 + 12.0;
            let h = (SCOPE_H - 8.0) / 3.0;
            let modes: [Rect; 3] = std::array::from_fn(|i| {
                let y = scope.y0 + i as f64 * (h + 4.0);
                Rect::new(mx, y, mx + 30.0, y + h)
            });
            let current = Self::mode_index(&s);
            for (i, m) in modes.iter().enumerate() {
                let on = current == i;
                let hot = self.hot == Some(Hot::Mode(i));
                let icon = icon_button(cx.scene, theme, *m, theme.panel_control, hot, on, color);
                Self::draw_mode_icon(cx.scene, i, m.center() - (8.0, 8.0), icon);
            }
            let remove = Rect::new(origin.x + PANEL_W - 44.0, top + 23.0, origin.x + PANEL_W - 14.0, top + 53.0);
            let ink = icon_button(cx.scene, theme, remove, theme.panel_control, self.hot == Some(Hot::RemoveDynamics), false, color);
            // A level line with its range arrow struck through: "no dynamics".
            let k = remove.center();
            let mut glyph = BezPath::new();
            glyph.move_to((k.x - 6.0, k.y + 3.0));
            glyph.line_to((k.x + 6.0, k.y + 3.0));
            glyph.move_to((k.x, k.y + 1.0));
            glyph.line_to((k.x, k.y - 6.0));
            glyph.move_to((k.x - 2.5, k.y - 3.5));
            glyph.line_to((k.x, k.y - 6.0));
            glyph.line_to((k.x + 2.5, k.y - 3.5));
            glyph.move_to((k.x - 6.0, k.y - 6.0));
            glyph.line_to((k.x + 6.0, k.y + 6.0));
            let stroke = Stroke::new(1.4).with_caps(mite_ui::kurbo::Cap::Round).with_join(mite_ui::kurbo::Join::Round);
            cx.scene.stroke(&stroke, Affine::IDENTITY, ink, None, &glyph);
            // Spectral: below it, the threshold's kind — a fixed level, or following the
            // surroundings (lit). Its icon: a line riding the tops of the bins.
            let relative = s.is_spectral().then(|| {
                let r = Rect::new(remove.x0, top + 58.0, remove.x1, top + 88.0);
                let on = s.dynamics.relative;
                let ink = icon_button(cx.scene, theme, r, theme.panel_control, self.hot == Some(Hot::Relative), on, color);
                let k = r.center();
                let mut bars = BezPath::new();
                let tops = [3.0, -2.0, 1.5, -4.0, 2.0];
                for (i, t) in tops.iter().enumerate() {
                    let x = k.x - 6.0 + 3.0 * i as f64;
                    bars.move_to((x, k.y + 7.0));
                    bars.line_to((x, k.y + t));
                }
                let mut line = BezPath::new();
                line.move_to((k.x - 8.0, k.y - 1.0));
                line.curve_to((k.x - 4.0, k.y - 5.0), (k.x - 1.0, k.y - 2.0), (k.x + 1.0, k.y - 5.0));
                line.curve_to((k.x + 3.0, k.y - 8.0), (k.x + 6.0, k.y - 3.0), (k.x + 8.0, k.y - 4.0));
                let stroke = Stroke::new(1.4).with_caps(mite_ui::kurbo::Cap::Round).with_join(mite_ui::kurbo::Join::Round);
                cx.scene.stroke(&stroke, Affine::IDENTITY, ink.multiply_alpha(0.6), None, &bars);
                cx.scene.stroke(&stroke, Affine::IDENTITY, ink, None, &line);
                r
            });
            // Plain compressor: what the detector listens to, right of the dials.
            let plain = s.dynamics.mode == DynamicsMode::Compressor && !s.is_spectral();
            let source = plain.then(|| {
                let r = Rect::new(remove.x0 - 106.0, top + 22.0, remove.x0 - 8.0, top + 54.0);
                let open = self.menu.as_ref().is_some_and(|m| m.kind == MenuKind::Source);
                let hot = self.pointer.is_some_and(|p| r.contains(p));
                raised(cx.scene, RoundedRect::from_rect(r, 6.0), theme.panel_control, if open || hot { color } else { theme.panel_control_border }, theme.shadows);
                let label = DETECT_NAMES[s.dynamics.source.index().min(DETECT_NAMES.len() - 1)];
                draw_source_icon(cx.scene, s.dynamics.source, Point::new(r.x0 + 6.0, r.center().y - 6.0), color);
                cx.text.draw(cx.scene, label, r.x0 + 30.0, r.y0 + 9.0, TextStyle::BODY.sized(11.0).weighted(600.0), theme.text_hi, Align::Left);
                let mut chevron = BezPath::new();
                let (cxr, cy) = (r.x1 - 12.0, r.center().y);
                chevron.move_to((cxr - 3.5, cy - 1.5));
                chevron.line_to((cxr, cy + 2.0));
                chevron.line_to((cxr + 3.5, cy - 1.5));
                let round = Stroke::new(1.5).with_caps(mite_ui::kurbo::Cap::Round).with_join(mite_ui::kurbo::Join::Round);
                cx.scene.stroke(&round, Affine::IDENTITY, theme.text_mid, None, &chevron);
                r
            });
            (Some(scope), Some(modes), Some(remove), relative, source)
        } else {
            (None, None, None, None, None)
        };

        // Right: remove the band, under close.
        let round = Stroke::new(1.5).with_caps(mite_ui::kurbo::Cap::Round);
        let close = Rect::new(origin.x + PANEL_W - 44.0, origin.y + 12.0, origin.x + PANEL_W - 14.0, origin.y + 42.0);
        let remove = Rect::new(close.x0, origin.y + 58.0, close.x1, origin.y + 88.0);
        let hot = |r: Rect| self.pointer.is_some_and(|p| r.contains(p));
        let ink = icon_button(cx.scene, theme, remove, theme.panel_control, hot(remove), false, color);
        draw_bin(cx.scene, remove.center(), ink);
        let close_ink = icon_button(cx.scene, theme, close, theme.panel_control, hot(close), false, color);
        let mut cross = BezPath::new();
        let k = close.center();
        cross.move_to((k.x - 3.0, k.y - 3.0));
        cross.line_to((k.x + 3.0, k.y + 3.0));
        cross.move_to((k.x + 3.0, k.y - 3.0));
        cross.line_to((k.x - 3.0, k.y + 3.0));
        cx.scene.stroke(&round, Affine::IDENTITY, close_ink, None, &cross);

        for knob in &mut self.panel_knobs {
            knob.color = Some(color);
            knob.draw(cx);
        }
        let ring = if s.kind.has_gain() { self.panel_knobs.iter().find(|k| k.id == self.ids[b].gain).map(|k| k.center) } else { None };
        if let Some(c) = ring {
            self.draw_ring(cx, c, b, &s, color);
        }
        self.panel_hits = Some(PanelHits { panel: rect, kind, slope, stereo, ring, scope, modes, remove_dynamics, relative, source, solo, remove, close });
    }

    /// The open dropdown, drawn over everything.
    fn draw_menu(&self, cx: &mut DrawCx<'_>) {
        let Some(menu) = &self.menu else { return };
        let theme = cx.theme;
        let color = if menu.kind == MenuKind::Resolution { theme.accent } else { Self::band_color(theme, &self.band(cx.host, menu.band)) };
        menu.list.draw(cx, color, |scene, i, at, ink| draw_icon(scene, menu.icon(i), at, ink));
    }

    /// Opens (or, if it is already open, closes) a band's dropdown.
    fn toggle_menu(&mut self, kind: MenuKind, band: usize, anchor: Rect, host: &dyn ParamHost) {
        if self.menu.as_ref().is_some_and(|m| m.kind == kind) {
            self.menu = None;
            return;
        }
        let ids = &self.ids[band];
        let (items, id): (&'static [&'static str], &str) = match kind {
            MenuKind::Type => (&BandType::NAMES, &ids.kind),
            MenuKind::Slope => (SLOPE.choices, &ids.slope),
            MenuKind::Stereo => (&Placement::NAMES, &ids.stereo),
            MenuKind::Resolution => (&LP_RESOLUTIONS, LP_RES.id),
            MenuKind::Source => (&DETECT_NAMES, &ids.dsrc),
        };
        let current = Self::plain(host, id).round().max(0.0) as usize;
        let band_kind = BandType::from_index(Self::plain(host, &ids.kind).round().max(0.0) as usize);
        // Sources: own, sidechain, and the other bands that are on (and the current one).
        let shown = if kind == MenuKind::Source {
            (0..items.len())
                .filter(|&i| match DetectSource::from_index(i) {
                    DetectSource::Band(n) => n != band && (self.band(host, n).enabled || i == current),
                    _ => true,
                })
                .collect()
        } else {
            (0..items.len()).collect()
        };
        let width = if kind == MenuKind::Type { 184.0 } else { 150.0 };
        let list = Dropdown::new(anchor, items, current, (DISPLAY_TOP, HELP_TOP)).with_shown(shown).with_min_width(width);
        self.menu = Some(Menu { kind, band, band_kind, list });
    }

    /// Routes an event to the open dropdown. It is modal: everything is consumed, and a click
    /// outside closes it.
    fn menu_event(&mut self, event: &UiEvent, host: &dyn ParamHost) -> bool {
        let Some(menu) = &mut self.menu else { return false };
        if let UiEvent::PointerMove { pos, .. } = *event {
            self.pointer = Some(pos);
        }
        match menu.list.event(event) {
            DropdownEvent::Open => {}
            DropdownEvent::Picked(i) => {
                let ids = &self.ids[menu.band];
                let id: &str = match menu.kind {
                    MenuKind::Type => &ids.kind,
                    MenuKind::Slope => &ids.slope,
                    MenuKind::Stereo => &ids.stereo,
                    MenuKind::Resolution => LP_RES.id,
                    MenuKind::Source => &ids.dsrc,
                };
                Self::set(host, id, i as f32);
                if menu.kind == MenuKind::Type {
                    // Cuts are Butterworth (flat) at Q 0.71; a Q carried over from a bell would
                    // add a resonant bump.
                    if BandType::from_index(i).has_slope() {
                        Self::set(host, &ids.q, std::f32::consts::FRAC_1_SQRT_2);
                    }
                    self.panel_knobs.clear();
                }
                self.menu = None;
            }
            DropdownEvent::Closed => self.menu = None,
        }
        true
    }

    fn draw_footer(&mut self, cx: &mut DrawCx<'_>) {
        let theme = cx.theme;
        let top = FOOTER_TOP;
        self.footer.draw_frame(cx);

        // Processing mode.
        let mode = PhaseMode::from_index(Self::plain(cx.host, MODE.id).round().max(0.0) as usize);
        let track = Rect::new(12.0, top + 10.0, 12.0 + 364.0, top + 46.0);
        panel(cx.scene, RoundedRect::from_rect(track, 8.0), theme.well, theme.well_border);
        let mut x = track.x0 + 2.0;
        for (i, w) in [118.0, 118.0, 120.0].into_iter().enumerate() {
            let r = Rect::new(x, track.y0 + 2.0, x + w, track.y1 - 2.0);
            let on = mode as usize == i;
            let hovered = !on && self.pointer.is_some_and(|p| r.contains(p));
            if on {
                raised(cx.scene, RoundedRect::from_rect(r, 6.0), theme.control, theme.control_border, theme.shadows);
            }
            let color = if on {
                theme.text_strong
            } else if hovered {
                theme.text_hi
            } else {
                theme.text_mid
            };
            cx.text.draw(cx.scene, PhaseMode::NAMES[i], r.center().x, r.y0 + 9.0, TextStyle::BODY.weighted(700.0), color, Align::Center);
            self.mode_hits[i] = r;
            x += w + 2.0;
        }

        // Linear Phase: its resolution, as a dropdown beside the modes.
        let mut label_x = 392.0;
        self.resolution_hit = (mode == PhaseMode::LinearPhase).then(|| {
            let res = (Self::plain(cx.host, LP_RES.id).round().max(0.0) as usize).min(LP_RESOLUTIONS.len() - 1);
            let r = Rect::new(388.0, track.y0, 388.0 + 112.0, track.y1);
            let open = self.menu.as_ref().is_some_and(|m| m.kind == MenuKind::Resolution);
            let hot = self.pointer.is_some_and(|p| r.contains(p));
            draw_dropdown_button(cx, r, LP_RESOLUTIONS[res], open, hot, |scene, at| draw_icon(scene, Icon::Resolution(res), at, theme.accent));
            label_x = r.x1 + 16.0;
            r
        });

        // The latency the engine reports to the host.
        let latency_ms = f64::from(self.shared.latency.load()) / self.sample_rate() * 1000.0;
        self.footer.draw_latency(cx, label_x, latency_ms);
        self.footer.draw_controls(cx);
    }

    fn draw_header(&mut self, cx: &mut DrawCx<'_>) {
        let theme = cx.theme;
        cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.window, None, &Rect::new(0.0, 0.0, WIDTH, HEADER));
        cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, theme.line, None, &Line::new((0.0, HEADER - 0.5), (WIDTH, HEADER - 0.5)));
        draw_mark(cx, 16.0, 15.0, 26.0);
        draw_title(cx, 52.0, 18.0, "EQ");
        let (y0, y1) = HEADER_ROW;
        let hot = |r: Rect| self.pointer.is_some_and(|p| r.contains(p));
        self.session.draw(cx);

        // EQ Match.
        let r = Rect::new(MATCH_X.0, y0, MATCH_X.1, y1);
        let ink = icon_button(cx.scene, theme, r, theme.panel_control, hot(r), self.matcher.is_some(), theme.accent);
        draw_match_glyph(cx.scene, r.center(), ink);
    }

    /// Masking: each collision as a 45° hatch with lit edges (tokens.json `masking`), and the
    /// carve the one under the pointer suggests, as a ghost Bell and a dashed ring.
    fn draw_masking(&self, cx: &mut DrawCx<'_>) {
        let a = self.mask_alpha;
        if a <= 0.001 {
            return;
        }
        let theme = cx.theme;
        let color = theme.masking;
        let (top, bottom) = (PLOT_TOP, PLOT_BOT);
        for (i, (_, c)) in self.collisions.iter().enumerate() {
            let (x0, x1) = (fx(spectrum_point_freq(c.start)), fx(spectrum_point_freq(c.end)));
            let hot = self.collision_hot == Some(i);
            let area = Rect::new(x0, top, x1, bottom);
            cx.scene.push_clip_layer(Fill::NonZero, Affine::IDENTITY, &area);
            let mut hatch = BezPath::new();
            let h = bottom - top;
            let mut x = x0 - h;
            while x < x1 {
                hatch.move_to((x, bottom));
                hatch.line_to((x + h, top));
                x += 8.0;
            }
            cx.scene.stroke(&Stroke::new(3.0), Affine::IDENTITY, color.multiply_alpha(a * if hot { 0.3 } else { 0.22 }), None, &hatch);
            cx.scene.pop_layer();
            for x in [x0, x1] {
                cx.scene.stroke(&Stroke::new(1.5), Affine::IDENTITY, color.multiply_alpha(a * 0.9), None, &Line::new((x, top), (x, bottom)));
            }
        }
        let Some(i) = self.collision_hot.filter(|&i| i < self.collisions.len()) else { return };
        let s = self.carve(i);
        let clamp = |db: f64| gy(if db.is_nan() { 0.0 } else { db.clamp(-40.0, 40.0) });
        // Another instance: what it has been playing (its average), like the sidechain's trace.
        if let Some(src) = self.source(self.collisions[i].0).filter(|src| src.other != Other::Sidechain) {
            let mut avg = vec![0.0; POINTS];
            src.average.read_db(&mut avg);
            let path = Self::curve_path(&avg, |db: f64| sy(db.clamp(-90.0, 6.0)));
            cx.scene.stroke(&Stroke::new(1.2), Affine::IDENTITY, theme.sidechain.multiply_alpha(0.7 * a), None, &path);
        }
        let band = Self::new_band_color(theme);
        let xs: Vec<(f64, Option<usize>)> = (0..POINTS).map(|i| (grid_x(i), None)).collect();
        let values = self.curve_at(&s, &xs, self.sample_rate());
        let curve = Self::xy_path(&xs, &values, clamp);
        let mut fill = curve.clone();
        fill.line_to((PLOT_W, MID));
        fill.line_to((0.0, MID));
        fill.close_path();
        cx.scene.fill(Fill::NonZero, Affine::IDENTITY, band.multiply_alpha(0.1 * a), None, &fill);
        cx.scene.stroke(&Stroke::new(1.5), Affine::IDENTITY, band.multiply_alpha(0.6 * a), None, &curve);
        let ring = Circle::new(self.carve_pos(i), CARVE_RING);
        cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.display.multiply_alpha(0.6 * a), None, &ring);
        let dashed = Stroke::new(2.0).with_dashes(0.0, [4.0, 3.0]);
        cx.scene.stroke(&dashed, Affine::IDENTITY, theme.accent.multiply_alpha(a), None, &ring);
        glow_stroke(cx, &Circle::new(self.carve_pos(i), CARVE_RING - 4.0), 1.2, theme.accent.multiply_alpha(0.5 * a), theme.accent, 6.0);
        // Carve on the other instance: a dashed ring in its colour with an arrow leaving it, dim
        // while that instance cannot be written yet.
        if let Some(c) = self.carve_there_pos(i) {
            let reachable = matches!(self.collision_peer(i), Some((_, Some(_))));
            let ink = if reachable { theme.sidechain } else { theme.text_disabled };
            let ring = Circle::new(c, CARVE_RING);
            cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.display.multiply_alpha(0.6 * a), None, &ring);
            cx.scene.stroke(&dashed, Affine::IDENTITY, ink.multiply_alpha(a), None, &ring);
            let round = Stroke::new(1.5).with_caps(mite_ui::kurbo::Cap::Round).with_join(mite_ui::kurbo::Join::Round);
            let mut arrow = BezPath::new();
            arrow.move_to(c + (-4.0, 4.0));
            arrow.line_to(c + (4.0, -4.0));
            arrow.move_to(c + (-1.0, -4.0));
            arrow.line_to(c + (4.0, -4.0));
            arrow.line_to(c + (4.0, 1.0));
            cx.scene.stroke(&round, Affine::IDENTITY, ink.multiply_alpha(a), None, &arrow);
        }
    }

    /// EQ Match on the plot: the EQ it aims for (in the sidechain's colour, the reference) and
    /// the EQ with the fitted bands added (a ghost of the curve), with a dashed ring on each
    /// band it would add.
    fn draw_match(&mut self, cx: &mut DrawCx<'_>) {
        let Some(m) = &self.matcher else { return };
        if m.aim.is_empty() {
            return;
        }
        let (aim, ghost, fitted) = (m.aim.clone(), m.ghost.clone(), m.fitted.clone());
        let theme = cx.theme;
        let current = self.static_total(cx.host);
        let clamp = |db: f64| gy(if db.is_nan() { 0.0 } else { db.clamp(-40.0, 40.0) });
        // The aim, where the match listens (each heard stretch its own line).
        let mut i = 0;
        while i < POINTS {
            let Some(start) = (i..POINTS).find(|&k| aim[k].is_some()) else { break };
            let end = (start..POINTS).find(|&k| aim[k].is_none()).unwrap_or(POINTS);
            if end - start >= 2 {
                let xs: Vec<(f64, Option<usize>)> = (start..end).map(|k| (grid_x(k), None)).collect();
                let values: Vec<f64> = (start..end).map(|k| aim[k].unwrap_or(0.0)).collect();
                let path = Self::xy_path(&xs, &values, clamp);
                cx.scene.stroke(&Stroke::new(1.2), Affine::IDENTITY, theme.sidechain.multiply_alpha(0.75), None, &path);
            }
            i = end;
        }
        let total: Vec<f64> = current.iter().zip(&ghost).map(|(c, g)| c + g).collect();
        let ghost_path = Self::curve_path(&total, clamp);
        glow_stroke(cx, &ghost_path, 1.8, theme.accent.multiply_alpha(0.75), theme.accent, 5.0);
        let dashed = Stroke::new(1.6).with_dashes(0.0, [3.0, 3.0]);
        for s in &fitted {
            let p = Point::new(fx(s.freq), gy(s.gain_db.clamp(-18.0, 18.0)));
            cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.display.multiply_alpha(0.5), None, &Circle::new(p, 7.0));
            cx.scene.stroke(&dashed, Affine::IDENTITY, theme.accent, None, &Circle::new(p, 7.0));
        }
    }

    /// The Match panel, in the band panel's place: learn (with a ring filling as audio is
    /// learned) and start over, the three dials, then close and add.
    fn draw_match_panel(&mut self, cx: &mut DrawCx<'_>) {
        let theme = cx.theme;
        let pointer = self.pointer;
        let heard = self.sc_heard;
        let Some(m) = &mut self.matcher else { return };
        let rect = Match::rect();
        let o = rect.origin();
        if theme.shadows {
            cx.scene.draw_blurred_rounded_rect(Affine::translate((0.0, 14.0)), rect, Color::from_rgba8(0, 0, 0, 140), 14.0, 20.0);
        }
        glass_panel(cx, RoundedRect::from_rect(rect, 14.0), theme.blur_panel);
        let color = theme.accent;
        draw_match_glyph(cx.scene, Point::new(o.x + 22.0, o.y + 24.0), color);
        let width = cx.text.draw(cx.scene, "Match", o.x + 40.0, o.y + 16.0, TextStyle::TITLE.sized(13.0), theme.text_hi, Align::Left);
        let hot = |r: Rect| pointer.is_some_and(|p| r.contains(p));
        let round = Stroke::new(1.5).with_caps(mite_ui::kurbo::Cap::Round).with_join(mite_ui::kurbo::Join::Round);
        // The reference file's name (or the one being read), and a × that goes back to the sidechain.
        let shown = m.loading.as_ref().map(|(n, _)| format!("{n} …")).or_else(|| m.file.as_ref().map(|(n, _)| n.clone()));
        let mut drop = None;
        if let Some(name) = shown {
            let name: String = if name.chars().count() > 40 { name.chars().take(39).chain(['…']).collect() } else { name };
            let x = o.x + 40.0 + f64::from(width) + 12.0;
            let w = cx.text.draw(cx.scene, &name, x, o.y + 17.0, TextStyle::BODY.weighted(600.0), theme.text_mid, Align::Left);
            if m.file.is_some() && m.loading.is_none() {
                let r = Rect::new(x + f64::from(w) + 8.0, o.y + 12.0, x + f64::from(w) + 32.0, o.y + 36.0);
                let ink = icon_button(cx.scene, theme, r, theme.panel_control, hot(r), false, color);
                let k = r.center();
                let mut cross = BezPath::new();
                cross.move_to((k.x - 2.5, k.y - 2.5));
                cross.line_to((k.x + 2.5, k.y + 2.5));
                cross.move_to((k.x + 2.5, k.y - 2.5));
                cross.line_to((k.x - 2.5, k.y + 2.5));
                cx.scene.stroke(&round, Affine::IDENTITY, ink, None, &cross);
                drop = Some(r);
            }
        }

        // Learn: a dot while paused (press to learn), two bars while learning (press to pause),
        // inside a ring that fills as audio is learned.
        let learn = Rect::new(o.x + 14.0, o.y + 54.0, o.x + 46.0, o.y + 86.0);
        let ink = icon_button(cx.scene, theme, learn, theme.panel_control, hot(learn), m.learning, color);
        let c = learn.center();
        let ink = if heard || m.file.is_some() || !m.learning { ink } else { theme.text_mid };
        if m.learning {
            for dx in [-2.5, 2.5] {
                cx.scene.stroke(&round, Affine::IDENTITY, ink, None, &Line::new((c.x + dx, c.y - 4.0), (c.x + dx, c.y + 4.0)));
            }
        } else {
            cx.scene.fill(Fill::NonZero, Affine::IDENTITY, ink, None, &Circle::new(c, 3.5));
        }
        let full = (m.learned_s() / MATCH_FULL_S).min(1.0);
        cx.scene.stroke(&Stroke::new(1.5), Affine::IDENTITY, theme.tick_off, None, &Circle::new(c, 10.5));
        if full > 0.0 {
            let arc = mite_ui::kurbo::Arc::new(c, (10.5, 10.5), -std::f64::consts::FRAC_PI_2, full * std::f64::consts::TAU, 0.0);
            cx.scene.stroke(&round, Affine::IDENTITY, color, None, &arc);
        }
        // Start over: a circular arrow.
        let restart = Rect::new(o.x + 54.0, o.y + 54.0, o.x + 86.0, o.y + 86.0);
        let ink = icon_button(cx.scene, theme, restart, theme.panel_control, hot(restart), false, color);
        let c = restart.center();
        let arc = mite_ui::kurbo::Arc::new(c, (5.5, 5.5), -0.9 * std::f64::consts::PI, 1.6 * std::f64::consts::PI, 0.0);
        cx.scene.stroke(&round, Affine::IDENTITY, ink, None, &arc);
        let tip = c + (5.5 * (0.7 * std::f64::consts::PI).cos(), 5.5 * (0.7 * std::f64::consts::PI).sin());
        let mut head = BezPath::new();
        head.move_to(tip + (-3.5, 0.5));
        head.line_to(tip);
        head.line_to(tip + (0.5, -3.5));
        cx.scene.stroke(&round, Affine::IDENTITY, ink, None, &head);
        // A reference file: a page with a wave on it, lit while one is the reference.
        let file = Rect::new(o.x + 94.0, o.y + 54.0, o.x + 126.0, o.y + 86.0);
        let ink = icon_button(cx.scene, theme, file, theme.panel_control, hot(file), m.file.is_some(), color);
        let c = file.center();
        let mut page = BezPath::new();
        page.move_to((c.x - 5.5, c.y - 7.5));
        page.line_to((c.x + 2.0, c.y - 7.5));
        page.line_to((c.x + 5.5, c.y - 4.0));
        page.line_to((c.x + 5.5, c.y + 7.5));
        page.line_to((c.x - 5.5, c.y + 7.5));
        page.close_path();
        cx.scene.stroke(&round, Affine::IDENTITY, ink, None, &page);
        let mut wave = BezPath::new();
        wave.move_to((c.x - 3.5, c.y + 1.5));
        wave.curve_to((c.x - 1.5, c.y - 3.0), (c.x + 1.5, c.y + 6.0), (c.x + 3.5, c.y + 1.5));
        cx.scene.stroke(&round, Affine::IDENTITY, ink, None, &wave);

        // Close, and add the bands (a tick, lit once there is something to add).
        let close = Rect::new(o.x + PANEL_W - 44.0, o.y + 12.0, o.x + PANEL_W - 14.0, o.y + 42.0);
        let apply = Rect::new(close.x0, o.y + 58.0, close.x1, o.y + 88.0);
        let ready = !m.fitted.is_empty();
        let ink = icon_button(cx.scene, theme, apply, theme.panel_control, ready && hot(apply), ready, color);
        let ink = if ready { ink } else { theme.text_disabled };
        let k = apply.center();
        let mut tick = BezPath::new();
        tick.move_to((k.x - 4.5, k.y + 0.5));
        tick.line_to((k.x - 1.5, k.y + 3.5));
        tick.line_to((k.x + 4.5, k.y - 3.5));
        cx.scene.stroke(&round, Affine::IDENTITY, ink, None, &tick);
        let ink = icon_button(cx.scene, theme, close, theme.panel_control, hot(close), false, color);
        let k = close.center();
        let mut cross = BezPath::new();
        cross.move_to((k.x - 3.0, k.y - 3.0));
        cross.line_to((k.x + 3.0, k.y + 3.0));
        cross.move_to((k.x + 3.0, k.y - 3.0));
        cross.line_to((k.x - 3.0, k.y + 3.0));
        cx.scene.stroke(&round, Affine::IDENTITY, ink, None, &cross);

        let mut local = DrawCx { scene: &mut *cx.scene, theme, text: &mut *cx.text, host: &m.controls, size: cx.size, fx: &mut *cx.fx };
        for knob in &m.knobs {
            knob.draw(&mut local);
        }
        m.hits = Some(MatchHits { panel: rect, learn, restart, file, drop, apply, close });
    }

    /// Match panel events, answered with its own dials' host. `None` lets the event through to
    /// the plot (nodes stay reachable; picking one closes Match).
    fn match_event(&mut self, event: &UiEvent, host: &dyn ParamHost) -> Option<bool> {
        let m = self.matcher.as_mut()?;
        let hits = m.hits?;
        let mut changed = false;
        for k in &mut m.knobs {
            changed |= k.event(event, &m.controls);
        }
        if m.knobs.iter().any(Knob::is_dragging) {
            return Some(true);
        }
        let pos = match *event {
            UiEvent::PointerMove { pos, .. } => {
                self.pointer = Some(pos);
                return hits.panel.contains(pos).then_some(true);
            }
            UiEvent::PointerDown { pos, button: MouseButton::Left, .. } => pos,
            UiEvent::PointerUp { pos, .. } | UiEvent::DoubleClick { pos, .. } | UiEvent::Wheel { pos, .. } => {
                return (changed || hits.panel.contains(pos)).then_some(true);
            }
            _ => return None,
        };
        if !hits.panel.contains(pos) {
            return None;
        }
        if hits.learn.contains(pos) {
            m.learning = !m.learning;
        } else if hits.file.contains(pos) {
            if m.picking.is_none() {
                m.picking = Some(mite_ui::dialog::pick_file("Match to an audio file", &[("Audio", crate::reference::EXTENSIONS)]));
            }
        } else if hits.drop.is_some_and(|r| r.contains(pos)) {
            m.file = None;
            m.error = None;
            m.refit();
        } else if hits.restart.contains(pos) {
            m.restart();
        } else if hits.close.contains(pos) {
            self.matcher = None;
        } else if hits.apply.contains(pos) {
            self.apply_match(host);
        }
        Some(true)
    }

    fn draw_help(&self, cx: &mut DrawCx<'_>) {
        self.help_text(cx.host).draw(cx, Rect::new(0.0, HELP_TOP, WIDTH, FOOTER_TOP));
    }
}

impl View for EqView {
    fn logical_size(&self) -> (f64, f64) {
        (WIDTH, HEIGHT)
    }

    fn zoom(&self) -> f64 {
        ZOOM
    }

    fn draw(&mut self, cx: &mut DrawCx<'_>) {
        let theme = cx.theme;
        let bands: Vec<BandSettings> = (0..BANDS).map(|b| self.band(cx.host, b)).collect();
        self.soloed = self.soloed_band(cx.host);
        self.linear = PhaseMode::from_index(Self::plain(cx.host, MODE.id).round().max(0.0) as usize) == PhaseMode::LinearPhase;
        self.draw_header(cx);
        cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.display, None, &Rect::new(0.0, DISPLAY_TOP, WIDTH, HELP_TOP));
        self.draw_grid(cx);
        cx.scene.push_clip_layer(Fill::NonZero, Affine::IDENTITY, &Rect::new(0.0, DISPLAY_TOP, PLOT_W, HELP_TOP));
        self.draw_spectrum(cx);
        self.draw_masking(cx);
        self.draw_curves(cx, &bands);
        self.draw_match(cx);
        self.draw_grab(cx);
        cx.scene.pop_layer();
        self.draw_nodes(cx, &bands);
        self.meter.draw(cx);
        let r = self.meter.rect;
        cx.text.draw(cx.scene, "OUT", r.center().x, DISPLAY_TOP + 596.0, TextStyle::LABEL.sized(9.0), theme.text_lo, Align::Center);
        self.draw_panel(cx, &bands);
        self.draw_match_panel(cx);
        self.draw_help(cx);
        self.draw_footer(cx);
        self.draw_menu(cx);
        self.session.draw_menu(cx);
    }

    fn event(&mut self, event: &UiEvent, host: &dyn ParamHost) -> bool {
        // The editor's own writes are noted; once nothing is being dragged, they become one undo
        // step.
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
        // Changes from outside the editor (automation, the host's own undo, a state load) are
        // not undo steps: the history just follows them.
        let busy = self.busy();
        self.session.follow(host, busy);
        let (l, r) = self.shared.meter.load();
        let mut dirty = self.meter.set_levels(l, r);
        // Dynamic bands and the spectral stage move with the audio. Their telemetry arrives once
        // per host block (which can be 20 ms), so the drawing eases towards each new value at the
        // frame rate instead of stepping from block to block.
        let dt = self.last_live.elapsed().as_secs_f64().min(0.1);
        self.last_live = Instant::now();
        let k = (1.0 - (-dt / LIVE_EASE_S).exp()) as f32;
        let mut ease = |shown: &mut f32, mid: &mut f32, now: f32| {
            *mid += (now - *mid) * k;
            let next = *shown + (*mid - *shown) * k;
            let next = if (now - next).abs() < 0.002 && (now - *mid).abs() < 0.002 { now } else { next };
            if (next - *shown).abs() > 0.001 {
                dirty = true;
            }
            *shown = next;
        };
        for ((shown, mid), now) in self.last_dynamic.iter_mut().zip(&mut self.mid_dynamic).zip(&self.shared.dynamic) {
            ease(shown, mid, now.load());
        }
        for ((shown, mid), now) in self.last_spectral.iter_mut().zip(self.mid_spectral.iter_mut()).zip(&self.shared.spectral) {
            ease(shown, mid, now.load());
        }
        // The selected band's level drives its scope dot.
        if let Some(b) = self.selected {
            let level = self.shared.level[b].load();
            if self.last_level <= -119.0 {
                (self.last_level, self.mid_level) = (level, level);
            }
            ease(&mut self.last_level, &mut self.mid_level, level);
        }
        // Peak hold: resting under the spectrum (not on a node, a control or the panel) for a moment
        // starts collecting the spectrum's maxima; a grab keeps the held curve still.
        let over_plot = self.pointer.is_some_and(|p| self.under_spectrum(p) && !self.panel_hits.is_some_and(|h| h.panel.contains(p)))
            && self.hover.is_none()
            && self.hot.is_none()
            && self.drag.is_none();
        if over_plot || self.grab.is_some() {
            let since = *self.hold_since.get_or_insert_with(Instant::now);
            if !self.holding && since.elapsed().as_secs_f64() >= HOLD_AFTER_S {
                self.holding = true;
                self.held.copy_from_slice(&self.post);
            }
        } else {
            self.hold_since = None;
            self.holding = false;
        }
        if self.holding && self.grab.is_none() {
            for (held, &now) in self.held.iter_mut().zip(&self.post) {
                if now > *held {
                    *held = now;
                    dirty = true;
                }
            }
        }
        let k = (1.0 - (-dt / HOLD_FADE_S).exp()) as f32;
        let target = if self.holding { 1.0 } else { 0.0 };
        let next = self.hold_alpha + (target - self.hold_alpha) * k;
        let next = if (next - target).abs() < 0.005 { target } else { next };
        dirty |= (next - self.hold_alpha).abs() > 0.001;
        self.hold_alpha = next;
        // The peak under a resting pointer follows the moving spectrum; its marker glides and fades.
        if self.grab.is_none() && self.drag.is_none() && self.hover.is_none() && self.hot.is_none() {
            let resting = self.pointer.filter(|&p| !self.panel_hits.is_some_and(|h| h.panel.contains(p)));
            let peak = resting.and_then(|p| self.find_peak(p));
            if peak != self.peak {
                self.peak = peak;
                dirty = true;
            }
        }
        let fade = (1.0 - (-dt / PEAK_FADE_S).exp()) as f32;
        let (pos, alpha) = &mut self.peak_marker;
        let target = if self.grab.is_none() { self.peak.map(|pk| Point::new(fx(pk.freq), sy(pk.db))) } else { None };
        if let Some(t) = target {
            let next = if *alpha < 0.02 { t } else { pos.lerp(t, f64::from(fade)) };
            dirty |= next.distance(*pos) > 0.05;
            *pos = next;
        }
        let next = *alpha + (if target.is_some() { 1.0 } else { 0.0 } - *alpha) * fade;
        let next = if next < 0.005 { 0.0 } else if next > 0.995 { 1.0 } else { next };
        dirty |= (next - *alpha).abs() > 0.002;
        *alpha = next;
        // The selected band's per-bin detail (spectral scope): ask the engine for it, follow it.
        let focus = self.selected.filter(|&b| self.band(host, b).is_spectral());
        self.shared.focus.store(focus.map_or(-1.0, |b| b as f32));
        if focus.is_some() {
            for (i, (level, gain)) in self.focus_level.iter_mut().zip(self.focus_gain.iter_mut()).enumerate() {
                let (l, g) = (self.shared.focus_level[i].load(), self.shared.focus_gain[i].load());
                let t = self.shared.focus_threshold[i].load();
                if (l - *level).abs() > 0.05 || (g - *gain).abs() > 0.02 || (t - self.focus_threshold[i]).abs() > 0.05 {
                    dirty = true;
                }
                (*level, *gain, self.focus_threshold[i]) = (l, g, t);
            }
            dirty |= self.update_scope();
        }
        // A solo lasts while its band is selected: closing the panel or picking another band ends it.
        for b in (0..BANDS).filter(|&b| self.selected != Some(b)) {
            if Self::plain(host, &self.ids[b].solo) >= 0.5 {
                Self::set(host, &self.ids[b].solo, 0.0);
                dirty = true;
            }
        }
        // The latency readout follows the engine (it changes once a mode switch completes).
        let latency = self.shared.latency.load();
        if latency != self.last_latency {
            self.last_latency = latency;
            dirty = true;
        }
        // The analyzer updates every UI tick so motion is as smooth as the display allows.
        dirty |= self.update_spectrum();
        dirty |= self.update_masking(dt);
        dirty |= self.update_match(host);
        // Host-side changes (automation, presets, undo) since the last frame, checked at frame
        // rate rather than every tick.
        if self.last_signature_check.elapsed() < std::time::Duration::from_millis(16) {
            return dirty;
        }
        self.last_signature_check = Instant::now();
        let mut signature = Vec::with_capacity(1 + BANDS * 13);
        signature.push(host.normalized(OUT_GAIN.id));
        signature.push(host.normalized(MODE.id));
        signature.push(host.normalized(LP_RES.id));
        for ids in &self.ids {
            for id in [
                &ids.on, &ids.kind, &ids.freq, &ids.gain, &ids.q, &ids.slope, &ids.stereo, &ids.dyn_on, &ids.range,
                &ids.thresh, &ids.attack, &ids.release, &ids.ratio, &ids.dmode, &ids.sustain, &ids.transient, &ids.ttime, &ids.transition, &ids.spectral, &ids.ssmooth, &ids.srel, &ids.rthresh,
            ] {
                signature.push(host.normalized(id));
            }
        }
        if signature != self.signature {
            self.signature = signature;
            dirty = true;
        }
        dirty
    }
}

impl EqView {
    /// Every event, with the editor's writes going through `host` (a [`RecordingHost`]).
    fn handle_event(&mut self, event: &UiEvent, host: &dyn ParamHost) -> bool {
        if let UiEvent::KeyDown { key, mods } = *event {
            return self.key_event(key, mods, host);
        }
        // The preset list is modal, and so is an open dropdown; otherwise the header's session
        // controls come first.
        if self.menu.is_some() && !self.session.menu_open() {
            return self.menu_event(event, host);
        }
        if let Some(answer) = self.session.event(event, host) {
            return answer;
        }
        // Footer dials and toggles.
        if self.footer.event(event, host) {
            return true;
        }
        if let Some(answer) = self.match_event(event, host) {
            return answer;
        }
        let in_panel = |hits: &Option<PanelHits>, p: Point| hits.is_some_and(|h| h.panel.contains(p));

        match *event {
            UiEvent::PointerMove { pos, mods } => {
                self.pointer = Some(pos);
                if let Some(g) = &mut self.grab {
                    // The ghost Bell stays on the peak; up and down set its gain.
                    let factor = if mods.fine() { 0.1 } else { 1.0 };
                    let dy = pos.y - g.last.y;
                    g.last = pos;
                    g.moved |= pos.distance(g.start) > 3.0;
                    g.gain = (g.gain - dy * factor / PX_PER_DB).clamp(-30.0, 30.0);
                    return true;
                }
                if let Some(d) = &mut self.drag {
                    let factor = if mods.fine() { 0.1 } else { 1.0 };
                    let (dx, dy) = (pos.x - d.last.x, pos.y - d.last.y);
                    d.last = pos;
                    let ids = &self.ids[d.band];
                    match d.mode {
                        DragMode::Move => {
                            d.log_freq = (d.log_freq + dx * factor * (FREQ_MAX / FREQ_MIN).ln() / PLOT_W)
                                .clamp(FREQ_MIN.ln(), FREQ_MAX.ln());
                            host.set_normalized(&ids.freq, host.normalize(&ids.freq, d.log_freq.exp() as f32));
                            let kind = BandType::from_index(Self::plain(host, &ids.kind).round() as usize);
                            if kind.has_gain() {
                                d.gain = (d.gain - dy * factor / PX_PER_DB).clamp(-30.0, 30.0);
                                host.set_normalized(&ids.gain, host.normalize(&ids.gain, d.gain as f32));
                            }
                        }
                        DragMode::Width => {
                            // interaction.md: one octave of width per 120 px; dragging up narrows.
                            d.log_q = (d.log_q - dy * factor * LN_2 / 120.0).clamp(0.025f64.ln(), 40f64.ln());
                            host.set_normalized(&ids.q, host.normalize(&ids.q, d.log_q.exp() as f32));
                        }
                        // Range drags stop where the gain dial ends, so reversing responds at once.
                        DragMode::Range | DragMode::Ring | DragMode::Scope => {
                            let (lo, hi) = Self::range_limits(host, ids);
                            let delta = match d.mode {
                                DragMode::Range => -dy / PX_PER_DB,
                                DragMode::Ring => (dx - dy) * RING_DB_PER_PX,
                                _ => -dy * RING_DB_PER_PX * 1.5,
                            };
                            d.range = (d.range + delta * factor).clamp(lo, hi);
                            Self::apply_range(host, ids, d.range);
                            if d.mode == DragMode::Scope {
                                d.thresh = (d.thresh + dx * factor * -SCOPE_FLOOR_DB / SCOPE_W).clamp(f64::from(THRESH.min), f64::from(THRESH.max));
                                host.set_normalized(&ids.thresh, host.normalize(&ids.thresh, d.thresh as f32));
                            }
                        }
                        DragMode::TransientAttack => {
                            d.transient = (d.transient - dy * factor * SCOPE_DB_PER_PX).clamp(f64::from(TRANSIENT.min), f64::from(TRANSIENT.max));
                            host.set_normalized(&ids.transient, host.normalize(&ids.transient, d.transient as f32));
                            Self::update_dynamics_on(host, ids);
                        }
                        DragMode::Split => {
                            // The line follows the pointer along the scope's time axis.
                            if let Some(r) = self.panel_hits.and_then(|h| h.scope) {
                                let ms = (scope_time(r, pos.x) * 1000.0).clamp(f64::from(TTIME.min), f64::from(TTIME.max));
                                host.set_normalized(&ids.ttime, host.normalize(&ids.ttime, ms as f32));
                            }
                        }
                        DragMode::Sustain => {
                            d.sustain = (d.sustain - dy * factor * SCOPE_DB_PER_PX).clamp(f64::from(SUSTAIN.min), f64::from(SUSTAIN.max));
                            host.set_normalized(&ids.sustain, host.normalize(&ids.sustain, d.sustain as f32));
                            Self::update_dynamics_on(host, ids);
                        }
                        DragMode::SpectralThreshold => {
                            // The line follows the pointer on the scope's level axis.
                            let per_px = -SCOPE_FLOOR_DB / (SCOPE_H - 2.0 * SCOPE_PAD);
                            if Self::plain(host, &ids.srel) >= 0.5 {
                                d.relative = (d.relative - dy * factor * per_px).clamp(f64::from(RTHRESH.min), f64::from(RTHRESH.max));
                                host.set_normalized(&ids.rthresh, host.normalize(&ids.rthresh, d.relative as f32));
                            } else {
                                d.thresh = (d.thresh - dy * factor * per_px).clamp(f64::from(THRESH.min), f64::from(THRESH.max));
                                host.set_normalized(&ids.thresh, host.normalize(&ids.thresh, d.thresh as f32));
                            }
                        }
                    }
                    return true;
                }
                let mut changed = false;
                for k in &mut self.panel_knobs {
                    changed |= k.event(event, host);
                }
                let hot = self.hot_at(host, pos);
                changed |= hot != self.hot;
                self.hot = hot;
                let hover = if in_panel(&self.panel_hits, pos) || hot.is_some() { None } else { self.node_at(host, pos) };
                changed |= hover != self.hover;
                self.hover = hover;
                // The suggestion's rings keep it up even past the edge of a narrow region.
                let on_rings = self.collision_hot.filter(|&i| {
                    i < self.collisions.len()
                        && (self.carve_pos(i).distance(pos) <= CARVE_RING + 4.0 || self.carve_there_pos(i).is_some_and(|c| c.distance(pos) <= CARVE_RING + 4.0))
                });
                // Nodes, controls and carve rings win over the spectrum underneath them.
                let peak = if hover.is_none() && hot.is_none() && on_rings.is_none() && !in_panel(&self.panel_hits, pos) { self.find_peak(pos) } else { None };
                changed |= peak != self.peak;
                self.peak = peak;
                let collision = if hover.is_none() && hot.is_none() && peak.is_none() && !in_panel(&self.panel_hits, pos) { on_rings.or_else(|| self.collision_at(pos)) } else { None };
                changed |= collision != self.collision_hot;
                self.collision_hot = collision;
                // The help strip follows the pointer over the display; footer buttons light on hover.
                changed || (pos.x < PLOT_W && pos.y > DISPLAY_TOP && pos.y < HELP_TOP) || pos.y > FOOTER_TOP || pos.y < HEADER
            }
            UiEvent::PointerDown { pos, button: MouseButton::Left, mods } => {
                // Header (the session's controls answered already): EQ Match.
                if pos.y < HEADER {
                    let (y0, y1) = HEADER_ROW;
                    if Rect::new(MATCH_X.0, y0, MATCH_X.1, y1).contains(pos) {
                        self.toggle_match();
                    }
                    return true;
                }
                // Footer: processing mode and resolution.
                if pos.y > FOOTER_TOP {
                    if let Some(i) = self.mode_hits.iter().position(|r| r.contains(pos)) {
                        Self::set(host, MODE.id, i as f32);
                        return true;
                    }
                    if let Some(r) = self.resolution_hit.filter(|r| r.contains(pos)) {
                        self.toggle_menu(MenuKind::Resolution, 0, r, host);
                        return true;
                    }
                }
                if let (Some(hot), Some(b)) = (self.hot_at(host, pos), self.selected) {
                    let s = self.band(host, b);
                    let range = if s.is_dynamic() { s.dynamics.range_db } else { 0.0 };
                    match hot {
                        Hot::Ring => self.begin_drag(host, b, DragMode::Ring, pos, range),
                        Hot::Scope if s.is_spectral() => self.begin_drag(host, b, DragMode::SpectralThreshold, pos, range),
                        Hot::Scope => {
                            // Transient scope: the hit's attack sets the attack amount, its tail the sustain.
                            let head = self.panel_hits.and_then(|h| h.scope).is_some_and(|r| pos.x < scope_x(r, s.dynamics.transient_ms / 1000.0));
                            let mode = match (s.dynamics.mode, head) {
                                (DynamicsMode::Compressor, _) => DragMode::Scope,
                                (DynamicsMode::Transient, true) => DragMode::TransientAttack,
                                (DynamicsMode::Transient, false) => DragMode::Sustain,
                            };
                            self.begin_drag(host, b, mode, pos, range);
                        }
                        Hot::Split => self.begin_drag(host, b, DragMode::Split, pos, range),
                        Hot::Mode(i) => {
                            let ids = &self.ids[b];
                            Self::set(host, &ids.dmode, if i == 2 { 1.0 } else { 0.0 });
                            Self::set(host, &ids.spectral, if i == 1 { 1.0 } else { 0.0 });
                            self.panel_knobs.clear();
                        }
                        // A ghost grip starts from where it sits, so the band follows the pointer.
                        Hot::Grip(g) => {
                            let from = if s.is_dynamic() { range } else { yg(g.y) - s.gain_db };
                            self.begin_drag(host, b, DragMode::Range, pos, from);
                        }
                        Hot::RemoveDynamics => self.remove_dynamics(host, b),
                        Hot::Relative => {
                            Self::set(host, &self.ids[b].srel, if s.dynamics.relative { 0.0 } else { 1.0 });
                            self.panel_knobs.clear();
                        }
                    }
                    return true;
                }
                if let (Some(hits), Some(b)) = (self.panel_hits, self.selected) {
                    if hits.panel.contains(pos) {
                        let ids = &self.ids[b];
                        if hits.kind.contains(pos) {
                            self.toggle_menu(MenuKind::Type, b, hits.kind, host);
                        } else if let Some(r) = hits.slope.filter(|r| r.contains(pos)) {
                            self.toggle_menu(MenuKind::Slope, b, r, host);
                        } else if hits.stereo.contains(pos) {
                            self.toggle_menu(MenuKind::Stereo, b, hits.stereo, host);
                        } else if let Some(r) = hits.source.filter(|r| r.contains(pos)) {
                            self.toggle_menu(MenuKind::Source, b, r, host);
                        } else if hits.solo.is_some_and(|r| r.contains(pos)) {
                            // One band at a time: soloing this one ends any other solo.
                            let on = Self::plain(host, &ids.solo) < 0.5;
                            for other in (0..BANDS).filter(|&o| o != b) {
                                if Self::plain(host, &self.ids[other].solo) >= 0.5 {
                                    Self::set(host, &self.ids[other].solo, 0.0);
                                }
                            }
                            Self::set(host, &self.ids[b].solo, if on { 1.0 } else { 0.0 });
                        } else if hits.remove.contains(pos) {
                            Self::set(host, &ids.on, 0.0);
                            self.select(None);
                        } else if hits.close.contains(pos) {
                            self.select(None);
                        } else {
                            for k in &mut self.panel_knobs {
                                k.event(event, host);
                            }
                        }
                        return true;
                    }
                }
                if let Some(b) = self.node_at(host, pos) {
                    if self.selected != Some(b) {
                        self.select(Some(b));
                    }
                    let mode = if mods.alt { DragMode::Width } else { DragMode::Move };
                    self.begin_drag(host, b, mode, pos, 0.0);
                    return true;
                }
                // A carve ring: the suggested Bell becomes a band, already in hand to drag.
                if let Some(i) = self.collision_hot.filter(|&i| i < self.collisions.len() && self.carve_there_pos(i).is_some_and(|c| c.distance(pos) <= CARVE_RING + 4.0)) {
                    self.carve_there(i);
                    return true;
                }
                if let Some(i) = self.collision_hot.filter(|&i| i < self.collisions.len() && self.carve_pos(i).distance(pos) <= CARVE_RING + 4.0) {
                    let s = self.carve(i);
                    self.add_bell(host, s.freq, s.gain_db, s.q);
                    self.collision_hot = None;
                    if let Some(b) = self.selected {
                        self.begin_drag(host, b, DragMode::Move, pos, 0.0);
                    }
                    return true;
                }
                if let Some(peak) = self.find_peak(pos) {
                    self.grab = Some(Grab { freq: peak.freq, gain: 0.0, start: pos, last: pos, moved: false });
                    return true;
                }
                if pos.y > DISPLAY_TOP && pos.y < HELP_TOP && self.selected.is_some() {
                    self.select(None);
                    return true;
                }
                false
            }
            UiEvent::PointerUp { button: MouseButton::Left, .. } => {
                if let Some(g) = self.grab.take() {
                    // Released after a drag: the ghost becomes a band. A plain click on a peak
                    // is a click on the display (clears the selection).
                    if g.moved {
                        self.add_bell(host, g.freq, g.gain, PEAK_Q);
                    } else {
                        self.select(None);
                    }
                    self.peak = None;
                    return true;
                }
                if let Some(d) = self.drag.take() {
                    let ids = &self.ids[d.band];
                    let kind = BandType::from_index(Self::plain(host, &ids.kind).round() as usize);
                    for id in Self::drag_ids(ids, d.mode, kind.has_gain()) {
                        host.end_gesture(id);
                    }
                    // A move on the plot may have put the band under the panel: dock it afresh.
                    if !matches!(d.mode, DragMode::Ring | DragMode::Scope | DragMode::SpectralThreshold) {
                        self.panel_dock = None;
                    }
                    return true;
                }
                let mut changed = false;
                for k in &mut self.panel_knobs {
                    changed |= k.event(event, host);
                }
                changed
            }
            UiEvent::PointerDown { pos, button: MouseButton::Right, .. } => {
                // interaction.md puts delete in the band menu; until menus exist, right-click deletes.
                if let Some(b) = self.node_at(host, pos) {
                    Self::set(host, &self.ids[b].on, 0.0);
                    if self.selected == Some(b) {
                        self.select(None);
                    }
                    return true;
                }
                // On a collision: ignore it from now on.
                if let Some(i) = self.collision_at(pos) {
                    let (other, c) = self.collisions.remove(i);
                    self.ignored.push((other, spectrum_point_freq(c.start), spectrum_point_freq(c.end)));
                    self.collision_hot = None;
                    return true;
                }
                false
            }
            UiEvent::DoubleClick { pos, .. } => {
                if let (Some(hot), Some(b)) = (self.hot_at(host, pos), self.selected) {
                    match hot {
                        Hot::Ring | Hot::Grip(_) => self.remove_dynamics(host, b),
                        Hot::Split => Self::set(host, &self.ids[b].ttime, TTIME.default),
                        Hot::Scope | Hot::Mode(_) | Hot::RemoveDynamics | Hot::Relative => {}
                    }
                    return true;
                }
                if in_panel(&self.panel_hits, pos) {
                    let mut changed = false;
                    for k in &mut self.panel_knobs {
                        changed |= k.event(event, host);
                    }
                    return changed;
                }
                if let Some(b) = self.node_at(host, pos) {
                    // Resets the band: 0 dB, Q 0.71 (interaction.md).
                    let ids = &self.ids[b];
                    Self::set(host, &ids.gain, 0.0);
                    Self::set(host, &ids.q, std::f32::consts::FRAC_1_SQRT_2);
                    return true;
                }
                if let Some(peak) = self.find_peak(pos) {
                    self.add_bell(host, peak.freq, PEAK_CUT_DB, PEAK_Q);
                    self.peak = None;
                    return true;
                }
                if pos.x < PLOT_W && pos.y > PLOT_TOP && pos.y < PLOT_BOT {
                    self.add_band(host, pos);
                    return true;
                }
                false
            }
            UiEvent::Wheel { pos, lines, mods } => {
                if let (Some(Hot::Ring), Some(b)) = (self.hot_at(host, pos), self.selected) {
                    // The wheel on the ring steps the range.
                    let s = self.band(host, b);
                    let from = if s.is_dynamic() { s.dynamics.range_db as f32 } else { 0.0 };
                    let ids = &self.ids[b];
                    host.begin_gesture(&ids.range);
                    host.begin_gesture(&ids.dyn_on);
                    Self::apply_range(host, ids, f64::from(RANGE.wheel(from, lines, mods.fine())));
                    host.end_gesture(&ids.dyn_on);
                    host.end_gesture(&ids.range);
                    return true;
                }
                if in_panel(&self.panel_hits, pos) {
                    let mut changed = false;
                    for k in &mut self.panel_knobs {
                        changed |= k.event(event, host);
                    }
                    return changed;
                }
                if let Some(b) = self.node_at(host, pos).or(self.selected) {
                    let s = self.band(host, b);
                    if s.kind.has_slope() {
                        // Cuts: the wheel steps the slope, up = steeper.
                        self.slope_wheel += lines;
                        let steps = self.slope_wheel.trunc();
                        if steps == 0.0 {
                            return false;
                        }
                        self.slope_wheel -= steps;
                        let next = (s.slope as f32 + steps).clamp(0.0, (SLOPE.choices.len() - 1) as f32);
                        Self::set(host, &self.ids[b].slope, next);
                        return true;
                    }
                    let id = &self.ids[b].q;
                    let q = Q.wheel(Self::plain(host, id), lines, mods.fine());
                    Self::set(host, id, q);
                    return true;
                }
                false
            }
            UiEvent::PointerLeft => {
                self.pointer = None;
                self.hover = None;
                self.hot = None;
                self.peak = None;
                self.collision_hot = None;
                true
            }
            _ => false,
        }
    }

}
