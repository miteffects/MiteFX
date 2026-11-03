//! Mite Gate's engine: the gate (`mite_dsp::gate`) and its output stage (mix, delta and bypass
//! against the latency-aligned dry signal, output gain), plus the history telemetry: one record
//! per 5 ms slice, whatever the host's block size. A lookahead change dips (fades out, switches,
//! fades back in) and the new latency is reported.

use mite_core::{HistoryTap, SpectrumTap};
use mite_dsp::gate::{Gate, GateEvent, GateSettings, MAX_EVENTS, MAX_LOOKAHEAD_MS};
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
    pub const WIDTH: usize = 8;
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

    #[allow(clippy::needless_range_loop)] // channels indexed with the dry path
    /// `spectrum` takes the key before the sidechain EQ (mono), for the editor's spectrum.
    pub fn process(&mut self, channels: &mut [&mut [f32]], key: Option<&[&[f32]]>, s: &GateSettings, out: &Output, history: &HistoryTap<{ record::WIDTH }>, spectrum: Option<&SpectrumTap>) {
        let n_ch = channels.len().min(2);
        self.event_count = 0;
        if n_ch == 0 {
            return;
        }
        let len = channels.iter().map(|c| c.len()).min().unwrap_or(0);
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
            self.finish_chunk(channels, start, n, n_ch, &t, history);
            start = end;
        }
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
}
