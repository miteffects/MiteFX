//! Mite Gate's engine: the gate (`mite_dsp::gate`) and its output stage (mix, delta and bypass
//! against the latency-aligned dry signal, output gain), plus the history telemetry: one record
//! per 5 ms slice, whatever the host's block size. A lookahead change dips (fades out, switches,
//! fades back in) and the new latency is reported.
//!
//! The MIDI trigger (brief §1.5): a note on at each opening and a note off at each closing, placed
//! where the audio does it, the lookahead's latency after the key (the host's delay compensation
//! then lines the notes up with the dry track). With the velocity taken from the key, the note
//! waits for the hit's peak ([`mite_dsp::gate::VELOCITY_WINDOW_MS`] after the opening): with at
//! least that much lookahead it still lands on the opening, without it that much late.

use mite_core::{HistoryTap, SpectrumTap};
use mite_dsp::gate::{EventKind, Gate, GateEvent, GateSettings, MAX_EVENTS, MAX_LOOKAHEAD_MS};
use mite_dsp::output::DryDelay;
use mite_dsp::{db_to_gain, LinearSmoother, MINUS_INFINITY_DB};

/// Fields of a history record (`GateShared::history`).
pub mod record {
    /// Peak in and out, L and R, dBFS.
    pub const IN: usize = 0;
    pub const OUT: usize = 2;
    /// The deepest gain, L and R, dB (≤ 0).
    pub const GAIN: usize = 4;
    /// The detector's highest level (what the thresholds are compared with), dBFS.
    pub const KEY: usize = 6;
    /// 1 while the gate is open (the Ducker: ducking).
    pub const ACTIVE: usize = 7;
    /// The velocity (1…127) of the loudest MIDI note started in the slice; 0: none.
    pub const NOTE: usize = 8;
    pub const WIDTH: usize = 9;
}

pub const HISTORY_SLICE_MS: f32 = 5.0;
pub const HISTORY_RECORDS: usize = 6000;
const CHUNK: usize = 512;
const SWITCH_MS: f32 = 10.0;
const GLIDE_MS: f32 = 20.0;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Output {
    /// 0…1.
    pub mix: f32,
    pub gain_db: f32,
    pub delta: bool,
    pub bypass: bool,
}

impl Default for Output {
    fn default() -> Self {
        Self { mix: 1.0, gain_db: 0.0, delta: false, bypass: false }
    }
}

/// The MIDI trigger's settings.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MidiSettings {
    pub on: bool,
    /// 0…127.
    pub note: u8,
    /// 0…15.
    pub channel: u8,
    /// The velocity from the key's peak (else `fixed`).
    pub from_level: bool,
    /// 1…127.
    pub fixed: u8,
}

impl Default for MidiSettings {
    fn default() -> Self {
        Self { on: false, note: 36, channel: 0, from_level: false, fixed: 100 }
    }
}

/// A MIDI note the last block sends, `offset` samples into it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MidiNote {
    pub offset: usize,
    pub on: bool,
    pub note: u8,
    pub channel: u8,
    /// 1…127 (0 for a note off).
    pub velocity: u8,
}

/// The velocity for a key peaking at `peak_db` with the threshold at `threshold_db`: 1 at the
/// threshold, 127 at full scale, straight in dB between.
pub fn level_velocity(peak_db: f32, threshold_db: f32) -> u8 {
    if threshold_db >= -0.01 {
        return 127;
    }
    let t = ((peak_db - threshold_db) / -threshold_db).clamp(0.0, 1.0);
    (1.0 + 126.0 * t).round() as u8
}

/// A note waiting for its time (absolute samples).
#[derive(Debug, Clone, Copy, Default)]
struct Due {
    time: u64,
    on: bool,
    velocity: u8,
}

const MIDI_QUEUE: usize = 64;
/// Notes one host block can send.
pub const MAX_MIDI: usize = 128;

pub struct GateEngine {
    sample_rate: f32,
    gate: Gate,
    dry: DryDelay,
    mix: LinearSmoother,
    gain: LinearSmoother,
    delta: LinearSmoother,
    bypass: LinearSmoother,
    dip: LinearSmoother,
    lookahead_ms: f32,
    slice_len: usize,
    slice_pos: usize,
    acc: [f32; record::WIDTH],
    /// The openings and closings of the last block, offsets in the host's block.
    events: [GateEvent; MAX_EVENTS],
    event_count: usize,
    pub midi: MidiSettings,
    /// The absolute sample at the start of the current host block.
    clock: u64,
    queue: [Due; MIDI_QUEUE],
    queue_head: usize,
    queue_len: usize,
    /// The latest time queued (notes never go out of order across a latency change).
    last_due: u64,
    /// An opening waiting for its peak (velocity from the key): when its note is due.
    awaiting_peak: Option<u64>,
    /// The note sounding (note, channel), so its note off matches it whatever changes meanwhile.
    held: Option<(u8, u8)>,
    release_pending: bool,
    notes: [MidiNote; MAX_MIDI],
    note_count: usize,
}

impl GateEngine {
    pub fn new(sample_rate: f32, s: &GateSettings, out: &Output) -> Self {
        let mut e = Self {
            sample_rate,
            gate: Gate::new(sample_rate),
            dry: DryDelay::new(1, 1),
            mix: LinearSmoother::new(1.0),
            gain: LinearSmoother::new(1.0),
            delta: LinearSmoother::new(0.0),
            bypass: LinearSmoother::new(0.0),
            dip: LinearSmoother::new(1.0),
            lookahead_ms: s.lookahead_ms,
            slice_len: 1,
            slice_pos: 0,
            acc: empty_record(),
            events: [GateEvent::default(); MAX_EVENTS],
            event_count: 0,
            midi: MidiSettings::default(),
            clock: 0,
            queue: [Due::default(); MIDI_QUEUE],
            queue_head: 0,
            queue_len: 0,
            last_due: 0,
            awaiting_peak: None,
            held: None,
            release_pending: false,
            notes: [MidiNote::default(); MAX_MIDI],
            note_count: 0,
        };
        e.prepare(sample_rate, s, out);
        e
    }

    /// Sizes everything for `sample_rate`. Allocates.
    pub fn prepare(&mut self, sample_rate: f32, s: &GateSettings, out: &Output) {
        self.sample_rate = sample_rate;
        self.gate = Gate::new(sample_rate);
        self.dry = DryDelay::new((MAX_LOOKAHEAD_MS * 0.001 * sample_rate).ceil() as usize + 2, CHUNK);
        self.slice_len = ((HISTORY_SLICE_MS * 0.001 * sample_rate).round() as usize).max(1);
        for sm in [&mut self.delta, &mut self.bypass, &mut self.dip] {
            sm.set_ramp(sample_rate, SWITCH_MS);
        }
        for sm in [&mut self.mix, &mut self.gain] {
            sm.set_ramp(sample_rate, GLIDE_MS);
        }
        self.reset(s, out);
    }

    pub fn reset(&mut self, s: &GateSettings, out: &Output) {
        self.lookahead_ms = s.lookahead_ms;
        self.gate.reset(&self.active(s));
        self.dry.reset();
        self.dry.set_delay(self.latency());
        self.mix.reset(out.mix);
        self.gain.reset(db_to_gain(out.gain_db));
        self.delta.reset(if out.delta { 1.0 } else { 0.0 });
        self.bypass.reset(if out.bypass { 1.0 } else { 0.0 });
        self.dip.reset(1.0);
        self.slice_pos = 0;
        self.acc = empty_record();
        self.event_count = 0;
        // A note still sounding is let go at the start of the next block (the gate starts closed).
        self.queue_len = 0;
        self.awaiting_peak = None;
        self.last_due = self.clock;
        self.release_pending = true;
    }

    /// `s` with the lookahead in force (a change waits for the dip).
    fn active(&self, s: &GateSettings) -> GateSettings {
        GateSettings { lookahead_ms: self.lookahead_ms, ..*s }
    }

    /// The latency in force, samples.
    pub fn latency(&self) -> usize {
        Gate::latency_for(&GateSettings { lookahead_ms: self.lookahead_ms, ..GateSettings::default() }, self.sample_rate)
    }

    /// The openings and closings of the last block (offsets into it).
    pub fn events(&self) -> &[GateEvent] {
        &self.events[..self.event_count]
    }

    /// The MIDI notes of the last block, in time order (offsets into it).
    pub fn midi_notes(&self) -> &[MidiNote] {
        &self.notes[..self.note_count]
    }

    fn send(&mut self, n: MidiNote) {
        if self.note_count < MAX_MIDI {
            self.notes[self.note_count] = n;
            self.note_count += 1;
        }
    }

    /// Lets go of the note sounding, if any, at `offset`.
    fn release(&mut self, offset: usize) {
        if let Some((note, channel)) = self.held.take() {
            self.send(MidiNote { offset, on: false, note, channel, velocity: 0 });
        }
    }

    fn enqueue(&mut self, time: u64, on: bool, velocity: u8) {
        if self.queue_len == MIDI_QUEUE {
            return;
        }
        let time = time.max(self.last_due);
        self.last_due = time;
        self.queue[(self.queue_head + self.queue_len) % MIDI_QUEUE] = Due { time, on, velocity };
        self.queue_len += 1;
    }

    /// The gate's events of a chunk starting at `at` (absolute) become notes due later.
    fn schedule(&mut self, events: &[GateEvent], at: u64, threshold_db: f32) {
        let latency = self.latency() as u64;
        for e in events {
            let time = at + e.offset as u64;
            match e.kind {
                EventKind::Open if self.midi.from_level => self.awaiting_peak = Some(time + latency),
                EventKind::Open => self.enqueue(time + latency, true, self.midi.fixed),
                EventKind::Peak => {
                    if let Some(due) = self.awaiting_peak.take() {
                        self.enqueue(due.max(time), true, level_velocity(e.level_db, threshold_db));
                    }
                }
                EventKind::Close => {
                    self.awaiting_peak = None;
                    self.enqueue(time + latency, false, 0);
                }
            }
        }
    }

    /// Sends the notes due before `until` (absolute); returns the loudest velocity started.
    fn send_due(&mut self, until: u64) -> u8 {
        let mut loudest = 0;
        while self.queue_len > 0 && self.queue[self.queue_head].time < until {
            let d = self.queue[self.queue_head];
            self.queue_head = (self.queue_head + 1) % MIDI_QUEUE;
            self.queue_len -= 1;
            let offset = d.time.saturating_sub(self.clock) as usize;
            self.release(offset);
            if d.on {
                let (note, channel) = (self.midi.note.min(127), self.midi.channel.min(15));
                self.send(MidiNote { offset, on: true, note, channel, velocity: d.velocity.clamp(1, 127) });
                self.held = Some((note, channel));
                loudest = loudest.max(d.velocity);
            }
        }
        loudest
    }

    #[allow(clippy::needless_range_loop)] // channels indexed with the dry path
    /// `spectrum` takes the key before the sidechain EQ (mono), for the editor's spectrum.
    pub fn process(&mut self, channels: &mut [&mut [f32]], key: Option<&[&[f32]]>, s: &GateSettings, out: &Output, history: &HistoryTap<{ record::WIDTH }>, spectrum: Option<&SpectrumTap>) {
        let n_ch = channels.len().min(2);
        self.event_count = 0;
        self.note_count = 0;
        let len = if n_ch == 0 { 0 } else { channels.iter().map(|c| c.len()).min().unwrap_or(0) };
        // Off or bypassed: nothing is sent, and a note sounding is let go.
        let triggering = self.midi.on && !out.bypass;
        if !triggering || self.release_pending {
            self.queue_len = 0;
            self.awaiting_peak = None;
            self.release(0);
            self.release_pending = false;
        }
        if n_ch == 0 {
            return;
        }
        // Listen replaces the output with the key: no mix, delta or bypass then.
        let (mix, delta, bypass) = if s.listen { (1.0, false, false) } else { (out.mix, out.delta, out.bypass) };
        self.mix.set_target(mix);
        self.delta.set_target(if delta { 1.0 } else { 0.0 });
        self.bypass.set_target(if bypass { 1.0 } else { 0.0 });
        self.gain.set_target(db_to_gain(out.gain_db));
        let mut start = 0;
        while start < len {
            if s.lookahead_ms != self.lookahead_ms {
                self.dip.set_target(0.0);
                if self.dip.current() == 0.0 {
                    self.lookahead_ms = s.lookahead_ms;
                    self.dry.set_delay(self.latency());
                    self.dip.set_target(1.0);
                }
            }
            let n = (len - start).min(CHUNK).min(self.slice_len - self.slice_pos);
            let end = start + n;
            self.dry.write(&channels[0][start..end], (n_ch == 2).then(|| &channels[1][start..end]));
            let active = self.active(s);
            let key_chunk: Option<[&[f32]; 2]> = key.filter(|k| !k.is_empty() && k.iter().all(|c| c.len() >= end)).map(|k| [&k[0][start..end], &k[k.len().min(2) - 1][start..end]]);
            if let Some(tap) = spectrum {
                // What keys the gate, before its EQ: the sidechain or the input, as one channel.
                let (a, b): (&[f32], &[f32]) = match (s.external_key, key_chunk.as_ref()) {
                    (true, Some(k)) => (k[0], k[1]),
                    (true, None) => (&[], &[]),
                    (false, _) => (&channels[0][start..end], &channels[n_ch - 1][start..end]),
                };
                if a.is_empty() {
                    tap.push(std::iter::repeat_n(0.0, n));
                } else {
                    tap.push(a.iter().zip(b).map(|(x, y)| 0.5 * (x + y)));
                }
            }
            let t = {
                let (first, rest) = channels.split_at_mut(1);
                let a = &mut first[0][start..end];
                if n_ch == 2 {
                    let b = &mut rest[0][start..end];
                    self.gate.process(&mut [a, b], key_chunk.as_ref().map(|k| &k[..]), &active)
                } else {
                    self.gate.process(&mut [a], key_chunk.as_ref().map(|k| &k[..1]), &active)
                }
            };
            let at = self.clock + start as u64;
            if triggering {
                self.schedule(&t.events[..t.event_count], at, s.threshold_db);
                let v = self.send_due(at + n as u64);
                self.acc[record::NOTE] = self.acc[record::NOTE].max(f32::from(v));
            }
            self.finish_chunk(channels, start, n, n_ch, &t, history);
            start = end;
        }
        self.clock += len as u64;
    }

    fn finish_chunk(&mut self, channels: &mut [&mut [f32]], start: usize, n: usize, n_ch: usize, t: &mite_dsp::gate::GateTelemetry, history: &HistoryTap<{ record::WIDTH }>) {
        for i in 0..n {
            let (mix, delta, bypass) = (self.mix.next_value(), self.delta.next_value(), self.bypass.next_value());
            let gain = self.gain.next_value() * self.dip.next_value();
            for (c, ch) in channels.iter_mut().enumerate().take(n_ch) {
                let wet = ch[start + i];
                let dry = self.dry.read(c, i);
                // Each blend as a·(1 − t) + b·t, so its ends are exact (mix 0 and bypass null).
                let mixed = dry * (1.0 - mix) + wet * mix;
                let y = mixed * (1.0 - delta) + (dry - wet) * delta;
                let y = y * gain;
                ch[start + i] = y * (1.0 - bypass) + dry * bypass;
            }
        }
        self.dry.advance(n);
        for e in &t.events[..t.event_count] {
            if self.event_count < MAX_EVENTS {
                self.events[self.event_count] = GateEvent { offset: e.offset + start, ..*e };
                self.event_count += 1;
            }
        }
        let a = &mut self.acc;
        for c in 0..2 {
            a[record::IN + c] = a[record::IN + c].max(t.in_peak_db[c]);
            a[record::OUT + c] = a[record::OUT + c].max(t.out_peak_db[c]);
            a[record::GAIN + c] = a[record::GAIN + c].min(t.gain_db[c]);
        }
        a[record::KEY] = a[record::KEY].max(t.key_db);
        a[record::ACTIVE] = if t.active { 1.0 } else { a[record::ACTIVE] };
        self.slice_pos += n;
        if self.slice_pos >= self.slice_len {
            history.push(&self.acc);
            self.acc = empty_record();
            self.slice_pos = 0;
        }
    }
}

fn empty_record() -> [f32; record::WIDTH] {
    let mut r = [0.0; record::WIDTH];
    for i in [record::IN, record::IN + 1, record::OUT, record::OUT + 1, record::KEY] {
        r[i] = MINUS_INFINITY_DB;
    }
    r
}

#[cfg(test)]
mod tests {
    use super::*;
    use mite_dsp::gate::Mode;

    fn tone(n: usize, db: f32) -> Vec<f32> {
        let a = db_to_gain(db);
        (0..n).map(|i| a * (i as f32 * 0.0731).sin()).collect()
    }

    fn run(e: &mut GateEngine, s: &GateSettings, out: &Output, l: &mut [f32], r: &mut [f32], block: usize) {
        let history = HistoryTap::new(64);
        for (a, b) in l.chunks_mut(block).zip(r.chunks_mut(block)) {
            e.process(&mut [a, b], None, s, out, &history, None);
        }
    }

    #[test]
    fn neutral_settings_and_bypass_null_to_the_delayed_input() {
        let x = tone(9600, -50.0);
        for (s, out) in [
            (GateSettings { range_db: 0.0, lookahead_ms: 3.0, ..GateSettings::default() }, Output::default()),
            (GateSettings { lookahead_ms: 3.0, ..GateSettings::default() }, Output { mix: 0.0, ..Output::default() }),
            (GateSettings { lookahead_ms: 3.0, ..GateSettings::default() }, Output { bypass: true, ..Output::default() }),
        ] {
            let mut e = GateEngine::new(48000.0, &s, &out);
            let (mut l, mut r) = (x.clone(), x.clone());
            run(&mut e, &s, &out, &mut l, &mut r, 333);
            let d = e.latency();
            assert_eq!(d, 144);
            for i in d..x.len() {
                assert_eq!(l[i], x[i - d], "sample {i}");
            }
        }
    }

    #[test]
    fn the_lookahead_opens_before_the_transient() {
        // Silence, then a burst: with lookahead the gate is open when the burst comes out.
        let s = GateSettings { lookahead_ms: 2.0, attack_ms: 0.5, ..GateSettings::default() };
        let out = Output::default();
        let mut e = GateEngine::new(48000.0, &s, &out);
        let mut x = vec![0.0f32; 4800];
        x.extend(tone(4800, -6.0));
        let (mut l, mut r) = (x.clone(), x.clone());
        run(&mut e, &s, &out, &mut l, &mut r, 64);
        let d = e.latency();
        // The first 0.5 ms of the burst out comes through near full level.
        let first: f32 = (0..24).map(|i| (l[4800 + d + i] - x[4800 + i]).abs()).fold(0.0, f32::max);
        assert!(first < 0.02, "{first}");
    }

    #[test]
    fn where_the_drawn_key_reaches_the_threshold_the_gate_is_open() {
        // The history's honesty: its key is what the gate compares with the threshold.
        let s = GateSettings { threshold_db: -24.0, hysteresis_db: 6.0, hold_ms: 0.0, ..GateSettings::default() };
        let out = Output::default();
        let mut e = GateEngine::new(48000.0, &s, &out);
        let history: HistoryTap<{ record::WIDTH }> = HistoryTap::new(4096);
        let mut x: Vec<f32> = Vec::new();
        for k in 0..20 {
            x.extend(tone(2400, if k % 2 == 0 { -40.0 } else { -12.0 + k as f32 * 0.3 }));
        }
        let mut r = x.clone();
        for (a, b) in x.chunks_mut(256).zip(r.chunks_mut(256)) {
            e.process(&mut [a, b], None, &s, &out, &history, None);
        }
        let (mut seen, mut over) = (0, 0);
        history.read_since(&mut seen, |rec| {
            if rec[record::KEY] >= s.threshold_db {
                over += 1;
                assert_eq!(rec[record::ACTIVE], 1.0, "key {} over the threshold, gate closed", rec[record::KEY]);
            }
        });
        assert!(over > 50);
    }

    #[test]
    fn delta_is_what_the_gate_took() {
        let s = GateSettings { mode: Mode::Expander, threshold_db: -20.0, ..GateSettings::default() };
        let x = tone(9600, -30.0);
        let out = Output::default();
        let mut e = GateEngine::new(48000.0, &s, &out);
        let (mut wet, mut wr) = (x.clone(), x.clone());
        run(&mut e, &s, &out, &mut wet, &mut wr, 256);
        let d_out = Output { delta: true, ..out };
        let mut e = GateEngine::new(48000.0, &s, &d_out);
        let (mut dl, mut dr) = (x.clone(), x.clone());
        run(&mut e, &s, &d_out, &mut dl, &mut dr, 256);
        for i in 0..x.len() {
            assert!((wet[i] + dl[i] - x[i]).abs() < 1e-6, "sample {i}");
        }
    }

    /// The notes of a whole run, at absolute times.
    fn notes(s: &GateSettings, out: &Output, midi: MidiSettings, x: &[f32], block: usize) -> Vec<(usize, MidiNote)> {
        let mut e = GateEngine::new(48000.0, s, out);
        e.midi = midi;
        let history = HistoryTap::new(64);
        let mut all = Vec::new();
        let (mut l, mut r) = (x.to_vec(), x.to_vec());
        for (k, (a, b)) in l.chunks_mut(block).zip(r.chunks_mut(block)).enumerate() {
            e.process(&mut [a, b], None, s, out, &history, None);
            all.extend(e.midi_notes().iter().map(|n| (k * block + n.offset, *n)));
        }
        all
    }

    /// Two hits (−20 and −6 dBFS peaks) rising over 1 ms, each followed by silence.
    fn hits() -> Vec<f32> {
        let mut x = vec![0.0f32; 48_000];
        for (start, db) in [(9_600usize, -20.0f32), (28_800, -6.0)] {
            let a = db_to_gain(db);
            for i in 0..4_800 {
                let t = i as f32;
                let env = if t < 48.0 { t / 48.0 } else { (-(t - 48.0) / 600.0).exp() };
                x[start + i] = a * env * (t * 0.31).sin().signum();
            }
        }
        x
    }

    const ON: MidiSettings = MidiSettings { on: true, note: 38, channel: 9, from_level: false, fixed: 90 };

    #[test]
    fn each_opening_sends_a_note_and_each_closing_its_note_off() {
        let s = GateSettings { hold_ms: 0.0, ..GateSettings::default() };
        let n = notes(&s, &Output::default(), ON, &hits(), 256);
        let kinds: Vec<bool> = n.iter().map(|(_, m)| m.on).collect();
        assert_eq!(kinds, [true, false, true, false], "{n:?}");
        for (_, m) in &n {
            assert_eq!((m.note, m.channel), (38, 9));
        }
        assert_eq!(n[0].1.velocity, 90);
        // The opening: within the hit's first millisecond (threshold −40 dB on a 1 ms rise).
        assert!(n[0].0 >= 9_600 && n[0].0 < 9_610, "{}", n[0].0);
        assert!(n[2].0 >= 28_800 && n[2].0 < 28_802, "{}", n[2].0);
    }

    #[test]
    fn notes_land_where_the_audio_opens_whatever_the_block_size() {
        let s = GateSettings { hold_ms: 0.0, lookahead_ms: 3.0, ..GateSettings::default() };
        // Absolute times and the notes (offsets differ with the block).
        let at = |v: Vec<(usize, MidiNote)>| v.into_iter().map(|(t, m)| (t, MidiNote { offset: 0, ..m })).collect::<Vec<_>>();
        let a = notes(&s, &Output::default(), ON, &hits(), 64);
        for block in [1, 333, 4096] {
            assert_eq!(at(notes(&s, &Output::default(), ON, &hits(), block)), at(a.clone()), "block {block}");
        }
        // The lookahead's latency later than without it: where the delayed audio opens.
        let s0 = GateSettings { lookahead_ms: 0.0, ..s };
        let b = notes(&s0, &Output::default(), ON, &hits(), 64);
        for (x, y) in a.iter().zip(&b) {
            assert_eq!(x.0, y.0 + 144);
        }
    }

    #[test]
    fn velocity_from_the_level_follows_the_hits_peak_and_keeps_the_timing_with_lookahead() {
        let s = GateSettings { hold_ms: 0.0, lookahead_ms: 2.0, ..GateSettings::default() };
        let level = MidiSettings { from_level: true, ..ON };
        let v = notes(&s, &Output::default(), level, &hits(), 128);
        let f = notes(&s, &Output::default(), ON, &hits(), 128);
        let ons: Vec<_> = v.iter().filter(|(_, m)| m.on).collect();
        assert_eq!(ons.len(), 2);
        // Peaks −20 and −6 dBFS over a −40 dB threshold: 1 + 126·20/40 and 1 + 126·34/40.
        assert!((i32::from(ons[0].1.velocity) - 64).abs() <= 2, "{}", ons[0].1.velocity);
        assert!((i32::from(ons[1].1.velocity) - 108).abs() <= 2, "{}", ons[1].1.velocity);
        // 2 ms of lookahead covers the velocity window: the same times as fixed velocity.
        assert_eq!(v.iter().map(|x| x.0).collect::<Vec<_>>(), f.iter().map(|x| x.0).collect::<Vec<_>>());
    }

    #[test]
    fn switching_the_trigger_off_or_bypassing_lets_the_note_go_and_sends_nothing_more() {
        let s = GateSettings { hold_ms: 0.0, ..GateSettings::default() };
        let x = hits();
        for off in [MidiSettings { on: false, ..ON }, ON] {
            let bypass = off.on;
            let out = Output { bypass, ..Output::default() };
            let mut e = GateEngine::new(48000.0, &s, &Output::default());
            e.midi = ON;
            let history = HistoryTap::new(64);
            let (mut l, mut r) = (x[..9_900].to_vec(), x[..9_900].to_vec());
            e.process(&mut [&mut l, &mut r], None, &s, &Output::default(), &history, None);
            assert!(e.midi_notes().iter().any(|m| m.on), "the first hit sounds");
            e.midi = off;
            let (mut l, mut r) = (x[9_900..].to_vec(), x[9_900..].to_vec());
            e.process(&mut [&mut l, &mut r], None, &s, &out, &history, None);
            assert_eq!(e.midi_notes(), [MidiNote { offset: 0, on: false, note: 38, channel: 9, velocity: 0 }]);
        }
    }

    #[test]
    fn a_note_change_while_one_sounds_lets_go_of_the_one_that_sounds() {
        let s = GateSettings { hold_ms: 0.0, ..GateSettings::default() };
        let x = hits();
        let mut e = GateEngine::new(48000.0, &s, &Output::default());
        e.midi = ON;
        let history = HistoryTap::new(64);
        let out = Output::default();
        let (mut l, mut r) = (x[..9_900].to_vec(), x[..9_900].to_vec());
        e.process(&mut [&mut l, &mut r], None, &s, &out, &history, None);
        e.midi.note = 40;
        let (mut l, mut r) = (x[9_900..].to_vec(), x[9_900..].to_vec());
        e.process(&mut [&mut l, &mut r], None, &s, &out, &history, None);
        let m = e.midi_notes();
        assert_eq!((m[0].on, m[0].note), (false, 38));
        assert_eq!((m[1].on, m[1].note), (true, 40));
    }

    #[test]
    fn the_history_marks_each_note_with_its_velocity() {
        let s = GateSettings { hold_ms: 0.0, ..GateSettings::default() };
        let mut e = GateEngine::new(48000.0, &s, &Output::default());
        e.midi = ON;
        let history: HistoryTap<{ record::WIDTH }> = HistoryTap::new(4096);
        let x = hits();
        let (mut l, mut r) = (x.clone(), x);
        for (a, b) in l.chunks_mut(512).zip(r.chunks_mut(512)) {
            e.process(&mut [a, b], None, &s, &Output::default(), &history, None);
        }
        let (mut seen, mut marks) = (0, Vec::new());
        history.read_since(&mut seen, |rec| {
            if rec[record::NOTE] > 0.0 {
                marks.push(rec[record::NOTE]);
            }
        });
        assert_eq!(marks, [90.0, 90.0]);
    }
}
