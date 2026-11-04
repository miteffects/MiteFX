//! Mite Delay parameters (docs/design/delay-brief.md §3).
//!
//! IDs are frozen once released (`tests/golden/mite_delay.params.txt`).

use mite_core::presets::CurrentPreset;
use mite_core::snapshots::Snapshots;
use mite_core::{BoolSpec, ChoiceSpec, ParamDisplay, ParamScale, ParamSpec};
use mite_dsp::delay::{DelaySettings, Mode, TapSettings, LOOP_BANDS, MAX_TIME_MS, MIN_TIME_MS, TAPS};
use mite_dsp::eq::{BandSettings, BandType, DEFAULT_SLOPE};
use mite_plugin::{bool_param, choice_param, float_param, float_param_named};
use nih_plug::prelude::*;

use crate::dsp::Output;

/// The state format version (`mite_plugin::state`).
pub const STATE_VERSION: u32 = 1;

#[allow(clippy::too_many_arguments)] // one line per parameter reads best
const fn spec(id: &'static str, name: &'static str, unit: &'static str, min: f32, max: f32, default: f32, scale: ParamScale, decimals: u8, wheel: (f32, f32), display: ParamDisplay) -> ParamSpec {
    ParamSpec { id, name, unit, min, max, default, scale, smoothing_ms: 20.0, decimals, wheel_step: wheel.0, wheel_step_fine: wheel.1, display }
}

// ---- The machine, modulation and space --------------------------------------------------------

pub const MODE: ChoiceSpec = ChoiceSpec { id: "mode", name: "Machine", choices: &Mode::NAMES, default: 0 };
pub const DRIVE: ParamSpec = spec("drive", "Drive", "%", 0.0, 100.0, 0.0, ParamScale::Linear, 0, (2.0, 0.5), ParamDisplay::Number);
pub const MOD_DEPTH: ParamSpec = spec("mod_depth", "Modulation", "%", 0.0, 100.0, 0.0, ParamScale::Linear, 0, (2.0, 0.5), ParamDisplay::Number);
pub const MOD_RATE: ParamSpec = spec("mod_rate", "Modulation Rate", "Hz", 0.05, 10.0, 0.8, ParamScale::Log, 2, (1.0 / 24.0, 1.0 / 96.0), ParamDisplay::Number);
pub const DIFFUSION: ParamSpec = spec("diffusion", "Diffusion", "%", 0.0, 100.0, 0.0, ParamScale::Linear, 0, (2.0, 0.5), ParamDisplay::Number);
pub const WIDTH: ParamSpec = spec("width", "Width", "%", 0.0, 150.0, 100.0, ParamScale::Linear, 0, (2.0, 0.5), ParamDisplay::Number);

// ---- Ducking ----------------------------------------------------------------------------------

pub const DUCK: ParamSpec = spec("duck", "Ducking", "dB", 0.0, 24.0, 0.0, ParamScale::Linear, 1, (0.5, 0.1), ParamDisplay::Number);
pub const DUCK_THRESH: ParamSpec = spec("duck_thresh", "Ducking Threshold", "dB", -60.0, 0.0, -30.0, ParamScale::Linear, 1, (0.5, 0.1), ParamDisplay::Number);
pub const DUCK_RELEASE: ParamSpec = spec("duck_release", "Ducking Release", "ms", 20.0, 2000.0, 300.0, ParamScale::Log, 0, (0.25, 1.0 / 16.0), ParamDisplay::Time);

// ---- Output -----------------------------------------------------------------------------------

pub const MIX: ParamSpec = spec("mix", "Mix", "%", 0.0, 100.0, 30.0, ParamScale::Linear, 0, (2.0, 0.5), ParamDisplay::Number);
pub const OUT_GAIN: ParamSpec = spec("out_gain", "Output", "dB", -24.0, 24.0, 0.0, ParamScale::Linear, 1, (0.5, 0.1), ParamDisplay::Number);
pub const BYPASS: BoolSpec = BoolSpec { id: "bypass", name: "Bypass", default: false };

// ---- The taps: `t{N}_…` -----------------------------------------------------------------------

/// Note values, shortest first, and their length in quarter notes.
pub const NOTES: [&str; 20] = ["1/64", "1/32T", "1/32", "1/16T", "1/32.", "1/16", "1/8T", "1/16.", "1/8", "1/4T", "1/8.", "1/4", "1/2T", "1/4.", "1/2", "1/1T", "1/2.", "1/1", "1/1.", "2/1"];
pub const NOTE_BEATS: [f32; 20] = [1.0 / 16.0, 1.0 / 12.0, 1.0 / 8.0, 1.0 / 6.0, 3.0 / 16.0, 0.25, 1.0 / 3.0, 3.0 / 8.0, 0.5, 2.0 / 3.0, 0.75, 1.0, 4.0 / 3.0, 1.5, 2.0, 8.0 / 3.0, 3.0, 4.0, 6.0, 8.0];
pub const QUARTER: usize = 11;

pub const T_ON: BoolSpec = BoolSpec { id: "on", name: "On", default: false };
/// The first tap is on by default: a delay with no echo does nothing.
pub const T1_ON: BoolSpec = BoolSpec { id: "on", name: "On", default: true };
pub const T_SYNC: BoolSpec = BoolSpec { id: "sync", name: "Sync", default: true };
pub const T_NOTE: ChoiceSpec = ChoiceSpec { id: "note", name: "Note", choices: &NOTES, default: QUARTER };
pub const T_TIME: ParamSpec = spec("time", "Time", "ms", MIN_TIME_MS, MAX_TIME_MS, 500.0, ParamScale::Log, 1, (1.0 / 48.0, 1.0 / 192.0), ParamDisplay::Time);
pub const T_LEVEL: ParamSpec = spec("level", "Level", "dB", -48.0, 6.0, 0.0, ParamScale::Linear, 1, (0.5, 0.1), ParamDisplay::Number);
pub const T_FEEDBACK: ParamSpec = spec("feedback", "Feedback", "%", 0.0, 100.0, 40.0, ParamScale::Linear, 0, (2.0, 0.5), ParamDisplay::Number);
pub const T_PAN: ParamSpec = spec("pan", "Pan", "%", -100.0, 100.0, 0.0, ParamScale::Linear, 0, (5.0, 1.0), ParamDisplay::Number);
pub const T_PINGPONG: BoolSpec = BoolSpec { id: "pingpong", name: "Ping-Pong", default: false };

// ---- The loop EQ: `e{N}_…` (Mite EQ's bands inside the feedback loop) --------------------------

pub const E_ON: BoolSpec = BoolSpec { id: "on", name: "On", default: false };
pub const E_TYPE: ChoiceSpec = ChoiceSpec { id: "type", name: "Type", choices: &BandType::NAMES, default: 0 };
pub const E_FREQ: ParamSpec = spec("freq", "Frequency", "Hz", 20.0, 20_000.0, 1000.0, ParamScale::Log, 0, (1.0 / 6.0, 1.0 / 48.0), ParamDisplay::Frequency);
pub const E_GAIN: ParamSpec = spec("gain", "Gain", "dB", -24.0, 24.0, 0.0, ParamScale::Linear, 1, (0.5, 0.1), ParamDisplay::Number);
pub const E_Q: ParamSpec = spec("q", "Q", "", 0.1, 10.0, std::f32::consts::FRAC_1_SQRT_2, ParamScale::Log, 2, (1.0 / 6.0, 1.0 / 24.0), ParamDisplay::Number);
pub const E_SLOPE: ChoiceSpec = ChoiceSpec { id: "slope", name: "Slope", choices: &["6 dB/oct", "12 dB/oct", "18 dB/oct", "24 dB/oct", "30 dB/oct", "36 dB/oct", "48 dB/oct", "72 dB/oct", "96 dB/oct"], default: DEFAULT_SLOPE };

/// A tap parameter's id, e.g. `t2_feedback` (`tap` from 0).
pub fn tap_id(tap: usize, param: &str) -> String {
    format!("t{}_{param}", tap + 1)
}

/// A loop band parameter's id, e.g. `e2_gain` (`band` from 0).
pub fn band_id(band: usize, param: &str) -> String {
    format!("e{}_{param}", band + 1)
}

/// A synced note's length in ms at `tempo` (bpm), held to the line's range.
pub fn note_ms(note: usize, tempo: f32) -> f32 {
    (NOTE_BEATS[note.min(NOTES.len() - 1)] * 60_000.0 / tempo.max(1.0)).clamp(MIN_TIME_MS, MAX_TIME_MS)
}

/// The note nearest `ms` at `tempo` (nearest by ratio, as the ear hears time).
pub fn nearest_note(ms: f32, tempo: f32) -> usize {
    let target = ms.max(1e-3).ln();
    (0..NOTES.len()).min_by(|&a, &b| (note_ms(a, tempo).ln() - target).abs().total_cmp(&(note_ms(b, tempo).ln() - target).abs())).unwrap_or(QUARTER)
}

const CONTINUOUS: [ParamSpec; 11] = [DRIVE, MOD_DEPTH, MOD_RATE, DIFFUSION, WIDTH, DUCK, DUCK_THRESH, DUCK_RELEASE, MIX, OUT_GAIN, T_TIME];

/// Every parameter's manifest line, in host order.
pub fn manifest() -> String {
    let mut lines = vec![MODE.manifest_line()];
    lines.extend(CONTINUOUS[..10].iter().map(ParamSpec::manifest_line));
    lines.push(BYPASS.manifest_line());
    for t in 0..TAPS {
        let prefix = |line: String| format!("t{}_{line}", t + 1);
        let on = if t == 0 { T1_ON } else { T_ON };
        lines.extend(
            [on.manifest_line(), T_SYNC.manifest_line(), T_NOTE.manifest_line(), T_TIME.manifest_line(), T_LEVEL.manifest_line(), T_FEEDBACK.manifest_line(), T_PAN.manifest_line(), T_PINGPONG.manifest_line()].map(prefix),
        );
    }
    for b in 0..LOOP_BANDS {
        let prefix = |line: String| format!("e{}_{line}", b + 1);
        lines.extend([E_ON.manifest_line(), E_TYPE.manifest_line(), E_FREQ.manifest_line(), E_GAIN.manifest_line(), E_Q.manifest_line(), E_SLOPE.manifest_line()].map(prefix));
    }
    lines.join("\n") + "\n"
}

/// Every parameter id, in host order.
pub fn ids() -> Vec<String> {
    manifest().lines().filter_map(|l| l.split(" | ").next()).map(str::to_owned).collect()
}

/// Monitoring, not sound: never in presets or snapshots.
pub fn is_monitoring(id: &str) -> bool {
    id == BYPASS.id
}

pub fn sound_ids() -> Vec<String> {
    ids().into_iter().filter(|id| !is_monitoring(id)).collect()
}

/// A tap or band parameter's bank (`'t'` or `'e'`), index (from 0) and short id.
fn split_id(id: &str) -> Option<(char, usize, &str)> {
    let (slot, short) = id.split_once('_')?;
    let bank = slot.chars().next()?;
    let i = slot[1..].parse::<usize>().ok()?.checked_sub(1)?;
    let count = match bank {
        't' => TAPS,
        'e' => LOOP_BANDS,
        _ => return None,
    };
    (i < count).then_some((bank, i, short))
}

/// Every continuous spec by its id, for stand-in hosts.
pub fn spec_for(id: &str) -> Option<ParamSpec> {
    if let Some(s) = CONTINUOUS[..10].iter().find(|s| s.id == id) {
        return Some(*s);
    }
    match split_id(id)? {
        ('t', _, "time") => Some(T_TIME),
        ('t', _, "level") => Some(T_LEVEL),
        ('t', _, "feedback") => Some(T_FEEDBACK),
        ('t', _, "pan") => Some(T_PAN),
        ('e', _, "freq") => Some(E_FREQ),
        ('e', _, "gain") => Some(E_GAIN),
        ('e', _, "q") => Some(E_Q),
        _ => None,
    }
}

/// Every choice spec by its id, for stand-in hosts.
pub fn choice_for(id: &str) -> Option<ChoiceSpec> {
    if id == MODE.id {
        return Some(MODE);
    }
    match split_id(id)? {
        ('t', _, "note") => Some(T_NOTE),
        ('e', _, "type") => Some(E_TYPE),
        ('e', _, "slope") => Some(E_SLOPE),
        _ => None,
    }
}

pub struct TapParams {
    pub on: BoolParam,
    pub sync: BoolParam,
    pub note: IntParam,
    pub time: FloatParam,
    pub level: FloatParam,
    pub feedback: FloatParam,
    pub pan: FloatParam,
    pub pingpong: BoolParam,
}

impl TapParams {
    fn new(tap: usize) -> Self {
        let name = |what: &str| format!("Tap {} {what}", tap + 1);
        Self {
            on: bool_param(if tap == 0 { &T1_ON } else { &T_ON }, name("On")),
            sync: bool_param(&T_SYNC, name("Sync")),
            note: choice_param(&T_NOTE, name("Note")),
            time: float_param_named(&T_TIME, name("Time")),
            level: float_param_named(&T_LEVEL, name("Level")),
            feedback: float_param_named(&T_FEEDBACK, name("Feedback")),
            pan: float_param_named(&T_PAN, name("Pan")),
            pingpong: bool_param(&T_PINGPONG, name("Ping-Pong")),
        }
    }
}

pub struct BandParams {
    pub on: BoolParam,
    pub kind: IntParam,
    pub freq: FloatParam,
    pub gain: FloatParam,
    pub q: FloatParam,
    pub slope: IntParam,
}

impl BandParams {
    fn new(band: usize) -> Self {
        let name = |what: &str| format!("Loop Band {} {what}", band + 1);
        Self {
            on: bool_param(&E_ON, name("On")),
            kind: choice_param(&E_TYPE, name("Type")),
            freq: float_param_named(&E_FREQ, name("Frequency")),
            gain: float_param_named(&E_GAIN, name("Gain")),
            q: float_param_named(&E_Q, name("Q")),
            slope: choice_param(&E_SLOPE, name("Slope")),
        }
    }
}

pub struct DelayParams {
    pub mode: IntParam,
    pub drive: FloatParam,
    pub mod_depth: FloatParam,
    pub mod_rate: FloatParam,
    pub diffusion: FloatParam,
    pub width: FloatParam,
    pub duck: FloatParam,
    pub duck_thresh: FloatParam,
    pub duck_release: FloatParam,
    pub mix: FloatParam,
    pub out_gain: FloatParam,
    pub bypass: BoolParam,
    pub taps: [TapParams; TAPS],
    pub bands: [BandParams; LOOP_BANDS],
    pub snapshots: std::sync::Arc<Snapshots>,
    pub preset: std::sync::Arc<CurrentPreset>,
}

impl Default for DelayParams {
    fn default() -> Self {
        Self {
            mode: choice_param(&MODE, MODE.name.to_owned()),
            drive: float_param(&DRIVE),
            mod_depth: float_param(&MOD_DEPTH),
            mod_rate: float_param(&MOD_RATE),
            diffusion: float_param(&DIFFUSION),
            width: float_param(&WIDTH),
            duck: float_param(&DUCK),
            duck_thresh: float_param(&DUCK_THRESH),
            duck_release: float_param(&DUCK_RELEASE),
            mix: float_param(&MIX),
            out_gain: float_param(&OUT_GAIN),
            // An ordinary parameter, never the host's bypass (REAPER stops processing a
            // host-bypassed plugin, and then the editor could not turn it off).
            bypass: bool_param(&BYPASS, BYPASS.name.to_owned()),
            taps: std::array::from_fn(TapParams::new),
            bands: std::array::from_fn(BandParams::new),
            snapshots: std::sync::Arc::new(Snapshots::new()),
            preset: std::sync::Arc::new(CurrentPreset::default()),
        }
    }
}

/// The engine's settings from plain values by id at `tempo` (bpm; the plugin and the editor's
/// stand-in host share it).
pub fn delay_settings(p: impl Fn(&str) -> f32, tempo: f32) -> DelaySettings {
    let index = |v: f32| v.round().max(0.0) as usize;
    DelaySettings {
        taps: std::array::from_fn(|t| {
            let v = |id: &str| p(&tap_id(t, id));
            let time_ms = if v(T_SYNC.id) >= 0.5 { note_ms(index(v(T_NOTE.id)), tempo) } else { v(T_TIME.id) };
            TapSettings {
                on: v(T_ON.id) >= 0.5,
                time_ms: time_ms.clamp(MIN_TIME_MS, MAX_TIME_MS),
                level_db: v(T_LEVEL.id),
                feedback: v(T_FEEDBACK.id) / 100.0,
                pan: v(T_PAN.id) / 100.0,
                pingpong: v(T_PINGPONG.id) >= 0.5,
            }
        }),
        mode: Mode::from_index(index(p(MODE.id))),
        drive: p(DRIVE.id) / 100.0,
        mod_depth: p(MOD_DEPTH.id) / 100.0,
        mod_rate: p(MOD_RATE.id),
        diffusion: p(DIFFUSION.id) / 100.0,
        width: p(WIDTH.id) / 100.0,
        duck_db: p(DUCK.id),
        duck_thresh_db: p(DUCK_THRESH.id),
        duck_release_ms: p(DUCK_RELEASE.id),
        eq: std::array::from_fn(|b| {
            let v = |id: &str| p(&band_id(b, id));
            BandSettings {
                enabled: v(E_ON.id) >= 0.5,
                kind: BandType::from_index(index(v(E_TYPE.id))),
                freq: f64::from(v(E_FREQ.id)),
                gain_db: f64::from(v(E_GAIN.id)),
                q: f64::from(v(E_Q.id)),
                slope: index(v(E_SLOPE.id)).min(E_SLOPE.choices.len() - 1),
                ..BandSettings::default()
            }
        }),
    }
}

impl DelayParams {
    fn plain(&self, id: &str) -> f32 {
        let f = |p: &FloatParam| p.value();
        let flag = |p: &BoolParam| f32::from(u8::from(p.value()));
        let int = |p: &IntParam| p.value() as f32;
        match id {
            "mode" => int(&self.mode),
            "drive" => f(&self.drive),
            "mod_depth" => f(&self.mod_depth),
            "mod_rate" => f(&self.mod_rate),
            "diffusion" => f(&self.diffusion),
            "width" => f(&self.width),
            "duck" => f(&self.duck),
            "duck_thresh" => f(&self.duck_thresh),
            "duck_release" => f(&self.duck_release),
            _ => match split_id(id) {
                Some(('t', t, short)) => {
                    let tap = &self.taps[t];
                    match short {
                        "on" => flag(&tap.on),
                        "sync" => flag(&tap.sync),
                        "note" => int(&tap.note),
                        "time" => f(&tap.time),
                        "level" => f(&tap.level),
                        "feedback" => f(&tap.feedback),
                        "pan" => f(&tap.pan),
                        "pingpong" => flag(&tap.pingpong),
                        _ => 0.0,
                    }
                }
                Some(('e', b, short)) => {
                    let band = &self.bands[b];
                    match short {
                        "on" => flag(&band.on),
                        "type" => int(&band.kind),
                        "freq" => f(&band.freq),
                        "gain" => f(&band.gain),
                        "q" => f(&band.q),
                        "slope" => int(&band.slope),
                        _ => 0.0,
                    }
                }
                _ => 0.0,
            },
        }
    }

    pub fn settings(&self, tempo: f32) -> DelaySettings {
        delay_settings(|id| self.plain(id), tempo)
    }

    pub fn output(&self) -> Output {
        Output { mix: self.mix.value() / 100.0, gain_db: self.out_gain.value(), bypass: self.bypass.value() }
    }
}

// SAFETY: every `ParamPtr` points into `self`, which nih-plug keeps alive (in an `Arc`) for as
// long as it uses the map. The ids match `manifest()` (checked by tests/params.rs).
unsafe impl Params for DelayParams {
    fn serialize_fields(&self) -> std::collections::BTreeMap<String, String> {
        let live = mite_plugin::state::live_values(self, &sound_ids());
        mite_plugin::state::session_fields(STATE_VERSION, &self.snapshots, &self.preset, live)
    }

    fn deserialize_fields(&self, serialized: &std::collections::BTreeMap<String, String>) {
        mite_plugin::state::load_session_fields(serialized, &self.snapshots, &self.preset);
    }

    fn param_map(&self) -> Vec<(String, ParamPtr, String)> {
        let p = |id: &str, ptr: ParamPtr, group: &str| (id.to_owned(), ptr, group.to_owned());
        let mut map = vec![
            p(MODE.id, self.mode.as_ptr(), "Machine"),
            p(DRIVE.id, self.drive.as_ptr(), "Machine"),
            p(MOD_DEPTH.id, self.mod_depth.as_ptr(), "Modulation"),
            p(MOD_RATE.id, self.mod_rate.as_ptr(), "Modulation"),
            p(DIFFUSION.id, self.diffusion.as_ptr(), "Space"),
            p(WIDTH.id, self.width.as_ptr(), "Space"),
            p(DUCK.id, self.duck.as_ptr(), "Ducking"),
            p(DUCK_THRESH.id, self.duck_thresh.as_ptr(), "Ducking"),
            p(DUCK_RELEASE.id, self.duck_release.as_ptr(), "Ducking"),
            p(MIX.id, self.mix.as_ptr(), ""),
            p(OUT_GAIN.id, self.out_gain.as_ptr(), ""),
            p(BYPASS.id, self.bypass.as_ptr(), ""),
        ];
        for (t, tap) in self.taps.iter().enumerate() {
            let group = format!("Tap {}", t + 1);
            for (id, ptr) in [
                (T_ON.id, tap.on.as_ptr()),
                (T_SYNC.id, tap.sync.as_ptr()),
                (T_NOTE.id, tap.note.as_ptr()),
                (T_TIME.id, tap.time.as_ptr()),
                (T_LEVEL.id, tap.level.as_ptr()),
                (T_FEEDBACK.id, tap.feedback.as_ptr()),
                (T_PAN.id, tap.pan.as_ptr()),
                (T_PINGPONG.id, tap.pingpong.as_ptr()),
            ] {
                map.push((tap_id(t, id), ptr, group.clone()));
            }
        }
        for (b, band) in self.bands.iter().enumerate() {
            for (id, ptr) in [(E_ON.id, band.on.as_ptr()), (E_TYPE.id, band.kind.as_ptr()), (E_FREQ.id, band.freq.as_ptr()), (E_GAIN.id, band.gain.as_ptr()), (E_Q.id, band.q.as_ptr()), (E_SLOPE.id, band.slope.as_ptr())] {
                map.push((band_id(b, id), ptr, "Loop EQ".to_owned()));
            }
        }
        map
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notes_are_in_order_and_a_quarter_is_a_beat() {
        assert!(NOTE_BEATS.windows(2).all(|w| w[0] < w[1]));
        assert_eq!(note_ms(QUARTER, 120.0), 500.0);
        assert_eq!(nearest_note(370.0, 120.0), NOTES.iter().position(|n| *n == "1/8.").expect("dotted eighth"));
        // A slow tempo's longest notes are held to the line.
        assert_eq!(note_ms(NOTES.len() - 1, 40.0), MAX_TIME_MS);
    }
}
