//! Mite Sat's engine: the saturator (`mite_dsp::sat`) and its output stage (mix and delta against
//! the band sum, bypass against the latency-aligned dry signal, output gain and balance), the
//! oversampling switch behind a dip (it moves the latency), and the history telemetry: one record
//! per 5 ms slice, whatever the host's block size.

use mite_core::{HistoryTap, SpectrumTap};
use mite_dsp::output::{balance, DryDelay};
use mite_dsp::oversample::Oversampler;
use mite_dsp::sat::{SatSettings, SatTelemetry, Saturator, BANDS};
use mite_dsp::{db_to_gain, LinearSmoother, MINUS_INFINITY_DB};

/// Fields of a history record (`SatShared::history`).
pub mod record {
    use mite_dsp::sat::BANDS;

    /// Peak in and out, L and R, dBFS.
    pub const IN: usize = 0;
    pub const OUT: usize = 2;
    /// Per band slot: its peak going in (linear: the transfer curve's x) and coming out, dBFS.
    pub const BAND_IN: usize = 4;
    pub const BAND_OUT: usize = BAND_IN + BANDS;
    /// Per band slot: the operating point the curve sits at (Tube's drifts).
    pub const POINT: usize = BAND_OUT + BANDS;
    /// Per band slot: auto gain's correction, linear.
    pub const AUTO: usize = POINT + BANDS;
    pub const WIDTH: usize = 32;
}

/// One history record per this much audio.
pub const HISTORY_SLICE_MS: f32 = 5.0;
/// Records kept: 30 s.
pub const HISTORY_RECORDS: usize = 6000;
/// Longest chunk the engine processes at once (scratch buffers are this long).
const CHUNK: usize = 512;
/// Switch fades (delta, bypass, the oversampling dip) and output glides (mix, gain, balance), ms.
const SWITCH_MS: f32 = 10.0;
const GLIDE_MS: f32 = 20.0;

/// The output stage as the parameters set it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Output {
    /// 0…1.
    pub mix: f32,
    pub delta: bool,
    pub bypass: bool,
    pub gain_db: f32,
    /// −1 … 1.
    pub pan: f32,
}

impl Default for Output {
    fn default() -> Self {
        Self { mix: 1.0, delta: false, bypass: false, gain_db: 0.0, pan: 0.0 }
    }
}

pub struct SatEngine {
    sample_rate: f32,
    sat: Saturator,
    dry: DryDelay,
    wet: [Vec<f32>; 2],
    band_sum: [Vec<f32>; 2],
    mix: LinearSmoother,
    delta: LinearSmoother,
    bypass: LinearSmoother,
    gain: LinearSmoother,
    pan: LinearSmoother,
    /// The oversampling switch: fades out, changes the factor, fades back in.
    dip: LinearSmoother,
    slice_len: usize,
    slice_pos: usize,
    acc: [f32; record::WIDTH],
}

impl SatEngine {
    pub fn new(sample_rate: f32, factor: usize, s: &SatSettings) -> Self {
        let mut e = Self {
            sample_rate,
            sat: Saturator::new(sample_rate, factor, s),
            dry: DryDelay::new(1, 1),
            wet: [Vec::new(), Vec::new()],
            band_sum: [Vec::new(), Vec::new()],
            mix: LinearSmoother::new(1.0),
            delta: LinearSmoother::new(0.0),
            bypass: LinearSmoother::new(0.0),
            gain: LinearSmoother::new(1.0),
            pan: LinearSmoother::new(0.0),
            dip: LinearSmoother::new(1.0),
            slice_len: 1,
            slice_pos: 0,
            acc: empty_record(),
        };
        e.prepare(sample_rate, factor, s, &Output::default());
        e
    }

    /// Sizes everything for `sample_rate`. Allocates.
    pub fn prepare(&mut self, sample_rate: f32, factor: usize, s: &SatSettings, out: &Output) {
        self.sample_rate = sample_rate;
        self.dry = DryDelay::new(Oversampler::latency_for(8) + 1, CHUNK);
        self.wet = [vec![0.0; CHUNK], vec![0.0; CHUNK]];
        self.band_sum = [vec![0.0; CHUNK], vec![0.0; CHUNK]];
        self.slice_len = ((HISTORY_SLICE_MS * 0.001 * sample_rate).round() as usize).max(1);
        for sm in [&mut self.delta, &mut self.bypass, &mut self.dip] {
            sm.set_ramp(sample_rate, SWITCH_MS);
        }
        for sm in [&mut self.mix, &mut self.gain, &mut self.pan] {
            sm.set_ramp(sample_rate, GLIDE_MS);
        }
        self.sat.prepare(sample_rate, factor, s);
        self.reset(s, out);
    }

    /// Clears the state and snaps every glide to `s` and `out`.
    pub fn reset(&mut self, s: &SatSettings, out: &Output) {
        self.sat.reset(s);
        self.dry.reset();
        self.dry.set_delay(self.latency());
        self.mix.reset(out.mix);
        self.delta.reset(if out.delta { 1.0 } else { 0.0 });
        self.bypass.reset(if out.bypass { 1.0 } else { 0.0 });
        self.gain.reset(db_to_gain(out.gain_db));
        self.pan.reset(out.pan);
        self.dip.reset(1.0);
        self.slice_pos = 0;
        self.acc = empty_record();
    }

    /// The latency in force, samples: the oversampling's round trip (it follows a change once the
    /// dip has faded out).
    pub fn latency(&self) -> usize {
        self.sat.latency()
    }

    /// The oversampling in force.
    pub fn oversampling(&self) -> usize {
        self.sat.oversampling()
    }

    /// The saturator (for tests and tools).
    pub fn saturator(&self) -> &Saturator {
        &self.sat
    }

    /// Processes one host block in place at oversampling `factor`, pushes the history records it
    /// completes, and the input and the output to `spectra` (mono each).
    #[allow(clippy::needless_range_loop)] // channels and scratch buffers indexed together
    pub fn process(&mut self, channels: &mut [&mut [f32]], s: &SatSettings, factor: usize, out: &Output, history: &HistoryTap<{ record::WIDTH }>, spectra: Option<[&SpectrumTap; 2]>) {
        let n_ch = channels.len().min(2);
        if n_ch == 0 {
            return;
        }
        let len = channels.iter().map(|c| c.len()).min().unwrap_or(0);
        if let Some([input, _]) = spectra {
            push_mono(input, channels);
        }
        self.mix.set_target(out.mix);
        self.delta.set_target(if out.delta { 1.0 } else { 0.0 });
        self.bypass.set_target(if out.bypass { 1.0 } else { 0.0 });
        self.gain.set_target(db_to_gain(out.gain_db));
        self.pan.set_target(if n_ch == 2 { out.pan } else { 0.0 });

        let mut start = 0;
        while start < len {
            // An oversampling change moves the latency: fade out, switch once silent, fade in.
            if factor != self.sat.oversampling() {
                self.dip.set_target(0.0);
                if self.dip.current() == 0.0 {
                    self.sat.set_oversampling(factor, s);
                    self.dry.set_delay(self.latency());
                    self.dip.set_target(1.0);
                }
            }
            let n = (len - start).min(CHUNK).min(self.slice_len - self.slice_pos);
            let end = start + n;
            for c in 0..n_ch {
                self.wet[c][..n].copy_from_slice(&channels[c][start..end]);
            }
            self.dry.write(&channels[0][start..end], (n_ch == 2).then(|| &channels[1][start..end]));
            let mut in_peak = [0.0f32; 2];
            for c in 0..n_ch {
                in_peak[c] = channels[c][start..end].iter().fold(0.0f32, |m, v| m.max(v.abs()));
            }
            let telemetry = {
                let [a, b] = &mut self.wet;
                let [da, db] = &mut self.band_sum;
                let mut wet: [&mut [f32]; 2] = [&mut a[..n], &mut b[..n]];
                let mut dry: [&mut [f32]; 2] = [&mut da[..n], &mut db[..n]];
                self.sat.process(&mut wet[..n_ch], &mut dry[..n_ch], s)
            };
            let mut out_peak = [0.0f32; 2];
            for i in 0..n {
                let (mix, delta, bypass) = (self.mix.next_value(), self.delta.next_value(), self.bypass.next_value());
                let gain = self.gain.next_value() * self.dip.next_value();
                let (pl, pr) = balance(self.pan.next_value());
                for c in 0..n_ch {
                    let wet = self.wet[c][i];
                    let bands = self.band_sum[c][i];
                    let dry = self.dry.read(c, i);
                    // Each blend as a·(1 − t) + b·t, so its ends are exact (mix 0 and bypass null).
                    let mixed = bands * (1.0 - mix) + wet * mix;
                    let y = mixed * (1.0 - delta) + (wet - bands) * delta;
                    let side = if n_ch == 2 { if c == 0 { pl } else { pr } } else { 1.0 };
                    let v = (y * (1.0 - bypass) + dry * bypass) * gain * side;
                    out_peak[c] = out_peak[c].max(v.abs());
                    channels[c][start + i] = v;
                }
            }
            self.dry.advance(n);
            if n_ch == 1 {
                in_peak[1] = in_peak[0];
                out_peak[1] = out_peak[0];
            }
            self.accumulate(&telemetry, in_peak, out_peak);
            self.slice_pos += n;
            if self.slice_pos >= self.slice_len {
                history.push(&self.acc);
                self.acc = empty_record();
                self.slice_pos = 0;
            }
            start = end;
        }
        if let Some([_, output]) = spectra {
            push_mono(output, channels);
        }
    }

    fn accumulate(&mut self, t: &SatTelemetry, in_peak: [f32; 2], out_peak: [f32; 2]) {
        let a = &mut self.acc;
        let db = |x: f32| if x > 0.0 { 20.0 * x.log10() } else { MINUS_INFINITY_DB };
        for c in 0..2 {
            a[record::IN + c] = a[record::IN + c].max(db(in_peak[c]));
            a[record::OUT + c] = a[record::OUT + c].max(db(out_peak[c]));
        }
        for b in 0..BANDS {
            a[record::BAND_IN + b] = a[record::BAND_IN + b].max(db(t.in_peak[b]));
            a[record::BAND_OUT + b] = a[record::BAND_OUT + b].max(db(t.out_peak[b]));
            a[record::POINT + b] = t.point[b];
            a[record::AUTO + b] = t.auto[b];
        }
    }
}

fn push_mono(tap: &SpectrumTap, channels: &[&mut [f32]]) {
    match channels {
        [l, r, ..] => tap.push(l.iter().zip(r.iter()).map(|(a, b)| 0.5 * (a + b))),
        [m] => tap.push(m.iter().copied()),
        [] => {}
    }
}

fn empty_record() -> [f32; record::WIDTH] {
    let mut r = [0.0; record::WIDTH];
    for i in [record::IN, record::IN + 1, record::OUT, record::OUT + 1] {
        r[i] = MINUS_INFINITY_DB;
    }
    for b in 0..BANDS {
        r[record::BAND_IN + b] = MINUS_INFINITY_DB;
        r[record::BAND_OUT + b] = MINUS_INFINITY_DB;
        r[record::AUTO + b] = 1.0;
    }
    r
}

#[cfg(test)]
mod tests {
    use super::*;
    use mite_dsp::sat::Model;

    const FS: f32 = 48_000.0;

    fn driven() -> SatSettings {
        let mut s = SatSettings::default();
        s.bands[0].model = Model::Hard;
        s.bands[0].drive_db = 18.0;
        s
    }

    fn noise(n: usize, seed: u32) -> Vec<f32> {
        let mut x = seed;
        (0..n)
            .map(|_| {
                x = x.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                ((x >> 8) as f32 / (1u32 << 24) as f32 - 0.5) * 0.8
            })
            .collect()
    }

    fn run(e: &mut SatEngine, input: &[f32], s: &SatSettings, factor: usize, out: &Output, block: usize) -> Vec<f32> {
        let tap = HistoryTap::new(1024);
        let mut y = input.to_vec();
        for chunk in y.chunks_mut(block) {
            e.process(&mut [chunk], s, factor, out, &tap, None);
        }
        y
    }

    #[test]
    fn bypass_and_mix_zero_null_against_the_delayed_input() {
        let s = driven();
        let input = noise(9600, 1);
        for factor in [1, 2, 4, 8] {
            for out in [Output { bypass: true, ..Output::default() }, Output { mix: 0.0, ..Output::default() }] {
                let mut e = SatEngine::new(FS, factor, &s);
                e.prepare(FS, factor, &s, &out);
                let y = run(&mut e, &input, &s, factor, &out, 256);
                let latency = e.latency();
                assert_eq!(latency, Oversampler::latency_for(factor));
                for i in latency..input.len() {
                    assert_eq!(y[i], input[i - latency], "{factor}× {out:?} at {i}");
                }
            }
        }
    }

    #[test]
    fn delta_plus_dry_is_the_wet_signal() {
        let s = driven();
        let input = noise(9600, 2);
        let mut wet_engine = SatEngine::new(FS, 2, &s);
        let wet = run(&mut wet_engine, &input, &s, 2, &Output::default(), 480);
        let delta_out = Output { delta: true, ..Output::default() };
        let mut delta_engine = SatEngine::new(FS, 2, &s);
        delta_engine.prepare(FS, 2, &s, &delta_out);
        let delta = run(&mut delta_engine, &input, &s, 2, &delta_out, 480);
        let lat = wet_engine.latency();
        for i in lat..input.len() {
            assert!((delta[i] + input[i - lat] - wet[i]).abs() < 1e-6, "at {i}");
        }
    }

    #[test]
    fn block_size_does_not_change_the_output() {
        let mut s = driven();
        s.crossovers[0] = Some(300.0);
        s.bands[1].drive_db = 9.0;
        let input = noise(12_000, 3);
        let mut reference = None;
        for block in [1, 7, 64, 128, 4096] {
            let mut e = SatEngine::new(FS, 4, &s);
            let y = run(&mut e, &input, &s, 4, &Output { mix: 0.7, ..Output::default() }, block);
            match &reference {
                None => reference = Some(y),
                Some(r) => assert_eq!(&y, r, "block {block}"),
            }
        }
    }

    #[test]
    fn an_oversampling_change_moves_the_latency_after_a_dip() {
        let s = driven();
        let mut e = SatEngine::new(FS, 2, &s);
        let input = noise(4800, 5);
        run(&mut e, &input, &s, 2, &Output::default(), 480);
        assert_eq!(e.latency(), 69);
        run(&mut e, &input, &s, 8, &Output::default(), 480);
        assert_eq!((e.latency(), e.oversampling()), (78, 8));
    }

    #[test]
    fn history_gets_one_record_per_slice() {
        let s = driven();
        let mut e = SatEngine::new(FS, 2, &s);
        let tap = HistoryTap::new(1024);
        let mut y = noise(48_000, 4);
        for chunk in y.chunks_mut(333) {
            e.process(&mut [chunk], &s, 2, &Output::default(), &tap, None);
        }
        assert_eq!(tap.count(), 200);
        let (mut seen, mut band_in) = (0, f32::MIN);
        tap.read_since(&mut seen, |r| band_in = band_in.max(r[record::BAND_IN]));
        // The noise peaks at 0.4 (−8 dBFS).
        assert!((band_in + 7.96).abs() < 0.5, "{band_in}");
    }
}
