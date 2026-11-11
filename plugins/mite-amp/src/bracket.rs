//! Running a model at the rate it was trained at when the host runs at another: the input is
//! resampled to the model's rate, the model runs there, and its output is resampled back. Both
//! conversions are 32-tap windowed sincs (`mite_dsp::resample`), linear phase, so the whole
//! bracket is a pure delay of [`Bracket::latency`] host samples (reported to the host).
//!
//! Positions are counted in whole samples since the start (u64), so nothing drifts.

use mite_dsp::resample::SincTable;
use mite_nam::NamModel;

/// Half the sinc's taps: how far a read reaches either side of its position.
const HALF: usize = 16;
/// Ring capacities (host and model samples). Calls are cut to `MAX_CALL` host samples.
const RING: usize = 16_384;
pub const MAX_CALL: usize = 2048;

/// A mirrored ring: every sample is stored twice, `RING` apart, so any window of up to `RING`
/// samples is one contiguous slice.
struct Ring {
    buf: Vec<f32>,
}

impl Ring {
    fn new() -> Self {
        Self { buf: vec![0.0; 2 * RING] }
    }

    fn put(&mut self, at: u64, v: f32) {
        let i = (at % RING as u64) as usize;
        self.buf[i] = v;
        self.buf[i + RING] = v;
    }

    /// The 32 samples a sinc read at `pos` needs, from `start` = floor(pos) − 15 (none before 0).
    fn window(&self, start: u64) -> &[f32] {
        let i = (start % RING as u64) as usize;
        &self.buf[i..i + 2 * HALF]
    }

    fn clear(&mut self) {
        self.buf.fill(0.0);
    }
}

pub struct Bracket {
    /// Host samples per model sample.
    host_per_model: f64,
    up: SincTable,
    down: SincTable,
    host_in: Ring,
    model_out: Ring,
    /// Host samples received, model samples produced.
    host_count: u64,
    model_count: u64,
    latency: usize,
    chunk_in: Vec<f32>,
    chunk_out: Vec<f32>,
}

impl Bracket {
    /// The latency a bracket between these rates has, host samples (0: none needed).
    pub fn latency_for(host_rate: f64, model_rate: f64) -> usize {
        if (host_rate - model_rate).abs() < 0.5 {
            return 0;
        }
        // An output at host time J reads the model up to (J − L)·ρ + 16, and that model sample
        // reads the host up to its time + 16: L ≥ 16·(1 + host/model) keeps both in the past.
        (HALF as f64 * (1.0 + host_rate / model_rate)).ceil() as usize + 1
    }

    /// For running a model of `model_rate` at `host_rate` (they differ). Allocates.
    pub fn new(host_rate: f64, model_rate: f64, max_block: usize) -> Self {
        Self {
            host_per_model: host_rate / model_rate,
            up: SincTable::for_rates(host_rate, model_rate),
            down: SincTable::for_rates(model_rate, host_rate),
            host_in: Ring::new(),
            model_out: Ring::new(),
            host_count: 0,
            model_count: 0,
            latency: Self::latency_for(host_rate, model_rate),
            chunk_in: vec![0.0; max_block],
            chunk_out: vec![0.0; max_block],
        }
    }

    pub fn latency(&self) -> usize {
        self.latency
    }

    pub fn reset(&mut self) {
        self.host_in.clear();
        self.model_out.clear();
        self.host_count = 0;
        self.model_count = 0;
    }

    fn read(table: &SincTable, ring: &Ring, pos: f64) -> f32 {
        let whole = pos.floor();
        if whole < (HALF - 1) as f64 {
            // The very start: reads before sample 0 are silence.
            return 0.0;
        }
        let start = whole as u64 - (HALF as u64 - 1);
        table.read(ring.window(start), (HALF - 1) as f64 + (pos - whole))
    }

    /// `x` (host rate) through `model` into `y`, `latency()` samples late. ≤ `MAX_CALL` samples.
    pub fn process(&mut self, model: &mut NamModel, x: &[f32], y: &mut [f32]) {
        let n = x.len().min(y.len());
        let first = self.host_count;
        for (i, &v) in x[..n].iter().enumerate() {
            self.host_in.put(first + i as u64, v);
        }
        self.host_count += n as u64;
        // Every model sample whose input window is now complete.
        let avail = self.host_count as f64;
        let max = self.chunk_in.len();
        loop {
            let mut k = 0;
            while k < max {
                let pos = (self.model_count + k as u64) as f64 * self.host_per_model;
                if pos.floor() + HALF as f64 >= avail {
                    break;
                }
                self.chunk_in[k] = Self::read(&self.up, &self.host_in, pos);
                k += 1;
            }
            if k == 0 {
                break;
            }
            model.process(&self.chunk_in[..k], &mut self.chunk_out[..k]);
            for j in 0..k {
                self.model_out.put(self.model_count + j as u64, self.chunk_out[j]);
            }
            self.model_count += k as u64;
            if k < max {
                break;
            }
        }
        // The outputs, read back at the host's rate.
        let l = self.latency as u64;
        for (i, out) in y[..n].iter_mut().enumerate() {
            let t = first + i as u64;
            *out = if t < l { 0.0 } else { Self::read(&self.down, &self.model_out, (t - l) as f64 / self.host_per_model) };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn latency_covers_both_reads() {
        assert_eq!(Bracket::latency_for(48_000.0, 48_000.0), 0);
        assert_eq!(Bracket::latency_for(44_100.0, 48_000.0), 32);
        assert_eq!(Bracket::latency_for(96_000.0, 48_000.0), 49);
    }
}
