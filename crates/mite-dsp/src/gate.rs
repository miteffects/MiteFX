//! The gate behind Mite Gate (docs/design/gate-brief.md §1): gate, expander and ducker, with
//! hysteresis, hold and lookahead, a key filtered by up to three EQ bands (Comp's), linked
//! across the channels. The ideas come from Nova's NovaGate; the code is new.
//!
//! - **Detector:** the key (the input or the external sidechain) through the key bands, as Peak
//!   (instant attack, 10 ms release) or RMS (10 ms), linked (0…1) to the louder channel.
//! - **Modes** (threshold `T`, hysteresis `H`, range `R`): the Gate opens at `L ≥ T` and closes at
//!   `L < T − H` once the hold has run out; between them it keeps its state. The Expander has no
//!   state: below `T` its gain is `(L − T)(ratio − 1)`, capped at `R`, with a knee `H` wide that
//!   ends at `T` ([`expander_gain_db`]). The Ducker is the gate inverted: it ducks to `R` at
//!   `L ≥ T` and lets go below `T − H`.
//! - **Motion:** the target gain through two one-pole stages in series (each at half the time), so
//!   every opening and closing is S-shaped: attack towards the mode's active state (open for the
//!   Gate and Expander, ducked for the Ducker), release back.
//! - **Lookahead** delays the audio, not the key.
//!
//! [`transfer_db`] is the static curve the editor's square draws. Nothing allocates outside
//! [`Gate::new`].

use crate::eq::{BandSettings, MAX_SECTIONS, Sections, design};
use crate::filter::Tdf2;
use crate::{MINUS_INFINITY_DB, db_to_gain, gain_to_db};

/// Key EQ bands: Comp's three, then three more (state 2, 2026-11-03: a custom sidechain EQ).
pub const KEY_BANDS: usize = 6;
/// The longest lookahead, ms (the delay lines are sized for it).
pub const MAX_LOOKAHEAD_MS: f32 = 10.0;
/// The detectors' windows: the peak's release and the RMS's time, ms.
const PEAK_RELEASE_MS: f32 = 10.0;
const RMS_MS: f32 = 10.0;
/// MIDI-style events (openings, closings, peaks) a block reports at most.
pub const MAX_EVENTS: usize = 32;
/// After an opening, how long the key's peak is measured for a trigger's velocity, ms: a drum
/// hit crosses the threshold on its way up and reaches its peak within a couple of milliseconds.
pub const VELOCITY_WINDOW_MS: f32 = 2.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    #[default]
    Gate,
    Expander,
    Ducker,
}

impl Mode {
    pub const NAMES: [&'static str; 3] = ["Gate", "Expander", "Ducker"];
    pub const ALL: [Mode; 3] = [Mode::Gate, Mode::Expander, Mode::Ducker];

    pub fn from_index(i: usize) -> Self {
        Self::ALL[i.min(2)]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Detector {
    #[default]
    Peak,
    Rms,
}

impl Detector {
    pub const NAMES: [&'static str; 2] = ["Peak", "RMS"];

    pub fn from_index(i: usize) -> Self {
        if i == 1 {
            Detector::Rms
        } else {
            Detector::Peak
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GateSettings {
    pub mode: Mode,
    pub threshold_db: f32,
    /// ≤ 0 dB: how far a closed gate (or the ducker) turns the audio down.
    pub range_db: f32,
    /// The expander's ratio (≥ 1).
    pub ratio: f32,
    pub hysteresis_db: f32,
    pub attack_ms: f32,
    pub hold_ms: f32,
    pub release_ms: f32,
    pub lookahead_ms: f32,
    pub detector: Detector,
    /// 0…1: how far each channel's detector follows the louder one.
    pub link: f32,
    pub external_key: bool,
    pub key_bands: [BandSettings; KEY_BANDS],
    /// Hear the key as the detector does.
    pub listen: bool,
}

impl Default for GateSettings {
    fn default() -> Self {
        Self {
            mode: Mode::Gate,
            threshold_db: -40.0,
            range_db: -80.0,
            ratio: 4.0,
            hysteresis_db: 4.0,
            attack_ms: 0.5,
            hold_ms: 30.0,
            release_ms: 100.0,
            lookahead_ms: 0.0,
            detector: Detector::Peak,
            link: 1.0,
            external_key: false,
            key_bands: [BandSettings::default(); KEY_BANDS],
            listen: false,
        }
    }
}

/// The expander's gain (dB, ≤ 0) at key level `l`: `(l − T)(ratio − 1)` below `T`, capped at the
/// range, with a quadratic knee `H` wide ending at `T` (so 0 dB from `T` up, no corner).
pub fn expander_gain_db(s: &GateSettings, l: f32) -> f32 {
    let (t, k, slope) = (
        s.threshold_db,
        s.hysteresis_db.max(0.0),
        s.ratio.max(1.0) - 1.0,
    );
    let g = if l >= t {
        0.0
    } else if k > 0.0 && l > t - k {
        // From slope 0 at T to the full slope at T − k: g = −slope·(T − l)²/(2k).
        -slope * (t - l) * (t - l) / (2.0 * k)
    } else {
        -slope * (t - l - k / 2.0)
    };
    soft_floor(g, s.range_db)
}

/// `g` (≤ 0) capped at the range with a soft bend (3 dB), so the floor has no corner.
fn soft_floor(g: f32, range: f32) -> f32 {
    let r = range.min(0.0);
    if r >= 0.0 {
        return 0.0;
    }
    let w = 3.0f32.min(-r);
    if g >= r + w {
        g
    } else if g <= r - w {
        r
    } else {
        let d = g - (r - w);
        r + d * d / (4.0 * w)
    }
}

/// The static output level (dB) at key level `l` (the key is the input), for a gate that is
/// `open` (Gate and Ducker: which branch of the hysteresis loop; the Expander has one curve).
pub fn transfer_db(s: &GateSettings, l: f32, open: bool) -> f32 {
    l + match s.mode {
        Mode::Gate => {
            if open {
                0.0
            } else {
                s.range_db.min(0.0)
            }
        }
        Mode::Ducker => {
            if open {
                s.range_db.min(0.0)
            } else {
                0.0
            }
        }
        Mode::Expander => expander_gain_db(s, l),
    }
}

/// What a [`GateEvent`] marks.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum EventKind {
    /// It opened (the Ducker: began to duck; the Expander: passed half its range).
    #[default]
    Open,
    /// It closed.
    Close,
    /// [`VELOCITY_WINDOW_MS`] after an opening (or at the closing, if sooner): `level_db` is the
    /// key's peak since the opening.
    Peak,
}

/// An event at `offset` samples into the block, with the key's level (the detector's, linked).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct GateEvent {
    pub offset: usize,
    pub kind: EventKind,
    pub level_db: f32,
}

/// What a block did, for the meters and the history.
#[derive(Debug, Clone, Copy)]
pub struct GateTelemetry {
    pub in_peak_db: [f32; 2],
    pub out_peak_db: [f32; 2],
    /// The deepest gain per channel, dB (≤ 0).
    pub gain_db: [f32; 2],
    /// The detector's highest level (the louder channel), dBFS.
    pub key_db: f32,
    /// Whether the gate is open at the end (the Ducker: ducking; the Expander: past half range).
    pub active: bool,
    pub events: [GateEvent; MAX_EVENTS],
    pub event_count: usize,
}

impl Default for GateTelemetry {
    fn default() -> Self {
        Self {
            in_peak_db: [MINUS_INFINITY_DB; 2],
            out_peak_db: [MINUS_INFINITY_DB; 2],
            gain_db: [0.0; 2],
            key_db: MINUS_INFINITY_DB,
            active: false,
            events: [GateEvent::default(); MAX_EVENTS],
            event_count: 0,
        }
    }
}

#[derive(Clone)]
struct Channel {
    key: [[Tdf2; MAX_SECTIONS]; KEY_BANDS],
    peak: f32,
    ms: f32,
    /// Open (Gate, Expander) or ducking (Ducker).
    active: bool,
    hold: usize,
    /// The expander's held target, dB.
    held_db: f32,
    stages: [f32; 2],
    delay: Vec<f32>,
}

impl Channel {
    fn new(len: usize) -> Self {
        Self {
            key: [[Tdf2::default(); MAX_SECTIONS]; KEY_BANDS],
            peak: 0.0,
            ms: 0.0,
            active: false,
            hold: 0,
            held_db: 0.0,
            stages: [1.0; 2],
            delay: vec![0.0; len],
        }
    }

    fn reset(&mut self, rest: f32) {
        for band in &mut self.key {
            for f in band {
                f.reset();
            }
        }
        self.peak = 0.0;
        self.ms = 0.0;
        self.active = false;
        self.hold = 0;
        self.held_db = 0.0;
        self.stages = [rest; 2];
        self.delay.fill(0.0);
    }
}

pub struct Gate {
    sample_rate: f32,
    chans: [Channel; 2],
    sections: [Sections; KEY_BANDS],
    designed: [BandSettings; KEY_BANDS],
    write: usize,
    /// Samples left of the velocity window after an opening (0: none running), and its peak.
    vel_left: usize,
    vel_peak: f32,
}

impl Gate {
    /// Allocates (the delay lines).
    pub fn new(sample_rate: f32) -> Self {
        let len = (MAX_LOOKAHEAD_MS * 0.001 * sample_rate).ceil() as usize + 2;
        let mut g = Self {
            sample_rate,
            chans: [Channel::new(len), Channel::new(len)],
            sections: [Sections::default(); KEY_BANDS],
            designed: [BandSettings::default(); KEY_BANDS],
            write: 0,
            vel_left: 0,
            vel_peak: 0.0,
        };
        g.design(&GateSettings::default(), true);
        g
    }

    pub fn sample_rate(&self) -> f32 {
        self.sample_rate
    }

    /// The gain at rest for `s` (a Ducker rests open, a Gate closed, an Expander at its floor).
    fn rest(s: &GateSettings) -> f32 {
        match s.mode {
            Mode::Ducker => 1.0,
            _ => db_to_gain(s.range_db.min(0.0)),
        }
    }

    /// Clears the state (no allocation), the gain resting where `s` leaves it with no signal.
    pub fn reset(&mut self, s: &GateSettings) {
        let rest = Self::rest(s);
        for c in &mut self.chans {
            c.reset(rest);
        }
        self.write = 0;
        self.vel_left = 0;
        self.vel_peak = 0.0;
        self.design(s, true);
    }

    fn design(&mut self, s: &GateSettings, force: bool) {
        for (i, band) in s.key_bands.iter().enumerate() {
            if force || *band != self.designed[i] {
                self.sections[i] = design(band, f64::from(self.sample_rate));
                self.designed[i] = *band;
            }
        }
    }

    /// The lookahead in samples.
    pub fn latency_for(s: &GateSettings, sample_rate: f32) -> usize {
        (s.lookahead_ms.clamp(0.0, MAX_LOOKAHEAD_MS) * 0.001 * sample_rate).round() as usize
    }

    /// Processes `channels` (one or two) in place; `key` is the sidechain (used when
    /// `s.external_key`; silent without it).
    #[allow(clippy::needless_range_loop)] // channels and their state indexed together
    pub fn process(
        &mut self,
        channels: &mut [&mut [f32]],
        key: Option<&[&[f32]]>,
        s: &GateSettings,
    ) -> GateTelemetry {
        let mut t = GateTelemetry::default();
        let n_ch = channels.len().min(2);
        if n_ch == 0 {
            return t;
        }
        let stereo = n_ch == 2;
        self.design(s, false);
        let len = channels.iter().map(|c| c.len()).min().unwrap_or(0);
        let fs = self.sample_rate;
        let coef = |ms: f32| 1.0 - (-1.0 / (0.001 * ms.max(0.001) * fs)).exp();
        let peak_rel = (-1.0 / (0.001 * PEAK_RELEASE_MS * fs)).exp();
        let rms_k = coef(RMS_MS);
        // Each of the two stages at half the time.
        let (att, rel) = (coef(0.5 * s.attack_ms), coef(0.5 * s.release_ms));
        let hold = (s.hold_ms.max(0.0) * 0.001 * fs) as usize;
        let look = Self::latency_for(s, fs);
        let dlen = self.chans[0].delay.len();
        let range = s.range_db.min(0.0);
        let floor = db_to_gain(range);
        let (open_at, close_at) = (s.threshold_db, s.threshold_db - s.hysteresis_db.max(0.0));
        let key_len = key.map_or(0, |k| k.iter().map(|c| c.len()).min().unwrap_or(0));
        let mut lin = [0.0f32; 2];
        let mut peak_in = [0.0f32; 2];
        let mut peak_out = [0.0f32; 2];
        let mut key_max = 0.0f32;
        let mut gain_min = [1.0f32; 2];
        let vel_window = ((VELOCITY_WINDOW_MS * 0.001 * fs).round() as usize).max(1);
        let push = |t: &mut GateTelemetry, e: GateEvent| {
            if t.event_count < MAX_EVENTS {
                t.events[t.event_count] = e;
                t.event_count += 1;
            }
        };
        for i in 0..len {
            // The key, filtered, per channel.
            let mut det = [0.0f32; 2];
            for c in 0..n_ch {
                let x = channels[c][i];
                peak_in[c] = peak_in[c].max(x.abs());
                let k = if s.external_key {
                    match key {
                        Some(k) if i < key_len && !k.is_empty() => k[c.min(k.len() - 1)][i],
                        _ => 0.0,
                    }
                } else {
                    x
                };
                let mut y = f64::from(k);
                for (b, sections) in self.sections.iter().enumerate() {
                    for (st, coeffs) in self.chans[c].key[b].iter_mut().zip(sections.as_slice()) {
                        y = st.process(coeffs, y);
                    }
                }
                det[c] = y as f32;
                let ch = &mut self.chans[c];
                ch.peak = det[c].abs().max(ch.peak * peak_rel);
                ch.ms += (det[c] * det[c] - ch.ms) * rms_k;
                lin[c] = match s.detector {
                    Detector::Peak => ch.peak,
                    Detector::Rms => ch.ms.sqrt(),
                };
            }
            if stereo {
                let loud = lin[0].max(lin[1]);
                for l in &mut lin {
                    *l += (loud - *l) * s.link;
                }
            }
            key_max = key_max.max(lin[0]).max(if stereo { lin[1] } else { 0.0 });
            for c in 0..n_ch {
                let l = gain_to_db(lin[c]);
                let ch = &mut self.chans[c];
                let was = ch.active;
                // The target gain and the state.
                let target = match s.mode {
                    Mode::Gate | Mode::Ducker => {
                        if l >= open_at {
                            ch.active = true;
                            ch.hold = hold;
                        } else if l >= close_at {
                            if ch.active {
                                ch.hold = hold;
                            }
                        } else if ch.hold > 0 {
                            ch.hold -= 1;
                        } else {
                            ch.active = false;
                        }
                        let pass = (s.mode == Mode::Gate) == ch.active;
                        if pass { 1.0 } else { floor }
                    }
                    Mode::Expander => {
                        let g = expander_gain_db(s, l);
                        if g >= ch.held_db {
                            ch.held_db = g;
                            ch.hold = hold;
                        } else if ch.hold > 0 {
                            ch.hold -= 1;
                        } else {
                            ch.held_db = g;
                        }
                        ch.active = ch.held_db > 0.5 * range;
                        db_to_gain(ch.held_db)
                    }
                };
                if c == 0 {
                    let peak = |v: f32| GateEvent { offset: i, kind: EventKind::Peak, level_db: gain_to_db(v) };
                    if ch.active != was && ch.active {
                        push(&mut t, GateEvent { offset: i, kind: EventKind::Open, level_db: l });
                        self.vel_left = vel_window;
                        self.vel_peak = lin[0];
                    } else if ch.active != was {
                        if self.vel_left > 0 {
                            push(&mut t, peak(self.vel_peak));
                            self.vel_left = 0;
                        }
                        push(&mut t, GateEvent { offset: i, kind: EventKind::Close, level_db: l });
                    } else if self.vel_left > 0 {
                        self.vel_peak = self.vel_peak.max(lin[0]);
                        self.vel_left -= 1;
                        if self.vel_left == 0 {
                            push(&mut t, peak(self.vel_peak));
                        }
                    }
                }
                // Two stages: attack towards the active state, release back. For the Ducker the
                // active state is down.
                let towards_active = |from: f32, to: f32| {
                    if s.mode == Mode::Ducker {
                        to < from
                    } else {
                        to > from
                    }
                };
                let k1 = if towards_active(ch.stages[0], target) {
                    att
                } else {
                    rel
                };
                ch.stages[0] += (target - ch.stages[0]) * k1;
                let k2 = if towards_active(ch.stages[1], ch.stages[0]) {
                    att
                } else {
                    rel
                };
                ch.stages[1] += (ch.stages[0] - ch.stages[1]) * k2;
                let g = if range >= 0.0 { 1.0 } else { ch.stages[1] };
                gain_min[c] = gain_min[c].min(g);
                // The audio, delayed by the lookahead.
                let w = self.write;
                ch.delay[w] = channels[c][i];
                let delayed = ch.delay[(w + dlen - look) % dlen];
                let y = if s.listen { det[c] } else { delayed * g };
                channels[c][i] = y;
                peak_out[c] = peak_out[c].max(y.abs());
            }
            self.write = (self.write + 1) % dlen;
        }
        for c in 0..2 {
            let src = if c < n_ch { c } else { 0 };
            t.in_peak_db[c] = gain_to_db(peak_in[src]);
            t.out_peak_db[c] = gain_to_db(peak_out[src]);
            t.gain_db[c] = gain_to_db(gain_min[src]);
        }
        t.key_db = gain_to_db(key_max);
        t.active = self.chans[0].active;
        t
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FS: f32 = 48000.0;

    fn tone(n: usize, db: f32) -> Vec<f32> {
        let a = db_to_gain(db);
        (0..n).map(|i| a * (i as f32 * 0.0731).sin()).collect()
    }

    fn run(g: &mut Gate, s: &GateSettings, x: &mut [f32]) -> Vec<GateTelemetry> {
        x.chunks_mut(64)
            .map(|c| g.process(&mut [c], None, s))
            .collect()
    }

    #[test]
    fn range_zero_is_the_input_delayed_bit_for_bit() {
        for mode in Mode::ALL {
            let s = GateSettings {
                mode,
                range_db: 0.0,
                lookahead_ms: 2.0,
                ..GateSettings::default()
            };
            let mut g = Gate::new(FS);
            g.reset(&s);
            let x = tone(8000, -50.0);
            let mut y = x.clone();
            run(&mut g, &s, &mut y);
            let d = Gate::latency_for(&s, FS);
            assert_eq!(d, 96);
            for i in d..x.len() {
                assert_eq!(y[i], x[i - d], "{mode:?} sample {i}");
            }
        }
    }

    #[test]
    fn the_gate_opens_over_the_threshold_closes_under_the_hysteresis_and_holds() {
        let s = GateSettings {
            threshold_db: -30.0,
            hysteresis_db: 6.0,
            hold_ms: 20.0,
            attack_ms: 0.1,
            release_ms: 5.0,
            ..GateSettings::default()
        };
        let mut g = Gate::new(FS);
        g.reset(&s);
        // Quiet (closed), loud (opens), then −33 dB: inside the hysteresis band, it stays open.
        let mut x = tone(4800, -50.0);
        x.extend(tone(4800, -20.0));
        x.extend(tone(9600, -33.0));
        // Then −40 dB: under T − H, it closes after the hold.
        x.extend(tone(9600, -40.0));
        let t = run(&mut g, &s, &mut x);
        let block = |ms: f32| &t[(ms * 48.0 / 64.0) as usize];
        assert!(!block(90.0).active);
        assert!(block(190.0).active);
        assert!(block(390.0).active, "the hysteresis keeps it open");
        assert!(block(405.0).active, "the hold keeps it open just after");
        assert!(!block(500.0).active);
        // Open, the gain is 0 dB; closed, the range.
        assert!(block(190.0).gain_db[0] > -0.01);
        assert!(block(590.0).gain_db[0] < -79.0);
    }

    #[test]
    fn the_motion_has_no_corner() {
        // The opening: the gain's slope starts at 0 and rises smoothly (two stages).
        let s = GateSettings {
            attack_ms: 5.0,
            range_db: -40.0,
            hold_ms: 0.0,
            ..GateSettings::default()
        };
        let mut g = Gate::new(FS);
        g.reset(&s);
        let mut gains = Vec::new();
        let mut x = [0.0f32; 48];
        let mut y: Vec<f32> = tone(960, -10.0);
        for chunk in x.chunks_mut(48) {
            g.process(&mut [chunk], None, &s);
        }
        for chunk in y.chunks_mut(1) {
            gains.push(g.process(&mut [chunk], None, &s).gain_db[0]);
        }
        let lin: Vec<f32> = gains.iter().map(|&d| db_to_gain(d)).collect();
        let d1 = lin[1] - lin[0];
        let dmax = lin.windows(2).map(|w| w[1] - w[0]).fold(0.0f32, f32::max);
        assert!(d1 < 0.1 * dmax, "the first step is gentle: {d1} vs {dmax}");
    }

    #[test]
    fn the_expander_curve_is_continuous_and_capped() {
        let s = GateSettings {
            mode: Mode::Expander,
            threshold_db: -30.0,
            ratio: 3.0,
            hysteresis_db: 6.0,
            range_db: -40.0,
            ..GateSettings::default()
        };
        let mut prev = expander_gain_db(&s, 0.0);
        assert_eq!(prev, 0.0);
        for k in 1..=900 {
            let l = -k as f32 * 0.1;
            let g = expander_gain_db(&s, l);
            assert!(g <= prev + 1e-5, "monotone at {l}");
            assert!((g - prev).abs() < 0.25, "continuous at {l}");
            prev = g;
        }
        assert!((expander_gain_db(&s, -90.0) + 40.0).abs() < 1e-4);
        // Far below the knee: the full slope.
        assert!((expander_gain_db(&s, -40.0) - (-2.0 * (10.0 - 3.0))).abs() < 1e-4);
    }

    #[test]
    fn the_ducker_ducks_on_the_key() {
        let s = GateSettings {
            mode: Mode::Ducker,
            threshold_db: -30.0,
            range_db: -12.0,
            external_key: true,
            attack_ms: 1.0,
            release_ms: 20.0,
            hold_ms: 0.0,
            ..GateSettings::default()
        };
        let mut g = Gate::new(FS);
        g.reset(&s);
        let key_loud = tone(4800, -10.0);
        let mut audio = tone(4800, -20.0);
        let mut last = GateTelemetry::default();
        for (a, k) in audio.chunks_mut(64).zip(key_loud.chunks(64)) {
            last = g.process(&mut [a], Some(&[k]), &s);
        }
        assert!(
            last.active && (last.gain_db[0] + 12.0).abs() < 0.1,
            "{:?}",
            last.gain_db
        );
        let quiet = vec![0.0f32; 9600];
        let mut audio = tone(9600, -20.0);
        for (a, k) in audio.chunks_mut(64).zip(quiet.chunks(64)) {
            last = g.process(&mut [a], Some(&[k]), &s);
        }
        assert!(!last.active && last.gain_db[0] > -0.1);
    }

    #[test]
    fn openings_and_closings_are_reported_where_they_happen() {
        let s = GateSettings {
            hold_ms: 0.0,
            ..GateSettings::default()
        };
        let mut g = Gate::new(FS);
        g.reset(&s);
        // The peak detector lets go over 10 ms, so the closing comes about 2000 samples later.
        let mut x = vec![0.0f32; 4096];
        x[200..300].copy_from_slice(&tone(100, -6.0));
        let t = g.process(&mut [&mut x], None, &s);
        assert!(t.event_count >= 3);
        assert!(t.events[0].kind == EventKind::Open && t.events[0].offset >= 200 && t.events[0].offset < 205);
        assert_eq!(t.events[t.event_count - 1].kind, EventKind::Close);
    }

    /// The peak event comes the velocity window after the opening, with the hit's peak (not the
    /// level at the crossing, which is near the threshold on any hit).
    #[test]
    fn the_peak_after_an_opening_is_the_hits_peak() {
        let s = GateSettings { hold_ms: 0.0, threshold_db: -40.0, ..GateSettings::default() };
        let window = (VELOCITY_WINDOW_MS * 0.001 * FS).round() as usize;
        for top in [-30.0f32, -12.0, -3.0] {
            let mut g = Gate::new(FS);
            g.reset(&s);
            // A hit rising over 1 ms (48 samples) to `top`, then decaying.
            let a = db_to_gain(top);
            let mut x: Vec<f32> = (0..4096)
                .map(|i| {
                    if i < 500 {
                        return 0.0;
                    }
                    let t = (i - 500) as f32;
                    let env = if t < 48.0 { t / 48.0 } else { (-(t - 48.0) / 400.0).exp() };
                    a * env * if (i / 7) % 2 == 0 { 1.0 } else { -1.0 }
                })
                .collect();
            let t = g.process(&mut [&mut x], None, &s);
            let ev = &t.events[..t.event_count];
            let open = ev.iter().find(|e| e.kind == EventKind::Open).unwrap();
            let peak = ev.iter().find(|e| e.kind == EventKind::Peak).unwrap();
            assert_eq!(peak.offset - open.offset, window);
            assert!(open.level_db < top - 6.0, "the crossing is low: {}", open.level_db);
            assert!((peak.level_db - top).abs() < 0.5, "{top}: peak {}", peak.level_db);
        }
    }

    /// A closing inside the window reports the peak first, so a trigger is never left without its
    /// velocity.
    #[test]
    fn a_closing_inside_the_window_reports_the_peak_first() {
        // A click just over the threshold: the peak detector falls back under it within 0.2 ms.
        let s = GateSettings { hold_ms: 0.0, hysteresis_db: 0.0, threshold_db: -40.0, ..GateSettings::default() };
        let mut g = Gate::new(FS);
        g.reset(&s);
        let mut x = vec![0.0f32; 256];
        x[10] = db_to_gain(-39.8);
        let t = g.process(&mut [&mut x], None, &s);
        let kinds: Vec<EventKind> = t.events[..t.event_count].iter().map(|e| e.kind).collect();
        assert_eq!(kinds, [EventKind::Open, EventKind::Peak, EventKind::Close]);
    }
}
