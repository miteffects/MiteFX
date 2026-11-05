//! Dynamics: the compressor behind Mite Comp (BLUEPRINT §6.2; docs/design/interaction.md, "Mite
//! Comp"). The ideas come from Nova's NovaComp and CompCurve; the code is new.
//!
//! Three layers:
//!
//! 1. **The curve.** Up to four [`Stage`]s, each a full dynamics stage working in one quadrant
//!    (compress, expand, upward compress, upward expand), composed **in series** in threshold
//!    order, like compressors patched one into the next. Thresholds are set on the input, so a
//!    stage starts where the input reaches its threshold whatever the stages below it do, and a
//!    stage never changes anything the stages below it do: the curve up to a stage's node is its
//!    own and theirs (user decision, 2026-10-23). Every stage's level map is monotone, so the
//!    composed curve is too, whatever the stages. [`curve_db`] is the one function the audio
//!    thread, the editor's transfer square and its history all evaluate, so what is drawn is what
//!    is applied. A stage's range caps its gain softly: the curve bends back parallel to the
//!    diagonal with no corner.
//! 2. **Character.** A [`Style`] reshapes how the stages move: attack and release scale, detector
//!    drive, a slow program-dependent tail, faster recovery off transients, and (Vari-Mu) a ratio
//!    that firms up the harder a stage is driven. It never changes the static curve.
//! 3. **Smart.** Trackers on the detector bend the compressor to the material: Adaptive scales
//!    the release by the crest factor; Vocal and Master also slide every threshold to hold a
//!    target gain reduction, and Vocal eases the detector off esses.
//!
//! The detector hears the input (feed-forward), the output (feedback) or the external key,
//! through up to three EQ bands. Stereo works as L/R or M/S with a continuous link. Lookahead
//! delays the audio, not the detector.
//!
//! Nothing here allocates outside [`Compressor::prepare`] (and the UI-only [`simulate_stage`]).

use crate::eq::{design, BandSettings, Sections, MAX_SECTIONS};
use crate::filter::{Coeffs, Tdf2};
use crate::{db_to_gain, gain_to_db, LinearSmoother, MINUS_INFINITY_DB};

/// Stages per compressor.
pub const STAGES: usize = 4;
/// The ratio parameter's top, and the point from which a ratio is ∞:1.
pub const RATIO_MAX: f32 = 100.0;
pub const INF_RATIO: f32 = 99.5;
/// Longest lookahead, ms (the delay line is sized for it).
pub const MAX_LOOKAHEAD_MS: f32 = 20.0;
/// Detector EQ bands.
pub const KEY_BANDS: usize = 3;
/// How softly a stage's range caps its gain (the knee of the bend back to the diagonal), dB.
const RANGE_SOFTNESS: f32 = 3.0;
/// Parameter glides: thresholds, ratios, knees, ranges, stages switching and makeup.
const SMOOTH_MS: f32 = 20.0;
/// Detector windows: the peak detector's release, RMS, and the K-weighted loudness detector.
const PEAK_RELEASE_MS: f32 = 10.0;
const RMS_MS: f32 = 10.0;
const LOUDNESS_MS: f32 = 50.0;
/// The fast recovery lane of program-dependent release, and the slow (opto/glue) tail.
const PROGRAM_FAST_MS: f32 = 60.0;
const SLOW_TAIL_MS: f32 = 1800.0;
/// Program-dependent release starts once the gain is this far past its target, dB.
const PROGRAM_MARGIN_DB: f32 = 3.0;
/// Auto release: how long sustained gain change takes to build the slow envelope, ms, how many
/// times slower than the stage's release it lets go, and the bounds of that slow release, ms.
pub const AUTO_BUILD_MS: f32 = 300.0;
pub const AUTO_SLOW_FACTOR: f32 = 5.0;
const AUTO_SLOW_MS: (f32, f32) = (300.0, 5000.0);
/// Auto makeup cancels the curve's gain at this input level, dBFS.
pub const AUTO_MAKEUP_REF_DB: f32 = -12.0;
/// Auto makeup matches loudness in and out (2026-11-03: it cancelled the curve at
/// [`AUTO_MAKEUP_REF_DB`], louder than the music, which mostly sits well below it). It predicts
/// the curve's gain at the music's level (instant as settings move) and trims that to what it
/// measures, over the whole signal: a compressor does not lift the gaps (unlike saturation), so
/// matching only the hits would leave the result louder. The trim is a closed loop (2026-11-04):
/// it meters the output after the auto makeup and integrates what is still missing, so the
/// prediction's bias on spiky material (its makeup swings with a level that follows the hits)
/// is made up too. The meters hold in silence (the 20 ms mean square under [`AUTO_FLOOR_DB`]).
///
/// 2026-11-05 (user: "sometimes too loud and sometimes too quiet"): the meters followed the music
/// within 0.4…3 s, so every phrase was brought back to the loudness it went in at, undoing the
/// compression between phrases (a sung line with phrases 21.5 dB apart came out 21 dB apart at
/// 4:1, 13.9 dB without auto makeup; `examples/vocal_comp`). Their times now grow with the
/// sound heard: as quick as before at first (the level, the meters and the trim at 0.4, 1 and
/// 3 s), then as long as the seconds of sound so far, up to [`AUTO_LONG_S`] for the level and
/// three times that for the trim: the song's loudness, not the phrase's.
const AUTO_LEVEL_MS: f32 = 400.0;
const AUTO_TRIM_MS: f32 = 3000.0;
/// The trim's meters (in, and out after the auto makeup), ms: a third of the trim's time, so
/// the loop settles without ringing (damping about 0.87).
const AUTO_METER_MS: f32 = 1000.0;
/// The level's and the meters' longest time, s (the trim's is three times it).
pub const AUTO_LONG_S: f32 = 10.0;
const AUTO_SHORT_MS: f32 = 20.0;
const AUTO_FLOOR_DB: f32 = -70.0;
const AUTO_TRIM_LIMIT_DB: f32 = 9.0;
/// Auto makeup is worked out again every this many samples (whatever the block size).
const AUTO_EVERY: u32 = 64;
/// Smart: crest factor that leaves the release alone, dB; programme below this level is silence.
const CREST_REF_DB: f32 = 10.0;
const PROGRAM_GATE_DB: f32 = -50.0;

/// The quadrant a stage works in. The index is saved state (append only).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Quadrant {
    /// Above the threshold, pulls down.
    #[default]
    Compress,
    /// Below the threshold, pulls down (an expander, a gate at high ratios).
    Expand,
    /// Below the threshold, lifts (quiet parts come up).
    UpCompress,
    /// Above the threshold, lifts (transients come up).
    UpExpand,
}

impl Quadrant {
    pub const NAMES: [&'static str; 4] = ["Compress", "Expand", "Upward compress", "Upward expand"];

    pub fn from_index(i: usize) -> Self {
        match i {
            1 => Self::Expand,
            2 => Self::UpCompress,
            3 => Self::UpExpand,
            _ => Self::Compress,
        }
    }

    /// Whether it lifts the level (drawn in `gainUp`) rather than pulling it down (`gr`).
    pub fn is_upward(self) -> bool {
        matches!(self, Self::UpCompress | Self::UpExpand)
    }

    /// Whether it acts above its threshold (else below).
    pub fn acts_above(self) -> bool {
        matches!(self, Self::Compress | Self::UpExpand)
    }

    /// The quadrant made by pulling the curve off the diagonal: `up` is above the line, `above`
    /// is the side of the threshold the pull starts on (interaction.md, "Quadrant").
    pub fn from_pull(up: bool, above: bool) -> Self {
        match (up, above) {
            (false, true) => Self::Compress,
            (false, false) => Self::Expand,
            (true, false) => Self::UpCompress,
            (true, true) => Self::UpExpand,
        }
    }
}

/// What a stage's detector measures. The index is saved state (append only).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Detector {
    /// Instant attack, 10 ms release (so a waveform's zero crossings are not silence).
    Peak,
    /// Power over 10 ms.
    #[default]
    Rms,
    /// K-weighted power over 50 ms (BS.1770's weighting, as loudness is heard).
    Loudness,
}

impl Detector {
    pub const NAMES: [&'static str; 3] = ["Peak", "RMS", "Loudness"];

    pub fn from_index(i: usize) -> Self {
        match i {
            0 => Self::Peak,
            2 => Self::Loudness,
            _ => Self::Rms,
        }
    }
}

/// One stage of the curve.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Stage {
    pub on: bool,
    pub quadrant: Quadrant,
    /// The input level where the stage starts acting, dB.
    pub threshold_db: f32,
    /// 1 … [`RATIO_MAX`]; from [`INF_RATIO`] it is ∞:1.
    pub ratio: f32,
    /// Knee width centred on the threshold, dB of input.
    pub knee_db: f32,
    /// How far the stage may move the level, dB (positive).
    pub range_db: f32,
    pub attack_ms: f32,
    pub release_ms: f32,
    /// Auto release: the release follows the program. A short peak lets go at `release_ms`;
    /// sustained gain change builds a slow second envelope (over [`AUTO_BUILD_MS`]) that lets go
    /// [`AUTO_SLOW_FACTOR`] times slower, so steady compression recovers without pumping.
    pub auto_release: bool,
    pub detector: Detector,
}

impl Default for Stage {
    fn default() -> Self {
        Self {
            on: false,
            quadrant: Quadrant::Compress,
            threshold_db: -18.0,
            ratio: 4.0,
            knee_db: 6.0,
            range_db: 60.0,
            attack_ms: 10.0,
            release_ms: 120.0,
            auto_release: false,
            detector: Detector::Rms,
        }
    }
}

/// The soft-kneed positive part of `x`: 0 well below the knee, `x` well above it, joined by the
/// quadratic that keeps value and slope continuous at both ends.
#[inline]
pub fn soft_above(x: f32, knee: f32) -> f32 {
    if knee <= 0.0 {
        return x.max(0.0);
    }
    let h = 0.5 * knee;
    if x >= h {
        x
    } else if x <= -h {
        0.0
    } else {
        let t = x + h;
        t * t / (2.0 * knee)
    }
}

/// `m` (≥ 0) capped at `range`, softly: unchanged well under it, `range` well over it.
#[inline]
pub fn soft_cap(m: f32, range: f32) -> f32 {
    m - soft_above(m - range.max(0.0), RANGE_SOFTNESS)
}

/// Auto release's slow envelope coefficients at `sample_rate` for an effective release of
/// `release_ms`: (build, fall).
#[inline]
fn auto_coefs(release_ms: f32, sample_rate: f32) -> (f32, f32) {
    (coef(AUTO_BUILD_MS, sample_rate), coef((release_ms * AUTO_SLOW_FACTOR).clamp(AUTO_SLOW_MS.0, AUTO_SLOW_MS.1), sample_rate))
}

/// Auto release, one sample: the slow envelope `slow` follows `target` (building over the build
/// time, letting go over the slow release), and the stage plays whichever of it and the fast
/// envelope `e` moves the level more.
#[inline]
fn auto_release(slow: &mut f32, e: f32, target: f32, (build, fall): (f32, f32)) -> f32 {
    let k = if target.abs() > slow.abs() { build } else { fall };
    *slow = k * *slow + (1.0 - k) * target;
    if slow.abs() > e.abs() {
        *slow
    } else {
        e
    }
}

/// The gain (dB, signed) stage `s` applies at detected level `level_db`. `mu_boost` (≥ 0) firms
/// the ratio up (Vari-Mu); the drawn curve uses 0.
#[inline]
pub fn stage_gain_db(s: &Stage, level_db: f32, mu_boost: f32) -> f32 {
    if !s.on {
        return 0.0;
    }
    let over = level_db - s.threshold_db;
    let r = s.ratio.max(1.0);
    let mut comp = if r >= INF_RATIO { 1.0 } else { 1.0 - 1.0 / r };
    let mut expand = r - 1.0;
    if mu_boost > 0.0 {
        comp = (comp * (1.0 + mu_boost)).min(1.0);
        expand *= 1.0 + mu_boost;
    }
    match s.quadrant {
        Quadrant::Compress => -soft_cap(comp * soft_above(over, s.knee_db), s.range_db),
        Quadrant::Expand => -soft_cap(expand * soft_above(-over, s.knee_db), s.range_db),
        Quadrant::UpCompress => soft_cap(comp * soft_above(-over, s.knee_db), s.range_db),
        Quadrant::UpExpand => soft_cap(expand * soft_above(over, s.knee_db), s.range_db),
    }
}

/// The stages' indices in processing order: by threshold, low to high (also the numbers the
/// editor shows). Stages that are off sort last.
pub fn order(stages: &[Stage; STAGES]) -> [usize; STAGES] {
    let mut idx = [0, 1, 2, 3];
    idx.sort_by(|&a, &b| {
        let (sa, sb) = (&stages[a], &stages[b]);
        sb.on.cmp(&sa.on).then(sa.threshold_db.total_cmp(&sb.threshold_db)).then(a.cmp(&b))
    });
    idx
}

/// Each stage's threshold in the level it works on: what the stages below it make of the input
/// at its threshold. A stage's input threshold never moves; this is where it lands after them.
pub fn level_thresholds(stages: &[Stage; STAGES]) -> [f32; STAGES] {
    let chained = chained(stages);
    std::array::from_fn(|i| chained[i].threshold_db)
}

/// The stages with their thresholds and knees in the level they work on: the threshold where
/// the stages below put the input threshold, the knee scaled by their slope there, so it spans
/// its width of input (and a stage reaches no further below its threshold than half its knee).
fn chained(stages: &[Stage; STAGES]) -> [Stage; STAGES] {
    let mut out = *stages;
    let order = order(stages);
    for (n, &k) in order.iter().enumerate() {
        let (threshold, slope) = into_level(stages[k].threshold_db, |level| {
            order[..n].iter().fold(level, |l, &j| l + stage_gain_db(&out[j], l, 0.0))
        });
        out[k].threshold_db = threshold;
        out[k].knee_db = stages[k].knee_db * slope;
    }
    out
}

/// Where input level `t` lands through `lower` (the stages below a stage), and their slope there.
#[inline]
pub(crate) fn into_level(t: f32, lower: impl Fn(f32) -> f32) -> (f32, f32) {
    let (down, up) = (lower(t - 0.01), lower(t + 0.01));
    (lower(t), ((up - down) / 0.02).max(0.0))
}

/// The output of the stages in threshold order, up to and including `last` (all of them when
/// `None`), for input `input_db`, every threshold moved by `offset_db` (Smart's slide).
fn run(stages: &[Stage; STAGES], last: Option<usize>, input_db: f32, offset_db: f32) -> f32 {
    let chained = chained(stages);
    let mut level = input_db;
    for i in order(stages) {
        level += stage_gain_db(&chained[i], level - offset_db, 0.0);
        if Some(i) == last {
            break;
        }
    }
    level
}

/// The static curve: the output level for input level `input_db`, the stages in series in
/// threshold order, every threshold moved by `offset_db` (Smart's slide).
pub fn curve_db(stages: &[Stage; STAGES], input_db: f32, offset_db: f32) -> f32 {
    run(stages, None, input_db, offset_db)
}

/// The curve as stage `slot` and the stages below it make it: where its node sits. The stages
/// above it never change it.
pub fn partial_curve_db(stages: &[Stage; STAGES], slot: usize, input_db: f32, offset_db: f32) -> f32 {
    run(stages, Some(slot), input_db, offset_db)
}

/// The level stage `slot` sees for input `input_db`: what the stages before it (lower
/// thresholds) have made of the input.
pub fn level_before(stages: &[Stage; STAGES], slot: usize, input_db: f32, offset_db: f32) -> f32 {
    let chained = chained(stages);
    let mut level = input_db;
    for i in order(stages) {
        if i == slot {
            break;
        }
        level += stage_gain_db(&chained[i], level - offset_db, 0.0);
    }
    level
}

/// How each Character moves the stages.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Style {
    /// Attack and release time multipliers.
    pub attack: f32,
    pub release: f32,
    /// Detector drive, dB (pushes into the thresholds).
    pub drive_db: f32,
    /// 0…1: blend of a slow, program-dependent second release stage (opto, glue).
    pub slow_tail: f32,
    /// 0…1: faster recovery once the gain is well past its target (off transients).
    pub program: f32,
    /// 0…1: the ratio firms up the harder a stage is driven (Vari-Mu).
    pub ratio_bend: f32,
}

impl Default for Style {
    fn default() -> Self {
        STYLES[0]
    }
}

/// Character names; the last is Custom (the six values set by hand). The index is saved state.
pub const CHARACTERS: [&str; 7] = ["Clean", "Punch", "FET", "Opto", "Bus", "Vari-Mu", "Custom"];
pub const CUSTOM: usize = 6;

/// The built-in Characters (Custom's defaults are Clean's, so switching to it changes nothing).
pub const STYLES: [Style; 6] = [
    Style { attack: 1.0, release: 1.0, drive_db: 0.0, slow_tail: 0.0, program: 0.0, ratio_bend: 0.0 },
    Style { attack: 0.55, release: 0.70, drive_db: 0.0, slow_tail: 0.0, program: 0.35, ratio_bend: 0.0 },
    Style { attack: 0.30, release: 0.55, drive_db: 1.6, slow_tail: 0.0, program: 0.20, ratio_bend: 0.0 },
    Style { attack: 1.60, release: 2.00, drive_db: 0.0, slow_tail: 0.55, program: 0.60, ratio_bend: 0.15 },
    Style { attack: 1.30, release: 1.60, drive_db: 0.0, slow_tail: 0.10, program: 0.70, ratio_bend: 0.0 },
    Style { attack: 1.40, release: 1.40, drive_db: 0.4, slow_tail: 0.25, program: 0.40, ratio_bend: 0.50 },
];

/// What adapts. The index is saved state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Smart {
    #[default]
    Off,
    /// The release follows the material (crest factor).
    Adaptive,
    /// FabFilter Pro-C 2's Vocal style (user, 2026-11-05: "as similar as possible to fab
    /// filter's vocal type"; its manual: "automatic knee and ratio settings, so compressing your
    /// lead vocal is as easy as choosing the right threshold"): every compressing stage plays
    /// ∞:1 over a very soft knee ([`VOCAL_KNEE_DB`]), so the further the voice goes over, the
    /// harder it is held, and nothing gets past the threshold + half the knee. Plus the sibilance
    /// guard; the release is RELEASE (no crest scaling: see [`adaptive_release_scale`]). (Until 2026-11-05 it slid the thresholds to hold a target
    /// reduction, a leveler that rode the voice up and down.)
    Vocal,
    /// Adaptive (gently), plus the thresholds slide slowly (up to ±9 dB).
    Master,
}

impl Smart {
    pub const NAMES: [&'static str; 4] = ["Off", "Adaptive", "Vocal", "Master"];

    pub fn from_index(i: usize) -> Self {
        match i {
            1 => Self::Adaptive,
            2 => Self::Vocal,
            3 => Self::Master,
            _ => Self::Off,
        }
    }

    /// Whether it slides the thresholds.
    pub fn tracks_threshold(self) -> bool {
        self == Self::Master
    }

    /// Whether it sets the compressing stages' ratio and knee itself (they show as automatic).
    pub fn sets_ratio_and_knee(self) -> bool {
        self == Self::Vocal
    }
}

/// Vocal's knee, dB. With ∞:1 the ratio rises smoothly with the level: 1:1 at the threshold
/// − 18 dB, 2:1 at the threshold (4.5 dB of reduction there), 4:1 at + 9 dB, ∞:1 from + 18 dB
/// (the output never above the threshold).
pub const VOCAL_KNEE_DB: f32 = 36.0;

impl Stage {
    /// The stage as `smart` plays it: Vocal sets a compressing stage's ratio and knee (one left
    /// at 1:1, the editor's way of switching a stage off, stays off).
    pub fn played(self, smart: Smart) -> Self {
        if smart.sets_ratio_and_knee() && self.quadrant == Quadrant::Compress && self.ratio > 1.001 {
            Self { ratio: RATIO_MAX, knee_db: VOCAL_KNEE_DB, ..self }
        } else {
            self
        }
    }
}

/// Smart's release multiplier for a crest factor (fast envelope over slow RMS, dB): spiky
/// material releases faster, sustained material slower. Not in Vocal (2026-11-05, user: "i move
/// attack from 0 to 250 and i hear no difference in vocal mode"): a voice is sustained, so its
/// release was stretched up to 3×, the reduction never let go between syllables (still −2.2 of
/// −4.6 dB at the next one's start) and ATTACK only shaped what was left. Pro-C's Vocal releases
/// at RELEASE.
pub fn adaptive_release_scale(crest_db: f32, smart: Smart, amount: f32) -> f32 {
    if matches!(smart, Smart::Off | Smart::Vocal) {
        return 1.0;
    }
    let raw = 2f32.powf((CREST_REF_DB - crest_db.clamp(0.0, 24.0)) / 8.0);
    let (lo, hi) = if smart == Smart::Master { (0.6, 2.0) } else { (0.3, 3.0) };
    1.0 + amount.clamp(0.0, 1.0) * (raw.clamp(lo, hi) - 1.0)
}

/// Everything the compressor plays, as the plugin's parameters describe it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CompSettings {
    pub stages: [Stage; STAGES],
    /// The Character in force (a built-in one, or Custom's values).
    pub style: Style,
    /// Detect on the output instead of the input.
    pub feedback: bool,
    pub mid_side: bool,
    /// Stereo link, 0…1.
    pub link: f32,
    pub lookahead_ms: f32,
    pub hold_ms: f32,
    pub makeup_db: f32,
    pub auto_makeup: bool,
    pub smart: Smart,
    pub smart_amount: f32,
    pub target_gr_db: f32,
    /// Sibilance guard depth (Vocal), 0…1.
    pub sibilance: f32,
    /// Detect on the external key (the sidechain input).
    pub external_key: bool,
    pub key_bands: [BandSettings; KEY_BANDS],
    /// Output the filtered detector signal instead of the result.
    pub listen: bool,
    /// Hear only this stage's gain change (by slot).
    pub solo: Option<usize>,
    /// The oversampling factor the engine runs the compressor at (1, 2 or 4).
    pub oversampling: usize,
}

impl Default for CompSettings {
    fn default() -> Self {
        Self {
            stages: [Stage::default(); STAGES],
            style: Style::default(),
            feedback: false,
            mid_side: false,
            link: 1.0,
            lookahead_ms: 0.0,
            hold_ms: 0.0,
            makeup_db: 0.0,
            auto_makeup: false,
            smart: Smart::Off,
            smart_amount: 0.7,
            target_gr_db: 6.0,
            sibilance: 0.5,
            external_key: false,
            key_bands: [BandSettings::default(); KEY_BANDS],
            listen: false,
            solo: None,
            oversampling: 1,
        }
    }
}

impl CompSettings {
    /// Samples of lookahead at `sample_rate` (the latency the plugin reports).
    pub fn latency(&self, sample_rate: f32) -> usize {
        (self.lookahead_ms.clamp(0.0, MAX_LOOKAHEAD_MS) * 0.001 * sample_rate).round() as usize
    }

    /// The settings as they play: the stages as Smart sets them ([`Stage::played`]). The audio
    /// thread and the editor both use this, so the curve drawn is the one applied.
    pub fn played(&self) -> Self {
        Self { stages: self.stages.map(|st| st.played(self.smart)), ..*self }
    }

    /// The makeup in force, dB: the set makeup plus `auto_db` (what auto makeup measured, when
    /// it is on).
    pub fn makeup_total_db(&self, auto_db: f32) -> f32 {
        let auto = if self.auto_makeup { auto_db } else { 0.0 };
        (self.makeup_db + auto).clamp(-24.0, 48.0)
    }
}

/// One block's telemetry (the editor's history, meters and stage strip).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CompTelemetry {
    /// Peak in and out per channel (L/R), dBFS.
    pub in_peak_db: [f32; 2],
    pub out_peak_db: [f32; 2],
    /// Per processing channel (L/R or M/S): the deepest cut (≤ 0) and the highest lift (≥ 0), dB.
    pub cut_db: [f32; 2],
    pub lift_db: [f32; 2],
    /// Each stage's gain at the end of the block (signed, dB; the channel moving most), by slot.
    pub stage_db: [f32; STAGES],
    /// The linked peak detector's highest level over the block, dBFS.
    pub detector_db: f32,
    /// Each detector's highest level over the block (peak, RMS, loudness; the louder channel),
    /// dBFS: what the stages' thresholds are compared with ([`MINUS_INFINITY_DB`] for a detector
    /// no stage uses).
    pub detector_levels_db: [f32; 3],
    /// Smart's slide of the thresholds, dB.
    pub smart_offset_db: f32,
}

impl Default for CompTelemetry {
    fn default() -> Self {
        Self {
            in_peak_db: [MINUS_INFINITY_DB; 2],
            out_peak_db: [MINUS_INFINITY_DB; 2],
            cut_db: [0.0; 2],
            lift_db: [0.0; 2],
            stage_db: [0.0; STAGES],
            detector_db: MINUS_INFINITY_DB,
            detector_levels_db: [MINUS_INFINITY_DB; 3],
            smart_offset_db: 0.0,
        }
    }
}

/// A one-pole coefficient for a time constant.
#[inline]
fn coef(ms: f32, sample_rate: f32) -> f32 {
    (-1.0 / (0.001 * ms.max(0.01) * sample_rate)).exp()
}

/// An RBJ biquad (detector-side filters only).
fn biquad(kind: Biquad, sample_rate: f32, freq: f32, q: f32, gain_db: f32) -> Coeffs {
    let w0 = 2.0 * std::f64::consts::PI * f64::from(freq) / f64::from(sample_rate);
    let (cs, sn) = (w0.cos(), w0.sin());
    let alpha = sn / (2.0 * f64::from(q));
    let (b0, b1, b2, a0, a1, a2) = match kind {
        Biquad::HighPass => ((1.0 + cs) / 2.0, -(1.0 + cs), (1.0 + cs) / 2.0, 1.0 + alpha, -2.0 * cs, 1.0 - alpha),
        Biquad::LowPass => ((1.0 - cs) / 2.0, 1.0 - cs, (1.0 - cs) / 2.0, 1.0 + alpha, -2.0 * cs, 1.0 - alpha),
        Biquad::HighShelf => {
            let a = 10f64.powf(f64::from(gain_db) / 40.0);
            let k = 2.0 * a.sqrt() * alpha;
            (
                a * ((a + 1.0) + (a - 1.0) * cs + k),
                -2.0 * a * ((a - 1.0) + (a + 1.0) * cs),
                a * ((a + 1.0) + (a - 1.0) * cs - k),
                (a + 1.0) - (a - 1.0) * cs + k,
                2.0 * ((a - 1.0) - (a + 1.0) * cs),
                (a + 1.0) - (a - 1.0) * cs - k,
            )
        }
    };
    Coeffs { b0: b0 / a0, b1: b1 / a0, b2: b2 / a0, a1: a1 / a0, a2: a2 / a0 }
}

#[derive(Clone, Copy)]
enum Biquad {
    HighPass,
    LowPass,
    HighShelf,
}

/// Per-channel state (a processing channel: L/R or M/S).
#[derive(Clone)]
struct Chan {
    env: [f32; STAGES],
    /// Auto release's slow envelopes.
    slow: [f32; STAGES],
    hold: [u32; STAGES],
    tail: f32,
    peak: f32,
    /// The detector's last two samples (for the peak between samples).
    prev: [f32; 2],
    rms: f32,
    loud: f32,
    k_weight: [Tdf2; 2],
    key: [[Tdf2; MAX_SECTIONS]; KEY_BANDS],
    sib_hp: Tdf2,
    sib_lp: Tdf2,
    /// The last output sample before makeup (the feedback detector's input).
    feedback: f32,
    delay: Vec<f32>,
}

impl Chan {
    fn new() -> Self {
        Self {
            env: [0.0; STAGES],
            slow: [0.0; STAGES],
            hold: [0; STAGES],
            tail: 0.0,
            peak: 0.0,
            prev: [0.0; 2],
            rms: 0.0,
            loud: 0.0,
            k_weight: [Tdf2::default(); 2],
            key: [[Tdf2::default(); MAX_SECTIONS]; KEY_BANDS],
            sib_hp: Tdf2::default(),
            sib_lp: Tdf2::default(),
            feedback: 0.0,
            delay: Vec::new(),
        }
    }

    fn reset(&mut self) {
        let delay = std::mem::take(&mut self.delay);
        *self = Self { delay, ..Self::new() };
        self.delay.fill(0.0);
    }
}

/// Smoothed values of one stage (continuous parameters glide; switching on and off fades, and a
/// quadrant change crossfades from the old quadrant).
#[derive(Clone)]
struct StageGlide {
    threshold: LinearSmoother,
    inv_ratio: LinearSmoother,
    knee: LinearSmoother,
    range: LinearSmoother,
    weight: LinearSmoother,
    quadrant: Quadrant,
    from_quadrant: Quadrant,
    fade: LinearSmoother,
}

impl StageGlide {
    fn new(s: &Stage) -> Self {
        let mut g = Self {
            threshold: LinearSmoother::new(s.threshold_db),
            inv_ratio: LinearSmoother::new(1.0 / s.ratio.max(1.0)),
            knee: LinearSmoother::new(s.knee_db),
            range: LinearSmoother::new(s.range_db),
            weight: LinearSmoother::new(if s.on { 1.0 } else { 0.0 }),
            quadrant: s.quadrant,
            from_quadrant: s.quadrant,
            fade: LinearSmoother::new(1.0),
        };
        g.snap(s);
        g
    }

    fn set_ramp(&mut self, sample_rate: f32) {
        for sm in [&mut self.threshold, &mut self.inv_ratio, &mut self.knee, &mut self.range, &mut self.weight, &mut self.fade] {
            sm.set_ramp(sample_rate, SMOOTH_MS);
        }
    }

    fn snap(&mut self, s: &Stage) {
        self.threshold.reset(s.threshold_db);
        self.inv_ratio.reset(1.0 / s.ratio.max(1.0));
        self.knee.reset(s.knee_db);
        self.range.reset(s.range_db);
        self.weight.reset(if s.on { 1.0 } else { 0.0 });
        self.quadrant = s.quadrant;
        self.from_quadrant = s.quadrant;
        self.fade.reset(1.0);
    }

    fn target(&mut self, s: &Stage) {
        self.threshold.set_target(s.threshold_db);
        self.inv_ratio.set_target(1.0 / s.ratio.max(1.0));
        self.knee.set_target(s.knee_db);
        self.range.set_target(s.range_db);
        self.weight.set_target(if s.on { 1.0 } else { 0.0 });
        if s.quadrant != self.quadrant {
            self.from_quadrant = self.quadrant;
            self.quadrant = s.quadrant;
            self.fade.reset(0.0);
            self.fade.set_target(1.0);
        }
    }

    fn is_smoothing(&self) -> bool {
        [&self.threshold, &self.inv_ratio, &self.knee, &self.range, &self.weight, &self.fade].iter().any(|sm| sm.is_smoothing())
    }

    /// The stage as it plays this sample: (stage, weight, crossfade from the old quadrant).
    #[inline]
    fn next(&mut self, base: &Stage) -> (Stage, f32, f32) {
        let inv = self.inv_ratio.next_value().max(1.0 / RATIO_MAX);
        let stage = Stage {
            on: true,
            quadrant: self.quadrant,
            threshold_db: self.threshold.next_value(),
            ratio: 1.0 / inv,
            knee_db: self.knee.next_value(),
            range_db: self.range.next_value(),
            ..*base
        };
        (stage, self.weight.next_value(), self.fade.next_value())
    }
}

/// Mite Comp's processor: stereo or mono, in place.
pub struct Compressor {
    /// The rate it runs at: the host's, times the oversampling factor.
    sample_rate: f32,
    /// The host's rate, and the oversampling factor it runs at (1, 2 or 4).
    base_rate: f32,
    multiple: usize,
    chans: [Chan; 2],
    glides: [StageGlide; STAGES],
    makeup: LinearSmoother,
    /// Delay line write position and its length (lookahead).
    write: usize,
    latency: usize,
    k_coeffs: [Coeffs; 2],
    sib_hp: Coeffs,
    sib_lp: Coeffs,
    key_sections: [Sections; KEY_BANDS],
    key_designed: [BandSettings; KEY_BANDS],
    peak_a: f32,
    rms_a: f32,
    loud_a: f32,
    // Smart trackers (mono, on the linked detector).
    fast_env: f32,
    slow_sq: f32,
    program_env: f32,
    crest_db: f32,
    sib_hf: f32,
    sib_bb: f32,
    sib_avg: f32,
    sib_guard: f32,
    offset_db: f32,
    /// Auto makeup: the music's mean square, the trim meters going in and coming out (before
    /// makeup), the gate's short level, the trim and the auto gain in force (dB), and the
    /// countdown to the next update.
    auto_level: f32,
    auto_in: f32,
    auto_out: f32,
    auto_short: f32,
    auto_trim: f32,
    auto_db: f32,
    auto_count: u32,
    /// Seconds of sound auto makeup has heard (its meters' times grow with it).
    auto_heard: f32,
    /// The meters' coefficients now (level, trim meters) and the trim's step.
    auto_k: (f32, f32, f32),
    /// The stages' thresholds and knees in the level they work on, while nothing glides.
    levels: [(f32, f32); STAGES],
    levels_valid: bool,
    prepared: bool,
}

impl Compressor {
    pub fn new(sample_rate: f32) -> Self {
        let s = CompSettings::default();
        let mut c = Self {
            sample_rate,
            base_rate: sample_rate,
            multiple: 1,
            chans: [Chan::new(), Chan::new()],
            glides: std::array::from_fn(|i| StageGlide::new(&s.stages[i])),
            makeup: LinearSmoother::new(0.0),
            write: 0,
            latency: 0,
            k_coeffs: [Coeffs::IDENTITY; 2],
            sib_hp: Coeffs::IDENTITY,
            sib_lp: Coeffs::IDENTITY,
            key_sections: [Sections::default(); KEY_BANDS],
            key_designed: s.key_bands,
            peak_a: 0.0,
            rms_a: 0.0,
            loud_a: 0.0,
            fast_env: 0.0,
            slow_sq: 0.0,
            program_env: 0.0,
            crest_db: CREST_REF_DB,
            sib_hf: 0.0,
            sib_bb: 0.0,
            sib_avg: 0.0,
            sib_guard: 0.0,
            offset_db: 0.0,
            auto_level: 0.0,
            auto_in: 0.0,
            auto_out: 0.0,
            auto_short: 0.0,
            auto_trim: 0.0,
            auto_db: 0.0,
            auto_count: 0,
            auto_heard: 0.0,
            auto_k: (0.0, 0.0, 0.0),
            levels: [(0.0, 0.0); STAGES],
            levels_valid: false,
            prepared: false,
        };
        c.prepare(sample_rate, &s);
        c
    }

    /// Sizes the delay lines for `sample_rate` (the host's) at any oversampling factor, and
    /// designs the filters to run at it (factor 1); starts from `s` with no glides. The only place
    /// that allocates.
    pub fn prepare(&mut self, sample_rate: f32, s: &CompSettings) {
        self.base_rate = sample_rate;
        let max = (MAX_LOOKAHEAD_MS * 0.001 * sample_rate).ceil() as usize * crate::oversample::MAX_FACTOR + 1;
        for ch in &mut self.chans {
            ch.delay = vec![0.0; max];
        }
        self.retune(1, s);
    }

    /// Runs at `multiple` (1, 2 or 4) × the host's rate from now on: the filters and time
    /// constants redesigned for it, the state cleared, `s` snapped. Allocation-free.
    pub fn retune(&mut self, multiple: usize, s: &CompSettings) {
        self.multiple = multiple.clamp(1, crate::oversample::MAX_FACTOR);
        let sample_rate = self.base_rate * self.multiple as f32;
        self.sample_rate = sample_rate;
        // K weighting (BS.1770): +4 dB above about 1.7 kHz, then a high pass near 38 Hz.
        self.k_coeffs = [
            biquad(Biquad::HighShelf, sample_rate, 1681.0, std::f32::consts::FRAC_1_SQRT_2, 4.0),
            biquad(Biquad::HighPass, sample_rate, 38.0, 0.5, 0.0),
        ];
        self.sib_hp = biquad(Biquad::HighPass, sample_rate, 4500.0, std::f32::consts::FRAC_1_SQRT_2, 0.0);
        self.sib_lp = biquad(Biquad::LowPass, sample_rate, 3000.0, std::f32::consts::FRAC_1_SQRT_2, 0.0);
        self.peak_a = coef(PEAK_RELEASE_MS, sample_rate);
        self.rms_a = coef(RMS_MS, sample_rate);
        self.loud_a = coef(LOUDNESS_MS, sample_rate);
        for (g, st) in self.glides.iter_mut().zip(&s.stages) {
            g.set_ramp(sample_rate);
            g.snap(st);
        }
        self.makeup.set_ramp(sample_rate, SMOOTH_MS);
        self.design_key(s, true);
        self.prepared = true;
        self.reset(s);
    }

    /// Clears every envelope, filter and delay line, and snaps the glides to `s`.
    pub fn reset(&mut self, s: &CompSettings) {
        for ch in &mut self.chans {
            ch.reset();
        }
        for (g, st) in self.glides.iter_mut().zip(&s.stages) {
            g.snap(st);
        }
        self.write = 0;
        self.latency = self.lookahead(s);
        self.fast_env = 0.0;
        self.slow_sq = 0.0;
        self.program_env = 0.0;
        self.crest_db = CREST_REF_DB;
        self.sib_hf = 0.0;
        self.sib_bb = 0.0;
        self.sib_avg = 0.0;
        self.sib_guard = 0.0;
        self.offset_db = 0.0;
        (self.auto_level, self.auto_in, self.auto_out, self.auto_short) = (0.0, 0.0, 0.0, 0.0);
        (self.auto_trim, self.auto_db, self.auto_count, self.auto_heard) = (0.0, 0.0, 0, 0.0);
        self.auto_k = (0.0, 0.0, 0.0);
        self.levels_valid = false;
        self.makeup.reset(s.makeup_total_db(0.0));
    }

    /// The latency in force, samples at the rate it runs at (the lookahead; a whole number of
    /// host samples).
    /// What auto makeup adds now, dB (0 while it is off).
    pub fn auto_db(&self) -> f32 {
        self.auto_db
    }

    pub fn latency(&self) -> usize {
        self.latency
    }

    /// Adds what `other` holds in its lookahead delay into this one's, aligned to the moment each
    /// will play it: a band that takes another's region (a merge) plays out what that band had
    /// delayed. Allocation-free; both must run at the same lookahead.
    pub fn absorb_delayed(&mut self, other: &Compressor) {
        let look = self.latency.min(other.latency);
        for (ch, src) in self.chans.iter_mut().zip(&other.chans) {
            let (n, m) = (ch.delay.len(), src.delay.len());
            for j in 1..=look.min(n - 1).min(m - 1) {
                ch.delay[(self.write + n - j) % n] += src.delay[(other.write + m - j) % m];
            }
        }
    }

    /// The oversampling factor it runs at.
    pub fn multiple(&self) -> usize {
        self.multiple
    }

    /// `s`'s lookahead in samples at the running rate: whole host samples, times the factor.
    fn lookahead(&self, s: &CompSettings) -> usize {
        s.latency(self.base_rate) * self.multiple
    }

    fn design_key(&mut self, s: &CompSettings, force: bool) {
        for (i, band) in s.key_bands.iter().enumerate() {
            if force || *band != self.key_designed[i] {
                self.key_sections[i] = design(band, f64::from(self.sample_rate));
                self.key_designed[i] = *band;
            }
        }
    }

    /// Processes `channels` (one or two) in place. `key` is the sidechain input (one or two
    /// channels, at least as long), used when `s.external_key` is on; without it the key is silent.
    pub fn process(&mut self, channels: &mut [&mut [f32]], key: Option<&[&[f32]]>, s: &CompSettings) -> CompTelemetry {
        let played = s.played();
        let s = &played;
        let mut t = CompTelemetry::default();
        let n_ch = channels.len().min(2);
        if n_ch == 0 || !self.prepared {
            return t;
        }
        // Linear maxima over the block until the end (dB then).
        t.detector_db = 0.0;
        t.detector_levels_db = [0.0; 3];
        let len = channels.iter().map(|c| c.len()).min().unwrap_or(0);
        let fs = self.sample_rate;
        let stereo = n_ch == 2;
        let ms = s.mid_side && stereo;
        let link = if stereo { s.link.clamp(0.0, 1.0) } else { 0.0 };
        self.design_key(s, false);
        self.latency = self.lookahead(s);
        let delay_len = self.chans[0].delay.len();
        let look = self.latency.min(delay_len - 1);

        for (g, st) in self.glides.iter_mut().zip(&s.stages) {
            g.target(st);
        }
        let order = order(&s.stages);
        // Smart: the release scale from the crest of the previous block.
        let rel_scale = adaptive_release_scale(self.crest_db, s.smart, s.smart_amount);
        let style = s.style;
        let attack_a: [f32; STAGES] = std::array::from_fn(|i| coef((s.stages[i].attack_ms * style.attack).max(0.02), fs));
        let release_a: [f32; STAGES] = std::array::from_fn(|i| coef((s.stages[i].release_ms * style.release * rel_scale).max(4.0), fs));
        let auto_a: [(f32, f32); STAGES] = std::array::from_fn(|i| auto_coefs((s.stages[i].release_ms * style.release * rel_scale).max(4.0), fs));
        let fast_a = coef(PROGRAM_FAST_MS, fs);
        let tail_a = coef(SLOW_TAIL_MS, fs);
        let hold = (s.hold_ms.max(0.0) * 0.001 * fs) as u32;

        // Smart: threshold tracking, against the main stage (the first compressing stage that
        // is on, in threshold order; else the first stage on).
        let main = order
            .iter()
            .copied()
            .find(|&i| s.stages[i].on && s.stages[i].quadrant == Quadrant::Compress)
            .or_else(|| order.iter().copied().find(|&i| s.stages[i].on));
        let tracking = s.smart.tracks_threshold() && main.is_some();
        let (main_threshold, main_slope) = main.map_or((-18.0, 1.0), |i| {
            let st = &s.stages[i];
            let slope = if st.ratio >= INF_RATIO { 1.0 } else { (1.0 - 1.0 / st.ratio.max(1.0)).max(0.05) };
            (st.threshold_db, slope)
        });
        let master = s.smart == Smart::Master;
        let swing = if master { 9.0 } else { 24.0 };
        let glide_a = 1.0 - coef(if master { 800.0 } else { 300.0 }, fs);
        let program_att = coef(if master { 1000.0 } else { 200.0 }, fs);
        let program_rel = coef(if master { 3000.0 } else { 1500.0 }, fs);
        let crest_att = coef(0.5, fs);
        let crest_rel = coef(30.0, fs);
        let slow_a = 1.0 - coef(150.0, fs);
        let sib_fast = 1.0 - coef(12.0, fs);
        let sib_slow = 1.0 - coef(1200.0, fs);
        let sib_att = 1.0 - coef(4.0, fs);
        let sib_rel = 1.0 - coef(80.0, fs);
        let sib_depth = if s.smart == Smart::Vocal { s.sibilance.clamp(0.0, 1.0) } else { 0.0 };
        let program_gate = db_to_gain(PROGRAM_GATE_DB);
        self.makeup.set_target(s.makeup_total_db(self.auto_db));
        // The meters' times grow with the sound heard (see AUTO_LEVEL_MS).
        let auto_times = |heard: f32| {
            let t = (heard * 1000.0).clamp(0.0, AUTO_LONG_S * 1000.0);
            let level = t.max(AUTO_LEVEL_MS);
            let meter = t.max(AUTO_METER_MS);
            (1.0 - coef(level, fs), 1.0 - coef(meter, fs), 1.0 - coef((3.0 * t).max(AUTO_TRIM_MS), fs / AUTO_EVERY as f32))
        };
        if self.auto_k.0 == 0.0 {
            self.auto_k = auto_times(self.auto_heard);
        }
        let auto_short_k = 1.0 - coef(AUTO_SHORT_MS, fs);
        let heard_step = AUTO_EVERY as f32 / fs;
        // The meters hold mean squares: the floor is a power (it was the amplitude, −35 dB).
        let auto_floor = db_to_gain(AUTO_FLOOR_DB).powi(2);
        let set_makeup_sq = db_to_gain(s.makeup_db).powi(2);

        let uses = |d: Detector| s.stages.iter().any(|st| st.on && st.detector == d);
        let (use_peak, use_rms, use_loud) = (uses(Detector::Peak), uses(Detector::Rms), uses(Detector::Loudness));
        let key_len = key.map_or(0, |k| k.iter().map(|c| c.len()).min().unwrap_or(0));
        let external = s.external_key;

        let mut makeup_last = (f32::NAN, 1.0);
        let mut settled: Option<[(Stage, f32, f32); STAGES]> = None;
        for i in 0..len {
            let (l, r) = (channels[0][i], if stereo { channels[1][i] } else { 0.0 });
            t.in_peak_db[0] = t.in_peak_db[0].max(l.abs());
            t.in_peak_db[1] = t.in_peak_db[1].max(if stereo { r.abs() } else { l.abs() });
            // Into the processing domain.
            let audio = if ms { [(l + r) * 0.5, (l - r) * 0.5] } else { [l, r] };
            // The detector's source.
            let mut det = if external {
                let (kl, kr) = match key {
                    Some(k) if i < key_len => (k[0][i], if k.len() > 1 { k[1][i] } else { k[0][i] }),
                    _ => (0.0, 0.0),
                };
                if ms { [(kl + kr) * 0.5, (kl - kr) * 0.5] } else if stereo { [kl, kr] } else { [0.5 * (kl + kr), 0.0] }
            } else if s.feedback {
                [self.chans[0].feedback, self.chans[1].feedback]
            } else {
                audio
            };
            // Detector EQ, then the sibilance guard (crossfades the detector towards its low part).
            let mut hf = [0.0f32; 2];
            for c in 0..n_ch {
                let ch = &mut self.chans[c];
                let mut x = f64::from(det[c]);
                for (b, sections) in self.key_sections.iter().enumerate() {
                    for (k, coeffs) in sections.as_slice().iter().enumerate() {
                        x = ch.key[b][k].process(coeffs, x);
                    }
                }
                let x = x as f32;
                // The guard's filters run only while it listens or still acts.
                if sib_depth > 0.0 || self.sib_guard > 0.0 {
                    hf[c] = ch.sib_hp.process(&self.sib_hp, f64::from(x)) as f32;
                    let lp = ch.sib_lp.process(&self.sib_lp, f64::from(x)) as f32;
                    det[c] = x + self.sib_guard * (lp - x);
                } else {
                    det[c] = x;
                }
            }
            // Levels: peak, RMS, loudness, per channel, then linked.
            let mut level = [[0.0f32; 3]; 2];
            for c in 0..n_ch {
                let ch = &mut self.chans[c];
                let x = det[c];
                // The peak between samples: where the last sample was a local maximum, the
                // parabola through it and its neighbours, so the reading does not wobble with
                // where the samples happen to fall on each cycle.
                let [a, b] = ch.prev;
                let (ya, yb, yc) = (a.abs(), b.abs(), x.abs());
                let between = if use_peak && yb > ya && yb >= yc && (a >= 0.0) == (b >= 0.0) && (b >= 0.0) == (x >= 0.0) {
                    let d = ya - 2.0 * yb + yc;
                    if d < 0.0 { yb - 0.125 * (ya - yc) * (ya - yc) / d } else { yb }
                } else {
                    0.0
                };
                ch.prev = [b, x];
                ch.peak = yc.max(between).max(self.peak_a * ch.peak);
                level[c][0] = ch.peak;
                if use_rms {
                    ch.rms = self.rms_a * ch.rms + (1.0 - self.rms_a) * x * x;
                    level[c][1] = ch.rms.sqrt();
                }
                if use_loud {
                    let shelf = ch.k_weight[0].process(&self.k_coeffs[0], f64::from(x));
                    let k = ch.k_weight[1].process(&self.k_coeffs[1], shelf) as f32;
                    ch.loud = self.loud_a * ch.loud + (1.0 - self.loud_a) * k * k;
                    level[c][2] = ch.loud.sqrt();
                }
            }
            if stereo {
                let [a, b] = &mut level;
                for (x, y) in a.iter_mut().zip(b.iter_mut()) {
                    let mx = x.max(*y);
                    *x += link * (mx - *x);
                    *y += link * (mx - *y);
                }
            }
            let mono = level[0][0].max(if stereo { level[1][0] } else { 0.0 });
            for (d, v) in t.detector_levels_db.iter_mut().enumerate() {
                *v = v.max(level[0][d]).max(if stereo { level[1][d] } else { 0.0 });
            }
            // In dB, once per detector the stages use (peak always: it is always tracked).
            let mut level_db = [[MINUS_INFINITY_DB; 3]; 2];
            for c in 0..n_ch {
                // Linked channels usually read the same: convert once.
                if c == 1 && level[1] == level[0] {
                    level_db[1] = level_db[0];
                    continue;
                }
                for (d, used) in [true, use_rms, use_loud].into_iter().enumerate() {
                    if used {
                        level_db[c][d] = gain_to_db(level[c][d]);
                    }
                }
            }

            // Smart trackers, on the linked peak (nothing reads them while Smart is off).
            if s.smart != Smart::Off {
                self.fast_env = if mono > self.fast_env { crest_att * self.fast_env + (1.0 - crest_att) * mono } else { crest_rel * self.fast_env + (1.0 - crest_rel) * mono };
                self.slow_sq += (mono * mono - self.slow_sq) * slow_a;
                if mono > program_gate {
                    self.program_env = if mono > self.program_env { program_att * self.program_env + (1.0 - program_att) * mono } else { program_rel * self.program_env + (1.0 - program_rel) * mono };
                }
            }
            if sib_depth > 0.0 {
                let hf_mag = hf[0].abs().max(hf[1].abs());
                self.sib_hf += (hf_mag * hf_mag - self.sib_hf) * sib_fast;
                self.sib_bb += (mono * mono - self.sib_bb) * sib_fast;
                let ratio = self.sib_hf.sqrt() / (self.sib_bb.sqrt() + 1e-6);
                self.sib_avg += (ratio - self.sib_avg) * sib_slow;
                let excess = ((ratio / (self.sib_avg + 1e-4) - 1.0) * 0.5).clamp(0.0, 1.0);
                let want = sib_depth * excess;
                self.sib_guard += (want - self.sib_guard) * if want > self.sib_guard { sib_att } else { sib_rel };
            } else if self.sib_guard > 0.0 {
                self.sib_guard -= self.sib_guard * sib_rel;
                // Gone: stop the filters, from rest.
                if self.sib_guard < 1e-6 {
                    self.sib_guard = 0.0;
                    for ch in &mut self.chans {
                        ch.sib_hp.reset();
                        ch.sib_lp.reset();
                    }
                }
            }
            let want_offset = if tracking {
                let program_db = gain_to_db(self.program_env);
                ((program_db - s.target_gr_db / main_slope) - main_threshold).clamp(-swing, swing) * s.smart_amount.clamp(0.0, 1.0)
            } else {
                0.0
            };
            self.offset_db += (want_offset - self.offset_db) * glide_a;

            // The stages as they play this sample. Once nothing glides they stay put for the rest
            // of this call (the settings are fixed within it), so they are worked out once.
            let stages_now = if let Some(settled) = settled {
                settled
            } else {
                // The stages, per channel, in series in threshold order.
                let mut stages_now = [(Stage::default(), 0.0f32, 1.0f32); STAGES];
                for (k, g) in self.glides.iter_mut().enumerate() {
                    stages_now[k] = g.next(&s.stages[k]);
                }
                // Thresholds and knees are on the input: each stage's, in the level it works on, is
                // what the stages below it (as they play now) make of them. Worked out again only
                // while something glides.
                let moving = self.glides.iter().any(StageGlide::is_smoothing);
                if !moving && self.levels_valid {
                    for (k, (stage, _, _)) in stages_now.iter_mut().enumerate() {
                        (stage.threshold_db, stage.knee_db) = self.levels[k];
                    }
                }
                for (n, &k) in order.iter().enumerate().filter(|_| moving || !self.levels_valid) {
                    let glides = &self.glides;
                    let (threshold, slope) = into_level(stages_now[k].0.threshold_db, |level| {
                        order[..n].iter().fold(level, |l, &j| {
                            let (stage, weight, fade) = stages_now[j];
                            let mut g = stage_gain_db(&stage, l, 0.0);
                            if fade < 1.0 {
                                g = fade * g + (1.0 - fade) * stage_gain_db(&Stage { quadrant: glides[j].from_quadrant, ..stage }, l, 0.0);
                            }
                            l + weight * g
                        })
                    });
                    stages_now[k].0.threshold_db = threshold;
                    stages_now[k].0.knee_db *= slope;
                    self.levels[k] = (threshold, stages_now[k].0.knee_db);
                }
                self.levels_valid = !moving;
                if !moving {
                    settled = Some(stages_now);
                }
                stages_now
            };
            let makeup_db = self.makeup.next_value();
            if makeup_db != makeup_last.0 {
                makeup_last = (makeup_db, db_to_gain(makeup_db));
            }
            let makeup = makeup_last.1;
            let mut out = [0.0f32; 2];
            // Linked channels that hear the same level from the same state end in the same
            // state (the usual stereo case): the second copies the first's work.
            let shared = n_ch == 2 && level_db[0] == level_db[1] && {
                let [a, b] = &self.chans;
                a.env == b.env && a.slow == b.slow && a.hold == b.hold && a.tail == b.tail
            };
            let mut first_gain_db = 0.0f32;
            let mut gain_lin = 1.0f32;
            for c in 0..n_ch {
                if c == 1 && shared {
                    let [a, b] = &mut self.chans;
                    (b.env, b.slow, b.hold, b.tail) = (a.env, a.slow, a.hold, a.tail);
                }
                let ch = &mut self.chans[c];
                let gain_db = if c == 1 && shared {
                    first_gain_db
                } else {
                    let mut sum = 0.0f32;
                    for &k in &order {
                        let (stage, weight, fade) = stages_now[k];
                        // A stage that is off and has let go adds nothing.
                        if weight == 0.0 && ch.env[k] == 0.0 {
                            continue;
                        }
                        let detected = level_db[c][stage.detector as usize] + style.drive_db - self.offset_db + sum;
                        let mu = if style.ratio_bend > 0.0 {
                            let over = if stage.quadrant.acts_above() { detected - stage.threshold_db } else { stage.threshold_db - detected };
                            style.ratio_bend * (over / 12.0).clamp(0.0, 1.0)
                        } else {
                            0.0
                        };
                        let mut target = stage_gain_db(&stage, detected, mu);
                        if fade < 1.0 {
                            let old = Stage { quadrant: self.glides[k].from_quadrant, ..stage };
                            target = fade * target + (1.0 - fade) * stage_gain_db(&old, detected, mu);
                        }
                        target *= weight;
                        let e = &mut ch.env[k];
                        if target.abs() > e.abs() {
                            *e = attack_a[k] * *e + (1.0 - attack_a[k]) * target;
                            ch.hold[k] = hold;
                        } else if ch.hold[k] > 0 {
                            ch.hold[k] -= 1;
                        } else {
                            let mut rc = release_a[k];
                            if style.program > 0.0 && e.abs() > target.abs() + PROGRAM_MARGIN_DB {
                                rc += (fast_a - rc) * style.program;
                            }
                            *e = rc * *e + (1.0 - rc) * target;
                            if weight == 0.0 && e.abs() < 1e-6 {
                                *e = 0.0;
                            }
                        }
                        if stage.auto_release || ch.slow[k] != 0.0 {
                            // Switched off, the slow envelope lets go as if the target were 0 dB,
                            // so the change never steps.
                            let aim = if stage.auto_release { target } else { 0.0 };
                            *e = auto_release(&mut ch.slow[k], *e, aim, auto_a[k]);
                            if !stage.auto_release && ch.slow[k].abs() < 1e-6 {
                                ch.slow[k] = 0.0;
                            }
                        }
                        sum += *e;
                    }
                    // The slow tail: a second, slow release stage blended in (Opto, Bus, Vari-Mu).
                    ch.tail = if sum.abs() > ch.tail.abs() { attack_a[order[0]] * ch.tail + (1.0 - attack_a[order[0]]) * sum } else { tail_a * ch.tail + (1.0 - tail_a) * sum };
                    let mut gain_db = if style.slow_tail > 0.0 { sum + (ch.tail - sum) * style.slow_tail } else { sum };
                    if let Some(k) = s.solo.filter(|&k| k < STAGES) {
                        gain_db = ch.env[k];
                    }
                    gain_db
                };
                first_gain_db = gain_db;
                t.cut_db[c] = t.cut_db[c].min(gain_db);
                t.lift_db[c] = t.lift_db[c].max(gain_db);
                // The audio, delayed by the lookahead.
                ch.delay[self.write] = audio[c];
                let read = (self.write + delay_len - look) % delay_len;
                let delayed = ch.delay[read];
                // The second of two shared channels takes the first's gain as it is.
                if !(c == 1 && shared) {
                    gain_lin = db_to_gain(gain_db);
                }
                let wet = delayed * gain_lin;
                ch.feedback = wet;
                out[c] = if s.listen { det[c] } else { wet * makeup };
            }
            self.write = (self.write + 1) % delay_len;
            // Auto makeup's meters: the power going in and coming out before makeup (not in silence).
            let power = |v: [f32; 2]| if ms { v[0] * v[0] + v[1] * v[1] } else { (v[0] * v[0] + v[1] * v[1]) / n_ch as f32 };
            let p_in = power(audio);
            // After the auto part of the makeup (not the set part): the trim closes the loop on it.
            let p_out = power(out) / set_makeup_sq;
            self.auto_short += (p_in - self.auto_short) * auto_short_k;
            let sounding = self.auto_short > auto_floor && !s.listen;
            if sounding {
                let (level_k, meter_k, _) = self.auto_k;
                self.auto_level += (p_in - self.auto_level) * level_k;
                self.auto_in += (p_in - self.auto_in) * meter_k;
                self.auto_out += (p_out - self.auto_out) * meter_k;
            }
            self.auto_count += 1;
            if self.auto_count >= AUTO_EVERY {
                self.auto_count = 0;
                if sounding && self.auto_heard < AUTO_LONG_S {
                    self.auto_heard += heard_step;
                    self.auto_k = auto_times(self.auto_heard);
                }
                let auto_step = self.auto_k.2;
                if s.auto_makeup && self.auto_level > 0.0 {
                    // The curve's gain at the music's level, then the trim towards what is measured.
                    let level = 10.0 * self.auto_level.log10();
                    let predicted = level - curve_db(&s.stages, level, self.offset_db);
                    if self.auto_in > 0.0 && self.auto_out > 0.0 {
                        // What is still missing, made up over the trim's time.
                        let error = 10.0 * (self.auto_in / self.auto_out).log10();
                        self.auto_trim = (self.auto_trim + error * auto_step).clamp(-AUTO_TRIM_LIMIT_DB, AUTO_TRIM_LIMIT_DB);
                    }
                    self.auto_db = (predicted + self.auto_trim).clamp(-24.0, 24.0);
                } else if !s.auto_makeup {
                    (self.auto_db, self.auto_trim) = (0.0, 0.0);
                }
                self.makeup.set_target(s.makeup_total_db(self.auto_db));
            }
            let (ol, or) = if ms { (out[0] + out[1], out[0] - out[1]) } else { (out[0], out[1]) };
            channels[0][i] = ol;
            if stereo {
                channels[1][i] = or;
            }
            t.out_peak_db[0] = t.out_peak_db[0].max(ol.abs());
            t.out_peak_db[1] = t.out_peak_db[1].max(if stereo { or.abs() } else { ol.abs() });
            t.detector_db = t.detector_db.max(mono);
        }
        // Crest of this block, for the next block's release scale.
        self.crest_db = gain_to_db(self.fast_env) - gain_to_db(self.slow_sq.sqrt());
        for v in t.in_peak_db.iter_mut().chain(t.out_peak_db.iter_mut()) {
            *v = gain_to_db(*v);
        }
        t.detector_db = gain_to_db(t.detector_db);
        for v in &mut t.detector_levels_db {
            *v = gain_to_db(*v);
        }
        // Each stage's gain now, from the channel it moves most.
        t.stage_db = std::array::from_fn(|k| {
            let (a, b) = (self.chans[0].env[k], self.chans[1].env[k]);
            if stereo && b.abs() > a.abs() { b } else { a }
        });
        t.smart_offset_db = self.offset_db;
        t
    }
}

/// A stage's gain (dB) over time for an input level envelope `input_db` (one value per sample at
/// `sample_rate`), through its real gain computer and ballistics with `style` — the stage panel's
/// envelope scope. UI only (allocates).
pub fn simulate_stage(stage: &Stage, style: &Style, sample_rate: f32, input_db: &[f32]) -> Vec<f32> {
    let st = Stage { on: true, ..*stage };
    let att = coef((st.attack_ms * style.attack).max(0.02), sample_rate);
    let rel = coef((st.release_ms * style.release).max(4.0), sample_rate);
    let fast = coef(PROGRAM_FAST_MS, sample_rate);
    let auto = auto_coefs((st.release_ms * style.release).max(4.0), sample_rate);
    let (mut e, mut slow) = (0.0f32, 0.0f32);
    input_db
        .iter()
        .map(|&x| {
            let target = stage_gain_db(&st, x + style.drive_db, 0.0);
            if target.abs() > e.abs() {
                e = att * e + (1.0 - att) * target;
            } else {
                let mut rc = rel;
                if style.program > 0.0 && e.abs() > target.abs() + PROGRAM_MARGIN_DB {
                    rc += (fast - rc) * style.program;
                }
                e = rc * e + (1.0 - rc) * target;
            }
            if st.auto_release {
                e = auto_release(&mut slow, e, target, auto);
            }
            e
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const FS: f32 = 48_000.0;

    fn stage(quadrant: Quadrant, threshold_db: f32, ratio: f32) -> Stage {
        Stage { on: true, quadrant, threshold_db, ratio, knee_db: 0.0, attack_ms: 0.05, release_ms: 5.0, auto_release: false, detector: Detector::Peak, ..Stage::default() }
    }

    fn settings(stages: &[Stage]) -> CompSettings {
        let mut s = CompSettings::default();
        for (slot, st) in s.stages.iter_mut().zip(stages) {
            *slot = *st;
        }
        s
    }

    /// Runs a constant level (a DC signal at `level_db`) for `seconds` and returns the gain at the
    /// end, dB.
    fn steady_gain(s: &CompSettings, level_db: f32, seconds: f32) -> f32 {
        let mut c = Compressor::new(FS);
        c.prepare(FS, s);
        let x = db_to_gain(level_db);
        let n = (seconds * FS) as usize;
        let mut buf = vec![x; n];
        c.process(&mut [&mut buf], None, s);
        gain_to_db(buf[n - 1] / x)
    }

    struct Lcg(u32);
    impl Lcg {
        fn next(&mut self) -> f32 {
            self.0 = self.0.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (self.0 >> 8) as f32 / (1u32 << 24) as f32
        }
    }

    #[test]
    fn every_curve_is_monotone() {
        let mut rng = Lcg(7);
        for _ in 0..500 {
            let mut stages = [Stage::default(); STAGES];
            for st in &mut stages {
                *st = Stage {
                    on: rng.next() > 0.3,
                    quadrant: Quadrant::from_index((rng.next() * 4.0) as usize),
                    threshold_db: -60.0 + 60.0 * rng.next(),
                    ratio: 1.0 + 30.0 * rng.next(),
                    knee_db: 24.0 * rng.next(),
                    range_db: 60.0 * rng.next(),
                    ..Stage::default()
                };
            }
            let mut last = f32::MIN;
            for i in 0..=1200 {
                let x = -90.0 + i as f32 * 0.1;
                let y = curve_db(&stages, x, 0.0);
                assert!(y >= last - 1e-3, "not monotone at {x}: {last} → {y} for {stages:?}");
                last = y;
            }
        }
    }

    #[test]
    fn no_stages_or_ratio_one_pass_audio_unchanged() {
        let mut rng = Lcg(3);
        let input: Vec<f32> = (0..4800).map(|_| rng.next() - 0.5).collect();
        for s in [settings(&[]), settings(&[Stage { ratio: 1.0, ..stage(Quadrant::Compress, -40.0, 1.0) }])] {
            let mut c = Compressor::new(FS);
            c.prepare(FS, &s);
            let (mut l, mut r) = (input.clone(), input.clone());
            c.process(&mut [&mut l, &mut r], None, &s);
            assert_eq!(l, input);
            assert_eq!(r, input);
        }
    }

    #[test]
    fn each_quadrant_has_its_slope() {
        let range = |st: Stage| Stage { range_db: 60.0, ..st };
        // 10 dB over a −20 dB threshold at 4:1 loses 7.5 dB.
        let g = steady_gain(&settings(&[range(stage(Quadrant::Compress, -20.0, 4.0))]), -10.0, 0.2);
        assert!((g + 7.5).abs() < 0.05, "compress {g}");
        // 10 dB under a −20 dB threshold at 2:1 expands by 10 dB more.
        let g = steady_gain(&settings(&[range(stage(Quadrant::Expand, -20.0, 2.0))]), -30.0, 0.2);
        assert!((g + 10.0).abs() < 0.05, "expand {g}");
        // Upward compression lifts the quiet part halfway back at 2:1.
        let g = steady_gain(&settings(&[Stage { range_db: 24.0, ..stage(Quadrant::UpCompress, -20.0, 2.0) }]), -30.0, 0.2);
        assert!((g - 5.0).abs() < 0.05, "upward compress {g}");
        // Upward expansion adds a dB per dB over at 2:1.
        let g = steady_gain(&settings(&[Stage { range_db: 24.0, ..stage(Quadrant::UpExpand, -20.0, 2.0) }]), -15.0, 0.2);
        assert!((g - 5.0).abs() < 0.05, "upward expand {g}");
    }

    #[test]
    fn range_caps_the_gain_softly() {
        // ∞:1 at −40 dB would take 30 dB off a −10 dB signal; the range stops it at 6.
        let st = Stage { range_db: 6.0, ..stage(Quadrant::Compress, -40.0, RATIO_MAX) };
        let g = steady_gain(&settings(&[st]), -10.0, 0.2);
        assert!((g + 6.0).abs() < 0.05, "{g}");
        // No corner where the range runs out (6 dB past the threshold at ∞:1).
        let stages = settings(&[st]).stages;
        let slope = |x: f32| (curve_db(&stages, x + 0.01, 0.0) - curve_db(&stages, x - 0.01, 0.0)) / 0.02;
        let mut last = slope(-39.0);
        for i in 0..400 {
            let x = -39.0 + i as f32 * 0.02;
            let now = slope(x);
            assert!((now - last).abs() < 0.05, "slope jumps at {x}: {last} → {now}");
            last = now;
        }
    }

    #[test]
    fn attack_and_release_follow_their_times() {
        let st = Stage { attack_ms: 10.0, release_ms: 100.0, ..stage(Quadrant::Compress, -20.0, RATIO_MAX) };
        let s = settings(&[st]);
        let mut c = Compressor::new(FS);
        c.prepare(FS, &s);
        let (lo, hi) = (db_to_gain(-40.0), db_to_gain(-10.0));
        let mut quiet = vec![lo; 4800];
        c.process(&mut [&mut quiet], None, &s);
        // A step to −10 dB: after one attack time, 1 − 1/e of the 10 dB.
        let mut loud = vec![hi; 4800];
        c.process(&mut [&mut loud], None, &s);
        let after_attack = gain_to_db(loud[479] / hi);
        assert!((after_attack + 10.0 * (1.0 - (-1.0f32).exp())).abs() < 0.32, "attack {after_attack}");
        // Down to −15 dB (still over the threshold, so the target is −5 dB): after one release
        // time, 1/e of the 5 dB step is left (the peak detector's 10 ms release adds a little).
        let mid = db_to_gain(-15.0);
        let mut quieter = vec![mid; 9600];
        c.process(&mut [&mut quieter], None, &s);
        let after_release = gain_to_db(quieter[4799] / mid);
        assert!((after_release + 5.0 + 5.0 * (-1.0f32).exp()).abs() < 0.2, "release {after_release}");
    }

    /// A stage at ∞:1 from −20 dB, 100 ms release (auto or not): −10 dB for `burst` samples after
    /// a quiet second, then −40 dB; the gain (dB) every sample of the recovery.
    fn recovery(auto_release: bool, burst: usize) -> Vec<f32> {
        let st = Stage { attack_ms: 1.0, release_ms: 100.0, auto_release, ..stage(Quadrant::Compress, -20.0, RATIO_MAX) };
        let s = settings(&[st]);
        let mut c = Compressor::new(FS);
        c.prepare(FS, &s);
        let (lo, hi) = (db_to_gain(-40.0), db_to_gain(-10.0));
        let mut x = vec![lo; FS as usize];
        x.extend(std::iter::repeat_n(hi, burst));
        x.extend(std::iter::repeat_n(lo, 2 * FS as usize));
        let mut y = x.clone();
        c.process(&mut [&mut y], None, &s);
        y.iter().zip(&x).skip(FS as usize + burst).map(|(a, b)| gain_to_db(a / b)).collect()
    }

    #[test]
    fn auto_release_lets_a_peak_go_quickly_and_sustained_compression_slowly() {
        // When the gain is back within 37 % of where it started (ms).
        let back = |g: &[f32]| g.iter().position(|v| v.abs() <= 0.37 * g[0].abs()).unwrap_or(g.len()) as f32 * 1000.0 / FS;
        let (peak, held) = (480, 2 * FS as usize);
        let (plain_peak, plain_held) = (back(&recovery(false, peak)), back(&recovery(false, held)));
        let (auto_peak, auto_held) = (back(&recovery(true, peak)), back(&recovery(true, held)));
        println!("recovery to 37 %: plain {plain_peak:.0} / {plain_held:.0} ms, auto {auto_peak:.0} / {auto_held:.0} ms");
        // Without it, the same release either way (the detector's 10 ms adds a little).
        assert!((plain_peak - plain_held).abs() < 15.0 && (90.0..130.0).contains(&plain_held), "{plain_peak} / {plain_held} ms");
        // With it: a 10 ms peak still lets go within 1.5 × the release ...
        assert!(auto_peak < 150.0, "peak: {auto_peak} ms");
        // ... and two seconds of compression take several times longer, smoothly (no step).
        assert!(auto_held > 3.0 * plain_held, "held: {auto_held} ms");
        let held = recovery(true, held);
        assert!(held.windows(2).all(|w| (w[1] - w[0]).abs() < 0.05 && w[1] >= w[0] - 1e-6), "the recovery steps or turns back");
    }

    #[test]
    fn auto_release_off_is_unchanged_and_the_scope_draws_the_engine() {
        // The engine's gain on a level envelope against `simulate_stage` for the same stage, with
        // auto release (a DC signal is its own level, detector Peak).
        let st = Stage { attack_ms: 5.0, release_ms: 80.0, auto_release: true, ..stage(Quadrant::Compress, -20.0, 4.0) };
        let levels: Vec<f32> = (0..96_000).map(|i| if (4800..60_000).contains(&i) { -6.0 } else { -40.0 }).collect();
        let sim = simulate_stage(&st, &Style::default(), FS, &levels);
        let s = settings(&[st]);
        let mut c = Compressor::new(FS);
        c.prepare(FS, &s);
        let x: Vec<f32> = levels.iter().map(|&l| db_to_gain(l)).collect();
        let mut y = x.clone();
        c.process(&mut [&mut y], None, &s);
        for i in (0..x.len()).step_by(97) {
            let g = gain_to_db(y[i] / x[i]);
            // The engine's peak detector releases over 10 ms where the level falls.
            let near = (i.saturating_sub(1500)..(i + 1).min(sim.len())).any(|j| (sim[j] - g).abs() < 0.3);
            assert!(near, "at {i}: engine {g:.2} dB, scope {:.2}", sim[i]);
        }
        // Switching it off mid-recovery lets the slow envelope go without a step.
        let mut s = settings(&[Stage { auto_release: true, ..st }]);
        let mut c = Compressor::new(FS);
        c.prepare(FS, &s);
        let mut a = vec![db_to_gain(-6.0); 96_000];
        c.process(&mut [&mut a], None, &s);
        s.stages[0].auto_release = false;
        let lo = db_to_gain(-40.0);
        let mut b = vec![lo; 96_000];
        c.process(&mut [&mut b], None, &s);
        let g: Vec<f32> = b.iter().map(|v| gain_to_db(v / lo)).collect();
        assert!(g.windows(2).all(|w| (w[1] - w[0]).abs() < 0.1), "switching auto release off steps");
    }

    #[test]
    fn link_moves_both_channels_together() {
        let st = stage(Quadrant::Compress, -20.0, 4.0);
        for (link, quiet_moves) in [(1.0, true), (0.0, false)] {
            let s = CompSettings { link, ..settings(&[st]) };
            let mut c = Compressor::new(FS);
            c.prepare(FS, &s);
            let (a, b) = (db_to_gain(-10.0), db_to_gain(-40.0));
            let (mut l, mut r) = (vec![a; 9600], vec![b; 9600]);
            c.process(&mut [&mut l, &mut r], None, &s);
            let g = gain_to_db(r[9599] / b);
            assert_eq!(g < -1.0, quiet_moves, "link {link}: quiet channel {g} dB");
        }
    }

    #[test]
    fn lookahead_delays_the_audio_by_its_reported_latency() {
        let s = CompSettings { lookahead_ms: 5.0, ..settings(&[]) };
        let mut c = Compressor::new(FS);
        c.prepare(FS, &s);
        let mut buf = vec![0.0f32; 1000];
        buf[0] = 1.0;
        c.process(&mut [&mut buf], None, &s);
        assert_eq!(c.latency(), 240);
        assert_eq!(buf.iter().position(|&x| x != 0.0), Some(240));
        assert_eq!(buf[240], 1.0);
    }

    #[test]
    fn moving_a_threshold_or_switching_a_stage_does_not_click() {
        let base = stage(Quadrant::Compress, -30.0, 8.0);
        let mut s = settings(&[base]);
        let mut c = Compressor::new(FS);
        c.prepare(FS, &s);
        let x = db_to_gain(-6.0);
        let mut last: Option<f32> = None;
        // The largest gain step from one sample to the next over a block.
        let mut run = |c: &mut Compressor, s: &CompSettings| {
            let mut buf = vec![x; 960];
            c.process(&mut [&mut buf], None, s);
            let mut worst = 0.0f32;
            for v in buf {
                let g = gain_to_db(v / x);
                if let Some(p) = last {
                    worst = worst.max((g - p).abs());
                }
                last = Some(g);
            }
            worst
        };
        for _ in 0..5 {
            run(&mut c, &s);
        }
        // Settled: from here on, only the parameter changes can move the gain.
        let mut worst = 0.0f32;
        s.stages[0].threshold_db = -6.0; // a 24 dB jump of the threshold
        for _ in 0..5 {
            worst = worst.max(run(&mut c, &s));
        }
        s.stages[0].on = false;
        for _ in 0..5 {
            worst = worst.max(run(&mut c, &s));
        }
        s.stages[0] = Stage { quadrant: Quadrant::Expand, threshold_db: -3.0, ..s.stages[0] };
        s.stages[0].on = true;
        for _ in 0..5 {
            worst = worst.max(run(&mut c, &s));
        }
        // Every change glides over 20 ms: the worst is a 21 dB expansion fading in (about 0.07 dB a
        // sample); a hard switch would step it by several dB in one sample.
        assert!(worst < 0.1, "largest gain step {worst} dB per sample");
    }

    /// Stereo material for auto makeup: hits every half second (a 60 Hz thump decaying over 80 ms
    /// at `peak_db`) over noise 30 dB under them, or steady noise at `peak_db − 12`.
    fn material(n: usize, peak_db: f32, hits: bool) -> Vec<f32> {
        let mut rng = Lcg(7);
        let a = db_to_gain(peak_db);
        (0..n)
            .map(|i| {
                let noise = (rng.next() - 0.5) * 2.0;
                if hits {
                    let t = (i % (FS as usize / 2)) as f32 / FS;
                    a * (-t / 0.08).exp() * (std::f32::consts::TAU * 60.0 * t).sin() + noise * a * 0.03
                } else {
                    noise * a * 0.25 * 3f32.sqrt()
                }
            })
            .collect()
    }

    fn run_blocks(s: &CompSettings, x: &[f32], block: usize) -> Vec<f32> {
        let mut c = Compressor::new(FS);
        c.prepare(FS, s);
        let mut y = x.to_vec();
        for b in y.chunks_mut(block) {
            c.process(&mut [b], None, s);
        }
        y
    }

    #[test]
    fn auto_makeup_matches_the_loudness_going_in() {
        let rms = |v: &[f32]| (v.iter().map(|x| f64::from(*x).powi(2)).sum::<f64>() / v.len() as f64).sqrt();
        // Long enough for its meters to reach their song-long times (AUTO_LONG_S).
        let n = 40 * FS as usize;
        let tail = 30 * FS as usize..;
        for (threshold, ratio) in [(-24.0, 4.0), (-30.0, 8.0), (-12.0, 2.0)] {
            for (hits, peak) in [(true, -6.0), (true, -20.0), (false, -6.0)] {
                let st = Stage { attack_ms: 10.0, release_ms: 120.0, detector: Detector::Rms, ..stage(Quadrant::Compress, threshold, ratio) };
                let s = CompSettings { auto_makeup: true, ..settings(&[st]) };
                let x = material(n, peak, hits);
                let y = run_blocks(&s, &x, 512);
                let db = 20.0 * (rms(&y[tail.clone()]) / rms(&x[tail.clone()])).log10();
                println!("{threshold} dB {ratio}:1, hits {hits} at {peak}: {db:.2} dB");
                assert!(db.abs() < 0.5, "{threshold} dB {ratio}:1, hits {hits} at {peak}: {db:.2} dB");
            }
        }
        // The set makeup adds on top; and the block size never changes the result.
        let st = stage(Quadrant::Compress, -24.0, 4.0);
        let x = material(4 * FS as usize, -6.0, true);
        let s = CompSettings { auto_makeup: true, makeup_db: 3.0, ..settings(&[st]) };
        let a = run_blocks(&s, &x, 512);
        assert_eq!(a, run_blocks(&s, &x, 37));
        let b = run_blocks(&CompSettings { makeup_db: 0.0, ..s }, &x, 512);
        let d = 20.0 * (rms(&a[3 * FS as usize..]) / rms(&b[3 * FS as usize..])).log10();
        assert!((d - 3.0).abs() < 0.05, "{d}");
    }

    #[test]
    fn smart_vocal_sets_the_ratio_and_knee_like_pro_c() {
        // Whatever ratio and knee the stage has, Vocal plays ∞:1 over a 36 dB knee: 4.5 dB of
        // reduction at the threshold, none 18 dB under it, the output held at the threshold from
        // 18 dB over it; the curve drawn (`played`) is the one heard.
        let st = Stage { release_ms: 80.0, attack_ms: 5.0, ..stage(Quadrant::Compress, -24.0, 2.0) };
        let s = CompSettings { smart: Smart::Vocal, smart_amount: 0.0, sibilance: 0.0, ..settings(&[st]) };
        let played = s.played().stages[0];
        assert_eq!((played.ratio, played.knee_db), (RATIO_MAX, VOCAL_KNEE_DB));
        assert!((curve_db(&s.played().stages, -24.0, 0.0) + 28.5).abs() < 0.01);
        assert!((curve_db(&s.played().stages, 0.0, 0.0) + 24.0).abs() < 0.01);
        for level in [-42.0, -30.0, -24.0, -12.0, 0.0] {
            let want = curve_db(&s.played().stages, level, 0.0) - level;
            let mut c = Compressor::new(FS);
            c.prepare(FS, &s);
            let x = db_to_gain(level);
            let mut buf = vec![x; (FS * 2.0) as usize];
            c.process(&mut [&mut buf], None, &s);
            let g = gain_to_db(buf[buf.len() - 1] / x);
            assert!((g - want).abs() < 0.3, "at {level} dB the gain is {g:.2}, the curve says {want:.2}");
        }
        // An expanding stage keeps its own ratio and knee; Off leaves the compressor's alone.
        let ex = stage(Quadrant::Expand, -50.0, 3.0);
        assert_eq!(ex.played(Smart::Vocal), ex);
        assert_eq!(st.played(Smart::Off), st);
        let inert = Stage { ratio: 1.0, ..st };
        assert_eq!(inert.played(Smart::Vocal), inert);
    }

    #[test]
    fn auto_makeup_does_not_ride_the_phrases() {
        // Once it has heard the song, auto makeup holds: a quiet section and a loud one come out
        // as far apart as the compressor alone makes them (it used to bring each back to its own
        // loudness, undoing the compression).
        let rms = |v: &[f32]| 20.0 * (v.iter().map(|x| f64::from(*x).powi(2)).sum::<f64>() / v.len() as f64).sqrt().log10();
        let sec = FS as usize;
        let mut x = material(30 * sec, -12.0, true);
        let quiet = material(6 * sec, -30.0, true);
        let loud = material(6 * sec, -10.0, true);
        x.extend_from_slice(&quiet);
        x.extend_from_slice(&loud);
        let st = Stage { attack_ms: 10.0, release_ms: 120.0, detector: Detector::Rms, ..stage(Quadrant::Compress, -24.0, 4.0) };
        let gap = |auto: bool| {
            let y = run_blocks(&CompSettings { auto_makeup: auto, ..settings(&[st]) }, &x, 512);
            rms(&y[36 * sec + sec..]) - rms(&y[30 * sec + sec..36 * sec])
        };
        let (with, without) = (gap(true), gap(false));
        assert!((with - without).abs() < 1.0, "loud − quiet: {with:.1} dB with auto makeup, {without:.1} dB without");
    }

    #[test]
    fn adaptive_release_is_faster_on_spiky_material() {
        let spiky = adaptive_release_scale(20.0, Smart::Adaptive, 1.0);
        let steady = adaptive_release_scale(3.0, Smart::Adaptive, 1.0);
        assert!(spiky < 1.0 && steady > 1.0, "{spiky} {steady}");
        assert_eq!(adaptive_release_scale(20.0, Smart::Off, 1.0), 1.0);
        assert_eq!(adaptive_release_scale(3.0, Smart::Vocal, 1.0), 1.0, "Vocal releases at RELEASE");
    }

    #[test]
    fn the_external_key_drives_the_detector() {
        let st = Stage { range_db: 60.0, ..stage(Quadrant::Compress, -30.0, 4.0) };
        let s = CompSettings { external_key: true, ..settings(&[st]) };
        let mut c = Compressor::new(FS);
        c.prepare(FS, &s);
        let x = db_to_gain(-40.0);
        let key = vec![db_to_gain(-10.0); 9600];
        let mut buf = vec![x; 9600];
        c.process(&mut [&mut buf], Some(&[&key]), &s);
        let g = gain_to_db(buf[9599] / x);
        assert!((g + 15.0).abs() < 0.1, "a loud key ducks a quiet track: {g}");
    }

    #[test]
    fn a_gate_stays_open_on_a_steady_tone_above_its_threshold() {
        // A 100 Hz sine at −20 dBFS peak through a fast 4:1 gate at −40 dB: the waveform crosses
        // zero 200 times a second, but the level is steady, so the gate must not chatter.
        let st = Stage { range_db: 60.0, attack_ms: 0.5, release_ms: 60.0, ..stage(Quadrant::Expand, -40.0, 4.0) };
        let s = settings(&[st]);
        let mut c = Compressor::new(FS);
        c.prepare(FS, &s);
        let input: Vec<f32> = (0..48_000).map(|i| 0.1 * (2.0 * std::f32::consts::PI * 100.0 * i as f32 / FS).sin()).collect();
        let mut buf = input.clone();
        c.process(&mut [&mut buf], None, &s);
        let worst = buf[24_000..].iter().zip(&input[24_000..]).filter(|(_, x)| x.abs() > 0.05).map(|(y, x)| gain_to_db(y / x)).fold(0.0f32, f32::min);
        assert!(worst > -0.5, "the gate closes on zero crossings: {worst} dB");
    }

    #[test]
    fn a_stage_starts_where_the_input_reaches_its_threshold() {
        // 2:1 from −30 dB, then ∞:1 from −20 dB on the input: the limiter holds what the
        // compressor makes of −20 (−25) from there on.
        let comp = Stage { range_db: 60.0, ..stage(Quadrant::Compress, -30.0, 2.0) };
        let limit = Stage { range_db: 60.0, ..stage(Quadrant::Compress, -20.0, RATIO_MAX) };
        let stages = settings(&[comp, limit]).stages;
        assert!((curve_db(&stages, -20.0, 0.0) + 25.0).abs() < 0.01);
        assert!((curve_db(&stages, -5.0, 0.0) + 25.0).abs() < 0.01);
        assert_eq!(level_thresholds(&stages)[..2], [-30.0, -25.0]);
    }

    #[test]
    fn a_higher_stage_never_changes_anything_below_it() {
        let low = Stage { knee_db: 6.0, ..stage(Quadrant::Compress, -40.0, 2.0) };
        let high = Stage { knee_db: 6.0, ..stage(Quadrant::Compress, -20.0, 4.0) };
        let before = settings(&[low, high]).stages;
        for moved in [Stage { threshold_db: -26.0, ratio: 20.0, ..high }, Stage { threshold_db: -12.0, ratio: 1.5, ..high }, Stage { on: false, ..high }] {
            let after = settings(&[low, moved]).stages;
            // The curve below the higher stage's knee (3 dB of level each side of its threshold,
            // 6 dB of input behind the 2:1 stage), and the lower stage's own curve (where its
            // node sits) everywhere, are untouched.
            for i in 0..=300 {
                let x = -60.0 + 0.2 * i as f32;
                if x < moved.threshold_db.min(high.threshold_db) - 6.1 {
                    assert_eq!(curve_db(&after, x, 0.0), curve_db(&before, x, 0.0), "at {x}");
                }
                assert_eq!(partial_curve_db(&after, 0, x, 0.0), partial_curve_db(&before, 0, x, 0.0), "at {x}");
            }
        }
    }

    #[test]
    fn solo_plays_one_stage_alone() {
        let a = Stage { range_db: 60.0, ..stage(Quadrant::Compress, -25.0, 4.0) };
        let b = Stage { range_db: 60.0, ..stage(Quadrant::Compress, -30.0, 2.0) };
        let both = steady_gain(&settings(&[a, b]), -10.0, 0.2);
        let solo = steady_gain(&CompSettings { solo: Some(1), ..settings(&[a, b]) }, -10.0, 0.2);
        // Stage b (lower threshold) runs first: −10 dB is 20 over −30 at 2:1, so it takes 10 dB;
        // a starts at an input of −25, which b leaves at −27.5, so it takes 5.625 dB more (−20 is
        // 7.5 over at 4:1).
        assert!((solo + 10.0).abs() < 0.05 && (both + 15.625).abs() < 0.05, "solo {solo}, both {both}");
    }

    #[test]
    fn the_quadrant_follows_the_pull() {
        assert_eq!(Quadrant::from_pull(false, true), Quadrant::Compress);
        assert_eq!(Quadrant::from_pull(false, false), Quadrant::Expand);
        assert_eq!(Quadrant::from_pull(true, false), Quadrant::UpCompress);
        assert_eq!(Quadrant::from_pull(true, true), Quadrant::UpExpand);
    }

    #[test]
    fn stages_run_in_threshold_order_whatever_their_slots() {
        let a = stage(Quadrant::Compress, -10.0, 4.0);
        let b = stage(Quadrant::Expand, -40.0, 2.0);
        let one = settings(&[a, b]).stages;
        let two = settings(&[b, a]).stages;
        for x in [-60.0, -40.0, -20.0, 0.0] {
            assert_eq!(curve_db(&one, x, 0.0), curve_db(&two, x, 0.0));
        }
        assert_eq!(order(&one), [1, 0, 2, 3]);
    }
}
