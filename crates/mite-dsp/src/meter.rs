//! Level metering.

use crate::gain::MINUS_INFINITY_DB;

/// Peak meter with instant attack and a linear-in-dB release, matching the design's meter
/// ballistics (`meter.releaseDbPerSec` = 20 in tokens.json; peak hold is drawn by the UI).
#[derive(Debug, Clone)]
pub struct PeakMeter {
    level_db: f32,
    fall_per_sample: f32,
}

impl PeakMeter {
    /// Default release (tokens.json `meter.releaseDbPerSec`).
    pub const DEFAULT_RELEASE_DB_PER_S: f32 = 20.0;

    pub fn new(sample_rate: f32) -> Self {
        let mut m = Self { level_db: MINUS_INFINITY_DB, fall_per_sample: 0.0 };
        m.set_release(sample_rate, Self::DEFAULT_RELEASE_DB_PER_S);
        m
    }

    pub fn set_release(&mut self, sample_rate: f32, db_per_second: f32) {
        self.fall_per_sample = db_per_second / sample_rate.max(1.0);
    }

    pub fn reset(&mut self) {
        self.level_db = MINUS_INFINITY_DB;
    }

    /// Feeds a block and returns the meter level in dBFS afterwards.
    #[inline]
    pub fn process(&mut self, samples: &[f32]) -> f32 {
        // Per-block peak, then per-sample release across the block: cheap and still exact for
        // attack because the peak is taken before the fall is applied.
        let mut peak = 0.0_f32;
        for &s in samples {
            peak = peak.max(s.abs());
        }
        let fallen = self.level_db - self.fall_per_sample * samples.len() as f32;
        let peak_db = crate::gain::gain_to_db(peak);
        self.level_db = peak_db.max(fallen).max(MINUS_INFINITY_DB);
        self.level_db
    }

    #[inline]
    pub fn level_db(&self) -> f32 {
        self.level_db
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attack_is_instant() {
        let mut m = PeakMeter::new(48_000.0);
        let mut block = [0.0; 64];
        block[10] = 0.5;
        let level = m.process(&block);
        assert!((level - (-6.0206)).abs() < 1e-3);
    }

    #[test]
    fn release_falls_twenty_db_per_second() {
        let sr = 48_000.0;
        let mut m = PeakMeter::new(sr);
        m.process(&[1.0]);
        let silence = [0.0; 480]; // 10 ms
        for _ in 0..100 {
            m.process(&silence);
        }
        assert!((m.level_db() - (-20.0)).abs() < 0.05, "{}", m.level_db());
    }

    #[test]
    fn floors_at_silence() {
        let mut m = PeakMeter::new(48_000.0);
        for _ in 0..1000 {
            m.process(&[0.0; 512]);
        }
        assert_eq!(m.level_db(), MINUS_INFINITY_DB);
    }
}
